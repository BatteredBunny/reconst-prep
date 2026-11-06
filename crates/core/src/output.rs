// Bounded channel to the encoder threads, so backpressure keeps memory flat.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Sender, bounded};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

use crate::frame::{Rgb8, Size};

pub const DEFAULT_JPEG_QUALITY: u8 = 95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum ImageFormat {
    /// PNG with fast compression. Lossless, big files.
    Png,
    /// JPEG, quality 1-100 (default 95).
    Jpeg { quality: u8 },
}

impl ImageFormat {
    pub fn ext(&self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg { .. } => "jpg",
        }
    }
}

/// Always a Gray8 PNG regardless of the image format: masks must be lossless.
pub struct MaskWrite {
    pub path: PathBuf,
    /// Same dimensions as the image, `0` = ignore that pixel.
    pub gray: Vec<u8>,
}

pub struct WriteJob {
    pub path: PathBuf,
    pub rgb: Rgb8,
    /// Zero or more mask sidecars, one per consumer (see `crate::mask::MaskUse`).
    pub masks: Vec<MaskWrite>,
}

/// Fast settings: these are intermediates, and the size saving does not pay for the encode time.
fn write_png(
    w: impl std::io::Write,
    data: &[u8],
    width: u32,
    height: u32,
    color: ExtendedColorType,
) -> std::result::Result<(), image::ImageError> {
    PngEncoder::new_with_quality(w, CompressionType::Fast, PngFilterType::Adaptive)
        .write_image(data, width, height, color)
}

/// Dropping a `BufWriter` flushes but throws the error away, and a frame whose
/// last bytes never reached the disk would still be counted and later trusted by `--resume`.
fn write_file(
    path: &std::path::Path,
    write: impl FnOnce(&mut BufWriter<File>) -> std::result::Result<(), image::ImageError>,
) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut w = BufWriter::new(file);
    write(&mut w).with_context(|| format!("encoding {}", path.display()))?;
    w.into_inner()
        .map_err(|e| e.into_error())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn encode(job: &WriteJob, format: ImageFormat) -> Result<()> {
    let Size { w, h } = job.rgb.size();
    let rgb = job.rgb.data();
    for mask in &job.masks {
        write_file(&mask.path, |out| {
            write_png(out, &mask.gray, w, h, ExtendedColorType::L8)
        })?;
    }
    write_file(&job.path, |out| match format {
        ImageFormat::Png => write_png(out, rgb, w, h, ExtendedColorType::Rgb8),
        ImageFormat::Jpeg { quality } => JpegEncoder::new_with_quality(out, quality.clamp(1, 100))
            .write_image(rgb, w, h, ExtendedColorType::Rgb8),
    })
}

/// Written count and first failure, behind one lock so a waiter sees both together.
#[derive(Default)]
struct Report {
    written: u64,
    /// Published as it happens: otherwise a mid-run `submit` could only report a closed queue, hiding the real disk error.
    failure: Option<String>,
}

#[derive(Default)]
struct Shared {
    report: Mutex<Report>,
    changed: Condvar,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut Report)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, Report> {
        self.report.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn check(r: &Report) -> Result<()> {
        match &r.failure {
            Some(e) => bail!("writing images failed: {e}"),
            None => Ok(()),
        }
    }
}

/// Runs as its writer unwinds: a panic records no error of its own, and a waiter must not wait on a dead thread.
struct PanicSignal(Arc<Shared>);

impl Drop for PanicSignal {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.update(|r| {
                r.failure
                    .get_or_insert_with(|| "a writer thread panicked".into());
            });
        }
    }
}

/// `submit` blocks when the queue is full; `finish` joins everything and surfaces the first error.
pub struct WriterPool {
    tx: Option<Sender<WriteJob>>,
    handles: Vec<std::thread::JoinHandle<()>>,
    shared: Arc<Shared>,
    /// Counted so [`Self::drain`] knows when the queue has caught up.
    submitted: AtomicU64,
}

impl WriterPool {
    pub fn new(format: ImageFormat, threads: usize) -> Result<Self> {
        let threads = threads.max(1);
        let (tx, rx) = bounded::<WriteJob>(threads * 2);
        let shared = Arc::new(Shared::default());
        let handles = (0..threads)
            .map(|i| {
                let (rx, shared) = (rx.clone(), shared.clone());
                std::thread::Builder::new()
                    .name(format!("img-writer-{i}"))
                    .spawn(move || {
                        let _signal = PanicSignal(shared.clone());
                        for job in rx.iter() {
                            if let Err(e) = encode(&job, format) {
                                let e = format!("{e:#}");
                                return shared.update(|r| _ = r.failure.get_or_insert(e));
                            }
                            shared.update(|r| r.written += 1);
                        }
                    })
                    .context("spawning writer thread")
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            tx: Some(tx),
            handles,
            shared,
            submitted: AtomicU64::new(0),
        })
    }

    pub fn submit(&self, job: WriteJob) -> Result<()> {
        let tx = self.tx.as_ref().expect("pool already finished");
        let sent = tx.send(job).is_ok();
        if sent {
            self.submitted.fetch_add(1, Ordering::Relaxed);
        }
        // The send succeeding is not evidence of health: a worker that died still leaves the others receiving.
        Shared::check(&self.shared.lock())?;
        anyhow::ensure!(sent, "writer pool shut down early");
        Ok(())
    }

    /// Block until everything submitted so far is on disk. A clip recorded in
    /// the manifest while its images are still queued is one `--resume` will
    /// skip, so the record must never run ahead of the files.
    pub fn drain(&self) -> Result<()> {
        let submitted = self.submitted.load(Ordering::Relaxed);
        let mut report = self.shared.lock();
        loop {
            Shared::check(&report)?;
            if report.written >= submitted {
                return Ok(());
            }
            report = self
                .shared
                .changed
                .wait(report)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn written(&self) -> u64 {
        self.shared.lock().written
    }

    pub fn finish(mut self) -> Result<u64> {
        self.close();
        Shared::check(&self.shared.lock())?;
        Ok(self.written())
    }

    fn close(&mut self) {
        drop(self.tx.take());
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

impl Drop for WriterPool {
    /// A cancelled or failed run drops the pool instead of calling `finish`;
    /// without this the process can exit with encoders still mid-write.
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(path: PathBuf) -> WriteJob {
        WriteJob {
            path,
            rgb: Rgb8::new(Size::new(4, 2), vec![128; 4 * 2 * 3]).unwrap(),
            masks: vec![],
        }
    }

    #[test]
    fn drain_returns_once_everything_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let pool = WriterPool::new(ImageFormat::Png, 3).unwrap();
        for i in 0..20 {
            pool.submit(job(dir.path().join(format!("{i}.png"))))
                .unwrap();
        }
        pool.drain().unwrap();
        assert_eq!(pool.written(), 20);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 20);
        assert_eq!(pool.finish().unwrap(), 20);
    }

    #[test]
    fn a_write_error_reaches_drain() {
        let dir = tempfile::tempdir().unwrap();
        let pool = WriterPool::new(ImageFormat::Png, 1).unwrap();
        pool.submit(job(dir.path().join("missing-dir/0.png")))
            .unwrap();
        let e = pool.drain().expect_err("the directory does not exist");
        assert!(
            e.to_string().contains("writing images failed"),
            "unhelpful: {e}"
        );
    }
}

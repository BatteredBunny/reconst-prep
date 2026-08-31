// Bounded channel to the encoder threads, so backpressure keeps memory flat.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Sender, bounded};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

/// Default JPEG quality. Shared by the CLI flag and the GUI setting.
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
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<u8>,
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

fn encode(job: &WriteJob, format: ImageFormat) -> Result<()> {
    for mask in &job.masks {
        let file = File::create(&mask.path)
            .with_context(|| format!("creating {}", mask.path.display()))?;
        write_png(
            BufWriter::new(file),
            &mask.gray,
            job.w,
            job.h,
            ExtendedColorType::L8,
        )
        .with_context(|| format!("encoding {}", mask.path.display()))?;
    }
    let file =
        File::create(&job.path).with_context(|| format!("creating {}", job.path.display()))?;
    let mut w = BufWriter::new(file);
    match format {
        ImageFormat::Png => {
            write_png(&mut w, &job.rgb, job.w, job.h, ExtendedColorType::Rgb8)
                .with_context(|| format!("encoding {}", job.path.display()))?;
        }
        ImageFormat::Jpeg { quality } => {
            JpegEncoder::new_with_quality(&mut w, quality.clamp(1, 100))
                .write_image(&job.rgb, job.w, job.h, ExtendedColorType::Rgb8)
                .with_context(|| format!("encoding {}", job.path.display()))?;
        }
    }
    Ok(())
}

/// `submit` blocks when the queue is full; `finish` joins everything and surfaces the first error.
pub struct WriterPool {
    tx: Option<Sender<WriteJob>>,
    handles: Vec<std::thread::JoinHandle<Result<()>>>,
    written: Arc<AtomicU64>,
    /// Counted so [`Self::drain`] knows when the queue has caught up.
    submitted: AtomicU64,
    /// Published as it happens: otherwise a mid-run `submit` could only report a closed queue, hiding the real disk error.
    first_err: Arc<std::sync::Mutex<Option<String>>>,
}

impl WriterPool {
    pub fn new(format: ImageFormat, threads: usize) -> Result<Self> {
        let threads = threads.max(1);
        let (tx, rx) = bounded::<WriteJob>(threads * 2);
        let written = Arc::new(AtomicU64::new(0));
        let first_err: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let mut handles = Vec::with_capacity(threads);
        for i in 0..threads {
            let rx = rx.clone();
            let written = written.clone();
            let first_err = first_err.clone();
            handles.push(
                std::thread::Builder::new()
                    .name(format!("img-writer-{i}"))
                    .spawn(move || -> Result<()> {
                        for job in rx.iter() {
                            if let Err(e) = encode(&job, format) {
                                let mut slot = first_err.lock().unwrap_or_else(|p| p.into_inner());
                                slot.get_or_insert_with(|| format!("{e:#}"));
                                return Err(e);
                            }
                            written.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(())
                    })
                    .context("spawning writer thread")?,
            );
        }
        Ok(Self {
            tx: Some(tx),
            handles,
            written,
            submitted: AtomicU64::new(0),
            first_err,
        })
    }

    /// The first encode failure, if one has happened.
    fn encode_error(&self) -> Option<String> {
        self.first_err
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn submit(&self, job: WriteJob) -> Result<()> {
        let send = self
            .tx
            .as_ref()
            .expect("pool already finished")
            .send(job)
            .is_ok();
        if send {
            self.submitted.fetch_add(1, Ordering::Relaxed);
        }
        // The send succeeding is not evidence of health: a worker that died still leaves the others receiving.
        match self.encode_error() {
            Some(e) => bail!("writing images failed: {e}"),
            None if send => Ok(()),
            None => bail!("writer pool shut down early"),
        }
    }

    /// Block until everything submitted so far is on disk. A clip recorded in
    /// the manifest while its images are still queued is one `--resume` will
    /// skip, so the record must never run ahead of the files.
    pub fn drain(&self) -> Result<()> {
        // A worker that panics stops counting without recording an error, so
        // the wait is bounded rather than trusting the count to converge.
        const STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);
        let mut last_progress = std::time::Instant::now();
        let mut seen = self.written();
        while self.written() < self.submitted.load(Ordering::Relaxed) {
            if let Some(e) = self.encode_error() {
                bail!("writing images failed: {e}");
            }
            let now = self.written();
            if now != seen {
                seen = now;
                last_progress = std::time::Instant::now();
            } else if last_progress.elapsed() > STALL_LIMIT {
                bail!(
                    "image writing stalled with {} jobs outstanding",
                    self.submitted.load(Ordering::Relaxed) - now
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        match self.encode_error() {
            Some(e) => bail!("writing images failed: {e}"),
            None => Ok(()),
        }
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// Close the queue, join all writers, return total frames written.
    pub fn finish(mut self) -> Result<u64> {
        drop(self.tx.take()); // workers drain and exit
        let mut first_err = None;
        for h in self.handles.drain(..) {
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    first_err.get_or_insert(e);
                }
                Err(_) => {
                    first_err.get_or_insert(anyhow::anyhow!("writer thread panicked"));
                }
            }
        }
        if let Some(e) = first_err {
            bail!(e);
        }
        Ok(self.written.load(Ordering::Relaxed))
    }
}

impl Drop for WriterPool {
    /// A cancelled or failed run drops the pool instead of calling `finish`;
    /// without this the process can exit with encoders still mid-write.
    fn drop(&mut self) {
        drop(self.tx.take());
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

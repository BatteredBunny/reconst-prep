use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::cancel::{CancelToken, Cancelled};
use crate::decode::{ClipInfo, Decoder, Ffmpeg, RawFrame};
use crate::frame::{Rgb8, Size};
use crate::gray::{GrayFrame, metric_rgb, resize_rgb, rgb_to_gray};
use crate::mask::{Mask, MaskSet, PreparedMask};
use crate::output::{MaskWrite, WriteJob, WriterPool};
use crate::seg::SegLabels;
use crate::select::{Decision, FrameInfo, Selector, make_selector};
use crate::undistort::{LensOnlyParams, LensProfileFile, Undistorter};

use super::{PipelineConfig, Progress};

/// gyroflow's own rayon parallelism does not reach the decode rate; overlapping a few frames does (measured 13.7 -> ~20 fps).
const DEFAULT_UNDISTORT_THREADS: usize = 3;

/// Cores left to the feeder, the serial stage, the writer pool and ffmpeg itself.
const RESERVED_CORES: usize = 4;

/// Inference runs in these same workers and dwarfs the kernel (~2.9 s/frame against ~50 ms), so here the worker count *is* the throughput.
fn seg_workers() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    cores
        .saturating_sub(RESERVED_CORES)
        .max(DEFAULT_UNDISTORT_THREADS)
}

/// Where each mask sidecar set is written; created up front.
pub(super) struct MaskDirs {
    pub sfm: PathBuf,
    /// `None` when no active class is valid for trainer supervision.
    pub training: Option<PathBuf>,
}

/// The parts of a run that do not change from clip to clip.
pub(super) struct ClipRun<'a> {
    pub cfg: &'a PipelineConfig,
    pub ff: &'a Ffmpeg,
    pub writers: &'a WriterPool,
    pub mask_dirs: MaskDirs,
    pub n_clips: usize,
    pub t0: Instant,
    pub cancel: &'a CancelToken,
    /// `None` disables undistortion for the whole run.
    pub lens: Option<&'a LensProfileFile>,
    /// Settled once in preflight, so the manifest records what every clip was asked to run on.
    pub gpu: Option<usize>,
}

/// Totals across all clips so far; progress reports are run-wide.
#[derive(Default)]
pub(super) struct Totals {
    pub decoded: u64,
    pub kept: u64,
    /// The share of the above that `--resume` skipped rather than did. Held
    /// here so progress can report the dataset and the rate can report the run.
    pub carried_decoded: u64,
    pub carried_written: u64,
    pub carried_clips: usize,
}

/// Masks stay at metric resolution until the frame is kept.
struct PendingFrame {
    rgb: Rgb8,
    mask: Option<MaskSet>,
}

/// Masking and sharpness are absent on purpose: both can depend on neighbouring frames, so the serial stage computes them.
struct Processed {
    index: u64,
    rgb: Rgb8,
    /// Metric-resolution RGB thumbnail; the mask sources run on it.
    small: Rgb8,
    gray: GrayFrame,
    seg: Option<SegLabels>,
}

/// The parts of a run that *do* change from clip to clip.
pub(super) struct ClipJob<'a> {
    pub idx: usize,
    pub info: &'a ClipInfo,
    /// The exact frames to keep. Replay mode only.
    pub replay: Option<&'a [u64]>,
}

/// Where the stage reads frames from. A trait only so tests can drive the stage without ffmpeg.
pub(super) trait FrameSource: Sync {
    fn frames(&self) -> impl Iterator<Item = Result<RawFrame>> + '_;
    fn cancel(&self);
}

impl FrameSource for Decoder {
    fn frames(&self) -> impl Iterator<Item = Result<RawFrame>> + '_ {
        Decoder::frames(self)
    }
    fn cancel(&self) {
        Decoder::cancel(self)
    }
}

impl ClipRun<'_> {
    pub(super) fn process(
        &self,
        job: ClipJob<'_>,
        totals: &mut Totals,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<super::ManifestClip> {
        let clip = job.info;
        let cfg = self.cfg;
        let out = cfg.output_size.resolve(clip.size)?;
        let params = self.lens.map(|l| l.params(clip.size, out)).transpose()?;

        // Opened per clip: the model binds to one input geometry and clips may differ in aspect.
        let mask = cfg.mask.prepare(out)?;

        let mut selector = make_selector(&cfg.selection, job.replay)?;
        let decoder = Decoder::spawn(self.ff, &clip.path, &cfg.hwaccel)?;
        let stage = Stage {
            run: self,
            clip_idx: job.idx,
            clip,
            out,
            params: params.as_ref(),
            mask: &mask,
        };
        let outcome = stage.run(&decoder, &mut *selector, totals, progress)?;

        if outcome.decoded == 0 {
            bail!("{}: ffmpeg produced no frames", clip.path.display());
        }
        totals.decoded += outcome.decoded;
        totals.kept += outcome.kept.len() as u64;

        Ok(super::ManifestClip {
            path: clip.path.display().to_string(),
            stem: clip.stem.clone(),
            width: clip.size.w,
            height: clip.size.h,
            fps: clip.fps,
            frames_probed: clip.frames,
            frames_decoded: outcome.decoded,
            frames_kept: outcome.kept.len() as u64,
            out_width: out.w,
            out_height: out.h,
            pinhole_intrinsics: params.as_ref().map(LensOnlyParams::output_intrinsics),
            kept_frames: outcome.kept,
        })
    }
}

/// The concurrent stage of one clip, and everything fixed for the whole clip.
struct Stage<'a> {
    run: &'a ClipRun<'a>,
    clip_idx: usize,
    clip: &'a ClipInfo,
    out: Size,
    params: Option<&'a LensOnlyParams>,
    mask: &'a PreparedMask,
}

struct StageOutcome {
    decoded: u64,
    kept: Vec<u64>,
}

/// The serial half of the stage. Workers finish out of order, but masking can
/// depend on neighbouring frames and selection on all of them, so everything
/// order-dependent is held here and driven from one thread.
#[derive(Default)]
struct Serial {
    pending: HashMap<u64, PendingFrame>,
    reorder: BTreeMap<u64, Processed>,
    next_emit: u64,
    decoded: u64,
    kept: Vec<u64>,
}

impl Stage<'_> {
    fn run(
        &self,
        source: &impl FrameSource,
        selector: &mut dyn Selector,
        totals: &Totals,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<StageOutcome> {
        let cfg = self.run.cfg;
        let workers = super::threads(cfg.undistort_threads, || {
            if self.mask.runs_inference() {
                seg_workers()
            } else {
                DEFAULT_UNDISTORT_THREADS
            }
        });
        let mut serial = Serial::default();

        std::thread::scope(|s| -> Result<()> {
            // Built in here so an early return drops the receivers, which is what frees a worker blocked on a full `ptx`.
            let (ftx, frx) = crossbeam_channel::bounded::<RawFrame>(2);
            // One slot per worker plus slack: deeper queueing adds no overlap, and a seg run's ~20 workers each hold a whole output frame.
            let (ptx, prx) = crossbeam_channel::bounded::<Result<Processed>>(workers + 2);
            {
                let (clip, cancel) = (self.clip, self.run.cancel);
                let feeder_ptx = ptx.clone();
                s.spawn(move || feed(source, clip, cancel, &ftx, &feeder_ptx));
            }
            for _ in 0..workers {
                let frx = frx.clone();
                let ptx = ptx.clone();
                s.spawn(move || {
                    let mut undistorter = self
                        .params
                        .map(|p| Undistorter::new(p, cfg.interp, self.run.gpu));
                    for frame in frx.iter() {
                        let res = self.process_frame(frame, undistorter.as_mut());
                        let failed = res.is_err();
                        if ptx.send(res).is_err() || failed {
                            break;
                        }
                    }
                });
            }
            drop(frx);
            drop(ptx);

            let outcome = self.consume(&mut serial, &prx, selector, totals, progress);
            // Nothing else drains this queue, so an early exit strands every worker on a send.
            if outcome.is_err() {
                source.cancel();
                for _ in prx.iter() {}
            }
            outcome
        })?;

        // A cancelled decode ends quietly: flushing the selector would still write the last window,
        // and recording a truncated clip is one `--resume` skips forever.
        self.run.cancel.check()?;
        let decisions = selector.finish()?;
        self.emit(decisions, &mut serial)?;
        Ok(StageOutcome {
            decoded: serial.decoded,
            kept: serial.kept,
        })
    }

    /// Take processed frames as they land and put them back into decode order,
    /// which is the order masking and selection have to see.
    fn consume(
        &self,
        serial: &mut Serial,
        prx: &crossbeam_channel::Receiver<Result<Processed>>,
        selector: &mut dyn Selector,
        totals: &Totals,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        let mut smoother = self.mask.smoother();
        for processed in prx.iter() {
            if self.run.cancel.is_cancelled() {
                return Err(Cancelled.into());
            }
            let p = processed?;
            serial.reorder.insert(p.index, p);
            while let Some(p) = serial.reorder.remove(&serial.next_emit) {
                serial.next_emit += 1;

                // In decode order, so neither masking nor selection can depend on which worker won a race.
                let mut gray = p.gray;
                let seg = p.seg.map(|labels| smoother.push(labels));
                let judged = self.mask.judge(&p.small, &mut gray, seg.as_ref());

                serial.pending.insert(
                    p.index,
                    PendingFrame {
                        rgb: p.rgb,
                        mask: judged.mask,
                    },
                );
                let decisions = selector.push(FrameInfo {
                    index: p.index,
                    sharpness: judged.sharpness,
                    gray,
                });
                self.emit(decisions, serial)?;

                serial.decoded += 1;
                progress(self.progress(totals, serial.decoded, serial.kept.len()));
            }
        }
        anyhow::ensure!(
            serial.reorder.is_empty(),
            "{}: undistort pipeline lost frames (gap at index {})",
            self.clip.path.display(),
            serial.next_emit
        );
        Ok(())
    }

    /// One frame's work off the serial path: output pixels, then the metric thumbnail.
    fn process_frame(
        &self,
        frame: RawFrame,
        undistorter: Option<&mut Undistorter>,
    ) -> Result<Processed> {
        let rgb = render_output(frame.rgb, self.out, undistorter)?;
        let a = analyze_frame(&rgb, self.mask)?;
        Ok(Processed {
            index: frame.index,
            rgb,
            small: a.small,
            gray: a.gray,
            seg: a.seg,
        })
    }

    /// The only part that knows about file names, mask scaling and the encoder pool.
    fn emit(&self, decisions: Vec<Decision>, serial: &mut Serial) -> Result<()> {
        let cfg = self.run.cfg;
        for d in decisions {
            let frame = serial
                .pending
                .remove(&d.index)
                .context("selector decided on an unknown frame")?;
            if !d.keep {
                continue;
            }
            serial.kept.push(d.index);
            let name = super::manifest::image_name(&self.clip.stem, d.index, cfg.format);
            let masks = match frame.mask {
                // One sidecar per consumer, scaled to output size only now.
                Some(set) => {
                    let sidecar = |dir: &std::path::Path, m: &Mask| MaskWrite {
                        path: dir.join(format!("{name}.png")),
                        gray: m.scaled_to(self.out).data,
                    };
                    let mut out = vec![sidecar(&self.run.mask_dirs.sfm, &set.sfm)];
                    if let (Some(dir), Some(m)) = (&self.run.mask_dirs.training, &set.training) {
                        out.push(sidecar(dir, m));
                    }
                    out
                }
                None => vec![],
            };
            self.run.writers.submit(WriteJob {
                path: cfg.out_dir.join(&name),
                rgb: frame.rgb,
                masks,
            })?;
        }
        Ok(())
    }

    /// Run-wide, so it needs the finished clips' totals as well as this clip's.
    fn progress(&self, totals: &Totals, decoded: u64, kept: usize) -> Progress {
        Progress {
            clip_idx: self.clip_idx,
            n_clips: self.run.n_clips,
            clip_name: self.clip.stem.clone(),
            clip_total_frames: self.clip.frames,
            clip_decoded: decoded,
            total_decoded: totals.decoded + decoded,
            total_kept: totals.kept + kept as u64,
            total_written: totals.carried_written + self.run.writers.written(),
            decoded_this_run: totals.decoded + decoded - totals.carried_decoded,
            elapsed_s: self.run.t0.elapsed().as_secs_f64(),
        }
    }
}

/// A mid-clip frame size change would silently corrupt the undistort geometry.
fn feed(
    source: &impl FrameSource,
    clip: &ClipInfo,
    cancel: &CancelToken,
    ftx: &crossbeam_channel::Sender<RawFrame>,
    err_tx: &crossbeam_channel::Sender<Result<Processed>>,
) {
    for frame in source.frames() {
        if cancel.is_cancelled() {
            source.cancel();
            break;
        }
        let checked = frame.and_then(|f| {
            if f.rgb.size() != clip.size {
                bail!(
                    "{}: frame size {} does not match probed {}",
                    clip.path.display(),
                    f.rgb.size(),
                    clip.size
                );
            }
            Ok(f)
        });
        match checked {
            Ok(f) => {
                if ftx.send(f).is_err() {
                    break; // workers gone
                }
            }
            Err(e) => {
                let _ = err_tx.send(Err(e));
                break;
            }
        }
    }
}

/// The undistort kernel renders straight at the output size, fusing the downscale.
pub fn render_output(
    mut frame: Rgb8,
    out: Size,
    undistorter: Option<&mut Undistorter>,
) -> Result<Rgb8> {
    match undistorter {
        Some(u) => u.process_rgb(&mut frame),
        None if frame.size() == out => Ok(frame),
        None => resize_rgb(&frame, out),
    }
}

/// The per-frame inputs masking and the selection metrics both run on.
pub struct FrameAnalysis {
    /// Metric-resolution RGB thumbnail, the same size as `gray`.
    pub small: Rgb8,
    pub gray: GrayFrame,
    pub seg: Option<SegLabels>,
}

/// Inference is the expensive part and is stateless, so it belongs off the serial path.
pub fn analyze_frame(rgb: &Rgb8, mask: &PreparedMask) -> Result<FrameAnalysis> {
    // The thumbnail outlives `rgb`, so `metric_rgb`'s borrowed case must be copied here.
    let small = metric_rgb(rgb)?.into_owned();
    let gray = rgb_to_gray(&small);
    // Inference reads the output frame: a person a few dozen pixels tall in the thumbnail is not recoverable by upsampling.
    let seg = mask.segment(rgb)?;
    Ok(FrameAnalysis { small, gray, seg })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::HwAccel;
    use crate::mask::MaskConfig;
    use crate::output::ImageFormat;
    use crate::select::{SelectionConfig, SelectionMode};
    use crate::undistort::Interp;

    const SIZE: Size = Size::new(32, 24);

    /// Synthetic frames by index and size, so a test can drop one or resize one.
    struct Frames(Vec<(u64, Size)>);

    fn frames(n: u64) -> Frames {
        Frames((0..n).map(|i| (i, SIZE)).collect())
    }

    impl FrameSource for Frames {
        fn frames(&self) -> impl Iterator<Item = Result<RawFrame>> + '_ {
            self.0.iter().map(|&(index, size)| {
                let rgb = Rgb8::new(size, vec![index as u8; size.px() * 3])?;
                Ok(RawFrame { index, rgb })
            })
        }
        fn cancel(&self) {}
    }

    fn run_stage(
        source: &Frames,
        cancel: &CancelToken,
    ) -> (Result<StageOutcome>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = PipelineConfig {
            inputs: vec![],
            profile_path: None,
            out_dir: dir.path().to_path_buf(),
            output_size: super::super::OutputSpec::Same,
            format: ImageFormat::Png,
            selection: SelectionConfig {
                mode: SelectionMode::EveryNth { n: 1 },
                blur_floor: None,
            },
            hwaccel: HwAccel::Auto,
            ffmpeg_path: None,
            interp: Interp::Bilinear,
            gpu: false,
            mask: MaskConfig::default(),
            writer_threads: 1,
            // Several workers, so frames really do finish out of order.
            undistort_threads: 4,
            resume: false,
            frames_from: None,
        };
        let ff = Ffmpeg {
            ffmpeg: "ffmpeg".into(),
            ffprobe: None,
            version: String::new(),
        };
        let writers = WriterPool::new(cfg.format, 1).unwrap();
        let run = ClipRun {
            cfg: &cfg,
            ff: &ff,
            writers: &writers,
            mask_dirs: MaskDirs {
                sfm: dir.path().join("masks"),
                training: None,
            },
            n_clips: 1,
            t0: Instant::now(),
            cancel,
            lens: None,
            gpu: None,
        };
        let clip = ClipInfo {
            path: "synthetic.mkv".into(),
            stem: "synthetic".into(),
            size: SIZE,
            fps: 30.0,
            frames: None,
            codec: None,
            duration_s: None,
        };
        let mask = cfg.mask.prepare(SIZE).unwrap();
        let stage = Stage {
            run: &run,
            clip_idx: 0,
            clip: &clip,
            out: SIZE,
            params: None,
            mask: &mask,
        };
        let mut selector = make_selector(&cfg.selection, None).unwrap();
        let outcome = stage.run(source, &mut *selector, &Totals::default(), &mut |_| {});
        writers.finish().unwrap();
        (outcome, dir)
    }

    fn stage_error(source: &Frames) -> String {
        let (outcome, _dir) = run_stage(source, &CancelToken::new());
        outcome.err().expect("the stage must fail").to_string()
    }

    #[test]
    fn every_frame_comes_out_in_decode_order() {
        let (outcome, dir) = run_stage(&frames(40), &CancelToken::new());
        let outcome = outcome.unwrap();
        assert_eq!(outcome.decoded, 40);
        assert_eq!(outcome.kept, (0..40).collect::<Vec<_>>());
        assert!(dir.path().join("synthetic_000039.png").is_file());
    }

    #[test]
    fn a_missing_frame_is_reported_as_a_gap() {
        let mut source = frames(10);
        source.0.remove(4);
        assert!(stage_error(&source).contains("gap at index 4"));
    }

    #[test]
    fn a_frame_of_the_wrong_size_fails_the_clip() {
        let mut source = frames(10);
        source.0[3].1 = Size::new(64, 24);
        assert!(stage_error(&source).contains("does not match probed"));
    }

    #[test]
    fn a_cancelled_stage_writes_nothing_and_says_so() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let (outcome, dir) = run_stage(&frames(10), &cancel);
        assert!(crate::cancel::is_cancelled(&outcome.err().unwrap()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

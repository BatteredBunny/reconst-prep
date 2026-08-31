use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::cancel::{CancelToken, Cancelled};
use crate::decode::{ClipInfo, Decoder, Ffmpeg, RawFrame};
use crate::gray::{GrayFrame, laplacian_variance, metric_rgb, resize_rgb, rgb_to_gray};
use crate::mask::{Mask, MaskConfig, MaskSet};
use crate::output::{MaskWrite, WriteJob, WriterPool};
use crate::seg::SegLabels;
use crate::select::{ClipSelection, Decision, FrameInfo, Selector, make_selector};
use crate::undistort::{LensOnlyParams, Undistorter};

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

/// What one clip contributed: its manifest record, built here because this is
/// where every input to it already is, plus the one fact the record cannot hold.
pub(super) struct ClipDone {
    pub record: super::ManifestClip,
    /// Whether the wgpu kernel actually rendered, which is not the same as
    /// having asked for it: gyroflow falls back to the CPU on its own.
    pub gpu_used: bool,
}

/// Masks stay at metric resolution until the frame is kept.
struct PendingFrame {
    rgb: Vec<u8>,
    mask: Option<MaskSet>,
}

/// Masking and sharpness are absent on purpose: both can depend on neighbouring frames, so the serial stage computes them.
struct Processed {
    index: u64,
    rgb: Vec<u8>,
    /// Metric-resolution RGB thumbnail; the mask sources run on it.
    small: Vec<u8>,
    gray: GrayFrame,
    seg: Option<SegLabels>,
}

/// The parts of a run that *do* change from clip to clip.
pub(super) struct ClipJob<'a> {
    pub idx: usize,
    pub info: &'a ClipInfo,
    /// `None` disables undistortion for the whole run.
    pub profile_json: Option<&'a str>,
    /// The exact frames to keep. Replay mode only.
    pub replay: Option<&'a [u64]>,
}

impl ClipRun<'_> {
    pub(super) fn process(
        &self,
        job: ClipJob<'_>,
        totals: &mut Totals,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<ClipDone> {
        let ClipJob {
            idx: clip_idx,
            info: clip,
            profile_json,
            replay,
        } = job;
        let cfg = self.cfg;
        let (out_w, out_h) = cfg.output_size.resolve(clip.width, clip.height)?;
        let params = profile_json
            .map(|json| {
                LensOnlyParams::new(
                    json,
                    clip.width as usize,
                    clip.height as usize,
                    out_w,
                    out_h,
                )
            })
            .transpose()?;
        let intrinsics = params.as_ref().map(|p| p.output_intrinsics()).transpose()?;

        // Opened per clip: the model binds to one input geometry and clips may differ in aspect.
        let mask_cfg = cfg.mask.prepare(out_w as u32, out_h as u32)?;

        let mut selector = make_selector(&cfg.selection, ClipSelection { replay })?;
        let decoder = Decoder::spawn(self.ff, &clip.path, &cfg.hwaccel)?;
        let stage = Stage {
            run: self,
            clip_idx,
            clip,
            out: (out_w, out_h),
            params: &params,
            mask_cfg: &mask_cfg,
            emitter: Emitter {
                run: self,
                clip,
                out_w,
                out_h,
            },
        };
        let (decoded, kept_frames, gpu_used) =
            stage.run(&decoder, &mut *selector, totals, progress)?;
        // A cancelled decode ends the frame loop quietly, and recording a truncated clip is one `--resume` skips forever.
        self.cancel.check()?;

        if decoded == 0 {
            bail!("{}: ffmpeg produced no frames", clip.path.display());
        }
        totals.decoded += decoded;
        totals.kept += kept_frames.len() as u64;

        Ok(ClipDone {
            record: super::ManifestClip {
                path: clip.path.display().to_string(),
                stem: clip.stem.clone(),
                width: clip.width,
                height: clip.height,
                fps: clip.fps,
                frames_probed: clip.frames,
                frames_decoded: decoded,
                frames_kept: kept_frames.len() as u64,
                out_width: out_w,
                out_height: out_h,
                pinhole_intrinsics: intrinsics,
                kept_frames,
            },
            gpu_used,
        })
    }
}

/// The concurrent stage of one clip, and everything fixed for the whole clip.
struct Stage<'a> {
    run: &'a ClipRun<'a>,
    clip_idx: usize,
    clip: &'a ClipInfo,
    out: (usize, usize),
    params: &'a Option<LensOnlyParams>,
    mask_cfg: &'a MaskConfig,
    emitter: Emitter<'a>,
}

fn worker_count(cfg: &PipelineConfig, mask_cfg: &MaskConfig) -> usize {
    super::threads(cfg.undistort_threads, || {
        if mask_cfg.model.is_some() {
            seg_workers()
        } else {
            DEFAULT_UNDISTORT_THREADS
        }
    })
}

/// Which kernel the workers build, and what actually happened.
struct GpuChoice {
    /// `None` when the GPU was not asked for, or no device was listed.
    device: Option<usize>,
    /// Settled by the first worker to need it; the rest wait rather than each
    /// repeating gyroflow's error report.
    verdict: std::sync::OnceLock<bool>,
}

impl GpuChoice {
    fn pick(cfg: &PipelineConfig, params: &Option<LensOnlyParams>) -> Self {
        let device = (cfg.gpu && params.is_some())
            .then(|| {
                let devices = crate::undistort::gpu_devices();
                match devices.first() {
                    Some(name) => {
                        log::info!("GPU undistortion on {name}");
                        Some(0usize)
                    }
                    None => {
                        log::warn!("GPU undistortion: no device found; using the CPU kernel");
                        None
                    }
                }
            })
            .flatten();
        Self {
            device,
            verdict: std::sync::OnceLock::new(),
        }
    }

    fn undistorter(
        &self,
        params: &LensOnlyParams,
        interp: crate::undistort::Interp,
    ) -> Undistorter {
        let gpu = self.device.filter(|&d| {
            *self
                .verdict
                .get_or_init(|| crate::undistort::gpu_usable(params, interp, d))
        });
        match gpu {
            Some(d) => Undistorter::new_gpu(params, interp, d),
            None => Undistorter::new(params, interp),
        }
    }

    /// An unset verdict means no worker ever asked, so nothing rendered on it.
    fn was_used(&self) -> bool {
        self.device.is_some() && self.verdict.get().copied().unwrap_or(false)
    }
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
    /// Returns the frames decoded, the indices kept, and whether the GPU kernel ran.
    fn run(
        &self,
        decoder: &Decoder,
        selector: &mut dyn Selector,
        totals: &Totals,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(u64, Vec<u64>, bool)> {
        let cfg = self.run.cfg;
        let (clip, mask_cfg) = (self.clip, self.mask_cfg);
        let workers = worker_count(cfg, mask_cfg);
        let gpu = GpuChoice::pick(cfg, self.params);
        let mut serial = Serial::default();

        std::thread::scope(|s| -> Result<()> {
            // Built in here so an early return drops the receivers, which is what frees a worker blocked on a full `ptx`.
            let (ftx, frx) = crossbeam_channel::bounded::<RawFrame>(2);
            // One slot per worker plus slack: deeper queueing adds no overlap, and a seg run's ~20 workers each hold a whole output frame.
            let (ptx, prx) = crossbeam_channel::bounded::<Result<Processed>>(workers + 2);
            {
                let cancel = self.run.cancel;
                let feeder_ptx = ptx.clone();
                s.spawn(move || feed(decoder, clip, cancel, &ftx, &feeder_ptx));
            }
            let gpu = &gpu;
            for _ in 0..workers {
                let frx = frx.clone();
                let ptx = ptx.clone();
                let params = self.params;
                let out = self.out;
                s.spawn(move || {
                    let mut undistorter = params.as_ref().map(|p| gpu.undistorter(p, cfg.interp));
                    for mut frame in frx.iter() {
                        let res =
                            process_frame(&mut frame, undistorter.as_mut(), clip, out, mask_cfg);
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
                decoder.cancel();
                for _ in prx.iter() {}
            }
            outcome
        })?;

        let decisions = selector.finish()?;
        self.emitter
            .emit(decisions, &mut serial.pending, &mut serial.kept)?;
        Ok((serial.decoded, serial.kept, gpu.was_used()))
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
        let mut smoother = self.mask_cfg.smoother();
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
                let (mask, sharpness) =
                    mask_and_sharpness(self.mask_cfg, &p.small, &mut gray, seg.as_ref());

                serial
                    .pending
                    .insert(p.index, PendingFrame { rgb: p.rgb, mask });
                let decisions = selector.push(FrameInfo {
                    index: p.index,
                    sharpness,
                    gray,
                });
                self.emitter
                    .emit(decisions, &mut serial.pending, &mut serial.kept)?;

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
    decoder: &Decoder,
    clip: &ClipInfo,
    cancel: &CancelToken,
    ftx: &crossbeam_channel::Sender<RawFrame>,
    err_tx: &crossbeam_channel::Sender<Result<Processed>>,
) {
    for frame in decoder.frames() {
        if cancel.is_cancelled() {
            decoder.cancel();
            break;
        }
        let checked = frame.and_then(|f| {
            if (f.width, f.height) != (clip.width, clip.height) {
                bail!(
                    "{}: frame size {}x{} does not match probed {}x{}",
                    clip.path.display(),
                    f.width,
                    f.height,
                    clip.width,
                    clip.height
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
    rgb: &mut Vec<u8>,
    (in_w, in_h): (u32, u32),
    (out_w, out_h): (usize, usize),
    undistorter: Option<&mut Undistorter>,
) -> Result<Vec<u8>> {
    match undistorter {
        Some(u) => {
            let mut out = vec![0u8; out_w * out_h * 3];
            u.process_rgb(rgb, &mut out)?;
            Ok(out)
        }
        None if (out_w, out_h) == (in_w as usize, in_h as usize) => Ok(std::mem::take(rgb)),
        None => resize_rgb(rgb, in_w, in_h, out_w as u32, out_h as u32),
    }
}

/// The per-frame inputs masking and the selection metrics both run on.
pub struct FrameAnalysis {
    /// Metric-resolution RGB thumbnail; `gray` carries its dimensions.
    pub small: Vec<u8>,
    pub gray: GrayFrame,
    pub seg: Option<SegLabels>,
}

/// Inference is the expensive part and is stateless, so it belongs off the serial path.
pub fn analyze_frame(rgb: &[u8], w: u32, h: u32, mask_cfg: &MaskConfig) -> Result<FrameAnalysis> {
    let (small, mw, mh) = metric_rgb(rgb, w, h)?;
    // The thumbnail outlives `rgb`, so `metric_rgb`'s borrowed case must be copied here.
    let small = small.into_owned();
    let gray = rgb_to_gray(&small, mw, mh);
    // Inference reads the output frame: a person a few dozen pixels tall in the thumbnail is not recoverable by upsampling.
    let seg = mask_cfg.segment(rgb, w, h)?;
    Ok(FrameAnalysis { small, gray, seg })
}

/// The metric sees the SfM set: sharpness and motion are selection questions, not supervision ones, so the sky belongs in it.
pub fn mask_and_sharpness(
    mask_cfg: &MaskConfig,
    small: &[u8],
    gray: &mut GrayFrame,
    seg: Option<&SegLabels>,
) -> (Option<MaskSet>, f64) {
    let mask = mask_cfg
        .is_active()
        .then(|| mask_cfg.compute(small, gray, seg));
    if let Some(m) = &mask {
        gray.valid = Some(m.sfm.data.clone());
    }
    (mask, laplacian_variance(gray))
}

/// One frame's work off the serial path: output pixels, then the metric thumbnail.
fn process_frame(
    frame: &mut RawFrame,
    undistorter: Option<&mut Undistorter>,
    clip: &ClipInfo,
    out: (usize, usize),
    mask_cfg: &MaskConfig,
) -> Result<Processed> {
    let rgb = render_output(&mut frame.rgb, (clip.width, clip.height), out, undistorter)?;
    let a = analyze_frame(&rgb, out.0 as u32, out.1 as u32, mask_cfg)?;
    Ok(Processed {
        index: frame.index,
        rgb,
        small: a.small,
        gray: a.gray,
        seg: a.seg,
    })
}

/// The only part that knows about file names, mask scaling and the encoder pool.
struct Emitter<'a> {
    run: &'a ClipRun<'a>,
    clip: &'a ClipInfo,
    out_w: usize,
    out_h: usize,
}

impl Emitter<'_> {
    fn emit(
        &self,
        decisions: Vec<Decision>,
        pending: &mut HashMap<u64, PendingFrame>,
        kept: &mut Vec<u64>,
    ) -> Result<()> {
        let cfg = self.run.cfg;
        for d in decisions {
            let frame = pending
                .remove(&d.index)
                .context("selector decided on an unknown frame")?;
            if !d.keep {
                continue;
            }
            kept.push(d.index);
            let name = super::manifest::image_name(&self.clip.stem, d.index, cfg.format);
            let masks = match frame.mask {
                // One sidecar per consumer, scaled to output size only now.
                Some(set) => {
                    let sidecar = |dir: &std::path::Path, m: &Mask| MaskWrite {
                        path: dir.join(format!("{name}.png")),
                        gray: m.scaled_to(self.out_w as u32, self.out_h as u32).data,
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
                w: self.out_w as u32,
                h: self.out_h as u32,
                rgb: frame.rgb,
                masks,
            })?;
        }
        Ok(())
    }
}

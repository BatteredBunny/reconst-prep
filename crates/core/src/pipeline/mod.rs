use crate::cancel::CancelToken;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::decode::{ClipInfo, Ffmpeg, HwAccel, probe_clip};
use crate::frame::Size;
use crate::mask::MaskConfig;
use crate::output::{ImageFormat, WriterPool};
use crate::select::SelectionConfig;
use crate::undistort::{Interp, LensProfileFile};

mod clip;
mod manifest;

use clip::{ClipJob, ClipRun, MaskDirs, Totals};

pub use clip::{FrameAnalysis, analyze_frame, render_output};

pub use manifest::{ManifestClip, ManifestParams, ManifestProfile, ManifestTotals, RunManifest};

/// The downscale is fused into the undistort kernel: rendering straight at the output size measured faster and closer to the app reference.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OutputSpec {
    /// Same as input.
    Same,
    /// Uniform scale factor, aspect preserved (0 < s <= 1 typical).
    Scale { factor: f64 },
    /// A different aspect crops the field of view, exactly like a Gyroflow app export at that size.
    Exact { width: u32, height: u32 },
}

/// Checked while parsing, so a bad one fails there rather than after the first probe.
pub const MAX_SCALE: f64 = 8.0;

/// `same`, a scale factor like `0.5`, or `WxH` like `1920x1080`.
impl std::str::FromStr for OutputSpec {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        let bad = || {
            anyhow::anyhow!("size must be \"same\", a scale factor like 0.5, or WxH like 1920x1080")
        };
        if s == "same" {
            return Ok(OutputSpec::Same);
        }
        if let Some((w, h)) = s.split_once('x') {
            return Ok(OutputSpec::Exact {
                width: w.parse().map_err(|_| bad())?,
                height: h.parse().map_err(|_| bad())?,
            });
        }
        let factor: f64 = s.parse().map_err(|_| bad())?;
        if !(factor > 0.0 && factor <= MAX_SCALE) {
            bail!("scale factor must be above 0 and at most {MAX_SCALE}, not {factor}");
        }
        Ok(OutputSpec::Scale { factor })
    }
}

impl OutputSpec {
    pub fn resolve(&self, input: Size) -> Result<Size> {
        let (w, h) = match *self {
            OutputSpec::Same => (input.w, input.h),
            OutputSpec::Scale { factor } => {
                if !(factor > 0.0 && factor <= MAX_SCALE) {
                    bail!("scale factor {factor} out of range");
                }
                let scale = |v: u32| ((v as f64 * factor).round() as u32).max(2);
                (scale(input.w), scale(input.h))
            }
            OutputSpec::Exact { width, height } => (width, height),
        };
        // Even dimensions: friendlier to every downstream consumer.
        Ok(Size::new(w & !1, h & !1))
    }
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub inputs: Vec<PathBuf>,
    /// Gyroflow lens profile JSON. `None` disables undistortion entirely.
    pub profile_path: Option<PathBuf>,
    pub out_dir: PathBuf,
    pub output_size: OutputSpec,
    pub format: ImageFormat,
    pub selection: SelectionConfig,
    pub hwaccel: HwAccel,
    pub ffmpeg_path: Option<PathBuf>,
    pub interp: Interp,
    /// Falls back to the CPU kernel automatically when no device works.
    pub gpu: bool,
    /// Inactive by default. See `crate::mask`.
    pub mask: MaskConfig,
    /// 0 = auto (half the cores, min 2).
    pub writer_threads: usize,
    /// The kernel is rayon-parallel but does not reach all cores alone, so 2-3 buys the gap back. Library-only: both frontends leave it at auto.
    pub undistort_threads: usize,
    /// Refuses when the previous run used different parameters.
    pub resume: bool,
    /// Re-emit a dataset at a different size or format while keeping the filenames a reconstruction references.
    pub frames_from: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub clip_idx: usize,
    pub n_clips: usize,
    pub clip_name: String,
    pub clip_total_frames: Option<u64>,
    pub clip_decoded: u64,
    pub total_decoded: u64,
    pub total_kept: u64,
    pub total_written: u64,
    /// Frames this invocation decoded. Below `total_decoded` after a resume,
    /// and the only one of the two a rate may be computed from.
    pub decoded_this_run: u64,
    pub elapsed_s: f64,
}

impl Progress {
    /// Over what this invocation decoded, so a resume does not inflate it.
    pub fn decode_fps(&self) -> f64 {
        self.decoded_this_run as f64 / self.elapsed_s.max(1e-9)
    }

    /// `None` when the clip length is unknown or nothing has decoded yet.
    pub fn clip_eta_s(&self, rate: f64) -> Option<f64> {
        let total = self.clip_total_frames?;
        (self.clip_decoded > 0 && total > 0)
            .then(|| total.saturating_sub(self.clip_decoded) as f64 / rate.max(1e-9))
    }
}

/// Why a run will not write into its output folder. Typed so each frontend can give its own advice:
/// a flag for the CLI, a button for the GUI.
#[derive(Debug, thiserror::Error)]
pub enum OutDirConflict {
    /// Writing would leave images from two selections side by side, with a manifest describing only the second.
    #[error("{} already holds a dataset produced with different settings ({differences})", .dir.display())]
    DifferentDataset { dir: PathBuf, differences: String },
    /// Resuming would mix two selections into one dataset.
    #[error("{} cannot be resumed: it was produced with different settings ({differences})", .dir.display())]
    ResumeRefused { dir: PathBuf, differences: String },
}

/// The video containers a directory scan accepts, case-insensitively.
pub const VIDEO_EXT: &[&str] = &["mp4", "mov", "mkv", "avi", "mts", "m2ts", "webm"];

pub fn expand_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let mut in_dir: Vec<PathBuf> = std::fs::read_dir(input)
                .with_context(|| format!("reading {}", input.display()))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| VIDEO_EXT.contains(&e.to_ascii_lowercase().as_str()))
                            .unwrap_or(false)
                })
                .collect();
            in_dir.sort();
            files.extend(in_dir);
        } else if input.is_file() {
            files.push(input.clone());
        } else {
            bail!("input {} does not exist", input.display());
        }
    }
    if files.is_empty() {
        bail!("no video files found in the given inputs");
    }
    Ok(files)
}

fn unique_stems(clips: &mut [ClipInfo]) {
    let mut stems: Vec<String> = clips.iter().map(|c| c.stem.clone()).collect();
    dedupe_stems(&mut stems);
    for (c, stem) in clips.iter_mut().zip(stems) {
        c.stem = stem;
    }
}

fn dedupe_stems(stems: &mut [String]) {
    let mut taken: HashSet<String> = HashSet::new();
    for stem in stems.iter_mut() {
        if taken.contains(stem) {
            let mut n = 1;
            while taken.contains(&format!("{stem}_{n}")) {
                n += 1;
            }
            *stem = format!("{stem}_{n}");
        }
        taken.insert(stem.clone());
    }
}

fn threads(requested: usize, auto: impl FnOnce() -> usize) -> usize {
    if requested == 0 { auto() } else { requested }
}

/// Resolved before a frame is decoded, so a bad profile fails immediately rather than half an hour in.
struct Preflight {
    ff: Ffmpeg,
    lens: Option<LensProfileFile>,
    profile: Option<ManifestProfile>,
    clips: Vec<ClipInfo>,
    /// Decided once here: deciding per clip let the manifest disagree with the next run's prediction.
    gpu: Option<usize>,
    /// Per-clip frame lists keyed by output stem. Replay selection only.
    replay: HashMap<String, Vec<u64>>,
}

fn preflight(cfg: &PipelineConfig) -> Result<Preflight> {
    let ff = Ffmpeg::resolve(cfg.ffmpeg_path.as_deref())?;

    let lens = cfg
        .profile_path
        .as_deref()
        .map(LensProfileFile::load)
        .transpose()?;
    let profile = lens
        .as_ref()
        .zip(cfg.profile_path.as_ref())
        .map(|(l, path)| ManifestProfile {
            path: path.display().to_string(),
            name: l.summary().name.clone(),
            sha256: l.sha256().to_string(),
        });

    let files = expand_inputs(&cfg.inputs)?;
    let mut clips: Vec<ClipInfo> = files
        .iter()
        .map(|f| probe_clip(&ff, f))
        .collect::<Result<_>>()?;
    unique_stems(&mut clips);

    let gpu = gpu_device(cfg, lens.as_ref(), &clips[0])?;

    // Keyed by stem rather than path: the clips may have moved, and the stem is what the output filenames carry.
    let replay = match &cfg.frames_from {
        Some(path) => {
            let source = RunManifest::read(path)?;
            let mut by_stem: HashMap<String, Vec<u64>> = HashMap::new();
            for c in &source.clips {
                by_stem.insert(c.stem.clone(), c.kept_frames.clone());
            }
            for c in &clips {
                let list = by_stem.get(&c.stem).filter(|l| !l.is_empty());
                if list.is_none() {
                    bail!(
                        "{} records no kept frames for clip {:?}. Replaying a selection needs \
                         the same clips under the same names; it also needs a manifest from a \
                         version that records them.",
                        path.display(),
                        c.stem
                    );
                }
            }
            by_stem
        }
        None => HashMap::new(),
    };

    Ok(Preflight {
        ff,
        lens,
        profile,
        clips,
        gpu,
        replay,
    })
}

/// gyroflow only finds out whether a listed device works by rendering, so one frame of the first clip's geometry is rendered here.
fn gpu_device(
    cfg: &PipelineConfig,
    lens: Option<&LensProfileFile>,
    first: &ClipInfo,
) -> Result<Option<usize>> {
    let Some(lens) = lens.filter(|_| cfg.gpu) else {
        return Ok(None);
    };
    let devices = crate::undistort::gpu_devices();
    let Some(name) = devices.first() else {
        log::warn!("GPU undistortion: no device found; using the CPU kernel");
        return Ok(None);
    };
    let params = lens.params(first.size, cfg.output_size.resolve(first.size)?)?;
    if !crate::undistort::gpu_usable(&params, cfg.interp, 0) {
        return Ok(None);
    }
    log::info!("GPU undistortion on {name}");
    Ok(Some(0))
}

fn mask_dirs(cfg: &PipelineConfig) -> Result<MaskDirs> {
    let writing = cfg.mask.is_active();
    let dirs = MaskDirs {
        sfm: cfg.out_dir.join(crate::mask::SFM_MASK_DIR),
        training: (writing && cfg.mask.has_training_class())
            .then(|| cfg.out_dir.join(crate::mask::TRAINING_MASK_DIR)),
    };
    if !writing {
        return Ok(dirs);
    }
    for dir in std::iter::once(&dirs.sfm).chain(dirs.training.iter()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating mask dir {}", dir.display()))?;
    }
    Ok(dirs)
}

fn manifest_params(cfg: &PipelineConfig, gpu: bool) -> ManifestParams {
    ManifestParams {
        output_size: cfg.output_size,
        format: cfg.format,
        selection: cfg.selection.clone(),
        hwaccel: cfg.hwaccel.clone(),
        interpolation: cfg.interp,
        gpu,
        masking: cfg.mask.is_active().then(|| cfg.mask.describe()),
    }
}

/// Different parameters are refused: selection is size-dependent, so a resume at another `--size` mixes two rules in one dataset.
fn resume_source(
    cfg: &PipelineConfig,
    params: &ManifestParams,
    pre: &Preflight,
) -> Result<Option<RunManifest>> {
    let Some(previous) = RunManifest::read_from_dir(&cfg.out_dir)? else {
        return Ok(None);
    };
    let Some(differences) = manifest::parameter_mismatch(&previous, params, pre.profile.as_ref())
    else {
        // Writing over a dataset made by the same rules only replaces files.
        return Ok(cfg.resume.then_some(previous));
    };
    let dir = cfg.out_dir.clone();
    Err(if cfg.resume {
        OutDirConflict::ResumeRefused { dir, differences }
    } else {
        OutDirConflict::DifferentDataset { dir, differences }
    }
    .into())
}

/// `wall` is 0.0 for the per-clip rewrite: the run has not finished, so there is no rate to report yet.
fn run_totals(totals: &Totals, written: u64, wall: f64) -> ManifestTotals {
    // Rates over the work this invocation did, not over clips it skipped.
    let rate = |n: u64| if wall > 0.0 { n as f64 / wall } else { 0.0 };
    ManifestTotals {
        decoded: totals.decoded,
        kept: totals.kept,
        written,
        wall_seconds: wall,
        decoded_fps: rate(totals.decoded - totals.carried_decoded),
        written_fps: rate(written - totals.carried_written),
    }
}

pub fn run_pipeline(
    cfg: &PipelineConfig,
    progress: &mut dyn FnMut(Progress),
    cancel: &CancelToken,
) -> Result<RunManifest> {
    let t0 = Instant::now();
    let pre = preflight(cfg)?;
    let params = manifest_params(cfg, pre.gpu.is_some());

    std::fs::create_dir_all(&cfg.out_dir)
        .with_context(|| format!("creating output dir {}", cfg.out_dir.display()))?;

    let previous = resume_source(cfg, &params, &pre)?;
    let mask_dirs = mask_dirs(cfg)?;

    let writer_threads = threads(cfg.writer_threads, || {
        (std::thread::available_parallelism().map_or(4, |n| n.get()) / 2).max(2)
    });
    let writers = WriterPool::new(cfg.format, writer_threads)?;

    let run = ClipRun {
        cfg,
        ff: &pre.ff,
        writers: &writers,
        mask_dirs,
        n_clips: pre.clips.len(),
        t0,
        cancel,
        lens: pre.lens.as_ref(),
        gpu: pre.gpu,
    };

    let mut manifest = RunManifest::new(pre.ff.version.clone(), pre.profile.clone(), params);

    let mut totals = Totals::default();

    for (clip_idx, clip) in pre.clips.iter().enumerate() {
        cancel.check()?;

        // Resume is per clip: selection state is built fresh per clip, so a clip boundary is the only safe split.
        if let Some(p) = previous.as_ref()
            && let Some(done) = p.clip_for(&clip.path, &clip.stem)
            && p.clip_is_intact(done, &cfg.out_dir)
        {
            totals.decoded += done.frames_decoded;
            totals.kept += done.frames_kept;
            totals.carried_decoded += done.frames_decoded;
            totals.carried_written += done.frames_kept;
            totals.carried_clips += 1;
            manifest.clips.push(done.clone());
            continue;
        }

        let record = run.process(
            ClipJob {
                idx: clip_idx,
                info: clip,
                replay: pre.replay.get(&clip.stem).map(|v| v.as_slice()),
            },
            &mut totals,
            progress,
        )?;
        manifest.clips.push(record);

        // Rewritten after every clip so a killed run stays resumable: the file always describes the clips that finished.
        writers.drain()?;
        manifest.totals = run_totals(&totals, writers.written() + totals.carried_written, 0.0);
        manifest.write_to_dir(&cfg.out_dir)?;
    }

    let written = writers.finish()? + totals.carried_written;
    if totals.kept == 0 {
        bail!("zero frames selected. Are the selection thresholds too strict for this footage?");
    }
    if totals.carried_clips > 0 {
        log::info!(
            "resumed: {} of {} clips were already complete in {}",
            totals.carried_clips,
            pre.clips.len(),
            cfg.out_dir.display()
        );
    }

    manifest::write_cameras_txt(&cfg.out_dir, &manifest.clips)?;

    manifest.completed = true;
    manifest.totals = run_totals(&totals, written, t0.elapsed().as_secs_f64());
    manifest.write_to_dir(&cfg.out_dir)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_spec_parses_and_resolves_to_even_dims() {
        let input = Size::new(3840, 2880);
        let resolve = |s: &str| s.parse::<OutputSpec>().unwrap().resolve(input).unwrap();
        assert_eq!(resolve("same"), input);
        assert_eq!(resolve("0.5"), Size::new(1920, 1440));
        assert_eq!(resolve("1919X1079"), Size::new(1918, 1078));
        for bad in ["0", "9", "big"] {
            assert!(bad.parse::<OutputSpec>().is_err(), "{bad}");
        }
    }

    /// Every frame index two clips share would otherwise be written twice, and which wins is a race.
    #[test]
    fn deduped_stems_are_pairwise_distinct() {
        let mut stems = ["clip", "clip", "clip_1", "clip", "other"].map(String::from);
        dedupe_stems(&mut stems);
        assert_eq!(stems, ["clip", "clip_1", "clip_1_1", "clip_2", "other"]);

        let unique: HashSet<&String> = stems.iter().collect();
        assert_eq!(unique.len(), stems.len(), "collision in {stems:?}");
    }
}

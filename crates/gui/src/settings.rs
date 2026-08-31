// Only the knobs, never the clips or the output folder, so reopening cannot point at the last run's dataset.

use std::path::{Path, PathBuf};

use reconst_prep_core::decode::HwAccel;
use reconst_prep_core::mask::{MaskConfig, MaskSources, SkyParams};
use reconst_prep_core::pipeline::OutputSpec;
use reconst_prep_core::seg::SegClassParams;
use reconst_prep_core::select::{SelectionConfig, SelectionMode};
use reconst_prep_core::undistort::Interp;

use crate::{ModeChoice, SizeChoice};

/// A setting with a short list of named options, for the combo boxes. The name is also what serde writes, so menu and file cannot drift.
pub(crate) trait Choice: Sized + Clone + PartialEq {
    /// Every option, in the order the combo box lists them.
    fn all() -> Vec<Self>;
    /// What the option is called, on screen and on disk.
    fn label(&self) -> String;
}

impl Choice for HwAccel {
    fn all() -> Vec<Self> {
        let mut out = vec![HwAccel::Auto, HwAccel::None];
        out.extend(
            HwAccel::KNOWN_BACKENDS
                .iter()
                .map(|b| HwAccel::Backend(b.to_string())),
        );
        out
    }
    fn label(&self) -> String {
        self.to_string()
    }
}

impl Choice for Interp {
    fn all() -> Vec<Self> {
        vec![Interp::Bilinear, Interp::Bicubic, Interp::Lanczos4]
    }
    fn label(&self) -> String {
        self.name().to_string()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub undistort: bool,
    pub profile: Option<PathBuf>,
    pub size_choice: SizeChoice,
    pub scale_factor: f64,
    pub exact_w: u32,
    pub exact_h: u32,
    pub format_jpeg: bool,
    pub jpeg_quality: u8,
    pub mode: ModeChoice,
    pub nth: u32,
    pub motion_threshold: f64,
    pub window: u32,
    pub blur_floor_on: bool,
    pub blur_floor: f64,
    pub mask_sky: bool,
    pub sky: SkyParams,
    pub mask_people: bool,
    /// Path only: the model itself is the user's file, not ours to cache.
    pub seg_model: Option<PathBuf>,
    pub seg_width: u32,
    pub seg_temporal_window: u32,

    pub hwaccel: HwAccel,

    pub interp: Interp,
    /// Undistort on the GPU, with automatic CPU fallback.
    pub gpu: bool,
    /// Lens profiles picked before, most recent first.
    pub recent_profiles: Vec<PathBuf>,
    /// Position is deliberately not stored: restoring onto a monitor that is gone is worse than letting the compositor place it.
    pub window_size: Option<[f32; 2]>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            undistort: false,
            profile: None,
            // Native, matching `--size same`: a dataset the two frontends disagree about is one `--resume` refuses.
            size_choice: SizeChoice::Same,
            scale_factor: 0.5,
            exact_w: 1920,
            exact_h: 1080,
            format_jpeg: true,
            jpeg_quality: reconst_prep_core::output::DEFAULT_JPEG_QUALITY,
            mode: ModeChoice::Motion,
            nth: reconst_prep_core::select::DEFAULT_NTH,
            motion_threshold: reconst_prep_core::select::DEFAULT_MOTION_THRESHOLD,
            window: reconst_prep_core::select::DEFAULT_WINDOW,
            blur_floor_on: false,
            blur_floor: 100.0,
            mask_sky: false,
            sky: SkyParams::default(),
            mask_people: false,
            seg_model: None,
            seg_width: reconst_prep_core::seg::DEFAULT_SEG_WIDTH,
            seg_temporal_window: reconst_prep_core::seg::DEFAULT_TEMPORAL_WINDOW,
            hwaccel: HwAccel::Auto,
            interp: Interp::Bilinear,
            // On by default: the CPU kernel is the slow path, and a machine with no usable device falls back silently.
            gpu: true,
            recent_profiles: Vec::new(),
            window_size: None,
        }
    }
}

// Interrupted runs.

/// Deleted only when the run *completes*, so a cancel, a bad clip or a kill all leave it for the next launch to find.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterruptedRun {
    pub out_dir: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub settings: Settings,
    pub started_unix: u64,
}

impl InterruptedRun {
    /// Only if an unfinished manifest still backs it: a record pointing at a completed or deleted directory is stale.
    pub fn load() -> Option<Self> {
        let path = state_path()?;
        let text = std::fs::read_to_string(path).ok()?;
        let run: Self = serde_json::from_str(&text).ok()?;
        let manifest =
            reconst_prep_core::pipeline::RunManifest::read_from_dir(&run.out_dir).ok()??;
        (!manifest.completed).then_some(run)
    }

    /// Read from the manifest rather than remembered.
    pub fn progress(&self) -> Option<(u64, usize)> {
        let m = reconst_prep_core::pipeline::RunManifest::read_from_dir(&self.out_dir).ok()??;
        Some((m.totals.written, m.clips.len()))
    }

    pub fn save(&self) {
        if let Some(path) = state_path() {
            write_json(&path, self);
        }
    }

    pub fn clear() {
        if let Some(path) = state_path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// State, not configuration, so it goes in the state directory rather than beside the settings.
fn state_path() -> Option<PathBuf> {
    Some(
        reconst_prep_core::paths::state_dir()
            .ok()?
            .join("interrupted-run.json"),
    )
}

fn config_path() -> Option<PathBuf> {
    Some(
        reconst_prep_core::paths::config_dir()
            .ok()?
            .join("settings.json"),
    )
}

impl Settings {
    /// Missing or unreadable settings fall back to defaults rather than refusing to start.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                // One unreadable field costs every setting, and the next save
                // would write defaults over the file, so keep what was there.
                let backup = path.with_extension("json.bak");
                let kept = std::fs::rename(&path, &backup).is_ok();
                log::warn!(
                    "settings at {} could not be read ({e}); starting from defaults{}",
                    path.display(),
                    if kept {
                        format!(", previous file kept at {}", backup.display())
                    } else {
                        String::new()
                    }
                );
                Self::default()
            }
        }
    }

    /// Best-effort. A GUI that cannot write its config should still run.
    pub fn save(&self) {
        if let Some(path) = config_path() {
            write_json(&path, self);
        }
    }
}

/// Failures are swallowed: neither file is worth refusing to run, or to close, over.
fn write_json(path: &Path, value: &impl serde::Serialize) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(value) {
        let _ = std::fs::write(path, json);
    }
}

/// The run this configuration describes. Kept beside the settings rather
/// than on `App`: none of it reads anything but the settings themselves.
impl Settings {
    pub fn output_spec(&self) -> OutputSpec {
        match self.size_choice {
            SizeChoice::Same => OutputSpec::Same,
            SizeChoice::Scale => OutputSpec::Scale {
                factor: self.scale_factor,
            },
            SizeChoice::Exact => OutputSpec::Exact {
                width: self.exact_w,
                height: self.exact_h,
            },
        }
    }

    pub fn selection_config(&self) -> SelectionConfig {
        let mode = match self.mode {
            ModeChoice::Motion => SelectionMode::MotionGated {
                motion_threshold: self.motion_threshold,
                window: self.window,
            },
            ModeChoice::EveryNth => SelectionMode::EveryNth { n: self.nth },
        };
        SelectionConfig {
            mode,
            blur_floor: self.blur_floor_on.then_some(self.blur_floor),
        }
    }

    /// The GUI always asks for the stock ADE20K classes, unlike the CLI's per-class flags.
    pub fn mask_config(&self) -> MaskConfig {
        MaskConfig::from_sources(MaskSources {
            mask_sky: self.mask_sky,
            sky_heuristic: self.sky,
            model: self.seg_model.clone(),
            sky_class: SegClassParams::sky(),
            people_class: self.mask_people.then(SegClassParams::people),
            seg_width: self.seg_width,
            temporal_window: self.seg_temporal_window,
        })
    }

    /// The lens profile actually in effect: only when undistortion is on.
    pub fn active_profile(&self) -> Option<PathBuf> {
        self.undistort.then(|| self.profile.clone()).flatten()
    }

    pub fn masked_classes(&self) -> Vec<&'static str> {
        self.mask_config()
            .classes_with_source()
            .into_iter()
            .map(|(class, _)| class.name())
            .collect()
    }
}

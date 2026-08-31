use std::path::Path;

use anyhow::{Context, Result};

use crate::decode::HwAccel;
use crate::mask::ManifestMasking;
use crate::output::ImageFormat;
use crate::select::SelectionConfig;
use crate::undistort::Interp;

use super::OutputSpec;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestProfile {
    pub path: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestParams {
    pub output_size: OutputSpec,
    pub format: ImageFormat,
    pub selection: SelectionConfig,
    pub hwaccel: HwAccel,
    pub interpolation: Interp,
    #[serde(default)]
    pub gpu: bool,
    /// Absent when nothing was masked; the PNGs alone cannot say which class they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masking: Option<ManifestMasking>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestClip {
    pub path: String,
    pub stem: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames_probed: Option<u64>,
    pub frames_decoded: u64,
    pub frames_kept: u64,
    pub out_width: usize,
    pub out_height: usize,
    /// `None` without lens correction: the images still carry distortion, so there is no honest pinhole model.
    pub pinhole_intrinsics: Option<[f64; 4]>,
    /// Replayed by `--frames-from`, and how `--resume` checks a clip's images are on disk.
    #[serde(default)]
    pub kept_frames: Vec<u64>,
}

/// The single definition of the naming convention: `--resume` looks for exactly these names, so writer and check cannot drift.
pub fn image_name(stem: &str, index: u64, format: ImageFormat) -> String {
    format!("{stem}_{index:06}.{}", format.ext())
}

impl ManifestClip {
    /// The file name a kept frame was written under.
    pub fn image_name(&self, index: u64, format: ImageFormat) -> String {
        image_name(&self.stem, index, format)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestTotals {
    pub decoded: u64,
    pub kept: u64,
    pub written: u64,
    pub wall_seconds: f64,
    pub decoded_fps: f64,
    pub written_fps: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunManifest {
    pub tool: String,
    pub tool_version: String,
    pub gyroflow_core_rev: String,
    pub ffmpeg_version: String,
    pub created_unix: u64,
    /// A killed run leaves this false with only the finished clips listed, which is what `--resume` needs.
    #[serde(default = "default_true")]
    pub completed: bool,
    /// `None` when undistortion was off.
    pub profile: Option<ManifestProfile>,
    pub params: ManifestParams,
    pub clips: Vec<ManifestClip>,
    pub totals: ManifestTotals,
}

fn default_true() -> bool {
    true
}

/// File name of the manifest inside an output directory.
const MANIFEST_NAME: &str = "reconst-prep-manifest.json";

impl RunManifest {
    /// A manifest for a run about to start: no clips, zero totals, `completed: false`.
    pub fn new(
        ffmpeg_version: String,
        profile: Option<ManifestProfile>,
        params: ManifestParams,
    ) -> Self {
        Self {
            tool: crate::TOOL_NAME.to_string(),
            tool_version: crate::TOOL_VERSION.to_string(),
            gyroflow_core_rev: crate::GYROFLOW_CORE_REV.to_string(),
            ffmpeg_version,
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            completed: false,
            profile,
            params,
            clips: Vec::new(),
            totals: ManifestTotals {
                decoded: 0,
                kept: 0,
                written: 0,
                wall_seconds: 0.0,
                decoded_fps: 0.0,
                written_fps: 0.0,
            },
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parsing manifest {}", path.display()))
    }

    /// A malformed manifest is an error, not `None`: ignoring it would let a resume silently mix two selections.
    pub fn read_from_dir(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(MANIFEST_NAME);
        if !path.exists() {
            return Ok(None);
        }
        Self::read(&path).map(Some)
    }

    pub fn write_to_dir(&self, dir: &Path) -> Result<()> {
        let path = dir.join(MANIFEST_NAME);
        crate::paths::write_atomic(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    /// Falls back to the output stem: a dataset can outlive the directory layout its clips came from.
    pub fn clip_for(&self, path: &Path, stem: &str) -> Option<&ManifestClip> {
        let want = path.display().to_string();
        self.clips
            .iter()
            .find(|c| c.path == want)
            .or_else(|| self.clips.iter().find(|c| c.stem == stem))
    }

    /// The entry only appears once a clip finished, but files can be deleted or partially copied away afterwards.
    pub fn clip_is_intact(&self, clip: &ManifestClip, dir: &Path) -> bool {
        clip.kept_frames.len() as u64 == clip.frames_kept
            && clip
                .kept_frames
                .iter()
                .all(|&i| dir.join(clip.image_name(i, self.params.format)).is_file())
    }
}

/// Compared field by field so the message can name the difference. `hwaccel` is excluded: it changes decode speed, not which frames come out.
pub(super) fn parameter_mismatch(
    previous: &RunManifest,
    params: &ManifestParams,
    profile: Option<&ManifestProfile>,
) -> Option<String> {
    let p = &previous.params;
    let mut differences = Vec::new();
    let mut compare = |name: &str, a: &dyn std::fmt::Debug, b: &dyn std::fmt::Debug| {
        let (a, b) = (format!("{a:?}"), format!("{b:?}"));
        if a != b {
            differences.push(format!("{name} ({a} then, {b} now)"));
        }
    };
    compare("output size", &p.output_size, &params.output_size);
    compare("image format", &p.format, &params.format);
    compare("frame selection", &p.selection, &params.selection);
    compare("interpolation", &p.interpolation, &params.interpolation);
    compare("gpu undistortion", &p.gpu, &params.gpu);
    compare("masking", &p.masking, &params.masking);
    compare(
        "lens profile",
        &previous.profile.as_ref().map(|x| &x.sha256),
        &profile.map(|x| &x.sha256),
    );
    (!differences.is_empty()).then(|| differences.join("; "))
}

/// One PINHOLE camera per clip so downstream tooling need not estimate one; skipped when nothing was undistorted.
pub(super) fn write_cameras_txt(out_dir: &Path, clips: &[ManifestClip]) -> Result<()> {
    use std::fmt::Write as _;

    if clips.iter().all(|c| c.pinhole_intrinsics.is_none()) {
        return Ok(());
    }
    let mut s = String::from(
        "# Camera list with one line of data per camera:\n\
         #   CAMERA_ID, MODEL, WIDTH, HEIGHT, PARAMS[]\n\
         # Written by reconst-prep: images are already undistorted, so the model is\n\
         # PINHOLE with exact intrinsics from the lens profile. Do NOT let a\n\
         # reconstructor re-estimate distortion that is no longer in the images.\n",
    );
    for (i, c) in clips.iter().enumerate() {
        let Some([fx, fy, cx, cy]) = c.pinhole_intrinsics else {
            continue;
        };
        let id = i + 1;
        let _ = writeln!(s, "# camera {id} <- images {}_*", c.stem);
        let _ = writeln!(
            s,
            "{id} PINHOLE {} {} {fx} {fy} {cx} {cy}",
            c.out_width, c.out_height
        );
    }
    let path = out_dir.join("cameras.txt");
    crate::paths::write_atomic(&path, s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::{SelectionConfig, SelectionMode};

    fn manifest(size: OutputSpec) -> RunManifest {
        RunManifest {
            tool: "reconst-prep".into(),
            tool_version: "0.1.0".into(),
            gyroflow_core_rev: "deadbeef".into(),
            ffmpeg_version: "ffmpeg 8".into(),
            created_unix: 0,
            completed: true,
            profile: None,
            params: ManifestParams {
                output_size: size,
                format: ImageFormat::Jpeg { quality: 95 },
                selection: SelectionConfig::default(),
                gpu: false,
                hwaccel: HwAccel::Auto,
                interpolation: Interp::Lanczos4,
                masking: None,
            },
            clips: vec![],
            totals: ManifestTotals {
                decoded: 0,
                kept: 0,
                written: 0,
                wall_seconds: 0.0,
                decoded_fps: 0.0,
                written_fps: 0.0,
            },
        }
    }

    #[test]
    fn identical_parameters_do_not_block_a_resume() {
        let m = manifest(OutputSpec::Same);
        let params = manifest(OutputSpec::Same).params;
        assert_eq!(parameter_mismatch(&m, &params, None), None);
    }

    #[test]
    fn a_changed_output_size_is_refused_and_named() {
        // At --size 0.25 vs 0.5, 3 of 41 kept frames differ, so a resume across a size change mixes two selections.
        let m = manifest(OutputSpec::Same);
        let params = manifest(OutputSpec::Scale { factor: 0.5 }).params;
        let diff = parameter_mismatch(&m, &params, None).expect("must refuse");
        assert!(diff.contains("output size"), "unhelpful: {diff}");
    }

    #[test]
    fn a_changed_selection_is_refused() {
        let m = manifest(OutputSpec::Same);
        let mut params = manifest(OutputSpec::Same).params;
        params.selection = SelectionConfig {
            mode: SelectionMode::EveryNth { n: 5 },
            blur_floor: None,
        };
        let diff = parameter_mismatch(&m, &params, None).expect("must refuse");
        assert!(diff.contains("frame selection"), "unhelpful: {diff}");
    }

    #[test]
    fn hwaccel_alone_does_not_block_a_resume() {
        let m = manifest(OutputSpec::Same);
        let mut params = manifest(OutputSpec::Same).params;
        params.hwaccel = HwAccel::None;
        assert_eq!(parameter_mismatch(&m, &params, None), None);
    }

    #[test]
    fn a_manifest_round_trips() {
        let m = manifest(OutputSpec::Exact {
            width: 1920,
            height: 1080,
        });
        let json = serde_json::to_string(&m).unwrap();
        let back: RunManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.params.output_size, m.params.output_size);
        assert!(back.completed);
    }

    /// The four `#[serde(default)]` fields are the whole back-compat contract,
    /// and a round trip of a value this build wrote never exercises them.
    #[test]
    fn a_manifest_predating_the_optional_fields_still_loads() {
        let older = r#"{
            "tool": "reconst-prep",
            "tool_version": "0.0.1",
            "gyroflow_core_rev": "deadbeef",
            "ffmpeg_version": "ffmpeg 7",
            "created_unix": 0,
            "profile": null,
            "params": {
                "output_size": { "kind": "same" },
                "format": { "format": "jpeg", "quality": 95 },
                "selection": {
                    "mode": { "mode": "motion-gated", "motion_threshold": 0.04, "window": 8 },
                    "blur_floor": null
                },
                "hwaccel": "auto",
                "interpolation": "lanczos4"
            },
            "clips": [{
                "path": "/tmp/clip.mp4", "stem": "clip",
                "width": 1920, "height": 1080, "fps": 30.0,
                "frames_probed": null, "frames_decoded": 10, "frames_kept": 0,
                "out_width": 1920, "out_height": 1080, "pinhole_intrinsics": null
            }],
            "totals": {
                "decoded": 0, "kept": 0, "written": 0,
                "wall_seconds": 0.0, "decoded_fps": 0.0, "written_fps": 0.0
            }
        }"#;
        let m: RunManifest = serde_json::from_str(older).expect("older manifest must still load");
        // The field postdates every manifest that lacks it, and those were all
        // written by runs that finished.
        assert!(m.completed);
        assert!(!m.params.gpu);
        // Both were plain strings in the manifest before they were typed; the
        // spelling on disk is the contract, not the Rust representation.
        assert_eq!(m.params.hwaccel, HwAccel::Auto);
        assert_eq!(m.params.interpolation, Interp::Lanczos4);
        assert_eq!(m.params.masking, None);
        assert!(m.clips[0].kept_frames.is_empty());
    }

    /// The GPU and lens-profile refusals are the two `parameter_mismatch`
    /// compares that no test covered, and both change which frames come out.
    #[test]
    fn a_changed_backend_or_profile_is_refused() {
        let m = manifest(OutputSpec::Same);
        let mut params = manifest(OutputSpec::Same).params;
        params.gpu = true;
        let diff = parameter_mismatch(&m, &params, None).expect("must refuse a backend change");
        assert!(diff.contains("gpu"), "unhelpful: {diff}");

        let profile = ManifestProfile {
            path: "/tmp/lens.json".into(),
            name: "DJI O4".into(),
            sha256: "abc123".into(),
        };
        let params = manifest(OutputSpec::Same).params;
        let diff = parameter_mismatch(&m, &params, Some(&profile))
            .expect("must refuse a profile appearing");
        assert!(diff.contains("profile"), "unhelpful: {diff}");
    }

    #[test]
    fn a_clip_is_not_intact_when_an_image_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(OutputSpec::Same);
        let clip = ManifestClip {
            path: "/tmp/clip.mp4".into(),
            stem: "clip".into(),
            width: 3840,
            height: 2160,
            fps: 60.0,
            frames_probed: Some(100),
            frames_decoded: 100,
            frames_kept: 2,
            out_width: 3840,
            out_height: 2160,
            pinhole_intrinsics: None,
            kept_frames: vec![0, 15],
        };

        assert!(!m.clip_is_intact(&clip, dir.path()), "nothing written yet");

        std::fs::write(dir.path().join(clip.image_name(0, m.params.format)), b"x").unwrap();
        assert!(!m.clip_is_intact(&clip, dir.path()), "one of two written");

        std::fs::write(dir.path().join(clip.image_name(15, m.params.format)), b"x").unwrap();
        assert!(m.clip_is_intact(&clip, dir.path()));
    }

    /// `frames_kept` and the recorded list disagreeing means the record itself
    /// is inconsistent, so the clip must be redone rather than trusted.
    #[test]
    fn a_clip_whose_count_disagrees_with_its_list_is_not_intact() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(OutputSpec::Same);
        let mut clip = ManifestClip {
            path: "/tmp/clip.mp4".into(),
            stem: "clip".into(),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frames_probed: None,
            frames_decoded: 10,
            frames_kept: 3,
            out_width: 1920,
            out_height: 1080,
            pinhole_intrinsics: None,
            kept_frames: vec![0],
        };
        std::fs::write(dir.path().join(clip.image_name(0, m.params.format)), b"x").unwrap();
        assert!(!m.clip_is_intact(&clip, dir.path()));

        clip.frames_kept = 1;
        assert!(m.clip_is_intact(&clip, dir.path()));
    }
}

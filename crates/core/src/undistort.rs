// Lens-only undistortion via gyroflow-core's CPU kernel

use std::path::Path;

use anyhow::{Context, Result, bail};
use gyroflow_core::gpu::{BufferDescription, BufferSource, Buffers};
use gyroflow_core::lens_profile::LensProfile;
use gyroflow_core::stabilization::{
    ComputeParams, GPU_LIST, Interpolation, PixelType, RGB8, RGBA8, Stabilization,
    distortion_models::DistortionModel,
};
use sha2::Digest as _;

use crate::frame::{Rgb8, Size};

/// One per clip, since clips may differ in size. Only built from a [`LensProfileFile`], which has already checked the profile.
pub struct LensOnlyParams {
    lens: LensProfile,
    input: Size,
    output: Size,
}

/// A lens profile read and checked once, then shared by every clip and preview.
#[derive(Clone)]
pub struct LensProfileFile {
    lens: LensProfile,
    sha256: String,
    summary: ProfileSummary,
}

impl LensProfileFile {
    pub fn load(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)
            .with_context(|| format!("reading lens profile {}", path.display()))?;
        Self::parse(&json).with_context(|| format!("lens profile {} rejected", path.display()))
    }

    /// Takes raw JSON, not a path: gyroflow's `load_from_file` routes through a VFS layer we must not touch.
    pub fn parse(json: &str) -> Result<Self> {
        // Never bare serde: from_json also runs init(), computing the r_limit that culls pixels past the monotonic radius.
        let lens = LensProfile::from_json(json)
            .map_err(|e| anyhow::anyhow!("lens profile parse failed: {e}"))?;
        if lens.fisheye_params.distortion_coeffs.is_empty() {
            bail!("lens profile has no distortion coefficients");
        }
        if lens.calib_dimension.w == 0 || lens.calib_dimension.h == 0 {
            bail!("lens profile has no calibration dimensions");
        }
        let summary = ProfileSummary {
            name: lens.name.clone(),
            camera: format!("{} {}", lens.camera_brand, lens.camera_model)
                .trim()
                .to_string(),
            calib: Size::new(lens.calib_dimension.w as u32, lens.calib_dimension.h as u32),
            fps: lens.fps,
        };
        let sha256 = crate::paths::hex(&sha2::Sha256::digest(json.as_bytes()));
        Ok(Self {
            lens,
            sha256,
            summary,
        })
    }

    pub fn summary(&self) -> &ProfileSummary {
        &self.summary
    }

    /// Of the file's bytes, so a resume can tell whether the profile changed.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn params(&self, input: Size, output: Size) -> Result<LensOnlyParams> {
        if input.is_empty() || output.is_empty() {
            bail!("zero input/output dimensions");
        }
        Ok(LensOnlyParams {
            lens: self.lens.clone(),
            input,
            output,
        })
    }
}

/// Bilinear by default: under a 2:1 downscale all three register within 0.004 px of a Gyroflow app export, at 30.5 fps against Lanczos4's 18.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interp {
    Bilinear,
    Bicubic,
    #[serde(alias = "lanczos")]
    Lanczos4,
}

impl Interp {
    pub const ALL: [Interp; 3] = [Interp::Bilinear, Interp::Bicubic, Interp::Lanczos4];

    fn to_gf(self) -> Interpolation {
        match self {
            Interp::Bilinear => Interpolation::Bilinear,
            Interp::Bicubic => Interpolation::Bicubic,
            Interp::Lanczos4 => Interpolation::Lanczos4,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Interp::Bilinear => "bilinear",
            Interp::Bicubic => "bicubic",
            Interp::Lanczos4 => "lanczos4",
        }
    }
}

/// The serde spelling, so the flag, the settings file and the manifest accept the same names.
impl std::str::FromStr for Interp {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use serde::Deserialize as _;
        use serde::de::IntoDeserializer as _;
        Self::deserialize(s.to_ascii_lowercase().into_deserializer())
    }
}

/// Gyroflow treats <= 0.01 as absent; matching that keeps our intrinsics equal to an app export's.
fn stretch(v: f64) -> f64 {
    if v > 0.01 { v } else { 1.0 }
}

impl LensOnlyParams {
    /// Field-by-field on purpose: upstream has no constructor, and two Default values are actively wrong.
    #[allow(clippy::field_reassign_with_default)]
    fn compute_params(&self) -> ComputeParams {
        let mut cp = ComputeParams::default();

        // Unknown ids fall back to OpenCVFisheye, which is also right: all 9810 published profiles are that.
        cp.distortion_model = DistortionModel::from_name(
            self.lens
                .distortion_model
                .as_deref()
                .unwrap_or("opencv_fisheye"),
        );
        cp.digital_lens = self
            .lens
            .digital_lens
            .as_ref()
            .map(|x| DistortionModel::from_name(x));
        cp.digital_lens_params = self.lens.digital_lens_params.clone();
        cp.lens = self.lens.clone();

        // INPUT frame size: never rescale the camera matrix ourselves, get_lens_data_at_timestamp does it.
        (cp.width, cp.height) = self.input.as_usize();
        (cp.output_width, cp.output_height) = self.output.as_usize();

        // Default 0.0 clamps fov to 0.001 => ~1000x zoom, garbage output.
        cp.fov_scale = 1.0;
        // Default 0.0 silently DISABLES lens correction entirely.
        cp.lens_correction_amount = 1.0;
        // "Not underwater".
        cp.light_refraction_coefficient = 1.0;

        // Explicitly OFF: no gyro, no smoothing, no rolling shutter, no zoom.
        cp.frame_count = 1;
        cp.scaled_fps = 60.0; // only used to derive `frame` when not passed
        cp.frame_readout_time = 0.0; // -> exactly one transform matrix, no RS
        cp.fovs = vec![]; // no adaptive-zoom curve -> per-frame fov factor 1.0
        cp.minimal_fovs = vec![];
        cp.fov_overview = false;
        cp.suppress_rotation = true; // force R = identity
        cp.max_zoom = None;
        // cp.gyro stays empty: quat_at_timestamp() returns identity below 2 quaternions.
        cp
    }

    /// Mirrors FrameTransform::get_fov at fov_scale 1: horizontal half-angle exact, cropped vertically.
    pub fn fov(&self) -> f64 {
        self.input.w as f64 / (self.output.w.max(1) as f64)
    }

    /// PINHOLE intrinsics of the *undistorted output*, for the manifest and cameras.txt.
    pub fn output_intrinsics(&self) -> [f64; 4] {
        let k = self.lens.get_camera_matrix(self.input.as_usize());
        // The matrix comes back at CALIBRATION resolution; scale it as get_lens_data_at_timestamp does.
        let calib_w = self.lens.calib_dimension.w as f64;
        let calib_h = self.lens.calib_dimension.h as f64;
        let stretch_x = stretch(self.lens.input_horizontal_stretch);
        let stretch_y = stretch(self.lens.input_vertical_stretch);
        let ratio_x = self.input.w as f64 / calib_w * stretch_x;
        let ratio_y = self.input.h as f64 / calib_h * stretch_y;
        let fov = self.fov();
        // get_new_k: focals divided by fov with horizontal stretch backed out.
        let img_dim_ratio = 1.0 / stretch_x;
        [
            k[(0, 0)] * ratio_x * img_dim_ratio / fov,
            k[(1, 1)] * ratio_y * img_dim_ratio / fov,
            self.output.w as f64 / 2.0,
            self.output.h as f64 / 2.0,
        ]
    }
}

/// Also publishes into gyroflow's global `GPU_LIST`, which is what `set_device(i)` indexes into.
pub fn gpu_devices() -> Vec<String> {
    let stab = Stabilization::default();
    let list = stab.list_devices();
    *GPU_LIST.write() = list.clone();
    list
}

/// gyroflow initializes wgpu on the first frame, so the only way to ask is to render one.
pub fn gpu_usable(params: &LensOnlyParams, interp: Interp, device: usize) -> bool {
    let mut u = Undistorter::new(params, interp, Some(device));
    // `process_rgb` drops the RGBA staging when it detects the fallback.
    u.process_rgb(&mut Rgb8::zeroed(params.input)).is_ok() && u.rgba.is_some()
}

/// One `Stabilization` reused across a clip, so the transform matrix is computed once. Not Sync.
pub struct Undistorter {
    stab: Stabilization,
    /// GPU mode only: wgpu has no packed 3-byte texture format, so every frame is converted in and out.
    rgba: Option<(Vec<u8>, Vec<u8>)>,
    /// Verified against `info.backend` on the first frame, then trusted.
    backend_checked: bool,
    input: Size,
    output: Size,
}

impl Undistorter {
    /// `gpu` is a [`gpu_devices`] index. A failed GPU init falls back to gyroflow's CPU kernel, which `process_rgb` detects on the first frame.
    pub fn new(params: &LensOnlyParams, interp: Interp, gpu: Option<usize>) -> Self {
        let device = gpu.map_or(-1, |d| d as isize);
        let mut stab = Stabilization::default();
        stab.set_compute_params(params.compute_params());
        stab.init_size(params.input.as_usize(), params.output.as_usize());
        stab.interpolation = interp.to_gf();
        // set_device(-1) => BackendType::Cpu, a real switch: no adapter or Vulkan loader is created at all.
        stab.set_device(device);
        let rgba = (device >= 0).then(|| {
            (
                vec![0u8; params.input.px() * 4],
                vec![0u8; params.output.px() * 4],
            )
        });
        Self {
            stab,
            rgba,
            backend_checked: false,
            input: params.input,
            output: params.output,
        }
    }

    /// `frame` is `&mut` only because `BufferSource::Cpu` demands it; it is unmodified.
    pub fn process_rgb(&mut self, frame: &mut Rgb8) -> Result<Rgb8> {
        anyhow::ensure!(
            frame.size() == self.input,
            "frame is {}, the undistorter was built for {}",
            frame.size(),
            self.input
        );
        let mut rendered = Rgb8::zeroed(self.output);
        let (input, out) = (frame.data_mut(), rendered.data_mut());
        let backend = match self.rgba.take() {
            Some((mut rgba_in, mut rgba_out)) => {
                for (dst, src) in rgba_in
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(input.as_chunks::<3>().0)
                {
                    dst[..3].copy_from_slice(src);
                }
                let backend = self.render::<RGBA8>(4, &mut rgba_in, &mut rgba_out);
                for (dst, src) in out
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(rgba_out.as_chunks::<4>().0)
                {
                    dst.copy_from_slice(&src[..3]);
                }
                // Put back before `?`, so a failed frame does not silently switch the next one to the CPU path.
                self.rgba = Some((rgba_in, rgba_out));
                backend?
            }
            None => self.render::<RGB8>(3, input, out)?,
        };
        if !self.backend_checked {
            self.backend_checked = true;
            match (self.rgba.is_some(), backend) {
                // GPU asked for and running: done.
                (true, "wgpu") => {}
                // gyroflow fell back to CPU, so drop the staging and run the CPU path at full speed.
                (true, other) => {
                    log::warn!(
                        "GPU undistortion unavailable (backend {other:?}); using the CPU kernel"
                    );
                    self.rgba = None;
                    self.stab.set_device(-1);
                }
                // A GPU backend here means set_device(-1) regressed upstream.
                (false, other) => anyhow::ensure!(
                    other == "CPU",
                    "expected the CPU undistort path, got backend {other}"
                ),
            }
        }
        Ok(rendered)
    }

    fn render<T: PixelType>(
        &mut self,
        bpp: usize,
        input: &mut [u8],
        out: &mut [u8],
    ) -> Result<&'static str> {
        let cpu = |size: Size, buffer| {
            let (w, h) = size.as_usize();
            BufferDescription {
                size: (w, h, w * bpp), // w, h, stride
                rect: None,
                rotation: None,
                data: BufferSource::Cpu { buffer },
                texture_copy: false,
            }
        };
        let mut buffers = Buffers {
            input: cpu(self.input, input),
            output: cpu(self.output, out),
        };
        Self::run::<T>(&mut self.stab, &mut buffers)
    }

    fn run<T: PixelType>(stab: &mut Stabilization, buffers: &mut Buffers) -> Result<&'static str> {
        // With no gyro and zero readout time the transform is timestamp-independent, and Some(frame) skips frame_at_timestamp.
        stab.ensure_ready_for_processing::<T>(0, Some(0), buffers);
        let info = stab
            .process_pixels::<T>(0, Some(0), buffers, None)
            .map_err(|e| anyhow::anyhow!("gyroflow-core process_pixels failed: {e:?}"))?;
        Ok(info.backend)
    }
}

/// What a profile claims about its camera, for display and for checking against the footage.
#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub name: String,
    pub camera: String,
    pub calib: Size,
    pub fps: f64,
}

impl ProfileSummary {
    /// Calibration is per-aspect: the wrong variant silently undistorts to the wrong field of view.
    pub fn mismatch(&self, clip: Size, clip_fps: f64) -> Option<String> {
        if self.calib.is_empty() || clip.is_empty() {
            return None;
        }
        let aspect = |s: Size| s.w as f64 / s.h as f64;
        let (calib_ar, clip_ar) = (aspect(self.calib), aspect(clip));
        if (calib_ar - clip_ar).abs() > 0.02 {
            return Some(format!(
                "profile is calibrated for {}×{} ({calib_ar:.2}:1). The clip is {}×{} \
                 ({clip_ar:.2}:1). A different aspect means a different calibrated crop",
                self.calib.w, self.calib.h, clip.w, clip.h
            ));
        }
        if self.fps > 0.0 && clip_fps > 0.0 && (self.fps - clip_fps).abs() > 1.0 {
            return Some(format!(
                "profile was calibrated at {:.2} fps, the clip is {clip_fps:.2} fps. \
                 That is usually a different sensor readout mode",
                self.fps
            ));
        }
        None
    }
}

/// Convenience for tests and previews: undistort a whole RGB image in one call.
pub fn undistort_rgb_image(params: &LensOnlyParams, interp: Interp, rgb: &Rgb8) -> Result<Rgb8> {
    Undistorter::new(params, interp, None)
        .process_rgb(&mut rgb.clone())
        .context("undistort failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// None here means something bypassed LensProfile::from_json, so init() never ran.
    #[test]
    fn a_parsed_profile_has_run_init() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/dji_o4_pro_stock_white_4k60.json"
        );
        let lens = LensProfileFile::load(Path::new(path)).unwrap();
        assert!(lens.lens.fisheye_params.radial_distortion_limit.is_some());
    }
}

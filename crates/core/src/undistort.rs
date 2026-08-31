// Lens-only undistortion via gyroflow-core's CPU kernel

use anyhow::{Context, Result, bail};
use gyroflow_core::gpu::{BufferDescription, BufferSource, Buffers};
use gyroflow_core::lens_profile::LensProfile;
use gyroflow_core::stabilization::{
    ComputeParams, GPU_LIST, Interpolation, PixelType, RGB8, RGBA8, Stabilization,
    distortion_models::DistortionModel,
};

/// One per clip, since clips may differ in size.
pub struct LensOnlyParams {
    pub lens: LensProfile,
    pub in_w: usize,
    pub in_h: usize,
    pub out_w: usize,
    pub out_h: usize,
}

/// Bilinear by default: under a 2:1 downscale all three register within 0.004 px of a Gyroflow app export, at 30.5 fps against Lanczos4's 18.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interp {
    Bilinear,
    Bicubic,
    Lanczos4,
}

impl Interp {
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

impl std::str::FromStr for Interp {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "bilinear" => Ok(Interp::Bilinear),
            "bicubic" => Ok(Interp::Bicubic),
            "lanczos4" | "lanczos" => Ok(Interp::Lanczos4),
            other => bail!("unknown interpolation {other:?} (bilinear|bicubic|lanczos4)"),
        }
    }
}

/// Gyroflow treats <= 0.01 as absent; matching that keeps our intrinsics equal to an app export's.
fn stretch(v: f64) -> f64 {
    if v > 0.01 { v } else { 1.0 }
}

impl LensOnlyParams {
    /// Takes raw JSON, not a path: gyroflow's `load_from_file` routes through a VFS layer we must not touch.
    pub fn new(
        profile_json: &str,
        in_w: usize,
        in_h: usize,
        out_w: usize,
        out_h: usize,
    ) -> Result<Self> {
        // Never bare serde: from_json also runs init(), computing the r_limit that culls pixels past the monotonic radius.
        let lens = LensProfile::from_json(profile_json)
            .map_err(|e| anyhow::anyhow!("lens profile parse failed: {e}"))?;
        if lens.fisheye_params.distortion_coeffs.is_empty() {
            bail!("lens profile has no distortion coefficients");
        }
        if lens.calib_dimension.w == 0 || lens.calib_dimension.h == 0 {
            bail!("lens profile has no calibration dimensions");
        }
        if in_w == 0 || in_h == 0 || out_w == 0 || out_h == 0 {
            bail!("zero input/output dimensions");
        }
        Ok(Self {
            lens,
            in_w,
            in_h,
            out_w,
            out_h,
        })
    }

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
        cp.width = self.in_w;
        cp.height = self.in_h;
        cp.output_width = self.out_w;
        cp.output_height = self.out_h;

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
        self.in_w as f64 / (self.out_w.max(1) as f64)
    }

    /// PINHOLE intrinsics of the *undistorted output*, for the manifest and cameras.txt.
    pub fn output_intrinsics(&self) -> Result<[f64; 4]> {
        let k = self.lens.get_camera_matrix((self.in_w, self.in_h), false);
        // The matrix comes back at CALIBRATION resolution; scale it as get_lens_data_at_timestamp does.
        let calib_w = self.lens.calib_dimension.w as f64;
        let calib_h = self.lens.calib_dimension.h as f64;
        if calib_w <= 0.0 || calib_h <= 0.0 {
            bail!("profile has no calibration dimensions");
        }
        let stretch_x = stretch(self.lens.input_horizontal_stretch);
        let stretch_y = stretch(self.lens.input_vertical_stretch);
        let ratio_x = self.in_w as f64 / calib_w * stretch_x;
        let ratio_y = self.in_h as f64 / calib_h * stretch_y;
        let fov = self.fov();
        // get_new_k: focals divided by fov with horizontal stretch backed out.
        let img_dim_ratio = 1.0 / stretch_x;
        Ok([
            k[(0, 0)] * ratio_x * img_dim_ratio / fov,
            k[(1, 1)] * ratio_y * img_dim_ratio / fov,
            self.out_w as f64 / 2.0,
            self.out_h as f64 / 2.0,
        ])
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
    let mut u = Undistorter::new_gpu(params, interp, device);
    let mut input = vec![0u8; params.in_w * params.in_h * 3];
    let mut out = vec![0u8; params.out_w * params.out_h * 3];
    // `process_rgb` drops the RGBA staging when it detects the fallback.
    u.process_rgb(&mut input, &mut out).is_ok() && u.rgba.is_some()
}

/// One `Stabilization` reused across a clip, so the transform matrix is computed once. Not Sync.
pub struct Undistorter {
    stab: Stabilization,
    /// GPU mode only: wgpu has no packed 3-byte texture format, so every frame is converted in and out.
    rgba: Option<(Vec<u8>, Vec<u8>)>,
    /// Verified against `info.backend` on the first frame, then trusted.
    backend_checked: bool,
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
}

impl Undistorter {
    pub fn new(params: &LensOnlyParams, interp: Interp) -> Self {
        Self::build(params, interp, -1)
    }

    /// A failed init falls back to gyroflow's CPU kernel, which `process_rgb` detects on the first frame.
    pub fn new_gpu(params: &LensOnlyParams, interp: Interp, device: usize) -> Self {
        Self::build(params, interp, device as isize)
    }

    fn build(params: &LensOnlyParams, interp: Interp, device: isize) -> Self {
        let mut stab = Stabilization::default();
        stab.set_compute_params(params.compute_params());
        stab.init_size((params.in_w, params.in_h), (params.out_w, params.out_h));
        stab.interpolation = interp.to_gf();
        // set_device(-1) => BackendType::Cpu, a real switch: no adapter or Vulkan loader is created at all.
        stab.set_device(device);
        let rgba = (device >= 0).then(|| {
            (
                vec![0u8; params.in_w * params.in_h * 4],
                vec![0u8; params.out_w * params.out_h * 4],
            )
        });
        Self {
            stab,
            rgba,
            backend_checked: false,
            in_w: params.in_w,
            in_h: params.in_h,
            out_w: params.out_w,
            out_h: params.out_h,
        }
    }

    /// `input` is `&mut` only because `BufferSource::Cpu` demands it; it is unmodified.
    pub fn process_rgb(&mut self, input: &mut [u8], out: &mut [u8]) -> Result<()> {
        anyhow::ensure!(
            input.len() == self.in_w * self.in_h * 3,
            "bad input buffer size"
        );
        anyhow::ensure!(
            out.len() == self.out_w * self.out_h * 3,
            "bad output buffer size"
        );
        let backend = if self.rgba.is_some() {
            let (rgba_in, _) = self.rgba.as_mut().unwrap();
            for (dst, src) in rgba_in.chunks_exact_mut(4).zip(input.chunks_exact(3)) {
                dst[..3].copy_from_slice(src);
            }
            let backend = self.process::<RGBA8>(4)?;
            let (_, rgba_out) = self.rgba.as_ref().unwrap();
            for (dst, src) in out.chunks_exact_mut(3).zip(rgba_out.chunks_exact(4)) {
                dst.copy_from_slice(&src[..3]);
            }
            backend
        } else {
            self.process_cpu_rgb(input, out)?
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
        Ok(())
    }

    fn process_cpu_rgb(&mut self, input: &mut [u8], out: &mut [u8]) -> Result<&'static str> {
        let mut buffers = Self::buffers(
            (self.in_w, self.in_h, self.in_w * 3),
            input,
            (self.out_w, self.out_h, self.out_w * 3),
            out,
        );
        Self::run::<RGB8>(&mut self.stab, &mut buffers)
    }

    fn process<T: PixelType>(&mut self, bpp: usize) -> Result<&'static str> {
        let (rgba_in, rgba_out) = self.rgba.as_mut().unwrap();
        let mut buffers = Self::buffers(
            (self.in_w, self.in_h, self.in_w * bpp),
            rgba_in,
            (self.out_w, self.out_h, self.out_w * bpp),
            rgba_out,
        );
        Self::run::<T>(&mut self.stab, &mut buffers)
    }

    fn buffers<'a>(
        in_size: (usize, usize, usize),
        input: &'a mut [u8],
        out_size: (usize, usize, usize),
        out: &'a mut [u8],
    ) -> Buffers<'a> {
        Buffers {
            input: BufferDescription {
                size: in_size, // w, h, stride
                rect: None,
                rotation: None,
                data: BufferSource::Cpu { buffer: input },
                texture_copy: false,
            },
            output: BufferDescription {
                size: out_size,
                rect: None,
                rotation: None,
                data: BufferSource::Cpu { buffer: out },
                texture_copy: false,
            },
        }
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
    pub calib_w: u32,
    pub calib_h: u32,
    pub fps: f64,
}

impl ProfileSummary {
    /// Every check that does not need a frame geometry, so a profile is rejected the same way everywhere.
    pub fn validate(profile_json: &str) -> Result<Self> {
        let lens = LensProfile::from_json(profile_json)
            .map_err(|e| anyhow::anyhow!("lens profile parse failed: {e}"))?;
        if lens.fisheye_params.distortion_coeffs.is_empty() {
            bail!("lens profile has no distortion coefficients");
        }
        if lens.calib_dimension.w == 0 || lens.calib_dimension.h == 0 {
            bail!("lens profile has no calibration dimensions");
        }
        Ok(Self {
            name: lens.name.clone(),
            camera: format!("{} {}", lens.camera_brand, lens.camera_model)
                .trim()
                .to_string(),
            calib_w: lens.calib_dimension.w as u32,
            calib_h: lens.calib_dimension.h as u32,
            fps: lens.fps,
        })
    }

    /// Calibration is per-aspect: the wrong variant silently undistorts to the wrong field of view.
    pub fn mismatch(&self, clip_w: u32, clip_h: u32, clip_fps: f64) -> Option<String> {
        if self.calib_w == 0 || self.calib_h == 0 || clip_w == 0 || clip_h == 0 {
            return None;
        }
        let calib_ar = self.calib_w as f64 / self.calib_h as f64;
        let clip_ar = clip_w as f64 / clip_h as f64;
        if (calib_ar - clip_ar).abs() > 0.02 {
            return Some(format!(
                "profile is calibrated for {}×{} ({calib_ar:.2}:1). The clip is {clip_w}×{clip_h} \
                 ({clip_ar:.2}:1). A different aspect means a different calibrated crop",
                self.calib_w, self.calib_h
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
pub fn undistort_rgb_image(params: &LensOnlyParams, interp: Interp, rgb: &[u8]) -> Result<Vec<u8>> {
    let mut und = Undistorter::new(params, interp);
    let mut input = rgb.to_vec();
    let mut out = vec![0u8; params.out_w * params.out_h * 3];
    und.process_rgb(&mut input, &mut out)
        .context("undistort failed")?;
    Ok(out)
}

/// Serialized as its name, the same spelling `--interpolation` takes, so a
/// manifest stays readable and the enum stays the single definition of it.
impl serde::Serialize for Interp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> serde::Deserialize<'de> for Interp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = <std::borrow::Cow<'_, str>>::deserialize(d)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

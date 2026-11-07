use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::Result;

use reconst_prep_core::cancel::CancelToken;
use reconst_prep_core::decode::{ClipInfo, Ffmpeg, HwAccel, decode_frame_at_scaled};
use reconst_prep_core::frame::{Rgb8, Size};
use reconst_prep_core::gray::resize_rgb;
use reconst_prep_core::mask::{Mask, MaskClass, MaskConfig, PreparedMask};
use reconst_prep_core::pipeline::{OutputSpec, analyze_frame, render_output};
use reconst_prep_core::undistort::{Interp, LensProfileFile, Undistorter};

/// Mask edges and sharpness are judged at 1:1, so there has to be something to zoom into.
const MAX_DISPLAY_W: u32 = 1920;

/// 16:9 whatever the footage's aspect, so the list does not reflow as rows load.
pub const THUMB_W: u32 = 192;
pub const THUMB_H: u32 = 108;

/// An image ready to become a texture: RGB8, or RGBA8 for the mask overlay.
#[derive(Clone)]
pub struct DisplayImage {
    pub size: Size,
    pub data: Vec<u8>,
}

impl From<Rgb8> for DisplayImage {
    fn from(rgb: Rgb8) -> Self {
        Self {
            size: rgb.size(),
            data: rgb.into_data(),
        }
    }
}

pub struct PreviewData {
    pub clip_stem: String,
    pub start_s: f64,
    /// Also the number in the output filenames.
    pub frame_index: u64,
    /// Fitted into the output's display box so flipping between the two keeps the frame in place.
    pub source: DisplayImage,
    /// The frame as it will be written.
    pub output: DisplayImage,
    pub masked_fraction: f64,
    /// Always computed, so toggling the blur filter never rebuilds the preview. `None` until the final step has run.
    pub sharpness: Option<f64>,
}

/// Resolved once, on first use and off the UI thread: resolving launches ffmpeg and ffprobe.
#[derive(Clone, Default)]
pub struct SharedFfmpeg(Arc<OnceLock<Result<Ffmpeg, String>>>);

impl SharedFfmpeg {
    pub fn get(&self) -> Result<&Ffmpeg> {
        self.0
            .get_or_init(|| Ffmpeg::resolve(None).map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// The mask last prepared for a preview, so a slider tweak does not reload the model.
#[derive(Clone, Default)]
pub struct MaskCache(Arc<Mutex<Option<PreparedMask>>>);

impl MaskCache {
    fn get(&self, cfg: &MaskConfig, frame: Size) -> Result<PreparedMask> {
        let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mask = match &*slot {
            Some(prev) => prev.reconfigure(cfg, frame)?,
            None => cfg.prepare(frame)?,
        };
        *slot = Some(mask.clone());
        Ok(mask)
    }
}

/// Equality decides whether a settings change needs a new frame, so nothing that leaves the frame unchanged belongs in here.
#[derive(Clone, PartialEq)]
pub struct PreviewRequest {
    /// As probed, so the preview need not probe again.
    pub clip: ClipInfo,
    pub profile: Option<std::path::PathBuf>,
    /// Position in the clip, 0..1.
    pub pos: f64,
    pub output_spec: OutputSpec,
    pub interp: Interp,
    pub hwaccel: HwAccel,
    pub mask: MaskConfig,
}

/// Matching the dots beside the toggles in the Masking category.
pub fn class_color(class: MaskClass) -> [u8; 3] {
    match class {
        MaskClass::Sky => [64, 200, 235],    // cyan
        MaskClass::People => [255, 150, 60], // orange
    }
}

/// `None` when neither duration nor frame count is known: callers differ on the fallback, so it stays with them.
pub fn duration_of(info: &ClipInfo) -> Option<f64> {
    info.duration_s
        .or_else(|| info.frames.map(|f| f as f64 / info.fps.max(1.0)))
}

/// Cover and crop, never stretch: fitting-and-padding would put before and after at different scales.
fn fit_into(rgb: &Rgb8, frame: Size) -> Result<DisplayImage> {
    let size = rgb.size();
    let scale = (frame.w as f64 / size.w as f64).max(frame.h as f64 / size.h as f64);
    let fill = |v: u32, min: u32| ((v as f64 * scale).round() as u32).max(min);
    let scaled_size = Size::new(fill(size.w, frame.w), fill(size.h, frame.h));
    let scaled = resize_rgb(rgb, scaled_size)?;
    if scaled_size == frame {
        return Ok(scaled.into());
    }
    let (row, scaled_row) = (frame.w as usize * 3, scaled_size.w as usize * 3);
    let (ox, oy) = (
        (scaled_size.w - frame.w) as usize / 2,
        (scaled_size.h - frame.h) as usize / 2,
    );
    let mut out = vec![0u8; frame.px() * 3];
    for (y, dst) in out.chunks_exact_mut(row).enumerate() {
        let src = (y + oy) * scaled_row + ox * 3;
        dst.copy_from_slice(&scaled.data()[src..src + row]);
    }
    Ok(DisplayImage {
        size: frame,
        data: out,
    })
}

/// Display size for the output frame: capped, and even.
fn display_size(size: Size) -> Size {
    if size.w <= MAX_DISPLAY_W {
        return size;
    }
    let capped = size.at_width(MAX_DISPLAY_W);
    Size::new(capped.w, capped.h & !1)
}

/// Each class tints its own pixels its own colour, later classes over earlier ones.
fn build_overlay(layers: &[(MaskClass, Mask)], size: Size) -> Option<DisplayImage> {
    if layers.is_empty() {
        return None;
    }
    let mut rgba = vec![0u8; size.px() * 4];
    for (class, mask) in layers {
        let scaled = mask.scaled_to(size);
        let c = class_color(*class);
        for (px, &valid) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&scaled.data) {
            if valid == 0 {
                px[0] = c[0];
                px[1] = c[1];
                px[2] = c[2];
                // Weak enough that the mask edge can still be judged against the image under it.
                px[3] = 110;
            }
        }
    }
    Some(DisplayImage { size, data: rgba })
}

/// Every update repaints the pane, so the preview assembles on screen instead of appearing complete after the slowest step.
pub enum PreviewUpdate {
    /// The decoded frame, untouched: what Before shows.
    Raw(Box<PreviewData>),
    /// An expensive step has started; the header names it beside a spinner.
    Stage(&'static str),
    /// Undistorted through the lens profile, or the plain resize.
    Rendered(DisplayImage),
    /// The last step: the mask overlay and the numbers that came with it.
    Finished {
        overlay: Option<DisplayImage>,
        masked_fraction: f64,
        sharpness: f64,
    },
}

pub fn build_preview(
    req: &PreviewRequest,
    ff: &SharedFfmpeg,
    masks: &MaskCache,
    cancel: &CancelToken,
    emit: &mut dyn FnMut(PreviewUpdate),
) -> Result<()> {
    let t0 = Instant::now();
    let ff = ff.get()?;
    let info = &req.clip;
    let duration_s = duration_of(info).unwrap_or(10.0);
    let start_s = req.pos.clamp(0.0, 0.98) * duration_s;
    let out_size = req.output_spec.resolve(info.size)?;

    let frame = decode_frame_at_scaled(ff, &info.path, &req.hwaccel, start_s, None)?;
    anyhow::ensure!(
        frame.rgb.size() == info.size,
        "frame size {} does not match probed {}",
        frame.rgb.size(),
        info.size,
    );
    let t_decode = t0.elapsed().as_secs_f64();

    // The raw frame goes up before any work starts, as both Before and the stand-in result.
    let display = display_size(out_size);
    let source = fit_into(&frame.rgb, display)?;
    emit(PreviewUpdate::Raw(Box::new(PreviewData {
        clip_stem: info.stem.clone(),
        start_s,
        frame_index: (start_s * info.fps).round() as u64,
        source: source.clone(),
        output: source,
        masked_fraction: 0.0,
        sharpness: None,
    })));
    let superseded = || cancel.is_cancelled();

    // With no profile this is the plain resize, or at native size the decoded buffer itself.
    let params = req
        .profile
        .as_deref()
        .map(|p| LensProfileFile::load(p)?.params(info.size, out_size))
        .transpose()?;
    if params.is_some() {
        emit(PreviewUpdate::Stage("undistorting"));
    }
    let mut undistorter = params
        .as_ref()
        .map(|p| Undistorter::new(p, req.interp, None));
    let out = render_output(frame.rgb, out_size, undistorter.as_mut())?;
    anyhow::ensure!(!superseded(), "preview superseded");
    emit(PreviewUpdate::Rendered(if display == out_size {
        out.clone().into()
    } else {
        resize_rgb(&out, display)?.into()
    }));
    let t_render = t0.elapsed().as_secs_f64();

    if req.mask.is_active() {
        emit(PreviewUpdate::Stage("masking"));
    }
    // Loading the model here makes the preview where a bad model is found, not the start of a long run.
    let mask = masks.get(&req.mask, out_size)?;
    // The same metric thumbnail and judgement the run uses, so the readouts here are the run's numbers.
    let mut a = analyze_frame(&out, &mask)?;
    anyhow::ensure!(!superseded(), "preview superseded");
    let judged = mask.judge(&a.small, &mut a.gray, a.seg.as_ref());
    emit(PreviewUpdate::Finished {
        overlay: build_overlay(&judged.layers, display),
        masked_fraction: judged.mask.map_or(0.0, |m| 1.0 - m.sfm.valid_fraction()),
        sharpness: judged.sharpness,
    });
    log::info!(
        "preview ready in {:.1}s (decode {t_decode:.1}s, render {:.1}s, mask {:.1}s)",
        t0.elapsed().as_secs_f64(),
        t_render - t_decode,
        t0.elapsed().as_secs_f64() - t_render,
    );
    Ok(())
}

/// Taken a tenth of the way in, not at frame zero, where drone footage is still on the ground or in a black lead-in.
pub fn build_thumbnail(
    path: &std::path::Path,
    duration_s: f64,
    ff: &SharedFfmpeg,
) -> Result<DisplayImage> {
    // Software decode deliberately: one 4K HEVC frame is 1.09 s on the CPU against 1.20 s through VAAPI, whose setup dwarfs it.
    let hwaccel = HwAccel::None;
    // A little more than we draw: `fit_into` still crops 4:3 to 16:9.
    let f = decode_frame_at_scaled(
        ff.get()?,
        path,
        &hwaccel,
        duration_s * 0.1,
        Some(THUMB_W.max(THUMB_H * 16 / 9)),
    )?;
    fit_into(&f.rgb, Size::new(THUMB_W, THUMB_H))
}

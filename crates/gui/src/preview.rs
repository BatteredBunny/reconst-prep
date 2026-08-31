use std::time::Instant;

use anyhow::Result;

use reconst_prep_core::cancel::CancelToken;
use reconst_prep_core::decode::{ClipInfo, Ffmpeg, HwAccel, decode_frame_at_scaled, probe_clip};
use reconst_prep_core::gray::resize_rgb;
use reconst_prep_core::mask::{Mask, MaskClass, MaskConfig};
use reconst_prep_core::pipeline::{OutputSpec, analyze_frame, mask_and_sharpness, render_output};
use reconst_prep_core::undistort::{Interp, LensOnlyParams, Undistorter};

/// Mask edges and sharpness are judged at 1:1, so there has to be something to zoom into.
const MAX_DISPLAY_W: u32 = 1920;

/// 16:9 whatever the footage's aspect, so the list does not reflow as rows load.
pub const THUMB_W: u32 = 192;
pub const THUMB_H: u32 = 108;

/// An image ready to become a texture.
#[derive(Clone)]
pub struct DisplayImage {
    pub rgb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub struct PreviewData {
    pub clip_stem: String,
    pub start_s: f64,
    /// Also the number in the output filenames.
    pub frame_index: u64,
    /// Fitted into the output's display box so the two can be wiped against each other in one rect.
    pub source: DisplayImage,
    /// The frame as it will be written.
    pub output: DisplayImage,
    pub masked_fraction: f64,
    /// Always computed, so toggling the blur filter never rebuilds the preview. `None` until the final step has run.
    pub sharpness: Option<f64>,
}

/// Equality decides whether a settings change needs a new frame, so nothing that leaves the frame unchanged belongs in here.
#[derive(Clone, PartialEq)]
pub struct PreviewRequest {
    pub path: std::path::PathBuf,
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

/// Cover and crop, never stretch: fitting-and-padding would put the two sides of the wipe at different scales.
fn fit_into(rgb: &[u8], w: u32, h: u32, box_w: u32, box_h: u32) -> Result<DisplayImage> {
    let scale = (box_w as f64 / w as f64).max(box_h as f64 / h as f64);
    let (sw, sh) = (
        ((w as f64 * scale).round() as u32).max(box_w),
        ((h as f64 * scale).round() as u32).max(box_h),
    );
    let scaled = resize_rgb(rgb, w, h, sw, sh)?;
    if (sw, sh) == (box_w, box_h) {
        return Ok(DisplayImage {
            rgb: scaled,
            w: box_w,
            h: box_h,
        });
    }
    let mut out = vec![0u8; (box_w as usize) * (box_h as usize) * 3];
    let (ox, oy) = ((sw - box_w) / 2, (sh - box_h) / 2);
    for y in 0..box_h {
        let dst = (y as usize) * box_w as usize * 3;
        let src = (((y + oy) as usize) * sw as usize + ox as usize) * 3;
        out[dst..dst + box_w as usize * 3].copy_from_slice(&scaled[src..src + box_w as usize * 3]);
    }
    Ok(DisplayImage {
        rgb: out,
        w: box_w,
        h: box_h,
    })
}

/// Display size for the output frame: capped, and even.
fn display_size(w: u32, h: u32) -> (u32, u32) {
    if w <= MAX_DISPLAY_W {
        return (w, h);
    }
    let dw = MAX_DISPLAY_W;
    let dh = ((h as u64 * dw as u64 / w as u64).max(1) as u32) & !1;
    (dw, dh)
}

/// Each class tints its own pixels its own colour, later classes over earlier ones.
fn build_overlay(layers: &[(MaskClass, Mask)], w: u32, h: u32) -> Option<DisplayImage> {
    if layers.is_empty() {
        return None;
    }
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    for (class, mask) in layers {
        let scaled = mask.scaled_to(w, h);
        let c = class_color(*class);
        for (px, &valid) in rgba.chunks_exact_mut(4).zip(&scaled.data) {
            if valid == 0 {
                px[0] = c[0];
                px[1] = c[1];
                px[2] = c[2];
                // Weak enough that the mask edge can still be judged against the image under it.
                px[3] = 110;
            }
        }
    }
    Some(DisplayImage { rgb: rgba, w, h })
}

/// Every update repaints the pane, so the preview assembles on screen instead of appearing complete after the slowest step.
pub enum PreviewUpdate {
    /// The decoded frame, untouched, on both sides of the wipe.
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
    cancel: &CancelToken,
    emit: &mut dyn FnMut(PreviewUpdate),
) -> Result<()> {
    let t0 = Instant::now();
    let ff = Ffmpeg::resolve(None)?;
    let info = probe_clip(&ff, &req.path)?;
    let duration_s = duration_of(&info).unwrap_or(10.0);
    let start_s = req.pos.clamp(0.0, 0.98) * duration_s;
    let (in_w, in_h) = (info.width, info.height);
    let (out_w, out_h) = req.output_spec.resolve(in_w, in_h)?;

    let mut frame = decode_frame_at_scaled(&ff, &req.path, &req.hwaccel, start_s, None)?;
    anyhow::ensure!(
        (frame.width, frame.height) == (in_w, in_h),
        "frame size {}x{} does not match probed {in_w}x{in_h}",
        frame.width,
        frame.height,
    );
    let t_decode = t0.elapsed().as_secs_f64();

    // The raw frame goes up before any work starts, on both sides of the wipe.
    let (dw, dh) = display_size(out_w as u32, out_h as u32);
    let source = fit_into(&frame.rgb, in_w, in_h, dw, dh)?;
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
        .as_ref()
        .map(|p| -> Result<_> {
            let json = std::fs::read_to_string(p)?;
            LensOnlyParams::new(&json, in_w as usize, in_h as usize, out_w, out_h)
        })
        .transpose()?;
    if params.is_some() {
        emit(PreviewUpdate::Stage("undistorting"));
    }
    let mut undistorter = params.as_ref().map(|p| Undistorter::new(p, req.interp));
    let out = render_output(
        &mut frame.rgb,
        (in_w, in_h),
        (out_w, out_h),
        undistorter.as_mut(),
    )?;
    anyhow::ensure!(!superseded(), "preview superseded");
    emit(PreviewUpdate::Rendered(
        if (dw, dh) == (out_w as u32, out_h as u32) {
            DisplayImage {
                rgb: out.clone(),
                w: dw,
                h: dh,
            }
        } else {
            DisplayImage {
                rgb: resize_rgb(&out, out_w as u32, out_h as u32, dw, dh)?,
                w: dw,
                h: dh,
            }
        },
    ));
    let t_render = t0.elapsed().as_secs_f64();

    if req.mask.is_active() {
        emit(PreviewUpdate::Stage("masking"));
    }
    // Loading the model here makes the preview where a bad --seg-model is found, not the start of a long run.
    let mask_cfg = req.mask.prepare(out_w as u32, out_h as u32)?;
    // The same metric thumbnail the run uses, so the readouts here are the run's numbers.
    let a = analyze_frame(&out, out_w as u32, out_h as u32, &mask_cfg)?;
    anyhow::ensure!(!superseded(), "preview superseded");
    let mut gray = a.gray;
    // Per class, so the overlay can tint each source its own colour. Before masking narrows `gray` to the SfM set.
    let layers = if mask_cfg.is_active() {
        mask_cfg.class_layers(&a.small, &gray, a.seg.as_ref())
    } else {
        Vec::new()
    };
    let (mask, sharpness) = mask_and_sharpness(&mask_cfg, &a.small, &mut gray, a.seg.as_ref());
    emit(PreviewUpdate::Finished {
        overlay: build_overlay(&layers, dw, dh),
        masked_fraction: mask.map_or(0.0, |m| 1.0 - m.sfm.valid_fraction()),
        sharpness,
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
pub fn build_thumbnail(path: &std::path::Path, duration_s: f64) -> Result<DisplayImage> {
    // Software decode deliberately: one 4K HEVC frame is 1.09 s on the CPU against 1.20 s through VAAPI, whose setup dwarfs it.
    let hwaccel = HwAccel::None;
    let ff = Ffmpeg::resolve(None)?;
    // A little more than we draw: `fit_into` still crops 4:3 to 16:9.
    let f = decode_frame_at_scaled(
        &ff,
        path,
        &hwaccel,
        duration_s * 0.1,
        Some(THUMB_W.max(THUMB_H * 16 / 9)),
    )?;
    fit_into(&f.rgb, f.width, f.height, THUMB_W, THUMB_H)
}

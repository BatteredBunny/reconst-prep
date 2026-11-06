// Selection math runs on a 480 px grayscale thumbnail, so its cost is negligible next to decode and undistort.

use anyhow::{Context, Result};
use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::frame::{Rgb8, Size};

/// Width of the grayscale thumbnails selection metrics are computed on.
pub const METRIC_WIDTH: u32 = 480;

/// Small grayscale frame for metric computation.
#[derive(Clone)]
pub struct GrayFrame {
    pub size: Size,
    pub data: Vec<u8>,
    /// Set when masking is on; both metrics below skip masked-out regions.
    pub valid: Option<Vec<u8>>,
}

impl GrayFrame {
    /// Both metrics then skip pixels where `valid` is 0.
    pub fn restrict_to(&mut self, valid: &[u8]) {
        debug_assert_eq!(valid.len(), self.data.len());
        self.valid = Some(valid.to_vec());
    }

    #[inline]
    fn is_valid(&self, i: usize) -> bool {
        match &self.valid {
            Some(v) => v[i] != 0,
            None => true,
        }
    }
}

pub fn resize_rgb(src: &Rgb8, to: Size) -> Result<Rgb8> {
    resize_rgb_with(src, to, FilterType::Lanczos3)
}

fn resize_rgb_with(src: &Rgb8, to: Size, filter: FilterType) -> Result<Rgb8> {
    let from = src.size();
    let src_img =
        ImageRef::new(from.w, from.h, src.data(), PixelType::U8x3).context("source image view")?;
    let mut dst = Image::new(to.w, to.h, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src_img,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
        )
        .context("resize")?;
    Rgb8::new(to, dst.into_vec())
}

/// Never interpolate labels: averaging categorical data invents half-valid pixels and boundary classes.
pub fn nn_scale(src: &[u8], from: Size, to: Size) -> Vec<u8> {
    let (Size { w: sw, h: sh }, Size { w: dw, h: dh }) = (from, to);
    let (sw_u, sh_u) = (sw as u64, sh as u64);
    let (dw_u, dh_u) = (dw as usize, dh as usize);
    // One division per output column instead of per pixel; same floor and clamp.
    let cols: Vec<usize> = (0..dw)
        .map(|x| (x as u64 * sw_u / dw.max(1) as u64).min(sw_u.saturating_sub(1)) as usize)
        .collect();
    let mut out = vec![0u8; dw_u * dh_u];
    let mut prev: Option<usize> = None;
    for y in 0..dh_u {
        let sy = (y as u64 * sh_u / dh.max(1) as u64).min(sh_u.saturating_sub(1)) as usize;
        let (done, rest) = out.split_at_mut(y * dw_u);
        let dst = &mut rest[..dw_u];
        match prev {
            // Upscaling repeats source rows; copying the row already built is cheaper than gathering it again.
            Some(p) if p == sy => dst.copy_from_slice(&done[(y - 1) * dw_u..]),
            _ => {
                let row = sy * sw as usize;
                for (d, &sx) in dst.iter_mut().zip(&cols) {
                    *d = src[row + sx];
                }
            }
        }
        prev = Some(sy);
    }
    out
}

/// RGB8 -> grayscale (BT.601 integer approximation, plenty for metrics).
pub fn rgb_to_gray(rgb: &Rgb8) -> GrayFrame {
    let data = rgb
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| ((77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8) as u8)
        .collect();
    GrayFrame {
        size: rgb.size(),
        data,
        valid: None,
    }
}

/// Bilinear: cheaper than Lanczos and enough for metrics.
pub fn metric_rgb(rgb: &Rgb8) -> Result<std::borrow::Cow<'_, Rgb8>> {
    if rgb.size().w <= METRIC_WIDTH {
        return Ok(std::borrow::Cow::Borrowed(rgb));
    }
    let to = rgb.size().at_width(METRIC_WIDTH);
    resize_rgb_with(rgb, to, FilterType::Bilinear).map(std::borrow::Cow::Owned)
}

/// A masked pixel counts only when its whole 3x3 neighbourhood is valid: sampling across a boundary reads it as a hard edge.
pub fn laplacian_variance(g: &GrayFrame) -> f64 {
    let (w, h) = g.size.as_usize();
    if w < 3 || h < 3 {
        return 0.0;
    }
    let d = &g.data;
    let mut sum = 0.0f64;
    let mut sum2 = 0.0f64;
    let mut n = 0u64;
    for y in 1..h - 1 {
        let row = y * w;
        for x in 1..w - 1 {
            let i = row + x;
            // `is_some` is not redundant: it keeps the unmasked path at one check per pixel instead of five.
            if g.valid.is_some()
                && !(g.is_valid(i)
                    && g.is_valid(i - 1)
                    && g.is_valid(i + 1)
                    && g.is_valid(i - w)
                    && g.is_valid(i + w))
            {
                continue;
            }
            let c = d[i] as i32;
            let lap =
                (d[i - 1] as i32 + d[i + 1] as i32 + d[i - w] as i32 + d[i + w] as i32) - 4 * c;
            let v = lap as f64;
            sum += v;
            sum2 += v * v;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    let n = n as f64;
    let mean = sum / n;
    sum2 / n - mean * mean
}

/// Pixels masked in either frame are skipped, so a hovering drone under a windy sky does not read as movement.
pub fn mean_abs_diff(a: &GrayFrame, b: &GrayFrame) -> f64 {
    debug_assert_eq!(a.data.len(), b.data.len());
    let n = a.data.len().min(b.data.len());
    if n == 0 {
        return 0.0;
    }
    let mut sum = 0u64;
    let mut counted = 0u64;
    for i in 0..n {
        if !a.is_valid(i) || !b.is_valid(i) {
            continue;
        }
        sum += (a.data[i] as i32 - b.data[i] as i32).unsigned_abs() as u64;
        counted += 1;
    }
    if counted == 0 {
        return 0.0;
    }
    sum as f64 / (counted as f64 * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fine detail, so resampling actually has something to lose.
    fn checkerboard(w: u32, h: u32) -> Rgb8 {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 3 + y / 3).is_multiple_of(2) {
                    230
                } else {
                    25
                };
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        Rgb8::new(Size::new(w, h), rgb).unwrap()
    }

    /// The thumbnail derives from the OUTPUT frame, so the same clip at 0.25 and 0.5 kept 41 frames both times, 3 of them different.
    #[test]
    fn metric_thumbnails_depend_on_the_output_size() {
        let src = checkerboard(1920, 1440);
        let big = resize_rgb(&src, Size::new(1280, 960)).unwrap();
        let small = resize_rgb(&src, Size::new(960, 720)).unwrap();

        let a = metric_gray(&big);
        let b = metric_gray(&small);
        assert_eq!(a.size, b.size, "same thumbnail geometry");
        assert_ne!(
            a.data, b.data,
            "if this ever passes, selection became size-independent and \
             --frames-from could stop being mandatory"
        );

        let (sa, sb) = (laplacian_variance(&a), laplacian_variance(&b));
        assert!(
            (sa - sb).abs() > f64::EPSILON,
            "sharpness {sa} vs {sb} must differ for the claim above to bite"
        );
    }

    /// The metric thumbnail exactly as the pipeline derives it.
    fn metric_gray(rgb: &Rgb8) -> GrayFrame {
        rgb_to_gray(&metric_rgb(rgb).unwrap())
    }

    #[test]
    fn masked_pixels_are_skipped_by_both_metrics() {
        let size = Size::new(4, 4);
        let mut a = rgb_to_gray(&Rgb8::new(size, vec![0; 3 * 16]).unwrap());
        let b = rgb_to_gray(&Rgb8::new(size, vec![255; 3 * 16]).unwrap());
        assert!((mean_abs_diff(&a, &b) - 1.0).abs() < 1e-9);
        // Masking everything leaves nothing to compare, not a false zero.
        a.valid = Some(vec![0; 16]);
        assert_eq!(mean_abs_diff(&a, &b), 0.0);
    }
}

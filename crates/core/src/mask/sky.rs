// The sky heuristic: a flood fill from the top edge, and nothing about what
// the resulting mask is then used for.

use crate::gray::GrayFrame;

use super::Mask;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SkyParams {
    /// Minimum luma for a pixel to be a candidate.
    pub luma_min: u8,
    /// Minimum `blue - red`; negative admits grey overcast, higher catches only clear blue.
    pub blue_bias: i16,
    /// Region growing stops at a luma step above this, so the fill cannot leak past the horizon.
    pub gradient_max: u8,
    /// Grow the masked region by this many pixels at metric resolution.
    pub dilate: u32,
}

impl Default for SkyParams {
    fn default() -> Self {
        Self {
            luma_min: 130,
            blue_bias: -8,
            gradient_max: 14,
            dilate: 3,
        }
    }
}

/// Connectivity to the top edge is what spares bright roofs and water; a missing horizon fails visibly.
pub fn sky_mask(rgb: &[u8], gray: &GrayFrame, p: SkyParams) -> Mask {
    let (w, h) = (gray.w as usize, gray.h as usize);
    let mut mask = Mask::all_valid(gray.w, gray.h);
    if w == 0 || h == 0 || rgb.len() < w * h * 3 {
        return mask;
    }

    let candidate = |i: usize| -> bool {
        let px = &rgb[i * 3..i * 3 + 3];
        gray.data[i] >= p.luma_min && (px[2] as i16 - px[0] as i16) >= p.blue_bias
    };

    // `mask` doubles as the visited set: a pixel is enqueued exactly when it is set to 0.
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for x in 0..w {
        if candidate(x) {
            mask.data[x] = 0;
            queue.push_back(x);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        let luma = gray.data[i] as i16;
        let mut visit = |n: usize, queue: &mut std::collections::VecDeque<usize>| {
            if mask.data[n] == 0 || !candidate(n) {
                return;
            }
            if (gray.data[n] as i16 - luma).abs() > p.gradient_max as i16 {
                return;
            }
            mask.data[n] = 0;
            queue.push_back(n);
        };
        if x > 0 {
            visit(i - 1, &mut queue);
        }
        if x + 1 < w {
            visit(i + 1, &mut queue);
        }
        if y > 0 {
            visit(i - w, &mut queue);
        }
        if y + 1 < h {
            visit(i + w, &mut queue);
        }
    }

    mask.grow_invalid(p.dilate);
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gray::rgb_to_gray;

    fn sky_mask_from_rgb(rgb: &[u8], w: u32, h: u32, p: SkyParams) -> Mask {
        let gray = rgb_to_gray(rgb, w, h);
        sky_mask(rgb, &gray, p)
    }

    /// Bright blue top half, dark ground bottom half.
    fn synthetic(w: u32, h: u32) -> Vec<u8> {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for _ in 0..w {
                if y < h / 2 {
                    rgb.extend_from_slice(&[150, 180, 230]);
                } else {
                    rgb.extend_from_slice(&[60, 70, 40]);
                }
            }
        }
        rgb
    }

    #[test]
    fn sky_mask_takes_the_top_and_stops_at_the_horizon() {
        let (w, h) = (64u32, 64u32);
        let rgb = synthetic(w, h);
        let m = sky_mask_from_rgb(
            &rgb,
            w,
            h,
            SkyParams {
                dilate: 0,
                ..Default::default()
            },
        );
        assert_eq!(m.data[0], 0, "top-left should be masked as sky");
        let bottom = (h as usize - 1) * w as usize;
        assert_eq!(m.data[bottom], 255, "ground must survive");
        let frac = m.valid_fraction();
        assert!((0.45..=0.55).contains(&frac), "valid fraction {frac}");
    }

    #[test]
    fn bright_ground_not_connected_to_the_top_is_kept() {
        let (w, h) = (32u32, 32u32);
        let mut rgb = synthetic(w, h);
        // A bright blue patch in the lower half, isolated from the sky.
        for y in 24..28 {
            for x in 8..16 {
                let i = ((y * w + x) * 3) as usize;
                rgb[i..i + 3].copy_from_slice(&[150, 180, 230]);
            }
        }
        let m = sky_mask_from_rgb(
            &rgb,
            w,
            h,
            SkyParams {
                dilate: 0,
                ..Default::default()
            },
        );
        assert_eq!(m.data[(26 * w + 12) as usize], 255);
    }

    #[test]
    fn dilate_grows_the_masked_region() {
        let (w, h) = (32u32, 32u32);
        let rgb = synthetic(w, h);
        let tight = sky_mask_from_rgb(
            &rgb,
            w,
            h,
            SkyParams {
                dilate: 0,
                ..Default::default()
            },
        );
        let grown = sky_mask_from_rgb(
            &rgb,
            w,
            h,
            SkyParams {
                dilate: 2,
                ..Default::default()
            },
        );
        assert!(grown.valid_fraction() < tight.valid_fraction());
    }
}

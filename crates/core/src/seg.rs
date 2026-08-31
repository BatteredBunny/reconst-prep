// Weights are never downloaded, so the user's model is unknown and is inspected at load time rather than trusted.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use tract_onnx::prelude::*;

use crate::gray::{nn_scale, resize_rgb};
use crate::mask::{Mask, MaskClass};

/// `tract`'s error is neither Send nor Sync, so it cannot become an `anyhow::Error` directly.
trait TractExt<T> {
    fn tract(self) -> Result<T>;
}

impl<T> TractExt<T> for TractResult<T> {
    fn tract(self) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// Segmentation gets its own downscale: in the 480 px metric thumbnail a bystander in 4K vanishes into the resample.
pub const DEFAULT_SEG_WIDTH: u32 = 640;

/// Default temporal-mode window: 1 = off.
pub const DEFAULT_TEMPORAL_WINDOW: u32 = 1;

/// Exports with an extra background channel shift these by one; `SegModel` infers that from the channel count.
pub const ADE20K_SKY: usize = 2;
pub const ADE20K_PERSON: usize = 12;

/// ImageNet normalization, as every ADE20K export was trained; anything else yields nonsense rather than an error.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// What to take out of the segmentation model, and how hard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SegClassParams {
    /// Label id in the model's output; defaults come from ADE20K.
    pub class_id: usize,
    /// Grow the masked region by this many pixels, at inference resolution.
    pub dilate: u32,
}

impl SegClassParams {
    /// Dilate differs by class: a sky edge is a soft gradient, a moving limb smears over ten pixels.
    pub fn sky() -> Self {
        Self {
            class_id: ADE20K_SKY,
            dilate: 3,
        }
    }
    pub fn people() -> Self {
        Self {
            class_id: ADE20K_PERSON,
            dilate: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SegConfig {
    /// Rounded up to a multiple of 32, which every encoder-decoder segmentation net needs.
    pub width: u32,
    pub sky: Option<SegClassParams>,
    pub people: Option<SegClassParams>,
    /// Costs `window / 2` frames of lag; 1 disables it.
    pub temporal_window: u32,
}

impl Default for SegConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_SEG_WIDTH,
            sky: None,
            people: None,
            temporal_window: 1,
        }
    }
}

impl SegConfig {
    pub fn is_active(&self) -> bool {
        self.sky.is_some() || self.people.is_some()
    }

    /// The classes taken from the model, in a fixed order.
    pub fn classes(&self) -> Vec<(MaskClass, SegClassParams)> {
        let mut out = Vec::new();
        if let Some(p) = self.sky {
            out.push((MaskClass::Sky, p));
        }
        if let Some(p) = self.people {
            out.push((MaskClass::People, p));
        }
        out
    }
}

/// One frame's per-pixel label map at inference resolution.
#[derive(Clone, Debug)]
pub struct SegLabels {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// `into_runnable` hands back an `Arc`ed plan, so every worker shares one copy of the weights.
type Plan = std::sync::Arc<TypedSimplePlan>;

/// Bound to one input geometry. Stateless, so one instance is shared by every worker.
pub struct SegModel {
    plan: Plan,
    /// Inference input size, (w, h).
    size: (u32, u32),
    /// Number of label channels the model emits.
    n_classes: usize,
    /// Some exports prepend a background channel, shifting every ADE20K id.
    class_offset: usize,
    cfg: SegConfig,
}

impl std::fmt::Debug for SegModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SegModel({}x{}, {} classes, id offset {})",
            self.size.0, self.size.1, self.n_classes, self.class_offset
        )
    }
}

impl SegModel {
    /// Only the frame aspect is used, to pick a height for the configured width.
    pub fn load(path: &Path, cfg: &SegConfig, frame_w: u32, frame_h: u32) -> Result<Self> {
        ensure!(frame_w > 0 && frame_h > 0, "zero frame size");
        let want_w = round_up_32(cfg.width.max(32));
        let want_h =
            round_up_32(((frame_h as u64 * want_w as u64) / frame_w as u64).max(32) as u32);

        let model = tract_onnx::onnx()
            .model_for_path(path)
            .tract()
            .with_context(|| format!("reading ONNX model {}", path.display()))?;

        // Honour a declared fixed input size: the user cannot change one without re-exporting.
        let declared = model
            .input_fact(0)
            .ok()
            .and_then(|f| f.shape.as_concrete_finite().ok().flatten())
            .filter(|s| s.len() == 4);
        let (in_w, in_h) = match declared.as_deref() {
            Some([_, _, h, w]) if *h > 1 && *w > 1 => (*w as u32, *h as u32),
            _ => (want_w, want_h),
        };

        let plan = model
            .with_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec!(1usize, 3, in_h as usize, in_w as usize),
                ),
            )
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .tract()
            .with_context(|| {
                format!(
                    "{} would not accept a 1x3x{in_h}x{in_w} float input. \
                     Is it an image segmentation model?",
                    path.display()
                )
            })?;

        // Run once now so a mismatched model fails at start-up rather than 30 minutes into a run.
        let probe = Tensor::zero::<f32>(&[1, 3, in_h as usize, in_w as usize]).tract()?;
        let out = plan
            .run(tvec!(probe.into()))
            .tract()
            .with_context(|| format!("first inference with {} failed", path.display()))?;
        let shape = out.first().map(|t| t.shape().to_vec()).unwrap_or_default();
        if shape.len() != 4 || shape[0] != 1 {
            bail!(
                "{}: expected a 1xCxHxW logit output, got {shape:?}",
                path.display()
            );
        }
        let n_classes = shape[1];
        // 151 = ADE20K with a background channel prepended, shifting every id.
        let class_offset = usize::from(n_classes == 151);
        let max_id = cfg
            .classes()
            .iter()
            .map(|(_, p)| p.class_id + class_offset)
            .max()
            .unwrap_or(0);
        if max_id >= n_classes {
            bail!(
                "{}: the model emits {n_classes} classes, but class id {max_id} was asked for. \
                 Pass the right id for this model's label set.",
                path.display()
            );
        }

        Ok(Self {
            plan,
            size: (in_w, in_h),
            n_classes,
            class_offset,
            cfg: cfg.clone(),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn n_classes(&self) -> usize {
        self.n_classes
    }

    /// Stateless: safe to call from several workers at once.
    pub fn labels(&self, rgb: &[u8], w: u32, h: u32) -> Result<SegLabels> {
        let (iw, ih) = self.size;
        ensure!(
            rgb.len() >= (w as usize) * (h as usize) * 3,
            "segmentation input buffer too small"
        );
        let scaled = if (w, h) == (iw, ih) {
            std::borrow::Cow::Borrowed(rgb)
        } else {
            std::borrow::Cow::Owned(resize_rgb(rgb, w, h, iw, ih)?)
        };

        let (iw_u, ih_u) = (iw as usize, ih as usize);
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, ih_u, iw_u), |(_, c, y, x)| {
            let v = scaled[(y * iw_u + x) * 3 + c] as f32 / 255.0;
            (v - MEAN[c]) / STD[c]
        });
        let out = self
            .plan
            .run(tvec!(Tensor::from(input).into()))
            .tract()
            .context("segmentation inference failed")?;
        let logits = out
            .first()
            .context("segmentation model produced no output")?
            .to_plain_array_view::<f32>()
            .tract()?;
        let shape = logits.shape();
        ensure!(
            shape.len() == 4 && shape[1] == self.n_classes,
            "segmentation output changed shape mid-run: {shape:?}"
        );
        let (oh, ow) = (shape[2], shape[3]);

        // argmax at the output stride before upsampling: upsampling logits first invents a third class along every boundary.
        let mut small = vec![0u8; oh * ow];
        for y in 0..oh {
            for x in 0..ow {
                let mut best = 0usize;
                let mut best_v = f32::NEG_INFINITY;
                for c in 0..self.n_classes {
                    let v = logits[[0, c, y, x]];
                    if v > best_v {
                        best_v = v;
                        best = c;
                    }
                }
                small[y * ow + x] = best.min(u8::MAX as usize) as u8;
            }
        }
        let data = if (ow, oh) == (iw_u, ih_u) {
            small
        } else {
            nn_scale(&small, ow as u32, oh as u32, iw, ih)
        };
        Ok(SegLabels { w: iw, h: ih, data })
    }

    /// `0` = ignore, matching `Mask`.
    pub fn masks(&self, labels: &SegLabels) -> Vec<(MaskClass, Mask)> {
        self.cfg
            .classes()
            .into_iter()
            .map(|(class, p)| {
                let id = (p.class_id + self.class_offset) as u8;
                let mut mask = Mask {
                    w: labels.w,
                    h: labels.h,
                    data: labels
                        .data
                        .iter()
                        .map(|&l| if l == id { 0 } else { 255 })
                        .collect(),
                };
                mask.grow_invalid(p.dilate);
                (class, mask)
            })
            .collect()
    }
}

/// Mode, not median: labels are categorical. Driven from the serial stage, so it depends only on frame order.
pub struct SegSmoother {
    window: usize,
    history: std::collections::VecDeque<Vec<u8>>,
}

impl SegSmoother {
    pub fn new(window: u32) -> Self {
        Self {
            window: (window.max(1) as usize) | 1, // odd only, so no 2-2 ties
            history: std::collections::VecDeque::new(),
        }
    }

    /// The window is trailing, so the pipeline gains no latency, only lag in the mask itself.
    pub fn push(&mut self, labels: SegLabels) -> SegLabels {
        if self.window <= 1 {
            return labels;
        }
        let (w, h) = (labels.w, labels.h);
        self.history.push_back(labels.data);
        if self.history.len() > self.window {
            self.history.pop_front();
        }
        let n = self.history.back().map(|f| f.len()).unwrap_or(0);
        let mut out = vec![0u8; n];
        // Linear tally rather than a cleared 256-entry histogram: windows are 3-7 long and a pixel rarely takes two labels.
        let mut tally: Vec<(u8, usize)> = Vec::with_capacity(self.window);
        for (i, o) in out.iter_mut().enumerate() {
            tally.clear();
            for frame in &self.history {
                let v = frame[i];
                match tally.iter_mut().find(|(label, _)| *label == v) {
                    Some((_, count)) => *count += 1,
                    None => tally.push((v, 1)),
                }
            }
            // Strict `>` over a first-seen-ordered tally: ties go to the earliest label in the window.
            let mut best = (0u8, 0usize);
            for &(label, count) in &tally {
                if count > best.1 {
                    best = (label, count);
                }
            }
            *o = best.0;
        }
        SegLabels { w, h, data: out }
    }
}

/// Segmentation nets downsample by 32; other sizes either fail outright or silently pad.
fn round_up_32(v: u32) -> u32 {
    v.div_ceil(32) * 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_up_to_the_stride() {
        assert_eq!(round_up_32(1), 32);
        assert_eq!(round_up_32(640), 640);
        assert_eq!(round_up_32(641), 672);
    }

    #[test]
    fn class_defaults_differ_in_dilate() {
        assert!(SegClassParams::people().dilate > SegClassParams::sky().dilate);
        assert_eq!(SegClassParams::sky().class_id, ADE20K_SKY);
        assert_eq!(SegClassParams::people().class_id, ADE20K_PERSON);
    }

    fn labels(data: &[u8]) -> SegLabels {
        SegLabels {
            w: data.len() as u32,
            h: 1,
            data: data.to_vec(),
        }
    }

    #[test]
    fn smoother_is_identity_at_window_one() {
        let mut s = SegSmoother::new(1);
        assert_eq!(s.push(labels(&[1, 2, 3])).data, vec![1, 2, 3]);
    }

    #[test]
    fn smoother_takes_the_mode_over_the_window() {
        let mut s = SegSmoother::new(3);
        // Pixel 0 is steady. Pixel 1 flickers to 9 for one frame.
        s.push(labels(&[2, 2]));
        s.push(labels(&[2, 9]));
        let out = s.push(labels(&[2, 2]));
        assert_eq!(
            out.data,
            vec![2, 2],
            "a one-frame flicker must be voted out"
        );
    }

    #[test]
    fn smoother_follows_a_sustained_change() {
        let mut s = SegSmoother::new(3);
        s.push(labels(&[2]));
        s.push(labels(&[7]));
        assert_eq!(s.push(labels(&[7])).data, vec![7], "2 of 3 wins");
    }

    #[test]
    fn even_windows_become_odd() {
        assert_eq!(SegSmoother::new(4).window, 5);
    }
}

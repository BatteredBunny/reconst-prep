// Sky is masked because its clouds move independently of the camera and are otherwise matched as static geometry.

use anyhow::Result;

use crate::gray::{GrayFrame, nn_scale};

mod sky;

pub use sky::{SkyParams, sky_mask};

/// Per-pixel validity, `0` = ignore: COLMAP's convention, so the PNG needs no inversion.
#[derive(Clone)]
pub struct Mask {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn all_valid(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: vec![255; (w as usize) * (h as usize)],
        }
    }

    /// Fraction of pixels still usable, 0..1.
    pub fn valid_fraction(&self) -> f64 {
        if self.data.is_empty() {
            return 1.0;
        }
        let kept = self.data.iter().filter(|&&v| v != 0).count();
        kept as f64 / self.data.len() as f64
    }

    /// Nearest-neighbour scale, for the reason spelled out on `nn_scale`.
    pub fn scaled_to(&self, w: u32, h: u32) -> Mask {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        Mask {
            w,
            h,
            data: nn_scale(&self.data, self.w, self.h, w, h),
        }
    }

    fn intersect(&mut self, other: &Mask) {
        debug_assert_eq!((self.w, self.h), (other.w, other.h));
        for (a, b) in self.data.iter_mut().zip(&other.data) {
            if *b == 0 {
                *a = 0;
            }
        }
    }

    /// Grows the masked region to cover class boundaries, where the sources are weakest.
    pub(crate) fn grow_invalid(&mut self, r: u32) {
        if r == 0 {
            return;
        }
        let (w, h, r) = (self.w as usize, self.h as usize, r as usize);
        // One scratch buffer, swapped between the passes.
        let mut scratch = self.data.clone();
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                if self.data[row + x] == 0 {
                    continue;
                }
                let lo = x.saturating_sub(r);
                let hi = (x + r).min(w - 1);
                if self.data[row + lo..=row + hi].contains(&0) {
                    scratch[row + x] = 0;
                }
            }
        }
        std::mem::swap(&mut self.data, &mut scratch);
        scratch.copy_from_slice(&self.data);
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                if self.data[row + x] == 0 {
                    continue;
                }
                let lo = y.saturating_sub(r);
                let hi = (y + r).min(h - 1);
                if (lo..=hi).any(|yy| self.data[yy * w + x] == 0) {
                    scratch[row + x] = 0;
                }
            }
        }
        std::mem::swap(&mut self.data, &mut scratch);
    }
}

/// Masking a class in every frame costs a trainer all supervision there (measured PSNR 19.8 against 22.3); in some frames it costs none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaskUse {
    /// Structure-from-motion only. Never hand this to a trainer.
    SfmOnly,
    /// Both structure-from-motion and trainer supervision.
    SfmAndTraining,
}

impl MaskUse {
    fn sidecars(self) -> &'static [&'static str] {
        match self {
            MaskUse::SfmOnly => &["sfm"],
            MaskUse::SfmAndTraining => &["sfm", "training"],
        }
    }
}

/// One thing that can be masked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskClass {
    Sky,
    People,
}

impl MaskClass {
    pub fn name(self) -> &'static str {
        match self {
            MaskClass::Sky => "sky",
            MaskClass::People => "people",
        }
    }

    pub fn use_(self) -> MaskUse {
        match self {
            // Masked in every frame; see MaskUse.
            MaskClass::Sky => MaskUse::SfmOnly,
            // Some frames only: other views supervise the background behind.
            MaskClass::People => MaskUse::SfmAndTraining,
        }
    }

    fn manifest_entry(self, source: &str, params: Option<serde_json::Value>) -> ManifestMaskClass {
        ManifestMaskClass {
            name: self.name().into(),
            source: source.into(),
            written_to: self
                .use_()
                .sidecars()
                .iter()
                .map(ToString::to_string)
                .collect(),
            params,
        }
    }
}

/// One frame's masks, one per consumer, at metric resolution until the frame is kept.
#[derive(Clone, Debug)]
pub struct MaskSet {
    /// Every active class ANDed together; also feeds the selection metrics.
    pub sfm: Mask,
    /// `None` when no class is safe to hide, which is not the same as an all-valid mask.
    pub training: Option<Mask>,
}

/// A model as *asked for*: opening one needs a frame geometry only the pipeline knows.
#[derive(Debug, Clone, PartialEq)]
pub struct SegRequest {
    pub model_path: std::path::PathBuf,
    pub cfg: crate::seg::SegConfig,
}

/// Which sources are switched on, as three independent flags.
#[derive(Clone, Copy)]
struct Active {
    seg_sky: bool,
    seg_people: bool,
    heuristic_sky: bool,
}

/// What masking to apply; `None` fields are off.
#[derive(Debug, Clone, Default)]
pub struct MaskConfig {
    /// Heuristic sky parameters, ignored when a model is asked for sky.
    pub sky: Option<SkyParams>,
    pub seg: Option<SegRequest>,
    /// Set by `prepare`; shared across workers because inference is stateless.
    pub model: Option<std::sync::Arc<crate::seg::SegModel>>,
}

/// `model` is out: an opened model is a consequence of `seg`, never a difference from it.
impl PartialEq for MaskConfig {
    fn eq(&self, other: &Self) -> bool {
        (&self.sky, &self.seg) == (&other.sky, &other.seg)
    }
}

/// What a frontend asks for, before the decision tree turns it into a `MaskConfig`.
#[derive(Debug, Clone)]
pub struct MaskSources {
    pub mask_sky: bool,
    /// Used only when no model is asked for sky.
    pub sky_heuristic: SkyParams,
    pub model: Option<std::path::PathBuf>,
    pub sky_class: crate::seg::SegClassParams,
    /// `None` leaves people unmasked.
    pub people_class: Option<crate::seg::SegClassParams>,
    pub seg_width: u32,
    pub temporal_window: u32,
}

impl MaskConfig {
    /// The one masking decision tree, shared by CLI and GUI.
    pub fn from_sources(sources: MaskSources) -> Self {
        let MaskSources {
            mask_sky,
            sky_heuristic,
            model,
            sky_class,
            people_class,
            seg_width,
            temporal_window,
        } = sources;
        let cfg = crate::seg::SegConfig {
            width: seg_width,
            sky: (mask_sky && model.is_some()).then_some(sky_class),
            people: people_class,
            temporal_window,
        };
        Self {
            sky: (mask_sky && model.is_none()).then_some(sky_heuristic),
            seg: model
                .filter(|_| cfg.is_active())
                .map(|model_path| SegRequest { model_path, cfg }),
            model: None,
        }
    }
}

/// The name and the `<image-filename>.png` convention are COLMAP's.
pub const SFM_MASK_DIR: &str = "masks";
/// A separate directory rather than a flag: handing a trainer the wrong set is silent and expensive.
pub const TRAINING_MASK_DIR: &str = "masks_train";

/// Which sidecar each class ended up in: sky and person need opposite handling downstream.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ManifestMasking {
    /// Sidecar name -> directory, for the sets actually written.
    pub sidecars: std::collections::BTreeMap<String, String>,
    pub classes: Vec<ManifestMaskClass>,
    /// The consumer split, spelled out in the dataset itself.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ManifestMaskClass {
    /// `sky`, `static`, …
    pub name: String,
    /// How the class was detected: `heuristic`, `segmentation`, …
    pub source: String,
    /// Which sidecar sets carry this class.
    pub written_to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

const MASK_NOTE: &str = "masks/ carries every masked class and is for \
    structure-from-motion. masks_train/ carries only the classes that are \
    masked in some frames but not all; hand that set, not masks/, to a splat \
    trainer. A class masked in EVERY frame (the sky) leaves its region \
    unsupervised in every view, so a trainer never corrects splats that drift \
    there. That measured 2.5 dB PSNR worse than not masking at all.";

/// Absent rather than fatal when params will not serialize: the manifest is a record, not the run.
fn manifest_params<T: serde::Serialize>(params: Option<T>) -> Option<serde_json::Value> {
    params.and_then(|p| serde_json::to_value(p).ok())
}

impl std::fmt::Debug for Mask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mask({}x{})", self.w, self.h)
    }
}

impl MaskConfig {
    /// Read off the fields rather than `classes()`, which allocates on the per-frame path.
    pub fn is_active(&self) -> bool {
        let a = self.active();
        a.seg_sky || a.seg_people || a.heuristic_sky
    }

    /// Which sources are switched on, without the allocation `classes_with_source`
    /// needs. One derivation, so the per-frame predicates and the class list
    /// cannot answer differently.
    fn active(&self) -> Active {
        let seg = self.seg.as_ref();
        let seg_sky = seg.is_some_and(|s| s.cfg.sky.is_some());
        Active {
            seg_sky,
            seg_people: seg.is_some_and(|s| s.cfg.people.is_some()),
            // The model wins over the heuristic whenever one is asked for sky.
            heuristic_sky: self.sky.is_some() && !seg_sky,
        }
    }

    /// Only the frame aspect matters here; the inference size is `SegConfig::width`.
    pub fn prepare(&self, frame_w: u32, frame_h: u32) -> Result<Self> {
        let mut out = self.clone();
        out.model = match &self.seg {
            Some(req) if req.cfg.is_active() => Some(std::sync::Arc::new(
                crate::seg::SegModel::load(&req.model_path, &req.cfg, frame_w, frame_h)?,
            )),
            _ => None,
        };
        Ok(out)
    }

    /// An absent `masks_train/` says "no exclusions apply", which an all-valid mask does not.
    pub fn has_training_class(&self) -> bool {
        // People is the only class whose `MaskUse` admits a trainer.
        self.active().seg_people
    }

    /// Stateless, so safe to call from the parallel workers.
    pub fn segment(&self, rgb: &[u8], w: u32, h: u32) -> Result<Option<crate::seg::SegLabels>> {
        match (&self.seg, &self.model) {
            (Some(req), Some(model)) if req.cfg.is_active() => model.labels(rgb, w, h).map(Some),
            (Some(req), None) if req.cfg.is_active() => {
                anyhow::bail!("segmentation was requested but MaskConfig::prepare was never called")
            }
            _ => Ok(None),
        }
    }

    /// Driven from the serial stage so masks never depend on thread scheduling.
    pub fn smoother(&self) -> crate::seg::SegSmoother {
        crate::seg::SegSmoother::new(
            self.seg
                .as_ref()
                .map(|s| s.cfg.temporal_window)
                .unwrap_or(1),
        )
    }

    /// `rgb`/`gray` are the metric thumbnail reused from the metrics; `seg` is already smoothed.
    pub fn compute(
        &self,
        rgb: &[u8],
        gray: &GrayFrame,
        seg: Option<&crate::seg::SegLabels>,
    ) -> MaskSet {
        let mut sfm = Mask::all_valid(gray.w, gray.h);
        let mut training = self
            .has_training_class()
            .then(|| Mask::all_valid(gray.w, gray.h));

        for (class, layer) in self.class_layers(rgb, gray, seg) {
            sfm.intersect(&layer);
            if class.use_() == MaskUse::SfmAndTraining
                && let Some(t) = &mut training
            {
                t.intersect(&layer);
            }
        }
        MaskSet { sfm, training }
    }

    /// Kept per class for the GUI overlay, which must say *which* source removed a pixel.
    pub fn class_layers(
        &self,
        rgb: &[u8],
        gray: &GrayFrame,
        seg: Option<&crate::seg::SegLabels>,
    ) -> Vec<(MaskClass, Mask)> {
        let mut out = Vec::new();
        if let (Some(model), Some(labels)) = (&self.model, seg) {
            out.extend(
                model
                    .masks(labels)
                    .into_iter()
                    .map(|(class, m)| (class, m.scaled_to(gray.w, gray.h))),
            );
        }
        if let Some(p) = self.sky.filter(|_| self.active().heuristic_sky) {
            out.push((MaskClass::Sky, sky_mask(rgb, gray, p)));
        }
        out
    }

    /// The active classes and how each was detected, in the order applied.
    pub fn classes_with_source(&self) -> Vec<(MaskClass, &'static str)> {
        let a = self.active();
        let mut out = Vec::new();
        if a.seg_sky {
            out.push((MaskClass::Sky, "segmentation"));
        }
        if a.seg_people {
            out.push((MaskClass::People, "segmentation"));
        }
        if a.heuristic_sky {
            out.push((MaskClass::Sky, "heuristic"));
        }
        out
    }

    /// The active classes, in the order they are applied.
    pub fn classes(&self) -> Vec<MaskClass> {
        self.classes_with_source()
            .into_iter()
            .map(|(c, _)| c)
            .collect()
    }

    /// The manifest's account of what was masked and where it was written.
    pub fn describe(&self) -> ManifestMasking {
        let seg_cfg = self.seg.as_ref().map(|s| &s.cfg);
        let classes = self
            .classes_with_source()
            .into_iter()
            .map(|(c, source)| {
                let params = match (c, source) {
                    (MaskClass::Sky, "heuristic") => manifest_params(self.sky),
                    (MaskClass::Sky, _) => manifest_params(seg_cfg.and_then(|s| s.sky)),
                    (MaskClass::People, _) => manifest_params(seg_cfg.and_then(|s| s.people)),
                };
                c.manifest_entry(source, params)
            })
            .collect();
        let mut sidecars = std::collections::BTreeMap::new();
        sidecars.insert("sfm".to_string(), SFM_MASK_DIR.to_string());
        if self.has_training_class() {
            sidecars.insert("training".to_string(), TRAINING_MASK_DIR.to_string());
        }
        ManifestMasking {
            sidecars,
            classes,
            note: MASK_NOTE.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gray::rgb_to_gray;

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
    fn sky_alone_writes_no_training_set() {
        let (w, h) = (16u32, 16u32);
        let rgb = synthetic(w, h);
        let cfg = MaskConfig {
            sky: Some(SkyParams::default()),
            seg: None,
            model: None,
        };
        assert!(!cfg.has_training_class());
        let gray = rgb_to_gray(&rgb, w, h);
        assert!(
            cfg.compute(&rgb, &gray, None).training.is_none(),
            "an all-valid training mask is not the same as no training mask"
        );
    }

    /// A new class or a changed `MaskUse` must not let the two forms drift apart.
    #[test]
    fn fast_predicates_match_the_class_list() {
        use crate::seg::{SegClassParams, SegConfig};

        let skies = [None, Some(SkyParams::default())];
        let seg_skies = [None, Some(SegClassParams::sky())];
        let seg_people = [None, Some(SegClassParams::people())];
        for sky in skies {
            for with_model in [false, true] {
                for s in seg_skies {
                    for p in seg_people {
                        let cfg = MaskConfig {
                            sky,
                            seg: with_model.then(|| SegRequest {
                                model_path: "unused.onnx".into(),
                                cfg: SegConfig {
                                    sky: s,
                                    people: p,
                                    ..Default::default()
                                },
                            }),
                            model: None,
                        };
                        assert_eq!(
                            cfg.is_active(),
                            !cfg.classes().is_empty(),
                            "is_active disagrees with classes() for {cfg:?}"
                        );
                        assert_eq!(
                            cfg.has_training_class(),
                            cfg.classes()
                                .iter()
                                .any(|c| c.use_() == MaskUse::SfmAndTraining),
                            "has_training_class disagrees with classes() for {cfg:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn manifest_names_the_class_split() {
        // `describe` only reads the config, so no model file need exist.
        let cfg = MaskConfig {
            sky: Some(SkyParams::default()),
            seg: Some(SegRequest {
                model_path: "unused.onnx".into(),
                cfg: crate::seg::SegConfig {
                    people: Some(crate::seg::SegClassParams::people()),
                    ..Default::default()
                },
            }),
            model: None,
        };
        let d = cfg.describe();
        let sky = d.classes.iter().find(|c| c.name == "sky").unwrap();
        let people = d.classes.iter().find(|c| c.name == "people").unwrap();
        assert_eq!(sky.written_to, vec!["sfm"]);
        assert_eq!(people.written_to, vec!["sfm", "training"]);
        assert_eq!(d.sidecars["sfm"], SFM_MASK_DIR);
        assert_eq!(d.sidecars["training"], TRAINING_MASK_DIR);
    }
}

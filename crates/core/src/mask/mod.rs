// Sky is masked because its clouds move independently of the camera and are otherwise matched as static geometry.

use anyhow::Result;

use std::sync::Arc;

use crate::frame::{Rgb8, Size};
use crate::gray::{GrayFrame, laplacian_variance, nn_scale};
use crate::seg::{SegLabels, SegModel, SegSmoother};

mod sky;

pub use sky::{SkyParams, sky_mask};

/// Per-pixel validity, `0` = ignore: COLMAP's convention, so the PNG needs no inversion.
#[derive(Clone)]
pub struct Mask {
    pub size: Size,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn all_valid(size: Size) -> Self {
        Self {
            size,
            data: vec![255; size.px()],
        }
    }

    pub fn valid_fraction(&self) -> f64 {
        if self.data.is_empty() {
            return 1.0;
        }
        let kept = self.data.iter().filter(|&&v| v != 0).count();
        kept as f64 / self.data.len() as f64
    }

    /// Nearest-neighbour scale, for the reason spelled out on `nn_scale`.
    pub fn scaled_to(&self, size: Size) -> Mask {
        if size == self.size {
            return self.clone();
        }
        Mask {
            size,
            data: nn_scale(&self.data, self.size, size),
        }
    }

    fn intersect(&mut self, other: &Mask) {
        debug_assert_eq!(self.size, other.size);
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
        let ((w, h), r) = (self.size.as_usize(), r as usize);
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

/// Sidecar names as the manifest spells them; the directories are [`SFM_MASK_DIR`] and [`TRAINING_MASK_DIR`].
const SFM_SIDECAR: &str = "sfm";
const TRAINING_SIDECAR: &str = "training";

impl MaskUse {
    fn sidecars(self) -> &'static [&'static str] {
        match self {
            MaskUse::SfmOnly => &[SFM_SIDECAR],
            MaskUse::SfmAndTraining => &[SFM_SIDECAR, TRAINING_SIDECAR],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskSource {
    Segmentation,
    Heuristic,
}

impl MaskSource {
    pub fn name(self) -> &'static str {
        match self {
            MaskSource::Segmentation => "segmentation",
            MaskSource::Heuristic => "heuristic",
        }
    }
}

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

    fn manifest_entry(
        self,
        source: MaskSource,
        params: Option<serde_json::Value>,
    ) -> ManifestMaskClass {
        ManifestMaskClass {
            name: self.name().into(),
            source: source.name().into(),
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

/// What masking to apply; `None` fields are off. Opening the model is [`MaskConfig::prepare`]'s job.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaskConfig {
    /// Heuristic sky parameters, ignored when a model is asked for sky.
    pub sky: Option<SkyParams>,
    pub seg: Option<SegRequest>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MaskSourceError {
    #[error("people masking needs a segmentation model")]
    PeopleWithoutModel,
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
    pub fn from_sources(sources: MaskSources) -> Result<Self, MaskSourceError> {
        let MaskSources {
            mask_sky,
            sky_heuristic,
            model,
            sky_class,
            people_class,
            seg_width,
            temporal_window,
        } = sources;
        if people_class.is_some() && model.is_none() {
            return Err(MaskSourceError::PeopleWithoutModel);
        }
        let cfg = crate::seg::SegConfig {
            width: seg_width,
            sky: (mask_sky && model.is_some()).then_some(sky_class),
            people: people_class,
            temporal_window,
        };
        Ok(Self {
            sky: (mask_sky && model.is_none()).then_some(sky_heuristic),
            seg: model
                .filter(|_| cfg.is_active())
                .map(|model_path| SegRequest { model_path, cfg }),
        })
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
        write!(f, "Mask({})", self.size)
    }
}

impl MaskConfig {
    /// The active classes and how each was detected, in the order applied.
    /// Every other predicate reads from this, so none can disagree with it.
    pub fn sources(&self) -> impl Iterator<Item = (MaskClass, MaskSource)> + '_ {
        let seg = self.seg.as_ref().map(|s| &s.cfg);
        let seg_sky = seg.is_some_and(|c| c.sky.is_some());
        [
            seg_sky.then_some((MaskClass::Sky, MaskSource::Segmentation)),
            seg.and_then(|c| c.people)
                .map(|_| (MaskClass::People, MaskSource::Segmentation)),
            // The model wins over the heuristic whenever one is asked for sky.
            (self.sky.is_some() && !seg_sky).then_some((MaskClass::Sky, MaskSource::Heuristic)),
        ]
        .into_iter()
        .flatten()
    }

    pub fn is_active(&self) -> bool {
        self.sources().next().is_some()
    }

    /// An absent `masks_train/` says "no exclusions apply", which an all-valid mask does not.
    pub fn has_training_class(&self) -> bool {
        self.sources()
            .any(|(c, _)| c.use_() == MaskUse::SfmAndTraining)
    }

    /// Only the frame aspect matters here; the inference size is `SegConfig::width`.
    pub fn prepare(&self, frame: Size) -> Result<PreparedMask> {
        let model = match &self.seg {
            Some(req) if req.cfg.is_active() => {
                Some(Arc::new(SegModel::load(&req.model_path, &req.cfg, frame)?))
            }
            _ => None,
        };
        Ok(PreparedMask {
            cfg: self.clone(),
            frame,
            model,
        })
    }

    pub fn describe(&self) -> ManifestMasking {
        let seg_cfg = self.seg.as_ref().map(|s| &s.cfg);
        let classes = self
            .sources()
            .map(|(c, source)| {
                let params = match (c, source) {
                    (MaskClass::Sky, MaskSource::Heuristic) => manifest_params(self.sky),
                    (MaskClass::Sky, MaskSource::Segmentation) => {
                        manifest_params(seg_cfg.and_then(|s| s.sky))
                    }
                    (MaskClass::People, _) => manifest_params(seg_cfg.and_then(|s| s.people)),
                };
                c.manifest_entry(source, params)
            })
            .collect();
        let mut sidecars = std::collections::BTreeMap::new();
        sidecars.insert(SFM_SIDECAR.to_string(), SFM_MASK_DIR.to_string());
        if self.has_training_class() {
            sidecars.insert(TRAINING_SIDECAR.to_string(), TRAINING_MASK_DIR.to_string());
        }
        ManifestMasking {
            sidecars,
            classes,
            note: MASK_NOTE.into(),
        }
    }
}

/// A [`MaskConfig`] with its model opened. Only this can produce masks, so forgetting `prepare` does not compile.
#[derive(Clone)]
pub struct PreparedMask {
    cfg: MaskConfig,
    /// The geometry the model was opened for.
    frame: Size,
    /// Shared across workers because inference is stateless.
    model: Option<Arc<SegModel>>,
}

pub struct FrameJudgement {
    pub mask: Option<MaskSet>,
    /// Per class, at metric resolution, for the GUI overlay; the pipeline ignores them.
    pub layers: Vec<(MaskClass, Mask)>,
    pub sharpness: f64,
}

impl PreparedMask {
    pub fn config(&self) -> &MaskConfig {
        &self.cfg
    }

    /// Opening a model reloads, optimises and test-runs it, so a change that leaves
    /// the model request and the frame alone keeps the one already open.
    pub fn reconfigure(&self, cfg: &MaskConfig, frame: Size) -> Result<Self> {
        if cfg.seg == self.cfg.seg && frame == self.frame {
            return Ok(Self {
                cfg: cfg.clone(),
                ..self.clone()
            });
        }
        cfg.prepare(frame)
    }

    pub fn runs_inference(&self) -> bool {
        self.model.is_some()
    }

    /// Stateless, so safe to call from the parallel workers.
    pub fn segment(&self, rgb: &Rgb8) -> Result<Option<SegLabels>> {
        self.model.as_ref().map(|m| m.labels(rgb)).transpose()
    }

    /// Driven from the serial stage so masks never depend on thread scheduling.
    pub fn smoother(&self) -> SegSmoother {
        SegSmoother::new(
            self.cfg
                .seg
                .as_ref()
                .map(|s| s.cfg.temporal_window)
                .unwrap_or(1),
        )
    }

    /// `small`/`gray` are the metric thumbnail; `seg` is already smoothed.
    /// Narrows `gray` to the SfM set before measuring sharpness: that is a
    /// selection question, not a supervision one, so the sky belongs out of it.
    pub fn judge(
        &self,
        small: &Rgb8,
        gray: &mut GrayFrame,
        seg: Option<&SegLabels>,
    ) -> FrameJudgement {
        let mut judgement = FrameJudgement {
            mask: None,
            layers: Vec::new(),
            sharpness: 0.0,
        };
        if self.cfg.is_active() {
            judgement.layers = self.class_layers(small, gray, seg);
            let set = self.mask_set(&judgement.layers, gray.size);
            gray.restrict_to(&set.sfm.data);
            judgement.mask = Some(set);
        }
        judgement.sharpness = laplacian_variance(gray);
        judgement
    }

    fn class_layers(
        &self,
        rgb: &Rgb8,
        gray: &GrayFrame,
        seg: Option<&SegLabels>,
    ) -> Vec<(MaskClass, Mask)> {
        let mut out = Vec::new();
        if let (Some(model), Some(labels)) = (&self.model, seg) {
            out.extend(
                model
                    .masks(labels)
                    .into_iter()
                    .map(|(class, m)| (class, m.scaled_to(gray.size))),
            );
        }
        let heuristic_sky = self
            .cfg
            .sources()
            .any(|s| s == (MaskClass::Sky, MaskSource::Heuristic));
        if let Some(p) = self.cfg.sky.filter(|_| heuristic_sky) {
            out.push((MaskClass::Sky, sky_mask(rgb, gray, p)));
        }
        out
    }

    fn mask_set(&self, layers: &[(MaskClass, Mask)], size: Size) -> MaskSet {
        let mut sfm = Mask::all_valid(size);
        let mut training = self.cfg.has_training_class().then(|| Mask::all_valid(size));
        for (class, layer) in layers {
            sfm.intersect(layer);
            if class.use_() == MaskUse::SfmAndTraining
                && let Some(t) = &mut training
            {
                t.intersect(layer);
            }
        }
        MaskSet { sfm, training }
    }
}

/// Bright blue top half, dark ground bottom half: a sky every source should find. Shared by the tests here and in `sky`.
#[cfg(test)]
fn synthetic(w: u32, h: u32) -> Rgb8 {
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
    Rgb8::new(Size::new(w, h), rgb).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gray::rgb_to_gray;

    use super::synthetic;

    #[test]
    fn sky_alone_writes_no_training_set() {
        let (w, h) = (16u32, 16u32);
        let rgb = synthetic(w, h);
        let cfg = MaskConfig {
            sky: Some(SkyParams::default()),
            seg: None,
        };
        assert!(!cfg.has_training_class());
        let mut gray = rgb_to_gray(&rgb);
        let judged = cfg
            .prepare(Size::new(w, h))
            .unwrap()
            .judge(&rgb, &mut gray, None);
        assert!(
            judged.mask.unwrap().training.is_none(),
            "an all-valid training mask is not the same as no training mask"
        );
        assert_eq!(judged.layers.len(), 1);
        assert!(gray.valid.is_some(), "sharpness must skip the masked sky");
    }

    #[test]
    fn people_without_a_model_is_refused() {
        let sources = MaskSources {
            mask_sky: false,
            sky_heuristic: SkyParams::default(),
            model: None,
            sky_class: crate::seg::SegClassParams::sky(),
            people_class: Some(crate::seg::SegClassParams::people()),
            seg_width: crate::seg::DEFAULT_SEG_WIDTH,
            temporal_window: 1,
        };
        assert_eq!(
            MaskConfig::from_sources(sources),
            Err(MaskSourceError::PeopleWithoutModel)
        );
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

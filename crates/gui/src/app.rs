use std::path::PathBuf;
use std::sync::Arc;

use crate::native::images::ImageSource;
use crate::{Start, clips, preview, settings};
use async_channel::{Receiver, Sender, unbounded};

use reconst_prep_core::cancel::CancelToken;
use reconst_prep_core::decode::ClipInfo;

use reconst_prep_core::mask::MaskSourceError;
use reconst_prep_core::output::ImageFormat;
use reconst_prep_core::pipeline::{
    OutputSpec, PipelineConfig, Progress, RunManifest, run_pipeline,
};
use reconst_prep_core::profiles::ProfileEntry;
use reconst_prep_core::undistort::{LensProfileFile, ProfileSummary};

use preview::{MaskCache, PreviewData, PreviewRequest, PreviewUpdate, SharedFfmpeg};
use settings::{InterruptedRun, Settings};

/// Why a run stopped. A cancel is not a failure, and the two are told apart by
/// a flag set where the error is still typed, not by a substring of its text.
pub(crate) struct RunError {
    pub message: String,
    pub cancelled: bool,
}

// Worker messages.

pub(crate) enum Msg {
    Progress(Progress),
    RunDone(Result<Box<RunManifest>, RunError>),
    /// Carries its request's generation: a stale update landing over a newer one would show settings no longer on screen.
    Preview(u64, PreviewUpdate, Vec<ImageSource>),
    PreviewReady(u64),
    PreviewFailed(u64, String),
    Probed(PathBuf, Result<Box<ClipInfo>, String>),
    /// `None` when the clip cannot be decoded, which stops it being retried forever.
    Thumbnail(PathBuf, Option<ImageSource>),
    ProfileIndex(Result<Vec<ProfileEntry>, String>),
    ProfileFetched(Result<PathBuf, String>),
    ModelProgress(f32),
    ModelFetched(Result<PathBuf, String>),
}

/// Worker messages wake the UI through its asynchronous receiver.
pub(crate) struct Emit {
    pub tx: Sender<Msg>,
}

impl Emit {
    pub fn send(&self, msg: Msg) {
        let _ = self.tx.try_send(msg);
    }
}

/// The lens-profile browser: a search over Gyroflow's cached index.
#[derive(Default)]
pub(crate) struct ProfileBrowser {
    pub open: bool,
    pub query: String,
    pub entries: Vec<ProfileEntry>,
    pub loading: bool,
    pub error: Option<String>,
    /// Paths the user picked before, most recent first.
    pub recent: Vec<PathBuf>,
}

/// Not a catalogue browser: one model is on offer, behind a button in People's advanced options.
#[derive(Default)]
pub(crate) struct ModelPicker {
    /// Cache filename of the entry being downloaded, and its progress 0..1.
    pub downloading: Option<(String, f32)>,
    pub error: Option<String>,
}

/// The preview worker: what is in flight and what was last asked for.
#[derive(Default)]
pub(crate) struct PreviewJob {
    pub busy: bool,
    pub complete: bool,
    /// Named in the header beside a spinner.
    pub stage: Option<&'static str>,
    pub error: Option<String>,
    /// Bumped on every request; worker messages from any other generation are dropped on arrival.
    generation: u64,
    /// Replaced per request, so a superseded worker stops instead of finishing into the void.
    cancel: CancelToken,
    /// The request the current settings would issue; a change resets the debounce deadline.
    pending: Option<PreviewRequest>,
}

/// Prepared display resources. Pointer gestures stay local to the preview component.
#[derive(Default)]
pub(crate) struct PreviewView {
    pub source: Option<ImageSource>,
    pub output: Option<ImageSource>,
    pub overlay: Option<ImageSource>,
}

/// Everything about the preview pane: which frame, what is in flight, what came back, and how it is shown.
pub(crate) struct PreviewState {
    pub clip: usize,
    /// Position in the clip, 0..1.
    pub pos: f64,
    pub data: Option<PreviewData>,
    pub job: PreviewJob,
    pub view: PreviewView,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            clip: 0,
            pos: 0.25,
            data: None,
            job: PreviewJob::default(),
            view: PreviewView::default(),
        }
    }
}

impl PreviewState {
    fn on_update(&mut self, update: PreviewUpdate, images: Vec<ImageSource>) {
        match update {
            PreviewUpdate::Raw(p) => {
                self.job.busy = false;
                self.view.source = images.first().cloned();
                self.view.output = images.get(1).cloned();
                // The old frame's mask must not tint the new frame.
                self.view.overlay = None;
                self.data = Some(*p);
            }
            PreviewUpdate::Stage(stage) => self.job.stage = Some(stage),
            PreviewUpdate::Rendered(img) => {
                self.job.stage = None;
                self.view.output = images.first().cloned();
                if let Some(p) = &mut self.data {
                    p.output = img;
                }
            }
            PreviewUpdate::Finished {
                overlay,
                masked_fraction,
                sharpness,
            } => {
                self.job.stage = None;
                self.job.complete = true;
                self.view.overlay = images.first().cloned();
                let _ = overlay;
                if let Some(p) = &mut self.data {
                    p.masked_fraction = masked_fraction;
                    p.sharpness = Some(sharpness);
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct App {
    pub cfg: Settings,

    pub clips: clips::ClipLibrary,

    // Lens correction
    pub browser: ProfileBrowser,

    // Masking
    pub models: ModelPicker,

    pub out_dir: Option<PathBuf>,

    pub interrupted: Option<InterruptedRun>,
    resume_next: bool,

    pub preview: PreviewState,
    /// Shared by every probe, thumbnail and preview worker.
    ffmpeg: SharedFfmpeg,
    masks: MaskCache,

    // --- Run
    pub running: bool,
    pub cancel: CancelToken,
    pub progress: Option<Progress>,
    pub last_result: Option<Result<(), RunError>>,
    pub toasts: Vec<String>,

    /// A fact about the window, not the job.
    pub resume_prompt: bool,

    pub about_open: bool,
    /// The manifest of the run that just finished, while its summary is up.
    pub summary: Option<Arc<RunManifest>>,

    /// Tracked so the title is only re-sent to the compositor when it changes.

    /// The profile as validated, against the path it came from. `validation`
    /// draws every repaint, and re-read and re-parsed the file each one. A
    /// profile edited in place is not noticed until it is picked again.
    profile_summary: Option<(PathBuf, Result<ProfileSummary, String>)>,

    pub channel: Channel,
    debounce: Option<std::sync::mpsc::Sender<u64>>,
}

/// The "s" in "3 clips", for any of the integer types the counts arrive as.
pub(crate) fn plural<N: PartialEq + From<u8>>(n: N) -> &'static str {
    if n == N::from(1) { "" } else { "s" }
}

pub(crate) struct Channel {
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for Channel {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl App {
    /// Takes whatever the model cache offers.
    pub fn from_settings(s: Settings) -> Self {
        Self::from_settings_with(s, reconst_prep_core::models::cached().into_iter().next())
    }

    /// `cached_model` is a parameter so tests do not depend on this machine's cache.
    pub(crate) fn from_settings_with(s: Settings, cached_model: Option<PathBuf>) -> Self {
        let mut app = Self {
            // A question at launch, never an action.
            interrupted: InterruptedRun::load(),
            resume_prompt: true,
            ..Self::default()
        };
        app.adopt_settings(s, cached_model);
        app
    }

    /// In place, so worker channels and in-flight cancel tokens survive.
    fn adopt_settings(&mut self, mut cfg: Settings, cached_model: Option<PathBuf>) {
        // The browser owns the recent list while the window is open.
        self.browser.recent = std::mem::take(&mut cfg.recent_profiles);
        cfg.seg_model = cfg
            .seg_model
            .take()
            .filter(|p| p.is_file())
            .or(cached_model);
        // The checkbox is disabled without a model, so checked-but-disabled could never be unticked.
        cfg.mask_people = cfg.mask_people && cfg.seg_model.is_some();
        self.cfg = cfg;
    }

    /// A flag that was not given leaves the persisted setting alone.
    pub fn with(mut self, start: Start) -> Self {
        for p in start.inputs {
            self.clips.add(p);
        }
        if let Some(p) = start.profile {
            self.cfg.undistort = true;
            self.cfg.profile = Some(p);
        }
        if let Some(d) = start.out_dir {
            self.out_dir = Some(d);
        }
        self
    }

    /// The persisted block, plus the recent list the browser holds while the window is open.
    pub fn to_settings(&self) -> Settings {
        Settings {
            recent_profiles: self.browser.recent.clone(),
            ..self.cfg.clone()
        }
    }

    /// A title that names the job, so several windows can be told apart. Plain ASCII: the window
    /// manager draws it in its own font, which may lack symbols like an arrow.
    pub fn window_title(&self) -> String {
        let n = self.clips.files().len();
        let out = self.out_dir.as_deref().map(short_name);
        match (n, out) {
            (0, _) => "reconst-prep".to_string(),
            (n, Some(out)) => format!("reconst-prep - {n} clip{} into {out}", plural(n)),
            (n, None) => format!("reconst-prep - {n} clip{}", plural(n)),
        }
    }

    // -- config -> core types ---------------------------------------------

    /// The run Process would start, or what still has to be filled in, phrased as the next thing to do.
    /// One function, so the button and the run can never disagree about readiness.
    pub fn run_config(&self) -> Result<PipelineConfig, &'static str> {
        if self.clips.inputs().is_empty() {
            return Err("Add at least one video.");
        }
        let Some(out_dir) = self.out_dir.clone() else {
            return Err("Choose an output folder.");
        };
        if self.cfg.lens_unready() {
            return Err("Pick a lens profile, or turn off lens correction.");
        }
        let mask = self.cfg.mask_config().map_err(|e| match e {
            MaskSourceError::PeopleWithoutModel => "People masking needs a segmentation model.",
        })?;
        Ok(PipelineConfig {
            inputs: self.clips.inputs().to_vec(),
            profile_path: self.cfg.active_profile(),
            out_dir,
            output_size: self.cfg.output_spec(),
            format: self.cfg.image_format(),
            selection: self.cfg.selection_config(),
            hwaccel: self.cfg.hwaccel.clone(),
            ffmpeg_path: None,
            interp: self.cfg.interp,
            gpu: self.cfg.gpu,
            mask,
            writer_threads: 0,
            undistort_threads: 0,
            resume: self.resume_next,
            frames_from: None,
        })
    }

    /// The whole run in one line: what would happen if Process were pressed.
    pub fn job_summary(&self) -> String {
        let n = self.clips.files().len();
        if n == 0 {
            return "no clips loaded".to_string();
        }
        let size = match self.cfg.output_spec() {
            OutputSpec::Same => "native".to_string(),
            OutputSpec::Scale { factor } => format!("×{factor}"),
            OutputSpec::Exact { width, height } => format!("{width}×{height}"),
        };
        let format = match self.cfg.image_format() {
            ImageFormat::Jpeg { quality } => format!("JPEG q{quality}"),
            ImageFormat::Png => "PNG".to_string(),
        };
        let mut s = format!("{n} clip{}  ·  {size} {format}", plural(n));
        if self.cfg.undistort {
            s.push_str("  ·  undistorted");
        }
        let masked = self.cfg.masked_classes();
        if !masked.is_empty() {
            s.push_str(&format!("  ·  masking {}", masked.join(", ")));
        }
        if let Some(out) = self.out_dir.as_ref().and_then(|p| p.file_name()) {
            s.push_str(&format!("  →  {}", out.to_string_lossy()));
        }
        s
    }

    /// Is the selected folder the one an unfinished run was writing into?
    pub fn can_resume_here(&self) -> bool {
        (self.interrupted.as_ref().zip(self.out_dir.as_ref()))
            .is_some_and(|(run, out)| &run.out_dir == out)
    }

    /// Clips, settings and output folder together, because the pipeline refuses a resume whose parameters differ.
    pub fn adopt_interrupted(&mut self) {
        let Some(run) = self.interrupted.clone() else {
            return;
        };
        let cached_model = reconst_prep_core::models::cached().into_iter().next();
        self.adopt_settings(run.settings.clone(), cached_model);
        self.clips.clear();
        for p in &run.inputs {
            self.clips.add(p.clone());
        }
        self.out_dir = Some(run.out_dir.clone());
        self.preview.clip = 0;
        log::info!("resuming the run into {}", run.out_dir.display());
    }

    /// Every worker answers through the same asynchronous event channel.
    pub fn spawn_worker(&self, name: &str, job: impl FnOnce() -> Msg + Send + 'static) {
        self.spawn_worker_with(name, move |out| out.send(job()));
    }

    /// A failed spawn panics on purpose: swallowing it strands the caller's busy flag on for the session.
    pub fn spawn_worker_with(&self, name: &str, job: impl FnOnce(&Emit) + Send + 'static) {
        let out = Emit {
            tx: self.channel.tx.clone(),
        };
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || job(&out))
            .unwrap_or_else(|e| panic!("spawn {name} thread: {e}"));
    }

    pub fn probe_missing(&mut self) {
        // Capped: dropping two hundred clips must not put two hundred threads and ffprobe processes in flight.
        const WORKERS: usize = 8;

        while let Some(path) = self.clips.claim_unprobed(WORKERS) {
            let ff = self.ffmpeg.clone();
            self.spawn_worker("probe", move || {
                let res = ff
                    .get()
                    .and_then(|ff| reconst_prep_core::decode::probe_clip(ff, &path))
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                Msg::Probed(path, res)
            });
        }
    }

    /// Only once a clip has probed: a file ffprobe could not read will not yield a frame, and retrying costs an ffmpeg launch per repaint.
    pub fn request_thumbnails(&mut self) {
        // Each decode is capped at two threads, so six at once still costs less CPU than one uncapped decode.
        const WORKERS: usize = 6;

        while let Some((path, duration_s)) = self.clips.claim_unthumbed(WORKERS) {
            let ff = self.ffmpeg.clone();
            self.spawn_worker("thumbnail", move || {
                let res = preview::build_thumbnail(&path, duration_s, &ff)
                    .and_then(|img| ImageSource::rgb(&img))
                    .ok();
                Msg::Thumbnail(path, res)
            });
        }
    }

    /// Clamped in one place: readers that clamp differently disagree about which clip they mean.
    pub fn preview_clip_path(&self) -> Option<&PathBuf> {
        let files = self.clips.files();
        files.get(self.preview.clip.min(files.len().checked_sub(1)?))
    }

    pub fn preview_clip_info(&self) -> Option<&ClipInfo> {
        self.clips.probe(self.preview_clip_path()?)?.as_ref().ok()
    }

    /// Narrower than `PipelineConfig`: format and quality change the written file but not the frame on screen.
    fn preview_request(&self) -> Option<PreviewRequest> {
        if self.cfg.lens_unready() {
            return None;
        }
        Some(PreviewRequest {
            clip: self.preview_clip_info()?.clone(),
            profile: self.cfg.active_profile(),
            pos: self.preview.pos,
            output_spec: self.cfg.output_spec(),
            interp: self.cfg.interp,
            hwaccel: self.cfg.hwaccel.clone(),
            mask: self.cfg.mask_config().ok()?,
        })
    }

    /// Called after an action or worker event, never in a render loop.
    pub fn reconcile(&mut self) {
        self.refresh_profile_summary();
        self.clips.refresh();
        self.preview.clip = self
            .preview
            .clip
            .min(self.clips.files().len().saturating_sub(1));
        if let Some(size) = self.clips.source_ceiling() {
            self.cfg.exact_w = self.cfg.exact_w.clamp(2, size.w.max(2));
            self.cfg.exact_h = self.cfg.exact_h.clamp(2, size.h.max(2));
        }
        self.probe_missing();
        self.request_thumbnails();
        let request = self.preview_request();
        if request != self.preview.job.pending {
            let job = &mut self.preview.job;
            let unavailable = request.is_none();
            job.pending = request;
            job.cancel.cancel();
            job.generation += 1;
            job.busy = false;
            job.complete = false;
            job.stage = None;
            job.error = None;
            let generation = job.generation;
            if unavailable {
                self.preview.data = None;
                self.preview.view.source = None;
                self.preview.view.output = None;
                self.preview.view.overlay = None;
                return;
            }
            if self.debounce.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                let out = self.channel.tx.clone();
                std::thread::Builder::new()
                    .name("preview-debounce".into())
                    .spawn(move || {
                        while let Ok(mut generation) = rx.recv() {
                            loop {
                                match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                                    Ok(new) => generation = new,
                                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                        if out.try_send(Msg::PreviewReady(generation)).is_err() {
                                            return;
                                        }
                                        break;
                                    }
                                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                                }
                            }
                        }
                    })
                    .expect("spawn debounce worker");
                self.debounce = Some(tx);
            }
            let _ = self.debounce.as_ref().unwrap().send(generation);
        }
    }

    pub fn request_preview(&mut self) {
        if self.preview.job.busy {
            return;
        }
        let Some(req) = self.preview_request() else {
            return;
        };
        let job = &mut self.preview.job;
        // A newer request supersedes anything in flight: its results can no longer land.
        job.cancel.cancel();
        job.cancel = CancelToken::new();
        job.generation += 1;
        let generation = job.generation;
        let cancel = job.cancel.clone();
        job.busy = true;
        job.complete = false;
        job.stage = None;
        job.error = None;
        log::info!("preview: {} at {:.0}%", req.clip.stem, req.pos * 100.0);
        let (ff, masks) = (self.ffmpeg.clone(), self.masks.clone());
        self.spawn_worker_with("preview", move |out| {
            let mut emit = |u: PreviewUpdate| {
                let images = crate::native::images::for_update(&u);
                match images {
                    Ok(images) => out.send(Msg::Preview(generation, u, images)),
                    Err(e) => out.send(Msg::PreviewFailed(generation, e.to_string())),
                }
            };
            if let Err(e) = preview::build_preview(&req, &ff, &masks, &cancel, &mut emit) {
                out.send(Msg::PreviewFailed(generation, format!("{e:#}")));
            }
        });
    }

    /// Explicit, one click, never during a run. The hash check lives in `models::fetch`.
    pub fn fetch_model(&mut self, entry: &'static reconst_prep_core::models::CatalogueEntry) {
        if self.models.downloading.is_some() {
            return;
        }
        self.models.error = None;
        log::info!("downloading {} ({:.0} MB)", entry.name, entry.size_mb());
        self.models.downloading = Some((entry.file.to_string(), 0.0));
        // The picker has no Stop button yet, so nothing holds the other end.
        let cancel = CancelToken::never();
        self.spawn_worker_with("model-fetch", move |out| {
            let mut last = 0.0f32;
            let mut progress = |done: u64, total: u64| {
                let f = (done as f32 / total.max(1) as f32).min(1.0);
                // Only wake the UI on visible movement.
                if f - last >= 0.01 {
                    last = f;
                    out.send(Msg::ModelProgress(f));
                }
            };
            let res = reconst_prep_core::models::fetch(entry, &mut progress, &cancel)
                .map_err(|e| format!("{e:#}"));
            out.send(Msg::ModelFetched(res));
        });
    }

    pub fn preview_duration_s(&self) -> Option<f64> {
        preview::duration_of(self.preview_clip_info()?)
    }

    pub fn preview_frame_pos_step(&self) -> Option<f64> {
        let info = self.preview_clip_info()?;
        let duration_s = preview::duration_of(info)?;
        (info.fps > 0.0 && duration_s > 0.0).then(|| 1.0 / (duration_s * info.fps))
    }

    /// The record written here is deleted only on *success*, which is how the next launch knows to offer it.
    pub fn start_run(&mut self, resume: bool) {
        if self.running {
            return;
        }
        self.resume_next = resume;
        let Ok(cfg) = self.run_config() else {
            return;
        };
        self.to_settings().save();
        InterruptedRun {
            out_dir: cfg.out_dir.clone(),
            inputs: cfg.inputs.clone(),
            settings: self.to_settings(),
            started_unix: reconst_prep_core::paths::unix_now(),
        }
        .save();
        log::info!(
            "run {}: {}",
            if resume { "resuming" } else { "starting" },
            self.job_summary()
        );
        self.cancel = CancelToken::new();
        self.running = true;
        self.progress = None;
        self.last_result = None;
        let cancel = self.cancel.clone();
        self.spawn_worker_with("pipeline", move |out| {
            let mut progress = |p: Progress| out.send(Msg::Progress(p));
            // The whole manifest comes back, so the summary is built from the same record the dataset carries.
            let res = run_pipeline(&cfg, &mut progress, &cancel)
                .map(Box::new)
                .map_err(|e| RunError {
                    cancelled: reconst_prep_core::cancel::is_cancelled(&e),
                    message: format!("{e:#}"),
                });
            out.send(Msg::RunDone(res));
        });
    }

    fn on_run_done(&mut self, r: Result<Box<RunManifest>, RunError>) {
        self.running = false;
        self.resume_next = false;
        let result = match r {
            Ok(m) => {
                log::info!(
                    "run finished: {} written from {} decoded over {} clip{} in {:.1}s",
                    m.totals.written,
                    m.totals.decoded,
                    m.clips.len(),
                    plural(m.clips.len()),
                    m.totals.wall_seconds
                );
                InterruptedRun::clear();
                // No toast on success: the summary modal already has the same numbers.
                self.summary = Some(Arc::from(m));
                Ok(())
            }
            Err(e) => {
                log::warn!("run stopped: {}", e.message);
                // A cancel needs no toast: the dock already reports it and offers Resume.
                if !e.cancelled {
                    self.toasts.push(e.message.clone());
                }
                Err(e)
            }
        };
        // Re-read: whether there is anything to pick up is a question about the dataset on disk.
        self.interrupted = InterruptedRun::load();
        self.last_result = Some(result);
    }

    fn refresh_profile_summary(&mut self) {
        let Some(path) = self.cfg.profile.clone() else {
            self.profile_summary = None;
            return;
        };
        if self
            .profile_summary
            .as_ref()
            .is_none_or(|(cached, _)| *cached != path)
        {
            let parsed = LensProfileFile::load(&path)
                .map(|file| file.summary().clone())
                .map_err(|error| format!("{error:#}"));
            self.profile_summary = Some((path, parsed));
        }
    }

    /// Parsed on profile changes, so rendering never performs filesystem I/O.
    pub fn profile_summary(&self) -> Option<&Result<ProfileSummary, String>> {
        self.profile_summary.as_ref().map(|(_, summary)| summary)
    }

    pub fn on_message(&mut self, msg: Msg) {
        match msg {
            Msg::Progress(p) => self.progress = Some(p),
            Msg::RunDone(r) => self.on_run_done(r),
            Msg::Preview(generation, _, _)
            | Msg::PreviewFailed(generation, _)
            | Msg::PreviewReady(generation)
                if generation != self.preview.job.generation => {} // superseded mid-flight
            Msg::Preview(_, update, images) => self.preview.on_update(update, images),
            Msg::PreviewReady(_) => self.request_preview(),
            Msg::PreviewFailed(_, e) => {
                self.preview.job.busy = false;
                self.preview.job.stage = None;
                self.preview.job.error = Some(e);
            }
            Msg::Probed(path, res) => self.clips.on_probed(path, res.map(|b| *b)),
            Msg::Thumbnail(path, res) => {
                self.clips.on_thumbnail(path, res);
            }
            Msg::ProfileIndex(res) => {
                self.browser.loading = false;
                match res {
                    Ok(e) => self.browser.entries = e,
                    Err(e) => self.browser.error = Some(e),
                }
            }
            Msg::ModelProgress(f) => {
                if let Some((_, p)) = &mut self.models.downloading {
                    *p = f;
                }
            }
            Msg::ModelFetched(res) => {
                self.models.downloading = None;
                self.models.error = res.as_ref().err().cloned();
                match res {
                    Ok(p) => self.cfg.seg_model = Some(p),
                    Err(e) => log::warn!("model download failed: {e}"),
                }
            }
            Msg::ProfileFetched(res) => {
                self.browser.loading = false;
                match res {
                    Ok(p) => {
                        self.browser.recent.retain(|r| r != &p);
                        self.browser.recent.insert(0, p.clone());
                        self.browser.recent.truncate(8);
                        self.cfg.profile = Some(p);
                        self.browser.open = false;
                    }
                    Err(e) => self.browser.error = Some(e),
                }
            }
        }
    }

    /// A `.json` is a lens profile, a `.onnx` is a segmentation model, anything else is footage.
    pub fn take_dropped(&mut self, dropped: Vec<PathBuf>) {
        for p in dropped {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            match ext.as_deref() {
                Some("json") => self.cfg.profile = Some(p),
                Some("onnx") => self.cfg.seg_model = Some(p),
                _ => _ = self.clips.add(p),
            }
        }
    }
}

pub fn short_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

impl App {
    pub fn preview_cancel(&self) -> CancelToken {
        self.preview.job.cancel.clone()
    }

    pub fn shutdown(&mut self) {
        self.cancel.cancel();
        self.preview.job.cancel.cancel();
        self.debounce = None;
        self.channel.rx.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reconst_prep_core::decode::HwAccel;
    use reconst_prep_core::undistort::Interp;

    fn json(s: &Settings) -> String {
        serde_json::to_string(s).unwrap()
    }

    /// Why `App` holds a `Settings` rather than a copy of its fields: a mirror loses a setting without failing to compile.
    #[test]
    fn settings_survive_a_round_trip() {
        // Every field non-default, so a field serde skips or renames fails here.
        let before = Settings {
            undistort: true,
            profile: Some("/tmp/lens.json".into()),
            size_choice: crate::settings::SizeChoice::Scale,
            scale_factor: 0.75,
            exact_w: 2560,
            exact_h: 1440,
            format_jpeg: false,
            jpeg_quality: 71,
            mode: crate::settings::ModeChoice::EveryNth,
            nth: 42,
            motion_threshold: 0.07,
            window: 12,
            blur_floor_on: true,
            blur_floor: 250.0,
            mask_sky: true,
            sky: reconst_prep_core::mask::SkyParams {
                luma_min: 99,
                ..Default::default()
            },
            seg_width: 640,
            seg_temporal_window: 5,
            hwaccel: HwAccel::Backend("vaapi".into()),
            interp: Interp::Lanczos4,
            recent_profiles: vec!["/tmp/a.json".into(), "/tmp/b.json".into()],
            window_size: Some([1234.0, 567.0]),
            ..Settings::default()
        };
        // No cached model offered: with one the round trip would depend on this machine's cache.
        let after = App::from_settings_with(before.clone(), None).to_settings();
        assert_eq!(json(&before), json(&after));
    }

    /// A model is adopted from the cache when the saved path has gone, and People cannot load ticked without one.
    #[test]
    fn people_is_only_ticked_when_a_model_is_there() {
        let saved = Settings {
            mask_people: true,
            seg_model: Some("/nonexistent/gone.onnx".into()),
            ..Settings::default()
        };
        let without = App::from_settings_with(saved.clone(), None).to_settings();
        assert_eq!(without.seg_model, None);
        assert!(!without.mask_people, "no model, so People cannot be ticked");

        let cached = PathBuf::from("/cache/segformer.onnx");
        let with = App::from_settings_with(saved, Some(cached.clone())).to_settings();
        assert_eq!(with.seg_model, Some(cached));
        assert!(with.mask_people, "the saved tick survives an adopted model");
    }

    #[test]
    fn repeated_start_does_not_change_an_active_run() {
        let mut app = App {
            running: true,
            ..App::default()
        };
        app.start_run(true);
        assert!(
            !app.resume_next,
            "second start must not change the active run mode"
        );
        assert!(app.running);
    }

    #[test]
    fn stale_preview_results_and_failures_cannot_replace_current_state() {
        let mut app = App::default();
        app.preview.job.generation = 4;
        app.preview.job.busy = true;
        app.preview.job.stage = Some("current");
        app.on_message(Msg::Preview(3, PreviewUpdate::Stage("stale"), vec![]));
        app.on_message(Msg::PreviewFailed(3, "old error".into()));
        assert_eq!(app.preview.job.stage, Some("current"));
        assert!(app.preview.job.busy);
        assert!(app.preview.job.error.is_none());
        app.on_message(Msg::PreviewFailed(4, "current error".into()));
        assert!(!app.preview.job.busy);
        assert_eq!(app.preview.job.error.as_deref(), Some("current error"));
    }

    #[test]
    fn drop_routes_models_profiles_and_footage() {
        let mut app = App::default();
        app.take_dropped(vec![
            "lens.JSON".into(),
            "mask.ONNX".into(),
            "clip.mp4".into(),
        ]);
        assert_eq!(
            app.cfg.profile.as_deref(),
            Some(std::path::Path::new("lens.JSON"))
        );
        assert_eq!(
            app.cfg.seg_model.as_deref(),
            Some(std::path::Path::new("mask.ONNX"))
        );
        assert_eq!(app.clips.inputs(), &[PathBuf::from("clip.mp4")]);
    }

    #[test]
    fn readiness_and_resume_adoption_preserve_the_run_configuration() {
        let mut app = App::default();
        assert_eq!(app.run_config().err(), Some("Add at least one video."));
        app.clips.add("clip.mp4".into());
        assert_eq!(app.run_config().err(), Some("Choose an output folder."));
        app.interrupted = Some(InterruptedRun {
            inputs: vec!["saved.mp4".into()],
            out_dir: "dataset".into(),
            settings: Settings {
                nth: 19,
                ..Settings::default()
            },
            started_unix: 0,
        });
        app.adopt_interrupted();
        assert!(!app.running, "adoption must not start the pipeline");
        assert!(app.can_resume_here());
        assert_eq!(app.cfg.nth, 19);
        assert_eq!(app.clips.inputs(), &[PathBuf::from("saved.mp4")]);
        assert!(app.run_config().is_ok());
    }

    /// For `interp` an unknown name fails the whole file; `hwaccel` deliberately accepts anything, as the CLI does.
    #[test]
    fn choices_are_stored_by_name() {
        let s: Settings = serde_json::from_str(r#"{"hwaccel": "vaapi", "interp": "bicubic"}"#)
            .expect("the names this build writes");
        assert_eq!(s.hwaccel, HwAccel::Backend("vaapi".into()));
        assert_eq!(s.interp, Interp::Bicubic);
        assert!(json(&s).contains(r#""interp":"bicubic""#));

        assert!(serde_json::from_str::<Settings>(r#"{"interp": "nearest"}"#).is_err());
    }
}

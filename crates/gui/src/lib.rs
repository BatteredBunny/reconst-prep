use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

use reconst_prep_core::cancel::CancelToken;
use reconst_prep_core::decode::ClipInfo;

use reconst_prep_core::mask::MaskSourceError;
use reconst_prep_core::output::ImageFormat;
use reconst_prep_core::pipeline::{
    OutputSpec, PipelineConfig, Progress, RunManifest, run_pipeline,
};
use reconst_prep_core::profiles::ProfileEntry;
use reconst_prep_core::undistort::{LensProfileFile, ProfileSummary};

mod clips;
mod icons;
mod preview;
#[cfg(test)]
mod screenshots;
mod settings;
mod theme;
mod ui;
mod widgets;

use preview::{MaskCache, PreviewData, PreviewRequest, PreviewUpdate, SharedFfmpeg};
use settings::{InterruptedRun, Settings};

/// What the command line handed the GUI; everything else comes from the persisted settings.
#[derive(Default, Debug)]
pub struct Start {
    pub inputs: Vec<PathBuf>,
    pub profile: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
}

/// Launch the GUI. Blocks until the window is closed.
pub fn run(start: Start) -> eframe::Result {
    let settings = Settings::load();
    let size = settings.window_size.unwrap_or([1360.0, 900.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([900.0, 600.0])
            .with_title("reconst-prep")
            .with_icon(theme::app_icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "reconst-prep",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::from_settings(settings).with(start)))
        }),
    )
}

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
    Preview(u64, PreviewUpdate),
    PreviewFailed(u64, String),
    Probed(PathBuf, Result<Box<ClipInfo>, String>),
    /// `None` when the clip cannot be decoded, which stops it being retried forever.
    Thumbnail(PathBuf, Option<Box<preview::DisplayImage>>),
    ProfileIndex(Result<Vec<ProfileEntry>, String>),
    ProfileFetched(Result<PathBuf, String>),
    ModelProgress(f32),
    ModelFetched(Result<PathBuf, String>),
}

/// A type rather than a bare channel because egui only redraws on input: a message sent without a repaint sits in the queue.
pub(crate) struct Emit {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Emit {
    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

/// The lens-profile browser: a search over Gyroflow's cached index.
#[derive(Default)]
pub(crate) struct ProfileBrowser {
    pub open: bool,
    pub query: String,
    /// Set when the dialog opens, cleared by the search box the frame it takes the caret.
    pub focus_query: bool,
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
    /// Named in the header beside a spinner.
    pub stage: Option<&'static str>,
    pub error: Option<String>,
    /// Bumped on every request; worker messages from any other generation are dropped on arrival.
    generation: u64,
    /// Replaced per request, so a superseded worker stops instead of finishing into the void.
    cancel: CancelToken,
    /// The request the current settings would issue, and when it last changed. See `auto_preview`.
    pending: Option<PreviewRequest>,
    pending_at: f64,
    pub previewed: Option<PreviewRequest>,
}

/// Preview pane view state: what is on screen, not what was computed.
pub(crate) struct PreviewView {
    /// Wipe position across the preview, 0 = all source, 1 = all output.
    pub wipe: f32,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub source_tex: Option<egui::TextureHandle>,
    pub output_tex: Option<egui::TextureHandle>,
    pub overlay_tex: Option<egui::TextureHandle>,
    /// Kept alive to fade out under the new one, so tuning a slider never flashes an empty pane.
    pub fading_out: Option<egui::TextureHandle>,
    pub fade_started: Option<f64>,
}

impl Default for PreviewView {
    fn default() -> Self {
        Self {
            wipe: 0.5,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            source_tex: None,
            output_tex: None,
            overlay_tex: None,
            fading_out: None,
            fade_started: None,
        }
    }
}

impl PreviewView {
    /// Every path that replaces the picture goes through here, so the outgoing one always fades.
    fn set_output(&mut self, ctx: &egui::Context, img: &preview::DisplayImage) {
        self.fading_out = self.output_tex.take();
        self.fade_started = Some(ctx.input(|i| i.time));
        self.output_tex = Some(ui::preview_pane::rgb_texture(ctx, "preview-output", img));
    }
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
    fn on_update(&mut self, ctx: &egui::Context, update: PreviewUpdate) {
        match update {
            PreviewUpdate::Raw(p) => {
                self.job.busy = false;
                self.view.set_output(ctx, &p.output);
                self.view.source_tex = Some(ui::preview_pane::rgb_texture(
                    ctx,
                    "preview-source",
                    &p.source,
                ));
                // The old frame's mask must not tint the new frame.
                self.view.overlay_tex = None;
                self.data = Some(*p);
            }
            PreviewUpdate::Stage(stage) => self.job.stage = Some(stage),
            PreviewUpdate::Rendered(img) => {
                self.job.stage = None;
                self.view.set_output(ctx, &img);
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
                self.view.overlay_tex = overlay
                    .as_ref()
                    .map(|o| ui::preview_pane::rgba_texture(ctx, "preview-overlay", o));
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
    pub toasts: Vec<ui::status::Toast>,

    /// A fact about the window, not the job.
    resume_prompt: ui::resume::Prompt,

    pub about_open: bool,
    /// The manifest of the run that just finished, while its summary is up.
    pub summary: Option<Arc<RunManifest>>,

    /// Tracked so the title is only re-sent to the compositor when it changes.
    shown_title: String,

    /// The profile as validated, against the path it came from. `validation`
    /// draws every repaint, and re-read and re-parsed the file each one. A
    /// profile edited in place is not noticed until it is picked again.
    profile_summary: Option<(PathBuf, Result<ProfileSummary, String>)>,

    channel: Channel,
}

/// The "s" in "3 clips", for any of the integer types the counts arrive as.
pub(crate) fn plural<N: PartialEq + From<u8>>(n: N) -> &'static str {
    if n == N::from(1) { "" } else { "s" }
}

pub(crate) struct Channel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Default for Channel {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl App {
    /// Takes whatever the model cache offers.
    fn from_settings(s: Settings) -> Self {
        Self::from_settings_with(s, reconst_prep_core::models::cached().into_iter().next())
    }

    /// `cached_model` is a parameter so tests do not depend on this machine's cache.
    fn from_settings_with(s: Settings, cached_model: Option<PathBuf>) -> Self {
        let mut app = Self {
            // A question at launch, never an action.
            interrupted: InterruptedRun::load(),
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
    fn with(mut self, start: Start) -> Self {
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
    fn to_settings(&self) -> Settings {
        Settings {
            recent_profiles: self.browser.recent.clone(),
            ..self.cfg.clone()
        }
    }

    /// A title that names the job, so several windows can be told apart.
    fn window_title(&self) -> String {
        let n = self.clips.files().len();
        let out = self.out_dir.as_deref().map(widgets::short_name);
        match (n, out) {
            (0, _) => "reconst-prep".to_string(),
            (n, Some(out)) => format!("reconst-prep - {n} clip{} → {out}", plural(n)),
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

    /// Every worker goes through here or [`Self::spawn_worker_with`], so none can forget the repaint that shows its answer.
    fn spawn_worker(
        &self,
        name: &str,
        ctx: &egui::Context,
        job: impl FnOnce() -> Msg + Send + 'static,
    ) {
        self.spawn_worker_with(name, ctx, move |out| out.send(job()));
    }

    /// A failed spawn panics on purpose: swallowing it strands the caller's busy flag on for the session.
    fn spawn_worker_with(
        &self,
        name: &str,
        ctx: &egui::Context,
        job: impl FnOnce(&Emit) + Send + 'static,
    ) {
        let out = Emit {
            tx: self.channel.tx.clone(),
            ctx: ctx.clone(),
        };
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || job(&out))
            .unwrap_or_else(|e| panic!("spawn {name} thread: {e}"));
    }

    pub fn probe_missing(&mut self, ctx: &egui::Context) {
        // Capped: dropping two hundred clips must not put two hundred threads and ffprobe processes in flight.
        const WORKERS: usize = 8;

        while let Some(path) = self.clips.claim_unprobed(WORKERS) {
            let ff = self.ffmpeg.clone();
            self.spawn_worker("probe", ctx, move || {
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
    pub fn request_thumbnails(&mut self, ctx: &egui::Context) {
        // Each decode is capped at two threads, so six at once still costs less CPU than one uncapped decode.
        const WORKERS: usize = 6;

        while let Some((path, duration_s)) = self.clips.claim_unthumbed(WORKERS) {
            let ff = self.ffmpeg.clone();
            self.spawn_worker("thumbnail", ctx, move || {
                let res = preview::build_thumbnail(&path, duration_s, &ff)
                    .map(Box::new)
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

    /// There is no Reload button; the settle wait is the whole mechanism, so a slider drag decodes once the gesture pauses.
    pub fn auto_preview(&mut self, ctx: &egui::Context) {
        const SETTLE_S: f64 = 0.25;

        let req = self.preview_request();
        let now = ctx.input(|i| i.time);
        let job = &mut self.preview.job;
        if req != job.pending {
            job.pending = req.clone();
            job.pending_at = now;
        }
        if req == job.previewed || job.busy {
            return;
        }
        if now - job.pending_at < SETTLE_S {
            // Nothing else would wake us at the end of the pause.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE_S));
            return;
        }
        job.previewed = req;
        self.request_preview(ctx);
    }

    pub fn request_preview(&mut self, ctx: &egui::Context) {
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
        job.stage = None;
        job.error = None;
        log::info!("preview: {} at {:.0}%", req.clip.stem, req.pos * 100.0);
        let (ff, masks) = (self.ffmpeg.clone(), self.masks.clone());
        self.spawn_worker_with("preview", ctx, move |out| {
            let mut emit = |u: PreviewUpdate| out.send(Msg::Preview(generation, u));
            if let Err(e) = preview::build_preview(&req, &ff, &masks, &cancel, &mut emit) {
                out.send(Msg::PreviewFailed(generation, format!("{e:#}")));
            }
        });
    }

    /// Explicit, one click, never during a run. The hash check lives in `models::fetch`.
    pub fn fetch_model(
        &mut self,
        entry: &'static reconst_prep_core::models::CatalogueEntry,
        ctx: &egui::Context,
    ) {
        if self.models.downloading.is_some() {
            return;
        }
        self.models.error = None;
        log::info!("downloading {} ({:.0} MB)", entry.name, entry.size_mb());
        self.models.downloading = Some((entry.file.to_string(), 0.0));
        // The picker has no Stop button yet, so nothing holds the other end.
        let cancel = CancelToken::never();
        self.spawn_worker_with("model-fetch", ctx, move |out| {
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
    pub fn start_run(&mut self, ctx: &egui::Context, resume: bool) {
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
        self.spawn_worker_with("pipeline", ctx, move |out| {
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
        let written = self.progress.as_ref().map_or(0, |p| p.total_written);
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
                self.toasts.push(ui::status::stopped_toast(&e, written));
                Err(e)
            }
        };
        // Re-read: whether there is anything to pick up is a question about the dataset on disk.
        self.interrupted = InterruptedRun::load();
        self.last_result = Some(result);
    }

    /// The validated lens profile, parsed once per path rather than per frame.
    pub fn profile_summary(&mut self) -> Option<&Result<ProfileSummary, String>> {
        let path = self.cfg.profile.clone()?;
        if self
            .profile_summary
            .as_ref()
            .is_none_or(|(p, _)| *p != path)
        {
            let parsed = LensProfileFile::load(&path)
                .map(|f| f.summary().clone())
                .map_err(|e| format!("{e:#}"));
            self.profile_summary = Some((path, parsed));
        }
        self.profile_summary.as_ref().map(|(_, s)| s)
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.channel.rx.try_recv() {
            match msg {
                Msg::Progress(p) => self.progress = Some(p),
                Msg::RunDone(r) => self.on_run_done(r),
                Msg::Preview(generation, _) | Msg::PreviewFailed(generation, _)
                    if generation != self.preview.job.generation => {} // superseded mid-flight
                Msg::Preview(_, update) => self.preview.on_update(ctx, update),
                Msg::PreviewFailed(_, e) => {
                    self.preview.job.busy = false;
                    self.preview.job.stage = None;
                    self.preview.job.error = Some(e);
                }
                Msg::Probed(path, res) => self.clips.on_probed(path, res.map(|b| *b)),
                Msg::Thumbnail(path, res) => {
                    let name = format!("thumb:{}", path.display());
                    let tex = res.map(|img| ui::preview_pane::rgb_texture(ctx, &name, &img));
                    self.clips.on_thumbnail(path, tex);
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
    }

    /// A `.json` is a lens profile, a `.onnx` is a segmentation model, anything else is footage.
    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
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

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        self.drain_messages(ctx);

        self.take_dropped(ctx);
        self.clips.refresh();
        self.probe_missing(ctx);
        self.request_thumbnails(ctx);

        self.auto_preview(ctx);

        // Icon, name, and the one control about the program rather than the job.
        egui::Panel::top("titlebar").show(root, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::new(&theme::icon_texture(ctx))
                        .fit_to_exact_size(egui::vec2(18.0, 18.0)),
                );
                ui.label(egui::RichText::new("reconst-prep").strong().size(13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::help_button(ui).clicked() {
                        self.about_open = true;
                    }
                });
            });
            ui.add_space(2.0);
        });

        // Before the columns, so it spans the whole window rather than only the part right of the config column.
        if ui::status::has_content(self) {
            egui::Panel::bottom("status").show(root, |ui| ui::status::show(self, ui, ctx));
        }

        egui::Panel::left("config")
            .default_size(390.0)
            .show(root, |ui| {
                // Pinned rather than appended: with every category open the column outgrows any window and Process scrolls off.
                egui::Panel::bottom("run_controls").show(ui, |ui| ui::run_controls(self, ui, ctx));
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| ui::config_column(self, ui, ctx));
            });

        egui::CentralPanel::default().show(root, |ui| ui::preview_pane::show(self, ui, ctx));

        ui::about::window(self, ctx);
        ui::lens::window(self, ctx);
        ui::summary::window(self, ctx);
        {
            // Split off so the prompt can hold a `&mut` to its own state and to the app at once.
            let mut prompt = std::mem::take(&mut self.resume_prompt);
            ui::resume::window(self, &mut prompt, ctx);
            self.resume_prompt = prompt;
        }
        ui::status::drop_overlay(ctx);
        ui::status::toasts(self, ctx);

        let title = self.window_title();
        if title != self.shown_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.shown_title = title;
        }
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.cfg.window_size = Some([rect.width(), rect.height()]);
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.to_settings().save();
        }
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

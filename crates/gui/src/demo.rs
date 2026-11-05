// The README screenshots. `RECONST_PREP_SCREENSHOTS=<dir>` plays every scene against the clip,
// lens profile and output folder given on the command line, saves each as `<dir>/<name>.png`, and exits.
// A run is started for the last scene and settings are saved as usual, so run this with its own
// XDG_CONFIG_HOME and XDG_STATE_HOME, as `nix run .#screenshots` does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

use crate::{App, ModeChoice, SizeChoice};

/// Rendered at twice the 1280x720 point layout, so text stays sharp on HiDPI screens and the
/// collage never has to scale a shot up. Assumes a display at scale 1, as Xvfb is.
const PIXELS_PER_POINT: f32 = 2.0;
pub const WINDOW: [f32; 2] = [1280.0 * PIXELS_PER_POINT, 720.0 * PIXELS_PER_POINT];

/// Long enough for a category to finish opening and the preview to finish fading in.
const SETTLE_S: f64 = 1.5;
/// A 4K frame segmented on the processor under a software renderer is slow, but not this slow.
const TIMEOUT_S: f64 = 300.0;

struct Scene {
    name: &'static str,
    /// Categories and subsections drawn open; every other one is closed.
    open: &'static [&'static str],
    setup: fn(&mut App, &egui::Context),
    /// On top of the preview being finished for the settings on screen.
    ready: fn(&App) -> bool,
}

const SCENES: &[Scene] = &[
    Scene {
        name: "main",
        open: &["cat_videos", "cat_lens", "cat_mask"],
        setup: |app, _| {
            app.cfg.undistort = app.cfg.profile.is_some();
            app.cfg.mask_sky = true;
            app.cfg.mask_people = app.cfg.seg_model.is_some();
            app.view.wipe = 0.5;
        },
        ready: |_| true,
    },
    Scene {
        name: "lens",
        open: &["cat_videos", "cat_lens"],
        setup: |app, _| {
            app.cfg.mask_sky = false;
            app.cfg.mask_people = false;
            app.view.wipe = 0.5;
        },
        ready: |_| true,
    },
    Scene {
        name: "masking",
        open: &["cat_mask", "sky_params", "seg_params"],
        setup: |app, _| {
            app.cfg.mask_sky = true;
            app.cfg.mask_people = app.cfg.seg_model.is_some();
            app.view.wipe = 0.3;
        },
        ready: |_| true,
    },
    Scene {
        name: "selection",
        open: &["cat_select", "cat_filters", "blur_params", "cat_output"],
        setup: |app, _| {
            app.cfg.mode = ModeChoice::Motion;
            app.cfg.blur_floor_on = true;
            app.cfg.size_choice = SizeChoice::Scale;
            app.cfg.scale_factor = 0.5;
        },
        ready: |_| true,
    },
    Scene {
        name: "processing",
        open: &["cat_videos", "cat_select", "cat_output"],
        setup: |app, ctx| app.start_run(ctx, false),
        // The demo clip is a still, so movement-based selection keeps little of it: count decoded frames instead.
        ready: |app| {
            app.progress
                .as_ref()
                .is_some_and(|p| p.total_written >= 1 && p.clip_decoded >= 30)
        },
    },
];

/// Whether the demo pins `id_source` open or closed. `None` outside screenshot mode.
pub fn forced_open(ctx: &egui::Context, id_source: &str) -> Option<bool> {
    let open: &'static [&'static str] = ctx.data(|d| d.get_temp(open_id()))?;
    Some(open.contains(&id_source))
}

fn open_id() -> egui::Id {
    egui::Id::unique("demo_open")
}

#[derive(Clone, Copy)]
enum Phase {
    Start,
    Waiting {
        since: f64,
        ready_since: Option<f64>,
    },
    Shooting,
}

pub struct Screenshots {
    out: PathBuf,
    scene: usize,
    phase: Phase,
    failed: Vec<String>,
    shots: (
        Sender<Arc<egui::ColorImage>>,
        Receiver<Arc<egui::ColorImage>>,
    ),
}

impl Screenshots {
    pub fn from_env() -> Option<Self> {
        let out = PathBuf::from(std::env::var_os("RECONST_PREP_SCREENSHOTS")?);
        Some(Self {
            out,
            scene: 0,
            phase: Phase::Start,
            failed: Vec::new(),
            shots: unbounded(),
        })
    }

    pub fn drive(self, app: App) -> Box<dyn eframe::App> {
        Box::new(Driven { app, demo: self })
    }

    fn step(&mut self, app: &mut App, ctx: &egui::Context) {
        // Workers wake the UI when they answer, but nothing wakes it when a settle wait ends.
        ctx.request_repaint_after(Duration::from_millis(100));
        if ctx.pixels_per_point() != PIXELS_PER_POINT {
            ctx.set_pixels_per_point(PIXELS_PER_POINT);
        }
        let now = ctx.input(|i| i.time);
        let Some(scene) = SCENES.get(self.scene) else {
            // Exiting under a running pipeline crashes in the GPU driver's teardown.
            if app.running {
                app.cancel.cancel();
                return;
            }
            self.finish();
        };

        match self.phase {
            Phase::Start => {
                if app.clips.inputs.is_empty() {
                    self.failed.push("no clip given on the command line".into());
                    self.finish();
                }
                ctx.data_mut(|d| d.insert_temp(open_id(), scene.open));
                (scene.setup)(app, ctx);
                self.phase = Phase::Waiting {
                    since: now,
                    ready_since: None,
                };
            }
            Phase::Waiting { since, ready_since } => {
                if let Some(e) = app.preview_error.clone() {
                    self.fail(format!("preview failed: {e}"));
                } else if !(preview_finished(app) && (scene.ready)(app)) {
                    self.phase = Phase::Waiting {
                        since,
                        ready_since: None,
                    };
                    if now - since > TIMEOUT_S {
                        self.fail("timed out".into());
                    }
                } else {
                    let ready_since = ready_since.unwrap_or(now);
                    self.phase = if now - ready_since >= SETTLE_S {
                        self.shoot(ctx);
                        Phase::Shooting
                    } else {
                        Phase::Waiting {
                            since,
                            ready_since: Some(ready_since),
                        }
                    };
                }
            }
            Phase::Shooting => {
                if let Ok(image) = self.shots.1.try_recv() {
                    let path = self.out.join(format!("{}.png", scene.name));
                    match save(&image, &path) {
                        Ok(()) => log::info!("wrote {}", path.display()),
                        Err(e) => self.failed.push(format!("{}: {e:#}", path.display())),
                    }
                    self.next();
                }
            }
        }
    }

    fn shoot(&self, ctx: &egui::Context) {
        let tx = self.shots.0.clone();
        let wake = ctx.clone();
        ctx.request_screenshot(move |image| {
            let _ = tx.send(image);
            wake.request_repaint();
        });
    }

    fn fail(&mut self, reason: String) {
        self.failed
            .push(format!("{}: {reason}", SCENES[self.scene].name));
        self.next();
    }

    fn next(&mut self) {
        self.scene += 1;
        self.phase = Phase::Start;
    }

    fn finish(&self) -> ! {
        for failure in &self.failed {
            log::error!("screenshot failed: {failure}");
        }
        // Not a viewport close: that saves the window's settings on the way out.
        std::process::exit(if self.failed.is_empty() { 0 } else { 1 });
    }
}

/// The frame on screen is the one the current settings ask for, masks and all, beside a clip thumbnail.
fn preview_finished(app: &App) -> bool {
    let thumbs_in = app
        .clips
        .files()
        .iter()
        .all(|f| app.clips.thumbs.contains_key(f));
    let current = app.previewed.is_some() && app.previewed == app.preview_request();
    let finished = app.preview.as_ref().is_some_and(|p| p.sharpness.is_some());
    thumbs_in && current && finished && !app.preview_busy && app.preview_stage.is_none()
}

fn save(image: &egui::ColorImage, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    image::save_buffer(
        path,
        image.as_raw(),
        image.width() as u32,
        image.height() as u32,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(())
}

struct Driven {
    app: App,
    demo: Screenshots,
}

impl eframe::App for Driven {
    fn ui(&mut self, root: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.demo.step(&mut self.app, root.ctx());
        eframe::App::ui(&mut self.app, root, frame);
    }
}

// The README screenshots, rendered offscreen by egui_kittest. Ignored by default: it needs the demo clip,
// lens profile and segmentation model that `nix build .#screenshots` hands it, and starting a run saves
// settings, so it refuses to run unless XDG_CONFIG_HOME and XDG_STATE_HOME point away from your own.
//
// Writes `<name>.png` with callouts, cropped to what they talk about, and `clean/<name>.png`, the whole
// window without them, for the collage.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT as _, Queryable as _};

use crate::settings::Settings;
use crate::{App, ModeChoice, SizeChoice, Start, theme};

const WINDOW: egui::Vec2 = egui::vec2(1280.0, 720.0);
/// Twice the point layout, so text stays sharp on HiDPI screens and the collage never scales a shot up.
const PIXELS_PER_POINT: f32 = 2.0;
/// A 4K frame segmented on the processor is slow, but not this slow.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Frames for a section to finish opening and the preview to finish fading in.
const SETTLE_STEPS: usize = 20;
/// Space kept around what a cropped shot is about.
const CROP_MARGIN: f32 = 14.0;

struct Scene {
    name: &'static str,
    /// Sections drawn open, by the name their toggle announces; every other one is closed.
    open: &'static [&'static str],
    setup: fn(&mut Harness<'_, Shot>),
    /// On top of the preview being finished for the settings on screen.
    ready: fn(&App) -> bool,
    /// Also kept in the crop, besides the callouts and what they point at. `None` keeps the whole window.
    crop: Option<&'static [&'static str]>,
    callouts: &'static [Callout],
}

const SCENES: &[Scene] = &[
    Scene {
        name: "main",
        open: &["Videos", "Lens correction", "Masking"],
        setup: |h| {
            let app = &mut h.state_mut().app;
            app.cfg.undistort = app.cfg.profile.is_some();
            app.cfg.mask_sky = true;
            app.cfg.mask_people = app.cfg.seg_model.is_some();
            app.view.wipe = 0.5;
        },
        ready: |_| true,
        crop: None,
        callouts: &[
            // Clear of the card's remove button.
            Callout::new(Right, "Drop videos here", Containing("DJI_0021.mp4")).gap(180.0),
            Callout::new(
                Right,
                "Drag to compare before and after",
                Inside("Before and after", |app, r| {
                    egui::pos2(
                        r.lerp_inside(egui::vec2(app.view.wipe, 0.5)).x,
                        r.center().y,
                    )
                }),
            ),
            Callout::new(Above, "Step one frame", Named("One frame back")),
            Callout::new(
                Above,
                "Scrub through the clip",
                Inside("Position in the clip", |app, r| {
                    egui::pos2(
                        r.lerp_inside(egui::vec2(app.preview_pos as f32, 0.0)).x,
                        r.top(),
                    )
                }),
            ),
            Callout::new(Right, "Start", Named("PROCESS")),
        ],
    },
    Scene {
        name: "lens",
        open: &["Videos", "Lens correction"],
        setup: |h| {
            let app = &mut h.state_mut().app;
            app.cfg.mask_sky = false;
            app.cfg.mask_people = false;
            app.view.wipe = 0.5;
        },
        ready: |_| true,
        crop: Some(&["Lens correction section", "interpolation"]),
        callouts: &[
            Callout::new(
                Right,
                "Profile from a file or Gyroflow's database",
                Named("Search database…"),
            ),
            Callout::new(
                Right,
                "Undistort on the graphics card",
                Named("Use the graphics card"),
            ),
        ],
    },
    Scene {
        name: "masking",
        open: &["Masking", "Sky advanced options", "People advanced options"],
        setup: |h| {
            let app = &mut h.state_mut().app;
            app.cfg.mask_sky = true;
            app.cfg.mask_people = app.cfg.seg_model.is_some();
            app.view.wipe = 0.3;
        },
        ready: |_| true,
        crop: Some(&["Masking section"]),
        callouts: &[
            Callout::new(Right, "Tune sky detection", Named("Sky advanced options")).gap(40.0),
            Callout::new(
                Right,
                "People masking uses a segmentation model",
                Named("People advanced options"),
            )
            .gap(40.0),
            Callout::new(
                Below,
                "Masked sky, tinted",
                Inside("Before and after", |_, r| {
                    r.lerp_inside(egui::vec2(0.6, 0.1))
                }),
            ),
        ],
    },
    Scene {
        name: "selection",
        open: &[
            "Frame selection",
            "Filters",
            "Blur filter advanced options",
            "Output",
        ],
        setup: |h| {
            let app = &mut h.state_mut().app;
            app.cfg.mode = ModeChoice::Motion;
            app.cfg.blur_floor_on = true;
            app.cfg.size_choice = SizeChoice::Scale;
            app.cfg.scale_factor = 0.5;
        },
        ready: |_| true,
        crop: Some(&["Frame selection section"]),
        callouts: &[
            Callout::new(
                Right,
                "Keep a frame once the camera moved enough",
                Named("movement needed"),
            )
            .gap(40.0),
            Callout::new(
                Right,
                "Drop blurry frames",
                Named("Blur filter advanced options"),
            )
            .gap(40.0),
            Callout::new(Right, "Output size and format", Named("exact")).gap(40.0),
        ],
    },
    Scene {
        name: "processing",
        open: &["Videos", "Frame selection", "Output"],
        setup: |h| h.get_by_label("PROCESS").click_accesskit(),
        // The demo clip is a still, so movement-based selection keeps little of it: count decoded frames instead.
        ready: |app| {
            app.progress
                .as_ref()
                .is_some_and(|p| p.total_written >= 1 && p.clip_decoded >= 30)
        },
        crop: Some(&["PROCESS"]),
        callouts: &[
            Callout::new(
                Above,
                "Cancel any time, written frames stay",
                Named("CANCEL"),
            )
            .gap(30.0),
            Callout::new(
                Above,
                "Frames kept, decoded and time left",
                Containing(" decoded"),
            )
            .gap(30.0),
        ],
    },
];

#[derive(Clone, Copy)]
enum Side {
    Right,
    Above,
    Below,
}
use Side::{Above, Below, Right};

/// What a callout points at, found through the names the app gives screen readers.
#[derive(Clone, Copy)]
enum Target {
    Named(&'static str),
    /// A name or a shown text with this in it, for one that carries a path or a count. The outermost
    /// one: a clip's thumbnail is named after it too.
    Containing(&'static str),
    /// A spot within a widget that the app paints rather than lays out, like the wipe's handle.
    Inside(&'static str, fn(&App, egui::Rect) -> egui::Pos2),
}
use Target::{Containing, Inside, Named};

impl Target {
    /// Everything with the name together, so a slider takes in its number box and caption, and a
    /// clip's card its thumbnail.
    fn find(self, harness: &Harness<'_, Shot>) -> egui::Rect {
        match self {
            // A plain label carries its text as its value, as kittest's own queries assume.
            Named(name) => around(harness, name, |node| {
                let text = match node.role() {
                    egui::accesskit::Role::Label => node.value(),
                    _ => node.label(),
                };
                text.as_deref() == Some(name)
            }),
            Containing(part) => around(harness, part, |node| {
                let has = |text: Option<String>| text.is_some_and(|t| t.contains(part));
                // The window is titled after the clip as well.
                node.role() != egui::accesskit::Role::Window
                    && (has(node.label()) || has(node.value()))
            }),
            Inside(name, at) => {
                let rect = Named(name).find(harness);
                egui::Rect::from_center_size(at(&harness.state().app, rect), egui::Vec2::ZERO)
            }
        }
    }
}

fn around(
    harness: &Harness<'_, Shot>,
    what: &str,
    matches: impl Fn(&AccessKitNode<'_>) -> bool,
) -> egui::Rect {
    let rect = harness
        .query_all_by(|node| matches(node))
        .map(|node| node.rect())
        .fold(egui::Rect::NOTHING, egui::Rect::union);
    assert!(rect.is_positive(), "nothing on screen is called {what:?}");
    if dump_tree() {
        eprintln!("{what:?} is at {rect:?}");
    }
    rect
}

/// A red label with an arrow to what it explains.
#[derive(Clone, Copy)]
struct Callout {
    text: &'static str,
    target: Target,
    side: Side,
    /// From the target's edge to the label's.
    gap: f32,
}

impl Callout {
    const fn new(side: Side, text: &'static str, target: Target) -> Self {
        Self {
            text,
            target,
            side,
            gap: 60.0,
        }
    }

    const fn gap(self, gap: f32) -> Self {
        Self { gap, ..self }
    }
}

/// Where a callout landed, once its target has been found.
struct Placed {
    callout: Callout,
    target: egui::Rect,
}

const RED: egui::Color32 = egui::Color32::from_rgb(0xff, 0x4d, 0x4d);

impl Placed {
    /// Returns the label's rect, for the crop.
    fn paint(&self, painter: &egui::Painter) -> egui::Rect {
        let Callout {
            text, side, gap, ..
        } = self.callout;
        let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), RED);
        let padding = egui::vec2(7.0, 5.0);
        let size = galley.size() + 2.0 * padding;
        let t = self.target;
        let (centre, start, tip) = match side {
            Side::Right => {
                let c = egui::pos2(t.right() + gap + size.x / 2.0, t.center().y);
                (c, c - egui::vec2(size.x / 2.0, 0.0), t.right_center())
            }
            Side::Above => {
                let c = egui::pos2(t.center().x, t.top() - gap - size.y / 2.0);
                (c, c + egui::vec2(0.0, size.y / 2.0), t.center_top())
            }
            Side::Below => {
                let c = egui::pos2(t.center().x, t.bottom() + gap + size.y / 2.0);
                (c, c - egui::vec2(0.0, size.y / 2.0), t.center_bottom())
            }
        };
        let label = egui::Rect::from_center_size(centre, size);

        // The line stops at the arrowhead's base, so it does not poke through the point.
        let head = 9.0;
        let along = (tip - start).normalized();
        let base = tip - along * head;
        let across = along.rot90() * head * 0.45;
        painter.line_segment([start, base], egui::Stroke::new(2.0, RED));
        painter.add(egui::Shape::convex_polygon(
            vec![tip, base + across, base - across],
            RED,
            egui::Stroke::NONE,
        ));

        painter.rect(
            label,
            7.0,
            egui::Color32::from_rgba_unmultiplied(16, 17, 20, 230),
            egui::Stroke::new(1.0, RED),
            egui::StrokeKind::Inside,
        );
        painter.galley(label.min + padding, galley, RED);
        label
    }
}

/// The app, with the scene's callouts drawn over it.
struct Shot {
    app: App,
    callouts: Vec<Placed>,
    labels: Vec<egui::Rect>,
}

impl eframe::App for Shot {
    fn ui(&mut self, root: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, root, frame);
        let painter = root.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("callouts"),
        ));
        self.labels = self.callouts.iter().map(|c| c.paint(&painter)).collect();
    }
}

#[test]
#[ignore = "renders the README screenshots; run through `nix build .#screenshots`"]
fn readme() {
    for var in ["XDG_CONFIG_HOME", "XDG_STATE_HOME"] {
        assert!(
            std::env::var_os(var).is_some(),
            "set {var}: starting a run saves settings, and these must not overwrite your own"
        );
    }
    let path =
        |var: &str| PathBuf::from(std::env::var_os(var).unwrap_or_else(|| panic!("set {var}")));
    let out = path("SCREENSHOTS_OUT");
    let start = Start {
        inputs: vec![path("SCREENSHOTS_CLIP")],
        profile: Some(path("SCREENSHOTS_PROFILE")),
        out_dir: Some(std::env::temp_dir().join("reconst-prep-screenshots")),
    };
    let model = std::env::var_os("SCREENSHOTS_MODEL").map(PathBuf::from);

    let mut harness = Harness::builder()
        .with_size(WINDOW)
        .with_pixels_per_point(PIXELS_PER_POINT)
        .wgpu()
        .build_eframe(|cc| {
            theme::install(&cc.egui_ctx);
            Shot {
                // The user's own settings would make the screenshots differ from machine to machine.
                app: App::from_settings_with(Settings::default(), model).with(start),
                callouts: Vec::new(),
                labels: Vec::new(),
            }
        });

    for scene in SCENES {
        shoot(&mut harness, scene, &out);
    }

    // Dropping the harness under a running pipeline crashes in the GPU driver's teardown.
    if harness.state().app.running {
        harness.get_by_label("CANCEL").click_accesskit();
        wait_until(&mut harness, "the run to stop", |app| !app.running);
    }
}

fn shoot(harness: &mut Harness<'_, Shot>, scene: &Scene, out: &Path) {
    harness.state_mut().callouts.clear();
    open_only(harness, scene.open);
    (scene.setup)(harness);
    wait_until(harness, scene.name, |app| {
        preview_finished(app) && (scene.ready)(app)
    });
    harness.run_steps(SETTLE_STEPS);

    if dump_tree() {
        eprintln!("--- {} ---\n{:#?}", scene.name, harness.root());
    }
    save(
        &render(harness),
        &out.join("clean").join(format!("{}.png", scene.name)),
    );

    let callouts = scene
        .callouts
        .iter()
        .map(|&callout| Placed {
            callout,
            target: callout.target.find(harness),
        })
        .collect();
    harness.state_mut().callouts = callouts;
    harness.step();

    let mut image = render(harness);
    if let Some(keep) = scene.crop {
        let state = harness.state();
        let area = state
            .labels
            .iter()
            .copied()
            .chain(state.callouts.iter().map(|c| c.target))
            .chain(keep.iter().map(|label| harness.get_by_label(label).rect()))
            .fold(egui::Rect::NOTHING, egui::Rect::union)
            .expand(CROP_MARGIN)
            .intersect(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW));
        let px = |v: f32| (v * PIXELS_PER_POINT).round() as u32;
        image = image::imageops::crop_imm(
            &image,
            px(area.left()),
            px(area.top()),
            px(area.width()),
            px(area.height()),
        )
        .to_image();
    }
    save(&image, &out.join(format!("{}.png", scene.name)));
}

/// Clicks section toggles until exactly `open` are open. One click at a time: opening or closing a
/// section shows or hides the toggles inside it.
fn open_only(harness: &mut Harness<'_, Shot>, open: &[&str]) {
    for _ in 0..32 {
        let wrong = harness
            .query_all_by_role(egui::accesskit::Role::DisclosureTriangle)
            .find(|node| {
                let node = node.accesskit_node();
                let name = node.label().unwrap_or_default();
                let name = name.strip_suffix(" section").unwrap_or(&name);
                // A category's title can carry a count, as in "Videos (2)".
                let wanted = open
                    .iter()
                    .any(|o| name == *o || name.starts_with(&format!("{o} (")));
                let is_open = node.toggled() == Some(egui::accesskit::Toggled::True);
                wanted != is_open
            });
        let Some(toggle) = wrong else {
            harness.run_steps(SETTLE_STEPS);
            return;
        };
        toggle.click_accesskit();
        harness.run_steps(2);
    }
    panic!("sections did not settle on {open:?}");
}

fn wait_until(harness: &mut Harness<'_, Shot>, what: &str, done: impl Fn(&App) -> bool) {
    let since = Instant::now();
    loop {
        harness.step();
        let app = &harness.state().app;
        if let Some(e) = &app.preview_error {
            panic!("{what}: preview failed: {e}");
        }
        if done(app) {
            return;
        }
        assert!(since.elapsed() < TIMEOUT, "{what}: timed out");
        // Workers answer on their own threads; this is only how often to look.
        std::thread::sleep(Duration::from_millis(20));
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

/// Logs where callouts land and the accessibility tree, for finding what to name a target.
fn dump_tree() -> bool {
    std::env::var_os("SCREENSHOTS_DUMP_TREE").is_some()
}

fn render(harness: &mut Harness<'_, Shot>) -> image::RgbaImage {
    harness.render().expect("rendering the window")
}

fn save(image: &image::RgbaImage, path: &Path) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("creating the output folder");
    }
    image
        .save(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

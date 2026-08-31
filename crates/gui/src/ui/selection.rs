// Nothing here affects the preview; selection numbers show up in the status bar during a run.

use eframe::egui;

use crate::icons;
use crate::settings::Settings;
use crate::widgets::{category, pick, slider};
use crate::{App, ModeChoice};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    category(
        ui,
        "cat_select",
        icons::SELECTION,
        "Frame selection",
        None,
        true,
        |ui| {
            ui.horizontal(|ui| {
                pick(ui, &mut app.cfg.mode, ModeChoice::Motion, "on movement").on_hover_text(
                    "Keeps a frame once the shot has moved on from the last one kept, and \
                     takes the sharpest frame available.\nBest for reconstruction. Even \
                     coverage, no near-duplicates.",
                );
                pick(ui, &mut app.cfg.mode, ModeChoice::EveryNth, "every Nth")
                    .on_hover_text("Fixed spacing. Ignores what is in the frame. Predictable.");
            });

            match app.cfg.mode {
                ModeChoice::Motion => {
                    ui.add(
                        egui::Slider::new(&mut app.cfg.motion_threshold, 0.005..=0.3)
                            .logarithmic(true)
                            .text("movement needed"),
                    )
                    .on_hover_text(
                        "How much the shot must change before the next frame is kept. Lower \
                         values keep more frames.\nDrone footage usually works between 0.02 \
                         and 0.10.",
                    );
                    slider(
                        ui,
                        &mut app.cfg.window,
                        1..=30,
                        "window (frames)",
                        Settings::default().window,
                        "How many frames to compare before choosing. The sharpest one wins.",
                    );
                }
                ModeChoice::EveryNth => {
                    slider(
                        ui,
                        &mut app.cfg.nth,
                        1..=120,
                        "keep every Nth",
                        Settings::default().nth,
                        "Keeps one frame out of every N.",
                    );
                }
            }
        },
    );
}

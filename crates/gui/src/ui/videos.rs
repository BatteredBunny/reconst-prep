// Geometry is shown because a batch whose clips differ in frame size fails partway through a run.

use eframe::egui;

use crate::App;
use crate::icons::{self, icon_button};
use crate::widgets::{category, mmss, mono, short_name, warn};

/// The decoded copy is larger (`preview::THUMB_W`) so it stays sharp at 2x scaling.
const THUMB: egui::Vec2 = egui::vec2(96.0, 54.0);
/// Row height, fixed, set by the thumbnail.
const ROW_H: f32 = 62.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let n = app.clips.files().len();
    let title = if n == 0 {
        "Videos".to_string()
    } else {
        format!("Videos ({n})")
    };
    category(ui, "cat_videos", icons::VIDEOS, &title, None, true, |ui| {
        ui.horizontal(|ui| {
            if icons::button(ui, icons::ADD_FILES, "Add files…").clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter(
                        "video",
                        &crate::widgets::extensions(crate::widgets::VIDEO_EXT),
                    )
                    .pick_files()
            {
                for f in files {
                    app.clips.add(f);
                }
            }
            if icons::button(ui, icons::ADD_FOLDER, "Add folder…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                app.clips.add(dir);
            }
            if !app.clips.inputs.is_empty()
                && icons::button(ui, icons::CLEAR, "Clear")
                    .on_hover_text("Remove every clip in the list.")
                    .clicked()
            {
                app.clips.clear();
                app.preview = None;
            }
        });

        let sizes = app.clips.mixed_resolutions();
        if sizes.len() > 1 {
            warn(
                ui,
                format!(
                    "clips have {} different frame sizes ({}). A run needs one size. \
                     Split them into separate runs.",
                    sizes.len(),
                    sizes
                        .iter()
                        .map(|(w, h)| format!("{w}×{h}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }

        let mut remove = None;
        let mut select = None;
        // No frame at all while the list is empty: an empty bordered box reads as a broken widget.
        if !app.clips.files().is_empty() {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(2))
                .show(ui, |ui| {
                    // Borrowed, not copied: both decisions a row can produce are carried out after the loop.
                    let app = &*app;
                    for (i, path) in app.clips.files().iter().enumerate() {
                        match row(app, ui, path, i == app.preview_clip) {
                            Some(Action::Select) => select = Some(i),
                            Some(Action::Remove) => remove = Some(path.clone()),
                            None => {}
                        }
                    }
                });
        }

        if let Some(i) = select {
            app.preview_clip = i;
        }
        if let Some(p) = remove {
            app.clips.remove(&p);
            app.preview_clip = app
                .preview_clip
                .min(app.clips.files().len().saturating_sub(1));
        }
    });
}

enum Action {
    Select,
    Remove,
}

/// The whole row is the click target.
fn row(app: &App, ui: &mut egui::Ui, path: &std::path::Path, selected: bool) -> Option<Action> {
    // Reserve a slot in the paint list so the background draws *under* content not yet laid out.
    let bg = ui.painter().add(egui::Shape::Noop);
    let mut action = None;

    let response = ui
        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            ui.set_min_height(ROW_H);
            ui.horizontal(|ui| {
                ui.add_space(3.0);
                thumbnail(app, ui, path);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(mono(short_name(path)).size(12.0));
                    meta(app, ui, path);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(3.0);
                    if app.clips.inputs.iter().any(|i| i == path)
                        && icon_button(ui, icons::REMOVE, "Remove this clip").clicked()
                    {
                        action = Some(Action::Remove);
                    }
                });
            });
        })
        .response;

    if response.clicked() && action.is_none() {
        action = Some(Action::Select);
    }

    // Selection is an amber bar down the left edge plus a lift in the fill.
    let fill = match (selected, response.hovered()) {
        (true, _) => egui::Color32::from_rgb(0x25, 0x27, 0x2c),
        (false, true) => egui::Color32::from_rgb(0x1c, 0x1e, 0x22),
        (false, false) => egui::Color32::TRANSPARENT,
    };
    let mut shapes = vec![egui::Shape::rect_filled(response.rect, 2, fill)];
    if selected {
        shapes.push(egui::Shape::rect_filled(
            egui::Rect::from_min_max(
                response.rect.left_top(),
                egui::pos2(response.rect.left() + 2.0, response.rect.bottom()),
            ),
            0,
            crate::theme::AMBER,
        ));
    }
    ui.painter().set(bg, egui::Shape::Vec(shapes));

    response.on_hover_text(path.display().to_string());
    action
}

/// Or a placeholder of exactly the same size, so the row does not change height when the decode lands.
fn thumbnail(app: &App, ui: &mut egui::Ui, path: &std::path::Path) {
    match app.clips.thumbs.get(path) {
        Some(Some(tex)) => {
            ui.add(egui::Image::new((tex.id(), THUMB)).corner_radius(2));
        }
        other => {
            let (rect, _) = ui.allocate_exact_size(THUMB, egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 2, egui::Color32::from_rgb(0x0a, 0x0b, 0x0d));
            if matches!(other, Some(None)) {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::FAILED,
                    egui::FontId::proportional(15.0),
                    egui::Color32::from_gray(70),
                );
            } else {
                // Painted by hand: the rect is already allocated, so a widget here would drag the layout cursor back.
                let time = ui.input(|i| i.time);
                let start = time * std::f64::consts::TAU; // one turn per second
                let sweep = std::f64::consts::TAU * 0.75;
                let n = 24;
                let points: Vec<egui::Pos2> = (0..=n)
                    .map(|i| {
                        let a = start + sweep * i as f64 / n as f64;
                        rect.center() + 9.0 * egui::vec2(a.cos() as f32, a.sin() as f32)
                    })
                    .collect();
                ui.painter().add(egui::Shape::line(
                    points,
                    egui::Stroke::new(2.0, egui::Color32::from_gray(95)),
                ));
                ui.ctx().request_repaint();
            }
        }
    }
}

fn meta(app: &App, ui: &mut egui::Ui, path: &std::path::Path) {
    let line = |ui: &mut egui::Ui, text: String| {
        ui.label(mono(text).weak().size(10.0));
    };
    match app.clips.probes.get(path) {
        Some(Ok(info)) => {
            let duration = info.duration_s.map(mmss).unwrap_or_else(|| "-".to_string());
            line(
                ui,
                format!("{duration} · {}", info.codec.as_deref().unwrap_or("?")),
            );
            line(
                ui,
                format!("{}×{} · {:.2} fps", info.width, info.height, info.fps),
            );
        }
        None if app.clips.probing(path) => {
            line(ui, "probing…".to_string());
            line(ui, " ".to_string());
        }
        Some(Err(e)) => {
            ui.label(
                mono(format!("{} unreadable", icons::FAILED))
                    .color(ui.visuals().error_fg_color)
                    .size(10.0),
            )
            .on_hover_text(e);
            line(ui, " ".to_string());
        }
        None => {
            line(ui, " ".to_string());
            line(ui, " ".to_string());
        }
    }
}

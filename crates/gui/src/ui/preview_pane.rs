// A wipe rather than two panes, because the differences that matter are small and local: a straightened line, a mask edge, a crop.

use eframe::egui;

use crate::App;
use crate::preview::DisplayImage;
use crate::widgets::mono;

/// How near the split the pointer must be for a drag to move the wipe rather than pan.
const WIPE_GRAB: f32 = 14.0;
const HANDLE_R: f32 = 13.0;

/// Height of the scrubber block. Fixed, so the picture above it never moves.
const SCRUBBER_H: f32 = 78.0;
/// The strip under the track where the time labels go.
const AXIS_H: f32 = 12.0;
/// Uniform, so a row cannot shuffle as its contents change.
const SQUARE: egui::Vec2 = egui::vec2(24.0, 22.0);

pub fn rgb_texture(ctx: &egui::Context, name: &str, img: &DisplayImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([img.w as usize, img.h as usize], &img.rgb),
        egui::TextureOptions::LINEAR,
    )
}

pub fn rgba_texture(ctx: &egui::Context, name: &str, img: &DisplayImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.rgb),
        // Nearest: a mask edge is a hard edge, and it is the thing being judged.
        egui::TextureOptions::NEAREST,
    )
}

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    if let Some(e) = &app.preview_error {
        crate::widgets::error(ui, e.clone());
    }

    if app.preview.is_none() {
        empty_state(app, ui);
        return;
    }
    // egui only redraws on input, so animation has to ask.
    if app.preview_busy || app.view.fade_started.is_some() {
        ctx.request_repaint();
    }

    header_line(app, ui);
    image_view(app, ui);
    ui.add_space(6.0);
    scrubber(app, ui);
}

/// "add footage" only when there is none, and a spinner while the first preview is still decoding.
fn empty_state(app: &App, ui: &mut egui::Ui) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        if app.clips.files().is_empty() {
            ui.label(
                egui::RichText::new("Add input footage to proceed.")
                    .weak()
                    .size(14.0),
            );
        } else if app.preview_error.is_none() {
            ui.spinner();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Decoding preview…").weak().size(14.0));
        }
    });
}

/// No geometry: the input size is on the clip's row and the output size is a setting in Output.
fn header_line(app: &mut App, ui: &mut egui::Ui) {
    let Some(p) = &app.preview else { return };
    let mut text = format!(
        "{} @ {:.1}s   frame {}",
        p.clip_stem, p.start_s, p.frame_index
    );
    // Computed regardless, so the toggle is free; shown only while the blur filter gives the number a meaning.
    if app.cfg.blur_floor_on
        && let Some(s) = p.sharpness
    {
        text.push_str(&format!("   sharpness {s:.0}"));
    }
    if p.masked_fraction > 0.0 {
        text.push_str(&format!("   {:.0}% masked", p.masked_fraction * 100.0));
    }
    ui.horizontal(|ui| {
        ui.label(mono(text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let working = if app.preview_busy {
                Some("decoding")
            } else {
                app.preview_stage
            };
            if let Some(w) = working {
                ui.label(mono(format!("{w}…")).weak().size(11.0));
                ui.spinner();
            }
        });
    });
}

/// Source and output in one rect, split by the wipe, with the mask overlay on the output side only.
fn image_view(app: &mut App, ui: &mut egui::Ui) {
    let (Some(source), Some(output)) = (&app.view.source_tex, &app.view.output_tex) else {
        return;
    };
    let size = output.size();
    let aspect = size[1] as f32 / size[0] as f32;
    let avail = ui.available_size();
    // Leave room for the scrubber below.
    let max_h = (avail.y - SCRUBBER_H - 26.0).max(160.0);
    let w = avail.x.min(max_h / aspect);
    if !(w.is_finite() && w > 1.0) {
        return; // the pane has collapsed to nothing
    }
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(w, w * aspect), egui::Sense::click_and_drag());

    let split = |t: f32| rect.left() + t.clamp(0.0, 1.0) * rect.width();
    let handle_centre = egui::pos2(split(app.view.wipe), rect.center().y);

    // A drag starting near the split moves the wipe, anywhere else it pans; decided once, so a pan crossing the line does not lurch.
    let near_split = response
        .interact_pointer_pos()
        .or(response.hover_pos())
        .is_some_and(|pos| {
            (pos.x - handle_centre.x).abs() <= WIPE_GRAB
                || pos.distance(handle_centre) <= HANDLE_R * 1.6
        });
    let wiping = ui.data_mut(|d| {
        let id = response.id.with("wiping");
        if response.drag_started() {
            d.insert_temp(id, near_split);
        }
        if !response.dragged() {
            d.remove::<bool>(id);
        }
        d.get_temp::<bool>(id).unwrap_or(false)
    });

    if wiping {
        if let Some(pos) = response.interact_pointer_pos() {
            app.view.wipe = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        }
    } else if response.dragged() {
        app.view.pan -= response.drag_delta() / rect.size() / app.view.zoom;
    }
    if near_split && !response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered() && app.view.zoom > 1.0 {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            app.view.zoom = (app.view.zoom * (1.0 + scroll * 0.002)).clamp(1.0, 8.0);
        }
    }

    let half = egui::vec2(0.5, 0.5) / app.view.zoom;
    let limit = egui::vec2(0.5 - half.x, 0.5 - half.y);
    app.view.pan = egui::vec2(
        app.view.pan.x.clamp(-limit.x, limit.x),
        app.view.pan.y.clamp(-limit.y, limit.y),
    );
    let centre = egui::pos2(0.5, 0.5) + app.view.pan;
    let uv = egui::Rect::from_center_size(centre, half * 2.0);

    // --- paint -------------------------------------------------------------
    let painter = ui.painter_at(rect);
    let t = app.view.wipe.clamp(0.0, 1.0);
    let split_x = split(t);
    let uv_split = uv.left() + t * uv.width();
    let white = egui::Color32::WHITE;

    // Crossfade: the outgoing frame under the incoming one, both at full rect.
    let now = ui.input(|i| i.time);
    let fade = match app.view.fade_started {
        Some(started) => (((now - started) as f32) / crate::theme::MOTION_S).clamp(0.0, 1.0),
        None => 1.0,
    };
    if fade >= 1.0 {
        app.view.fading_out = None;
        app.view.fade_started = None;
    }
    if let Some(old) = &app.view.fading_out {
        painter.image(old.id(), rect, uv, white);
    }
    let alpha = egui::Color32::from_white_alpha((fade * 255.0) as u8);

    if t > 0.0 {
        painter.image(
            source.id(),
            egui::Rect::from_min_max(rect.left_top(), egui::pos2(split_x, rect.bottom())),
            egui::Rect::from_min_max(uv.left_top(), egui::pos2(uv_split, uv.bottom())),
            alpha,
        );
    }
    if t < 1.0 {
        let dst = egui::Rect::from_min_max(egui::pos2(split_x, rect.top()), rect.right_bottom());
        let src = egui::Rect::from_min_max(egui::pos2(uv_split, uv.top()), uv.right_bottom());
        painter.image(output.id(), dst, src, alpha);
        if let Some(overlay) = &app.view.overlay_tex {
            painter.image(overlay.id(), dst, src, alpha);
        }
    }
    // Dim the old frame while a new one decodes, rather than emptying the pane.
    if app.preview_busy {
        painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));
        // An `Area`, not `ui.put`: a widget placed inside an allocated rect drags the layout cursor back there.
        egui::Area::new(ui.id().with("preview_spinner"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.center() - egui::vec2(16.0, 16.0))
            .show(ui.ctx(), |ui| ui.add(egui::Spinner::new()));
    }

    wipe_handle(&painter, rect, split_x, near_split || wiping);
    corner_labels(&painter, rect, split_x);
    zoom_controls(app, ui, rect);
    step_controls(app, ui, rect);
}

/// The split line, and the grip that moves it.
fn wipe_handle(painter: &egui::Painter, rect: egui::Rect, x: f32, hot: bool) {
    let line = if hot {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_white_alpha(160)
    };
    painter.line_segment(
        [
            egui::pos2(x, rect.top()),
            egui::pos2(x, rect.center().y - HANDLE_R),
        ],
        egui::Stroke::new(1.0, line),
    );
    painter.line_segment(
        [
            egui::pos2(x, rect.center().y + HANDLE_R),
            egui::pos2(x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, line),
    );
    let centre = egui::pos2(x, rect.center().y);
    painter.circle(
        centre,
        HANDLE_R,
        egui::Color32::from_black_alpha(150),
        egui::Stroke::new(1.5, line),
    );
    for (dx, arrow) in [(-4.0, "◀"), (4.0, "▶")] {
        painter.text(
            centre + egui::vec2(dx, 0.0),
            egui::Align2::CENTER_CENTER,
            arrow,
            egui::FontId::proportional(8.0),
            line,
        );
    }
}

/// "before" and "after", not "source" and "undistorted": naming the operation would mean a label that changes as you work.
fn corner_labels(painter: &egui::Painter, rect: egui::Rect, split_x: f32) {
    // Each label is clipped to its own side, so the wipe cuts through the word rather than blinking it out.
    let label = |x: f32, align: egui::Align2, text: &str, clip: egui::Rect| {
        let painter = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
        let at = egui::pos2(x, rect.top() + 4.0);
        let font = egui::FontId::monospace(11.0);
        // Outlined: white alone vanishes into an overexposed sky, black alone into a shadow.
        for dx in [-1.0, 0.0, 1.0] {
            for dy in [-1.0, 0.0, 1.0] {
                if dx != 0.0 || dy != 0.0 {
                    painter.text(
                        at + egui::vec2(dx, dy),
                        align,
                        text,
                        font.clone(),
                        egui::Color32::from_black_alpha(190),
                    );
                }
            }
        }
        painter.text(at, align, text, font, egui::Color32::WHITE);
    };
    let left = egui::Rect::from_min_max(rect.left_top(), egui::pos2(split_x, rect.bottom()));
    let right = egui::Rect::from_min_max(egui::pos2(split_x, rect.top()), rect.right_bottom());
    label(rect.left() + 6.0, egui::Align2::LEFT_TOP, "before", left);
    label(rect.right() - 6.0, egui::Align2::RIGHT_TOP, "after", right);
}

/// An `Area` rather than a child `Ui`: anything allocating this far from the layout cursor drags it, and the timeline then draws over the picture.
fn overlay_bar(
    ui: &mut egui::Ui,
    id_source: &str,
    pivot: egui::Align2,
    pos: egui::Pos2,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Area::new(ui.id().with(id_source))
        .order(egui::Order::Middle)
        .pivot(pivot)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                // Dark enough to survive a bright sky behind it.
                .fill(egui::Color32::from_black_alpha(210))
                .corner_radius(3)
                .inner_margin(egui::Margin::symmetric(5, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        add(ui);
                    });
                });
        });
}

/// Zoom, in the bottom-left corner of the image it zooms.
fn zoom_controls(app: &mut App, ui: &mut egui::Ui, rect: egui::Rect) {
    const STEP: f32 = 1.4;
    // The readout stays a button at 100 %, only disabled: a label is a different width and the row would shuffle.
    const READOUT: egui::Vec2 = egui::vec2(52.0, 22.0);

    let pos = rect.left_bottom() + egui::vec2(10.0, -44.0);
    overlay_bar(ui, "zoom_controls", egui::Align2::LEFT_TOP, pos, |ui| {
        let zoomed = app.view.zoom > 1.001;
        if ui
            .add_enabled(zoomed, egui::Button::new("−").min_size(SQUARE))
            .on_hover_text("Zoom out")
            .clicked()
        {
            app.view.zoom = (app.view.zoom / STEP).max(1.0);
        }
        if ui
            .add_enabled(
                app.view.zoom < 7.999,
                egui::Button::new("+").min_size(SQUARE),
            )
            .on_hover_text("Zoom in")
            .clicked()
        {
            app.view.zoom = (app.view.zoom * STEP).min(8.0);
        }
        if ui
            .add_enabled(
                zoomed,
                egui::Button::new(
                    // Explicit colour: the default disabled text sinks into the footage.
                    mono(format!("{:.0}%", app.view.zoom * 100.0))
                        .color(egui::Color32::from_gray(210)),
                )
                .min_size(READOUT),
            )
            .on_hover_text("Back to fit")
            .clicked()
        {
            app.view.zoom = 1.0;
            app.view.pan = egui::Vec2::ZERO;
        }
    });
}

/// In the corner opposite the zoom: a pixel of the scrubber is several frames, so it cannot land on a chosen one.
fn step_controls(app: &mut App, ui: &mut egui::Ui, rect: egui::Rect) {
    let step = app.preview_frame_pos_step();
    let pos = rect.right_bottom() + egui::vec2(-10.0, -44.0);
    overlay_bar(ui, "step_controls", egui::Align2::RIGHT_TOP, pos, |ui| {
        for (label, dir, hover) in [
            ("◀", -1.0, "One frame back"),
            ("▶", 1.0, "One frame forward"),
        ] {
            if ui
                .add_enabled(step.is_some(), egui::Button::new(label).min_size(SQUARE))
                .on_hover_text(hover)
                .clicked()
                && let Some(s) = step
            {
                app.preview_pos = (app.preview_pos + dir * s).clamp(0.0, 1.0);
            }
        }
    });
}

// Scrubber.

/// Everything drawn comes from the probe, so selecting a clip is instant: drawing the clip's own frames cost two dozen keyframe decodes per change.
fn scrubber(app: &mut App, ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SCRUBBER_H),
        egui::Sense::click_and_drag(),
    );
    let response = crate::widgets::clickable(response);
    let painter = ui.painter_at(rect);
    let track = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.right(), rect.bottom() - AXIS_H),
    );
    let duration_s = app.preview_duration_s().unwrap_or(0.0);

    painter.rect_filled(track, 2, egui::Color32::from_rgb(0x0a, 0x0b, 0x0d));
    // The playhead alone marks the position; an "elapsed" fill read as a slab of selection.
    let x = track.left() + (app.preview_pos as f32).clamp(0.0, 1.0) * track.width();

    if duration_s > 0.0 {
        // The step grows once one-second ticks would sit closer than a few pixels and smear into a bar.
        let px_per_s = track.width() as f64 / duration_s;
        if let Some(step) = [1.0, 5.0, 10.0, 30.0, 60.0, 300.0]
            .into_iter()
            .find(|s| px_per_s * s >= 4.0)
        {
            let mut t = step;
            while t < duration_s {
                let x = track.left() + (t / duration_s) as f32 * track.width();
                painter.line_segment(
                    [
                        egui::pos2(x, track.bottom() - 10.0),
                        egui::pos2(x, track.bottom()),
                    ],
                    egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
                );
                t += step;
            }
        }
        for i in 0..=6 {
            let f = i as f32 / 6.0;
            let t = f as f64 * duration_s;
            painter.text(
                egui::pos2(track.left() + f * track.width(), rect.bottom()),
                match i {
                    0 => egui::Align2::LEFT_BOTTOM,
                    6 => egui::Align2::RIGHT_BOTTOM,
                    _ => egui::Align2::CENTER_BOTTOM,
                },
                crate::widgets::mmss(t),
                egui::FontId::monospace(9.0),
                egui::Color32::from_gray(110),
            );
            // A tick on the track itself, so the axis reads as a scale rather than a row of loose numbers.
            painter.line_segment(
                [
                    egui::pos2(track.left() + f * track.width(), track.bottom() - 18.0),
                    egui::pos2(track.left() + f * track.width(), track.bottom()),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
            );
        }
    }

    // The playhead, with a grab tab on top so it reads as draggable.
    painter.line_segment(
        [egui::pos2(x, track.top()), egui::pos2(x, track.bottom())],
        egui::Stroke::new(2.0, crate::theme::AMBER),
    );
    painter.rect_filled(
        egui::Rect::from_center_size(egui::pos2(x, track.top() + 5.0), egui::vec2(11.0, 10.0)),
        2,
        crate::theme::AMBER,
    );

    if (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        app.preview_pos = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64;
    }
}

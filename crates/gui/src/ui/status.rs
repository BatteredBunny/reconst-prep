// The bottom bar, the drag-and-drop overlay and completion toasts, all from `Progress`.

use eframe::egui;

use crate::App;
use crate::icons::{self, icon_button};
use crate::widgets::mono;

/// "Kept", everywhere it is said.
const DONE_COLOUR: egui::Color32 = egui::Color32::from_rgb(94, 196, 112);

/// The hairline grey, never a dimmed amber, so an empty bar cannot be misread as a nearly-full one.
const TRACK: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x2d, 0x32);

/// Only two kinds: a run that *worked* gets no toast, because the summary modal carries the same numbers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// It did not finish, but nothing went wrong — a cancel.
    Note,
    /// It broke.
    Failed,
}

/// A message about something that finished. Failures stay until dismissed.
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    /// Seconds left before it fades, or `None` for "until dismissed".
    pub ttl: Option<f32>,
}

impl Toast {
    pub fn note(text: String) -> Self {
        Self {
            text,
            kind: ToastKind::Note,
            ttl: Some(10.0),
        }
    }
    pub fn error(text: String) -> Self {
        Self {
            text,
            kind: ToastKind::Failed,
            ttl: None,
        }
    }

    fn look(&self, visuals: &egui::Visuals) -> (egui::Color32, &'static str) {
        match self.kind {
            ToastKind::Note => (visuals.warn_fg_color, icons::PROBLEM),
            ToastKind::Failed => (visuals.error_fg_color, icons::FAILED),
        }
    }
}

/// The totals of a run, in one line and one order, wherever they are said.
pub fn totals_line(kept: u64, decoded: u64, written: u64) -> String {
    format!("{kept} kept / {decoded} decoded  ·  {written} written")
}

/// A cancel is not a failure: what was written is a valid partial dataset that resuming continues.
pub fn stopped_toast(err: &crate::RunError, written: u64) -> Toast {
    if err.cancelled {
        Toast::note(format!(
            "Cancelled. {written} frame{} written. RESUME carries on from here.",
            crate::plural(written)
        ))
    } else {
        Toast::error(err.message.clone())
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.add_space(2.0);
    match (&app.progress, app.running) {
        // A finished run keeps its numbers but loses the bar and the ETA.
        (Some(p), false) => {
            let done = format!(
                "{}  ·  {}",
                p.clip_name,
                totals_line(p.total_kept, p.total_decoded, p.total_written)
            );
            ui.horizontal(|ui| match &app.last_result {
                Some(Err(e)) if !e.cancelled => {
                    ui.colored_label(ui.visuals().error_fg_color, icons::FAILED);
                    ui.label(&e.message);
                }
                Some(Err(_)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, icons::PROBLEM);
                    ui.label(mono(format!("cancelled  ·  {done}")));
                }
                _ => {
                    ui.colored_label(DONE_COLOUR, icons::DONE);
                    ui.label(mono(done));
                }
            });
        }
        (Some(p), _) => {
            let target = match p.clip_total_frames {
                Some(t) if t > 0 => (p.clip_decoded as f32 / t as f32).clamp(0.0, 1.0),
                _ => 0.0,
            };
            // Eased, so progress arriving in bursts reads as movement.
            let clip_frac = ctx.animate_value_with_time(
                egui::Id::new("clip_progress"),
                target,
                crate::theme::MOTION_S,
            );
            let fps_target = (p.decoded_this_run as f64 / p.elapsed_s.max(1e-9)) as f32;
            let fps = ctx.animate_value_with_time(egui::Id::new("fps"), fps_target, 0.4);
            let eta = match p.clip_eta_s(fps as f64) {
                Some(left) => format!("{} left in clip", crate::widgets::mmss(left)),
                None => String::new(),
            };
            let frames = match p.clip_total_frames {
                Some(t) => format!("{}/{}", p.clip_decoded, t),
                None => format!("{}", p.clip_decoded),
            };
            ui.horizontal(|ui| {
                if app.running {
                    ui.spinner();
                }
                // This clip first, then the run: the frame count belongs to the clip it counts.
                ui.label(mono(format!(
                    "clip {}/{}  {}  {frames}",
                    p.clip_idx + 1,
                    p.n_clips,
                    p.clip_name
                )));
                ui.label(mono(format!(
                    "{:.1} fps · {} kept / {} decoded · {} written  {eta}",
                    fps, p.total_kept, p.total_decoded, p.total_written
                )));
            });
            ui.add_space(4.0);
            bar(ui, clip_frac);
        }
        (None, true) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("starting…");
            });
        }
        (None, false) => {}
    }
    ui.add_space(2.0);
}

/// Full window width: the one thing here that has to be readable without being looked at.
fn bar(ui: &mut egui::Ui, frac: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, TRACK);
    let mut filled = rect;
    filled.set_right(rect.left() + rect.width() * frac);
    painter.rect_filled(filled, 0.0, crate::theme::AMBER);
}

/// Before a run the bar could only repeat the settings already on screen beside it.
pub fn has_content(app: &App) -> bool {
    app.running || app.progress.is_some()
}

/// egui reports hovered files before the drop.
pub fn drop_overlay(ctx: &egui::Context) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let screen = ctx.viewport_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop_overlay"),
    ));
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
    painter.text(
        screen.center() - egui::vec2(0.0, 30.0),
        egui::Align2::CENTER_CENTER,
        icons::DROP_HERE,
        egui::FontId::proportional(44.0),
        crate::theme::AMBER,
    );
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "Drop clips, a lens profile (.json), or a segmentation model (.onnx)",
        egui::FontId::proportional(18.0),
        egui::Color32::WHITE,
    );
}

/// Completion messages, stacked in the corner.
pub fn toasts(app: &mut App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let dt = ctx.input(|i| i.stable_dt);
    let mut dismissed: Option<usize> = None;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -48.0))
        .show(ctx, |ui| {
            for (i, toast) in app.toasts.iter().enumerate() {
                let (colour, icon) = toast.look(ui.visuals());
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(colour, icon);
                        ui.label(&toast.text);
                        if icon_button(ui, icons::REMOVE, "Dismiss").clicked() {
                            dismissed = Some(i);
                        }
                    });
                });
            }
        });
    if let Some(i) = dismissed {
        app.toasts.remove(i);
    }
    for toast in app.toasts.iter_mut() {
        if let Some(ttl) = &mut toast.ttl {
            *ttl -= dt;
        }
    }
    app.toasts.retain(|t| t.ttl.is_none_or(|ttl| ttl > 0.0));
    if app.toasts.iter().any(|t| t.ttl.is_some()) {
        ctx.request_repaint();
    }
}

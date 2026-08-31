// One collapsible category per concern, in the order a run is assembled.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{primary_button, secondary_button};

pub mod about;
pub mod advanced;
pub mod filters;
pub mod lens;
pub mod masking;
pub mod output;
pub mod preview_pane;
pub mod resume;
pub mod selection;
pub mod status;
pub mod summary;
pub mod videos;

pub fn config_column(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.add_space(4.0);
    videos::show(app, ui);
    lens::show(app, ui, ctx);
    masking::show(app, ui, ctx);
    selection::show(app, ui);
    filters::show(app, ui);
    output::show(app, ui);
    advanced::show(app, ui);

    ui.add_space(10.0);
}

/// Pinned to the bottom of the config column. See `App::ui`.
pub fn run_controls(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.add_space(4.0);
    let blocker = app.not_ready_reason();
    let ready = blocker.is_none() && !app.running;
    // Resuming is not a mode ticked beforehand: it means something only when the folder holds an unfinished dataset.
    let can_resume = ready && app.can_resume_here();

    ui.horizontal(|ui| {
        let (icon, text) = if can_resume {
            (icons::RESUME, "RESUME")
        } else {
            (icons::RUN, "PROCESS")
        };
        let response = ui
            .add_enabled_ui(ready, |ui| primary_button(ui, icon, text))
            .inner;
        let response = if can_resume {
            response.on_hover_text(
                "Carry on where the last run stopped: clips already complete in this folder \
                 are skipped. Refuses to run if the folder was made with different settings, \
                 rather than mixing two selections into one dataset.",
            )
        } else {
            response
        };
        if response.clicked() {
            app.start_run(ctx, can_resume);
        }
        if let Some(reason) = blocker {
            response.on_disabled_hover_text(reason);
        }
        if can_resume
            && secondary_button(ui, icons::RUN, "START OVER")
                .on_hover_text("Ignore what is in the folder and process every clip again.")
                .clicked()
        {
            app.start_run(ctx, false);
        }
        if app.running
            && secondary_button(ui, icons::CANCEL, "CANCEL")
                .on_hover_text("Stop between frames. What is already written stays.")
                .clicked()
        {
            app.cancel.cancel();
        }
    });
    if let Some(reason) = blocker {
        ui.weak(reason);
    }
    ui.add_space(4.0);
}

// Everything shown is crate metadata, so it cannot drift from Cargo.toml.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{modal, mono};

pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let closed = modal(
        ctx,
        "about",
        icons::HELP,
        reconst_prep_core::TOOL_NAME,
        420.0,
        |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.add(
                    egui::Image::new(&crate::theme::icon_texture(ui.ctx()))
                        .fit_to_exact_size(egui::vec2(64.0, 64.0)),
                );
                ui.add_space(8.0);
            });
            ui.label(env!("CARGO_PKG_DESCRIPTION"));

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            let row = |ui: &mut egui::Ui, key: &str, value: String| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(key).weak().size(11.0));
                    ui.label(mono(value).size(11.0));
                });
            };
            row(ui, "version", reconst_prep_core::TOOL_VERSION.to_string());
            row(ui, "licence", env!("CARGO_PKG_LICENSE").to_string());
            row(
                ui,
                "gyroflow-core",
                reconst_prep_core::GYROFLOW_CORE_REV.to_string(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // The icon sits beside the link, not inside it: a glyph in a monospaced string opens a gap.
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.label(icons::REPOSITORY);
                crate::widgets::clickable(ui.hyperlink_to(
                    mono(env!("CARGO_PKG_REPOSITORY")).size(11.0),
                    env!("CARGO_PKG_REPOSITORY"),
                ));
            });
        },
    );
    if closed {
        app.about_open = false;
    }
}

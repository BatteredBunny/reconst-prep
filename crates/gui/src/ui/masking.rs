// Sky and People go to different sidecar sets because they are not valid for the same consumers. Each source owns its own checkbox.

use eframe::egui;

use reconst_prep_core::mask::MaskClass;

use crate::App;
use crate::icons;
use crate::preview::class_color;
use crate::widgets::{category, check, check_enabled, mono, slider, subsection_beside, warn};

/// The coloured dot beside a source, matching its tint in the preview overlay.
fn dot(ui: &mut egui::Ui, class: MaskClass) {
    let c = class_color(class);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(
        rect.center(),
        4.0,
        egui::Color32::from_rgb(c[0], c[1], c[2]),
    );
}

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    category(
        ui,
        "cat_mask",
        icons::MASKING,
        "Masking",
        None,
        true,
        |ui| {
            let (mask_sky, sky) = (&mut app.cfg.mask_sky, &mut app.cfg.sky);
            subsection_beside(
                ui,
                "sky_params",
                "advanced options",
                |ui| {
                    dot(ui, MaskClass::Sky);
                    check(ui, mask_sky, "Sky").on_hover_text(
                        "Clouds move on their own. A reconstruction that tries to match them \
                         puts the camera in the wrong place and leaves floating debris above \
                         the scene.",
                    );
                },
                |ui| sky_params(ui, sky),
            );

            ui.add_space(4.0);
            // Disabled without the model rather than checkable: the run would trip over it later.
            let has_model = app.cfg.seg_model.is_some();
            let downloading = app.models.downloading.clone();
            let model_error = app.models.error.clone();
            // The click is carried out of the closure: fetching needs the whole App, already borrowed here.
            let mut download_clicked = false;
            let (mask_people, seg_width, seg_temporal_window) = (
                &mut app.cfg.mask_people,
                &mut app.cfg.seg_width,
                &mut app.cfg.seg_temporal_window,
            );
            subsection_beside(
                ui,
                "seg_params",
                "advanced options",
                |ui| {
                    dot(ui, MaskClass::People);
                    check_enabled(ui, has_model, mask_people, "People")
                        .on_hover_text("Moving objects like people are best masked out.")
                        .on_disabled_hover_text(
                            "Needs the segmentation model — download it under advanced options.",
                        );
                    if !has_model {
                        ui.colored_label(ui.visuals().warn_fg_color, icons::PROBLEM)
                            .on_hover_text("Segmentation model is missing, please download it.");
                    }
                },
                |ui| {
                    model_row(
                        ui,
                        has_model,
                        &downloading,
                        &model_error,
                        &mut download_clicked,
                    );
                    seg_params(ui, seg_width, seg_temporal_window)
                },
            );
            if download_clicked && let Some(entry) = reconst_prep_core::models::CATALOGUE.first() {
                app.fetch_model(entry, ctx);
            }
        },
    );
}

/// Takes the fields it edits rather than the whole `App`: two closures cannot both hold `&mut App`.
fn sky_params(ui: &mut egui::Ui, sky: &mut reconst_prep_core::mask::SkyParams) {
    use reconst_prep_core::mask::SkyParams;

    slider(
        ui,
        &mut sky.luma_min,
        40..=250,
        "brightness",
        SkyParams::default().luma_min,
        "How bright a pixel must be to count as sky. Lower it for an overcast shot.",
    );
    slider(
        ui,
        &mut sky.blue_bias,
        -60..=80,
        "blueness",
        SkyParams::default().blue_bias,
        "How blue a pixel must be to count as sky. Negative values accept grey cloud. \
         Raise it to catch only clear blue.",
    );
    slider(
        ui,
        &mut sky.gradient_max,
        2..=60,
        "edge stop",
        SkyParams::default().gradient_max,
        "How strong an edge stops the sky spreading. This is what holds the mask at the \
         horizon. Lower it if the mask leaks into the ground.",
    );
    slider(
        ui,
        &mut sky.dilate,
        0..=12,
        "grow (px)",
        SkyParams::default().dilate,
        "Widens the mask. Raise it if a fringe of sky survives along the horizon.",
    );
}

/// Split borrows, same reason as `sky_params`.
fn seg_params(ui: &mut egui::Ui, seg_width: &mut u32, seg_temporal_window: &mut u32) {
    slider(
        ui,
        seg_width,
        256..=1280,
        "detail (px wide)",
        crate::settings::Settings::default().seg_width,
        "How closely the model looks. A distant person is only a few pixels tall, so too \
         low a value misses them. Higher values cost time on every frame.",
    );
    slider(
        ui,
        seg_temporal_window,
        1..=9,
        "steady over N frames",
        crate::settings::Settings::default().seg_temporal_window,
        "Averages the mask over neighbouring frames. Raise it if the mask edge flickers \
         around a moving person.",
    );
}

/// A Download button until the model is there, progress while it comes, nothing once it has arrived.
fn model_row(
    ui: &mut egui::Ui,
    has_model: bool,
    downloading: &Option<(String, f32)>,
    error: &Option<String>,
    download_clicked: &mut bool,
) {
    use reconst_prep_core::models::CATALOGUE;

    if has_model {
        return;
    }
    if let Some((_, progress)) = downloading {
        // Plain text, not a progress bar: the bar was the only amber fill in the window not marking a decision.
        ui.label(mono(format!("downloading…  {:.0}%", progress * 100.0)));
        ui.ctx().request_repaint();
        return;
    }
    if let Some(e) = error {
        crate::widgets::error(ui, e.clone());
    }
    let Some(entry) = CATALOGUE.first() else {
        warn(ui, "no model available to download.");
        return;
    };
    // The same warning symbol beside the disabled People checkbox, repeated on the row it points at.
    ui.horizontal(|ui| {
        ui.colored_label(ui.visuals().warn_fg_color, icons::PROBLEM)
            .on_hover_text("People masking cannot run until this is downloaded.");
        if icons::button(ui, icons::DOWNLOAD, "Download model")
            .on_hover_text(format!(
                "{}, {:.0} MB.\nLicence: {}.",
                entry.name,
                entry.size_mb(),
                entry.license
            ))
            .clicked()
        {
            *download_clicked = true;
        }
    });
}

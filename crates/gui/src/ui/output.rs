// Output: where the dataset goes, how big, and in what format.

use eframe::egui;

use crate::icons;
use crate::settings::Settings;
use crate::widgets::{category, mono, pick, short_name, slider, warn};
use crate::{App, SizeChoice};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Applied before the category is drawn: inside the body closure it is
    // skipped whenever the category is collapsed, which is how a size carried
    // over from a bigger batch survived into a run that would upscale.
    let (max_w, max_h) = app.clips.source_ceiling().unwrap_or((7680, 4320));
    app.cfg.exact_w = app.cfg.exact_w.min(max_w);
    app.cfg.exact_h = app.cfg.exact_h.min(max_h);

    category(
        ui,
        "cat_output",
        icons::OUTPUT,
        "Output",
        None,
        true,
        |ui| {
            ui.horizontal(|ui| {
                if icons::button(ui, icons::BROWSE, "Folder…").clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    app.out_dir = Some(d);
                }
                match &app.out_dir {
                    Some(p) => {
                        ui.label(mono(short_name(p)))
                            .on_hover_text(p.display().to_string());
                    }
                    None => warn(ui, "none chosen"),
                }
            });

            ui.horizontal(|ui| {
                crate::widgets::pick_row(ui);
                ui.label("Size");
                pick(ui, &mut app.cfg.size_choice, SizeChoice::Same, "native");
                pick(ui, &mut app.cfg.size_choice, SizeChoice::Scale, "scale");
                pick(ui, &mut app.cfg.size_choice, SizeChoice::Exact, "exact");
            });
            match app.cfg.size_choice {
                SizeChoice::Same => {}
                SizeChoice::Scale => {
                    ui.add(
                        egui::Slider::new(&mut app.cfg.scale_factor, 0.1..=1.0)
                            .text("factor")
                            .fixed_decimals(2),
                    );
                }
                SizeChoice::Exact => {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut app.cfg.exact_w)
                                .range(320..=max_w)
                                .speed(16),
                        );
                        ui.label("×");
                        ui.add(
                            egui::DragValue::new(&mut app.cfg.exact_h)
                                .range(240..=max_h)
                                .speed(16),
                        );
                        if app.clips.source_ceiling().is_some() {
                            ui.label(
                                egui::RichText::new(format!("max {max_w}×{max_h}"))
                                    .weak()
                                    .size(10.0),
                            )
                            .on_hover_text(
                                "The smallest frame size in the batch. Upscaling adds pixels \
                                 without adding detail, so it is not offered.",
                            );
                        }
                    })
                    .response
                    .on_hover_text(
                        "A different aspect ratio than the input crops the field of view.",
                    );
                }
            }

            ui.horizontal(|ui| {
                crate::widgets::pick_row(ui);
                ui.label("Format");
                pick(ui, &mut app.cfg.format_jpeg, true, "JPEG");
                pick(ui, &mut app.cfg.format_jpeg, false, "PNG");
            });
            if app.cfg.format_jpeg {
                slider(
                    ui,
                    &mut app.cfg.jpeg_quality,
                    50..=100,
                    "quality",
                    Settings::default().jpeg_quality,
                    "JPEG quality. Below about 90 the compression artefacts start showing up as \
                 features a reconstructor will happily match.",
                );
            }
        },
    );
}

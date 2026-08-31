// A filter rejects on a property of the frame alone, so filters compose where selection modes do not.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{category, check, subsection_beside};

/// No enable checkbox on the header: independent filters, each owning its own, as in `masking.rs`.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    category(
        ui,
        "cat_filters",
        icons::FILTERS,
        "Filters",
        None,
        true,
        |ui| {
            let (on, floor) = (&mut app.cfg.blur_floor_on, &mut app.cfg.blur_floor);
            subsection_beside(
                ui,
                "blur_params",
                "advanced options",
                |ui| {
                    check(ui, on, "Filter out blurry frames")
                        .on_hover_text("Drops any frame softer than the threshold.");
                },
                |ui| {
                    ui.add(
                        egui::Slider::new(floor, 1.0..=5000.0)
                            .logarithmic(true)
                            .text("min sharpness"),
                    )
                    .on_hover_text(
                        "Compare it against the sharpness of the previewed frame, shown above \
                         the picture. Same measurement, so a frame reading below this is one \
                         this filter drops.",
                    );
                },
            );
        },
    );
}

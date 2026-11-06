// Real controls that are rarely touched and belong to no one category. Last in the column, and collapsed.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{Category, combo};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    Category::new("cat_advanced", icons::ADVANCED, "Advanced")
        .collapsed()
        .show(ui, |ui| {
            combo(ui, "hardware decode", &mut app.cfg.hwaccel);
        });
}

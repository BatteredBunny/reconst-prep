// Real controls that are rarely touched and belong to no one category. Last in the column, and collapsed.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{category, combo};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let icon = icons::ADVANCED;
    category(ui, "cat_advanced", icon, "Advanced", None, false, |ui| {
        combo(ui, "hardware decode", &mut app.cfg.hwaccel);
    });
}

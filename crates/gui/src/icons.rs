// One name per *meaning*, not per glyph. Pairs that must stay distinguishable sit together: REMOVE vs CLEAR, PROBLEM vs FAILED.

use eframe::egui;
use egui_phosphor::regular as ph;

// --- Config categories, in column order ------------------------------------

pub const VIDEOS: &str = ph::FILM_STRIP;
/// An aperture, matching the app icon.
pub const LENS: &str = ph::APERTURE;
pub const MASKING: &str = ph::EYE_SLASH;
pub const SELECTION: &str = ph::SELECTION;
/// A funnel, deliberately not selection's glyph: the two categories are adjacent in the column.
pub const FILTERS: &str = ph::FUNNEL;
pub const OUTPUT: &str = ph::IMAGES;
pub const ADVANCED: &str = ph::GEAR;

// --- Actions ---------------------------------------------------------------

pub const RUN: &str = ph::PLAY;
pub const CANCEL: &str = ph::PROHIBIT;
pub const ADD_FILES: &str = ph::FILE_PLUS;
pub const ADD_FOLDER: &str = ph::FOLDER_PLUS;
pub const BROWSE: &str = ph::FOLDER_OPEN;
pub const SEARCH: &str = ph::MAGNIFYING_GLASS;
pub const DOWNLOAD: &str = ph::CLOUD_ARROW_DOWN;
pub const RESUME: &str = ph::ARROW_CLOCKWISE;
/// Take one item out of a list.
pub const REMOVE: &str = ph::X;
/// Empty the whole list. Deliberately not the same glyph as [`REMOVE`].
pub const CLEAR: &str = ph::TRASH;

// --- Status ----------------------------------------------------------------

pub const DONE: &str = ph::CHECK_CIRCLE;
/// Something is wrong but the run can still start.
pub const PROBLEM: &str = ph::WARNING;
/// Something already went wrong. Deliberately not [`PROBLEM`]'s glyph.
pub const FAILED: &str = ph::X_CIRCLE;
pub const DROP_HERE: &str = ph::FILE_ARROW_DOWN;

pub const HELP: &str = ph::QUESTION;
pub const REPOSITORY: &str = ph::GITHUB_LOGO;

/// Drawn as a ring rather than wearing a button, for the ones beside a readout or a title.
fn circle_button(ui: &mut egui::Ui, glyph: &str, size: f32, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let colour = visuals.fg_stroke.color;
    ui.painter()
        .circle_stroke(rect.center(), 8.0, egui::Stroke::new(1.0, colour));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(size),
        colour,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    crate::widgets::clickable(response).on_hover_text(name)
}

/// The question mark: the one control about the program rather than the job.
pub fn help_button(ui: &mut egui::Ui) -> egui::Response {
    circle_button(ui, "?", 11.0, "About reconst-prep")
}

/// The cross in the corner of every dialog. See [`crate::widgets::modal`].
pub fn close_button(ui: &mut egui::Ui) -> egui::Response {
    circle_button(ui, REMOVE, 10.0, "Close")
}

/// An icon in front of a label, with the one gap width this UI uses.
pub fn labelled(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// egui takes a widget's accessible name from its text, so an icon-only button would announce a Unicode codepoint.
pub fn icon_button(ui: &mut egui::Ui, icon: &str, name: &str) -> egui::Response {
    let response = ui.small_button(icon);
    let enabled = response.enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    crate::widgets::clickable(response).on_hover_text(name)
}

/// Announces only the words: leaving the glyph in the accessible name opens every button with a stray codepoint.
pub fn button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    let response = ui.button(labelled(icon, text));
    let enabled = response.enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    response
}

// The shared widget vocabulary, and the wrappers that give every clickable thing the right cursor and accessible name.

use eframe::egui;

/// Returns the body's value, or `None` while the category is collapsed.
pub fn category<R>(
    ui: &mut egui::Ui,
    id_source: &str,
    icon: &str,
    title: &str,
    enable: Option<&mut bool>,
    default_open: bool,
    add_body: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(id_source);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );

    let mut title_clicked = false;
    let mut header = state.show_header(ui, |ui| {
        if let Some(flag) = enable {
            let response = ui.checkbox(flag, "");
            // An unlabelled checkbox announces itself as nothing at all, so it borrows the category's name.
            let (on, enabled) = (*flag, response.enabled());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    enabled,
                    on,
                    format!("Enable {title}"),
                )
            });
        }
        // The title toggles the category too: egui's collapsing header reacts only to the 14 px arrow.
        title_clicked = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("{icon}  {}", title.to_uppercase()))
                        .strong()
                        .size(13.0),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked();
    });
    // Ticking a category does *not* open it: opening has its own gesture, the arrow or the title.
    if title_clicked {
        header.toggle();
    }

    let is_open = header.is_open();
    let (toggle, _, body) = header.body(add_body);
    let toggle = clickable(toggle);
    toggle.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::CollapsingHeader,
            true,
            is_open,
            format!("{title} section"),
        )
    });
    ui.add_space(3.0);
    body.map(|r| r.inner)
}

/// Slider that restores its default on double-click.
pub fn slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    text: &str,
    default: T,
    help: &str,
) -> egui::Response {
    let response = ui.add(egui::Slider::new(value, range).text(text));
    if response.double_clicked() {
        *value = default;
    }
    response.on_hover_text(help)
}

/// A short inline warning: yellow, one line, next to what it is about.
pub fn warn(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!("{} {}", crate::icons::PROBLEM, text.into()),
    );
}

/// For something that has already gone wrong: red, and [`crate::icons::FAILED`].
pub fn error(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.colored_label(
        ui.visuals().error_fg_color,
        format!("{} {}", crate::icons::FAILED, text.into()),
    );
}

/// Always present and never opened by switching its feature on, so nothing appears or disappears under the pointer.
pub fn subsection_beside<R>(
    ui: &mut egui::Ui,
    id_source: &str,
    title: &str,
    header: impl FnOnce(&mut egui::Ui),
    add_body: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(id_source);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let row = ui.horizontal(|ui| {
        header(ui);
        ui.add_space(6.0);
        clickable(state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon));
        let label = ui
            .add(
                egui::Label::new(egui::RichText::new(title).weak().size(11.0))
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if label.clicked() {
            state.toggle(ui);
        }
    });
    state
        .show_body_indented(&row.response, ui, add_body)
        .map(|r| r.inner)
}

/// egui honours `Visuals::interact_cursor` for `Button` alone, so everything else has to ask.
pub fn clickable(response: egui::Response) -> egui::Response {
    if response.hovered() && response.enabled() {
        response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// A checkbox with the right cursor.
pub fn check(ui: &mut egui::Ui, on: &mut bool, text: &str) -> egui::Response {
    clickable(ui.checkbox(on, text))
}

/// For a checkbox only available under some condition; the caller says why with `on_disabled_hover_text`.
pub fn check_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    on: &mut bool,
    text: &str,
) -> egui::Response {
    clickable(ui.add_enabled(enabled, egui::Checkbox::new(on, text)))
}

/// Opens a row to pill height before layout, so a caption ahead of [`pick`] pills is centred. (`set_row_height` is a no-op outside wrapping layouts.)
pub fn pick_row(ui: &mut egui::Ui) {
    let h =
        ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y + 2.0;
    ui.allocate_exact_size(egui::vec2(0.0, h), egui::Sense::hover());
    // The invisible spacer still costs one item gap; give it back.
    ui.add_space(-ui.spacing().item_spacing.x);
}

/// Not `selectable_value`: egui's grows by the frame stroke on hover, so the option and its neighbours jump under the pointer.
pub fn pick<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    text: &str,
) -> egui::Response {
    let selected = *current == value;
    let mut response = selectable(ui, selected, text);
    if response.clicked() && !selected {
        *current = value;
        response.mark_changed();
    }
    response
}

/// For callers that decide what a click means: a list row, or an option whose text is a [`egui::text::LayoutJob`].
pub fn selectable(
    ui: &mut egui::Ui,
    selected: bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    let response = ui
        .scope(|ui| {
            let w = &mut ui.style_mut().visuals.widgets;
            w.inactive.bg_fill = egui::Color32::TRANSPARENT;
            w.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            w.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::TRANSPARENT);
            ui.add(egui::Button::selectable(selected, text).frame_when_inactive(true))
        })
        .inner;
    clickable(response)
}

/// Both the list and the names come from the setting's own [`Choice`] impl, so menu and settings file cannot drift.
pub fn combo<T: crate::settings::Choice>(ui: &mut egui::Ui, label: &str, current: &mut T) {
    egui::ComboBox::from_label(label)
        .selected_text(current.label())
        .show_ui(ui, |ui| {
            for option in T::all() {
                let text = option.label();
                pick(ui, current, option, &text);
            }
        });
}

/// One size and weight, so a row of them reads as buttons of equal standing; only the fill tells them apart.
fn run_button(
    ui: &mut egui::Ui,
    icon: &str,
    text: &str,
    fill: Option<egui::Color32>,
) -> egui::Response {
    let enabled = ui.is_enabled();
    // A disabled button drops the fill, and with it the reason to recolour the text.
    let fill = enabled.then_some(fill).flatten();
    let mut label = egui::RichText::new(crate::icons::labelled(icon, text))
        .strong()
        .size(14.0);
    if fill.is_some() {
        // Near-black on amber: the normal light-grey text does not read on it.
        label = label.color(crate::theme::ON_AMBER);
    }
    let mut button = egui::Button::new(label).min_size(egui::vec2(150.0, 32.0));
    if let Some(fill) = fill {
        button = button.fill(fill).stroke(egui::Stroke::NONE);
    }
    let response = ui.add(button);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    response
}

/// The one amber fill in the interface: the action being recommended.
pub fn primary_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    run_button(ui, icon, text, Some(crate::theme::AMBER))
}

/// Same size and weight as [`primary_button`], no accent: amber marks one thing per screen.
pub fn secondary_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    run_button(ui, icon, text, None)
}

/// Dimmed backdrop, title bar with a close button, Escape to leave. Returns true when it should close.
pub fn modal<R>(
    ctx: &egui::Context,
    id_source: &str,
    icon: &str,
    title: &str,
    width: f32,
    add_body: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let id = egui::Id::new(id_source);
    // The backdrop would take the same release as a click on itself; the guard stores the pass it was drawn on, so a dialog closed from outside self-heals.
    let now = ctx.cumulative_pass_nr();
    let first_frame = ctx.data_mut(|d| {
        let last: Option<u64> = d.get_temp(id);
        d.insert_temp(id, now);
        last.is_none_or(|l| now > l + 1)
    });

    let mut closed = false;
    let frame = egui::Frame::new()
        .fill(egui::Color32::from_rgb(0x17, 0x18, 0x1b))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(0x2a, 0x2d, 0x32),
        ))
        .corner_radius(2)
        .inner_margin(egui::Margin::same(14));

    let response = egui::Modal::new(id)
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(170))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{icon}  {}", title.to_uppercase()))
                        .strong()
                        .size(13.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::icons::close_button(ui).clicked() {
                        closed = true;
                    }
                });
            });
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(6.0);
            add_body(ui);
        });
    let close = closed || (!first_frame && response.should_close());
    if close {
        ctx.data_mut(|d| d.remove::<bool>(id));
    }
    close
}

/// Both cases: GTK matches a filter's pattern literally, so a filter of `mp4` hides `DJI_..._D.MP4`.
pub fn extensions(exts: &[&str]) -> Vec<String> {
    exts.iter()
        .flat_map(|e| [e.to_lowercase(), e.to_uppercase()])
        .collect()
}

pub use reconst_prep_core::pipeline::VIDEO_EXT;

/// Monospaced so a readout does not jitter sideways as it updates.
pub fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).monospace()
}

/// Its own name, or the whole path when it has none (a root, or a path ending in `..`).
pub fn short_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Seconds as `m:ss`. Clip lengths and timeline marks, nothing longer.
pub fn mmss(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

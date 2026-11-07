//! Inspector building blocks. Each owns its layout in inspector.css; settings panels compose them.
use super::*;

/// A titled block of related settings, separated from its neighbours by whitespace and a hairline.
#[component]
pub(super) fn Group(
    title: &'static str,
    icon: &'static str,
    /// Right-aligned header content, typically the group's master switch or a status.
    trailing: Option<Element>,
    /// False while the master switch is off: the body and the gap above it go entirely.
    #[props(default = true)]
    open: bool,
    children: Element,
) -> Element {
    rsx! {
        section { class: "group", "data-group": title,
            div { class: "group-head",
                Icon { name: icon }
                h3 { "{title}" }
                if let Some(trailing) = trailing {
                    div { class: "group-trailing", {trailing} }
                }
            }
            if open {
                div { class: "group-body", {children} }
            }
        }
    }
}

/// Label on the left, compact control on the right; `stacked` puts the label above a control
/// too wide to share the line.
#[component]
pub(super) fn SettingRow(
    label: &'static str,
    #[props(default)] stacked: bool,
    children: Element,
) -> Element {
    rsx! {
        div { class: if stacked { "setting-row setting-row--stacked" } else { "setting-row" }, "data-control": label,
            span { class: "row-label", "{label}" }
            div { class: "row-control", {children} }
        }
    }
}

/// Label, value and reset on one line, track underneath.
#[component]
pub(super) fn SliderRow(
    label: &'static str,
    value: f64,
    min: f64,
    max: f64,
    #[props(default = 1.0)] step: f64,
    default: f64,
    #[props(default)] unit: &'static str,
    onchange: EventHandler<f64>,
) -> Element {
    let display = if step >= 1. {
        format!("{value:.0}")
    } else if step >= 0.01 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    };
    rsx! {
        div { class: "slider-row", "data-control": label,
            div { class: "slider-row-head",
                span { class: "row-label", "{label}" }
                output { class: "row-value", "{display}{unit}" }
                // Always present: appearing only off-default made it flicker as a drag crossed the default.
                IconButton {
                    icon: "arrow-counter-clockwise",
                    label: "Reset {label}",
                    size: "small",
                    onclick: move |_| onchange.call(default),
                }
            }
            Slider { label, value, min, max, step, onchange }
        }
    }
}

/// On/off as a pill switch. The real checkbox stays focusable and clickable underneath.
#[component]
pub(super) fn Switch(
    label: &'static str,
    checked: bool,
    #[props(default)] disabled: bool,
    /// Visible text; groups that already name the setting in their header pass false.
    #[props(default = true)]
    show_label: bool,
    onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        label {
            class: if show_label { "switch switch--labelled" } else { "switch" },
            "data-control": label,
            "data-checked": "{checked}",
            "data-disabled": "{disabled}",
            if show_label {
                span { class: "row-label", "{label}" }
            }
            input {
                r#type: "checkbox",
                aria_label: label,
                checked,
                disabled,
                oninput: move |e| onchange.call(e.checked()),
            }
            span { class: "switch-track", aria_hidden: "true", span { class: "switch-knob" } }
        }
    }
}

/// A short exclusive choice shown in full, instead of hiding two or three options in a menu.
#[component]
pub(super) fn ChoiceGroup(
    label: &'static str,
    value: String,
    options: Vec<(String, String)>,
    onchange: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "choice-group", role: "radiogroup", aria_label: label,
            for (id, text) in options {
                Button {
                    size: "small",
                    selected: id == value,
                    role: "radio",
                    aria_checked: "{id == value}",
                    onclick: move |_| onchange.call(id.clone()),
                    "{text}"
                }
            }
        }
    }
}

/// Collapsed secondary options, local to the setting they tune.
#[component]
pub(super) fn Disclosure(
    label: String,
    open: Signal<bool>,
    #[props(default)] aria_label: Option<String>,
    children: Element,
) -> Element {
    let mut open = open;
    rsx! {
        div { class: "disclosure",
            Button {
                size: "small",
                block: true,
                aria_label: aria_label.unwrap_or_else(|| label.clone()),
                aria_expanded: "{open}",
                onclick: move |_| open.toggle(),
                Icon { name: if open() { "caret-down" } else { "caret-right" } }
                span { "{label}" }
            }
            if open() {
                div { class: "disclosure-body", {children} }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Status {
    Ready,
    Missing,
    Error,
}

/// An external file a setting depends on (lens profile, model): what is chosen, or what to do.
#[component]
pub(super) fn ResourcePicker(
    icon: &'static str,
    title: String,
    detail: String,
    status: Status,
    actions: Element,
    #[props(default)] busy: Option<f32>,
    /// Full path or message on hover; the visible text stays short because nothing is ellipsized.
    #[props(default)]
    tooltip: String,
) -> Element {
    let tone = match status {
        Status::Ready => "ready",
        Status::Missing => "missing",
        Status::Error => "error",
    };
    rsx! {
        div { class: "resource", "data-tone": tone, title: "{tooltip}",
            div { class: "resource-main",
                span { class: "resource-icon", Icon { name: icon } }
                div { class: "resource-text",
                    strong { "{title}" }
                    span { "{detail}" }
                }
            }
            if let Some(fraction) = busy {
                div { class: "resource-progress",
                    div { class: "resource-progress-fill", style: "width: {fraction * 100.}%;" }
                }
            }
            div { class: "resource-actions", {actions} }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Note {
    Hint,
    Warning,
    Error,
}

#[component]
pub(super) fn Notice(tone: Note, children: Element) -> Element {
    let (name, icon) = match tone {
        Note::Hint => ("hint", None),
        Note::Warning => ("warning", Some("warning")),
        Note::Error => ("error", Some("warning-circle")),
    };
    rsx! {
        div { class: "notice", "data-tone": name, role: if tone == Note::Hint { "note" } else { "alert" },
            if let Some(icon) = icon { Icon { name: icon } }
            p { {children} }
        }
    }
}

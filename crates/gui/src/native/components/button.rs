use super::*;

/// Visual weight. Ghost is the default everywhere; one primary action per surface.
#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum Tone {
    #[default]
    Ghost,
    Primary,
}

/// Shared action surface. Transparent at rest; every state lives in button.css.
#[component]
pub(super) fn Button(
    children: Element,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] disabled: bool,
    #[props(default)] action: String,
    #[props(default)] title: String,
    aria_label: Option<String>,
    #[props(default)] tone: Tone,
    /// "small" (26px), "normal" (32px) or "large" (36px).
    #[props(default = "normal".to_string())]
    size: String,
    /// Fill the container and align content to the start, for list rows and disclosures.
    #[props(default)]
    block: bool,
    #[props(default)] selected: Option<bool>,
    #[props(extends = button, extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let tone = match tone {
        Tone::Ghost => "",
        Tone::Primary => "ui-button--primary",
    };
    let block = if block { "ui-button--block" } else { "" };
    rsx! {
        button {
            class: "ui-button ui-button--{size} {tone} {block}",
            r#type: "button",
            disabled,
            "data-action": action,
            title,
            aria_label,
            aria_pressed: selected.map(|value| value.to_string()),
            onclick: move |event| onclick.call(event),
            ..attributes,
            {children}
        }
    }
}

#[component]
pub(super) fn IconButton(
    icon: String,
    label: String,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] disabled: bool,
    #[props(default)] selected: Option<bool>,
    #[props(default = "normal".to_string())] size: String,
) -> Element {
    rsx! {
        Button {
            size: "icon-{size}",
            disabled,
            selected,
            title: label.clone(),
            aria_label: label,
            onclick,
            Icon { name: icon }
        }
    }
}

/// A two-state button for a view option: pressed while on. The label stays the same both ways,
/// so the pressed state alone says which way it is.
#[component]
pub(super) fn ToggleButton(
    label: &'static str,
    icon: &'static str,
    /// Icon while on, when it should change with the state.
    #[props(default)]
    icon_on: Option<&'static str>,
    aria_label: &'static str,
    /// Tooltip: what turning it on shows.
    title: &'static str,
    value: Signal<bool>,
) -> Element {
    let mut value = value;
    let on = value();
    rsx! {
        Button {
            size: "small",
            aria_label,
            title,
            selected: on,
            onclick: move |_| value.toggle(),
            Icon { name: if on { icon_on.unwrap_or(icon) } else { icon } }
            "{label}"
        }
    }
}

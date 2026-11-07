use super::change;
use crate::app::{App, Msg, short_name};
use crate::settings::{Choice, ModeChoice, SizeChoice};
use dioxus::prelude::*;

mod icons;
use icons::{AppIcon, Icon};
mod button;
use button::{Button, IconButton, ToggleButton, Tone};
mod measure;
use measure::*;
mod overlays;
use overlays::*;
mod decor;
use decor::*;
mod controls;
use controls::*;
mod rows;
use rows::*;
mod assets;
use assets::*;
mod inspector;
use inspector::*;
mod configuration;
use configuration::*;
mod preview;
use preview::*;
mod status;
use status::*;
mod dialogs;
use dialogs::*;

/// Screenshot captures open the panels each README scene documents; normal runs never match.
fn screenshot_scene(scene: &str) -> bool {
    cfg!(test) && std::env::var("SCREENSHOTS_SCENE").is_ok_and(|s| s == scene)
}

/// Shared by the viewer bar and the stage.
#[derive(Clone, Copy)]
struct ViewOptions {
    /// The untouched frame instead of the result.
    show_before: Signal<bool>,
    mask_visible: Signal<bool>,
    shelf: Signal<bool>,
}

/// One workspace: viewer and contextual inspector side by side, run controls docked underneath.
#[component]
pub fn Workspace() -> Element {
    let mut app = use_context::<Signal<App>>();
    let view = use_context_provider(|| ViewOptions {
        show_before: Signal::new(false),
        mask_visible: Signal::new(true),
        shelf: Signal::new(false),
    });
    let has_inputs = !app.read().clips.inputs().is_empty();
    let shelf_open = has_inputs && !app.read().running && (view.shelf)();
    rsx! {
        div { class: "shell",
            onmounted: move |_event| {
                #[cfg(test)]
                crate::screenshots::mounted(_event.data());
            },
            div { class: "shell-main",
                main { class: "viewer",
                    ViewerBar { shelf_open }
                    Preview {}
                }
                Inspector {}
            }
            RunDock {}
        }
        Dialogs {}
        div { class: "toasts",
            for (index, text) in app.read().toasts.iter().enumerate() {
                div { class: "toast", role: "status",
                    span { "{text}" }
                    IconButton {
                        icon: "x",
                        label: "Dismiss notification",
                        size: "small",
                        onclick: move |_| {
                            app.write().toasts.remove(index);
                        },
                    }
                }
            }
        }
    }
}

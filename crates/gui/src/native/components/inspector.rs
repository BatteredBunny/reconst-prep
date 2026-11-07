use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Tab {
    Adjust,
    Frames,
    Output,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Adjust, Tab::Frames, Tab::Output];
    fn label(self) -> &'static str {
        match self {
            Tab::Adjust => "Adjust",
            Tab::Frames => "Frames",
            Tab::Output => "Output",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Tab::Adjust => "sliders-horizontal",
            Tab::Frames => "frame-corners",
            Tab::Output => "images",
        }
    }
    fn initial() -> Self {
        if screenshot_scene("selection") {
            Tab::Frames
        } else if screenshot_scene("output") {
            Tab::Output
        } else {
            Tab::Adjust
        }
    }
}

/// Contextual settings beside the preview. Tabs swap only this column; the workspace never changes screens.
#[component]
pub(super) fn Inspector() -> Element {
    let app = use_context::<Signal<App>>();
    let mut tab = use_signal(Tab::initial);
    let state = app.read();
    let locked = state.running;
    // A tab whose settings currently block Process carries a dot, so the reason is findable.
    let needs_attention = |t: Tab| match t {
        Tab::Adjust => state.cfg.lens_unready(),
        _ => false,
    };
    rsx! {
        aside { class: "inspector",
            Halftone { width: 320, height: 2400 }
            nav { class: "inspector-tabs", role: "tablist", aria_label: "Settings",
                for t in Tab::ALL {
                    Button {
                        size: "small",
                        role: "tab",
                        selected: tab() == t,
                        aria_selected: "{tab() == t}",
                        onclick: move |_| tab.set(t),
                        Icon { name: t.icon() }
                        "{t.label()}"
                        if needs_attention(t) {
                            span { class: "tab-dot", aria_label: "Needs attention" }
                        }
                    }
                }
            }
            div { class: "inspector-body", role: "tabpanel", aria_label: tab().label(),
                "data-locked": "{locked}",
                match tab() {
                    Tab::Adjust => rsx! { AdjustPanel {} },
                    Tab::Frames => rsx! { FramesPanel {} },
                    Tab::Output => rsx! { OutputPanel {} },
                }
            }
        }
    }
}

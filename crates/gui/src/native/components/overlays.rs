//! Layers above the workspace: dialogs, popovers, and the window-wide layer a drag runs on.
use super::*;

#[component]
pub(super) fn Modal(title: &'static str, onclose: EventHandler<()>, children: Element) -> Element {
    let focus = use_hook(|| {
        std::rc::Rc::new(std::cell::RefCell::new(
            None::<(dioxus_native::NodeHandle, Option<blitz_dom::NodeId>)>,
        ))
    });
    let window = dioxus_native::use_window();
    let cleanup_focus = focus.clone();
    let cleanup_window = window.clone();
    use_drop(move || {
        if let Some((handle, previous)) = cleanup_focus.borrow_mut().take() {
            dioxus::core::spawn_forever(async move {
                tokio::task::yield_now().await;
                if let Some(previous) = previous {
                    let mut doc = handle.doc_mut();
                    if doc.get_element_by_id("active-modal").is_none()
                        && doc
                            .get_node(previous)
                            .is_some_and(|node| node.is_focussable())
                    {
                        doc.set_focus_to(previous);
                        cleanup_window.request_redraw();
                    }
                }
            });
        }
    });
    let mounted_focus = focus.clone();
    let mounted_window = window.clone();
    rsx! {
        div { class: "modal-backdrop",
            div {
                id: "active-modal",
                class: "modal",
                tabindex: "-1",
                role: "dialog",
                aria_modal: "true",
                aria_label: title,
                onmounted: move |event| {
                    let handle = event.data().downcast::<dioxus_native::NodeHandle>().cloned();
                    let saved = mounted_focus.clone();
                    let window = mounted_window.clone();
                    async move {
                        tokio::task::yield_now().await;
                        if let Some(handle) = handle {
                            let mut doc = handle.doc_mut();
                            let previous = doc.get_focussed_node_id();
                            // A search field takes focus straight away. Otherwise the dialog itself
                            // does, so keys reach it without ringing the close button as if chosen.
                            let first = doc
                                .query_selector_all("#active-modal input")
                                .unwrap_or_default()
                                .into_iter()
                                .find(|id| {
                                    doc.get_node(*id).is_some_and(|node| node.is_focussable())
                                })
                                .or_else(|| doc.get_element_by_id("active-modal"));
                            if let Some(first) = first {
                                doc.set_focus_to(first);
                            }
                            drop(doc);
                            *saved.borrow_mut() = Some((handle, previous));
                            window.request_redraw();
                        }
                    }
                },
                onkeydown: move |e| {
                    if e.key() == Key::Named(NamedKey::Escape) {
                        e.prevent_default();
                        onclose.call(());
                    } else if e.key() == Key::Named(NamedKey::Tab) {
                        e.prevent_default();
                        let handle = focus.borrow().as_ref().map(|(handle, _)| handle.clone());
                        let backwards = e.modifiers().shift();
                        let window = window.clone();
                        spawn(async move {
                            tokio::task::yield_now().await;
                            if let Some(handle) = handle {
                                let mut doc = handle.doc_mut();
                                let nodes: Vec<_> = doc
                                    .query_selector_all("#active-modal *")
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter(|id| {
                                        doc.get_node(*id).is_some_and(|node| node.is_focussable())
                                    })
                                    .collect();
                                if !nodes.is_empty() {
                                    let current = doc.get_focussed_node_id();
                                    let index = nodes.iter().position(|id| Some(*id) == current);
                                    let next = match index {
                                        Some(i) if backwards => (i + nodes.len() - 1) % nodes.len(),
                                        Some(i) => (i + 1) % nodes.len(),
                                        None => 0,
                                    };
                                    doc.set_focus_to(nodes[next]);
                                    window.request_redraw();
                                }
                            }
                        });
                    }
                },
                div { class: "modal-heading",
                    h2 { "{title}" }
                    IconButton {
                        icon: "x",
                        label: "Close dialog",
                        onclick: move |_| onclose.call(()),
                    }
                }
                div { class: "modal-body", {children} }
            }
        }
    }
}

/// A floating panel next to its trigger, placed against the window so no scrolling ancestor
/// clips it: below the trigger, or above when the window ends first.
///
/// The panel sits inside a full-window layer that closes it on click. Blitz hit-tests a fixed
/// layer above absolutely positioned content whatever their z-index, so a panel beside the layer
/// rather than inside it could not be clicked. Closing on click rather than press also means the
/// press that dismisses never lands on the trigger as a second click that reopens it.
// TODO: replace with native implementation once fixed by blitz (z-index ordering of fixed and
// absolute boxes in hit testing): the panel could then sit beside its layer, anchored by CSS.
#[component]
pub(super) fn Popover(
    anchor: MountedSlot,
    onclose: EventHandler<()>,
    #[props(default)] class: String,
    /// Match the trigger's width, as a dropdown does.
    #[props(default)]
    match_width: bool,
    children: Element,
) -> Element {
    // Hidden until measured, so it never flashes at the window's corner.
    let mut placement = use_signal(|| "visibility: hidden;".to_string());
    let window = dioxus_native::use_window();
    rsx! {
        div { class: "popover-layer", onclick: move |_| onclose.call(()),
            div {
                class: "popover {class}",
                style: placement(),
                onclick: move |event| event.stop_propagation(),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Named(NamedKey::Escape) {
                        event.stop_propagation();
                        event.prevent_default();
                        onclose.call(());
                    }
                },
                onmounted: move |event| {
                    let panel = event.data();
                    let window = window.clone();
                    async move {
                        let Some(trigger) = anchor.peek().clone() else { return };
                        let (Some(trigger), Some(panel)) = (settled_rect(&trigger).await, settled_rect(&panel).await) else {
                            return;
                        };
                        let window_height = window.surface_size().to_logical::<f64>(window.scale_factor()).height;
                        let field = [trigger.origin.x, trigger.origin.y, trigger.size.width, trigger.size.height];
                        placement.set(popover_placement(field, panel.size.height, window_height, match_width));
                    }
                },
                {children}
            }
        }
    }
}

/// Below the trigger, or above it when the window ends first and there is room above.
fn popover_placement(
    trigger: [f64; 4],
    panel_height: f64,
    window_height: f64,
    match_width: bool,
) -> String {
    const GAP: f64 = 6.;
    let [left, top, width, height] = trigger;
    let below = top + height + GAP;
    let above = top - GAP - panel_height;
    let y = if below + panel_height > window_height && above >= 0. {
        above
    } else {
        below
    };
    let width = if match_width {
        format!(" width: {width}px;")
    } else {
        String::new()
    };
    format!("left: {left}px; top: {y}px;{width}")
}

/// A pointer drag that keeps going when the pointer leaves the element it started on. While it
/// runs, a [`DragLayer`] takes the moves; releasing anywhere, or the window losing focus, ends it.
pub(super) struct Drag<T: 'static> {
    pressed: Signal<bool>,
    active: Signal<Option<T>>,
}
impl<T> Clone for Drag<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Drag<T> {}

impl<T: Copy + 'static> Drag<T> {
    /// Call on pointerdown. False for any button but the primary one, which never drags.
    pub(super) fn press(mut self, event: &PointerEvent) -> bool {
        let primary = !event
            .trigger_button()
            .is_some_and(|button| button != dioxus::html::input_data::MouseButton::Primary);
        self.pressed.set(primary);
        primary
    }
    /// Starts the drag, unless the button came up while the caller was measuring.
    pub(super) fn begin(mut self, state: T) {
        if *self.pressed.peek() {
            self.active.set(Some(state));
        }
    }
    pub(super) fn end(mut self) {
        self.pressed.set(false);
        self.active.set(None);
    }
    pub(super) fn state(self) -> Option<T> {
        *self.active.read()
    }
}

pub(super) fn use_drag<T: Copy + 'static>() -> Drag<T> {
    let drag = Drag {
        pressed: use_signal(|| false),
        active: use_signal(|| None),
    };
    // Releases outside the window never reach the layer; the window still reports them.
    dioxus_native::use_window_event(move |event, _| {
        use dioxus_native::winit::event::{ElementState, WindowEvent};
        let released = matches!(
            event,
            WindowEvent::Focused(false)
                | WindowEvent::PointerButton {
                    state: ElementState::Released,
                    ..
                }
        );
        if released && *drag.pressed.peek() {
            drag.end();
        }
    });
    drag
}

/// The window-wide layer a running [`Drag`] receives its moves on, so the pointer can leave the
/// element it started on.
#[component]
pub(super) fn DragLayer(
    cursor: &'static str,
    onmove: EventHandler<PointerEvent>,
    onend: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "drag-layer",
            style: "cursor: {cursor};",
            onpointermove: move |event| onmove.call(event),
            onpointerup: move |_| onend.call(()),
            onpointercancel: move |_| onend.call(()),
            onpointerleave: move |_| onend.call(()),
        }
    }
}

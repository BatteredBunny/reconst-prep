//! Inputs Blitz has no usable native widget for: range sliders and select menus.
use super::*;

/// Blitz paints no thumb on a native range input, so the slider draws its own track and thumb.
/// With `timeline_duration` it becomes the stage's seek ruler instead.
// TODO: replace with native implementation once fixed by blitz (input type=range).
#[component]
pub(super) fn Slider(
    label: &'static str,
    value: f64,
    min: f64,
    max: f64,
    #[props(default = 1.0)] step: f64,
    onchange: EventHandler<f64>,
    #[props(default)] timeline_duration: Option<f64>,
) -> Element {
    let mut track_width = use_signal(|| 800.0_f64);
    let mut element: MountedSlot = use_signal(|| None);
    // The track's left edge and width, captured when the drag starts.
    let drag = use_drag::<(f64, f64)>();
    let measure_ticks = move || {
        let Some(mounted) = element.peek().clone() else {
            return;
        };
        spawn(async move {
            if let Some(rect) = settled_rect(&mounted).await {
                track_width.set(rect.size.width);
            }
        });
    };
    dioxus_native::use_window_event(move |event, _| {
        if timeline_duration.is_some()
            && matches!(
                event,
                dioxus_native::winit::event::WindowEvent::SurfaceResized(_)
            )
        {
            measure_ticks();
        }
    });
    let position = ((value - min) / (max - min).max(f64::EPSILON)).clamp(0., 1.) * 100.;
    let pick = move |x: f64, left: f64, width: f64| {
        let raw = min + ((x - left) / width.max(1.)).clamp(0., 1.) * (max - min);
        let value = if step > 0. {
            min + ((raw - min) / step).round() * step
        } else {
            raw
        };
        onchange.call(value.clamp(min, max));
    };
    rsx! {
        div {
            class: if timeline_duration.is_some() { "slider timeline-scrubber" } else { "slider" },
            role: "slider",
            tabindex: "0",
            aria_label: label,
            aria_valuemin: "{min}",
            aria_valuemax: "{max}",
            aria_valuenow: "{value}",
            onmounted: move |event| {
                element.set(Some(event.data()));
                if timeline_duration.is_some() {
                    measure_ticks();
                }
            },
            onpointerdown: move |event| {
                event.prevent_default();
                if !drag.press(&event) {
                    return;
                }
                let mounted = element.peek().clone();
                spawn(async move {
                    let Some(mounted) = mounted else { return };
                    let Ok(rect) = mounted.get_client_rect().await else { return };
                    let _ = mounted.set_focus(true).await;
                    pick(event.client_coordinates().x, rect.origin.x, rect.size.width);
                    drag.begin((rect.origin.x, rect.size.width));
                });
            },
            onpointerup: move |_| drag.end(),
            onpointercancel: move |_| drag.end(),
            onkeydown: move |event: KeyboardEvent| {
                let next = match event.key() {
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => Some(value - step),
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => Some(value + step),
                    Key::Named(NamedKey::Home) => Some(min),
                    Key::Named(NamedKey::End) => Some(max),
                    _ => None,
                };
                if let Some(next) = next {
                    event.prevent_default();
                    onchange.call(next.clamp(min, max));
                }
            },
            if let Some(duration) = timeline_duration {
                div { class: "timeline-track",
                    for time in timeline_ticks(duration, track_width()) {
                        span { class: "timeline-tick minor", style: "left: {time / duration * 100.}%;" }
                    }
                    for index in 0..=6 {
                        span { class: "timeline-tick major", style: "left: {f64::from(index) / 6. * 100.}%;" }
                    }
                    div { class: "timeline-playhead", style: "left: {position}%;", div { class: "timeline-grab" } }
                }
                for index in 0..=6 {
                    span { class: match index { 0 => "timeline-time first", 6 => "timeline-time last", _ => "timeline-time" },
                        style: "left: {f64::from(index) / 6. * 100.}%;",
                        {timeline_time(duration * f64::from(index) / 6.)}
                    }
                }
            } else {
                div { class: "slider-track" }
                div { class: "slider-fill", style: "width: {position}%;" }
                div { class: "slider-thumb", style: "left: {position}%;" }
            }
        }
        if drag.state().is_some() {
            DragLayer {
                cursor: "ew-resize",
                onmove: move |event: PointerEvent| {
                    if let Some((left, width)) = drag.state() {
                        pick(event.client_coordinates().x, left, width);
                    }
                },
                onend: move |_| drag.end(),
            }
        }
    }
}

/// A select menu. Blitz paints a native select blank and has no popup for it.
// TODO: replace with native implementation once fixed by blitz (select).
#[component]
pub(super) fn Picker(
    label: &'static str,
    value: String,
    options: Vec<(String, String)>,
    onchange: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut trigger: MountedSlot = use_signal(|| None);
    let disabled = options.is_empty();
    let key_options = options.clone();
    let key_value = value.clone();
    let selected = options
        .iter()
        .find(|(id, _)| id == &value)
        .map(|(_, text)| text.as_str())
        .unwrap_or("No clips loaded");
    rsx! {
        div { class: "picker",
            Button {
                size: "small",
                block: true,
                disabled,
                aria_label: label,
                aria_haspopup: "listbox",
                aria_expanded: "{open}",
                onmounted: move |event: MountedEvent| trigger.set(Some(event.data())),
                onclick: move |_| open.toggle(),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Named(NamedKey::Escape) && open() {
                        event.stop_propagation();
                        event.prevent_default();
                        open.set(false);
                        focus_later(trigger);
                    }
                    let delta = match event.key() {
                        Key::Named(NamedKey::ArrowDown) => Some(1isize),
                        Key::Named(NamedKey::ArrowUp) => Some(-1),
                        _ => None,
                    };
                    if let Some(delta) = delta && !key_options.is_empty() {
                        event.prevent_default();
                        let index = key_options
                            .iter()
                            .position(|(id, _)| id == &key_value)
                            .unwrap_or(0);
                        let next = (index as isize + delta).rem_euclid(key_options.len() as isize)
                            as usize;
                        onchange.call(key_options[next].0.clone());
                    }
                },
                span { class: "picker-label", "{selected}" }
                Icon { name: "caret-down" }
            }
            if open() {
                Popover {
                    class: "picker-options",
                    anchor: trigger,
                    match_width: true,
                    onclose: move |_| {
                        open.set(false);
                        focus_later(trigger);
                    },
                    ScrollFade { class: "picker-list",
                        div { role: "listbox", aria_label: label,
                            for (id, text) in options {
                                Button {
                                    size: "small",
                                    block: true,
                                    role: "option",
                                    aria_selected: id == value,
                                    onclick: move |_| {
                                        onchange.call(id.clone());
                                        open.set(false);
                                        focus_later(trigger);
                                    },
                                    "{text}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Gives focus back to the element after the current change has rendered.
fn focus_later(slot: MountedSlot) {
    let mounted = slot.peek().clone();
    spawn(async move {
        tokio::task::yield_now().await;
        if let Some(mounted) = mounted {
            let _ = mounted.set_focus(true).await;
        }
    });
}

fn timeline_ticks(duration: f64, width: f64) -> Vec<f64> {
    if duration <= 0. {
        return Vec::new();
    }
    let Some(step) = [1., 5., 10., 30., 60., 300.]
        .into_iter()
        .find(|step| width / duration * step >= 4.)
    else {
        return Vec::new();
    };
    (1..(duration / step).ceil() as usize)
        .map(|i| i as f64 * step)
        .collect()
}
fn timeline_time(seconds: f64) -> String {
    let seconds = seconds.max(0.).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    #[test]
    fn ticks_follow_seconds_and_duration_not_arbitrary_percentages() {
        assert_eq!(
            timeline_ticks(10., 800.),
            (1..10).map(f64::from).collect::<Vec<_>>()
        );
        assert_eq!(timeline_ticks(1000., 800.)[0], 5.);
        assert_eq!(timeline_time(115.7), "1:55");
        assert!(timeline_ticks(0., 800.).is_empty());
    }
}

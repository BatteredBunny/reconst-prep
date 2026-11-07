//! Visual pieces with no setting of their own: texture, activity, scroll cues and scrolling text.
use super::*;

/// Comic-style halftone dots behind a chrome panel, clipped to it. Blitz skips gradient backgrounds
/// past 500 tiles and paints no SVG patterns, so every dot is a subpath of one path, built once.
/// `width` and `height` only need to cover the largest the panel gets. The panel's other children
/// must be positioned so they paint above the dots.
// TODO: replace with native implementation once fixed by blitz (gradient tiles past 500, or SVG
// patterns): a radial-gradient background tiled at the pitch.
#[component]
pub(super) fn Halftone(width: u32, height: u32) -> Element {
    const PITCH: u32 = 7;
    let path = use_hook(|| {
        let mut d = String::new();
        // Every other row shifts half a step, so the dots stagger like a print halftone.
        for (row, y) in (PITCH / 2..height).step_by(PITCH as usize).enumerate() {
            let start = if row % 2 == 0 { PITCH / 2 } else { PITCH };
            for x in (start..width).step_by(PITCH as usize) {
                d.push_str(&format!("M{x},{y}m-1,0a1,1 0 1,0 2,0a1,1 0 1,0 -2,0"));
            }
        }
        d
    });
    rsx! {
        div { class: "halftone", aria_hidden: "true",
            svg { width: "{width}", height: "{height}", view_box: "0 0 {width} {height}",
                path { d: "{path}", fill: "#ffffff", fill_opacity: "0.04" }
            }
        }
    }
}

/// Indeterminate activity, spun by CSS so it costs nothing once removed.
#[component]
pub(super) fn Spinner(#[props(default)] large: bool) -> Element {
    rsx! { span { class: if large { "spinner spinner--large" } else { "spinner" }, aria_hidden: "true" } }
}

/// A scrolling list with cues for what is out of view: its edges fade where content is hidden,
/// and a thin thumb shows where in the list the view is. Blitz's own overlay scrollbar only shows
/// while scrolling, so the list would otherwise look complete.
// TODO: replace the thumb with native implementation once fixed by blitz (a persistent scrollbar).
#[component]
pub(super) fn ScrollFade(#[props(default)] class: String, children: Element) -> Element {
    let mut list: MountedSlot = use_signal(|| None);
    let mut view = use_signal(|| None::<ScrollView>);
    let update = move || {
        let Some(mounted) = list.peek().clone() else {
            return;
        };
        spawn(async move {
            let (Ok(offset), Ok(size), Some(rect)) = (
                mounted.get_scroll_offset().await,
                mounted.get_scroll_size().await,
                settled_rect(&mounted).await,
            ) else {
                return;
            };
            let next = ScrollView::new(offset.y, rect.size.height, size.height);
            if *view.peek() != next {
                view.set(next);
            }
        });
    };
    rsx! {
        div { class: "scroll-fade",
            div {
                class: "scroll-fade-list {class}",
                onmounted: move |event| { list.set(Some(event.data())); update(); },
                onscroll: move |_| update(),
                {children}
            }
            if let Some(view) = view() {
                if view.above { div { class: "scroll-fade-edge scroll-fade-edge--top" } }
                if view.below { div { class: "scroll-fade-edge scroll-fade-edge--bottom" } }
                div { class: "scroll-fade-thumb", style: "top: {view.thumb_top}%; height: {view.thumb_height}%;" }
            }
        }
    }
}

/// Where a scrolled list's view sits within its content; `None` when everything fits.
#[derive(Clone, Copy, PartialEq)]
struct ScrollView {
    above: bool,
    below: bool,
    /// Percentages of the list's visible height.
    thumb_top: f64,
    thumb_height: f64,
}
impl ScrollView {
    fn new(offset: f64, visible: f64, content: f64) -> Option<Self> {
        if content <= visible + 1. {
            return None;
        }
        let thumb_height = (visible / content * 100.).max(8.);
        let travel = (offset / (content - visible)).clamp(0., 1.);
        Some(Self {
            above: offset > 1.,
            below: offset + visible < content - 1.,
            thumb_top: travel * (100. - thumb_height),
            thumb_height,
        })
    }
}

/// Text that scrolls like a ticker when it does not fit, and sits still when it does.
///
/// Only the characters that fit, plus one, are laid out, and they slide by less than a character
/// between steps. Moving the whole string instead would leave clipped text sitting over
/// neighbouring controls, and Blitz hit-tests clipped text: it took clicks meant for them.
// TODO: replace with native implementation once fixed by blitz (hit testing that respects
// overflow clipping, and pointer-events on text): a CSS keyframe slide of the whole text.
#[component]
pub(super) fn Marquee(class: &'static str, text: String) -> Element {
    const GAP: &str = "     ";
    const SPEED: f64 = 30.; // px per second
    const REST: std::time::Duration = std::time::Duration::from_millis(1400);
    let mut frame: MountedSlot = use_signal(|| None);
    let mut copy: MountedSlot = use_signal(|| None);
    // Once measured as not fitting: the box's width, the characters shown and one character's advance.
    let mut ticker = use_signal(|| None::<(f64, usize, f64)>);
    let mut travelled = use_signal(|| 0.0_f64);
    let measure = move |chars: usize| {
        let (Some(frame), Some(copy)) = (frame.peek().clone(), copy.peek().clone()) else {
            return;
        };
        spawn(async move {
            let (Some(outer), Some(inner)) =
                (settled_rect(&frame).await, settled_rect(&copy).await)
            else {
                return;
            };
            // A monospace face: every character advances the same.
            let advance = inner.size.width / chars.max(1) as f64;
            if inner.size.width > outer.size.width + 0.5 {
                let width = outer.size.width;
                ticker.set(Some((
                    width,
                    (width / advance).ceil() as usize + 1,
                    advance,
                )));
            }
        });
    };
    // A new text shows whole again until measured: the switcher's next video may fit.
    use_effect(use_reactive!(|text| {
        ticker.set(None);
        travelled.set(0.);
        measure(text.chars().count());
    }));
    let chars = text.chars().count();
    let looped: Vec<char> = text.chars().chain(GAP.chars()).collect();
    let loop_chars = looped.len();
    use_future(move || async move {
        let tick = std::time::Duration::from_millis(33);
        loop {
            tokio::time::sleep(tick).await;
            let Some((_, _, advance)) = *ticker.peek() else {
                continue;
            };
            let length = loop_chars as f64 * advance;
            let next = *travelled.peek() + SPEED * tick.as_secs_f64();
            if next >= length {
                travelled.set(0.);
                tokio::time::sleep(REST).await;
            } else {
                travelled.set(next);
            }
        }
    });
    rsx! {
        span { class: "marquee {class}", title: "{text}",
            // Pinned while ticking: the window of characters is wider than the box and would
            // otherwise size it, so the box would jump between the two widths.
            style: ticker().map(|(width, ..)| format!("width: {width}px; flex-shrink: 0;")).unwrap_or_default(),
            onmounted: move |event| { frame.set(Some(event.data())); measure(chars); },
            match ticker() {
                Some((_, shown, advance)) => {
                    let steps = (travelled() / advance).floor();
                    let start = steps as usize % loop_chars;
                    let window: String = (0..shown).map(|i| looped[(start + i) % loop_chars]).collect();
                    let shift = travelled() - steps * advance;
                    rsx! { span { class: "marquee-text", style: "transform: translateX(-{shift}px);", "{window}" } }
                }
                None => rsx! {
                    span { class: "marquee-text", onmounted: move |event| { copy.set(Some(event.data())); measure(chars); }, "{text}" }
                },
            }
        }
    }
}

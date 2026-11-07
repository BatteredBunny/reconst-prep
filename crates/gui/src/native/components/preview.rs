use super::*;

/// The stage, its transport and the seek ruler. Gesture state is local so dragging never
/// rebuilds the inspector or shelf.
#[component]
pub(super) fn Preview() -> Element {
    let app = use_context::<Signal<App>>();
    let view = use_context::<ViewOptions>();
    let mut gestures = use_signal(Gestures::default);
    let mut area: MountedSlot = use_signal(|| None);
    let area_size = use_signal(|| [800.0_f64, 500.0_f64]);
    let has_clips = !app.read().clips.files().is_empty();
    // The transport row only exists with clips, so it changes how much room the stage has.
    use_effect(use_reactive!(|has_clips| {
        let _ = has_clips;
        measure_preview_area(area, area_size);
    }));
    let mut mounted: MountedSlot = use_signal(|| None);
    let drag = use_drag::<Pan>();
    dioxus_native::use_window_event(move |event, _| {
        if matches!(
            event,
            dioxus_native::winit::event::WindowEvent::SurfaceResized(_)
        ) {
            measure_preview_area(area, area_size);
        }
    });

    let state = app.read();
    let source = state.preview.view.source.clone();
    let output = state.preview.view.output.clone();
    let overlay = state
        .preview
        .view
        .overlay
        .clone()
        .filter(|_| (view.mask_visible)());
    let Gestures { zoom, pan } = *gestures.read();
    let show_before = (view.show_before)();
    let image_space = [area_size()[0].max(80.), area_size()[1].max(80.)];
    let fitted = output
        .as_ref()
        .map(|image| fitted_size(image_space, [image.0.width, image.0.height]))
        .unwrap_or(image_space);
    let view_transform = [zoom, pan[0], pan[1]];
    let duration = state.preview_duration_s().unwrap_or(0.);
    let step = state.preview_frame_pos_step();
    let busy = state.preview.job.busy;
    let error = state.preview.job.error.clone();
    rsx! {
        div { class: "stage-area",
            onmounted: move |event| { area.set(Some(event.data())); measure_preview_area(area, area_size); },
            if let Some(src) = source {
                div {
                    id: "preview-stage",
                    class: "image-stage",
                    "data-showing": if show_before { "before" } else { "after" },
                    style: "width: {fitted[0]}px; height: {fitted[1]}px;",
                    onmounted: move |event| mounted.set(Some(event.data())),
                    onpointerdown: move |event| {
                        event.prevent_default();
                        if !drag.press(&event) {
                            return;
                        }
                        let element = mounted.peek().clone();
                        spawn(async move {
                            let Some(element) = element else { return };
                            let Ok(rect) = element.get_client_rect().await else { return };
                            if rect.size.width <= 0. || rect.size.height <= 0. {
                                return;
                            }
                            let p = event.client_coordinates();
                            drag.begin(Pan {
                                start: [p.x as f32, p.y as f32],
                                size: [rect.size.width as f32, rect.size.height as f32],
                                pan: gestures.peek().pan,
                            });
                        });
                    },
                    onpointerup: move |_| drag.end(),
                    onpointercancel: move |_| drag.end(),
                    onwheel: move |event| {
                        event.prevent_default();
                        let delta = match event.delta() {
                            dioxus::html::geometry::WheelDelta::Pixels(value) => value.y,
                            dioxus::html::geometry::WheelDelta::Lines(value) => value.y * 40.,
                            dioxus::html::geometry::WheelDelta::Pages(value) => value.y * 400.,
                        };
                        if delta != 0. { zoom_preview(gestures, (1. + delta * 0.002) as f32); }
                    },
                    // Until the first result renders, the source frame stands in for it.
                    match output.filter(|_| !show_before) {
                        Some(result) => rsx! {
                            div { class: "image-transform",
                                FrameImage { source: result, alt: "Processed frame", view: view_transform }
                            }
                            if let Some(mask) = overlay {
                                div { class: "image-transform",
                                    FrameImage { source: mask, alt: "Mask overlay", nearest: true, view: view_transform }
                                }
                            }
                        },
                        None => rsx! {
                            div { class: "image-transform",
                                FrameImage { source: src, alt: "Source frame", view: view_transform }
                            }
                        },
                    }
                }
            } else if !has_clips {
                ImportPrompt {}
            } else {
                div { class: "stage-placeholder",
                    if state.cfg.lens_unready() {
                        Icon { name: "aperture" }
                        p { "Choose a lens profile to preview the corrected frame." }
                    }
                }
            }
            if busy && error.is_none() {
                div { class: "stage-loading",
                    div { class: "stage-loading-card", role: "status",
                        Spinner { large: true }
                        span {
                            {match state.preview.job.stage { Some(stage) => format!("Preview {stage}…"), None => "Updating preview…".to_string() }}
                        }
                    }
                }
            }
            if let Some(error) = error {
                div { class: "stage-error", Notice { tone: Note::Error, "{error}" } }
            }
        }
        if drag.state().is_some() {
            DragLayer {
                cursor: "grabbing",
                onmove: move |event: PointerEvent| {
                    if let Some(drag) = drag.state() {
                        let p = event.client_coordinates();
                        let mut a = gestures.write();
                        let limit = (0.5 - 0.5 / a.zoom).max(0.);
                        a.pan = [
                            (drag.pan[0] - (p.x as f32 - drag.start[0]) / drag.size[0].max(1.) / a.zoom).clamp(-limit, limit),
                            (drag.pan[1] - (p.y as f32 - drag.start[1]) / drag.size[1].max(1.) / a.zoom).clamp(-limit, limit),
                        ];
                    }
                },
                onend: move |_| drag.end(),
            }
        }
        if has_clips {
            div { class: "transport",
                div { class: "transport-cluster",
                    IconButton {
                        icon: "caret-left", label: "Previous frame", disabled: step.is_none(),
                        onclick: move |_| change(app, |a| {
                            a.preview.pos = (a.preview.pos - a.preview_frame_pos_step().unwrap_or(0.)).max(0.);
                        }),
                    }
                    IconButton {
                        icon: "caret-right", label: "Next frame", disabled: step.is_none(),
                        onclick: move |_| change(app, |a| {
                            a.preview.pos = (a.preview.pos + a.preview_frame_pos_step().unwrap_or(0.)).min(1.);
                        }),
                    }
                }
                span { class: "timecode",
                    span { class: "timecode-now", {timecode(state.preview.pos * duration)} }
                    span { class: "timecode-total", " / {timecode(duration)}" }
                }
                if let Some(data) = &state.preview.data {
                    span { class: "transport-fact", title: "{data.clip_stem} at {data.start_s:.2}s", "frame {data.frame_index}" }
                    if data.masked_fraction > 0. {
                        span { class: "transport-fact", "{data.masked_fraction * 100.:.0}% masked" }
                    }
                }
                div { class: "bar-spacer" }
                div { class: "transport-cluster",
                    IconButton { icon: "magnifying-glass-minus", label: "Zoom out", disabled: zoom <= 1.001,
                        onclick: move |_| zoom_preview(gestures, 1. / 1.4) }
                    Button { size: "small", aria_label: "Back to fit", title: "Back to fit", disabled: zoom <= 1.001,
                        onclick: move |_| { let mut a = gestures.write(); a.zoom = 1.; a.pan = [0.; 2]; },
                        span { class: "zoom-readout", "{zoom * 100.:.0}%" }
                    }
                    IconButton { icon: "magnifying-glass-plus", label: "Zoom in", disabled: zoom >= 7.999,
                        onclick: move |_| zoom_preview(gestures, 1.4) }
                }
            }
            div { class: "seek",
                Slider {
                    label: "Frame position", value: state.preview.pos, min: 0., max: 1.,
                    step: step.unwrap_or(0.001),
                    timeline_duration: Some(duration),
                    onchange: move |value| change(app, |a| a.preview.pos = value),
                }
            }
        }
    }
}

/// The empty workspace: one clear way in, with drag-and-drop as the shortcut.
#[component]
fn ImportPrompt() -> Element {
    let app = use_context::<Signal<App>>();
    rsx! {
        div { class: "import-prompt",
            div { class: "import-icon", Icon { name: "film-strip" } }
            h2 { "Drop videos to begin" }
            p { "Add clips or a whole folder. The preview stays here while you tune the dataset." }
            div { class: "import-actions",
                Button { action: "add-videos", onclick: move |_| add_files(app),
                    Icon { name: "file-plus" } "Add videos…"
                }
                Button { onclick: move |_| add_folder(app),
                    Icon { name: "folder-plus" } "Add folder…"
                }
            }
        }
    }
}

fn timecode(seconds: f64) -> String {
    let tenths = (seconds.max(0.) * 10.).floor() as u64;
    format!("{}:{:02}.{}", tenths / 600, (tenths / 10) % 60, tenths % 10)
}

fn zoom_preview(mut gestures: Signal<Gestures>, factor: f32) {
    let mut a = gestures.write();
    a.zoom = (a.zoom * factor).clamp(1., 8.);
    let limit = (0.5 - 0.5 / a.zoom).max(0.);
    a.pan = a.pan.map(|v| v.clamp(-limit, limit));
}

#[component]
pub(super) fn FrameImage(
    source: crate::native::images::ImageSource,
    alt: &'static str,
    #[props(default = false)] nearest: bool,
    view: [f32; 3],
) -> Element {
    rsx! {
        crate::native::images::RasterImage {
            class: "preview-image",
            align_left: true,
            view: Some(view),
            source,
            alt: alt.to_string(),
            nearest,
        }
    }
}

#[component]
pub(super) fn Thumbnail(source: crate::native::images::ImageSource) -> Element {
    rsx! {
        crate::native::images::RasterImage {
            class: "thumbnail",
            source,
            alt: "Video thumbnail".to_string(),
            cover: true,
        }
    }
}

/// Where a pan started, against the stage size it was measured at.
#[derive(Clone, Copy)]
struct Pan {
    start: [f32; 2],
    size: [f32; 2],
    pan: [f32; 2],
}

/// Fast gesture state stays local: panning must not rebuild the settings or clip list.
#[derive(Clone, Copy)]
struct Gestures {
    zoom: f32,
    pan: [f32; 2],
}
impl Default for Gestures {
    fn default() -> Self {
        Self {
            zoom: 1.,
            pan: [0.; 2],
        }
    }
}

/// Blitz has no resize observer, and layout can trail the event that changed it by a few frames,
/// so the stage is read again until it has settled.
// TODO: replace with native implementation once fixed by blitz (onresize on the stage area).
fn measure_preview_area(area: MountedSlot, mut size: Signal<[f64; 2]>) {
    if let Some(mounted) = area.peek().clone() {
        spawn(async move {
            // Beyond the next frame, read again as a resize finishes laying out.
            for extra_wait in [0, 100, 280] {
                tokio::time::sleep(std::time::Duration::from_millis(extra_wait)).await;
                if let Some(rect) = settled_rect(&mounted).await {
                    let measured = [rect.size.width, rect.size.height];
                    if *size.peek() != measured {
                        size.set(measured);
                    }
                }
            }
        });
    }
}
fn fitted_size(available: [f64; 2], image: [u32; 2]) -> [f64; 2] {
    if image[0] == 0 || image[1] == 0 {
        return available;
    }
    let aspect = f64::from(image[1]) / f64::from(image[0]);
    let width = available[0].min(available[1] / aspect).max(1.);
    [width, width * aspect]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_viewport_fits_actual_photo_without_internal_letterbox() {
        assert_eq!(fitted_size([900., 600.], [400, 300]), [800., 600.]);
        assert_eq!(fitted_size([900., 600.], [1600, 900]), [900., 506.25]);
    }
    #[test]
    fn timecode_shows_tenths_and_never_rounds_past_the_frame() {
        assert_eq!(timecode(0.), "0:00.0");
        assert_eq!(timecode(28.96), "0:28.9");
        assert_eq!(timecode(115.7), "1:55.7");
        assert_eq!(timecode(-1.), "0:00.0");
    }
}

//! Footage: the viewer bar's current-video switcher and the collapsible thumbnail shelf.
use super::*;

pub(super) fn add_files(app: Signal<App>) {
    if let Some(paths) = rfd::FileDialog::new().pick_files() {
        change(app, |a| {
            for p in paths {
                a.clips.add(p);
            }
        });
    }
}

pub(super) fn add_folder(app: Signal<App>) {
    if let Some(p) = rfd::FileDialog::new().pick_folder() {
        change(app, |a| {
            a.clips.add(p);
        });
    }
}

/// Above the stage: which video drives the preview, before or after, and whether the mask shows.
#[component]
pub(super) fn ViewerBar(shelf_open: bool) -> Element {
    let mut app = use_context::<Signal<App>>();
    let view = use_context::<ViewOptions>();
    let mut shelf = view.shelf;
    let show_before = view.show_before;
    let mask_visible = view.mask_visible;
    let mut switcher = use_signal(|| None::<std::rc::Rc<MountedData>>);
    let state = app.read();
    let files = state.clips.files();
    let current = files.get(state.preview.clip).map(|p| short_name(p));
    let has_overlay = state.preview.view.overlay.is_some();
    let has_image = state.preview.view.source.is_some();
    let count = files.len();
    // A run reads the inputs as they were when it started.
    let running = state.running;
    let index = state.preview.clip;
    rsx! {
        div { class: "viewer-bar",
            Button {
                size: "icon-normal",
                title: "About reconst-prep",
                aria_label: "About reconst-prep",
                onclick: move |_| app.write().about_open = true,
                AppIcon {}
            }
            // Keeps About clear of the video switcher, which is easy to hit by mistake beside it.
            span { class: "bar-divider", aria_hidden: "true" }
            if let Some(name) = current {
                div { class: "switcher",
                    Button {
                        action: "videos",
                        aria_label: "Show videos",
                        disabled: running,
                        aria_haspopup: "dialog",
                        aria_expanded: "{shelf_open}",
                        title: if shelf_open { "Hide videos" } else { "Show videos" },
                        selected: shelf_open,
                        onmounted: move |event: MountedEvent| switcher.set(Some(event.data())),
                        onclick: move |_| shelf.set(!shelf_open),
                        Icon { name: "film-strip" }
                        Marquee { class: "switcher-name", text: name }
                        if count > 1 {
                            span { class: "switcher-count", "{index + 1} of {count}" }
                        }
                        Icon { name: if shelf_open { "caret-up" } else { "caret-down" } }
                    }
                    if shelf_open {
                        Popover { anchor: switcher, onclose: move |_| shelf.set(false), AssetShelf {} }
                    }
                }
            } else {
                span { class: "viewer-empty-title",
                    Icon { name: "warning-circle" }
                    "No videos selected!"
                }
            }
            IconButton { icon: "file-plus", label: "Add videos", disabled: running, onclick: move |_| add_files(app) }
            IconButton { icon: "folder-plus", label: "Add folder", disabled: running, onclick: move |_| add_folder(app) }
            div { class: "bar-spacer" }
            if has_image {
                ToggleButton {
                    label: "Before",
                    icon: "square-half",
                    aria_label: "Show before",
                    title: "Show the untouched frame instead of the result",
                    value: show_before,
                }
            }
            if has_overlay {
                ToggleButton {
                    label: "Mask",
                    icon: "eye-slash",
                    icon_on: "eye",
                    aria_label: "Toggle mask overlay",
                    title: "Tint what the mask removes",
                    value: mask_visible,
                }
            }
        }
    }
}

/// Imported footage. Thumbnails select the preview video; removing and importing live here too.
#[component]
pub(super) fn AssetShelf() -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let sizes = state.clips.mixed_resolutions();
    let folders: Vec<_> = state
        .clips
        .inputs()
        .iter()
        .filter(|p| p.is_dir())
        .cloned()
        .collect();
    let files: Vec<_> = state.clips.files().to_vec();
    let selected = state.preview.clip;
    rsx! {
        div { class: "asset-shelf",
            ScrollFade { class: "shelf-track",
                for (index, path) in files.into_iter().enumerate() {
                    AssetCard { index, path, selected: index == selected }
                }
                for path in folders {
                    {
                        let remove = path.clone();
                        rsx! {
                            div { class: "asset-folder", title: "{path.display()}",
                                Icon { name: "folder-open" }
                                span { "{short_name(&path)}" }
                                IconButton {
                                    icon: "x",
                                    label: "Remove input",
                                    size: "small",
                                    onclick: move |_| change(app, |a| a.clips.remove(&remove)),
                                }
                            }
                        }
                    }
                }
                div { class: "shelf-actions",
                    Button {
                        size: "small",
                        aria_label: "Clear inputs",
                        onclick: move |_| change(app, |a| {
                            a.clips.clear();
                            a.preview.clip = 0;
                        }),
                        Icon { name: "trash" }
                        "Clear all"
                    }
                }
            }
            if sizes.len() > 1 {
                Notice { tone: Note::Warning,
                    {
                        format!(
                            "Videos have different frame sizes ({}). Process them as separate runs.",
                            sizes.iter().map(|s| format!("{}×{}", s.w, s.h)).collect::<Vec<_>>().join(", "),
                        )
                    }
                }
            }
        }
    }
}

#[component]
fn AssetCard(index: usize, path: std::path::PathBuf, selected: bool) -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let name = short_name(&path);
    let direct = state.clips.inputs().contains(&path);
    let (info, detail, failed) = match state.clips.probe(&path) {
        Some(Ok(i)) => (
            format!(
                "{}×{} · {}",
                i.size.w,
                i.size.h,
                duration(i.duration_s.unwrap_or(0.))
            ),
            format!("{} · {:.2} fps", path.display(), i.fps),
            false,
        ),
        Some(Err(e)) => (
            "Unreadable".into(),
            format!("{}: {e}", path.display()),
            true,
        ),
        None => ("Reading…".into(), path.display().to_string(), false),
    };
    let thumb = state.clips.thumb(&path).and_then(|t| t.as_ref()).cloned();
    let remove = path.clone();
    rsx! {
        div { class: "asset-card", "data-selected": "{selected}", "data-failed": "{failed}",
            button {
                class: "asset-select",
                r#type: "button",
                aria_pressed: "{selected}",
                aria_label: "Preview {name}",
                title: "{detail}",
                onclick: move |_| change(app, |a| a.preview.clip = index),
                div { class: "asset-thumb",
                    if let Some(src) = thumb {
                        Thumbnail { source: src }
                    } else {
                        Icon { name: "film-strip" }
                    }
                }
                div { class: "asset-text",
                    Marquee { class: "asset-name", text: name.clone() }
                    span { class: "asset-meta", "{info}" }
                }
            }
            if direct {
                div { class: "asset-remove",
                    IconButton {
                        icon: "x",
                        label: "Remove input",
                        size: "small",
                        onclick: move |_| change(app, |a| a.clips.remove(&remove)),
                    }
                }
            }
        }
    }
}

pub(super) fn duration(seconds: f64) -> String {
    let total = seconds.max(0.).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

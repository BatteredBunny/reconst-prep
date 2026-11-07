//! The three inspector panels. Settings here apply to every video in the run.
use super::*;
use reconst_prep_core::{decode::HwAccel, undistort::Interp};

#[component]
pub(super) fn AdjustPanel() -> Element {
    rsx! {
        LensSettings {}
        MaskingSettings {}
    }
}

#[component]
pub(super) fn FramesPanel() -> Element {
    rsx! {
        SelectionSettings {}
        BlurSettings {}
    }
}

#[component]
pub(super) fn OutputPanel() -> Element {
    rsx! {
        SizeSettings {}
        FormatSettings {}
        PerformanceSettings {}
    }
}

/// Core messages start lowercase so they compose into CLI output; the inspector shows them alone.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn open_profile_browser(app: Signal<App>) {
    change(app, |a| {
        a.browser.open = true;
        a.browser.query.clear();
        a.browser.error = None;
        if a.browser.entries.is_empty() {
            a.browser.loading = true;
            a.spawn_worker("profile-index", || {
                Msg::ProfileIndex(
                    reconst_prep_core::profiles::index(false, false).map_err(|e| format!("{e:#}")),
                )
            });
        }
    });
}

fn browse_profile(app: Signal<App>) {
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("Lens profile", &["json"])
        .pick_file()
    {
        change(app, |a| a.cfg.profile = Some(p));
    }
}

#[component]
fn LensSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let cfg = app.read().cfg.clone();
    rsx! {
        Group {
            title: "Lens correction",
            icon: "aperture",
            open: cfg.undistort,
            trailing: rsx! {
                Switch {
                    label: "Enable lens correction",
                    show_label: false,
                    checked: cfg.undistort,
                    onchange: move |value| change(app, |a| a.cfg.undistort = value),
                }
            },
            LensProfile {}
            SettingRow { label: "Interpolation", stacked: true,
                ChoiceGroup {
                    label: "Interpolation",
                    value: cfg.interp.label(),
                    options: Interp::all()
                        .into_iter()
                        .map(|v| {
                            let text = match v {
                                Interp::Bilinear => "Bilinear",
                                Interp::Bicubic => "Bicubic",
                                Interp::Lanczos4 => "Lanczos",
                            };
                            (v.label(), text.to_string())
                        })
                        .collect(),
                    onchange: move |selected: String| {
                        if let Some(v) = Interp::all().into_iter().find(|v| v.label() == selected) {
                            change(app, |a| a.cfg.interp = v);
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn LensProfile() -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let actions_when_set = rsx! {
        IconButton { icon: "magnifying-glass", label: "Search lens profiles", size: "small", onclick: move |_| open_profile_browser(app) }
        IconButton { icon: "folder-open", label: "Choose profile file", size: "small", onclick: move |_| browse_profile(app) }
        IconButton { icon: "x", label: "Remove lens profile", size: "small", onclick: move |_| change(app, |a| a.cfg.profile = None) }
    };
    let Some(path) = state.cfg.profile.clone() else {
        return rsx! {
            ResourcePicker {
                icon: "warning-circle",
                title: "Lens profile needed",
                detail: "Match your camera and recording mode.",
                status: Status::Missing,
                actions: rsx! {
                    Button { size: "small", aria_label: "Search lens profiles", onclick: move |_| open_profile_browser(app),
                        Icon { name: "magnifying-glass" }
                        "Search database"
                    }
                    Button { size: "small", onclick: move |_| browse_profile(app),
                        Icon { name: "folder-open" }
                        "Browse…"
                    }
                },
            }
        };
    };
    let name = short_name(&path);
    let mismatch = match state.profile_summary() {
        Some(Ok(s)) => state
            .preview_clip_info()
            .and_then(|info| s.mismatch(info.size, info.fps)),
        _ => None,
    };
    rsx! {
        match state.profile_summary() {
            Some(Ok(s)) => rsx! {
                ResourcePicker {
                    icon: "aperture",
                    title: if s.camera.is_empty() { s.name.clone() } else { s.camera.clone() },
                    detail: format!("{}×{} · {:.2} fps", s.calib.w, s.calib.h, s.fps),
                    tooltip: "{path.display()}",
                    status: if mismatch.is_some() { Status::Error } else { Status::Ready },
                    actions: actions_when_set,
                }
            },
            Some(Err(error)) => rsx! {
                ResourcePicker {
                    icon: "warning-circle",
                    title: "{name}",
                    detail: "{error}",
                    tooltip: "{path.display()}",
                    status: Status::Error,
                    actions: actions_when_set,
                }
            },
            None => rsx! {
                ResourcePicker {
                    icon: "aperture",
                    title: "{name}",
                    detail: "Reading profile…",
                    status: Status::Ready,
                    actions: actions_when_set,
                }
            },
        }
        if let Some(problem) = mismatch {
            Notice { tone: Note::Warning, {sentence(&problem)} }
        }
    }
}

#[component]
fn MaskingSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let cfg = app.read().cfg.clone();
    let scene = screenshot_scene("masking");
    let sky_open = use_signal(|| scene);
    let people_open = use_signal(|| scene);
    let mut sky_toggle = sky_open;
    let mut people_toggle = people_open;
    let defaults = reconst_prep_core::mask::SkyParams::default();
    rsx! {
        Group { title: "Masking", icon: "eye-slash",
            div { class: "mask-class",
                Icon { name: "cloud" }
                Switch {
                    label: "Sky",
                    checked: cfg.mask_sky,
                    onchange: move |v| change(app, |a| a.cfg.mask_sky = v),
                }
                IconButton {
                    icon: "sliders-horizontal",
                    label: "Sky advanced options",
                    size: "small",
                    selected: sky_open(),
                    onclick: move |_| sky_toggle.toggle(),
                }
            }
            if sky_open() {
                div { class: "nested",
                    SliderRow { label: "Brightness", value: cfg.sky.luma_min as f64, min: 40., max: 250.,
                        default: defaults.luma_min as f64,
                        onchange: move |v| change(app, |a| a.cfg.sky.luma_min = v as _) }
                    SliderRow { label: "Blueness", value: cfg.sky.blue_bias as f64, min: -60., max: 80.,
                        default: defaults.blue_bias as f64,
                        onchange: move |v| change(app, |a| a.cfg.sky.blue_bias = v as _) }
                    SliderRow { label: "Edge stop", value: cfg.sky.gradient_max as f64, min: 2., max: 60.,
                        default: defaults.gradient_max as f64,
                        onchange: move |v| change(app, |a| a.cfg.sky.gradient_max = v as _) }
                    SliderRow { label: "Grow mask", value: cfg.sky.dilate as f64, min: 0., max: 12., unit: " px",
                        default: defaults.dilate as f64,
                        onchange: move |v| change(app, |a| a.cfg.sky.dilate = v as _) }
                }
            }
            div { class: "mask-class",
                Icon { name: "person" }
                Switch {
                    label: "People",
                    checked: cfg.mask_people,
                    disabled: cfg.seg_model.is_none(),
                    onchange: move |v| change(app, |a| a.cfg.mask_people = v),
                }
                IconButton {
                    icon: "sliders-horizontal",
                    label: "People advanced options",
                    size: "small",
                    selected: people_open(),
                    onclick: move |_| people_toggle.toggle(),
                }
            }
            if cfg.seg_model.is_none() || people_open() {
                div { class: "nested",
                    SegmentationModel {}
                    if people_open() {
                        SliderRow { label: "Segmentation width", value: cfg.seg_width as f64, min: 256., max: 1280., step: 32., unit: " px",
                            default: reconst_prep_core::seg::DEFAULT_SEG_WIDTH as f64,
                            onchange: move |v| change(app, |a| a.cfg.seg_width = v as _) }
                        SliderRow { label: "Temporal window", value: cfg.seg_temporal_window as f64, min: 1., max: 9.,
                            default: reconst_prep_core::seg::DEFAULT_TEMPORAL_WINDOW as f64,
                            onchange: move |v| change(app, |a| a.cfg.seg_temporal_window = v as _) }
                    }
                }
            }
        }
    }
}

#[component]
fn SegmentationModel() -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let downloading = state.models.downloading.as_ref().map(|(_, p)| *p);
    let missing_icon = if downloading.is_some() {
        "cloud-arrow-down"
    } else {
        "warning-circle"
    };
    let choose = move |_: MouseEvent| {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("ONNX model", &["onnx"])
            .pick_file()
        {
            change(app, |a| a.cfg.seg_model = Some(p));
        }
    };
    rsx! {
        if let Some(path) = state.cfg.seg_model.clone() {
            ResourcePicker {
                icon: "person",
                title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                detail: "Segmentation model",
                tooltip: "{path.display()}",
                status: Status::Ready,
                actions: rsx! {
                    IconButton { icon: "folder-open", label: "Choose local model…", size: "small", onclick: choose }
                },
            }
        } else {
            ResourcePicker {
                icon: missing_icon,
                title: "Segmentation model needed",
                detail: match downloading {
                    Some(p) => format!("Downloading · {:.0}%", p * 100.),
                    None => "Needed to mask people.".to_string(),
                },
                status: Status::Missing,
                busy: downloading,
                actions: rsx! {
                    Button {
                        size: "small",
                        disabled: downloading.is_some(),
                        onclick: move |_| change(app, |a| {
                            if let Some(e) = reconst_prep_core::models::CATALOGUE.first() {
                                a.fetch_model(e)
                            }
                        }),
                        Icon { name: "cloud-arrow-down" }
                        "Download"
                    }
                    Button { size: "small", aria_label: "Choose local model…", onclick: choose,
                        Icon { name: "folder-open" }
                        "Choose file…"
                    }
                },
            }
        }
        if let Some(error) = &state.models.error {
            Notice { tone: Note::Error, "{error}" }
        }
    }
}

#[component]
fn SelectionSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let cfg = app.read().cfg.clone();
    rsx! {
        Group { title: "Frame selection", icon: "selection",
            ChoiceGroup {
                label: "Keep frames",
                value: (if cfg.mode == ModeChoice::Motion { "motion" } else { "nth" }).to_string(),
                options: vec![
                    ("motion".into(), "On movement".into()),
                    ("nth".into(), "Every Nth frame".into()),
                ],
                onchange: move |selected: String| change(app, |a| {
                    a.cfg.mode = if selected == "motion" { ModeChoice::Motion } else { ModeChoice::EveryNth };
                }),
            }
            if cfg.mode == ModeChoice::Motion {
                Notice { tone: Note::Hint, "Keeps the sharpest nearby frame once the camera has moved enough." }
                SliderRow { label: "Movement threshold", value: cfg.motion_threshold, min: 0.005, max: 0.3, step: 0.005,
                    default: reconst_prep_core::select::DEFAULT_MOTION_THRESHOLD,
                    onchange: move |v| change(app, |a| a.cfg.motion_threshold = v) }
                SliderRow { label: "Sharpest of N frames", value: cfg.window as f64, min: 1., max: 30.,
                    default: reconst_prep_core::select::DEFAULT_WINDOW as f64,
                    onchange: move |v| change(app, |a| a.cfg.window = v as _) }
            } else {
                Notice { tone: Note::Hint, "Keeps frames at a fixed interval, regardless of movement." }
                SliderRow { label: "Keep every Nth frame", value: cfg.nth as f64, min: 1., max: 120.,
                    default: reconst_prep_core::select::DEFAULT_NTH as f64,
                    onchange: move |v| change(app, |a| a.cfg.nth = v as _) }
            }
        }
    }
}

#[component]
fn BlurSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let cfg = state.cfg.clone();
    let sharpness = state.preview.data.as_ref().and_then(|d| d.sharpness);
    rsx! {
        Group {
            title: "Blur filter",
            icon: "funnel",
            open: cfg.blur_floor_on,
            trailing: rsx! {
                Switch {
                    label: "Filter blurry frames",
                    show_label: false,
                    checked: cfg.blur_floor_on,
                    onchange: move |v| change(app, |a| a.cfg.blur_floor_on = v),
                }
            },
            SliderRow { label: "Minimum sharpness", value: cfg.blur_floor, min: 1., max: 5000.,
                default: crate::settings::DEFAULT_BLUR_FLOOR,
                onchange: move |v| change(app, |a| a.cfg.blur_floor = v) }
            if let Some(value) = sharpness {
                div { class: "readout-row",
                    span { "Previewed frame" }
                    output { "data-tone": if value >= cfg.blur_floor { "ok" } else { "warn" },
                        if value >= cfg.blur_floor { "{value:.0} · kept" } else { "{value:.0} · skipped" }
                    }
                }
            }
        }
    }
}

#[component]
fn SizeSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let state = app.read();
    let cfg = state.cfg.clone();
    let source = state.preview_clip_info().map(|i| i.size);
    let result = source.map(|s| match cfg.size_choice {
        SizeChoice::Same => (s.w, s.h),
        SizeChoice::Scale => (
            (f64::from(s.w) * cfg.scale_factor).round() as u32,
            (f64::from(s.h) * cfg.scale_factor).round() as u32,
        ),
        SizeChoice::Exact => (cfg.exact_w, cfg.exact_h),
    });
    rsx! {
        Group {
            title: "Output",
            icon: "images",
            trailing: rsx! {
                if let Some((w, h)) = result {
                    output { class: "group-value", "{w} × {h}" }
                }
            },
            SettingRow { label: "Image size", stacked: true,
                ChoiceGroup {
                    label: "Image size",
                    value: match cfg.size_choice {
                        SizeChoice::Same => "same",
                        SizeChoice::Scale => "scale",
                        SizeChoice::Exact => "exact",
                    }
                    .to_string(),
                    options: vec![
                        ("same".into(), "Original".into()),
                        ("scale".into(), "Scale".into()),
                        ("exact".into(), "Exact".into()),
                    ],
                    onchange: move |selected: String| change(app, |a| {
                        a.cfg.size_choice = match selected.as_str() {
                            "scale" => SizeChoice::Scale,
                            "exact" => SizeChoice::Exact,
                            _ => SizeChoice::Same,
                        };
                    }),
                }
            }
            if cfg.size_choice == SizeChoice::Scale {
                SliderRow { label: "Scale factor", value: cfg.scale_factor, min: 0.1, max: 1., step: 0.01, unit: "×",
                    default: crate::settings::DEFAULT_SCALE_FACTOR,
                    onchange: move |v| change(app, |a| a.cfg.scale_factor = v) }
            }
            if cfg.size_choice == SizeChoice::Exact {
                div { class: "dimension-row",
                    DimensionInput { label: "Width", value: cfg.exact_w, onchange: move |v| change(app, |a| a.cfg.exact_w = v) }
                    span { class: "dimension-times", "×" }
                    DimensionInput { label: "Height", value: cfg.exact_h, onchange: move |v| change(app, |a| a.cfg.exact_h = v) }
                }
                if let Some(s) = source {
                    Notice { tone: Note::Hint, "Limited to the source size, {s.w} × {s.h}." }
                }
            }
        }
    }
}

#[component]
fn DimensionInput(label: &'static str, value: u32, onchange: EventHandler<u32>) -> Element {
    rsx! {
        label { class: "dimension",
            span { "{label}" }
            input {
                r#type: "number",
                min: "2",
                aria_label: label,
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<u32>() {
                        onchange.call(v.max(2));
                    }
                },
            }
        }
    }
}

#[component]
fn FormatSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let cfg = app.read().cfg.clone();
    rsx! {
        Group { title: "Format", icon: "file-arrow-down",
            SettingRow { label: "File type",
                ChoiceGroup {
                    label: "Image format",
                    value: (if cfg.format_jpeg { "jpeg" } else { "png" }).to_string(),
                    options: vec![("jpeg".into(), "JPEG".into()), ("png".into(), "PNG".into())],
                    onchange: move |selected: String| change(app, |a| a.cfg.format_jpeg = selected == "jpeg"),
                }
            }
            if cfg.format_jpeg {
                SliderRow { label: "JPEG quality", value: cfg.jpeg_quality as f64, min: 50., max: 100.,
                    default: reconst_prep_core::output::DEFAULT_JPEG_QUALITY as f64,
                    onchange: move |v| change(app, |a| a.cfg.jpeg_quality = v as _) }
            }
        }
    }
}

#[component]
fn PerformanceSettings() -> Element {
    let app = use_context::<Signal<App>>();
    let cfg = app.read().cfg.clone();
    let open = use_signal(|| screenshot_scene("output"));
    rsx! {
        div { class: "group group--quiet", "data-group": "Advanced",
            Disclosure { label: "Hardware and decoding", aria_label: "Advanced options", open,
                Switch {
                    label: "Use graphics card",
                    checked: cfg.gpu,
                    onchange: move |v| change(app, |a| a.cfg.gpu = v),
                }
                SettingRow { label: "Video decoding",
                    Picker {
                        label: "Video decoding",
                        value: cfg.hwaccel.label(),
                        options: HwAccel::all().into_iter().map(|v| (v.label(), v.label())).collect(),
                        onchange: move |selected| {
                            if let Some(v) = HwAccel::all().into_iter().find(|v| v.label() == selected) {
                                change(app, |a| a.cfg.hwaccel = v);
                            }
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_messages_read_as_sentences() {
        assert_eq!(
            super::sentence("profile is calibrated"),
            "Profile is calibrated"
        );
        assert_eq!(super::sentence(""), "");
    }
}

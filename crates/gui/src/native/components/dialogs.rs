use super::*;

#[component]
pub(super) fn Dialogs() -> Element {
    let mut app = use_context::<Signal<App>>();
    let commands = use_context::<crate::native::platform::NativeCommands>();
    let a = app.read();
    let resume = a.resume_prompt && a.interrupted.is_some() && !a.running;
    rsx! {
        if a.browser.open && !resume && a.summary.is_none() {
            ProfileBrowser {}
        }
        if resume && a.summary.is_none() {
            if let Some(run) = a.interrupted.as_ref() {
                Modal {
                    title: "Pick up where you left off",
                    onclose: move |_| app.write().resume_prompt = false,
                    p { "An unfinished dataset is waiting in:" }
                    p { class: "path", "{run.out_dir.display()}" }
                    if let Some((written, clips)) = run.progress() {
                        p { "{written} frames written across {clips} clips." }
                    }
                    p { class: "modal-hint",
                        "Restore its settings and inputs, then choose Resume when you are ready. Existing images stay in place."
                    }
                    div { class: "modal-actions",
                        Button {
                            tone: Tone::Primary,
                            onclick: move |_| change(
                                app,
                                |a| {
                                    a.adopt_interrupted();
                                    a.resume_prompt = false;
                                },
                            ),
                            "Restore run"
                        }
                        Button { onclick: move |_| app.write().resume_prompt = false, "Not now" }
                        Button {
                            onclick: move |_| change(
                                app,
                                |a| {
                                    crate::settings::InterruptedRun::clear();
                                    a.interrupted = None;
                                    a.resume_prompt = false;
                                },
                            ),
                            "Forget run"
                        }
                    }
                }
            }
        }
        if a.about_open && !resume && !a.browser.open && a.summary.is_none() {
            Modal {
                title: "About reconst-prep",
                onclose: move |_| app.write().about_open = false,
                p { {env!("CARGO_PKG_DESCRIPTION")} }
                p {
                    {
                        format!(
                            "Version {} · {}",
                            reconst_prep_core::TOOL_VERSION,
                            env!("CARGO_PKG_LICENSE"),
                        )
                    }
                }
                p { class: "path", "gyroflow-core {reconst_prep_core::GYROFLOW_CORE_REV}" }
                a { href: env!("CARGO_PKG_REPOSITORY"), "Project repository ↗" }
            }
        }
        if let Some(manifest) = a.summary.as_ref() {
            Modal {
                title: "Your dataset is ready",
                onclose: move |_| app.write().summary = None,
                div { class: "summary-total",
                    strong { "{manifest.totals.written}" }
                    span { "image{crate::app::plural(manifest.totals.written)} written" }
                }
                p { class: "modal-hint",
                    {
                        let clips = manifest.clips.len();
                        format!(
                            "From {clips} clip{} in {}",
                            crate::app::plural(clips),
                            duration(manifest.totals.wall_seconds),
                        )
                    }
                }
                for (stem, gap) in manifest.clips.iter().filter_map(|clip| coverage_gap(clip).map(|gap| (clip.stem.clone(), gap))) {
                    Notice { tone: Note::Warning,
                        "{stem} has {gap:.0}s with no frame kept. Lower the movement threshold, or check that stretch for blur."
                    }
                }
                if let Some(path) = &a.out_dir {
                    {
                        let path = path.clone();
                        rsx! {
                            div { class: "modal-actions", Button { tone: Tone::Primary, onclick: move |_| commands.open_folder(&path), Icon { name: "folder-open" } "Open output folder" } }
                        }
                    }
                }
            }
        }
    }
}

/// The longest stretch of a clip without a kept frame, in seconds, when it stands out: well past
/// the clip's usual spacing, and long enough to lose part of the scene. A short pause in a drone
/// shot is normal and not worth a warning.
fn coverage_gap(clip: &reconst_prep_core::pipeline::ManifestClip) -> Option<f64> {
    const MIN_SECONDS: f64 = 4.;
    const VS_USUAL: f64 = 4.;
    let mut gaps: Vec<u64> = clip
        .kept_frames
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .collect();
    gaps.sort_unstable();
    let seconds = |frames: u64| frames as f64 / clip.fps.max(1e-9);
    let usual = seconds(*gaps.get(gaps.len() / 2)?);
    let worst = seconds(*gaps.last()?);
    (usual > 0. && worst >= MIN_SECONDS && worst / usual >= VS_USUAL).then_some(worst)
}

#[component]
pub(super) fn ProfileBrowser() -> Element {
    let mut app = use_context::<Signal<App>>();
    let a = app.read();
    let entries: Vec<_> = reconst_prep_core::profiles::search(&a.browser.entries, &a.browser.query)
        .into_iter()
        .take(100)
        .cloned()
        .collect();
    rsx! {
        Modal {
            title: "Find a lens profile",
            onclose: move |_| app.write().browser.open = false,
            input {
                r#type: "search",
                placeholder: "Camera, lens, resolution…",
                aria_label: "Search lens profiles",
                value: "{a.browser.query}",
                oninput: move |e| app.write().browser.query = e.value(),
            }
            if a.browser.loading {
                p { role: "status", "Loading profiles…" }
            }
            if let Some(error) = &a.browser.error {
                Notice { tone: Note::Error, "{error}" }
                Button {
                    onclick: move |_| change(
                        app,
                        |a| {
                            a.browser.loading = true;
                            a.browser.error = None;
                            a.spawn_worker(
                                "profile-index",
                                || Msg::ProfileIndex(
                                    reconst_prep_core::profiles::index(false, true)
                                        .map_err(|e| format!("{e:#}")),
                                ),
                            );
                        },
                    ),
                    "Retry"
                }
            }
            if !a.browser.recent.is_empty() {
                h3 { "Recent profiles" }
                for p in a.browser.recent.clone() {
                    {
                        let name = short_name(&p);
                        rsx! {
                            Button {
                                size: "small",
                                block: true,
                                onclick: move |_| change(
                                    app,
                                    |a| {
                                        a.cfg.profile = Some(p.clone());
                                        a.browser.open = false;
                                    },
                                ),
                                "{name}"
                            }
                        }
                    }
                }
            }
            if !a.browser.recent.is_empty() {
                div { class: "modal-separator", role: "separator" }
                h3 { "All profiles" }
            }
            ScrollFade { class: "profile-results",
                for entry in entries {
                    {
                        let name = entry.path.clone();
                        rsx! {
                            Button {
                                size: "small",
                                block: true,
                                disabled: a.browser.loading,
                                onclick: move |_| {
                                    let entry = entry.clone();
                                    change(
                                        app,
                                        |a| {
                                            a.browser.loading = true;
                                            a.browser.error = None;
                                            a.spawn_worker(
                                                "profile-download",
                                                move || Msg::ProfileFetched(
                                                    reconst_prep_core::profiles::fetch(&entry)
                                                        .map_err(|e| format!("{e:#}")),
                                                ),
                                            );
                                        },
                                    );
                                },
                                "{name}"
                            }
                        }
                    }
                }
            }
        }
    }
}
// Phosphor Regular, MIT. See crates/gui/assets/phosphor/LICENSE.

#[cfg(test)]
mod tests {
    use super::coverage_gap;
    use reconst_prep_core::pipeline::ManifestClip;

    fn clip(kept_frames: Vec<u64>) -> ManifestClip {
        ManifestClip {
            path: String::new(),
            stem: "clip".into(),
            width: 1,
            height: 1,
            fps: 10.,
            frames_probed: None,
            frames_decoded: 0,
            frames_kept: kept_frames.len() as u64,
            out_width: 1,
            out_height: 1,
            pinhole_intrinsics: None,
            kept_frames,
        }
    }

    #[test]
    fn only_long_standout_gaps_warn() {
        // Every 0.3s with one 2.3s pause: a drone hovering, not lost coverage.
        let mut hover: Vec<u64> = (0..300).step_by(3).collect();
        hover.extend((323..600).step_by(3));
        assert_eq!(coverage_gap(&clip(hover)), None);
        // Every second, then nothing from frame 190 to 500: 31s.
        let mut hole: Vec<u64> = (0..200).step_by(10).collect();
        hole.extend((500..700).step_by(10));
        assert_eq!(coverage_gap(&clip(hole)), Some(31.));
        assert_eq!(coverage_gap(&clip(vec![5])), None);
    }
}

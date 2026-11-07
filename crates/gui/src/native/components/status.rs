use super::*;

/// Full-width foot of the workspace: where frames go, whether the run can start, and the run itself.
/// Progress, completion, cancellation and errors all resolve in this one place.
#[component]
pub(super) fn RunDock() -> Element {
    let app = use_context::<Signal<App>>();
    let commands = use_context::<crate::native::platform::NativeCommands>();
    let a = app.read();
    let reason = a.run_config().err();
    let ready = reason.is_none() && !a.running;
    let resume = ready && a.can_resume_here();
    let out = a.out_dir.clone();
    let progress = a.progress.as_ref().filter(|_| a.running);
    // A run stops between frames, so a cancel can take a moment to land.
    let stopping = a.running && a.cancel.is_cancelled();
    rsx! {
        footer { class: "run-dock", "data-state": if a.running { "running" } else { "idle" },
            Halftone { width: 4000, height: 60 }
            if a.running {
                ProgressTrack {
                    fraction: progress.map_or(0., |p| clip_fraction(p.clip_decoded, p.clip_total_frames)),
                    frames: progress.map_or_else(String::new, |p| frame_count(p.clip_decoded, p.clip_total_frames)),
                    elapsed: progress.map_or_else(String::new, |p| mmss(p.elapsed_s)),
                }
            }
            div { class: "dock-destination",
                Button {
                    block: true,
                    action: "choose-output",
                    aria_label: "Choose output folder",
                    title: out.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Choose where the frames are written".into()),
                    disabled: a.running,
                    onclick: move |_| {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            change(app, |a| a.out_dir = Some(path));
                        }
                    },
                    Icon { name: "folder-simple" }
                    if let Some(path) = &out {
                        span { class: "dest-label", "Save to" }
                        span { class: "dest-path", "{path.display()}" }
                    } else {
                        span { class: "dest-missing", "Choose output folder…" }
                    }
                }
            }
            div { class: "dock-status", role: "status", aria_live: "polite",
                if stopping {
                    Spinner {}
                    span { class: "dock-primary", "Stopping after the current frame…" }
                } else if a.running {
                    Spinner {}
                    if let Some(p) = progress {
                        span { class: "dock-primary", "Clip {p.clip_idx + 1} of {p.n_clips} · {p.clip_name}" }
                        span { class: "dock-facts",
                            "{frame_count(p.clip_decoded, p.clip_total_frames)} frames · {p.decode_fps():.1} fps · {p.total_written} written"
                            if let Some(eta) = p.clip_eta_s(p.decode_fps()) { " · {mmss(eta)} left" }
                        }
                    } else {
                        span { class: "dock-primary", "Starting…" }
                    }
                } else {
                    match &a.last_result {
                        Some(Err(error)) if !error.cancelled => rsx! {
                            span { class: "dock-icon", "data-tone": "error", Icon { name: "x-circle" } }
                            span { class: "dock-primary", title: "{error.message}", "Stopped: {error.message}" }
                        },
                        Some(Err(_)) => rsx! {
                            span { class: "dock-icon", "data-tone": "warning", Icon { name: "warning" } }
                            span { class: "dock-primary",
                                if let Some(p) = &a.progress { "Cancelled · {p.total_written} written so far" } else { "Cancelled" }
                            }
                            if resume { span { class: "dock-facts", "Resume continues where it stopped." } }
                        },
                        Some(Ok(())) => rsx! {
                            span { class: "dock-icon", "data-tone": "success", Icon { name: "check-circle" } }
                            span { class: "dock-primary",
                                if let Some(p) = &a.progress { "Done · {p.total_written} image{crate::app::plural(p.total_written)} written" } else { "Done" }
                            }
                            if let Some(path) = out.clone() {
                                Button { size: "small", onclick: move |_| commands.open_folder(&path), "Open folder" }
                            }
                        },
                        // With nothing imported the viewer bar already says so.
                        None if a.clips.inputs().is_empty() => rsx! {},
                        None => rsx! {
                            if let Some(reason) = reason {
                                span { class: "dock-icon", "data-tone": "warning", Icon { name: "warning-circle" } }
                                span { class: "dock-primary", "{reason}" }
                            } else {
                                span { class: "dock-icon", "data-tone": "success", Icon { name: "check-circle" } }
                                span { class: "dock-facts", {job_without_destination(&a.job_summary())} }
                            }
                        },
                    }
                }
            }
            div { class: "dock-actions",
                if a.running {
                    Button {
                        size: "large",
                        action: "cancel",
                        aria_label: "Cancel processing",
                        title: "Stop between frames. What is already written stays.",
                        disabled: stopping,
                        // Through `change` so the dock re-renders as stopping straight away.
                        onclick: move |_| change(app, |a| { a.cancel.cancel(); }),
                        if stopping { Spinner {} "Stopping…" } else { Icon { name: "prohibit" } "Cancel" }
                    }
                } else {
                    if resume {
                        Button {
                            size: "large",
                            action: "start-over",
                            onclick: move |_| change(app, |a| a.start_run(false)),
                            "Start over"
                        }
                    }
                    Button {
                        size: "large",
                        tone: Tone::Primary,
                        action: "process",
                        disabled: !ready,
                        title: reason.unwrap_or_default(),
                        onclick: move |_| change(app, |a| a.start_run(resume)),
                        if resume { Icon { name: "arrow-clockwise" } "Resume" } else { Icon { name: "play" } "Process" }
                    }
                }
            }
        }
    }
}

/// One wavelength of the progress wave, in px. status.css shifts the wave by exactly this much
/// per loop, so the loop is seamless.
const WAVELENGTH: f64 = 20.;
/// Wider than any window, so the wave never runs out as the fill grows.
const WAVE_WIDTH: f64 = 4000.;

/// A sine of quadratic segments: each half wavelength is one hump, alternating up and down.
fn wave_path() -> String {
    const MID: f64 = 5.;
    const CONTROL: f64 = 5.; // A quadratic's peak sits halfway to its control point: 2.5px amplitude.
    let half = WAVELENGTH / 2.;
    let humps = ((WAVE_WIDTH + WAVELENGTH) / half) as usize;
    let mut d = format!("M0,{MID}");
    for i in 0..humps {
        let dy = if i % 2 == 0 { -CONTROL } else { CONTROL };
        d.push_str(&format!(" q{},{dy} {half},0", half / 2.));
    }
    d
}

#[component]
fn ProgressTrack(fraction: f64, frames: String, elapsed: String) -> Element {
    let path = use_hook(wave_path);
    let percent = fraction * 100.;
    rsx! {
        div {
            class: "run-progress-track",
            role: "progressbar",
            aria_label: "Current clip progress",
            aria_valuemin: "0",
            aria_valuemax: "100",
            aria_valuenow: "{fraction * 100.}",
            aria_valuetext: "{frames} frames decoded",
            title: "{fraction * 100.:.1}% · {frames} frames · {elapsed} elapsed",
            div { class: "run-progress-rest", style: "left: {percent}%;" }
            div { class: "run-progress-head", style: "left: {percent}%;" }
            div { class: "run-progress-fill", style: "width: {percent}%;",
                svg {
                    class: "run-progress-wave",
                    width: "{WAVE_WIDTH + WAVELENGTH}",
                    height: "10",
                    view_box: "0 0 {WAVE_WIDTH + WAVELENGTH} 10",
                    "aria-hidden": "true",
                    path { d: "{path}", fill: "none", stroke: "currentColor", stroke_width: "2.5", stroke_linecap: "round" }
                }
            }
        }
    }
}

/// The destination already has its own slot in the dock.
fn job_without_destination(summary: &str) -> String {
    summary.split("  →  ").next().unwrap_or(summary).to_string()
}

fn clip_fraction(decoded: u64, total: Option<u64>) -> f64 {
    total
        .filter(|total| *total > 0)
        .map_or(0., |total| (decoded as f64 / total as f64).clamp(0., 1.))
}
fn frame_count(decoded: u64, total: Option<u64>) -> String {
    total
        .filter(|total| *total > 0)
        .map_or_else(|| decoded.to_string(), |total| format!("{decoded}/{total}"))
}
fn mmss(seconds: f64) -> String {
    let seconds = seconds.max(0.).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_is_per_clip_bounded_and_handles_unknown_lengths() {
        assert_eq!(clip_fraction(25, Some(100)), 0.25);
        assert_eq!(clip_fraction(150, Some(100)), 1.);
        assert_eq!(clip_fraction(25, None), 0.);
        assert_eq!(clip_fraction(25, Some(0)), 0.);
        assert_eq!(frame_count(25, None), "25");
        assert_eq!(frame_count(25, Some(0)), "25");
        assert_eq!(frame_count(25, Some(100)), "25/100");
        assert_eq!(mmss(125.), "2:05");
        assert_eq!(
            job_without_destination("1 clip  ·  native JPEG q95  →  out"),
            "1 clip  ·  native JPEG q95"
        );
    }
}

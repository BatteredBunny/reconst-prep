//! Controlled native-window captures. Each scene runs in a separate process because winit
//! permits one event loop per process. `nix build .#screenshots` supplies Xvfb and isolated state.
use crate::{
    Start,
    app::App,
    native::{self, platform::NativeCommands},
    settings::{Settings, SizeChoice},
};
use dioxus::prelude::*;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

thread_local! {
    static DOCUMENT: std::cell::RefCell<Option<dioxus_native::NodeHandle>> = const { std::cell::RefCell::new(None) };
}

pub fn mounted(data: std::rc::Rc<MountedData>) {
    DOCUMENT.with(|document| {
        *document.borrow_mut() = data.downcast::<dioxus_native::NodeHandle>().cloned()
    });
}

/// Query actual laid-out native controls. Missing/hidden targets fail the capture rather than
/// silently leaving an arrow pointing at a stale hard-coded coordinate.
fn target_bounds() -> anyhow::Result<std::collections::BTreeMap<String, [f32; 4]>> {
    DOCUMENT.with(|document| {
        let document = document.borrow();
        let handle = document
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("screenshot document not mounted"))?;
        let doc = handle.doc();
        let mut targets = std::collections::BTreeMap::new();
        for (name, selector) in [
            ("videos", "[data-action='videos']"),
            ("add-videos", "[aria-label='Add videos']"),
            ("compare", "[aria-label='Show before']"),
            ("mask-toggle", "[aria-label='Toggle mask overlay']"),
            ("preview", "#preview-stage"),
            ("stage-area", ".stage-area"),
            ("zoom-in", "[aria-label='Zoom in']"),
            ("zoom-out", "[aria-label='Zoom out']"),
            ("zoom-reset", "[aria-label='Back to fit']"),
            ("playhead", ".timeline-playhead"),
            ("minor-tick", ".timeline-tick.minor"),
            ("previous", "[aria-label='Previous frame']"),
            ("timeline", "[aria-label='Frame position']"),
            ("inspector", ".inspector"),
            ("tabs", ".inspector-tabs"),
            ("destination", "[data-action='choose-output']"),
            ("process", "[data-action='process']"),
            ("cancel", "[data-action='cancel']"),
            ("dock", ".run-dock"),
            ("status", ".dock-status"),
            ("progress-track", ".run-progress-track"),
            ("progress-fill", ".run-progress-fill"),
            ("spinner", ".spinner"),
            ("lens", "[data-group='Lens correction']"),
            ("search", "[aria-label='Search lens profiles']"),
            ("interpolation", "[aria-label='Interpolation']"),
            ("interpolation-row", "[data-control='Interpolation']"),
            ("profile", "[data-group='Lens correction'] .resource"),
            ("sky-row", "[data-control='Sky']"),
            ("model", "[data-group='Masking'] .resource"),
            ("masking", "[data-group='Masking']"),
            ("sky", "[aria-label='Sky advanced options']"),
            ("people", "[aria-label='People advanced options']"),
            ("selection", "[data-group='Frame selection']"),
            ("movement", "[data-control='Movement threshold']"),
            ("window", "[data-control='Sharpest of N frames']"),
            ("blur-group", "[data-group='Blur filter']"),
            ("blur", "[data-control='Filter blurry frames']"),
            ("output", "[data-group='Output']"),
            ("size", "[aria-label='Image size']"),
            ("format", "[data-group='Format']"),
            ("advanced", "[data-group='Advanced']"),
        ] {
            if let Ok(ids) = doc.query_selector_all(selector)
                && let Some(node) = ids.first().and_then(|id| doc.get_node(*id))
            {
                let p = node.absolute_position(0., 0.);
                let size = node.final_layout().size;
                if size.width > 0. && size.height > 0. {
                    targets.insert(name.to_owned(), [p.x, p.y, size.width, size.height]);
                }
            }
        }
        Ok(targets)
    })
}

static CAPTURE: Mutex<Option<Arc<Capture>>> = Mutex::new(None);
struct Capture {
    scene: String,
    proof: bool,
    output: PathBuf,
    result: Mutex<Option<Result<(), String>>>,
}

pub fn observe(app: Signal<App>) {
    let capture = use_hook(|| CAPTURE.lock().unwrap().clone());
    let mut started = use_signal(|| false);
    let mut phase = use_signal(|| 0_u8);
    let commands = use_context::<NativeCommands>();
    let (tx, rx) = use_hook(|| async_channel::bounded::<Result<(), String>>(1));
    let completion = capture.clone();
    use_future(move || {
        let rx = rx.clone();
        let completion = completion.clone();
        let commands = commands.clone();
        async move {
            if let Some(capture) = completion {
                while let Ok(result) = rx.recv().await {
                    if capture.proof && result.is_ok() && phase() < 2 {
                        phase.set(phase() + 1);
                        if phase() == 2 {
                            app.read().cancel.cancel();
                        }
                        started.set(false);
                    } else {
                        *capture.result.lock().unwrap() = Some(result);
                        commands.close();
                        break;
                    }
                }
            }
        }
    });
    let watchdog_capture = capture.clone();
    let watchdog_tx = tx.clone();
    use_hook(move || {
        if watchdog_capture.is_some() {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(300));
                let _ =
                    watchdog_tx.try_send(Err("preview/capture timed out after 300 seconds".into()));
            });
        }
    });
    use_effect(move || {
        let Some(capture) = capture.clone() else {
            return;
        };
        if started() {
            return;
        }
        let state = app.read();
        if let Some(error) = &state.preview.job.error {
            let _ = tx.try_send(Err(error.clone()));
            return;
        }
        if !state.preview.job.complete {
            return;
        }
        if capture.scene == "processing" && !state.running && state.last_result.is_none() {
            drop(state);
            native::change(app, |app| app.start_run(false));
            return;
        }
        if capture.scene == "processing" {
            let stage = phase();
            let threshold = if capture.proof {
                if stage == 0 { 0.2 } else { 0.55 }
            } else {
                0.5
            };
            if capture.proof && stage == 2 {
                if state.running {
                    return;
                }
                if !matches!(&state.last_result, Some(Err(error)) if error.cancelled) {
                    let _ = tx.try_send(Err(
                        "progress proof did not end with typed cancellation".into()
                    ));
                    return;
                }
            } else if !state.progress.as_ref().is_some_and(|p| {
                p.total_written >= 1
                    && p.clip_decoded >= 30
                    && p.clip_total_frames.is_some_and(|total| {
                        total > 0 && p.clip_decoded as f64 / total as f64 >= threshold
                    })
            }) {
                return;
            }
        }
        drop(state);
        started.set(true);
        let tx = tx.clone();
        let stage = phase();
        spawn(async move {
            // Read layout only after the state has been presented, on the UI thread.
            tokio::time::sleep(Duration::from_millis(800)).await;
            match target_bounds() {
                Ok(mut bounds) => {
                    if let Some(p) = &app.read().progress {
                        bounds.insert(
                            "progress-data".into(),
                            [
                                p.clip_decoded as f32,
                                p.clip_total_frames.unwrap_or(0) as f32,
                                p.total_written as f32,
                                p.elapsed_s as f32,
                            ],
                        );
                    }
                    std::thread::spawn(move || {
                        let result = capture_window(&capture, &bounds, stage);
                        let _ = tx.try_send(result.map_err(|e| e.to_string()));
                    });
                }
                Err(error) => {
                    let _ = tx.try_send(Err(error.to_string()));
                }
            }
        });
    });
}

fn capture_window(
    capture: &Capture,
    bounds: &std::collections::BTreeMap<String, [f32; 4]>,
    stage: u8,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(capture.output.join("clean"))?;
    let name = if capture.proof {
        ["progress-early", "progress-later", "cancelled"][stage as usize]
    } else {
        &capture.scene
    };
    let output = capture.output.join("clean").join(format!("{name}.png"));
    let status = std::process::Command::new("import")
        .args(["-window", "root"])
        .arg(&output)
        .status()?;
    anyhow::ensure!(status.success(), "ImageMagick capture failed: {status}");
    let metadata = capture.output.join("clean").join(format!("{name}.json"));
    std::fs::write(&metadata, serde_json::to_vec_pretty(bounds)?)?;
    if capture.proof {
        return Ok(());
    }
    let status = std::process::Command::new("python3")
        .args([
            "-c",
            include_str!("screenshots/annotate.py"),
            &capture.scene,
        ])
        .arg(&output)
        .arg(capture.output.join(format!("{}.png", capture.scene)))
        .arg(metadata)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/NordwandMono-Regular.ttf"
        ))
        .status()?;
    anyhow::ensure!(status.success(), "screenshot annotations failed: {status}");
    Ok(())
}

#[test]
#[ignore = "requires Xvfb, ImageMagick and demo inputs; run nix build .#screenshots"]
fn readme() {
    for variable in [
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "SCREENSHOTS_CLIP",
        "SCREENSHOTS_PROFILE",
        "SCREENSHOTS_OUT",
        "SCREENSHOTS_SCENE",
    ] {
        assert!(
            std::env::var_os(variable).is_some(),
            "set {variable} for isolated screenshot rendering"
        );
    }
    let path = |name: &str| PathBuf::from(std::env::var_os(name).unwrap());
    let scene = std::env::var("SCREENSHOTS_SCENE").unwrap();
    assert!(
        [
            "main",
            "lens",
            "masking",
            "selection",
            "output",
            "processing"
        ]
        .contains(&scene.as_str())
    );
    let model = std::env::var_os("SCREENSHOTS_MODEL").map(PathBuf::from);
    let mut app = App::from_settings_with(Settings::default(), model).with(Start {
        inputs: vec![path("SCREENSHOTS_CLIP")],
        profile: Some(path("SCREENSHOTS_PROFILE")),
        out_dir: Some(path("XDG_STATE_HOME").join("demo-output")),
    });
    app.resume_prompt = false;
    app.cfg.mask_sky = scene != "lens";
    app.cfg.mask_people = scene == "masking" && app.cfg.seg_model.is_some();
    app.cfg.size_choice = SizeChoice::Scale;
    app.cfg.scale_factor = 0.5;
    app.cfg.gpu = false;
    if scene == "selection" {
        app.cfg.blur_floor_on = true;
    }
    let proof = std::env::var_os("SCREENSHOTS_PROGRESS_PROOF").is_some();
    if proof {
        app.cfg.undistort = false;
        app.cfg.mask_sky = false;
        app.cfg.mask_people = false;
        app.cfg.size_choice = SizeChoice::Same;
    }
    let capture = Arc::new(Capture {
        scene,
        proof,
        output: path("SCREENSHOTS_OUT"),
        result: Mutex::new(None),
    });
    *CAPTURE.lock().unwrap() = Some(capture.clone());
    native::run_app(app, [1280.0, 720.0]).expect("native screenshot window");
    let result = capture
        .result
        .lock()
        .unwrap()
        .take()
        .expect("window closed before capture");
    result.expect("native screenshot capture");
    if proof {
        let read = |name: &str| -> std::collections::BTreeMap<String, [f32; 4]> {
            serde_json::from_slice(
                &std::fs::read(capture.output.join("clean").join(format!("{name}.json"))).unwrap(),
            )
            .unwrap()
        };
        let early = read("progress-early");
        let later = read("progress-later");
        let cancelled = read("cancelled");
        assert!(early["progress-data"][0] < later["progress-data"][0]);
        assert!(
            early["progress-fill"][2] > 0. && early["progress-fill"][2] < later["progress-fill"][2]
        );
        for snapshot in [&early, &later] {
            // While running, Cancel replaces Process in the same dock slot.
            assert!(
                snapshot.contains_key("cancel")
                    && !snapshot.contains_key("process")
                    && snapshot.contains_key("spinner")
            );
            let actual = snapshot["progress-fill"][2] / snapshot["progress-track"][2];
            let expected = snapshot["progress-data"][0] / snapshot["progress-data"][1];
            assert!(
                (actual - expected).abs() < 0.1,
                "rendered bar must reflect decoded frame fraction"
            );
        }
        assert!(!cancelled.contains_key("progress-track") && !cancelled.contains_key("spinner"));
        assert!(cancelled.contains_key("status") && cancelled.contains_key("process"));
    }
    *CAPTURE.lock().unwrap() = None;
}

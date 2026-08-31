// Asked once at launch, and only when both halves agree: the note exists and the manifest still says `completed: false`.

use eframe::egui;

use crate::App;
use crate::icons;
use crate::widgets::{modal, mono, primary_button};

/// Kept out of `App` because it is a fact about this window, not about the job.
#[derive(Default)]
pub struct Prompt {
    pub dismissed: bool,
    /// Read once: the manifest behind it does not change while the prompt is
    /// up, and reading it per repaint parsed the whole file every frame.
    progress: Option<Option<(u64, usize)>>,
}

pub fn window(app: &mut App, ui_state: &mut Prompt, ctx: &egui::Context) {
    if ui_state.dismissed || app.running {
        return;
    }
    let Some(run) = app.interrupted.clone() else {
        return;
    };

    let progress = *ui_state.progress.get_or_insert_with(|| run.progress());

    let mut choice: Option<Choice> = None;
    let closed = modal(
        ctx,
        "unfinished_run",
        icons::RESUME,
        "Unfinished run",
        480.0,
        |ui| {
            ui.label("A run into this folder stopped before it finished.");
            ui.add_space(6.0);
            ui.label(mono(run.out_dir.display().to_string()).size(11.0));
            let detail = match progress {
                Some((written, clips)) => format!(
                    "{written} frames written across {clips} clip{}{}",
                    crate::plural(clips),
                    ago(run.started_unix)
                ),
                None => format!("started{}", ago(run.started_unix)),
            };
            ui.label(egui::RichText::new(detail).weak().size(11.0));
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "Resuming keeps those frames and continues with the clips that are not \
                     done. It restores the settings the run used, because a resume with \
                     different settings is refused rather than mixed into one dataset.",
                )
                .weak()
                .size(11.0),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if primary_button(ui, icons::RESUME, "Resume it").clicked() {
                    choice = Some(Choice::Resume);
                }
                if crate::widgets::clickable(ui.button("Not now"))
                    .on_hover_text("Leave it alone. You will be asked again next time.")
                    .clicked()
                {
                    choice = Some(Choice::Later);
                }
                if icons::button(ui, icons::CLEAR, "Forget it")
                    .on_hover_text(
                        "Stop tracking this run. The images already written are not touched.",
                    )
                    .clicked()
                {
                    choice = Some(Choice::Forget);
                }
            });
        },
    );

    match choice {
        Some(Choice::Resume) => {
            ui_state.dismissed = true;
            app.adopt_interrupted();
        }
        Some(Choice::Later) => ui_state.dismissed = true,
        Some(Choice::Forget) => {
            ui_state.dismissed = true;
            crate::settings::InterruptedRun::clear();
            app.interrupted = None;
        }
        // Closing the dialog is "not now": the question comes back next launch.
        None if closed => ui_state.dismissed = true,
        None => {}
    }
}

enum Choice {
    Resume,
    Later,
    Forget,
}

/// " · 2 hours ago", or nothing if the clock disagrees with itself.
fn ago(started_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let Some(secs) = now.checked_sub(started_unix).filter(|s| *s > 0) else {
        return String::new();
    };
    let (n, unit) = match secs {
        s if s < 90 => (s, "second"),
        s if s < 5400 => (s / 60, "minute"),
        s if s < 129_600 => (s / 3600, "hour"),
        s => (s / 86400, "day"),
    };
    format!(" · {n} {unit}{} ago", crate::plural(n))
}

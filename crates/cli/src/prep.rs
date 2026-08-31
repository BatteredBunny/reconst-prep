//! The bare invocation: run the dataset pipeline and report on it.

use indicatif::{ProgressBar, ProgressStyle};

use reconst_prep_core::cancel::{CancelToken, is_cancelled};
use reconst_prep_core::pipeline::{Progress, run_pipeline};

use crate::cli::PrepArgs;
use crate::config::build_config;

/// Exits 2 for an unusable configuration, 1 for a failed run.
pub fn run(args: PrepArgs) {
    let cfg = match build_config(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };

    let cancel = install_cancel_handler();
    let mut progress = ClipProgress::new(args.quiet);

    let result = run_pipeline(&cfg, &mut |p| progress.update(&p), &cancel);
    progress.clear();

    match result {
        Ok(m) => {
            eprintln!(
                "done: {} frames decoded, {} kept, {} written to {} in {:.1}s ({:.1} decoded fps, {:.1} written fps)",
                m.totals.decoded,
                m.totals.kept,
                m.totals.written,
                cfg.out_dir.display(),
                m.totals.wall_seconds,
                m.totals.decoded_fps,
                m.totals.written_fps,
            );
        }
        // The clip being decoded is discarded rather than recorded half-done, so this is an interruption.
        Err(e) if is_cancelled(&e) => {
            eprintln!("cancelled — partial clip discarded; rerun with --resume to continue");
            std::process::exit(130);
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

/// First ^C stops cleanly between frames; a second exits immediately.
fn install_cancel_handler() -> CancelToken {
    let cancel = CancelToken::new();
    let flag = cancel.clone();
    let _ = ctrlc::set_handler(move || {
        if flag.cancel() {
            std::process::exit(130);
        }
        eprintln!(
            "\ninterrupt: finishing current frame and cleaning up (press ^C again to force quit)"
        );
    });
    cancel
}

/// One bar per clip: a clip is the only thing with a known length to draw against.
struct ClipProgress {
    quiet: bool,
    current: Option<(usize, ProgressBar)>,
}

impl ClipProgress {
    fn new(quiet: bool) -> Self {
        Self {
            quiet,
            current: None,
        }
    }

    fn update(&mut self, p: &Progress) {
        if self.quiet {
            return;
        }
        if self.current.as_ref().is_none_or(|(i, _)| *i != p.clip_idx) {
            self.clear();
            self.current = Some((p.clip_idx, clip_bar(p)));
        }
        let Some((_, bar)) = &self.current else {
            return;
        };
        bar.set_position(p.clip_decoded);
        bar.set_message(format!(
            "kept {} written {} ({:.1} fps)",
            p.total_kept,
            p.total_written,
            p.decoded_this_run as f64 / p.elapsed_s.max(1e-9),
        ));
    }

    /// Leave the terminal clean, whether the run finished or was interrupted.
    fn clear(&mut self) {
        if let Some((_, bar)) = self.current.take() {
            bar.finish_and_clear();
        }
    }
}

fn clip_bar(p: &Progress) -> ProgressBar {
    // No frame count means no bar and no ETA: there is nothing to be a fraction of.
    let (bar, template) = match p.clip_total_frames {
        Some(total) => (
            ProgressBar::new(total),
            // `wide_bar` takes what the rest of the line leaves, so the whole thing fits one terminal row.
            "{prefix} {wide_bar} {pos}/{len} {msg} eta {eta}",
        ),
        None => (ProgressBar::no_length(), "{prefix} {pos} {msg}"),
    };
    if let Ok(style) = ProgressStyle::with_template(template) {
        bar.set_style(style.progress_chars("=> "));
    }
    bar.set_prefix(format!(
        "[{}/{}] {}",
        p.clip_idx + 1,
        p.n_clips,
        p.clip_name
    ));
    bar
}

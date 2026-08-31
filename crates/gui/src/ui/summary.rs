// Answers what no live readout can: not how many frames were kept but *where*, which is only knowable once the clip is through.

use eframe::egui;

use reconst_prep_core::pipeline::{ManifestClip, RunManifest};

use crate::App;
use crate::icons;
use crate::widgets::{modal, mono};

/// Kept frames per clip, and the widest hole between them.
struct Coverage {
    kept: u64,
    decoded: u64,
    /// Longest run of consecutive frames with nothing kept, in seconds.
    gap_s: f64,
    /// Typical spacing, for comparison against the gap.
    median_gap_s: f64,
}

fn coverage(clip: &ManifestClip) -> Coverage {
    let fps = clip.fps.max(1e-9);
    let mut gaps: Vec<u64> = clip
        .kept_frames
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .collect();
    let worst = gaps.iter().copied().max().unwrap_or(0);
    gaps.sort_unstable();
    let median = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
    Coverage {
        kept: clip.frames_kept,
        decoded: clip.frames_decoded,
        gap_s: worst as f64 / fps,
        median_gap_s: median as f64 / fps,
    }
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(manifest) = app.summary.clone() else {
        return;
    };
    let closed = modal(ctx, "summary", icons::DONE, "Run finished", 520.0, |ui| {
        body(&manifest, ui);
    });
    if closed {
        app.summary = None;
    }
}

fn body(m: &RunManifest, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(mono(crate::ui::status::totals_line(
            m.totals.kept,
            m.totals.decoded,
            m.totals.written,
        )));
        ui.label(
            egui::RichText::new(format!(
                "{:.0}s at {:.1} fps",
                m.totals.wall_seconds, m.totals.decoded_fps
            ))
            .weak()
            .size(11.0),
        );
    });

    ui.add_space(10.0);
    ui.label(egui::RichText::new("COVERAGE").strong().size(11.0));
    ui.add_space(4.0);

    let mut worst_ratio = 0.0f64;
    for clip in &m.clips {
        let c = coverage(clip);
        let ratio = if c.median_gap_s > 0.0 {
            c.gap_s / c.median_gap_s
        } else {
            0.0
        };
        worst_ratio = worst_ratio.max(ratio);
        ui.horizontal(|ui| {
            ui.label(mono(clip.stem.as_str()).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = mono(format!(
                    "{:>5} kept / {:<6}  every {:.1}s  worst hole {:.1}s",
                    c.kept, c.decoded, c.median_gap_s, c.gap_s
                ))
                .size(11.0);
                // Amber only when it is worth looking at.
                if ratio >= 4.0 {
                    ui.label(text.color(ui.visuals().warn_fg_color));
                } else {
                    ui.label(text.weak());
                }
            });
        });
    }

    if worst_ratio >= 4.0 {
        ui.add_space(6.0);
        crate::widgets::warn(
            ui,
            "at least one clip has a hole several times its usual spacing. Lower the \
             movement threshold, or check that stretch for blur",
        );
    }
}

// The same lens calibrated at 4:3 and 16:9 has a different crop, so a wrong variant silently undistorts to the wrong field of view.

use eframe::egui;

use reconst_prep_core::profiles;

use crate::icons::{self, icon_button};
use crate::widgets::{self, category, check, combo, mono, short_name, warn};
use crate::{App, Msg};

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut on = app.cfg.undistort;
    category(
        ui,
        "cat_lens",
        icons::LENS,
        "Lens correction",
        Some(&mut on),
        false,
        |ui| {
            ui.weak("Removes the fisheye distortion using a Gyroflow lens profile.");
            ui.add_space(4.0);
            picker(app, ui, ctx);
            validation(app, ui);
            ui.add_space(6.0);
            how(app, ui);
        },
    );
    app.cfg.undistort = on;
}

/// Both settings only ever affect undistortion, so they live here rather than in Advanced.
fn how(app: &mut App, ui: &mut egui::Ui) {
    check(ui, &mut app.cfg.gpu, "Use the graphics card").on_hover_text(
        "Undistort on the graphics card. Fastest at native output size; \
         falls back to the processor when no usable device is found.",
    );
    combo(ui, "interpolation", &mut app.cfg.interp);
}

// TODO: calibrate in-app; most fisheye rigs have no profile in gyroflow's database.
fn picker(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.horizontal(|ui| {
        if icons::button(ui, icons::BROWSE, "Browse…").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("json", &crate::widgets::extensions(&["json"]))
                .pick_file()
        {
            app.cfg.profile = Some(f);
        }
        if icons::button(ui, icons::SEARCH, "Search database…")
            .on_hover_text(
                "Search Gyroflow's published calibrations. The index is cached, so this only \
                 touches the network when the cache is stale. Not during a run.",
            )
            .clicked()
        {
            app.browser.open = true;
            app.browser.focus_query = true;
            // Opening is a fresh question: a stale query or fetch error would bury the recents and the suggestion.
            app.browser.query.clear();
            app.browser.error = None;
            if app.browser.entries.is_empty() {
                load_index(app, ctx);
            }
        }
    });
    let Some(path) = app.cfg.profile.clone() else {
        warn(ui, "no profile chosen");
        return;
    };
    ui.horizontal(|ui| {
        ui.label(mono(short_name(&path)))
            .on_hover_text(path.display().to_string());
        if icon_button(ui, icons::REMOVE, "Forget this lens profile").clicked() {
            app.cfg.profile = None;
        }
    });
}

/// The profile as loaded, and whether it matches the footage.
fn validation(app: &mut App, ui: &mut egui::Ui) {
    let summary = match app.profile_summary() {
        None => return,
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => {
            let e = e.clone();
            widgets::error(ui, e);
            return;
        }
    };
    ui.label(
        mono(format!(
            "{} · {}×{} · {:.2} fps",
            if summary.camera.is_empty() {
                summary.name.clone()
            } else {
                summary.camera.clone()
            },
            summary.calib_w,
            summary.calib_h,
            summary.fps
        ))
        .weak()
        .size(11.0),
    );
    // Compare against the clip that is actually being previewed.
    if let Some(info) = app.preview_clip_info()
        && let Some(problem) = summary.mismatch(info.width, info.height, info.fps)
    {
        warn(ui, problem);
    }
}

fn load_index(app: &mut App, ctx: &egui::Context) {
    app.browser.loading = true;
    app.browser.error = None;
    app.spawn_worker("profile-index", ctx, || {
        Msg::ProfileIndex(profiles::index(false, false).map_err(|e| format!("{e:#}")))
    });
}

/// How many profiles the dialog lists before anything is typed.
const OPENING_ROWS: usize = 5;

/// How tall the result list may get before it scrolls instead.
const LIST_MAX_H: f32 = 320.0;

/// Drawn from the top level: inside the category it grew the config column past the window.
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.browser.open {
        return;
    }
    let closed = widgets::modal(
        ctx,
        "profile_browser",
        icons::LENS,
        "Lens profile database",
        560.0,
        |ui| browser(app, ui, ctx),
    );
    if closed {
        app.browser.open = false;
    }
}

/// The search panel: suggestions first, then whatever the query matches.
fn browser(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut submit = false;
    ui.horizontal(|ui| {
        ui.label(icons::SEARCH);
        let response = ui.add(
            egui::TextEdit::singleline(&mut app.browser.query)
                .hint_text("maker, model, resolution…")
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut app.browser.focus_query) {
            response.request_focus();
        }
        submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });

    if app.browser.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("fetching the profile index…");
        });
    }
    if let Some(e) = &app.browser.error {
        widgets::error(ui, e.clone());
    }

    // Auto-suggest from the probe: profile filenames encode frame size.
    let suggestion = suggestion_query(app);
    let typed = !app.browser.query.trim().is_empty();
    // Recents only while nothing is typed: a second list beside the results reads as more results.
    let shown = if typed { 0 } else { recents(app, ui) };
    if !typed && let Some(q) = &suggestion {
        ui.horizontal(|ui| {
            ui.weak("suggested from this clip:");
            if widgets::clickable(ui.small_button(mono(q.as_str())))
                .on_hover_text("search for it, for the whole list of variants")
                .clicked()
            {
                app.browser.query = q.clone();
            }
        });
    }

    // An empty query lists the clip's likely variants, or the head of the database, rather than nothing.
    let query = if typed {
        app.browser.query.clone()
    } else {
        suggestion.clone().unwrap_or_default()
    };
    let limit = if typed {
        200
    } else {
        OPENING_ROWS.saturating_sub(shown)
    };
    if limit == 0 || app.browser.entries.is_empty() {
        // An Enter that picked nothing must not strand the caret.
        app.browser.focus_query |= submit;
        return;
    }
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let hits = profiles::search(&app.browser.entries, &query);
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(if typed {
            format!("{} of {} profiles", hits.len(), app.browser.entries.len())
        } else if suggestion.is_some() {
            format!("{} matching this clip", hits.len())
        } else {
            format!("{} profiles in the database", hits.len())
        })
        .weak()
        .size(11.0),
    );
    // Enter takes the top hit.
    let mut pick: Option<profiles::ProfileEntry> =
        submit.then(|| hits.first().map(|e| (*e).clone())).flatten();
    // Ceiling and floor both: a dialog's `Ui` inherits *last frame's* rect, so a scroll area sized from it could never grow.
    let row_h = ui.text_style_height(&egui::TextStyle::Button)
        + 2.0 * ui.spacing().button_padding.y
        + ui.spacing().item_spacing.y
        + 4.0;
    let list_h = (hits.len().min(limit) as f32 * row_h).min(LIST_MAX_H);
    egui::ScrollArea::vertical()
        .id_salt("profile_hits")
        .max_height(list_h)
        .min_scrolled_height(list_h)
        .show(ui, |ui| {
            // One line per row, elided rather than wrapped.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for e in hits.iter().take(limit) {
                if widgets::selectable(ui, false, row_text(ui, e, &terms))
                    .on_hover_text(&e.path)
                    .clicked()
                {
                    pick = Some((*e).clone());
                }
            }
        });
    if let Some(entry) = pick {
        app.browser.loading = true;
        app.browser.error = None;
        // The dialog stays up until the download lands, so a failed fetch reports into the list it came from.
        app.spawn_worker("profile-fetch", ctx, move || {
            Msg::ProfileFetched(profiles::fetch(&entry).map_err(|e| format!("{e:#}")))
        });
    } else {
        // An Enter that picked nothing must not strand the caret.
        app.browser.focus_query |= submit;
    }
}

/// Returns how many rows it drew, which is what the list below is topped up to [`OPENING_ROWS`] from.
fn recents(app: &mut App, ui: &mut egui::Ui) -> usize {
    if app.browser.recent.is_empty() {
        return 0;
    }
    ui.label(egui::RichText::new("Recent").weak().size(11.0));
    // The pick is carried out of the loop rather than the list cloned into it.
    let mut picked = None;
    let mut shown = 0;
    for p in app.browser.recent.iter().take(OPENING_ROWS) {
        shown += 1;
        if widgets::selectable(ui, app.cfg.profile.as_ref() == Some(p), short_name(p))
            .on_hover_text(p.display().to_string())
            .clicked()
        {
            picked = Some(p.clone());
        }
    }
    if let Some(p) = picked {
        // Already on disk: no fetch, so this closes the dialog itself.
        app.cfg.profile = Some(p);
        app.browser.open = false;
    }
    ui.add_space(6.0);
    shown
}

/// The maker directory is dimmed in front because the search matches the whole path, so "dji" selects rows whose name never says DJI.
fn row_text(
    ui: &egui::Ui,
    entry: &profiles::ProfileEntry,
    terms: &[String],
) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    // The extension is the one part of the matched path never shown.
    let stem = entry.path.strip_suffix(".json").unwrap_or(&entry.path);
    let (dir, name) = match stem.rsplit_once('/') {
        Some((d, n)) => (d, n),
        None => ("", stem),
    };

    // No wrapping set here: egui replaces a job's own `TextWrapping` with the containing widget's.
    let mut job = egui::text::LayoutJob::default();
    if !dir.is_empty() {
        push_terms(
            &mut job,
            &format!("{dir}/"),
            terms,
            ui.visuals().weak_text_color(),
            &font,
        );
    }
    push_terms(&mut job, name, terms, ui.visuals().text_color(), &font);
    job
}

fn push_terms(
    job: &mut egui::text::LayoutJob,
    text: &str,
    terms: &[String],
    base: egui::Color32,
    font: &egui::FontId,
) {
    let hay = text.to_ascii_lowercase();
    let mut hit = vec![false; text.len()];
    for term in terms {
        let mut from = 0;
        // Every occurrence, not the first.
        while let Some(at) = hay[from..].find(term.as_str()) {
            let start = from + at;
            hit[start..start + term.len()].fill(true);
            from = start + 1;
        }
    }

    let mut runs: Vec<(bool, usize, usize)> = Vec::new();
    for (i, ch) in text.char_indices() {
        let on = hit[i];
        match runs.last_mut() {
            Some(last) if last.0 == on => last.2 = i + ch.len_utf8(),
            _ => runs.push((on, i, i + ch.len_utf8())),
        }
    }
    for (on, start, end) in runs {
        job.append(
            &text[start..end],
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: if on { crate::theme::AMBER } else { base },
                ..Default::default()
            },
        );
    }
}

/// A search query built from what we already know about the clip.
fn suggestion_query(app: &App) -> Option<String> {
    let info = app.preview_clip_info()?;
    Some(format!("{}x{}", info.width, info.height))
}

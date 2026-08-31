// Amber marks meaning (primary action, active toggle, slider handle) and never decoration. Dark only.

use eframe::egui;
use egui::{Color32, CornerRadius, Stroke};

/// Graphite rather than pure black, so panels can sit *above* it without a border.
const BACKGROUND: Color32 = Color32::from_rgb(0x0f, 0x10, 0x12);
/// Panels and inputs, one step up from the background.
const PANEL: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
const PANEL_HOVER: Color32 = Color32::from_rgb(0x1f, 0x21, 0x25);
const PANEL_ACTIVE: Color32 = Color32::from_rgb(0x27, 0x2a, 0x2f);
/// The one accent.
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb4, 0x54);
/// Text drawn *on* the accent: the normal light grey disappears into it.
pub const ON_AMBER: Color32 = BACKGROUND;
const HAIRLINE: Color32 = Color32::from_rgb(0x2a, 0x2d, 0x32);

const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe0);
const TEXT_WEAK: Color32 = Color32::from_rgb(0x87, 0x8d, 0x96);

const CORNER: CornerRadius = CornerRadius::same(2);

/// Motion serves feedback; anything slower reads as the UI itself being slow.
pub const MOTION_S: f32 = 0.12;

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    // Both slots, so the window manager's preference cannot hand the user a half-styled light theme.
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_visuals_of(theme, visuals());
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.slider_width = 150.0;
            style.spacing.indent = 14.0;
            style.animation_time = MOTION_S;
        });
    }
}

/// Lexend is vendored so the interface cannot look different on a machine with other fonts installed; readouts stay monospaced.
fn install_fonts(ctx: &egui::Context) {
    const LEXEND: &[u8] = include_bytes!("../assets/fonts/Lexend[wght].ttf");

    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("lexend".into(), egui::FontData::from_static(LEXEND).into());
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "lexend".into());
    }

    // Phosphor into the monospaced family too, which `add_to_fonts` does not do: an icon in a readout would be a tofu box.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.push("phosphor".into());
    }
    ctx.set_fonts(fonts);
}

/// The window/task-bar icon, rasterized from `assets/icon.svg`.
pub fn app_icon() -> egui::IconData {
    let png = include_bytes!("../../../assets/icon-256.png");
    let img = image::load_from_memory(png)
        .expect("embedded icon PNG is valid")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }
}

/// Decoded on first call, then served from the context's data store.
pub fn icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("app-icon-texture");
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let icon = app_icon();
    let img = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    // Mipmapped: the 256px source is drawn at 18pt, where plain linear minification reads as blur.
    let tex = ctx.load_texture(
        "app-icon",
        img,
        egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();

    v.panel_fill = BACKGROUND;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0b, 0x0d);
    v.faint_bg_color = Color32::from_rgb(0x14, 0x15, 0x18);
    v.window_stroke = Stroke::new(1.0, HAIRLINE);
    v.window_corner_radius = CORNER;
    v.menu_corner_radius = CORNER;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = CORNER;

    w.inactive.bg_fill = PANEL;
    w.inactive.weak_bg_fill = PANEL;
    w.inactive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = CORNER;

    w.hovered.bg_fill = PANEL_HOVER;
    w.hovered.weak_bg_fill = PANEL_HOVER;
    // Hover is a hairline brightening, not a colour change.
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3d, 0x41, 0x48));
    w.hovered.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xf0, 0xf2, 0xf5));
    w.hovered.corner_radius = CORNER;

    w.active.bg_fill = PANEL_ACTIVE;
    w.active.weak_bg_fill = PANEL_ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0, AMBER);
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = CORNER;

    w.open.bg_fill = PANEL_ACTIVE;
    w.open.bg_stroke = Stroke::new(1.0, HAIRLINE);
    w.open.corner_radius = CORNER;

    v.selection.bg_fill = AMBER.linear_multiply(0.28);
    v.selection.stroke = Stroke::new(1.0, AMBER);

    // A hand over anything clickable; egui leaves this off, and this interface has a lot of custom-painted clickable area.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    v.hyperlink_color = AMBER;
    v.warn_fg_color = Color32::from_rgb(0xe8, 0xc0, 0x60);
    v.error_fg_color = Color32::from_rgb(0xe8, 0x6a, 0x6a);
    v.weak_text_alpha = 0.72;
    v.weak_text_color = Some(TEXT_WEAK);

    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;

    v
}

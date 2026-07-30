//! Visual polish layered on top of egui's own dark/light defaults: one
//! restrained accent color, slightly rounder corners, and roomier spacing
//! than egui's default (quite compact, "dev tool"-density) look -- more
//! comfortable for a document-viewing app. Both the dark and light variants
//! are set explicitly (via `*_of(Theme::_, ..)`) so switching the OS theme
//! doesn't fall back to stock egui for the one not currently active.

use egui::{Color32, Context, CornerRadius, Margin, Stroke, Theme, Vec2, Visuals};

const CORNER_RADIUS: u8 = 6;
const ACCENT_DARK: Color32 = Color32::from_rgb(88, 150, 219);
const ACCENT_LIGHT: Color32 = Color32::from_rgb(37, 99, 160);

pub fn apply(ctx: &Context) {
    ctx.set_visuals_of(Theme::Dark, themed_visuals(Visuals::dark(), ACCENT_DARK));
    ctx.set_visuals_of(Theme::Light, themed_visuals(Visuals::light(), ACCENT_LIGHT));

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.menu_margin = Margin::same(10);
        style.spacing.window_margin = Margin::same(10);
        style.spacing.indent = 20.0;

        // A modest bump across the board -- egui's defaults read a little
        // small for a document-reading app.
        for font_id in style.text_styles.values_mut() {
            font_id.size += 1.0;
        }
    });
}

fn themed_visuals(mut visuals: Visuals, accent: Color32) -> Visuals {
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, accent);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.weak_bg_fill = accent;
    visuals.widgets.hovered.bg_fill = accent.gamma_multiply(0.5);
    visuals.widgets.hovered.weak_bg_fill = accent.gamma_multiply(0.18);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(CORNER_RADIUS);
    }
    visuals.window_corner_radius = CornerRadius::same(CORNER_RADIUS + 2);
    visuals.menu_corner_radius = CornerRadius::same(CORNER_RADIUS + 2);

    visuals
}

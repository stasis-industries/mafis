use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke};

// ── Web UI dark mode palette (exact match) ───────────────────────────
pub const BG_BODY: Color32 = Color32::from_rgb(18, 18, 22);
pub const BG_PANEL: Color32 = Color32::from_rgb(24, 24, 30);
pub const BG_CARD: Color32 = Color32::from_rgb(30, 30, 38);
pub const BG_INPUT: Color32 = Color32::from_rgb(36, 36, 44);
pub const BG_HOVER: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 15); // ~6%
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 228, 224);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(140, 140, 148);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(90, 90, 98);
pub const BORDER: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 18); // ~7%
pub const BORDER_STRONG: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 31); // ~12%
pub const BORDER_FOCUS: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 89); // ~35%

// ── State colors (consistent across themes) ──────────────────────────
pub const STATE_IDLE: Color32 = Color32::from_rgb(160, 160, 160);
pub const STATE_RUNNING: Color32 = Color32::from_rgb(45, 160, 0);
pub const STATE_PAUSED: Color32 = Color32::from_rgb(230, 140, 0);
pub const STATE_FAULT: Color32 = Color32::from_rgb(230, 44, 2);

// ── Scorecard zone colors (dark mode) ────────────────────────────────
pub const ZONE_GOOD: Color32 = Color32::from_rgb(100, 180, 100);
pub const ZONE_FAIR: Color32 = Color32::from_rgb(200, 165, 65);
pub const ZONE_POOR: Color32 = Color32::from_rgb(200, 80, 80);

// ── Chart colors ─────────────────────────────────────────────────────
pub const CHART_PRIMARY: Color32 = Color32::from_rgb(45, 160, 0);
pub const CHART_SECONDARY: Color32 = Color32::from_rgb(230, 140, 0);
pub const CHART_BASELINE: Color32 = Color32::from_rgb(100, 100, 108);
pub const CHART_HEAT: Color32 = Color32::from_rgb(230, 44, 2);

use super::ThemeApplied;

/// Applies the web-matched theme once the EguiContext entity exists.
pub fn apply_theme_once(
    mut contexts: EguiContexts,
    mut applied: ResMut<ThemeApplied>,
) -> Result {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    // ── Fonts ───────────────────────────────────────────────────────
    let mut fonts = FontDefinitions::default();

    // DM Mono — monospace values, controls, labels
    fonts.font_data.insert(
        "DMMono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/DMMono-Regular.ttf"
        ))),
    );

    // Inter — body text, descriptions
    fonts.font_data.insert(
        "Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/Inter-Variable.ttf"
        ))),
    );

    // Proportional: Inter first, DM Mono as fallback
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".to_owned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("DMMono".to_owned());

    // Monospace: DM Mono first
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "DMMono".to_owned());

    ctx.set_fonts(fonts);

    // ── Style ───────────────────────────────────────────────────────
    let mut style = (*ctx.style()).clone();

    // Zero rounding — matches web border-radius: 0
    let zero = CornerRadius::ZERO;
    style.visuals.window_corner_radius = zero;
    style.visuals.widgets.noninteractive.corner_radius = zero;
    style.visuals.widgets.inactive.corner_radius = zero;
    style.visuals.widgets.hovered.corner_radius = zero;
    style.visuals.widgets.active.corner_radius = zero;
    style.visuals.widgets.open.corner_radius = zero;

    // Dark palette matching web dark mode
    style.visuals.dark_mode = true;
    style.visuals.override_text_color = Some(TEXT_PRIMARY);
    style.visuals.window_fill = BG_PANEL;
    style.visuals.panel_fill = BG_PANEL;
    style.visuals.faint_bg_color = BG_BODY;
    style.visuals.extreme_bg_color = BG_BODY;

    // Widget states
    style.visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);

    style.visuals.widgets.inactive.bg_fill = BG_INPUT;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);

    style.visuals.widgets.hovered.bg_fill = BG_CARD;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);

    style.visuals.widgets.active.bg_fill = BG_INPUT;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, TEXT_PRIMARY);

    style.visuals.widgets.open.bg_fill = BG_CARD;
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);

    // Selection
    style.visuals.selection.bg_fill = Color32::from_rgba_premultiplied(45, 160, 0, 40);
    style.visuals.selection.stroke = Stroke::new(1.0, STATE_RUNNING);

    // Window stroke
    style.visuals.window_stroke = Stroke::new(1.0, BORDER_STRONG);

    // Spacing — dense but readable (matching web)
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(8);
    style.spacing.indent = 12.0;
    style.spacing.slider_width = 140.0;

    ctx.set_style(style);

    applied.0 = true;
    Ok(())
}

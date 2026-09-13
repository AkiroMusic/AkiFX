//! AkiZen-inspired design system for the AkiFX egui GUI.
//!
//! Defines 13 color tokens, dark-theme visuals, font loading, and reusable UI helpers.
//! All color constants are public so downstream modules (rack UI, etc.) can reference them.

#[cfg_attr(test, allow(unused_imports))]
use nih_plug_egui::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, Stroke, StrokeKind};
#[cfg(test)]
#[allow(unused_imports)]
use nih_plug_egui::egui::FontData as _FontDataAlias;
use nih_plug_egui::egui::Vec2;
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// Color tokens — AkiZen palette
// ---------------------------------------------------------------------------

/// Deepest background — `#10101a`
pub const INK_950: Color32 = Color32::from_rgb(0x10, 0x10, 0x1a);
/// Panel background — `#16161f`
pub const INK_900: Color32 = Color32::from_rgb(0x16, 0x16, 0x1f);
/// Card background — `#2a2a3a`
pub const CHARCOAL_700: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x3a);
/// Borders / tracks — `#363649`
pub const CHARCOAL_600: Color32 = Color32::from_rgb(0x36, 0x36, 0x49);
/// Secondary text — `#8f92a8`
pub const MIST_400: Color32 = Color32::from_rgb(0x8f, 0x92, 0xa8);
/// Primary text — `#b4b7c9`
pub const MIST_300: Color32 = Color32::from_rgb(0xb4, 0xb7, 0xc9);
/// Warm accent deep — `#c9955f` (palette-complete; reserved for future warm highlights)
#[allow(dead_code)]
pub const SAND_500: Color32 = Color32::from_rgb(0xc9, 0x95, 0x5f);
/// Warm accent mid — `#d4a574`
pub const SAND_400: Color32 = Color32::from_rgb(0xd4, 0xa5, 0x74);
/// Warm accent light — `#e3c39d`
pub const SAND_300: Color32 = Color32::from_rgb(0xe3, 0xc3, 0x9d);
/// Cool accent deep — `#4a7261`
pub const JADE_600: Color32 = Color32::from_rgb(0x4a, 0x72, 0x61);
/// Cool accent mid — `#5b8a72`
pub const JADE_500: Color32 = Color32::from_rgb(0x5b, 0x8a, 0x72);
/// Cool accent light — `#79a68d`
pub const JADE_400: Color32 = Color32::from_rgb(0x79, 0xa6, 0x8d);
/// Cool accent pale — `#9cc2ac`
pub const JADE_300: Color32 = Color32::from_rgb(0x9c, 0xc2, 0xac);

// ---------------------------------------------------------------------------
// Alpha-variant tokens — semi-transparent overlays and glows
// ---------------------------------------------------------------------------

/// `JADE_400` at ~60% alpha (premultiplied) — for hover backgrounds on dark surfaces.
///
/// Premultiply math (`0.6 × 255 = 153` alpha):
/// - R: `0x79 × 0.6 ≈ 0x49` (73)
/// - G: `0xa6 × 0.6 ≈ 0x63` (99)
/// - B: `0x8d × 0.6 ≈ 0x54` (84)
/// - A: 153
pub const JADE_400_60: Color32 = Color32::from_rgba_premultiplied(0x49, 0x63, 0x54, 153);

/// Full-alpha `JADE_400` alias — for glow strokes and accent outlines.
pub const GLOW_JADE: Color32 = JADE_400;

/// `CHARCOAL_700` at ~50% alpha (premultiplied) — for lifted-row / elevated-card overlays.
///
/// Premultiply math (`0.5 × 255 = 128` alpha):
/// - R: `0x2a × 0.5 = 0x15` (21)
/// - G: `0x2a × 0.5 = 0x15` (21)
/// - B: `0x3a × 0.5 = 0x1d` (29)
/// - A: 128
pub const CHARCOAL_700_50: Color32 = Color32::from_rgba_premultiplied(0x15, 0x15, 0x1d, 128);

/// Full-alpha `CHARCOAL_700` alias — for dropdown menus / popover backgrounds.
pub const CHARCOAL_700_100: Color32 = CHARCOAL_700;

/// White at ~5% alpha for subtle borders — approximates `rgba(255,255,255,0.05)`
pub const BORDER_SUBTLE: Color32 = Color32::from_rgba_premultiplied(13, 13, 15, 255);

// ---------------------------------------------------------------------------
// Font IDs — registered in `load_fonts`, used across the UI
// ---------------------------------------------------------------------------

/// UI font zoom, percent (50–200). Driven by the top-bar zoom cluster and
/// persisted through `AkiFxParams::ui_zoom`.
static FONT_ZOOM_PCT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

/// Set the font zoom percent (clamped 50–200).
pub fn set_font_zoom_pct(pct: u32) {
    FONT_ZOOM_PCT.store(pct.clamp(50, 200), Ordering::Relaxed);
}

/// Current font zoom as a multiplier (1.0 = 100%).
pub fn font_zoom() -> f32 {
    FONT_ZOOM_PCT.load(Ordering::Relaxed) as f32 / 100.0
}

/// Zoom-scaled size helper.
fn zs(size: f32) -> f32 {
    size * font_zoom()
}

/// Body text font (Inter Regular, zoom-scaled).
pub fn body(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("inter".into()))
}

/// Heading font — Cormorant Garamond SemiBold (bound in `load_fonts`).
pub fn heading(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("cormorant".into()))
}

/// Monospace font (zoom-scaled).
pub fn mono(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Monospace)
}

/// Medium-weight body text (Inter Medium). Reserved for emphasized values.
#[allow(dead_code)]
pub fn body_medium(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("inter_medium".into()))
}

// ---------------------------------------------------------------------------
// Visual theme
// ---------------------------------------------------------------------------

/// Rewrite the egui dark theme using AkiZen tokens.
pub fn configure_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    // ── Background layers ─────────────────────────────────────────────
    v.panel_fill = INK_950;
    v.window_fill = INK_900;
    v.extreme_bg_color = INK_950;

    // ── Selection ─────────────────────────────────────────────────────
    v.selection.bg_fill = JADE_500;
    v.selection.stroke = Stroke::new(1.0, JADE_300);

    // ── Widget fills ──────────────────────────────────────────────────
    v.widgets.noninteractive.bg_fill = CHARCOAL_600;
    v.widgets.inactive.bg_fill = CHARCOAL_700;
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x30, 0x30, 0x42); // charcoal_650
    v.widgets.active.bg_fill = JADE_600;
    v.widgets.noninteractive.weak_bg_fill = CHARCOAL_600;
    v.widgets.inactive.weak_bg_fill = CHARCOAL_700;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x30, 0x30, 0x42);
    v.widgets.active.weak_bg_fill = JADE_600;

    // ── Widget strokes ────────────────────────────────────────────────
    // Borders = white at ~5% alpha (subtle)
    v.widgets.noninteractive.bg_stroke = Stroke::new(0.5, BORDER_SUBTLE);
    v.widgets.inactive.bg_stroke = Stroke::new(0.5, BORDER_SUBTLE);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, JADE_500);
    v.widgets.active.bg_stroke = Stroke::new(1.0, JADE_400);

    // ── Text ──────────────────────────────────────────────────────────
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MIST_300);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, MIST_400);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, MIST_300);
    v.widgets.active.fg_stroke = Stroke::new(1.0, MIST_300);

    // ── Rounding ──────────────────────────────────────────────────────
    // Windows / menus: 12px; buttons / widgets: 8px
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(12);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(8);
    v.widgets.inactive.corner_radius = CornerRadius::same(8);
    v.widgets.hovered.corner_radius = CornerRadius::same(8);
    v.widgets.active.corner_radius = CornerRadius::same(8);
    v.widgets.open.corner_radius = CornerRadius::same(8);

    // ── Separator ─────────────────────────────────────────────────────
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, CHARCOAL_600);

    // ── Window shadow disabled (zen: no drop shadows) ─────────────────
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, CHARCOAL_600);

    ctx.set_visuals(v);
}

// ---------------------------------------------------------------------------
// Font loading
// ---------------------------------------------------------------------------

/// Whether the bytes look like a font epaint can parse (TrueType/OTF). egui
/// panics on unparsable font data inside the host's process — which takes the
/// whole DAW down — so every asset is validated here and silently skipped if
/// it is not a font.
#[cfg(not(test))]
fn is_font_file(bytes: &[u8]) -> bool {
    // sfnt magic numbers: 0x00010000 (TrueType), 'OTTO' (CFF OpenType),
    // 'true'/'ttcf' (legacy TrueType / collection).
    bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || bytes.starts_with(b"OTTO")
        || bytes.starts_with(b"true")
        || bytes.starts_with(b"ttcf")
}

/// Load custom fonts: Inter (body), Noto Sans SC (CJK fallback) and
/// Cormorant Garamond SemiBold (display/heading serif, served by
/// [`heading`]). Files that fail validation are skipped (egui falls back to
/// its built-in fonts).
pub fn load_fonts(ctx: &egui::Context) {
    #[allow(unused_mut)]
    let mut fonts = FontDefinitions::default();

    // ── Inter Regular (body) ──────────────────────────────────────────
    #[cfg(not(test))]
    {
        let inter_regular = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
        if is_font_file(inter_regular) {
            fonts
                .font_data
                .insert("inter".to_owned(), FontData::from_static(inter_regular).into());
        }

        let inter_medium = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
        if is_font_file(inter_medium) {
            fonts.font_data.insert(
                "inter_medium".to_owned(),
                FontData::from_static(inter_medium).into(),
            );
        }
    }

    // ── Cormorant Garamond SemiBold (display serif) ────────────────────
    #[cfg(not(test))]
    {
        let cormorant = include_bytes!("../../assets/fonts/CormorantGaramond-SemiBold.ttf");
        if is_font_file(cormorant) {
            fonts.font_data.insert(
                "cormorant".to_owned(),
                FontData::from_static(cormorant).into(),
            );
        }
    }

    // ── Noto Sans SC (CJK fallback) ───────────────────────────────────
    #[cfg(not(test))]
    {
        let noto_sans_sc = include_bytes!("../../assets/fonts/NotoSansSC-Regular.otf");
        if is_font_file(noto_sans_sc) {
            fonts.font_data.insert(
                "noto_sans_sc".to_owned(),
                FontData::from_static(noto_sans_sc).into(),
            );
        }
    }

    // Register Inter as the primary proportional font (prepend so it wins)
    #[cfg(not(test))]
    {
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "inter_medium".to_owned());
            family.insert(0, "inter".to_owned());
            // CJK fallback: appended after Latin so it only activates for CJK glyphs
            family.push("noto_sans_sc".to_owned());
        }
        // CRITICAL: custom families referenced by FontId::new(.., Name(..))
        // MUST be bound here or any text laid out with them panics
        // ('FontFamily::Name(...) is not bound to any fonts').
        fonts.families.insert(
            FontFamily::Name("inter".into()),
            vec!["inter".to_owned(), "noto_sans_sc".to_owned()],
        );
        fonts.families.insert(
            FontFamily::Name("inter_medium".into()),
            vec!["inter_medium".to_owned(), "noto_sans_sc".to_owned()],
        );
        fonts.families.insert(
            FontFamily::Name("cormorant".into()),
            vec![
                "cormorant".to_owned(),
                "inter".to_owned(),
                "noto_sans_sc".to_owned(),
            ],
        );
    }


    ctx.set_fonts(fonts);

    // ── Text size overrides via style ──────────────────────────────────
    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(12.0, FontFamily::Name("inter".into())),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(12.0, FontFamily::Name("inter".into())),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(18.0, FontFamily::Name("inter_medium".into())),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(10.0, FontFamily::Name("inter".into())),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(10.0, FontFamily::Monospace),
    );
    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// UI helpers — reusable for rack UI and beyond
// ---------------------------------------------------------------------------

/// Section label: uppercase, small, secondary color.
/// Approximates 10px letter-spacing via uppercase transform (true letter-spacing
/// is not supported by egui's text layout; uppercase + small size achieves the
/// intended zen section-header feel).
pub fn section_label(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(egui::FontId::new(zs(10.0), FontFamily::Name("inter_medium".into())))
        .color(MIST_400)
}

/// Section label WITHOUT forced uppercase (for humanized group headers).
pub fn section_label_raw(text: &str) -> RichText {
    RichText::new(text.to_string())
        .font(egui::FontId::new(zs(10.0), FontFamily::Name("inter_medium".into())))
        .color(MIST_400)
}


/// Power toggle pill: 36×20 track with a 16px knob.
/// Returns the `Response` — call `.clicked()` on it to toggle.
pub fn power_toggle(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let desired_size = Vec2::new(36.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Track
        let track_fill = if on { JADE_500 } else { CHARCOAL_600 };
        painter.rect_filled(rect, 10.0, track_fill);

        // Glow stroke when on
        if on {
            let glow = Color32::from_rgba_premultiplied(0x5b, 0x8a, 0x72, 100); // jade_500 ~alpha
            painter.rect_stroke(rect, 10.0, Stroke::new(1.5, glow), StrokeKind::Outside);
        }

        // Knob
        let knob_r = 8.0;
        let knob_y = rect.center().y;
        let knob_x = if on {
            rect.right() - knob_r - 3.0
        } else {
            rect.left() + knob_r + 3.0
        };
        let knob_center = egui::pos2(knob_x, knob_y);
        painter.circle_filled(knob_center, knob_r, MIST_300);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_constants_are_distinct() {
        // Sanity: INK_950 and INK_900 differ
        assert_ne!(INK_950, INK_900);
        // Jade ramp is ordered
        assert_ne!(JADE_600, JADE_500);
        assert_ne!(JADE_500, JADE_400);
        assert_ne!(JADE_400, JADE_300);
        // Sand ramp is ordered
        assert_ne!(SAND_500, SAND_400);
        assert_ne!(SAND_400, SAND_300);

        // Alpha variants differ from their base
        assert_ne!(JADE_400_60, JADE_400);
        assert_ne!(CHARCOAL_700_50, CHARCOAL_700);
        // Aliases match their bases exactly
        assert_eq!(GLOW_JADE, JADE_400);
        assert_eq!(CHARCOAL_700_100, CHARCOAL_700);
    }

    #[test]
    fn section_label_uppercase() {
        let rt = section_label("modules");
        // RichText stores the text as-is; verify uppercasing
        assert_eq!(rt.text(), "MODULES");
    }

    #[test]
    fn body_font_id_uses_inter() {
        let fid = body(14.0);
        assert_eq!(fid.family, FontFamily::Name("inter".into()));
        assert_eq!(fid.size, 14.0);
    }

    #[test]
    fn cjk_font_embedded() {
        use std::path::Path;
        let font_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/fonts/NotoSansSC-Regular.otf");
        assert!(
            font_path.exists(),
            "NotoSansSC-Regular.otf must exist at {}",
            font_path.display()
        );
        let meta = std::fs::metadata(&font_path).expect("failed to read font metadata");
        let size = meta.len();
        assert!(
            (1_000_000..=10_000_000).contains(&size),
            "NotoSansSC-Regular.otf size {} bytes not in 1MB..=10MB range",
            size
        );
    }
}

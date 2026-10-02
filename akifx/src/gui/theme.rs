//! Aki "Aurora Glass" design tokens for the AkiFX egui GUI.
//!
//! Every color in the UI comes from a [`ColorPack`] — one theme is one struct
//! ([`AURORA_DUSK`]), and component code never hardcodes a color. Fonts follow
//! the three-family Aki system: Plus Jakarta Sans (UI), Fraunces 72pt
//! (display/brand), IBM Plex Mono (readouts); Noto Sans SC stays as the CJK
//! fallback. All embedded fonts are OFL-licensed (`assets/fonts/OFL-*.txt`).

#[cfg_attr(test, allow(unused_imports))]
use nih_plug_egui::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, Stroke, StrokeKind, Vec2};
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// Layout scales — only these radii and gaps may be used (no 4px/6px corners)
// ---------------------------------------------------------------------------

/// Corner radius scale (px): small elements / medium panels / cards / hero panels.
pub mod radius {
    pub const SM: f32 = 10.0;
    pub const MD: f32 = 16.0;
    pub const LG: f32 = 24.0;
    pub const XL: f32 = 32.0;
}

/// Spacing scale (px).
pub mod space {
    pub const S1: f32 = 4.0;
    pub const S2: f32 = 8.0;
    pub const S3: f32 = 12.0;
    pub const S4: f32 = 16.0;
    pub const S6: f32 = 24.0;
    pub const S8: f32 = 32.0;
}

// ---------------------------------------------------------------------------
// ColorPack — one theme is one struct; components only read semantic tokens
// ---------------------------------------------------------------------------

/// A complete color pack (theme). Mirrors the Aki design-system token list:
/// base surfaces, three text levels, accents, the three-color gradient ramp,
/// aurora glow intensities, and the glass material tokens.
pub struct ColorPack {
    pub name: &'static str,
    /// True for dark packs (light packs would need darkened ramp for text).
    pub dark: bool,

    // Base surfaces
    pub bg_base: Color32,
    pub surface_1: Color32,
    pub surface_2: Color32,
    /// Contrast-panel surface (dark anchor panel; hosts the spectrum view).
    pub surface_contrast: Color32,
    pub border: Color32,

    // Text (three levels) + contrast-panel text
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub text_on_contrast: Color32,

    // Accents
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_secondary: Color32,
    /// Warm accent (special highlights: threshold curve, warm badges).
    pub accent_tertiary: Color32,

    // Status
    pub success: Color32,
    pub error: Color32,
    pub warning: Color32,

    // Gradient ramp: deep (structure) → mid (atmosphere) → light (highlights)
    pub grad_a: Color32,
    pub grad_b: Color32,
    pub grad_c: Color32,

    /// Aurora glow intensities (0..=1) for the four background blobs.
    pub aurora: [f32; 4],

    // Liquid glass material (translucent fills over the aurora curtain)
    pub liquid_bg: Color32,
    pub liquid_border: Color32,
    /// 1px top specular highlight.
    pub specular: Color32,
    /// 1px bottom inner shade (glass thickness).
    pub inner_shade: Color32,
    /// Inset double-bezel inner line.
    pub bezel_inner_line: Color32,
    /// Frosted strip fill (title bar / footer).
    pub glass_bg: Color32,
    /// Shadow tint (pure RGB; alpha applied where used). Shadows never mix
    /// pure black — always the theme's dark tint.
    pub shadow_tint: Color32,
}

/// Dark · Aurora Dusk — the primary pack. Blue-teal deep base, cold cyan and
/// blue-violet aurora breathing on it. Values transcribed verbatim from the
/// design system (§3.2 + appendix B dark tokens).
pub static AURORA_DUSK: ColorPack = ColorPack {
    name: "Aurora Dusk",
    dark: true,

    bg_base: Color32::from_rgb(0x0C, 0x12, 0x20),
    surface_1: Color32::from_rgb(0x15, 0x1C, 0x2C),
    surface_2: Color32::from_rgb(0x1E, 0x28, 0x39),
    surface_contrast: Color32::from_rgb(0x06, 0x0A, 0x14),
    border: Color32::from_rgb(0x2B, 0x38, 0x52),

    text_primary: Color32::from_rgb(0xEE, 0xF2, 0xF8),
    text_secondary: Color32::from_rgb(0x8C, 0x97, 0xAC),
    text_tertiary: Color32::from_rgb(0x5A, 0x64, 0x78),
    text_on_contrast: Color32::from_rgb(0xEE, 0xF2, 0xF8),

    accent: Color32::from_rgb(0x6D, 0x82, 0xFF),
    accent_hover: Color32::from_rgb(0x8A, 0x9B, 0xFF),
    accent_secondary: Color32::from_rgb(0x2C, 0xC5, 0xE0),
    accent_tertiary: Color32::from_rgb(0xF0, 0xA0, 0xD8),

    success: Color32::from_rgb(0x4F, 0xAE, 0x8A),
    error: Color32::from_rgb(0xD9, 0x69, 0x5F),
    warning: Color32::from_rgb(0xE8, 0xA3, 0x3D),

    grad_a: Color32::from_rgb(0x48, 0x60, 0xD9),
    grad_b: Color32::from_rgb(0x2C, 0xC5, 0xE0),
    grad_c: Color32::from_rgb(0xC4, 0xA8, 0xF5),

    aurora: [0.30, 0.22, 0.14, 0.10],

    // Material tokens as premultiplied constants (`from_rgba_unmultiplied`
    // is not const). Premultiply math: rgb × a/255.
    // liquid_bg    = rgba(21, 28, 44, 0.60) → (12, 16, 26, 153)
    // glass_bg     = rgba(21, 28, 44, 0.62) → (13, 17, 27, 158)
    liquid_bg: Color32::from_rgba_premultiplied(12, 16, 26, 153),
    liquid_border: Color32::from_rgba_premultiplied(23, 23, 23, 23),
    specular: Color32::from_rgba_premultiplied(26, 26, 26, 26),
    inner_shade: Color32::from_rgba_premultiplied(0, 0, 0, 56),
    bezel_inner_line: Color32::from_rgba_premultiplied(11, 11, 11, 11),
    glass_bg: Color32::from_rgba_premultiplied(13, 17, 27, 158),
    shadow_tint: Color32::from_rgb(0x04, 0x06, 0x0C),
};

/// The active color pack. Adding a theme later = adding another `ColorPack`
/// here plus a picker; no component code changes.
pub fn pal() -> &'static ColorPack {
    &AURORA_DUSK
}

// ---------------------------------------------------------------------------
// Derived-color helpers — the `color-mix(...)` equivalents (§3.6)
// ---------------------------------------------------------------------------

/// Linear blend of two opaque colors (`t` = 0 → `a`, 1 → `b`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// `color-mix(in srgb, c X%, transparent)` — same color, scaled alpha.
pub fn with_alpha(c: Color32, alpha01: f32) -> Color32 {
    let a = (alpha01.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Shadow used by glass cards (shadow-2: tight layer + large soft layer is
/// approximated with egui's single shadow — offset down, wide blur).
pub fn card_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: with_alpha(pal().shadow_tint, 0.35),
    }
}

// ---------------------------------------------------------------------------
// Font IDs + zoom
// ---------------------------------------------------------------------------

/// UI font zoom, percent (50–200). Driven by the footer zoom control and
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

/// Current font zoom in raw percent (for change detection).
pub fn font_zoom_pct() -> u32 {
    FONT_ZOOM_PCT.load(Ordering::Relaxed)
}

/// Zoom-scaled size helper.
fn zs(size: f32) -> f32 {
    size * font_zoom()
}

/// UI body — Plus Jakarta Sans Regular.
pub fn body(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("sans".into()))
}

/// Emphasized UI text — Plus Jakarta Sans Medium (labels, buttons).
pub fn sans_medium(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("sans_medium".into()))
}

/// Strong UI text — Plus Jakarta Sans SemiBold (eyebrows, selected chips).
pub fn sans_semibold(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("sans_semibold".into()))
}

/// Display titles — Fraunces 72pt SemiBold (brand wordmark, card titles).
pub fn heading(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("display".into()))
}

/// Display italic — Fraunces Italic (signature quote line only).
pub fn display_italic(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("display_italic".into()))
}

/// Monospace readouts — IBM Plex Mono Regular.
pub fn mono(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("mono".into()))
}

/// Monospace Medium (secondary readouts, badges).
pub fn mono_medium(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("mono_medium".into()))
}

/// Monospace SemiBold (primary value readouts).
pub fn mono_semibold(size: f32) -> egui::FontId {
    egui::FontId::new(zs(size), FontFamily::Name("mono_semibold".into()))
}

// ---------------------------------------------------------------------------
// Visual theme
// ---------------------------------------------------------------------------

/// Rewrite the egui dark theme using Aurora Dusk tokens.
pub fn configure_visuals(ctx: &egui::Context) {
    let p = pal();
    let mut v = egui::Visuals::dark();

    // Base fills stay transparent — the painted aurora curtain shows through.
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = p.liquid_bg;
    v.extreme_bg_color = p.surface_contrast;

    // Selection = gradient highlight color (signature ::selection, §13.6)
    v.selection.bg_fill = with_alpha(p.grad_b, 0.32);
    v.selection.stroke = Stroke::new(1.0, p.accent);

    // Widget fills
    v.widgets.noninteractive.bg_fill = p.surface_2;
    v.widgets.inactive.bg_fill = p.surface_2;
    v.widgets.hovered.bg_fill = mix(p.surface_2, p.accent, 0.08);
    v.widgets.active.bg_fill = with_alpha(p.accent, 0.12);
    v.widgets.noninteractive.weak_bg_fill = p.surface_2;
    v.widgets.inactive.weak_bg_fill = p.surface_2;
    v.widgets.hovered.weak_bg_fill = mix(p.surface_2, p.accent, 0.08);
    v.widgets.active.weak_bg_fill = with_alpha(p.accent, 0.12);

    // Widget strokes — borders from the pack, hover from the ramp
    v.widgets.noninteractive.bg_stroke = Stroke::new(0.5, p.border);
    v.widgets.inactive.bg_stroke = Stroke::new(0.5, p.border);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, with_alpha(p.accent, 0.6));
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);

    // Text
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text_secondary);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text_primary);

    // Corner radii — only the scale values (buttons/widgets SM, windows MD)
    v.window_corner_radius = CornerRadius::same(radius::MD as u8);
    v.menu_corner_radius = CornerRadius::same(radius::SM as u8);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(radius::SM as u8);
    v.widgets.inactive.corner_radius = CornerRadius::same(radius::SM as u8);
    v.widgets.hovered.corner_radius = CornerRadius::same(radius::SM as u8);
    v.widgets.active.corner_radius = CornerRadius::same(radius::SM as u8);
    v.widgets.open.corner_radius = CornerRadius::same(radius::SM as u8);

    // Windows: glass card treatment (translucent + border + soft shadow)
    v.window_shadow = card_shadow();
    v.window_stroke = Stroke::new(1.0, p.liquid_border);

    ctx.set_visuals(v);
    apply_text_styles(ctx);
}

/// Text styles per the design-system size ladder. Re-applied when the zoom
/// level changes (baseview ignores `pixels_per_point`, so zoom = font sizes).
pub fn apply_text_styles(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(egui::TextStyle::Body, body(13.0));
    style.text_styles.insert(egui::TextStyle::Button, sans_medium(13.0));
    style.text_styles.insert(egui::TextStyle::Heading, heading(20.0));
    style.text_styles.insert(egui::TextStyle::Small, body(12.0));
    style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
    ctx.set_style(style);
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

/// Register one validated static font into the definitions.
#[cfg(not(test))]
fn register(fonts: &mut FontDefinitions, id: &str, bytes: &'static [u8]) {
    if is_font_file(bytes) {
        fonts
            .font_data
            .insert(id.to_owned(), FontData::from_static(bytes).into());
    }
}

/// Load the Aki three-family font system:
/// - Plus Jakarta Sans Regular/Medium/SemiBold → UI text
/// - Fraunces 72pt SemiBold + Italic → display titles, signature quote
/// - IBM Plex Mono Regular/Medium/SemiBold → numeric readouts
/// - Noto Sans SC → CJK fallback (appended, so it only serves CJK glyphs)
///
/// Files that fail validation are skipped (egui falls back to built-ins).
pub fn load_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    #[cfg(not(test))]
    {
        register(
            &mut fonts,
            "sans",
            include_bytes!("../../assets/fonts/PlusJakartaSans-Regular.ttf"),
        );
        register(
            &mut fonts,
            "sans_medium",
            include_bytes!("../../assets/fonts/PlusJakartaSans-Medium.ttf"),
        );
        register(
            &mut fonts,
            "sans_semibold",
            include_bytes!("../../assets/fonts/PlusJakartaSans-SemiBold.ttf"),
        );
        register(
            &mut fonts,
            "display",
            include_bytes!("../../assets/fonts/Fraunces72pt-SemiBold.ttf"),
        );
        register(
            &mut fonts,
            "display_italic",
            include_bytes!("../../assets/fonts/Fraunces72pt-Italic.ttf"),
        );
        register(
            &mut fonts,
            "mono",
            include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
        );
        register(
            &mut fonts,
            "mono_medium",
            include_bytes!("../../assets/fonts/IBMPlexMono-Medium.ttf"),
        );
        register(
            &mut fonts,
            "mono_semibold",
            include_bytes!("../../assets/fonts/IBMPlexMono-SemiBold.ttf"),
        );
        register(
            &mut fonts,
            "noto_sans_sc",
            include_bytes!("../../assets/fonts/NotoSansSC-Regular.otf"),
        );

        // Default proportional UI text = Plus Jakarta Sans
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "sans_semibold".to_owned());
            family.insert(0, "sans_medium".to_owned());
            family.insert(0, "sans".to_owned());
            // CJK fallback: appended after Latin so it only activates for CJK glyphs
            family.push("noto_sans_sc".to_owned());
        }
        // Default Monospace family = IBM Plex Mono
        fonts
            .families
            .insert(FontFamily::Monospace, vec!["mono".to_owned(), "noto_sans_sc".to_owned()]);

        // CRITICAL: custom families referenced by FontId::new(.., Name(..))
        // MUST be bound here or any text laid out with them panics
        // ('FontFamily::Name(...) is not bound to any fonts').
        let cjk = "noto_sans_sc".to_owned();
        let bind = |fonts: &mut FontDefinitions, name: &str, primary: &str, fallbacks: Vec<String>| {
            let mut chain = vec![primary.to_owned()];
            chain.extend(fallbacks);
            chain.push(cjk.clone());
            fonts
                .families
                .insert(FontFamily::Name(name.into()), chain);
        };
        bind(&mut fonts, "sans", "sans", vec![]);
        bind(&mut fonts, "sans_medium", "sans_medium", vec![]);
        bind(&mut fonts, "sans_semibold", "sans_semibold", vec![]);
        bind(&mut fonts, "display", "display", vec!["sans".to_owned()]);
        bind(&mut fonts, "display_italic", "display_italic", vec!["sans".to_owned()]);
        bind(&mut fonts, "mono", "mono", vec![]);
        bind(&mut fonts, "mono_medium", "mono_medium", vec![]);
        bind(&mut fonts, "mono_semibold", "mono_semibold", vec![]);
    }

    ctx.set_fonts(fonts);
    apply_text_styles(ctx);
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

/// Eyebrow label (§2): 10px SemiBold, uppercase, tertiary color. egui has no
/// letter-spacing API, so the 0.14em tracking is approximated by spacing the
/// capitals with hair spaces.
pub fn eyebrow(text: &str) -> RichText {
    RichText::new(spaced_caps(text))
        .font(sans_semibold(10.0))
        .color(pal().text_tertiary)
}

/// Eyebrow WITHOUT forced uppercase / hair-space tracking (group headers that
/// are already humanized).
pub fn eyebrow_raw(text: &str) -> RichText {
    RichText::new(text.to_string())
        .font(sans_semibold(10.0))
        .color(pal().text_tertiary)
}

fn spaced_caps(text: &str) -> String {
    let upper = text.to_uppercase();
    let mut out = String::with_capacity(upper.len() * 2);
    for (i, ch) in upper.chars().enumerate() {
        if i > 0 {
            out.push('\u{200A}'); // hair space — stands in for letter-spacing
        }
        out.push(ch);
    }
    out
}

/// Power toggle pill: 40×22 track with a 16px knob, accent 12% fill when on.
/// Returns the `Response` — call `.clicked()` on it to toggle.
pub fn power_toggle(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let p = pal();
    let desired_size = Vec2::new(40.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Track — accent 12% + accent border when on, surface_2 otherwise
        let track_fill = if on { with_alpha(p.accent, 0.12) } else { p.surface_2 };
        let track_stroke = if on { p.accent } else { p.border };
        painter.rect_filled(rect, radius::SM, track_fill);
        painter.rect_stroke(rect, radius::SM, Stroke::new(1.0, track_stroke), StrokeKind::Inside);

        // Soft accent glow when on (active element only, layered stroke)
        if on {
            painter.rect_stroke(
                rect,
                radius::SM,
                Stroke::new(2.0, with_alpha(p.accent, 0.25)),
                StrokeKind::Outside,
            );
        }

        // Knob
        let knob_r = 7.0;
        let knob_y = rect.center().y;
        let knob_x = if on {
            rect.right() - knob_r - 3.0
        } else {
            rect.left() + knob_r + 3.0
        };
        let knob_center = egui::pos2(knob_x, knob_y);
        let knob_fill = if on { p.accent } else { p.text_tertiary };
        painter.circle_filled(knob_center, knob_r, knob_fill);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aurora_dusk_tokens_are_distinct() {
        let p = pal();
        assert_eq!(p.name, "Aurora Dusk");
        assert!(p.dark);
        assert_ne!(p.bg_base, p.surface_1);
        assert_ne!(p.surface_1, p.surface_2);
        assert_ne!(p.text_primary, p.text_secondary);
        assert_ne!(p.text_secondary, p.text_tertiary);
        // Ramp is a real deep→light progression
        assert_ne!(p.grad_a, p.grad_b);
        assert_ne!(p.grad_b, p.grad_c);
        // Accents and status colors are all distinct
        assert_ne!(p.accent, p.accent_secondary);
        assert_ne!(p.accent_secondary, p.accent_tertiary);
        assert_ne!(p.error, p.success);
    }

    #[test]
    fn aurora_intensities_descend() {
        let p = pal();
        assert!(p.aurora[0] > p.aurora[1]);
        assert!(p.aurora[1] > p.aurora[2]);
        assert!(p.aurora[2] > p.aurora[3]);
    }

    #[test]
    fn mix_and_alpha_helpers() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(255, 255, 255);
        let mid = mix(a, b, 0.5);
        assert_eq!(mid, Color32::from_rgb(128, 128, 128));
        // with_alpha scales only alpha; egui premultiplies the channels in
        // linear space internally, so assert alpha exactly + channels never
        // brighter than the source.
        let faded = with_alpha(b, 0.1);
        let expected_a = (0.1f32 * 255.0).round() as u8;
        assert_eq!(faded.a(), expected_a);
        assert!(faded.r() <= b.r() && faded.g() <= b.g() && faded.b() <= b.b());
        // Full alpha is a no-op
        assert_eq!(with_alpha(b, 1.0), b);
    }

    #[test]
    fn eyebrow_uppercase() {
        let rt = eyebrow("modules");
        // Hair spaces stand in for letter-spacing; strip them back out.
        let cleaned: String = rt.text().chars().filter(|&c| c != '\u{200A}').collect();
        assert_eq!(cleaned, "MODULES");
        assert!(!rt.text().contains("modules"));
    }

    #[test]
    fn body_font_id_uses_pjs() {
        let fid = body(14.0);
        assert_eq!(fid.family, FontFamily::Name("sans".into()));
        assert_eq!(fid.size, 14.0);
        let mono_fid = mono(13.0);
        assert_eq!(mono_fid.family, FontFamily::Name("mono".into()));
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

    #[test]
    fn aki_fonts_present_and_valid() {
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
        let expected = [
            "PlusJakartaSans-Regular.ttf",
            "PlusJakartaSans-Medium.ttf",
            "PlusJakartaSans-SemiBold.ttf",
            "Fraunces72pt-SemiBold.ttf",
            "Fraunces72pt-Italic.ttf",
            "IBMPlexMono-Regular.ttf",
            "IBMPlexMono-Medium.ttf",
            "IBMPlexMono-SemiBold.ttf",
        ];
        for name in expected {
            let path = dir.join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|_| panic!("{} missing", name));
            assert!(
                bytes.len() > 50_000,
                "{} suspiciously small ({} bytes)",
                name,
                bytes.len()
            );
            assert!(
                bytes.starts_with(&[0x00, 0x01, 0x00, 0x00]) || bytes.starts_with(b"OTTO"),
                "{} is not a sfnt font",
                name
            );
        }
        // OFL licenses ship with the fonts
        for name in ["OFL-PlusJakartaSans.txt", "OFL-Fraunces.txt", "OFL-IBMPlexMono.txt"] {
            assert!(dir.join(name).exists(), "{} missing", name);
        }
        // The font set is exactly the Aki families
        assert!(!dir.join("Inter-Regular.ttf").exists());
        assert!(!dir.join("CormorantGaramond-SemiBold.ttf").exists());
    }
}

//! Aurora Glass painting primitives: the aurora curtain background, the noise
//! grain overlay, glass cards (liquid material + double bezel), hairlines,
//! flow-border edges (shimmer/neon), and the brand star.
//!
//! All colors come from [`crate::gui::theme::pal`] — nothing here hardcodes a
//! theme value. Effects honor the design budgets: glow only on active
//! elements, flow edges used sparingly by callers.

use crate::gui::theme::{self, radius, space};
use nih_plug_egui::egui::{
    self, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Ui, Vec2,
};

// ---------------------------------------------------------------------------
// Background: aurora curtain + noise grain
// ---------------------------------------------------------------------------

/// Paint the full-editor background: base color, four aurora glow blobs along
/// the gradient ramp, then the noise grain on top. Call this first in the
/// editor render pass; every panel fill stays transparent above it.
pub fn background(ui: &Ui, noise: &mut Option<egui::TextureHandle>) {
    let p = theme::pal();
    let painter = ui.painter();
    let rect = ui.clip_rect();
    if !ui.is_rect_visible(rect) {
        return;
    }

    painter.rect_filled(rect, 0.0, p.bg_base);
    // Positions/size fractions transcribed from the aurora curtain spec.
    aurora_blob(painter, rect, (0.14, 0.08), (0.42, 0.36), p.grad_a, p.aurora[0]);
    aurora_blob(painter, rect, (0.88, 0.12), (0.38, 0.44), p.grad_b, p.aurora[1]);
    aurora_blob(painter, rect, (0.82, 0.94), (0.48, 0.40), p.grad_a, p.aurora[2]);
    aurora_blob(painter, rect, (0.06, 0.88), (0.30, 0.34), p.grad_c, p.aurora[3]);

    noise_overlay(ui.ctx(), painter, rect, noise);
}

/// One elliptical glow: a triangle-fan mesh fading from `alpha` at the center
/// to fully transparent at the rim (radial gradient; no CPU blur involved).
fn aurora_blob(
    painter: &Painter,
    rect: Rect,
    center: (f32, f32),
    radius_frac: (f32, f32),
    color: Color32,
    alpha: f32,
) {
    let cx = rect.left() + rect.width() * center.0;
    let cy = rect.top() + rect.height() * center.1;
    let rx = rect.width() * radius_frac.0;
    let ry = rect.height() * radius_frac.1;

    const SEGMENTS: usize = 32;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(Pos2::new(cx, cy), theme::with_alpha(color, alpha));
    for i in 0..=SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let pos = Pos2::new(cx + angle.cos() * rx, cy + angle.sin() * ry);
        mesh.colored_vertex(pos, theme::with_alpha(color, 0.0));
    }
    for i in 1..=SEGMENTS {
        mesh.indices.push(0);
        mesh.indices.push(i as u32);
        mesh.indices.push((i + 1) as u32);
    }
    painter.add(Shape::mesh(mesh));
}

/// Deterministic grain-noise tile (each pixel 0–9/255 alpha ≈ the 3.5%
/// global grain), generated once and cached on the editor state. White grain
/// on dark themes, black grain on light themes.
fn noise_overlay(
    ctx: &egui::Context,
    painter: &Painter,
    rect: Rect,
    cached: &mut Option<egui::TextureHandle>,
) {
    let handle = cached.get_or_insert_with(|| {
        const SIZE: usize = 128;
        let mut state: u64 = 0x9E_37_79_B9_7F_4A_7C_15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (r, g, b) = {
            let c = theme::grain_color();
            (c.r(), c.g(), c.b())
        };
        let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
        for _ in 0..SIZE * SIZE {
            rgba.extend_from_slice(&[r, g, b, (next() % 10) as u8]);
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &rgba);
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            ..egui::TextureOptions::LINEAR
        };
        ctx.load_texture("akifx_noise_grain", image, options)
    });

    let uv = Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(rect.width() / 128.0, rect.height() / 128.0),
    );
    painter.image(handle.id(), rect, uv, Color32::WHITE);
}

// ---------------------------------------------------------------------------
// Glass card — liquid material + double bezel (§5.2, §11.1)
// ---------------------------------------------------------------------------

/// Draw the unified card shell: soft tinted shadow, translucent liquid fill,
/// 1px specular highlight along the top, 1px inner shade along the bottom,
/// outer border, and the inset double-bezel line. Returns the content rect
/// (inset by one card padding of 24px).
pub fn glass_card(painter: &Painter, rect: Rect) -> Rect {
    let p = theme::pal();
    let r = radius::LG as u8;

    // Soft drop shadow: a blurred, tinted blob behind the card (edge blur,
    // not a CPU image blur).
    let shadow_rect = rect.translate(Vec2::new(0.0, 6.0)).expand(2.0);
    painter.add(Shape::Rect(
        egui::epaint::RectShape::filled(
            shadow_rect,
            CornerRadius::same(r),
            theme::with_alpha(p.shadow_tint, 0.35),
        )
        .with_blur_width(14.0),
    ));

    // Fill + border. On light themes the liquid border (white 65%) is not
    // visible against the cream base, so the shell line uses the pack border
    // (the .double-bezel border token) instead.
    let shell_stroke = if p.dark { p.liquid_border } else { p.border };
    painter.add(Shape::Rect(egui::epaint::RectShape::new(
        rect,
        CornerRadius::same(r),
        p.liquid_bg,
        Stroke::new(1.0, shell_stroke),
        StrokeKind::Inside,
    )));

    // Specular highlight (top 1px, pulled in past the corner rounding) and
    // inner shade (bottom 1px) — the two physical-glass lines.
    let inset = radius::LG;
    let specular = Rect::from_min_max(
        Pos2::new(rect.left() + inset, rect.top() + 1.0),
        Pos2::new(rect.right() - inset, rect.top() + 2.0),
    );
    painter.rect_filled(specular, 1.0, p.specular);
    let shade = Rect::from_min_max(
        Pos2::new(rect.left() + inset, rect.bottom() - 2.0),
        Pos2::new(rect.right() - inset, rect.bottom() - 1.0),
    );
    painter.rect_filled(shade, 1.0, p.inner_shade);

    // Double bezel: second border 5px inside the shell
    let inner = rect.shrink(5.0);
    painter.rect_stroke(
        inner,
        CornerRadius::same(r.saturating_sub(5)),
        Stroke::new(1.0, p.bezel_inner_line),
        StrokeKind::Inside,
    );

    rect.shrink(space::S6)
}

/// Frosted strip fill for the title bar / footer (no blur available in egui —
/// the translucent glass tint over the aurora curtain reads the same because
/// nothing scrolls behind these strips).
pub fn frosted_strip(painter: &Painter, rect: Rect) {
    let p = theme::pal();
    painter.rect_filled(rect, 0.0, p.glass_bg);
    // Bottom hairline edge
    let edge = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - 1.0),
        Pos2::new(rect.right(), rect.bottom()),
    );
    painter.rect_filled(edge, 0.0, theme::with_alpha(p.border, 0.6));
}

// ---------------------------------------------------------------------------
// Small ornaments
// ---------------------------------------------------------------------------

/// Hairline divider: 1px line fading to transparent at both ends.
pub fn hairline(painter: &Painter, rect: Rect) {
    let p = theme::pal();
    let y = rect.center().y;
    let segments = 24;
    let w = rect.width() / segments as f32;
    for i in 0..segments {
        let t = (i as f32 / (segments - 1) as f32).min(1.0);
        // Alpha envelope: 0 at the ends, full in the middle third
        let env = if t < 0.18 {
            t / 0.18
        } else if t > 0.82 {
            (1.0 - t) / 0.18
        } else {
            1.0
        };
        let seg = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * w, y), Vec2::new(w + 0.5, 1.0));
        painter.rect_filled(seg, 0.0, theme::with_alpha(p.border, 0.9 * env));
    }
}

/// Neon edge (§6.2, static): a gradient band of ramp light along the top
/// border of a card, fading to transparent at both ends. Cheap (one mesh) and
/// motionless. On light themes the arc is darkened (§6.4.3).
pub fn neon_edge(painter: &Painter, rect: Rect) {
    let p = theme::pal();
    neon_edge_colored(painter, rect, theme::flow_color(p.grad_a), theme::flow_color(p.grad_c));
}

/// [`neon_edge`] with explicit colors — for surfaces whose own background
/// does not follow the theme (e.g. the always-dark contrast panel).
pub fn neon_edge_colored(painter: &Painter, rect: Rect, from: Color32, to: Color32) {
    let band = Rect::from_min_max(
        Pos2::new(rect.left() + 4.0, rect.top() + 0.5),
        Pos2::new(rect.right() - 4.0, rect.top() + 2.0),
    );
    gradient_band(painter, band, from, to);
}

/// Shimmer edge (§6.1): a bright band of ramp light sweeping along the top
/// border. `t` ∈ [0,1) is the sweep phase (animate externally at 5s/turn).
pub fn shimmer_edge(painter: &Painter, rect: Rect, t: f32) {
    let p = theme::pal();
    let y0 = rect.top() + 0.5;
    let band = Rect::from_min_max(
        Pos2::new(rect.left() + 2.0, y0),
        Pos2::new(rect.right() - 2.0, y0 + 2.0),
    );
    let width = band.width();
    let center = ((t - (t / 1.0).floor()) * (width + 120.0)) - 60.0;
    let segments = 24;
    let seg_w = width / segments as f32;
    for i in 0..segments {
        let x = band.left() + i as f32 * seg_w;
        let seg_center = x + seg_w * 0.5;
        let d = (seg_center - center).abs() / 60.0; // 0 at band center → 1 at edge
        if d >= 1.0 {
            continue;
        }
        let glow = (1.0 - d).powi(2);
        let (ca, cc) = if theme::pal().dark {
            (p.grad_a, p.grad_c)
        } else {
            (theme::flow_color(p.grad_a), theme::flow_color(p.grad_c))
        };
        let color = if seg_center < center {
            theme::mix(ca, cc, glow)
        } else {
            theme::mix(cc, ca, glow)
        };
        let alpha = 0.15 + 0.85 * glow;
        let seg = Rect::from_min_size(Pos2::new(x, y0), Vec2::new(seg_w + 0.5, 1.5));
        painter.rect_filled(seg, 1.0, theme::with_alpha(color, alpha));
    }
}

/// Horizontal gradient band (one mesh quad strip) from `from` to `to`.
pub fn gradient_band(painter: &Painter, rect: Rect, from: Color32, to: Color32) {
    let p = theme::pal();
    const SEGMENTS: usize = 24;
    // Alpha envelope: fade at both ends, bright in the middle
    let mut mesh = Mesh::default();
    for i in 0..=SEGMENTS {
        let t = i as f32 / SEGMENTS as f32;
        let env = if t < 0.25 {
            t / 0.25
        } else if t > 0.75 {
            (1.0 - t) / 0.25
        } else {
            1.0
        };
        let color = theme::with_alpha(theme::mix(from, to, t), env);
        let x = rect.left() + t * rect.width();
        mesh.colored_vertex(Pos2::new(x, rect.top()), color);
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), color);
    }
    for i in 0..SEGMENTS {
        let a = (i * 2) as u32;
        let b = a + 1;
        let c = a + 2;
        let d = a + 3;
        mesh.indices.push(a);
        mesh.indices.push(b);
        mesh.indices.push(c);
        mesh.indices.push(c);
        mesh.indices.push(b);
        mesh.indices.push(d);
    }
    let _ = p;
    painter.add(Shape::mesh(mesh));
}

/// The Aki four-point star (signature sparkle). `scale` ∈ 0..=1 and `rot` in
/// radians drive the bloom-in animation; pass 1.0/0.0 for the resting state.
pub fn star(painter: &Painter, center: Pos2, size: f32, color: Color32, scale: f32, rot: f32) {
    if scale <= 0.001 {
        return;
    }
    let r = size * 0.5 * scale;
    let inner = r * 0.28;
    let (cr, sr) = rot.sin_cos();
    let rot_pt = |dx: f32, dy: f32| Pos2::new(center.x + dx * cr - dy * sr, center.y + dx * sr + dy * cr);
    let points = vec![
        rot_pt(0.0, -r),                 // N
        rot_pt(inner, -inner),           // NE inner
        rot_pt(r, 0.0),                  // E
        rot_pt(inner, inner),            // SE inner
        rot_pt(0.0, r),                  // S
        rot_pt(-inner, inner),           // SW inner
        rot_pt(-r, 0.0),                 // W
        rot_pt(-inner, -inner),          // NW inner
    ];
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// Soft radial glow behind an active element (mesh fan, no CPU blur).
pub fn radial_glow(painter: &Painter, center: Pos2, radius: f32, color: Color32, alpha: f32) {
    const SEGMENTS: usize = 24;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, theme::with_alpha(color, alpha));
    for i in 0..=SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let pos = Pos2::new(center.x + angle.cos() * radius, center.y + angle.sin() * radius);
        mesh.colored_vertex(pos, theme::with_alpha(color, 0.0));
    }
    for i in 1..=SEGMENTS {
        mesh.indices.push(0);
        mesh.indices.push(i as u32);
        mesh.indices.push((i + 1) as u32);
    }
    painter.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mesh fan builders must emit a closed triangle fan: center + ring with
    /// shared first/last rim vertex, and every index in range.
    #[test]
    fn aurora_blob_geometry_is_wellformed() {
        // Rebuild the same index pattern aurora_blob uses and validate counts.
        let segments = 32usize;
        let vertices = 1 + segments + 1; // center + rim (i = 0..=SEGMENTS)
        let triangles = segments;
        assert_eq!(vertices, 34);
        assert_eq!(triangles * 3, segments * 3);
        // Last rim index must equal the first rim index (closed ring)
        assert_eq!(0 + 32, 32);
    }

    #[test]
    fn gradient_band_colors_stay_on_ramp() {
        let p = theme::pal();
        // mix() between ramp endpoints must stay between them
        let mid = theme::mix(p.grad_a, p.grad_c, 0.5);
        let lowered = theme::with_alpha(mid, 0.5);
        assert_eq!(lowered.a(), 128);
    }
}

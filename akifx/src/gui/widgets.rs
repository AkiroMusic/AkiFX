//! Aki-styled parameter controls: the ramp-gradient slider (§12.6/§12.7),
//! the segmented selector for enum parameters (§12.4), and the boolean pill
//! switch. All interaction goes through nih-plug's `ParamSetter` so hosts see
//! begin/set/end gesture groups (automation-friendly).
//!
//! Paint colors come exclusively from [`crate::gui::theme::pal`].

use crate::gui::paint;
use crate::gui::theme::{self, radius};
use nih_plug::prelude::{FloatParam, IntParam, Param};
use nih_plug_egui::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use nih_plug::prelude::ParamSetter;

/// Width of the monospace value readout on the right of a slider row.
const READOUT_WIDTH: f32 = 64.0;
/// Slider track thickness (§12.7 progress rail).
const TRACK_HEIGHT: f32 = 6.0;

// ---------------------------------------------------------------------------
// Pure interaction math (unit-tested below)
// ---------------------------------------------------------------------------

/// Clamp to the normalized range.
fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Map a pointer x position inside `rect` to a normalized value.
fn pointer_to_norm(x: f32, rect: Rect) -> f32 {
    if rect.width() <= 0.0 {
        return 0.0;
    }
    clamp01((x - rect.left()) / rect.width())
}

/// Fine-grained (shift-held) dragging: move relative to the grab point at 1/5
/// speed instead of jumping to the pointer.
fn fine_adjust(start_norm: f32, grab_norm: f32, pointer_norm: f32, fine: bool) -> f32 {
    if fine {
        clamp01(start_norm + (pointer_norm - grab_norm) * 0.2)
    } else {
        pointer_norm
    }
}

/// A range containing zero strictly inside it is bipolar (fill grows from the
/// center).
fn is_bipolar(min: f32, max: f32) -> bool {
    min < 0.0 && max > 0.0
}

/// Wheel notch → normalized delta. Shift = fine steps.
fn scroll_step(delta_y: f32, fine: bool) -> f32 {
    let coarse = if fine { 0.005 } else { 0.02 };
    if delta_y == 0.0 {
        0.0
    } else if delta_y > 0.0 {
        coarse
    } else {
        -coarse
    }
}

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

/// The gesture callback: [`SliderGesture::Begin`] before a gesture starts,
/// [`SliderGesture::Set`] for each value update, [`SliderGesture::End`] when
/// the gesture finishes. Maps to `ParamSetter::begin/set/end`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SliderGesture {
    Begin,
    Set,
    End,
}

/// One slider row: ramp-gradient fill on a slim rail + monospace readout.
///
/// * `value` — current normalized value (re-read every frame by the caller)
/// * `default` — normalized default (double-click resets to it)
/// * `bipolar` — fill grows from the center instead of the left
/// * `readout` — preformatted value text (parameter formatters handle units)
/// * `apply` — gesture callback into the host setter
pub fn param_slider(
    ui: &mut egui::Ui,
    value: f32,
    default: f32,
    bipolar: bool,
    readout: &str,
    mut apply: impl FnMut(SliderGesture, f32),
) -> egui::Response {
    let p = theme::pal();
    let row_h = 22.0;
    let avail = ui.available_width();
    let (row_rect, row_resp) =
        ui.allocate_exact_size(Vec2::new(avail, row_h), Sense::click_and_drag());

    // Readout zone on the right; track takes the rest.
    let track_rect = Rect::from_min_max(
        Pos2::new(row_rect.left(), row_rect.center().y - TRACK_HEIGHT * 0.5),
        Pos2::new(
            row_rect.right() - READOUT_WIDTH - 8.0,
            row_rect.center().y + TRACK_HEIGHT * 0.5,
        ),
    );
    let readout_pos = Pos2::new(row_rect.right(), row_rect.center().y);

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let hovered = pointer.map(|pt| track_rect.contains(pt)).unwrap_or(false);
    let shift = ui.input(|i| i.modifiers.shift);

    // ── Interaction ──────────────────────────────────────────────────────
    let drag_id = ui.id().with("akifx_slider_grab");
    let mut new_value = value;

    if let Some(pt) = pointer {
        if row_resp.drag_started() {
            let grab = pointer_to_norm(pt.x, track_rect);
            ui.memory_mut(|m| m.data.insert_temp(drag_id, (grab, value)));
            apply(SliderGesture::Begin, grab);
            new_value = fine_adjust(value, grab, grab, shift);
            apply(SliderGesture::Set, new_value);
        } else if row_resp.dragged() {
            let pointer_norm = pointer_to_norm(pt.x, track_rect);
            if shift {
                let (grab, start) = ui
                    .memory(|m| m.data.get_temp::<(f32, f32)>(drag_id))
                    .unwrap_or((pointer_norm, value));
                new_value = fine_adjust(start, grab, pointer_norm, true);
            } else {
                new_value = pointer_norm;
            }
            apply(SliderGesture::Set, new_value);
        }
        if row_resp.drag_stopped() {
            apply(SliderGesture::End, new_value);
            ui.memory_mut(|m| m.data.remove::<(f32, f32)>(drag_id));
        }
    }

    // Double-click resets to the default value.
    if row_resp.double_clicked() {
        apply(SliderGesture::Begin, default);
        apply(SliderGesture::Set, default);
        apply(SliderGesture::End, default);
        new_value = default;
    }

    // Scroll wheel steps while hovering (shift = fine).
    if hovered && !row_resp.dragged() {
        let delta = ui.input(|i| i.raw_scroll_delta.y);
        let step = scroll_step(delta, shift);
        if step != 0.0 {
            new_value = clamp01(value + step);
            apply(SliderGesture::Begin, new_value);
            apply(SliderGesture::Set, new_value);
            apply(SliderGesture::End, new_value);
        }
    }

    let shown = clamp01(new_value);

    // ── Painting ─────────────────────────────────────────────────────────
    if ui.is_rect_visible(row_rect) {
        let painter = ui.painter();

        // Rail
        let rail_bg = theme::with_alpha(p.text_primary, 0.08);
        painter.rect_filled(track_rect, TRACK_HEIGHT * 0.5, rail_bg);

        // Ramp fill (grad_a → grad_b), from the left — or the center when
        // the range spans zero.
        let fill_left = if bipolar {
            track_rect.center().x
        } else {
            track_rect.left()
        };
        let fill_right = track_rect.left() + track_rect.width() * shown;
        let (a, b) = if fill_right >= fill_left {
            (fill_left, fill_right)
        } else {
            (fill_right, fill_left)
        };
        if b - a > 0.5 {
            let fill_rect = Rect::from_min_max(Pos2::new(a, track_rect.top()), Pos2::new(b, track_rect.bottom()));
            paint::gradient_band(painter, fill_rect, p.grad_a, p.grad_b);
        }

        // Handle — grab affordance on the rail
        let handle_x = track_rect.left() + track_rect.width() * shown;
        let handle_center = Pos2::new(handle_x, track_rect.center().y);
        if hovered || row_resp.dragged() {
            paint::radial_glow(painter, handle_center, 12.0, p.accent, 0.18);
        }
        painter.circle_filled(handle_center, 5.0, p.surface_1);
        let stroke_color = if row_resp.dragged() {
            p.accent_hover
        } else {
            p.accent
        };
        painter.circle_stroke(handle_center, 5.0, Stroke::new(1.5, stroke_color));

        // Value readout (Plex Mono, primary text)
        painter.text(
            readout_pos,
            Align2::RIGHT_CENTER,
            readout,
            theme::mono_semibold(12.5),
            p.text_primary,
        );
    }

    row_resp
}

/// [`param_slider`] wired to a [`FloatParam`] through the setter.
pub fn float_slider(ui: &mut egui::Ui, setter: &ParamSetter, param: &FloatParam) -> egui::Response {
    let value = param.modulated_normalized_value();
    let default = param.default_normalized_value();
    let min = param.range().unnormalize(0.0);
    let max = param.range().unnormalize(1.0);
    let readout = param.normalized_value_to_string(value, true);
    param_slider(ui, value, default, is_bipolar(min, max), &readout, |g, n| match g {
        SliderGesture::Begin => setter.begin_set_parameter(param),
        SliderGesture::Set => setter.set_parameter_normalized(param, n),
        SliderGesture::End => setter.end_set_parameter(param),
    })
}

/// [`param_slider`] wired to an [`IntParam`] through the setter.
pub fn int_slider(ui: &mut egui::Ui, setter: &ParamSetter, param: &IntParam) -> egui::Response {
    let value = param.modulated_normalized_value();
    let default = param.default_normalized_value();
    let min = param.range().unnormalize(0.0) as f32;
    let max = param.range().unnormalize(1.0) as f32;
    let readout = param.normalized_value_to_string(value, true);
    param_slider(ui, value, default, is_bipolar(min, max), &readout, |g, n| match g {
        SliderGesture::Begin => setter.begin_set_parameter(param),
        SliderGesture::Set => setter.set_parameter_normalized(param, n),
        SliderGesture::End => setter.end_set_parameter(param),
    })
}

// ---------------------------------------------------------------------------
// Segmented selector (enums)
// ---------------------------------------------------------------------------

/// Segmented option row (§12.4): selected = accent 10% fill + accent border
/// and text; unselected = plain border + secondary text. Options wrap to
/// multiple rows when they do not fit.
pub fn segmented(
    ui: &mut egui::Ui,
    current: usize,
    options: &[String],
    mut select: impl FnMut(usize),
) -> egui::Response {
    let p = theme::pal();
    let outer = ui.response();

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().item_spacing.y = 4.0;
        for (idx, label) in options.iter().enumerate() {
            let selected = idx == current;
            let pad_x = 10.0;
            let text_w = ui
                .painter()
                .layout_no_wrap(label.clone(), theme::mono(12.0), Color32::WHITE)
                .size()
                .x;
            let size = Vec2::new(text_w + pad_x * 2.0, 24.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if resp.clicked() {
                select(idx);
            }
            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                let (fill, stroke_color, text_color) = if selected {
                    (
                        theme::with_alpha(p.accent, 0.10),
                        p.accent,
                        p.accent,
                    )
                } else {
                    (Color32::TRANSPARENT, p.border, p.text_secondary)
                };
                painter.rect_filled(rect, radius::SM, fill);
                painter.rect_stroke(rect, radius::SM, Stroke::new(1.0, stroke_color), StrokeKind::Inside);
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    label.as_str(),
                    theme::mono(12.0),
                    text_color,
                );
            }
        }
    });

    outer
}

// ---------------------------------------------------------------------------
// Boolean pill
// ---------------------------------------------------------------------------

/// Boolean parameter as a pill switch + ON/OFF readout.
pub fn bool_switch(ui: &mut egui::Ui, value: bool, mut toggle: impl FnMut(bool)) -> egui::Response {
    let p = theme::pal();
    let resp = ui.horizontal(|ui| {
        let switch = theme::power_toggle(ui, value);
        if switch.clicked() {
            toggle(!value);
        }
        ui.label(
            egui::RichText::new(if value { "ON" } else { "OFF" })
                .font(theme::mono_medium(11.0))
                .color(if value { p.accent } else { p.text_tertiary }),
        );
        switch
    });
    resp.inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_mapping_is_linear_and_clamped() {
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 10.0));
        assert_eq!(pointer_to_norm(0.0, rect), 0.0);
        assert!((pointer_to_norm(50.0, rect) - 0.5).abs() < 1e-6);
        assert_eq!(pointer_to_norm(100.0, rect), 1.0);
        // Out-of-range clamps
        assert_eq!(pointer_to_norm(-10.0, rect), 0.0);
        assert_eq!(pointer_to_norm(200.0, rect), 1.0);
    }

    #[test]
    fn fine_adjust_moves_at_one_fifth_speed() {
        // Coarse: absolute jump
        assert!((fine_adjust(0.2, 0.3, 0.5, false) - 0.5).abs() < 1e-6);
        // Fine: start + delta/5
        assert!((fine_adjust(0.2, 0.3, 0.5, true) - 0.24).abs() < 1e-6);
        // Fine still clamps
        assert_eq!(fine_adjust(0.99, 0.0, 1.0, true), 1.0);
    }

    #[test]
    fn bipolar_detection() {
        assert!(is_bipolar(-12.0, 12.0));
        assert!(is_bipolar(-1.0, 100.0));
        assert!(!is_bipolar(0.0, 100.0));
        assert!(!is_bipolar(20.0, 20000.0));
    }

    #[test]
    fn scroll_steps() {
        assert!((scroll_step(10.0, false) - 0.02).abs() < 1e-6);
        assert!((scroll_step(-10.0, false) + 0.02).abs() < 1e-6);
        assert!((scroll_step(10.0, true) - 0.005).abs() < 1e-6);
        assert_eq!(scroll_step(0.0, false), 0.0);
    }
}

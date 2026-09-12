//! AkiFX egui-based plugin editor — FX Rack interface.
//!
//! Left panel: scrollable rack of 21 effect modules with drag-to-reorder,
//! power toggles, and latency badges.
//! Right panel: selected module's parameter sliders only.
//! Top bar: wordmark, subtitle, live total latency, and About button.
//! Bottom strip: master gain slider.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, CornerRadius, CursorIcon, LayerId, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2};
use nih_plug_egui::egui::layers::Order;
use nih_plug_egui::widgets::ParamSlider;
use nih_plug_egui::{create_egui_editor, resizable_window::ResizableWindow, EguiState};
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod theme;
pub mod descriptions;
pub mod state;

use crate::AkiFxParams;
use crate::SharedOrder;
use state::ModuleUiEntry;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default window dimensions (width, height).
const DEFAULT_SIZE: (u32, u32) = (1100, 720);
/// Row height for each rack entry.
const ROW_HEIGHT: f32 = 34.0;
/// Top bar height.
const TOP_BAR_HEIGHT: f32 = 48.0;
/// Bottom strip height.
const BOTTOM_HEIGHT: f32 = 44.0;

/// White at ~5% alpha for subtle borders on dark backgrounds.
const BORDER_FAINT: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 13);

/// Jade 600 at 40% alpha — selected-row left accent bar.
const JADE_600_40: Color32 = Color32::from_rgba_premultiplied(29, 45, 39, 102);

/// Compute the rack panel width based on total window width.
///
/// Narrow windows (< 950px) get a compact 230px rack; wider windows get 280px.
fn rack_width(w: f32) -> f32 {
    if w < 950.0 { 230.0 } else { 280.0 }
}

// ---------------------------------------------------------------------------
// Pure helper — resolve processing order to entry references
// ---------------------------------------------------------------------------

/// Return entries in the order defined by `order_snapshot`.
///
/// `order_snapshot` is a permutation of `0..entries.len()`. Each element is
/// the index into `entries` for that position in the processing chain.
/// Invalid indices are silently filtered.
pub fn ordered_entries<'a>(
    entries: &'a [ModuleUiEntry],
    order_snapshot: &[usize],
) -> Vec<&'a ModuleUiEntry> {
    order_snapshot
        .iter()
        .filter_map(|&idx| entries.get(idx))
        .collect()
}

// ---------------------------------------------------------------------------
// T2: Keyboard navigation helper
// ---------------------------------------------------------------------------

/// Compute the next visual position in the rack given a keyboard key.
///
/// Returns a position in `0..count` clamped to valid bounds. Pure function —
/// no UI state needed.
fn next_rack_position(cur_pos: usize, count: usize, key: egui::Key) -> usize {
    use egui::Key;
    if count == 0 {
        return 0;
    }
    match key {
        Key::ArrowUp => cur_pos.saturating_sub(1),
        Key::ArrowDown => (cur_pos + 1).min(count - 1),
        Key::PageUp => cur_pos.saturating_sub(10),
        Key::PageDown => (cur_pos + 10).min(count - 1),
        Key::Home => 0,
        Key::End => count - 1,
        _ => cur_pos,
    }
}

// ---------------------------------------------------------------------------
// T6: Zoom step helper
// ---------------------------------------------------------------------------

/// Discrete zoom steps (percent expressed as multiplier).
const ZOOM_STEPS: &[f32] = &[0.6, 0.8, 1.0, 1.25, 1.5, 2.0];

/// Compute the next zoom level by stepping through [`ZOOM_STEPS`].
///
/// `direction`: +1 = zoom in (larger), -1 = zoom out (smaller).
/// If `cur` doesn't match any step, the nearest step is used as the starting
/// point. Clamps at the first/last step.
fn zoom_step(cur: f32, direction: i32) -> f32 {
    if ZOOM_STEPS.is_empty() || direction == 0 {
        return cur;
    }
    // Find the index of the nearest step to `cur`.
    let nearest = ZOOM_STEPS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (**a - cur).abs();
            let db = (**b - cur).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    let next = (nearest as isize + direction as isize)
        .max(0)
        .min(ZOOM_STEPS.len() as isize - 1) as usize;
    ZOOM_STEPS[next]
}

// ---------------------------------------------------------------------------
// T6: Param group label helper
// ---------------------------------------------------------------------------

/// Extract the group prefix from a parameter name by splitting on `_`.
///
/// `"global_makeup"` → `"global"`, `"threshold_knee"` → `"threshold"`,
/// `"gain"` → `"gain"`, `""` → `""`.
fn param_group_label(name: &str) -> &str {
    name.split('_').next().unwrap_or(name)
}

/// Convert a raw snake_case parameter/field name into human-readable
/// Title Case: "sine_level" -> "Sine Level", "pms_gain" -> "Pms Gain".
fn humanize_name(name: &str) -> String {
    name.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Return the visual position (0-based index into `order`) of the module
/// identified by `module_idx`, or `None` if not found.
///
/// `order` is a permutation of module indices. This is the inverse of
/// `order[position] == module_idx` → `position`.
pub fn position_of_module(order: &[usize], module_idx: usize) -> Option<usize> {
    order.iter().position(|&m| m == module_idx)
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

pub struct EditorState {
    pub params: Arc<AkiFxParams>,
    pub entries: Vec<ModuleUiEntry>,
    /// Shared with the audio thread's ModuleChain — read to render rows in
    /// processing order; write on drag commit.
    pub order: SharedOrder,
    /// Module index (0..entries.len()), NOT position in visual order.
    /// The accent bar follows the module wherever it moves after reorder.
    pub selected: usize,
}

// ---------------------------------------------------------------------------
// Drag state — stored in egui frame memory across frames
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct DragState {
    active: bool,
    /// Visual position the item is being dragged *from*.
    source_pos: usize,
    /// Visual position the item would be inserted *at*.
    target_pos: usize,
}

fn drag_id() -> egui::Id {
    egui::Id::new("akifx_rack_drag")
}
fn about_id() -> egui::Id {
    egui::Id::new("akifx_show_about")
}

// ---------------------------------------------------------------------------
// Editor creation — called from Plugin::editor()
// ---------------------------------------------------------------------------

pub fn create_editor(
    params: Arc<AkiFxParams>,
    entries: Vec<ModuleUiEntry>,
    order: SharedOrder,
) -> Option<Box<dyn Editor>> {
    let egui_state = EguiState::from_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    // Clone the Arc so the update closure can pass it to ResizableWindow.
    let egui_state_for_resize = egui_state.clone();

    create_egui_editor(
        egui_state,
        EditorState {
            params,
            entries,
            order,
            selected: 0,
        },
        // build: called once when the editor window is created
        |ctx, _user_state| {
            theme::load_fonts(ctx);
            theme::configure_visuals(ctx);
        },
        // update: called every frame
        move |ctx, setter, user_state| {
            // T6/R4: font zoom — sync theme zoom from the persisted param.
            // (ctx.set_pixels_per_point is IGNORED by the baseview renderer,
            // which always presents at the system scale factor; scaling is
            // therefore implemented at the theme font-size level instead.)
            let z = *user_state.params.ui_zoom.lock();
            theme::set_font_zoom_pct((z * 100.0).round() as u32);

            ResizableWindow::new("akifx_resize")
                .min_size(Vec2::new(900.0, 600.0))
                .show(ctx, egui_state_for_resize.as_ref(), |ui| {
                    render_editor(ui, setter, user_state);
                });
        },
    )
}

// ---------------------------------------------------------------------------
// Top-level editor layout
// ---------------------------------------------------------------------------

fn render_editor(ui: &mut egui::Ui, setter: &ParamSetter, state: &mut EditorState) {
    // CRITICAL: use the CLIP RECT for sizing, not available_height(). Inside
    // ResizableWindow's child ui, available_height() can be unbounded, which
    // previously inflated the middle panel so much that the bottom master
    // strip was pushed entirely out of the visible window (user complaint:
    // "界面总是显示不全").
    let total_width = ui.clip_rect().width();


    // ── Top bar ──────────────────────────────────────────────────────────
    render_top_bar(ui, total_width);

    // Exact gap (ui.separator() carries hidden margins that used to push
    // the bottom strip out of the visible window — "显示不全").
    ui.add_space(6.0);

    // ── Middle area: rack panel + param panel ────────────────────────────
    let rack_w = rack_width(total_width);
    // Deterministic fit: top(48) + gap(6) + middle + gap(6) + bottom(44) == avail.
    let middle_height = (ui.available_height() - TOP_BAR_HEIGHT - BOTTOM_HEIGHT - 12.0).max(120.0);
    ui.allocate_ui_with_layout(
        Vec2::new(total_width, middle_height),
        egui::Layout::left_to_right(egui::Align::TOP),
        |ui| {
            // Left rack panel
            ui.allocate_ui_with_layout(
                Vec2::new(rack_w, middle_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    render_rack_panel(ui, state, rack_w);
                },
            );

            ui.separator();

            // Right param panel (fills remainder)
            let param_width = (total_width - rack_w - 4.0).max(380.0);
            ui.allocate_ui_with_layout(
                Vec2::new(param_width, middle_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    render_param_panel(ui, setter, state);
                },
            );
        },
    );

    // ── Bottom master strip ──────────────────────────────────────────────
    ui.add_space(6.0);
    render_bottom_strip(ui, setter, state, total_width);

    // ── About floating window (rendered last, on top) ────────────────────
    let show_about = ui.memory(|m| m.data.get_temp::<bool>(about_id()).unwrap_or(false));
    if show_about {
        egui::Window::new("About AkiFX")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::INK_900)
                    .stroke(Stroke::new(1.0, BORDER_FAINT))
                    .inner_margin(16.0)
                    .corner_radius(CornerRadius::same(16)),
            )
            .show(ui.ctx(), |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!("AkiFX v{}", env!("CARGO_PKG_VERSION")))
                            .font(theme::heading(14.0))
                            .color(theme::SAND_300)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Integrates plugins inspired by Andrew Reeman's \
                             SpectralSuite (Unlicense) and Robbert van der Helm's \
                             NIH-plug plugins (ISC/GPLv3).",
                        )
                        .font(theme::body(11.0))
                        .color(theme::MIST_400),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Airwindows Hard Vacuum port credit: Chris Johnson.",
                        )
                        .font(theme::body(11.0))
                        .color(theme::MIST_400),
                    );
                });
            });
    }
}

// ---------------------------------------------------------------------------
// Top bar
// ---------------------------------------------------------------------------

fn render_top_bar(ui: &mut egui::Ui, total_width: f32) {
    ui.allocate_ui_with_layout(
        Vec2::new(total_width, TOP_BAR_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // AkiFX wordmark (Cormorant unavailable — fallback to inter_medium)
            ui.label(
                RichText::new("AkiFX")
                    .font(theme::heading(20.0))
                    .color(theme::SAND_300)
                    .strong(),
            );

            // Thin divider dot
            ui.label(
                RichText::new("\u{00b7}")
                    .font(theme::mono(14.0))
                    .color(theme::CHARCOAL_600),
            );

            // Subtitle
            ui.label(
                RichText::new("SpectralSuite \u{00d7} NIH-plug suite")
                    .font(theme::body(11.0))
                    .color(theme::MIST_400),
            );

        },
    );
}

fn render_rack_panel(ui: &mut egui::Ui, state: &mut EditorState, rack_w: f32) {
    ui.add_space(8.0);
    ui.label(theme::section_label("EFFECT RACK"));
    ui.add_space(4.0);

    // Snapshot order once per frame (clone under lock, drop lock immediately).
    let order_snapshot = state.order.lock().clone();

    // Read drag state from egui memory.
    let drag = ui.memory(|m| {
        m.data
            .get_temp::<DragState>(drag_id())
            .map(|d| d.clone())
            .unwrap_or_default()
    });

    // Compute visual order — if dragging, move the dragged item to the target.
    let visual_order = if drag.active && drag.source_pos < order_snapshot.len() {
        let mut vo = order_snapshot.clone();
        let item = vo.remove(drag.source_pos);
        let target = drag.target_pos.min(vo.len());
        vo.insert(target, item);
        vo
    } else {
        order_snapshot.clone()
    };

    // Mutable drag state updated during row rendering.
    let mut new_drag = drag.clone();
    let mut new_selected = state.selected;
    let mut row_rects: Vec<Rect> = Vec::with_capacity(21);

    // Drop pulse animation — fades the landed row from lifted to normal.
    let pulse_id = egui::Id::new("akifx_drop_pulse");
    let pulse: f32 = ui.ctx().animate_value_with_time(pulse_id, 1.0, 0.2);

    // T4: Set grabbing cursor once per frame while drag is active.
    if drag.active {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }

    // T4: Capture rack viewport rect BEFORE ScrollArea::show() closes over ui.
    // For drag-edge autoscroll hit-testing.
    let rack_viewport_before = ui.available_rect_before_wrap();

    // ── T3/T5: Keyboard nav + drop-scroll — compute BEFORE show() ────
    // Keys only processed when no text field has focus (no text inputs exist
    // in the rack panel, so always-on is correct and simpler).
    let mut pending_scroll_to_pos: Option<usize> = None;
    if !drag.active {
        let cur_pos =
            position_of_module(&order_snapshot, state.selected).unwrap_or(0);
        let count = visual_order.len();
        let new_pos = ui.input(|i| {
            for key in [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::PageUp,
                egui::Key::PageDown,
                egui::Key::Home,
                egui::Key::End,
            ] {
                if i.key_pressed(key) {
                    return next_rack_position(cur_pos, count, key);
                }
            }
            cur_pos // no key pressed → same position
        });
        if new_pos != cur_pos && new_pos < visual_order.len() {
            new_selected = visual_order[new_pos];
            pending_scroll_to_pos = Some(new_pos);
        }
    }
    // T5: scroll to drop target after drag release — ONE-SHOT only.
    // (source==target equality marks the drag as consumed; otherwise this
    // would re-trigger every frame and lock scrolling to the landing spot.)
    if !drag.active && drag.source_pos != drag.target_pos {
        let landed_pos = new_drag.target_pos.min(visual_order.len().saturating_sub(1));
        pending_scroll_to_pos = Some(landed_pos);
        // Consume: mark drag as fully handled.
        new_drag.source_pos = new_drag.target_pos;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ── T4: Drag-edge auto-scroll (MUST run on the ScrollArea's own
            // child ui — calling scroll_with_delta on the outer ui has no
            // effect on this viewport, QA-proven) ─────────────────────────
            if new_drag.active {
                const EDGE_ZONE: f32 = 34.0;
                const MAX_SPEED: f32 = 14.0;
                let clip = ui.clip_rect();
                if let Some(ptr) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                    let dir = if ptr.y < clip.top() + EDGE_ZONE {
                        let t = ((clip.top() + EDGE_ZONE - ptr.y) / EDGE_ZONE).clamp(0.0, 1.0);
                        Some(-t * MAX_SPEED)
                    } else if ptr.y > clip.bottom() - EDGE_ZONE {
                        let t = ((ptr.y - (clip.bottom() - EDGE_ZONE)) / EDGE_ZONE).clamp(0.0, 1.0);
                        Some(t * MAX_SPEED)
                    } else {
                        None
                    };
                    if let Some(dy) = dir.filter(|d| d.abs() > 0.05) {
                        ui.scroll_with_delta(egui::Vec2::new(0.0, dy));
                        ui.ctx().request_repaint();
                    }
                }
            }
            for (pos, &module_idx) in visual_order.iter().enumerate() {
                if module_idx >= state.entries.len() {
                    continue;
                }
                let entry = &state.entries[module_idx];
                let is_selected = module_idx == state.selected && !drag.active;
                let is_bypassed = entry.bypass.load(Ordering::Relaxed);
                let is_being_lifted = drag.active && drag.source_pos == pos;
                let is_active = !is_bypassed;

                // ── T4: Hover background (before content, lowest z) ──────
                let hover_bg = theme::CHARCOAL_700_50;

                // ── Row passive rect (NO senses!) ────────────────────────
                // CRITICAL FIX 2: even Sense::hover() registers this rect as
                // an interactable and can steal hover/click candidacy from
                // child widgets (QA-proven). Fully passive; hover state now
                // derives from sel_rect response instead.
                let row_rect = Rect::from_min_size(
                    ui.cursor().min,
                    Vec2::new(rack_w - 16.0, ROW_HEIGHT),
                );
                // Advance layout past the row WITHOUT registering any
                // interactable (Sense::hover()/click() on this full-row rect
                // steals hit-testing from child widgets — QA-proven).
                ui.advance_cursor_after_rect(row_rect);

                // ── T3/T5: Scroll into view for keyboard/drop target ─────
                if pending_scroll_to_pos == Some(pos) {
                    ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
                    pending_scroll_to_pos = None; // consume once
                }

                // ── Selected accent bar (left edge, 3px wide) ─────────
                {
                    let p = ui.painter();
                    if is_selected {
                        p.rect_filled(
                            Rect::from_min_size(
                                Pos2::new(row_rect.left(), row_rect.top()),
                                Vec2::new(3.0, ROW_HEIGHT),
                            ),
                            CornerRadius::ZERO,
                            JADE_600_40,
                        );
                    }

                    // T6: Lift background — painted AFTER accent, on top
                    if is_being_lifted {
                        let lift_layer = LayerId::new(
                            Order::Foreground,
                            ui.layer_id().id,
                        );
                        let lift_painter = ui.painter().clone().with_layer_id(lift_layer);
                        lift_painter.rect_filled(
                            row_rect,
                            CornerRadius::same(8),
                            theme::CHARCOAL_700_100,
                        );
                        lift_painter.rect_stroke(
                            row_rect,
                            CornerRadius::same(8),
                            Stroke::new(1.5, theme::GLOW_JADE),
                            StrokeKind::Outside,
                        );
                    }

                    // T5: Drop pulse — after release, the landed row fades from lifted to normal.
                    if !drag.active && pulse < 0.99 {
                        let lift_factor = (1.0 - pulse).clamp(0.0, 1.0);
                        let a = (lift_factor * 255.0) as u8;
                        p.rect_filled(
                            row_rect,
                            CornerRadius::same(8),
                            Color32::from_rgba_premultiplied(0x2a, 0x2a, 0x3a, a),
                        );
                    }
                } // drop painter borrow

                // ── Row content — parent-scope zones (no child-UI!) ────
                // T1: LED click zone (left 24px)
                let led_zone = Rect::from_min_size(
                    row_rect.min,
                    Vec2::new(24.0, ROW_HEIGHT),
                );
                let led_resp = ui.allocate_rect(led_zone, egui::Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);

                // T1: Grip drag zone (24px..48px)
                let grip_zone = Rect::from_min_size(
                    Pos2::new(row_rect.left() + 24.0, row_rect.top()),
                    Vec2::new(24.0, ROW_HEIGHT),
                );
                let grip_resp = ui.allocate_rect(grip_zone, egui::Sense::drag())
                    .on_hover_cursor(CursorIcon::Grab);

                // T1: Selection strip (48px..right-8) — also drives is_hovered
                let sel_zone = Rect::from_min_size(
                    Pos2::new(row_rect.left() + 48.0, row_rect.top()),
                    Vec2::new((rack_w - 16.0 - 48.0 - 8.0).max(40.0), ROW_HEIGHT),
                );
                let sel_resp_zone = ui.allocate_rect(sel_zone, egui::Sense::click());
                let is_hovered = !drag.active && !is_selected && sel_resp_zone.hovered();

                // Selection click
                if sel_resp_zone.clicked() && !drag.active {
                    new_selected = module_idx;
                }

                // LED toggle — click to bypass/enable
                if led_resp.clicked() && !drag.active {
                    entry.bypass.store(is_active, Ordering::Relaxed);
                }

                // LED tooltip (multiline: state + description)
                let led_tip = format!(
                    "Enable / disable \u{2014} disabled effects use zero CPU\n{}",
                    descriptions::module_intro(entry.id_prefix).unwrap_or("")
                );
                led_resp.on_hover_text(&led_tip);

                // Drag start
                if grip_resp.drag_started() {
                    new_drag.active = true;
                    new_drag.source_pos = pos;
                    new_drag.target_pos = pos;
                }

                // ── Paint content (after all rects allocated) ─────────
                {
                    let p = ui.painter();

                    // Hover bg — only for the clickable sel_zone area
                    if is_hovered && !is_being_lifted {
                        p.rect_filled(sel_zone, CornerRadius::same(8), hover_bg);
                    }

                    // Draw LED circle at zone center
                    let led_center = Pos2::new(led_zone.left() + 12.0, led_zone.center().y);
                    if is_active {
                        p.circle_filled(led_center, 4.0, theme::JADE_400);
                        p.circle_stroke(led_center, 6.0, Stroke::new(2.0, theme::JADE_400_60));
                    } else {
                        p.circle_filled(led_center, 4.0, theme::CHARCOAL_600);
                    }

                    // Draw grip lines
                    let grip_color = if grip_resp.hovered() {
                        theme::MIST_300
                    } else {
                        theme::MIST_400
                    };
                    let cx = grip_zone.center().x;
                    let cy = grip_zone.center().y;
                    for dy in [-3.0f32, 0.0, 3.0] {
                        p.line_segment(
                            [
                                Pos2::new(cx - 5.0, cy + dy),
                                Pos2::new(cx + 5.0, cy + dy),
                            ],
                            Stroke::new(1.5, grip_color),
                        );
                    }

                    // ── Passive labels (no Sense allocations) ─────────
                    // Index number (1-based, monospace) at x+52
                    let idx_color = if is_selected {
                        theme::MIST_400
                    } else {
                        theme::CHARCOAL_600
                    };
                    p.text(
                        Pos2::new(row_rect.left() + 52.0, row_rect.top() + ROW_HEIGHT * 0.5),
                        egui::Align2::LEFT_CENTER,
                        format!("{:>2}", pos + 1),
                        theme::mono(10.0),
                        idx_color,
                    );

                    // Module name at x+72 — full brightness always (LED carries state)
                    p.text(
                        Pos2::new(row_rect.left() + 72.0, row_rect.top() + ROW_HEIGHT * 0.5),
                        egui::Align2::LEFT_CENTER,
                        entry.name,
                        theme::body(12.0),
                        theme::MIST_300,
                    );

                    // Latency badge (right-aligned, only if > 0)
                    if entry.latency_samples > 0 {
                        p.text(
                            Pos2::new(row_rect.right() - 12.0, row_rect.top() + ROW_HEIGHT * 0.5),
                            egui::Align2::RIGHT_CENTER,
                        format!("{}s", entry.latency_samples),
                        theme::mono(9.0),
                        theme::SAND_400,
                        );
                    }
                } // drop painter borrow

                // Record row rect for drag hit-testing.
                row_rects.push(row_rect);

                ui.add_space(2.0);
            }
        });

    // ── T6: Drag — compute insertion target and draw 4px gap bar ────────
    if new_drag.active {
        // Hit-test pointer against row rects.
        if let Some(ptr_pos) = ui.input(|i| i.pointer.hover_pos()) {
            for (pos, rect) in row_rects.iter().enumerate() {
                if ptr_pos.y >= rect.top() && ptr_pos.y < rect.bottom() {
                    new_drag.target_pos = if ptr_pos.y < rect.center().y {
                        pos
                    } else {
                        pos + 1
                    };
                    break;
                }
            }
        }

        // T4: Drag-edge autoscroll — proximity-linear speed.
        const EDGE_ZONE: f32 = 30.0;
        const MAX_SPEED: f32 = 12.0;
        if let Some(ptr_pos) = ui.input(|i| i.pointer.hover_pos()) {
            let top_dist = ptr_pos.y - rack_viewport_before.top();
            let bot_dist = rack_viewport_before.bottom() - ptr_pos.y;
            let speed = if top_dist >= 0.0 && top_dist < EDGE_ZONE {
                -MAX_SPEED * (1.0 - top_dist / EDGE_ZONE)
            } else if bot_dist >= 0.0 && bot_dist < EDGE_ZONE {
                MAX_SPEED * (1.0 - bot_dist / EDGE_ZONE)
            } else {
                0.0
            };
            if speed != 0.0 {
                ui.scroll_with_delta(Vec2::new(0.0, speed));
                ui.ctx().request_repaint();
            }
        }

        // T6: Draw 4px tall JADE_400 gap bar (replacement for old 3px hline).
        if !row_rects.is_empty() {
            let bar_height = 4.0;
            let (bar_y, bar_left, bar_right) = if new_drag.target_pos < row_rects.len() {
                let r = &row_rects[new_drag.target_pos];
                (r.top() - bar_height / 2.0, r.left(), r.right())
            } else if let Some(last) = row_rects.last() {
                (last.bottom() - bar_height / 2.0, last.left(), last.right())
            } else {
                (0.0, 0.0, 0.0)
            };
            let bar_rect = Rect::from_min_size(
                Pos2::new(bar_left, bar_y),
                Vec2::new(bar_right - bar_left, bar_height),
            );
            ui.painter().rect_filled(
                bar_rect,
                CornerRadius::same(2),
                theme::JADE_400,
            );
        }

        // On release: commit the reorder and trigger drop pulse.
        if ui.input(|i| i.pointer.any_released()) {
            let mut final_order = order_snapshot.clone();
            if new_drag.source_pos < final_order.len() {
                let item = final_order.remove(new_drag.source_pos);
                let target = new_drag.target_pos.min(final_order.len());
                final_order.insert(target, item);
                // Write to SharedOrder (audio thread reads this).
                *state.order.lock() = final_order.clone();
                // Also persist to params.module_order for nih-plug state save.
                *state.params.module_order.lock() = final_order;
            }
            new_drag.active = false;
            // T6: Reset pulse to 0.0 so it animates to 1.0 (landed row fades from lifted).
            ui.memory_mut(|m| {
                m.data.insert_temp(pulse_id, 0.0_f32);
            });
            ui.ctx().request_repaint();
        }
    }

    // T6: Keep repainting during drop pulse animation.
    if !drag.active && pulse > 0.01 && pulse < 0.99 {
        ui.ctx().request_repaint();
    }

    // ── Commit frame state ───────────────────────────────────────────────
    state.selected = new_selected;
    ui.memory_mut(|m| m.data.insert_temp(drag_id(), new_drag));
}

// ---------------------------------------------------------------------------
// Parameter panel — right side, shows ONLY the selected entry's params
// ---------------------------------------------------------------------------

fn render_param_panel(ui: &mut egui::Ui, setter: &ParamSetter, state: &EditorState) {
    // Resolve the selected entry: state.selected is a module index.
    // Find its position in the current processing order.
    let order_snapshot = state.order.lock().clone();
    let visual_order = ordered_entries(&state.entries, &order_snapshot);
    let selected_pos = position_of_module(&order_snapshot, state.selected)
        .unwrap_or(0)
        .min(visual_order.len().saturating_sub(1));
    let entry = if let Some(e) = visual_order.get(selected_pos) {
        e
    } else {
        ui.add_space(16.0);
        ui.label(
            RichText::new("No module selected.")
                .font(theme::body(12.0))
                .color(theme::CHARCOAL_600),
        );
        return;
    };

    ui.add_space(12.0);

    // Module name heading (Cormorant unavailable — fallback to inter_medium 18px).
    ui.label(
        RichText::new(entry.name)
            .font(theme::heading(18.0))
            .color(theme::MIST_300)
            .strong(),
    );

    // T7: Module intro description
    if let Some(intro) = descriptions::module_intro(entry.id_prefix) {
        ui.label(
            RichText::new(intro)
                .font(theme::body(11.0))
                .color(theme::MIST_400),
        );
    }

    // Id prefix chip + power toggle + state chip.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(humanize_name(entry.id_prefix))
                .font(theme::body(10.0))
                .color(theme::MIST_400),
        );
        ui.add_space(8.0);

        // T8: Mini power toggle — wired to the same bypass flag as the rack.
        let is_active = !entry.bypass.load(Ordering::Relaxed);
        if theme::power_toggle(ui, is_active)
            .on_hover_text(
                "Enable / disable \u{2014} disabled effects use zero CPU",
            )
            .clicked()
        {
            entry.bypass.store(is_active, Ordering::Relaxed);
            ui.ctx().request_repaint();
        }

        ui.add_space(8.0);
        let (label, color) = if is_active {
            ("ACTIVE", theme::JADE_400)
        } else {
            ("BYPASSED", theme::CHARCOAL_600)
        };
        ui.label(RichText::new(label).font(theme::body(10.0)).color(color));
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // Scrollable parameter list — iterates ONLY this entry's params.
    // T1: id_salt per module so each module's param scroll is independent.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt(entry.id_prefix)
        .show(ui, |ui| {
            let params: &dyn Params = entry.params.as_ref();
            let mut has_params = false;
            let mut prev_nesting = String::new();

            for (name, param_ptr, nesting) in params.param_map() {
                // Safety: param_ptr comes from param_map() on a valid Params object.
                let flags = unsafe { param_ptr.flags() };
                if flags.contains(ParamFlags::HIDE_IN_GENERIC_UI) {
                    continue;
                }

                has_params = true;

                // T6: Insert group header on nesting change.
                if nesting != prev_nesting {
                    // Entering a deeper group — add separator + label.
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(2.0);
                    let group = param_group_label(&name);
                    ui.label(theme::section_label_raw(&humanize_name(&group)));
                    prev_nesting = nesting;
                }

                ui.add_space(4.0);
                let name_lower = name.to_lowercase();
                ui.label(
                    RichText::new(humanize_name(&name))
                        .font(theme::body(11.0))
                        .color(theme::MIST_400),
                );

                // Safety: param_ptr is valid — it comes from param_map() on a valid Params.
                let resp = unsafe {
                    render_param_widget(ui, setter, &param_ptr)
                };
                // T7: Attach param tooltip if available
                if let Some(tip) = descriptions::param_tip(entry.id_prefix, &name_lower) {
                    resp.on_hover_text(tip);
                }
            }

            if !has_params {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("No parameters available for this module.")
                        .font(theme::body(11.0))
                        .color(theme::CHARCOAL_600),
                );
            }
        });
}

/// Render a single parameter with the appropriate widget.
///
/// # Safety
///
/// `param_ptr` must point to a valid parameter.
unsafe fn render_param_widget(ui: &mut egui::Ui, setter: &ParamSetter, param_ptr: &ParamPtr) -> egui::Response {
    match param_ptr {
        ParamPtr::FloatParam(p) => {
            ui.add(ParamSlider::for_param(&**p, setter))
        }
        ParamPtr::IntParam(p) => {
            ui.add(ParamSlider::for_param(&**p, setter))
        }
        ParamPtr::BoolParam(p) => {
            ui.add(ParamSlider::for_param(&**p, setter))
        }
        ParamPtr::EnumParam(p) => {
            ui.add(ParamSlider::for_param(&**p, setter))
        }
    }
}

// ---------------------------------------------------------------------------
// Bottom master strip
// ---------------------------------------------------------------------------

fn render_bottom_strip(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    state: &EditorState,
    total_width: f32,
) {
    ui.allocate_ui_with_layout(
        Vec2::new(total_width, BOTTOM_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // MASTER section label
            ui.label(theme::section_label("MASTER"));
            ui.add_space(8.0);

            // Master gain slider
            let gain_resp = ui.add(
                ParamSlider::for_param(&state.params.gain.gain, setter).with_width(200.0),
            );
            gain_resp.on_hover_text("Master output gain");

            // Right-aligned cluster: live latency, UI zoom cycle, About.
            // (Parent scope — the only interactable zone verified reliable.)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // About ghost button
                let about_btn = ui.add(
                    egui::Button::new(
                        RichText::new("About").font(theme::body(11.0)),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, theme::CHARCOAL_600)),
                );
                if about_btn.on_hover_text("About AkiFX").clicked() {
                    let current = ui
                        .memory(|m| m.data.get_temp::<bool>(about_id()).unwrap_or(false));
                    ui.memory_mut(|m| m.data.insert_temp(about_id(), !current));
                }

                ui.add_space(10.0);

                // UI zoom cycle button (60 -> 80 -> 100 -> 125 -> 150 -> 200)
                let z = *state.params.ui_zoom.lock();
                let zoom_btn = ui.add(
                    egui::Button::new(
                        RichText::new(format!("{:.0}%", z * 100.0))
                            .font(theme::mono(11.0)),
                    )
                    .fill(theme::CHARCOAL_700_50)
                    .stroke(Stroke::new(1.0, theme::CHARCOAL_600))
                    .min_size(Vec2::new(56.0, 22.0)),
                );
                if zoom_btn
                    .on_hover_text("UI zoom \u{2014} click to cycle 60\u{2013}200%")
                    .clicked()
                {
                    *state.params.ui_zoom.lock() = zoom_step(z, 1);
                    ui.ctx().request_repaint();
                }

                ui.add_space(10.0);

                // Live total latency = SUM of non-bypassed entries
                let total_latency: u64 = state
                    .entries
                    .iter()
                    .filter(|e| !e.bypass.load(Ordering::Relaxed))
                    .map(|e| e.latency_samples)
                    .sum();
                ui.label(
                    RichText::new(format!("Latency: {} samples", total_latency))
                        .font(theme::mono(11.0))
                        .color(theme::MIST_300),
                )
                .on_hover_text(
                    "Total processing latency contributed by enabled effects (samples).",
                );
            });
        },
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::modules::gain::GainParams;
    use nih_plug::prelude::Params;

    /// ordered_entries with identity permutation returns entries in insertion order.
    #[test]
    fn ordered_entries_identity() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);
        let identity: Vec<usize> = (0..21).collect();

        let ordered = ordered_entries(&entries, &identity);
        assert_eq!(ordered.len(), 21);
        for (i, entry) in ordered.iter().enumerate() {
            assert_eq!(entry.name, entries[i].name);
        }
    }

    /// ordered_entries with a custom permutation reorders correctly.
    #[test]
    fn ordered_entries_custom_permutation() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);
        // Reverse order: last module first.
        let reversed: Vec<usize> = (0..21).rev().collect();

        let ordered = ordered_entries(&entries, &reversed);
        assert_eq!(ordered.len(), 21);
        assert_eq!(ordered[0].name, "Safety Limiter");
        assert_eq!(ordered[20].name, "Sine Generator");
    }

    /// ordered_entries filters out-of-range indices silently.
    #[test]
    fn ordered_entries_filters_invalid_indices() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);
        let order_with_gaps = vec![0, 99, 1, 200, 2];

        let ordered = ordered_entries(&entries, &order_with_gaps);
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].name, "Sine Generator");
        assert_eq!(ordered[1].name, "MIDI Inverter");
        assert_eq!(ordered[2].name, "Poly Mod Synth");
    }

    // ── position_of_module tests ────────────────────────────────────────

    /// Identity order: position_of_module(i) == i for all i.
    #[test]
    fn position_of_module_identity() {
        let order: Vec<usize> = (0..21).collect();
        for i in 0..21 {
            assert_eq!(
                position_of_module(&order, i),
                Some(i),
                "identity order: module {i} should be at position {i}"
            );
        }
    }

    /// Custom permutation maps correctly.
    #[test]
    fn position_of_module_custom_permutation() {
        // order[0]=20, order[1]=19, ..., order[20]=0  (reversed)
        let order: Vec<usize> = (0..21).rev().collect();
        // Module 20 is at position 0
        assert_eq!(position_of_module(&order, 20), Some(0));
        // Module 0 is at position 20
        assert_eq!(position_of_module(&order, 0), Some(20));
        // Module 10 is at position 10 (center stays)
        assert_eq!(position_of_module(&order, 10), Some(10));
    }

    /// Non-contiguous permutation with a gap.
    #[test]
    fn position_of_module_sparse_permutation() {
        let order = vec![5, 0, 19, 3];
        assert_eq!(position_of_module(&order, 5), Some(0));
        assert_eq!(position_of_module(&order, 0), Some(1));
        assert_eq!(position_of_module(&order, 19), Some(2));
        assert_eq!(position_of_module(&order, 3), Some(3));
    }

    /// Module index not in order → None.
    #[test]
    fn position_of_module_not_found() {
        let order: Vec<usize> = (0..21).collect();
        assert_eq!(position_of_module(&order, 99), None);
    }

    // ── Selection semantics tests ──────────────────────────────────────
    //
    // These test that `EditorState.selected` stores MODULE INDEX (not
    // position), and that after a reorder the selected module identity
    // is preserved.

    /// After reorder, selected module identity is preserved.
    ///
    /// Simulates: select module 19 ("Gain"), then reverse the order.
    /// selected should still point at module 19 (now at position 1).
    #[test]
    fn selected_preserves_module_identity_after_reorder() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);

        // selected = module index 19 (Gain)
        let selected_module: usize = 19;
        let order_after_reorder: Vec<usize> = (0..21).rev().collect();

        // Resolve the selected entry via module index
        let entry = &entries[selected_module];
        assert_eq!(entry.name, "Gain");

        // Find position in the new order
        let pos = position_of_module(&order_after_reorder, selected_module)
            .expect("module 19 must be in order");
        let visual = ordered_entries(&entries, &order_after_reorder);
        assert_eq!(visual[pos].name, "Gain");
    }

    /// selected=0 always points at the first module, regardless of order.
    #[test]
    fn selected_zero_follows_module_not_position() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);

        // Reversed order: module 20 (Safety Limiter) is at position 0
        let reversed: Vec<usize> = (0..21).rev().collect();
        let selected_module: usize = 0; // Sine Generator

        let pos = position_of_module(&reversed, selected_module).unwrap();
        let visual = ordered_entries(&entries, &reversed);
        // Position 0 in visual is Safety Limiter; Sine Generator is at position 20
        assert_eq!(visual[pos].name, "Sine Generator");
        assert_eq!(pos, 20);
    }

    /// BUG REPRODUCTION: selected as position (old code) points at wrong
    /// module after reorder; selected as module index (new code) is correct.
    #[test]
    fn selected_as_position_fails_after_reorder() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);

        // Before reorder: identity order, user clicks position 19 → Gain
        let identity: Vec<usize> = (0..21).collect();
        let visual_before = ordered_entries(&entries, &identity);
        assert_eq!(visual_before[19].name, "Gain");

        // OLD (buggy) semantics: selected = 19 (position)
        let selected_position: usize = 19;

        // After reorder: reversed. Position 19 is now module 1 (MIDI Inverter)
        let reversed: Vec<usize> = (0..21).rev().collect();
        let visual_after = ordered_entries(&entries, &reversed);

        // BUG: position 19 in reversed order ≠ Gain
        assert_ne!(
            visual_after[selected_position].name, "Gain",
            "position-based selection breaks after reorder"
        );

        // NEW (correct) semantics: selected = 19 (module index)
        let selected_module: usize = 19;
        let pos = position_of_module(&reversed, selected_module).unwrap();
        assert_eq!(
            visual_after[pos].name, "Gain",
            "module-index-based selection survives reorder"
        );
    }

    /// Param panel resolution: given module_idx + order, find the right entry.
    #[test]
    fn param_panel_resolves_selected_module_by_index() {
        let params = AkiFxParams::default();
        let entries = state::build_ui_entries(&params);
        let order: Vec<usize> = (0..21).rev().collect();

        // Select module 19 (Gain) — stored as module_idx, not position
        let selected_module: usize = 19;
        let pos = position_of_module(&order, selected_module)
            .expect("module 19 must be in order");
        let visual = ordered_entries(&entries, &order);
        let entry = visual.get(pos).expect("position must be valid");

        assert_eq!(entry.name, "Gain");
    }

    /// Param panel: absent module_idx falls back safely (None path).
    #[test]
    fn param_panel_handles_absent_module_gracefully() {
        let params = AkiFxParams::default();
        let _entries = state::build_ui_entries(&params);
        let order: Vec<usize> = (0..21).collect();

        // Module 99 doesn't exist
        let result = position_of_module(&order, 99);
        assert!(result.is_none(), "absent module should return None");
    }

    #[test]
    fn humanize_name_snake_case() {
        assert_eq!(humanize_name("sine_level"), "Sine Level");
        assert_eq!(humanize_name("pms_gain"), "Pms Gain");
        assert_eq!(humanize_name("mix"), "Mix");
        assert_eq!(humanize_name(""), "");
        assert_eq!(humanize_name("num_overlaps"), "Num Overlaps");
    }

    /// Param iteration over GainParams yields at least one visible parameter.
    #[test]
    fn param_panel_handles_gain_params_map() {
        let params = Arc::new(GainParams::new(0.0));
        let dyn_params: &dyn Params = params.as_ref();

        let mut count = 0;
        for (_name, param_ptr, _nesting) in dyn_params.param_map() {
            let flags = unsafe { param_ptr.flags() };
            if flags.contains(ParamFlags::HIDE_IN_GENERIC_UI) {
                continue;
            }
            count += 1;

            match param_ptr {
                ParamPtr::FloatParam(_)
                | ParamPtr::IntParam(_)
                | ParamPtr::BoolParam(_)
                | ParamPtr::EnumParam(_) => {}
            }
        }

        assert!(
            count > 0,
            "GainParams should expose at least one parameter via param_map()"
        );
    }

    // ── T7: next_rack_position tests ────────────────────────────────────

    #[test]
    fn next_rack_position_up_clamps_at_zero() {
        assert_eq!(next_rack_position(0, 21, egui::Key::ArrowUp), 0);
    }

    #[test]
    fn next_rack_position_down_increments() {
        assert_eq!(next_rack_position(0, 21, egui::Key::ArrowDown), 1);
    }

    #[test]
    fn next_rack_position_down_clamps_at_end() {
        assert_eq!(next_rack_position(20, 21, egui::Key::ArrowDown), 20);
    }

    #[test]
    fn next_rack_position_up_decrements() {
        assert_eq!(next_rack_position(10, 21, egui::Key::ArrowUp), 9);
    }

    #[test]
    fn next_rack_position_page_up_clamps_at_zero() {
        assert_eq!(next_rack_position(0, 21, egui::Key::PageUp), 0);
    }

    #[test]
    fn next_rack_position_page_down_jumps() {
        assert_eq!(next_rack_position(15, 21, egui::Key::PageDown), 20);
    }

    #[test]
    fn next_rack_position_page_up_jumps() {
        assert_eq!(next_rack_position(10, 21, egui::Key::PageUp), 0);
    }

    #[test]
    fn next_rack_position_page_down_clamps_at_end() {
        assert_eq!(next_rack_position(10, 21, egui::Key::PageDown), 20);
    }

    #[test]
    fn next_rack_position_home_goes_to_zero() {
        assert_eq!(next_rack_position(10, 21, egui::Key::Home), 0);
    }

    #[test]
    fn next_rack_position_end_goes_to_last() {
        assert_eq!(next_rack_position(10, 21, egui::Key::End), 20);
    }

    #[test]
    fn next_rack_position_empty_count_returns_zero() {
        assert_eq!(next_rack_position(0, 0, egui::Key::ArrowDown), 0);
    }

    #[test]
    fn next_rack_position_unknown_key_returns_cur() {
        assert_eq!(next_rack_position(5, 21, egui::Key::A), 5);
    }

    // ── T8: param_group_label tests ──────────────────────────────────────

    #[test]
    fn param_group_label_global_prefix() {
        assert_eq!(param_group_label("global_makeup"), "global");
    }

    #[test]
    fn param_group_label_threshold_prefix() {
        assert_eq!(param_group_label("threshold_knee"), "threshold");
    }

    #[test]
    fn param_group_label_no_underscore() {
        assert_eq!(param_group_label("gain"), "gain");
    }

    #[test]
    fn param_group_label_empty_string() {
        assert_eq!(param_group_label(""), "");
    }

    // ── zoom_step tests ───────────────────────────────────────────────

    #[test]
    fn zoom_step_in_from_1x() {
        assert!((zoom_step(1.0, 1) - 1.25).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_out_from_1x() {
        assert!((zoom_step(1.0, -1) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_in_clamps_at_max() {
        assert!((zoom_step(2.0, 1) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_out_clamps_at_min() {
        assert!((zoom_step(0.6, -1) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_unknown_cur_snaps_to_nearest() {
        // 0.9 is equidistant to 0.8 and 1.0; min_by picks 0.8 (first), so +1 → 1.0
        assert!((zoom_step(0.9, 1) - 1.0).abs() < 1e-6);
        // 1.1 is nearest to 1.0, so +1 → 1.25
        assert!((zoom_step(1.1, 1) - 1.25).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_zero_direction_returns_cur() {
        assert!((zoom_step(1.0, 0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_step_chains_in_and_out() {
        let v1 = zoom_step(1.0, 1); // 1.25
        let v2 = zoom_step(v1, 1);  // 1.5
        let v3 = zoom_step(v2, -1); // 1.25
        assert!((v1 - 1.25).abs() < 1e-6);
        assert!((v2 - 1.5).abs() < 1e-6);
        assert!((v3 - 1.25).abs() < 1e-6);
    }
}

//! AkiFX egui-based plugin editor — FX Rack interface in the Aki "Aurora
//! Glass" design language.
//!
//! Title bar: brand wordmark with the signature star and eyebrow subtitle.
//! Left rack: the 11 effect modules with drag-to-reorder, power toggles, and
//! latency badges, rendered as the navigation rail.
//! Right panel: the selected module's parameter controls.
//! Footer: global bypass, master gain, output meters, latency, zoom, About.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, CornerRadius, CursorIcon, LayerId, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2};
use nih_plug_egui::egui::layers::Order;
use nih_plug_egui::{create_egui_editor, resizable_window::ResizableWindow, EguiState};
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod paint;
mod theme;
mod widgets;
pub mod descriptions;
pub mod state;

use crate::AkiFxParams;
use crate::SharedOrder;
use state::ModuleUiEntry;
use theme::pal;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default window dimensions (width, height).
const DEFAULT_SIZE: (u32, u32) = (1100, 720);
/// Row height for each rack entry.
const ROW_HEIGHT: f32 = 34.0;
/// Title bar height.
const TOP_BAR_HEIGHT: f32 = 48.0;
/// Footer height.
const FOOTER_HEIGHT: f32 = 56.0;

/// Peak-meter display range (dBFS) and per-frame decay.
const METER_MIN_DB: f32 = -60.0;
const METER_MAX_DB: f32 = 6.0;
const METER_DECAY_DB_PER_FRAME: f32 = 1.9;

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

/// Truncate a label with an ellipsis so it fits `max_w` pixels at the given
/// font (rack rows keep name + latency badge on one line).
fn truncate_to_width(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    max_w: f32,
) -> String {
    const ELLIPSIS: char = '\u{2026}';
    if painter
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x
        <= max_w
    {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (0..chars.len()).rev() {
        let mut candidate: String = chars[..keep].iter().collect();
        candidate.push(ELLIPSIS);
        if painter
            .layout_no_wrap(candidate.clone(), font.clone(), Color32::WHITE)
            .size()
            .x
            <= max_w
        {
            return candidate;
        }
    }
    ELLIPSIS.to_string()
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
    /// Output peak meters shared with the audio thread (absolute value).
    pub peak_l: Arc<AtomicF32>,
    pub peak_r: Arc<AtomicF32>,
    /// Live spectrum handle from the Spectral Compressor module, when
    /// present (set by `Plugin::editor()`).
    pub spectrum_view: Option<crate::modules::spectral_compressor::analyzer::SpectrumView>,
    /// Cached grain-noise texture for the aurora background (lazily built on
    /// the first painted frame).
    pub noise: Option<egui::TextureHandle>,
    /// Peak-meter display state: shown values decay smoothly in the GUI
    /// (fast attack from the atomics, slow release).
    pub meter_db_l: f32,
    pub meter_db_r: f32,
    /// Latched overload indicator — stays red until the meter is clicked.
    pub clip_latched: bool,
    /// Last applied factory preset (index into `presets::PRESETS`).
    pub active_preset: Option<usize>,
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
    peak_l: Arc<AtomicF32>,
    peak_r: Arc<AtomicF32>,
    spectrum_view: Option<crate::modules::spectral_compressor::analyzer::SpectrumView>,
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
            peak_l,
            peak_r,
            spectrum_view,
            noise: None,
            meter_db_l: METER_MIN_DB,
            meter_db_r: METER_MIN_DB,
            clip_latched: false,
            active_preset: None,
        },
        // build: called once when the editor window is created
        |ctx, _user_state| {
            theme::load_fonts(ctx);
            theme::configure_visuals(ctx);
            // Seed the star bloom so it plays once on editor open.
            ctx.memory_mut(|m| {
                m.data
                    .insert_temp(egui::Id::new("akifx_star_bloom"), 0.0_f32)
            });
        },
        // update: called every frame
        move |ctx, setter, user_state| {
            // Font zoom — sync theme zoom from the persisted param.
            // (ctx.set_pixels_per_point is IGNORED by the baseview renderer,
            // which always presents at the system scale factor; scaling is
            // therefore implemented at the theme font-size level instead.)
            let z = *user_state.params.ui_zoom.lock();
            let z_pct = (z * 100.0).round() as u32;
            if theme::font_zoom_pct() != z_pct {
                theme::set_font_zoom_pct(z_pct);
                theme::apply_text_styles(ctx);
            }

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
    // Aurora curtain + grain behind everything; all fills above stay
    // transparent so the background breathes through the glass cards.
    paint::background(ui, &mut state.noise);

    // CRITICAL: use the CLIP RECT for sizing, not available_height(). Inside
    // ResizableWindow's child ui, available_height() can be unbounded, which
    // previously inflated the middle panel so much that the bottom master
    // strip was pushed entirely out of the visible window (user complaint:
    // "界面总是显示不全").
    let total_width = ui.clip_rect().width();
    let total_height = ui.clip_rect().height();

    // ── Title bar (frosted strip) ─────────────────────────────────────────
    let (title_rect, _) =
        ui.allocate_exact_size(Vec2::new(total_width, TOP_BAR_HEIGHT), egui::Sense::hover());
    paint::frosted_strip(ui.painter(), title_rect);
    ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(title_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
            |ui| {
        render_title_bar(ui, setter, state);
    });
    ui.add_space(8.0);

    // ── Middle: rack card + module card ───────────────────────────────────
    let rack_w = rack_width(total_width);
    // Deterministic fit: title(48) + gap(8) + middle + gap(8) + footer(56).
    let middle_height = (total_height - TOP_BAR_HEIGHT - FOOTER_HEIGHT - 16.0).max(120.0);
    let param_width = (total_width - rack_w - 12.0).max(380.0);
    ui.allocate_ui_with_layout(
        Vec2::new(total_width, middle_height),
        egui::Layout::left_to_right(egui::Align::TOP),
        |ui| {
            // Rack navigation card
            let (rack_rect, _) =
                ui.allocate_exact_size(Vec2::new(rack_w, middle_height), egui::Sense::hover());
            let rack_content = paint::glass_card(ui.painter(), rack_rect);
            ui.allocate_new_ui(
                egui::UiBuilder::new()
                    .max_rect(rack_content)
                    .id_salt("akifx_rack_card")
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    render_rack_panel(ui, state, rack_w);
                },
            );

            ui.add_space(12.0);

            // Module card (fills the remainder)
            let card_left = rack_rect.right() + 12.0;
            let card_rect = Rect::from_min_size(
                Pos2::new(card_left, rack_rect.top()),
                Vec2::new(param_width, middle_height),
            );
            let card_content = paint::glass_card(ui.painter(), card_rect);
            ui.allocate_new_ui(
                egui::UiBuilder::new()
                    .max_rect(card_content)
                    .id_salt("akifx_param_card")
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    render_param_panel(ui, setter, state);
                },
            );
        },
    );
    ui.add_space(8.0);

    // ── Footer (frosted strip) ────────────────────────────────────────────
    let (footer_rect, _) =
        ui.allocate_exact_size(Vec2::new(total_width, FOOTER_HEIGHT), egui::Sense::hover());
    paint::frosted_strip(ui.painter(), footer_rect);
    ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(footer_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
            |ui| {
        render_footer(ui, setter, state);
    });

    // ── About floating window (rendered last, on top) ────────────────────
    let show_about = ui.memory(|m| m.data.get_temp::<bool>(about_id()).unwrap_or(false));
    if show_about {
        let p = pal();
        egui::Window::new("About AkiFX")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .frame(
                egui::Frame::NONE
                    .fill(p.liquid_bg)
                    .stroke(Stroke::new(1.0, p.liquid_border))
                    .inner_margin(16.0)
                    .corner_radius(CornerRadius::same(theme::radius::MD as u8))
                    .shadow(theme::card_shadow()),
            )
            .show(ui.ctx(), |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!("AkiFX v{}", env!("CARGO_PKG_VERSION")))
                            .font(theme::heading(14.0))
                            .color(p.text_primary)
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
                        .color(p.text_secondary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Airwindows Hard Vacuum port credit: Chris Johnson.",
                        )
                        .font(theme::body(11.0))
                        .color(p.text_secondary),
                    );
                    // Signature quote line (one per app)
                    ui.add_space(10.0);
                    paint::hairline(ui.painter(), Rect::from_min_max(
                        Pos2::new(ui.cursor().left(), ui.cursor().top()),
                        Pos2::new(ui.cursor().right(), ui.cursor().top() + 1.0),
                    ));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("\u{201c}The space between notes is where \
                            the story lives.\u{201d}")
                            .font(theme::display_italic(11.0))
                            .color(p.text_tertiary),
                    );
                });
            });
    }
}

// ---------------------------------------------------------------------------
// Preset application (factory presets, title-bar PresetBar)
// ---------------------------------------------------------------------------

/// Bridges [`presets::PatchSink`] to the editor state and the host-notifying
/// [`ParamSetter`]: module enable flags write straight into the shared bypass
/// atomics, parameter values go through begin/set/end gesture groups.
struct GuiPresetSink<'a> {
    entries: &'a [ModuleUiEntry],
    setter: &'a ParamSetter<'a>,
}

impl crate::presets::PatchSink for GuiPresetSink<'_> {
    fn set_module_enabled(&mut self, module: usize, enabled: bool) {
        if let Some(entry) = self.entries.get(module) {
            entry.bypass.store(!enabled, Ordering::Relaxed);
        }
    }

    unsafe fn set_normalized(&mut self, _module: usize, _id: &str, ptr: &ParamPtr, normalized: f32) {
        match ptr {
            ParamPtr::FloatParam(p) => {
                self.setter.begin_set_parameter(&**p);
                self.setter.set_parameter_normalized(&**p, normalized);
                self.setter.end_set_parameter(&**p);
            }
            ParamPtr::IntParam(p) => {
                self.setter.begin_set_parameter(&**p);
                self.setter.set_parameter_normalized(&**p, normalized);
                self.setter.end_set_parameter(&**p);
            }
            ParamPtr::BoolParam(p) => {
                self.setter.begin_set_parameter(&**p);
                self.setter.set_parameter_normalized(&**p, normalized);
                self.setter.end_set_parameter(&**p);
            }
            ParamPtr::EnumParam(p) => {
                self.setter.begin_set_parameter(&**p);
                self.setter.set_parameter_normalized(&**p, normalized);
                self.setter.end_set_parameter(&**p);
            }
        }
    }
}

/// Apply a factory preset through the host-notifying sink and mirror the new
/// module power states into the persisted `module_enabled` params.
fn apply_preset(state: &mut EditorState, setter: &ParamSetter, index: usize) {
    let Some(preset) = crate::presets::PRESETS.get(index) else {
        return;
    };
    let mut sink = GuiPresetSink {
        entries: &state.entries,
        setter,
    };
    crate::presets::apply_to_sink(preset, state.params.as_ref(), &mut sink);
    crate::gui::state::sync_persisted_enabled(state.params.as_ref(), &state.entries);
    state.active_preset = Some(index);
}

// ---------------------------------------------------------------------------
// Title bar
// ---------------------------------------------------------------------------

fn render_title_bar(ui: &mut egui::Ui, setter: &ParamSetter, state: &mut EditorState) {
    let p = pal();
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(theme::space::S4);

        // Signature star with bloom-in animation (seeded at editor open)
        let star_size = 16.0;
        let (star_rect, _) =
            ui.allocate_exact_size(Vec2::new(star_size, star_size), egui::Sense::hover());
        let bloom = ui
            .ctx()
            .animate_value_with_time(egui::Id::new("akifx_star_bloom"), 1.0_f32, 0.45);
        paint::star(
            ui.painter(),
            star_rect.center(),
            star_size,
            p.grad_c,
            bloom,
            0.0,
        );
        if bloom < 0.999 {
            ui.ctx().request_repaint();
        }

        ui.add_space(2.0);

        // AkiFX wordmark (Fraunces SemiBold)
        ui.label(
            RichText::new("AkiFX")
                .font(theme::heading(22.0))
                .color(p.text_primary)
                .strong(),
        );

        ui.add_space(6.0);

        // Eyebrow subtitle
        ui.label(theme::eyebrow("SpectralSuite \u{00d7} NIH-plug suite"));

        // Right cluster: live total latency badge
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(theme::space::S4);
            let total_latency: u64 = state
                .entries
                .iter()
                .filter(|e| !e.bypass.load(Ordering::Relaxed))
                .map(|e| e.latency_samples)
                .sum();
            ui.label(
                RichText::new(format!("PDC {} smp", total_latency))
                    .font(theme::mono(11.0))
                    .color(p.text_secondary),
            )
            .on_hover_text(
                "Total processing latency contributed by enabled effects (samples).",
            );

            ui.add_space(theme::space::S3);

            // PresetBar: previous · name dropdown · next
            let current = state.active_preset;
            let step = |delta: i32, ui: &mut egui::Ui, state: &mut EditorState, setter: &ParamSetter| {
                let preset_count = crate::presets::PRESETS.len();
                let current = state.active_preset;
                let next = match current {
                    Some(i) => (i as i32 + delta).rem_euclid(preset_count as i32) as usize,
                    None if delta > 0 => 1.min(preset_count - 1),
                    None => 0,
                };
                apply_preset(state, setter, next);
                ui.ctx().request_repaint();
            };
            let prev_btn = ui.add(
                egui::Button::new(
                    RichText::new("\u{2039}").font(theme::sans_semibold(15.0)).color(p.text_secondary),
                )
                .fill(Color32::TRANSPARENT)
                .min_size(Vec2::new(24.0, 24.0)),
            );
            if prev_btn.on_hover_text("Previous preset").clicked() {
                step(-1, ui, state, setter);
            }

            let selected_name = current
                .and_then(|i| crate::presets::PRESETS.get(i))
                .map(|pr| pr.name)
                .unwrap_or("Preset");
            let presets = crate::presets::PRESETS;
            egui::ComboBox::from_id_salt("akifx_preset_bar")
                .width(150.0)
                .selected_text(selected_name)
                .show_ui(ui, |ui| {
                    for (i, pr) in presets.iter().enumerate() {
                        let label = if pr.reset_all {
                            pr.name.to_owned()
                        } else {
                            format!("{} \u{2014} {}", pr.name, pr.role)
                        };
                        if ui.selectable_label(current == Some(i), label).clicked() {
                            apply_preset(state, setter, i);
                            ui.ctx().request_repaint();
                        }
                    }
                })
                .response
                .on_hover_text("Factory presets \u{2014} light to heavy; never touches master gain, the limiter, or global bypass");

            let next_btn = ui.add(
                egui::Button::new(
                    RichText::new("\u{203a}").font(theme::sans_semibold(15.0)).color(p.text_secondary),
                )
                .fill(Color32::TRANSPARENT)
                .min_size(Vec2::new(24.0, 24.0)),
            );
            if next_btn.on_hover_text("Next preset").clicked() {
                step(1, ui, state, setter);
            }
        });
    });
}

fn render_rack_panel(ui: &mut egui::Ui, state: &mut EditorState, rack_w: f32) {
    let p = pal();
    ui.add_space(8.0);
    ui.label(theme::eyebrow("Effect Rack"));
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
                let hover_bg = theme::with_alpha(p.text_primary, 0.06);

                // ── Row passive rect (NO senses!) ────────────────────────
                // CRITICAL FIX 2: even Sense::hover() registers this rect as
                // an interactable and can steal hover/click candidacy from
                // child widgets (QA-proven). Fully passive; hover state now
                // derives from sel_rect response instead.
                let row_rect = Rect::from_min_size(
                    ui.cursor().min,
                    Vec2::new(ui.available_width().min(rack_w - 48.0), ROW_HEIGHT),
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

                // ── Selected pill: accent 13% fill + neon edge on top ────
                {
                    let p = ui.painter();
                    if is_selected {
                        p.rect_filled(
                            row_rect,
                            CornerRadius::same(theme::radius::SM as u8),
                            theme::with_alpha(pal().accent, 0.13),
                        );
                        paint::neon_edge(p, row_rect);
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
                            pal().surface_1,
                        );
                        lift_painter.rect_stroke(
                            row_rect,
                            CornerRadius::same(8),
                            Stroke::new(1.5, pal().accent),
                            StrokeKind::Outside,
                        );
                    }

                    // T5: Drop pulse — after release, the landed row fades from lifted to normal.
                    if !drag.active && pulse < 0.99 {
                        let lift_factor = (1.0 - pulse).clamp(0.0, 1.0);
                        p.rect_filled(
                            row_rect,
                            CornerRadius::same(8),
                            theme::with_alpha(pal().surface_1, lift_factor),
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
                    Vec2::new((rack_w - 48.0 - 48.0 - 8.0).max(40.0), ROW_HEIGHT),
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
                    // Persist the new state so the host saves it.
                    crate::gui::state::sync_persisted_enabled(state.params.as_ref(), &state.entries);
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
                        p.circle_filled(led_center, 4.0, pal().accent);
                        p.circle_stroke(
                            led_center,
                            6.0,
                            Stroke::new(2.0, theme::with_alpha(pal().accent, 0.6)),
                        );
                    } else {
                        p.circle_filled(led_center, 4.0, pal().border);
                    }

                    // Draw grip lines
                    let grip_color = if grip_resp.hovered() {
                        pal().text_primary
                    } else {
                        pal().text_secondary
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
                        pal().text_secondary
                    } else {
                        pal().text_tertiary
                    };
                    p.text(
                        Pos2::new(row_rect.left() + 52.0, row_rect.top() + ROW_HEIGHT * 0.5),
                        egui::Align2::LEFT_CENTER,
                        format!("{:>2}", pos + 1),
                        theme::mono(10.0),
                        idx_color,
                    );

                    // Latency badge (right-aligned, only if > 0); the name
                    // is truncated below so the two never collide.
                    let badge_w = if entry.latency_samples > 0 {
                        let text = format!("{} smp", entry.latency_samples);
                        let w = p
                            .layout_no_wrap(text.clone(), theme::mono(9.5), Color32::WHITE)
                            .size()
                            .x;
                        p.text(
                            Pos2::new(row_rect.right() - 10.0, row_rect.top() + ROW_HEIGHT * 0.5),
                            egui::Align2::RIGHT_CENTER,
                            text,
                            theme::mono(9.5),
                            pal().accent_secondary,
                        );
                        w
                    } else {
                        0.0
                    };

                    // Module name at x+72 — accent when selected (the pill
                    // carries selection), full brightness otherwise.
                    let name_x = row_rect.left() + 72.0;
                    let name_max_w = row_rect.right() - 10.0 - badge_w - 12.0 - name_x;
                    let name_text =
                        truncate_to_width(p, entry.name, theme::sans_medium(12.5), name_max_w);
                    p.text(
                        Pos2::new(name_x, row_rect.top() + ROW_HEIGHT * 0.5),
                        egui::Align2::LEFT_CENTER,
                        name_text,
                        theme::sans_medium(12.5),
                        if is_selected { pal().accent } else { pal().text_primary },
                    );
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

        // T6: Draw the 4px gap insertion indicator.
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
                pal().accent,
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

fn render_param_panel(ui: &mut egui::Ui, setter: &ParamSetter, state: &mut EditorState) {
    let p = pal();
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
                .color(p.text_tertiary),
        );
        return;
    };

    ui.add_space(12.0);

    // Module name heading (Fraunces SemiBold).
    ui.label(
        RichText::new(entry.name)
            .font(theme::heading(20.0))
            .color(p.text_primary)
            .strong(),
    );

    // Module intro description
    if let Some(intro) = descriptions::module_intro(entry.id_prefix) {
        ui.label(
            RichText::new(intro)
                .font(theme::body(11.0))
                .color(p.text_secondary),
        );
    }

    // Live spectrum + threshold curve for the Spectral Compressor.
    if entry.id_prefix == "spectral_compressor" {
        render_spectrum_view(ui, state);
    }

    // Id prefix chip + power toggle + state chip.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(humanize_name(entry.id_prefix))
                .font(theme::sans_semibold(10.0))
                .color(p.text_tertiary),
        );
        ui.add_space(8.0);

        // Mini power toggle — wired to the same bypass flag as the rack.
        let is_active = !entry.bypass.load(Ordering::Relaxed);
        if theme::power_toggle(ui, is_active)
            .on_hover_text(
                "Enable / disable \u{2014} disabled effects use zero CPU",
            )
            .clicked()
        {
            entry.bypass.store(is_active, Ordering::Relaxed);
            // Persist the new state so the host saves it.
            crate::gui::state::sync_persisted_enabled(state.params.as_ref(), &state.entries);
            ui.ctx().request_repaint();
        }

        ui.add_space(8.0);
        let (label, color) = if is_active {
            ("ACTIVE", p.accent)
        } else {
            ("BYPASSED", p.text_tertiary)
        };
        ui.label(RichText::new(label).font(theme::sans_semibold(10.0)).color(color));
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
            let mut last_header: Option<String> = None;

            for (name, param_ptr, nesting) in params.param_map() {
                // Safety: param_ptr comes from param_map() on a valid Params object.
                let flags = unsafe { param_ptr.flags() };
                if flags.contains(ParamFlags::HIDE_IN_GENERIC_UI) {
                    continue;
                }

                has_params = true;

                // Group header on nesting change; remember its text so the
                // first row's label is not the same word twice.
                if nesting != prev_nesting {
                    // Entering a deeper group — add separator + label.
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(2.0);
                    let group = param_group_label(&name);
                    last_header = Some(humanize_name(&group));
                    ui.label(theme::eyebrow_raw(last_header.as_deref().unwrap_or("")));
                    prev_nesting = nesting;
                }

                ui.add_space(4.0);
                let name_lower = name.to_lowercase();
                let label_text = humanize_name(&name);
                if Some(&label_text) != last_header.as_ref() {
                    ui.label(
                        RichText::new(label_text)
                            .font(theme::body(12.0))
                            .color(p.text_secondary),
                    );
                }
                // Only the first row of a group can share the header's word.
                last_header = None;

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
                        .color(p.text_tertiary),
                );
            }
            // Breathing room at the end of the scroll so the last row never
            // sits flush against the card's inner bezel.
            ui.add_space(theme::space::S6);
        });
}

/// Render a single parameter with the appropriate Aki widget: ramp-gradient
/// slider (float/int), segmented selector (enum, when the variant count fits),
/// or pill switch (bool).
///
/// # Safety
///
/// `param_ptr` must point to a valid parameter.
unsafe fn render_param_widget(ui: &mut egui::Ui, setter: &ParamSetter, param_ptr: &ParamPtr) -> egui::Response {
    match param_ptr {
        ParamPtr::FloatParam(p) => widgets::float_slider(ui, setter, &**p),
        ParamPtr::IntParam(p) => widgets::int_slider(ui, setter, &**p),
        ParamPtr::BoolParam(p) => {
            let param = unsafe { &**p };
            let value = param.value();
            widgets::bool_switch(ui, value, |v| {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, v);
                setter.end_set_parameter(param);
            })
        }
        ParamPtr::EnumParam(p) => {
            let param = unsafe { &**p };
            // Segmented selector when the variant count fits a row; a plain
            // normalized slider otherwise.
            let steps = param.step_count();
            if let Some(k) = steps.filter(|k| *k > 0 && *k <= 7) {
                let current = (param.modulated_normalized_value() * k as f32).round().clamp(0.0, k as f32) as usize;
                let options: Vec<String> = (0..=k)
                    .map(|i| param.normalized_value_to_string(i as f32 / k as f32, false))
                    .collect();
                widgets::segmented(ui, current, &options, |idx| {
                    setter.begin_set_parameter(param);
                    setter.set_parameter(param, idx as i32);
                    setter.end_set_parameter(param);
                })
            } else {
                let value = param.modulated_normalized_value();
                widgets::param_slider(
                    ui,
                    value,
                    param.default_normalized_value(),
                    false,
                    &param.normalized_value_to_string(value, true),
                    |g, n| match g {
                        widgets::SliderGesture::Begin => setter.begin_set_parameter(param),
                        widgets::SliderGesture::Set => setter.set_parameter_normalized(param, n),
                        widgets::SliderGesture::End => setter.end_set_parameter(param),
                    },
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Spectrum view (Spectral Compressor)
// ---------------------------------------------------------------------------

/// Draw the live input spectrum with the downwards threshold curve overlaid,
/// on a logarithmic frequency axis (20 Hz .. Nyquist), on a contrast panel
/// with a market-EQ style grid: labeled 100/1k/10k columns, faint octave
/// columns, and dB rows every 20 dB. This is the tuning surface for the
/// per-bin compressor: bands above the curve get compressed.
fn render_spectrum_view(ui: &mut egui::Ui, state: &EditorState) {
    const MIN_FREQ: f32 = 20.0;
    const TOP_DB: f32 = 0.0;
    const BOTTOM_DB: f32 = -100.0;
    /// Path thinning cap (design budget: ≤420 points).
    const MAX_POINTS: usize = 420;

    let Some(view) = state.spectrum_view.as_ref() else {
        return;
    };
    let snapshot = view.latest();
    if snapshot.magnitudes_db.is_empty() {
        return; // nothing published yet
    }

    ui.add_space(8.0);
    let width = ui.available_width();
    let height = 150.0;
    let (rect, _resp) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let p = pal();
    let on_contrast = theme::with_alpha(p.text_on_contrast, 0.08);

    // Contrast panel (visual anchor per §11.3)
    painter.rect_filled(rect, theme::radius::MD, p.surface_contrast);

    let nyquist = snapshot.sample_rate * 0.5;
    if snapshot.sample_rate <= 0.0 || nyquist <= MIN_FREQ {
        return;
    }

    // Frequency (log) -> x, dB -> y.
    let log_min = MIN_FREQ.ln();
    let log_max = nyquist.ln();
    let x_of = |freq: f32| -> f32 {
        let t = (freq.max(MIN_FREQ).ln() - log_min) / (log_max - log_min);
        rect.left() + rect.width() * t.clamp(0.0, 1.0)
    };
    let y_of = |db: f32| -> f32 {
        let t = (db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB);
        rect.bottom() - rect.height() * t.clamp(0.0, 1.0)
    };

    // Silence gate: skip curve tessellation while the last two frames were
    // silent (grid + legend still render once). State lives in egui frame
    // memory so this stays a shared-borrow function.
    let frame_max = snapshot
        .magnitudes_db
        .iter()
        .fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let silent = frame_max < BOTTOM_DB + 1.0;
    let silent_flag_id = egui::Id::new("akifx_spectrum_silent");
    let was_silent = ui.memory(|m| m.data.get_temp::<bool>(silent_flag_id).unwrap_or(false));
    let redraw_curves = !(silent && was_silent);
    ui.memory_mut(|m| m.data.insert_temp(silent_flag_id, silent));

    // ── Grid ─────────────────────────────────────────────────────────────
    let label_font = theme::mono(9.0);
    for &freq in &[100.0f32, 1000.0, 10000.0] {
        if freq >= nyquist {
            continue;
        }
        let x = x_of(freq);
        // Bold labeled columns
        painter.line_segment(
            [egui::pos2(x, rect.top() + 14.0), egui::pos2(x, rect.bottom() - 14.0)],
            Stroke::new(1.0, theme::with_alpha(p.text_on_contrast, 0.22)),
        );
        let text = if freq >= 1000.0 {
            format!("{}k", freq / 1000.0)
        } else {
            format!("{}", freq)
        };
        painter.text(
            egui::pos2(x + 3.0, rect.bottom() - 8.0),
            egui::Align2::LEFT_CENTER,
            text,
            label_font.clone(),
            theme::with_alpha(p.text_on_contrast, 0.45),
        );
    }
    for decade in [10.0f32, 100.0, 1000.0, 10000.0] {
        for mult in [2.0f32, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0] {
            let freq = decade * mult;
            if freq < MIN_FREQ || freq >= nyquist || freq == 100.0 || freq == 1000.0 || freq == 10000.0 {
                continue;
            }
            let x = x_of(freq);
            painter.line_segment(
                [egui::pos2(x, rect.top() + 14.0), egui::pos2(x, rect.bottom() - 14.0)],
                Stroke::new(0.5, on_contrast),
            );
        }
    }
    // dB rows every 20 dB
    for db in [-20.0f32, -40.0, -60.0, -80.0] {
        let y = y_of(db);
        painter.line_segment(
            [egui::pos2(rect.left() + 4.0, y), egui::pos2(rect.right() - 4.0, y)],
            Stroke::new(0.5, on_contrast),
        );
        painter.text(
            egui::pos2(rect.left() + 6.0, y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{}", db as i32),
            label_font.clone(),
            theme::with_alpha(p.text_on_contrast, 0.35),
        );
    }
    // 0 dB top reference (emphasized)
    painter.line_segment(
        [egui::pos2(rect.left() + 4.0, rect.top() + 3.0), egui::pos2(rect.right() - 4.0, rect.top() + 3.0)],
        Stroke::new(1.0, theme::with_alpha(p.text_on_contrast, 0.30)),
    );

    // ── Curves (per-pixel-column max pooling, ≤420 points) ───────────────
    if redraw_curves {
        let bins = &snapshot.magnitudes_db;
        let width_per_bin = nyquist * 2.0 / snapshot.window_size.max(1) as f32;
        let n_cols = ((rect.width() as usize).min(MAX_POINTS)).max(2);
        let mut pooled: Vec<f32> = vec![BOTTOM_DB; n_cols];
        for (bin, &db) in bins.iter().enumerate().skip(1) {
            let freq = (bin as f32 + 0.5) * width_per_bin;
            if freq < MIN_FREQ {
                continue;
            }
            let col = (((x_of(freq) - rect.left()) / rect.width()) * n_cols as f32) as usize;
            if let Some(slot) = pooled.get_mut(col.min(n_cols - 1)) {
                if db > *slot {
                    *slot = db;
                }
            }
        }

        // Filled area under the spectrum: vertical ramp fade (grad_b 35% at
        // the curve → transparent at the bottom).
        let mut mesh = egui::Mesh::default();
        let base_y = rect.bottom() - 1.0;
        for (col, &db) in pooled.iter().enumerate() {
            let x = rect.left() + (col as f32 + 0.5) / n_cols as f32 * rect.width();
            let y = y_of(db.max(BOTTOM_DB));
            mesh.colored_vertex(egui::pos2(x, y), theme::with_alpha(p.grad_b, 0.35));
            mesh.colored_vertex(egui::pos2(x, base_y), theme::with_alpha(p.grad_b, 0.0));
        }
        for col in 0..(n_cols - 1) {
            let a = (col * 2) as u32;
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
        painter.add(egui::Shape::mesh(mesh));

        // Spectrum stroke on top of the fill
        let points: Vec<egui::Pos2> = pooled
            .iter()
            .enumerate()
            .map(|(col, &db)| {
                let x = rect.left() + (col as f32 + 0.5) / n_cols as f32 * rect.width();
                egui::pos2(x, y_of(db.max(BOTTOM_DB)))
            })
            .collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, Stroke::new(1.5, p.grad_b)));
        }

        // Threshold curve overlay (warm accent), same pooling
        let thresholds = &snapshot.thresholds_db;
        if !thresholds.is_empty() {
            let mut pooled_t: Vec<f32> = vec![0.0; n_cols];
            let mut filled: Vec<bool> = vec![false; n_cols];
            for (bin, &db) in thresholds.iter().enumerate() {
                let freq = (bin as f32 + 0.5) * width_per_bin;
                if freq < MIN_FREQ {
                    continue;
                }
                let col = (((x_of(freq) - rect.left()) / rect.width()) * n_cols as f32) as usize;
                if col < n_cols {
                    if !filled[col] || db > pooled_t[col] {
                        pooled_t[col] = db;
                        filled[col] = true;
                    }
                }
            }
            let t_points: Vec<egui::Pos2> = pooled_t
                .iter()
                .enumerate()
                .filter(|(col, _)| filled[*col])
                .map(|(col, &db)| {
                    let x = rect.left() + (col as f32 + 0.5) / n_cols as f32 * rect.width();
                    egui::pos2(x, y_of(db.clamp(BOTTOM_DB, TOP_DB)))
                })
                .collect();
            if t_points.len() >= 2 {
                painter.add(egui::Shape::line(t_points, Stroke::new(1.5, p.accent_tertiary)));
            }
        }
    } else {
        // Keep repainting occasionally so the gate releases as soon as audio
        // returns (the analyzer republishes every few blocks).
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }

    // ── Legend + neon edge ───────────────────────────────────────────────
    paint::neon_edge(ui.painter(), rect);
    let legend_y = rect.top() + 9.0;
    // Right-aligned text so the labels never clip against the panel edge.
    painter.text(
        egui::pos2(rect.right() - 12.0, legend_y),
        egui::Align2::RIGHT_CENTER,
        "THRESHOLD",
        label_font.clone(),
        theme::with_alpha(p.text_on_contrast, 0.55),
    );
    painter.circle_filled(egui::pos2(rect.right() - 78.0, legend_y), 3.0, p.accent_tertiary);
    painter.text(
        egui::pos2(rect.right() - 86.0, legend_y),
        egui::Align2::RIGHT_CENTER,
        "SPECTRUM",
        label_font.clone(),
        theme::with_alpha(p.text_on_contrast, 0.55),
    );
    painter.circle_filled(egui::pos2(rect.right() - 152.0, legend_y), 3.0, p.grad_b);

    // Frame
    painter.rect_stroke(
        rect,
        theme::radius::MD,
        Stroke::new(1.0, on_contrast),
        egui::StrokeKind::Inside,
    );
}

// ---------------------------------------------------------------------------
// Peak meters
// ---------------------------------------------------------------------------

/// Draw the L/R output peak meters on a dB-linear scale (−60…+6 dBFS, 0 dB
/// reference line, red over-0 segment, latched clip bar). Peaks come from the
/// audio thread's atomics; the shown value attacks instantly and decays
/// smoothly in the GUI. Click the meter to clear the clip latch.
fn meter_pair(ui: &mut egui::Ui, state: &mut EditorState) {
    let p = pal();
    let peaks = [state.peak_l.load(Ordering::Relaxed), state.peak_r.load(Ordering::Relaxed)];
    let shown = [state.meter_db_l, state.meter_db_r];

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(210.0, 36.0), egui::Sense::click());
    if resp.clicked() {
        state.clip_latched = false;
    }

    // Update decay state (fast attack, fixed per-frame release).
    for (i, peak) in peaks.iter().enumerate() {
        let target_db = if *peak <= 1.0e-5 {
            METER_MIN_DB
        } else {
            (20.0 * peak.log10()).clamp(METER_MIN_DB, METER_MAX_DB)
        };
        let next = if target_db > shown[i] {
            target_db
        } else {
            (shown[i] - METER_DECAY_DB_PER_FRAME).max(target_db)
        };
        if i == 0 {
            state.meter_db_l = next;
        } else {
            state.meter_db_r = next;
        }
    }
    if peaks[0] > 1.0 || peaks[1] > 1.0 {
        state.clip_latched = true;
    }

    let painter = ui.painter_at(rect);
    let db_x = |db: f32| -> f32 {
        let t = (db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB);
        rect.left() + 22.0 + t.clamp(0.0, 1.0) * (rect.width() - 22.0 - 44.0)
    };

    for i in 0..2 {
        let bar_db = if i == 0 { state.meter_db_l } else { state.meter_db_r };
        let y = rect.top() + 5.0 + i as f32 * 13.0;
        let h = 9.0f32;
        let track = Rect::from_min_max(
            Pos2::new(db_x(METER_MIN_DB), y),
            Pos2::new(db_x(METER_MAX_DB), y + h),
        );

        // Track
        painter.rect_filled(track, 2.0, p.surface_contrast);

        // Fill up to the shown level: ramp gradient below 0 dB, red above.
        let shown_x = db_x(bar_db);
        if shown_x > track.left() + 0.5 {
            let zero_x = db_x(0.0);
            let fill_clip = Rect::from_min_max(
                track.min,
                Pos2::new(shown_x.min(zero_x).max(track.left()), track.bottom()),
            );
            if fill_clip.width() > 0.5 {
                let clip_painter = painter.clone().with_clip_rect(fill_clip);
                paint::gradient_band(&clip_painter, track, p.grad_a, p.grad_b);
            }
            if bar_db > 0.0 && shown_x > zero_x {
                let hot = Rect::from_min_max(Pos2::new(zero_x, y), Pos2::new(shown_x, y + h));
                painter.rect_filled(hot, 2.0, p.error);
            }
        }

        // Channel letter + numeric readout
        painter.text(
            Pos2::new(rect.left() + 8.0, y + h * 0.5),
            egui::Align2::LEFT_CENTER,
            if i == 0 { "L" } else { "R" },
            theme::mono_medium(9.5),
            p.text_secondary,
        );
        let db_text = if bar_db <= METER_MIN_DB + 0.5 {
            "-\u{221e}".to_owned()
        } else {
            format!("{:+.1}", bar_db)
        };
        painter.text(
            Pos2::new(track.right() + 6.0, y + h * 0.5),
            egui::Align2::LEFT_CENTER,
            db_text,
            theme::mono(9.5),
            p.text_secondary,
        );
    }

    // 0 dB reference line (drawn after bars so it stays visible)
    let zero_x = db_x(0.0);
    painter.line_segment(
        [Pos2::new(zero_x, rect.top() + 4.0), Pos2::new(zero_x, rect.bottom() - 4.0)],
        Stroke::new(1.0, theme::with_alpha(p.text_primary, 0.30)),
    );
    painter.text(
        Pos2::new(zero_x, rect.top() + 2.5),
        egui::Align2::CENTER_BOTTOM,
        "0",
        theme::mono(8.5),
        theme::with_alpha(p.text_primary, 0.35),
    );

    // Latched clip bar across the top
    if state.clip_latched {
        let clip_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 22.0, rect.top() + 0.5),
            Pos2::new(rect.right() - 44.0, rect.top() + 2.5),
        );
        painter.rect_filled(clip_rect, 1.0, p.error);
    }

    resp.on_hover_text(
        "Output peaks after processing (dBFS, \u{2212}60\u{2026}+6). Click to clear the clip indicator.",
    );
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn render_footer(ui: &mut egui::Ui, setter: &ParamSetter, state: &mut EditorState) {
    let p = pal();
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(theme::space::S4);

        ui.label(theme::eyebrow("Master"));
        ui.add_space(theme::space::S3);

        // Global bypass: one control for the whole chain (bit-identical
        // passthrough, automatable by the host). Engaged = warning tint plus
        // the single shimmer edge on screen (the chain is "alive but
        // muted").
        let bypassed = state.params.global_bypass.value();
        let (bp_rect, bp_resp) =
            ui.allocate_exact_size(Vec2::new(158.0, 28.0), egui::Sense::click());
        let bp_painter = ui.painter_at(bp_rect);
        bp_painter.rect_filled(
            bp_rect,
            theme::radius::SM,
            if bypassed {
                theme::with_alpha(p.warning, 0.12)
            } else {
                Color32::TRANSPARENT
            },
        );
        bp_painter.rect_stroke(
            bp_rect,
            theme::radius::SM,
            Stroke::new(
                1.0,
                if bypassed { p.warning } else { p.border },
            ),
            egui::StrokeKind::Inside,
        );
        if bypassed {
            let phase = (ui.input(|i| i.time) % 5.0) / 5.0;
            paint::shimmer_edge(&bp_painter, bp_rect, phase as f32);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        bp_painter.text(
            bp_rect.center(),
            egui::Align2::CENTER_CENTER,
            if bypassed { "GLOBAL BYPASS \u{00b7} ON" } else { "GLOBAL BYPASS \u{00b7} OFF" },
            theme::mono_medium(11.0),
            if bypassed { p.warning } else { p.text_primary },
        );
        if bp_resp
            .on_hover_text("Bypass the entire chain (bit-identical passthrough). Automatable.")
            .clicked()
        {
            setter.begin_set_parameter(&state.params.global_bypass);
            setter.set_parameter(&state.params.global_bypass, !bypassed);
            setter.end_set_parameter(&state.params.global_bypass);
            ui.ctx().request_repaint();
        }

        ui.add_space(theme::space::S4);

        // Master gain slider (same Aki slider as the param panel)
        ui.allocate_ui_with_layout(
            Vec2::new(230.0, 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let gain_resp = widgets::float_slider(ui, setter, &state.params.gain.gain);
                gain_resp.on_hover_text("Master output gain");
            },
        );

        ui.add_space(theme::space::S4);

        // Stereo output peak meters (fed by the audio thread's atomics).
        meter_pair(ui, state);

        // Right-aligned cluster: UI zoom cycle, About.
        // (Parent scope — the only interactable zone verified reliable.)
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(theme::space::S4);

            // About ghost button
            let about_btn = ui.add(
                egui::Button::new(
                    RichText::new("About").font(theme::body(11.0)).color(p.text_secondary),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.0, p.border)),
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
                        .font(theme::mono_medium(11.0))
                        .color(p.text_secondary),
                )
                .fill(theme::with_alpha(p.text_primary, 0.06))
                .stroke(Stroke::new(1.0, p.border))
                .min_size(Vec2::new(56.0, 22.0)),
            );
            if zoom_btn
                .on_hover_text("UI zoom \u{2014} click to cycle 60\u{2013}200%")
                .clicked()
            {
                *state.params.ui_zoom.lock() = zoom_step(z, 1);
                ui.ctx().request_repaint();
            }
        });
    });
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
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());
        let identity: Vec<usize> = (0..11).collect();

        let ordered = ordered_entries(&entries, &identity);
        assert_eq!(ordered.len(), 11);
        for (i, entry) in ordered.iter().enumerate() {
            assert_eq!(entry.name, entries[i].name);
        }
    }

    /// ordered_entries with a custom permutation reorders correctly.
    #[test]
    fn ordered_entries_custom_permutation() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());
        // Reverse order: last module first.
        let reversed: Vec<usize> = (0..11).rev().collect();

        let ordered = ordered_entries(&entries, &reversed);
        assert_eq!(ordered.len(), 11);
        assert_eq!(ordered[0].name, "Safety Limiter");
        assert_eq!(ordered[10].name, "Sine Generator");
    }

    /// ordered_entries filters out-of-range indices silently.
    #[test]
    fn ordered_entries_filters_invalid_indices() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());
        let order_with_gaps = vec![0, 99, 1, 200, 2];

        let ordered = ordered_entries(&entries, &order_with_gaps);
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].name, "Sine Generator");
        assert_eq!(ordered[1].name, "Soft Vacuum");
        assert_eq!(ordered[2].name, "Crisp");
    }

    // ── position_of_module tests ────────────────────────────────────────

    /// Identity order: position_of_module(i) == i for all i.
    #[test]
    fn position_of_module_identity() {
        let order: Vec<usize> = (0..11).collect();
        for i in 0..11 {
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
        // order[0]=10, order[1]=9, ..., order[10]=0  (reversed)
        let order: Vec<usize> = (0..11).rev().collect();
        // Module 10 is at position 0
        assert_eq!(position_of_module(&order, 10), Some(0));
        // Module 0 is at position 10
        assert_eq!(position_of_module(&order, 0), Some(10));
        // Module 5 is at position 5 (center stays)
        assert_eq!(position_of_module(&order, 5), Some(5));
    }

    /// Non-contiguous permutation with a gap.
    #[test]
    fn position_of_module_sparse_permutation() {
        let order = vec![5, 0, 8, 3];
        assert_eq!(position_of_module(&order, 5), Some(0));
        assert_eq!(position_of_module(&order, 0), Some(1));
        assert_eq!(position_of_module(&order, 8), Some(2));
        assert_eq!(position_of_module(&order, 3), Some(3));
    }

    /// Module index not in order → None.
    #[test]
    fn position_of_module_not_found() {
        let order: Vec<usize> = (0..11).collect();
        assert_eq!(position_of_module(&order, 99), None);
    }

    // ── Selection semantics tests ──────────────────────────────────────
    //
    // These test that `EditorState.selected` stores MODULE INDEX (not
    // position), and that after a reorder the selected module identity
    // is preserved.

    /// After reorder, selected module identity is preserved.
    ///
    /// Simulates: select module 9 ("Gain"), then reverse the order.
    /// selected should still point at module 9 (now at position 1).
    #[test]
    fn selected_preserves_module_identity_after_reorder() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());

        // selected = module index 9 (Gain)
        let selected_module: usize = 9;
        let order_after_reorder: Vec<usize> = (0..11).rev().collect();

        // Resolve the selected entry via module index
        let entry = &entries[selected_module];
        assert_eq!(entry.name, "Gain");

        // Find position in the new order
        let pos = position_of_module(&order_after_reorder, selected_module)
            .expect("module 9 must be in order");
        let visual = ordered_entries(&entries, &order_after_reorder);
        assert_eq!(visual[pos].name, "Gain");
    }

    /// selected=0 always points at the first module, regardless of order.
    #[test]
    fn selected_zero_follows_module_not_position() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());

        // Reversed order: module 10 (Safety Limiter) is at position 0
        let reversed: Vec<usize> = (0..11).rev().collect();
        let selected_module: usize = 0; // Sine Generator

        let pos = position_of_module(&reversed, selected_module).unwrap();
        let visual = ordered_entries(&entries, &reversed);
        // Position 0 in visual is Safety Limiter; Sine Generator is at position 10
        assert_eq!(visual[pos].name, "Sine Generator");
        assert_eq!(pos, 10);
    }

    /// BUG REPRODUCTION: selected as position (old code) points at wrong
    /// module after reorder; selected as module index (new code) is correct.
    #[test]
    fn selected_as_position_fails_after_reorder() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());

        // Before reorder: identity order, user clicks position 9 → Gain
        let identity: Vec<usize> = (0..11).collect();
        let visual_before = ordered_entries(&entries, &identity);
        assert_eq!(visual_before[9].name, "Gain");

        // OLD (buggy) semantics: selected = 9 (position)
        let selected_position: usize = 9;

        // After reorder: reversed. Position 9 is now module 1 (Soft Vacuum)
        let reversed: Vec<usize> = (0..11).rev().collect();
        let visual_after = ordered_entries(&entries, &reversed);

        // BUG: position 9 in reversed order ≠ Gain
        assert_ne!(
            visual_after[selected_position].name, "Gain",
            "position-based selection breaks after reorder"
        );

        // NEW (correct) semantics: selected = 9 (module index)
        let selected_module: usize = 9;
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
        let chain = crate::create_default_chain(&params);
        let entries = state::build_ui_entries(&params, chain.modules());
        let order: Vec<usize> = (0..11).rev().collect();

        // Select module 9 (Gain) - stored as module_idx, not position
        let selected_module: usize = 9;
        let pos = position_of_module(&order, selected_module)
            .expect("module 9 must be in order");
        let visual = ordered_entries(&entries, &order);
        let entry = visual.get(pos).expect("position must be valid");

        assert_eq!(entry.name, "Gain");
    }

    /// Param panel: absent module_idx falls back safely (None path).
    #[test]
    fn param_panel_handles_absent_module_gracefully() {
        let params = AkiFxParams::default();
        let chain = crate::create_default_chain(&params);
        let _entries = state::build_ui_entries(&params, chain.modules());
        let order: Vec<usize> = (0..11).collect();

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

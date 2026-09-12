//! GUI state types for module list rendering.
//!
//! Provides [`ModuleUiEntry`] — a snapshot of one module's metadata for the
//! sidebar — and [`build_ui_entries`] to construct the list from the umbrella
//! params.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nih_plug::prelude::Params;

use crate::AkiFxParams;

/// One module's metadata for the sidebar / drag-to-reorder GUI.
pub struct ModuleUiEntry {
    /// Human-readable module name (must match the module's `AkiFxModule::name()` return value).
    pub name: &'static str,
    /// Latency introduced by this module in samples (static snapshot).
    pub latency_samples: u64,
    /// Shared bypass flag — the same `Arc<AtomicBool>` the audio module reads.
    pub bypass: Arc<AtomicBool>,
    /// The `id_prefix` used in the `#[nested(id_prefix = "...")]` attribute on AkiFxParams.
    pub id_prefix: &'static str,
    /// Clone of this module's params Arc, upcast to `dyn Params`.
    pub params: Arc<dyn Params>,
}

// ── Latency lookup ──────────────────────────────────────────────────────────

/// Static latency per module index, matching `create_default_chain` push order.
/// Only PubertySimulator (index 13) introduces latency (2048 samples).
const LATENCY_BY_INDEX: [u64; 21] = [
    0,    // 0  sine_gen
    0,    // 1  midi_inverter
    0,    // 2  poly_mod_synth
    0,    // 3  playground
    0,    // 4  soft_vacuum
    0,    // 5  crisp
    0,    // 6  spectral_gate
    0,    // 7  frequency_shift
    0,    // 8  frequency_magnet
    0,    // 9  bin_scrambler
    0,    // 10 morph
    0,    // 11 phase_lock
    0,    // 12 sinusoidal_shaped_filter
    2048, // 13 puberty_simulator
    0,    // 14 crossover
    0,    // 15 diopser
    0,    // 16 loudness_war_winner
    0,    // 17 spectral_compressor
    0,    // 18 buffr_glitch
    0,    // 19 gain
    0,    // 20 safety_limiter
];

// ── build_ui_entries ────────────────────────────────────────────────────────

/// Build one [`ModuleUiEntry`] per module, in the same order `create_default_chain` pushes them.
///
/// Each entry gets:
/// - a fresh `Arc<AtomicBool>` bypass flag (wired to the module during chain construction)
/// - a clone of the concrete params `Arc` upcast to `Arc<dyn Params>`
/// - the static latency snapshot
///
/// The returned `Vec` always has exactly 21 elements.
pub fn build_ui_entries(params: &AkiFxParams) -> Vec<ModuleUiEntry> {
    // Macro: create one entry per module. Names MUST match create_default_chain order.
    macro_rules! entry {
        ($field:ident, $name:expr, $id_prefix:expr) => {{
            ModuleUiEntry {
                name: $name,
                latency_samples: 0, // filled below
                bypass: Arc::new(AtomicBool::new(true)),
                id_prefix: $id_prefix,
                params: params.$field.clone() as Arc<dyn Params>,
            }
        }};
    }

    let mut entries = vec![
        entry!(sine_gen,              "Sine Generator",          "sine_gen"),
        entry!(midi_inverter,         "MIDI Inverter",           "midi_inverter"),
        entry!(poly_mod_synth,        "Poly Mod Synth",          "poly_mod_synth"),
        entry!(playground,            "Playground",              "playground"),
        entry!(soft_vacuum,           "Soft Vacuum",             "soft_vacuum"),
        entry!(crisp,                 "Crisp",                   "crisp"),
        entry!(spectral_gate,         "Spectral Gate",           "spectral_gate"),
        entry!(frequency_shift,       "Frequency Shift",         "frequency_shift"),
        entry!(frequency_magnet,      "Frequency Magnet",        "frequency_magnet"),
        entry!(bin_scrambler,         "Bin Scrambler",           "bin_scrambler"),
        entry!(morph,                 "Morph",                   "morph"),
        entry!(phase_lock,            "Phase Lock",              "phase_lock"),
        entry!(sinusoidal_shaped_filter, "Sinusoidal Shaped Filter", "sinusoidal_shaped_filter"),
        entry!(puberty_simulator,     "Puberty Simulator",       "puberty_simulator"),
        entry!(crossover,             "Crossover",               "crossover"),
        entry!(diopser,               "Diopser",                 "diopser"),
        entry!(loudness_war_winner,   "Loudness War Winner",     "loudness_war_winner"),
        entry!(spectral_compressor,   "Spectral Compressor",     "spectral_compressor"),
        entry!(buffr_glitch,          "Buffr Glitch",            "buffr_glitch"),
        entry!(gain,                  "Gain",                    "gain"),
        entry!(safety_limiter,        "Safety Limiter",          "safety_limiter"),
    ];

    // Fill in static latency values.
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.latency_samples = LATENCY_BY_INDEX[i];
    }

    entries
}

/// Wire each UI entry's bypass flag to the corresponding live module's flag.
///
/// `build_ui_entries` cannot know the modules' bypass flags at construction
/// time (the chain owns them), so entries start with placeholder flags. This
/// function replaces them positionally: `entries[i]` must correspond to
/// `modules[i]` (both follow the same construction order). After wiring,
/// toggling an entry's bypass in the GUI directly controls whether the audio
/// thread processes that module.
///
 /// # Panics (debug) / no-op (release)
 ///
 /// In debug builds this asserts equal lengths; in release builds it wires
 /// only the overlapping prefix, leaving any surplus untouched.
pub fn wire_bypass_flags(
    entries: &mut [ModuleUiEntry],
    modules: &[Box<dyn crate::modules::AkiFxModule>],
) {
    debug_assert_eq!(
        entries.len(),
        modules.len(),
        "UI entries and chain modules must align 1:1"
    );
    for (entry, module) in entries.iter_mut().zip(modules.iter()) {
        entry.bypass = module.bypass_flag().clone();
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ui_entries_returns_21_with_correct_names() {
        let params = AkiFxParams::default();
        let entries = build_ui_entries(&params);

        assert_eq!(entries.len(), 21, "Expected exactly 21 UI entries");

        let expected_names = [
            "Sine Generator",
            "MIDI Inverter",
            "Poly Mod Synth",
            "Playground",
            "Soft Vacuum",
            "Crisp",
            "Spectral Gate",
            "Frequency Shift",
            "Frequency Magnet",
            "Bin Scrambler",
            "Morph",
            "Phase Lock",
            "Sinusoidal Shaped Filter",
            "Puberty Simulator",
            "Crossover",
            "Diopser",
            "Loudness War Winner",
            "Spectral Compressor",
            "Buffr Glitch",
            "Gain",
            "Safety Limiter",
        ];

        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.name, expected_names[i],
                "Entry {i} name mismatch: expected {:?}, got {:?}",
                expected_names[i], entry.name
            );
        }
    }

    #[test]
    fn build_ui_entries_latency_matches_expected() {
        let params = AkiFxParams::default();
        let entries = build_ui_entries(&params);

        // Only Puberty Simulator (index 13) has latency
        for (i, entry) in entries.iter().enumerate() {
            if i == 13 {
                assert_eq!(entry.latency_samples, 2048);
            } else {
                assert_eq!(entry.latency_samples, 0, "Entry {i} ({}) should have 0 latency", entry.name);
            }
        }
    }

    #[test]
    fn build_ui_entries_id_prefixes_match_field_names() {
        let params = AkiFxParams::default();
        let entries = build_ui_entries(&params);

        let expected_prefixes = [
            "sine_gen", "midi_inverter", "poly_mod_synth", "playground",
            "soft_vacuum", "crisp", "spectral_gate", "frequency_shift",
            "frequency_magnet", "bin_scrambler", "morph", "phase_lock",
            "sinusoidal_shaped_filter", "puberty_simulator", "crossover",
            "diopser", "loudness_war_winner", "spectral_compressor",
            "buffr_glitch", "gain", "safety_limiter",
        ];

        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.id_prefix, expected_prefixes[i],
                "Entry {i} id_prefix mismatch"
            );
        }
    }

    /// REGRESSION (Oracle-found): GUI entry bypass flags MUST be the SAME
    /// Arc<AtomicBool> as the live modules'. Before the fix, build_ui_entries
    /// created orphan flags and power toggles were cosmetic-only.
    ///
    /// After T2 (defaults all-off), both entry placeholders and chain modules
    /// start with `AtomicBool::new(true)` — but they are independent Arcs.
    /// This test proves exclusivity pre-wiring and shared identity post-wiring.
    #[test]
    fn wire_bypass_flags_shares_module_flags_end_to_end() {
        let params = Arc::new(crate::AkiFxParams::default());
        let chain = crate::create_default_chain(&params);
        let mut entries = build_ui_entries(&params);

        // ── Pre-wiring: entry and module flags are DIFFERENT Arcs ──────
        assert!(
            !Arc::ptr_eq(
                &entries[19].bypass,
                chain.modules()[19].bypass_flag()
            ),
            "pre-wiring: entry bypass must be a different Arc than module's"
        );

        // Toggle the entry's placeholder — module must NOT change.
        entries[19].bypass.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            chain.modules()[19].is_bypassed(),
            "pre-wiring: module bypass must remain true regardless of entry"
        );

        // ── Post-wiring: entry bypass IS the module's flag ─────────────
        wire_bypass_flags(&mut entries, chain.modules());
        assert!(
            std::sync::Arc::ptr_eq(
                &entries[19].bypass,
                chain.modules()[19].bypass_flag()
            ),
            "post-wiring: entry bypass flag must BE the module's flag"
        );

        // store(false) → module is no longer bypassed
        entries[19].bypass.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            !chain.modules()[19].is_bypassed(),
            "GUI toggle to active must gate audio-thread processing"
        );

        // store(true) → module is bypassed again
        entries[19].bypass.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            chain.modules()[19].is_bypassed(),
            "GUI toggle to bypassed must gate audio-thread processing"
        );
    }
}

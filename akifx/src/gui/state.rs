//! GUI state types for module list rendering.
//!
//! Provides [`ModuleUiEntry`] — a snapshot of one module's metadata for the
//! sidebar — and [`build_ui_entries`] to construct the list from the umbrella
//! params and the live chain modules.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nih_plug::prelude::Params;

use crate::AkiFxParams;

/// One module's metadata for the sidebar / drag-to-reorder GUI.
pub struct ModuleUiEntry {
    /// Human-readable module name (must match the module's `AkiFxModule::name()` return value).
    pub name: &'static str,
    /// Latency introduced by this module in samples, read live from the
    /// module at editor creation.
    pub latency_samples: u64,
    /// Shared bypass flag — the same `Arc<AtomicBool>` the audio module reads.
    pub bypass: Arc<AtomicBool>,
    /// The `id_prefix` used in the `#[nested(id_prefix = "...")]` attribute on AkiFxParams.
    pub id_prefix: &'static str,
    /// Clone of this module's params Arc, upcast to `dyn Params`.
    pub params: Arc<dyn Params>,
}

// ── build_ui_entries ────────────────────────────────────────────────────────

/// Build one [`ModuleUiEntry`] per module, in the same order `create_default_chain` pushes them.
///
/// Each entry gets:
/// - a fresh `Arc<AtomicBool>` bypass flag (wired to the module during chain construction)
/// - a clone of the concrete params `Arc` upcast to `Arc<dyn Params>`
/// - the module's current latency (`latency_samples()`), so latency badges
///   reflect reality. (A previous hand-maintained static table claimed only
///   Puberty Simulator had latency, under-reporting by ~10×2048 samples.)
///
/// The returned `Vec` always has exactly 21 elements.
pub fn build_ui_entries(
    params: &AkiFxParams,
    modules: &[Box<dyn crate::modules::AkiFxModule>],
) -> Vec<ModuleUiEntry> {
    // Macro: create one entry per module. Names MUST match create_default_chain order.
    macro_rules! entry {
        ($idx:expr, $field:ident, $name:expr, $id_prefix:expr) => {{
            ModuleUiEntry {
                name: $name,
                latency_samples: modules
                    .get($idx)
                    .map(|m| m.latency_samples())
                    .unwrap_or(0),
                bypass: Arc::new(AtomicBool::new(true)),
                id_prefix: $id_prefix,
                params: params.$field.clone() as Arc<dyn Params>,
            }
        }};
    }

    let entries = vec![
        entry!(0, sine_gen,              "Sine Generator",          "sine_gen"),
        entry!(1, midi_inverter,         "MIDI Inverter",           "midi_inverter"),
        entry!(2, poly_mod_synth,        "Poly Mod Synth",          "poly_mod_synth"),
        entry!(3, playground,            "Playground",              "playground"),
        entry!(4, soft_vacuum,           "Soft Vacuum",             "soft_vacuum"),
        entry!(5, crisp,                 "Crisp",                   "crisp"),
        entry!(6, spectral_gate,         "Spectral Gate",           "spectral_gate"),
        entry!(7, frequency_shift,       "Frequency Shift",         "frequency_shift"),
        entry!(8, frequency_magnet,      "Frequency Magnet",        "frequency_magnet"),
        entry!(9, bin_scrambler,         "Bin Scrambler",           "bin_scrambler"),
        entry!(10, morph,                "Morph",                   "morph"),
        entry!(11, phase_lock,           "Phase Lock",              "phase_lock"),
        entry!(12, sinusoidal_shaped_filter, "Sinusoidal Shaped Filter", "sinusoidal_shaped_filter"),
        entry!(13, puberty_simulator,    "Puberty Simulator",       "puberty_simulator"),
        entry!(14, crossover,            "Crossover",               "crossover"),
        entry!(15, diopser,              "Diopser",                 "diopser"),
        entry!(16, loudness_war_winner,  "Loudness War Winner",     "loudness_war_winner"),
        entry!(17, spectral_compressor,  "Spectral Compressor",     "spectral_compressor"),
        entry!(18, buffr_glitch,         "Buffr Glitch",            "buffr_glitch"),
        entry!(19, gain,                 "Gain",                    "gain"),
        entry!(20, safety_limiter,       "Safety Limiter",          "safety_limiter"),
    ];

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

/// Mirror the entries' live bypass flags into the persisted `module_enabled`
/// state so the host saves which modules were lit. Call after every GUI
/// power toggle. The audio thread never touches this state — only the GUI
/// (writes) and nih-plug's state serialization (reads) do.
pub fn sync_persisted_enabled(params: &AkiFxParams, entries: &[ModuleUiEntry]) {
    let mut enabled = params.module_enabled.lock();
    enabled.clear();
    enabled.extend(
        entries
            .iter()
            .map(|e| !e.bypass.load(Ordering::Relaxed)),
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ui_entries_returns_21_with_correct_names() {
        let params = Arc::new(AkiFxParams::default());
        let chain = crate::create_default_chain(&params);
        let entries = build_ui_entries(&params, chain.modules());

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
    fn build_ui_entries_latency_matches_live_modules() {
        let params = Arc::new(AkiFxParams::default());
        let mut chain = crate::create_default_chain(&params);
        chain.initialize_all(44100.0, 512);
        let entries = build_ui_entries(&params, chain.modules());

        // Latencies must mirror the live modules (no static table anymore).
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.latency_samples,
                chain.modules()[i].latency_samples(),
                "Entry {i} ({}) latency must match the live module",
                entry.name
            );
        }
        // Sanity: with all engines built, the spectral modules report their
        // engine latency (2048), while Puberty Simulator defaults to a
        // smaller 1024-sample window.
        assert_eq!(entries[6].latency_samples, 2048, "Spectral Gate");
        assert_eq!(entries[13].latency_samples, 1024, "Puberty Simulator");
        assert_eq!(entries[17].latency_samples, 2048, "Spectral Compressor");
        assert_eq!(entries[19].latency_samples, 0, "Gain");
    }

    #[test]
    fn build_ui_entries_id_prefixes_match_field_names() {
        let params = Arc::new(AkiFxParams::default());
        let chain = crate::create_default_chain(&params);
        let entries = build_ui_entries(&params, chain.modules());

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
        let mut entries = build_ui_entries(&params, chain.modules());

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

    /// Persisted enabled state must mirror the entries' bypass flags, so a
    /// saved session restores which modules were lit.
    #[test]
    fn sync_persisted_enabled_mirrors_bypass_flags() {
        let params = Arc::new(AkiFxParams::default());
        let chain = crate::create_default_chain(&params);
        let mut entries = build_ui_entries(&params, chain.modules());
        wire_bypass_flags(&mut entries, chain.modules());

        // All off by default.
        sync_persisted_enabled(&params, &entries);
        assert_eq!(
            *params.module_enabled.lock(),
            vec![false; 21],
            "all modules start bypassed"
        );

        // Light two modules → persisted state flips those entries.
        entries[19].bypass.store(false, Ordering::Relaxed); // Gain
        entries[20].bypass.store(false, Ordering::Relaxed); // Safety Limiter
        sync_persisted_enabled(&params, &entries);
        let enabled = params.module_enabled.lock();
        assert_eq!(enabled.len(), 21);
        assert!(!enabled[0], "untoggled module stays off");
        assert!(enabled[19], "Gain should be persisted as enabled");
        assert!(enabled[20], "Safety Limiter should be persisted as enabled");
    }
}

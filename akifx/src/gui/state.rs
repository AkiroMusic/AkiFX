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

/// Build one [`ModuleUiEntry`] per module, in registry order (the same order
/// `create_default_chain` pushes them).
///
/// Each entry gets:
/// - a fresh `Arc<AtomicBool>` bypass flag (wired to the module during chain construction)
/// - a clone of the concrete params `Arc` upcast to `Arc<dyn Params>`
/// - the module's current latency (`latency_samples()`), so latency badges
///   reflect reality
///
/// The module set comes from the registry tables — no hand-maintained list
/// here.
pub fn build_ui_entries(
    params: &AkiFxParams,
    modules: &[Box<dyn crate::modules::AkiFxModule>],
) -> Vec<ModuleUiEntry> {
    debug_assert_eq!(
        params.module_enabled.lock().len(),
        crate::MODULE_COUNT,
        "persisted enabled state must cover every module"
    );
    (0..crate::MODULE_COUNT)
        .map(|idx| ModuleUiEntry {
            name: crate::MODULE_NAMES[idx],
            latency_samples: modules.get(idx).map(|m| m.latency_samples()).unwrap_or(0),
            bypass: Arc::new(AtomicBool::new(true)),
            id_prefix: crate::MODULE_PREFIXES[idx],
            params: crate::params_for_module(params, idx)
                .expect("registry tables must cover every module"),
        })
        .collect()
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

        assert_eq!(entries.len(), 11, "Expected exactly 11 UI entries");

        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.name, crate::MODULE_NAMES[i],
                "Entry {i} name must match the registry table"
            );
            assert_eq!(
                entry.id_prefix, crate::MODULE_PREFIXES[i],
                "Entry {i} id_prefix must match the registry table"
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
        // Sanity: with all engines built, the SpectralSuite-derived modules
        // report fft_size + hop_size = 2048 + 512 (matching the C++
        // plugins), while Spectral Compressor reports its 2048 window.
        assert_eq!(entries[3].latency_samples, 2560, "Spectral Gate");
        assert_eq!(entries[5].latency_samples, 2048, "Spectral Compressor");
        assert_eq!(entries[9].latency_samples, 0, "Gain");
    }

    #[test]
    fn build_ui_entries_count_matches_registry() {
        let params = Arc::new(AkiFxParams::default());
        let chain = crate::create_default_chain(&params);
        let entries = build_ui_entries(&params, chain.modules());
        assert_eq!(entries.len(), crate::MODULE_COUNT);
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
                &entries[9].bypass,
                chain.modules()[9].bypass_flag()
            ),
            "pre-wiring: entry bypass must be a different Arc than module's"
        );

        // Toggle the entry's placeholder — module must NOT change.
        entries[9].bypass.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            chain.modules()[9].is_bypassed(),
            "pre-wiring: module bypass must remain true regardless of entry"
        );

        // ── Post-wiring: entry bypass IS the module's flag ─────────────
        wire_bypass_flags(&mut entries, chain.modules());
        assert!(
            std::sync::Arc::ptr_eq(
                &entries[9].bypass,
                chain.modules()[9].bypass_flag()
            ),
            "post-wiring: entry bypass flag must BE the module's flag"
        );

        // store(false) → module is no longer bypassed
        entries[9].bypass.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            !chain.modules()[9].is_bypassed(),
            "GUI toggle to active must gate audio-thread processing"
        );

        // store(true) → module is bypassed again
        entries[9].bypass.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            chain.modules()[9].is_bypassed(),
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
            vec![false; 11],
            "all modules start bypassed"
        );

        // Light two modules → persisted state flips those entries.
        entries[9].bypass.store(false, Ordering::Relaxed); // Gain
        entries[10].bypass.store(false, Ordering::Relaxed); // Safety Limiter
        sync_persisted_enabled(&params, &entries);
        let enabled = params.module_enabled.lock();
        assert_eq!(enabled.len(), 11);
        assert!(!enabled[0], "untoggled module stays off");
        assert!(enabled[9], "Gain should be persisted as enabled");
        assert!(enabled[10], "Safety Limiter should be persisted as enabled");
    }
}

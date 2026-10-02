//! Factory presets for the FX chain.
//!
//! A preset is a compile-time table of per-module patches: which modules to
//! enable and normalized (0..1) parameter values. Two hard rules shape the
//! data model:
//!
//! 1. Presets never touch the master chain state — the Gain module, the
//!    Safety Limiter, and the global bypass are not addressable from here.
//!    [`is_red_line_module`] is enforced by tests and re-checked defensively at
//!    apply time.
//! 2. Presets only enable/disable modules they actually patch; modules
//!    outside the recipe keep the user's power state.
//!
//! Values are stored normalized (0 = range minimum, 1 = maximum) so the same
//! table survives range edits; `Param::preview_plain` rounds integers, booleans
//! and enums through the same entry point.

use crate::{params_for_module, AkiFxParams, MODULE_COUNT};
use nih_plug::prelude::{Param, ParamPtr};

/// Whether a module index is red-line (master output chain state: Gain and
/// the Safety Limiter). Resolved by registry prefix, so a registry reorder
/// cannot silently change what presets may touch.
pub fn is_red_line_module(module: usize) -> bool {
    matches!(
        crate::MODULE_PREFIXES.get(module),
        Some(&"gain") | Some(&"safety_limiter")
    )
}

/// One module's contribution to a preset.
pub struct ModulePatch {
    /// Module index into the registry chain (see `MODULE_PREFIXES`).
    pub module: usize,
    /// Enable (un-bypass) the module as part of the recipe.
    pub enabled: bool,
    /// `(param id suffix, normalized value)` pairs. A suffix matches the
    /// module's `param_map` id exactly, or as the segment after the last `_`.
    pub params: &'static [(&'static str, f32)],
}

/// A complete factory preset.
pub struct ChainPreset {
    pub name: &'static str,
    /// One-line role description (shown as a tooltip, documented in MANUAL).
    pub role: &'static str,
    /// Init-style preset: reset every effect module to its defaults and
    /// bypass it. `patches` is still applied afterwards (Init leaves it empty).
    pub reset_all: bool,
    pub patches: &'static [ModulePatch],
}

/// The factory preset table. Ships empty for now — the bar and the apply
/// machinery are in place, so entries are a data-only change. Ordered
/// light → heavy when populated; reserve the first slot for Init.
pub static PRESETS: &[ChainPreset] = &[];

/// Destination for [`apply_to_sink`] — implemented against the host-notifying
/// `ParamSetter` in the GUI, and against direct parameter writes in tests.
pub trait PatchSink {
    fn set_module_enabled(&mut self, module: usize, enabled: bool);
    /// # Safety
    ///
    /// `ptr` must point to a valid parameter of the addressed module.
    unsafe fn set_normalized(&mut self, module: usize, id: &str, ptr: &ParamPtr, normalized: f32);
}

/// Resolve a preset's patches into sink calls. Returns the number of parameter
/// writes performed (modules that fail to resolve are skipped defensively).
pub fn apply_to_sink(preset: &ChainPreset, params: &AkiFxParams, sink: &mut impl PatchSink) -> usize {
    let mut writes = 0;

    if preset.reset_all {
        for module in 0..MODULE_COUNT {
            if is_red_line_module(module) {
                continue;
            }
            sink.set_module_enabled(module, false);
            if let Some(module_params) = params_for_module(params, module) {
                for (id, ptr, _) in module_params.param_map() {
                    let default = unsafe { default_normalized(&ptr) };
                    unsafe { sink.set_normalized(module, &id, &ptr, default) };
                    writes += 1;
                }
            }
        }
    }

    for patch in preset.patches {
        if patch.module >= MODULE_COUNT || is_red_line_module(patch.module) {
            continue;
        }
        sink.set_module_enabled(patch.module, patch.enabled);
        if let Some(module_params) = params_for_module(params, patch.module) {
            let map = module_params.param_map();
            for (suffix, normalized) in patch.params {
                if let Some((id, ptr, _)) = resolve_param(&map, suffix) {
                    unsafe { sink.set_normalized(patch.module, id, ptr, *normalized) };
                    writes += 1;
                }
            }
        }
    }

    writes
}

/// Resolve a param id suffix in a module's `param_map`: exact id first,
/// then as the last `_`-segment of a longer (prefixed) id. Exact wins so
/// `tilt` never grabs `enable_tilt`.
fn resolve_param<'a>(
    map: &'a [(String, ParamPtr, String)],
    suffix: &str,
) -> Option<&'a (String, ParamPtr, String)> {
    map.iter()
        .find(|(id, _, _)| id == suffix)
        .or_else(|| map.iter().find(|(id, _, _)| id.ends_with(&format!("_{suffix}"))))
}

/// Id-only check used by the integrity test (no pointers touched).
fn resolve_param_plain(map: &[String], suffix: &str) -> Option<usize> {
    map.iter()
        .position(|id| id == suffix)
        .or_else(|| map.iter().position(|id| id.ends_with(&format!("_{suffix}"))))
}

/// `Param::default_normalized_value` through the type-erased pointer.
///
/// # Safety
///
/// `ptr` must point to a valid parameter.
unsafe fn default_normalized(ptr: &ParamPtr) -> f32 {
    match ptr {
        ParamPtr::FloatParam(p) => (**p).default_normalized_value(),
        ParamPtr::IntParam(p) => (**p).default_normalized_value(),
        ParamPtr::BoolParam(p) => (**p).default_normalized_value(),
        ParamPtr::EnumParam(p) => (**p).default_normalized_value(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_default_chain;
    use std::collections::HashMap;

    /// Recorder sink. nih-plug deliberately keeps parameter writes
    /// (`ParamMut`) crate-private, so tests cannot mutate params outside a
    /// host — the roundtrip therefore verifies the table ↔ sink boundary:
    /// every table row resolves to exactly one sink call with the stored
    /// normalized value, and nothing else is emitted.
    struct RecordingSink {
        enabled: HashMap<usize, bool>,
        written: Vec<(usize, String, f32)>,
    }

    impl PatchSink for RecordingSink {
        fn set_module_enabled(&mut self, module: usize, enabled: bool) {
            self.enabled.insert(module, enabled);
        }

        unsafe fn set_normalized(&mut self, module: usize, id: &str, _ptr: &ParamPtr, normalized: f32) {
            self.written.push((module, id.to_owned(), normalized));
        }
    }

    #[test]
    fn table_integrity() {
        // The table ships empty; when entries return, Init must lead.
        if !PRESETS.is_empty() {
            assert!(PRESETS[0].reset_all, "Init must be the first preset");
        }

        let mut names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len(), "preset names must be unique");

        for preset in PRESETS {
            for patch in preset.patches {
                assert!(
                    patch.module < MODULE_COUNT,
                    "{}: module {} out of range",
                    preset.name,
                    patch.module
                );
                assert!(
                    !is_red_line_module(patch.module),
                    "{}: preset touches red-line module {}",
                    preset.name,
                    patch.module
                );
                for (suffix, normalized) in patch.params {
                    assert!(
                        (0.0..=1.0).contains(normalized),
                        "{}: {} value {} outside 0..1",
                        preset.name,
                        suffix,
                        normalized
                    );
                }
            }
        }
    }

    #[test]
    fn patch_ids_resolve_in_module_param_maps() {
        let params = AkiFxParams::default();
        let chain = create_default_chain(&params);
        let _ = chain;

        for preset in PRESETS {
            for patch in preset.patches {
                let module_params =
                    params_for_module(&params, patch.module).expect("module params");
                let map: Vec<String> = module_params
                    .param_map()
                    .into_iter()
                    .map(|(id, _, _)| id)
                    .collect();
                for (suffix, _) in patch.params {
                    assert!(
                        resolve_param_plain(&map, suffix).is_some(),
                        "{}: id '{}' not found in module {} (available: {})",
                        preset.name,
                        suffix,
                        patch.module,
                        map.join(", ")
                    );
                }
            }
        }
    }

    #[test]
    fn apply_roundtrip_readback() {
        let params = AkiFxParams::default();
        let chain = create_default_chain(&params);
        let _ = chain;

        for preset in PRESETS.iter().skip(1) {
            let mut sink = RecordingSink {
                enabled: HashMap::new(),
                written: Vec::new(),
            };
            let writes = apply_to_sink(preset, &params, &mut sink);
            assert!(writes > 0, "{}: no parameters written", preset.name);

            for patch in preset.patches {
                // Enable flags recorded for every patched module
                assert_eq!(
                    sink.enabled.get(&patch.module),
                    Some(&patch.enabled),
                    "{}: enable flag mismatch for module {}",
                    preset.name,
                    patch.module
                );
                for (suffix, expected) in patch.params {
                    let hits: Vec<&(usize, String, f32)> = sink
                        .written
                        .iter()
                        .filter(|(m, id, _)| *m == patch.module && id == suffix)
                        .collect();
                    assert_eq!(
                        hits.len(),
                        1,
                        "{}: id '{}' must resolve to exactly one write",
                        preset.name,
                        suffix
                    );
                    assert!(
                        (hits[0].2 - expected).abs() <= 1e-6,
                        "{}: {} wrote {} (expected {})",
                        preset.name,
                        suffix,
                        hits[0].2,
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn init_resets_defaults_and_bypasses_effects() {
        let params = AkiFxParams::default();
        let chain = create_default_chain(&params);
        let _ = chain;

        let Some(init) = PRESETS.first() else {
            return; // table ships empty; the contract holds once populated
        };
        let mut sink = RecordingSink {
            enabled: HashMap::new(),
            written: Vec::new(),
        };
        apply_to_sink(init, &params, &mut sink);

        // Every effect module bypassed exactly once
        for module in 0..MODULE_COUNT {
            if is_red_line_module(module) {
                assert!(
                    !sink.enabled.contains_key(&module),
                    "Init must not touch red-line module {}",
                    module
                );
                continue;
            }
            assert_eq!(
                sink.enabled.get(&module),
                Some(&false),
                "Init bypasses module {}",
                module
            );
        }

        // Every effect parameter written exactly once, at its default
        // normalized value (read through the Param trait).
        let mut expected_writes = 0;
        for module in 0..MODULE_COUNT {
            if is_red_line_module(module) {
                continue;
            }
            let module_params = params_for_module(&params, module).expect("module params");
            for (id, ptr, _) in module_params.param_map() {
                let default = unsafe { default_normalized(&ptr) };
                expected_writes += 1;
                let hits: Vec<&(usize, String, f32)> = sink
                    .written
                    .iter()
                    .filter(|(m, wid, _)| *m == module && *wid == id)
                    .collect();
                assert_eq!(hits.len(), 1, "Init writes '{}' once", id);
                assert!(
                    (hits[0].2 - default).abs() <= 1e-6,
                    "Init writes default for '{}' (got {} want {})",
                    id,
                    hits[0].2,
                    default
                );
            }
        }
        assert_eq!(
            sink.written.len(),
            expected_writes,
            "Init writes touch only effect modules"
        );
    }
}

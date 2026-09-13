use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::Arc;

pub mod dsp;
pub mod gui;
pub mod modules;
pub mod stft;

// Re-export SharedOrder for persistence and GUI use.
pub use modules::chain::SharedOrder;

// ── Re-exports for integration tests ────────────────────────────────────────

pub use modules::chain::ModuleChain;
pub use modules::AkiFxModule;

// Module types and params types referenced by the registry table below.
use modules::buffr_glitch::{BuffrGlitchModule, BuffrGlitchParams};
use modules::crisp::{CrispModule, CrispParams};
use modules::crossover::{CrossoverModule, CrossoverParams};
use modules::diopser::{DiopserModule, DiopserParams};
use modules::frequency_shift::{FrequencyShiftModule, FrequencyShiftParams};
use modules::gain::{GainModule, GainParams};
use modules::safety_limiter::{SafetyLimiterModule, SafetyLimiterParams};
use modules::sine_gen::{SineGenModule, SineGenParams};
use modules::soft_vacuum::{SoftVacuumModule, SoftVacuumParams};
use modules::spectral_compressor::{SpectralCompressorModule, SpectralCompressorParams};
use modules::spectral_gate::{SpectralGateModule, SpectralGateParams};

// The single declarative module table. Everything else (parameter tree,
// chain construction, GUI entries, name/prefix tables) is generated from it.
crate::define_modules! {
    sine_gen,            SineGenModule,            SineGenParams,            "sine_gen",            "Sine Generator";
    soft_vacuum,         SoftVacuumModule,         SoftVacuumParams,         "soft_vacuum",         "Soft Vacuum";
    crisp,               CrispModule,              CrispParams,              "crisp",               "Crisp";
    spectral_gate,       SpectralGateModule,       SpectralGateParams,       "spectral_gate",       "Spectral Gate";
    frequency_shift,     FrequencyShiftModule,     FrequencyShiftParams,     "frequency_shift",     "Frequency Shift";
    spectral_compressor, SpectralCompressorModule, SpectralCompressorParams, "spectral_compressor", "Spectral Compressor";
    crossover,           CrossoverModule,          CrossoverParams,          "crossover",           "Crossover";
    diopser,             DiopserModule,            DiopserParams,            "diopser",             "Diopser";
    buffr_glitch,        BuffrGlitchModule,        BuffrGlitchParams,        "buffr_glitch",        "Buffr Glitch";
    gain,                GainModule,               GainParams,               "gain",                "Gain";
    safety_limiter,      SafetyLimiterModule,      SafetyLimiterParams,      "safety_limiter",      "Safety Limiter";
}

// ── Plugin struct ───────────────────────────────────────────────────────────

pub struct AkiFx {
    params: Arc<AkiFxParams>,
    chain: ModuleChain,
    /// Pre-allocated scratch buffers for the mono fallback path (avoids
    /// per-block heap allocation on the audio thread).
    mono_scratch_l: Vec<f32>,
    mono_scratch_r: Vec<f32>,
    /// The latency value last reported to the host, so `process()` can
    /// re-report when toggling modules changes the chain's latency (DAWs
    /// compensate delay based on this — reporting it only in `initialize()`
    /// left the host with a stale value).
    reported_latency: u32,
    /// Reused event buffer for the block's MIDI input (avoids a fresh
    /// allocation on every block that carries MIDI).
    events_scratch: Vec<NoteEvent<()>>,
}

impl Default for AkiFx {
    fn default() -> Self {
        let params = Arc::new(AkiFxParams::default());
        let chain = create_default_chain(&params);
        Self {
            params,
            chain,
            mono_scratch_l: Vec::new(),
            mono_scratch_r: Vec::new(),
            reported_latency: 0,
            events_scratch: Vec::new(),
        }
    }
}

// ── nih-plug Plugin implementation ──────────────────────────────────────────

impl Plugin for AkiFx {
    const NAME: &'static str = "AkiFX";
    const VENDOR: &'static str = "Akiro";
    const URL: &'static str = "https://github.com/AkiroMusic/AkiFX";
    const EMAIL: &'static str = "akiro@example.com";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[],
            aux_output_ports: &[],
            names: PortNames::const_default(),
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        // Latencies are read from the live modules (which `initialize()` has
        // already configured) instead of a hand-maintained static table.
        let mut entries = gui::state::build_ui_entries(&self.params, self.chain.modules());
        // CRITICAL: share the live modules' bypass flags with the GUI so rack
        // power toggles actually gate audio processing (Oracle-found bug:
        // build_ui_entries otherwise creates orphan flags).
        gui::state::wire_bypass_flags(&mut entries, self.chain.modules());
        gui::create_editor(
            self.params.clone(),
            entries,
            self.chain.order().clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.chain
            .initialize_all(buffer_config.sample_rate, buffer_config.max_buffer_size as usize);

        // Validate and apply the persisted module order.
        let persisted = self.params.module_order.lock().clone();
        if is_valid_permutation(&persisted, self.chain.len()) {
            self.chain.set_order(persisted);
        }
        // Invalid or missing order leaves the chain at identity (set in new()).

        // Apply the persisted per-module enabled state (all-off on a fresh
        // load). nih-plug deserializes state before calling initialize(), so
        // this runs after a state restore as well.
        let enabled = self.params.module_enabled.lock().clone();
        if enabled.len() == self.chain.len() {
            for (module, &is_enabled) in
                self.chain.modules_mut().iter_mut().zip(enabled.iter())
            {
                module
                    .bypass_flag()
                    .store(!is_enabled, std::sync::atomic::Ordering::SeqCst);
            }
        }

        self.reported_latency = self.chain.latency_samples() as u32;
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.chain.reset_all();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Keep the host's latency report in sync: modules can be toggled or
        // reconfigured (oversampling, window size) at any time, and DAWs
        // compensate delay based on this value.
        let latency = self.chain.latency_samples() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }

        // Collect all MIDI input events for the block and broadcast them to
        // the chain (each module consumes what it understands). MIDI CC
        // events used to be dropped here, which made the MIDI Inverter's CC
        // transformation and Poly Mod Synth's choke handling unreachable.
        let note_events = &mut self.events_scratch;
        note_events.clear();
        while let Some(event) = context.next_event() {
            note_events.push(event);
        }

        // nih-plug Buffer::as_slice() returns &mut [&mut [f32]] — one slice per channel,
        // each slice containing all samples for that channel.
        // Use split_first_mut to safely obtain separate mutable references to channels 0 and 1.
        let channels = buffer.as_slice();
        if channels.len() >= 2 {
            let (first, rest) = channels.split_first_mut().unwrap();
            let (second, _) = rest.split_first_mut().unwrap();
            self.chain
                .process_with_midi(first, second, &note_events);
        } else if channels.len() == 1 {
            // Mono: reuse pre-allocated scratch (grows once, then stable) to
            // avoid per-block heap allocation on the audio thread.
            let len = channels[0].len();
            if self.mono_scratch_l.len() < len {
                self.mono_scratch_l.resize(len, 0.0);
                self.mono_scratch_r.resize(len, 0.0);
            }
            self.mono_scratch_l[..len].copy_from_slice(channels[0]);
            self.mono_scratch_r[..len].copy_from_slice(channels[0]);
            self.chain
                .process_with_midi(&mut self.mono_scratch_l[..len], &mut self.mono_scratch_r[..len], &note_events);
            channels[0].copy_from_slice(&self.mono_scratch_l[..len]);
        }

        // Drain transformed MIDI events from midi_inverter and send to host
        for module in self.chain.modules_mut().iter_mut() {
            let events = module.take_output_midi();
            for event in events {
                context.send_event(event);
            }
        }

        ProcessStatus::Normal
    }

    fn deactivate(&mut self) {}
}

// ── CLAP / VST3 exports ────────────────────────────────────────────────────

impl ClapPlugin for AkiFx {
    const CLAP_ID: &'static str = "com.akiro.akifx";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("All-in-one FX suite integrating SpectralSuite & NIH-plug plugins");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for AkiFx {
    const VST3_CLASS_ID: [u8; 16] = *b"AkiFX00000000001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(AkiFx);
nih_export_vst3!(AkiFx);

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Validate that `order` is a permutation of `0..len` (each index appears
/// exactly once, no out-of-bounds values).
fn is_valid_permutation(order: &[usize], len: usize) -> bool {
    if order.len() != len {
        return false;
    }
    let mut seen = vec![false; len];
    for &idx in order {
        if idx >= len || seen[idx] {
            return false;
        }
        seen[idx] = true;
    }
    true
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// The static tables and the live chain must agree on the module set.
    #[test]
    fn registry_tables_match_chain() {
        let params = AkiFxParams::default();
        let chain = create_default_chain(&params);

        assert_eq!(MODULE_COUNT, 11);
        assert_eq!(MODULE_NAMES.len(), MODULE_COUNT);
        assert_eq!(MODULE_PREFIXES.len(), MODULE_COUNT);
        assert_eq!(chain.len(), MODULE_COUNT);
        for (i, module) in chain.modules().iter().enumerate() {
            assert_eq!(module.name(), MODULE_NAMES[i], "module {i} name");
        }
    }

    /// Typed parameter access by index resolves every module.
    #[test]
    fn params_for_module_covers_all() {
        let params = AkiFxParams::default();
        for idx in 0..MODULE_COUNT {
            assert!(
                params_for_module(&params, idx).is_some(),
                "params_for_module({idx}) must resolve"
            );
        }
        assert!(params_for_module(&params, MODULE_COUNT).is_none());
    }
}

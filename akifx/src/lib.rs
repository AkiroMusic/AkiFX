use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::Arc;

pub mod gui;
pub mod modules;
pub mod stft;

// Re-export SharedOrder for persistence and GUI use.
pub use modules::chain::SharedOrder;

// ── Re-exports for integration tests ────────────────────────────────────────

pub use modules::chain::ModuleChain;
pub use modules::AkiFxModule;

use modules::bin_scrambler::{BinScramblerModule, BinScramblerParams};
use modules::buffr_glitch::{BuffrGlitchModule, BuffrGlitchParams};
use modules::crisp::{CrispModule, CrispParams};
use modules::crossover::{CrossoverModule, CrossoverParams};
use modules::diopser::{DiopserModule, DiopserParams};
use modules::frequency_magnet::{FrequencyMagnetModule, FrequencyMagnetParams};
use modules::frequency_shift::{FrequencyShiftModule, FrequencyShiftParams};
use modules::gain::{GainModule, GainParams};
use modules::loudness_war_winner::{LoudnessWarWinnerModule, LoudnessWarWinnerParams};
use modules::midi_inverter::{MidiInverterModule, MidiInverterParams};
use modules::morph::{MorphModule, MorphParams};
use modules::phase_lock::{PhaseLockModule, PhaseLockParams};
use modules::playground::{PlaygroundModule, PlaygroundParams};
use modules::poly_mod_synth::{PolyModSynthModule, PolyModSynthParams};
use modules::puberty_simulator::{PubertySimulatorModule, PubertySimulatorParams};
use modules::safety_limiter::{SafetyLimiterModule, SafetyLimiterParams};
use modules::sine_gen::{SineGenModule, SineGenParams};
use modules::sinusoidal_shaped_filter::{SinusoidalShapedFilterModule, SinusoidalShapedFilterParams};
use modules::soft_vacuum::{SoftVacuumModule, SoftVacuumParams};
use modules::spectral_compressor::compressor_bank::CompressorBank;
use modules::spectral_compressor::{SpectralCompressorModule, SpectralCompressorParams};
use modules::spectral_gate::{SpectralGateModule, SpectralGateParams};

// ── Gain helper (kept for backwards compat) ─────────────────────────────────

/// Apply gain to a buffer of interleaved samples.
/// This is a pure-Rust helper that can be tested without a host ProcessContext.
pub fn apply_gain(gain: f32, samples: &mut [f32]) {
    for sample in samples.iter_mut() {
        *sample *= gain;
    }
}

// ── Umbrella parameter struct ───────────────────────────────────────────────

/// Umbrella parameter struct that composes all 21 module params.
///
/// Each module's params are nested with an `id_prefix` so their parameter IDs
/// are namespaced. The `Arc<XxxParams>` instances are shared: one clone lives
/// here (for host serialization), the other lives inside the corresponding
/// module (for DSP access). They point to the same underlying `FloatParam`s,
/// so host automation changes propagate to the audio thread.
///
/// # Processing Order
///
/// 1.  sine_gen              — Oscillator source
/// 2.  midi_inverter         — MIDI transform (no audio effect)
/// 3.  poly_mod_synth        — Polyphonic MIDI-driven synth
/// 4.  playground            — Experimental spectral playground
/// 5.  soft_vacuum           — Distortion / saturation
/// 6.  crisp                 — Transient shaper / enhancer
/// 7.  spectral_gate         — Spectral noise gate
/// 8.  frequency_shift       — Bin-wise frequency shifting
/// 9.  frequency_magnet      — Bin pulling toward harmonics
/// 10. bin_scrambler         — Randomize bin ordering
/// 11. morph                 — Spectral morphing
/// 12. phase_lock            — Lock bin phases
/// 13. sinusoidal_shaped_filter — Sinusoidal spectral filter
/// 14. puberty_simulator     — FFT pitch shifter
/// 15. crossover             — Multiband split
/// 16. diopser               — Allpass phaser
/// 17. loudness_war_winner   — Limiter / maximizer
/// 18. spectral_compressor   — Per-bin compressor
/// 19. buffr_glitch          — Buffer stutter / glitch
/// 20. gain                  — Master gain
/// 21. safety_limiter        — Final safety limiter
#[derive(Params)]
pub struct AkiFxParams {
    #[nested(id_prefix = "sine_gen")]
    pub sine_gen: Arc<SineGenParams>,
    #[nested(id_prefix = "midi_inverter")]
    pub midi_inverter: Arc<MidiInverterParams>,
    #[nested(id_prefix = "poly_mod_synth")]
    pub poly_mod_synth: Arc<PolyModSynthParams>,
    #[nested(id_prefix = "playground")]
    pub playground: Arc<PlaygroundParams>,
    #[nested(id_prefix = "soft_vacuum")]
    pub soft_vacuum: Arc<SoftVacuumParams>,
    #[nested(id_prefix = "crisp")]
    pub crisp: Arc<CrispParams>,
    #[nested(id_prefix = "spectral_gate")]
    pub spectral_gate: Arc<SpectralGateParams>,
    #[nested(id_prefix = "frequency_shift")]
    pub frequency_shift: Arc<FrequencyShiftParams>,
    #[nested(id_prefix = "frequency_magnet")]
    pub frequency_magnet: Arc<FrequencyMagnetParams>,
    #[nested(id_prefix = "bin_scrambler")]
    pub bin_scrambler: Arc<BinScramblerParams>,
    #[nested(id_prefix = "morph")]
    pub morph: Arc<MorphParams>,
    #[nested(id_prefix = "phase_lock")]
    pub phase_lock: Arc<PhaseLockParams>,
    #[nested(id_prefix = "sinusoidal_shaped_filter")]
    pub sinusoidal_shaped_filter: Arc<SinusoidalShapedFilterParams>,
    #[nested(id_prefix = "puberty_simulator")]
    pub puberty_simulator: Arc<PubertySimulatorParams>,
    #[nested(id_prefix = "crossover")]
    pub crossover: Arc<CrossoverParams>,
    #[nested(id_prefix = "diopser")]
    pub diopser: Arc<DiopserParams>,
    #[nested(id_prefix = "loudness_war_winner")]
    pub loudness_war_winner: Arc<LoudnessWarWinnerParams>,
    #[nested(id_prefix = "spectral_compressor")]
    pub spectral_compressor: Arc<SpectralCompressorParams>,
    #[nested(id_prefix = "buffr_glitch")]
    pub buffr_glitch: Arc<BuffrGlitchParams>,
    #[nested(id_prefix = "gain")]
    pub gain: Arc<GainParams>,
    #[nested(id_prefix = "safety_limiter")]
    pub safety_limiter: Arc<SafetyLimiterParams>,

    /// Persisted module processing order. Initialized to identity `[0, 21)`.
    /// Deserialized by nih-plug via `#[persist]` when the host restores state.
    #[persist = "module_order"]
    pub module_order: parking_lot::Mutex<Vec<usize>>,

    /// Persisted UI zoom level. 1.0 = 100% (identity PPT).
    #[persist = "ui_zoom"]
    pub ui_zoom: parking_lot::Mutex<f32>,
}

impl Default for AkiFxParams {
    fn default() -> Self {
        Self {
            sine_gen: Arc::new(SineGenParams::new(-12.0, 440.0)),
            midi_inverter: Arc::new(MidiInverterParams::new(true)),
            poly_mod_synth: Arc::new(PolyModSynthParams::new()),
            playground: Arc::new(PlaygroundParams::new()),
            soft_vacuum: Arc::new(SoftVacuumParams::default()),
            crisp: Arc::new(CrispParams::new()),
            spectral_gate: Arc::new(SpectralGateParams::new()),
            frequency_shift: Arc::new(FrequencyShiftParams::new()),
            frequency_magnet: Arc::new(FrequencyMagnetParams::new()),
            bin_scrambler: Arc::new(BinScramblerParams::new()),
            morph: Arc::new(MorphParams::new()),
            phase_lock: Arc::new(PhaseLockParams::new()),
            sinusoidal_shaped_filter: Arc::new(SinusoidalShapedFilterParams::new()),
            puberty_simulator: Arc::new(PubertySimulatorParams::new()),
            crossover: Arc::new(CrossoverParams::default()),
            diopser: Arc::new(DiopserParams::new()),
            loudness_war_winner: Arc::new(LoudnessWarWinnerParams::default()),
            spectral_compressor: Arc::new(SpectralCompressorParams::new()),
            buffr_glitch: Arc::new(BuffrGlitchParams::default()),
            gain: Arc::new(GainParams::new(0.0)),
            safety_limiter: Arc::new(SafetyLimiterParams::new()),
            module_order: parking_lot::Mutex::new((0..21).collect()),
            ui_zoom: parking_lot::Mutex::new(1.0),
        }
    }
}

// ── Default chain constructor ───────────────────────────────────────────────

/// Build the default processing chain with all 21 modules in order.
///
/// Each module receives the corresponding `Arc<XxxParams>` from the umbrella,
/// so host automation changes propagate to the audio thread without extra work.
///
/// # Processing Order Rationale
///
/// 1. **Sources first** (sine_gen, midi_inverter, poly_mod_synth): Generate
///    or transform the audio/MIDI signal before any effects.
/// 2. **Playground** after sources: experimental spectral processing on
///    freshly generated audio.
/// 3. **Distortion** (soft_vacuum, crisp): Shape dynamics and timbre early
///    to feed downstream spectral effects.
/// 4. **Spectral effects** (spectral_gate → frequency_shift → frequency_magnet
///    → bin_scrambler → morph → phase_lock → sinusoidal_shaped_filter):
///    Process the frequency domain in order of increasing complexity.
/// 5. **Pitch shift** (puberty_simulator): FFT-based pitch shifting after
///    spectral effects to avoid double-processing.
/// 6. **Multiband + phaser** (crossover, diopser): Time-domain processing
///    that benefits from spectrally-shaped input.
/// 7. **Dynamics** (loudness_war_winner, spectral_compressor): Control
///    dynamics after tonal shaping.
/// 8. **Glitch** (buffr_glitch): Buffer-based stutter effects late in chain
///    to capture the fully processed signal.
/// 9. **Master** (gain, safety_limiter): Final gain stage and brickwall
///    limiter to protect the output.
pub fn create_default_chain(params: &AkiFxParams) -> ModuleChain {
    let mut chain = ModuleChain::new();

    // Helper macro to push a module with its shared params and a fresh bypass flag.
    macro_rules! push {
        ($module:ident, $params:expr) => {{
            let bypass = Arc::new(std::sync::atomic::AtomicBool::new(true));
            chain.push(Box::new($module::new($params.clone(), bypass)));
        }};
    }

    push!(SineGenModule, params.sine_gen);
    push!(MidiInverterModule, params.midi_inverter);
    push!(PolyModSynthModule, params.poly_mod_synth);
    push!(PlaygroundModule, params.playground);
    push!(SoftVacuumModule, params.soft_vacuum);
    push!(CrispModule, params.crisp);
    push!(SpectralGateModule, params.spectral_gate);
    push!(FrequencyShiftModule, params.frequency_shift);
    push!(FrequencyMagnetModule, params.frequency_magnet);
    push!(BinScramblerModule, params.bin_scrambler);
    push!(MorphModule, params.morph);
    push!(PhaseLockModule, params.phase_lock);
    push!(SinusoidalShapedFilterModule, params.sinusoidal_shaped_filter);
    push!(PubertySimulatorModule, params.puberty_simulator);
    push!(CrossoverModule, params.crossover);
    push!(DiopserModule, params.diopser);
    push!(LoudnessWarWinnerModule, params.loudness_war_winner);
    push!(SpectralCompressorModule, params.spectral_compressor);
    push!(BuffrGlitchModule, params.buffr_glitch);
    push!(GainModule, params.gain);
    push!(SafetyLimiterModule, params.safety_limiter);

    assert_eq!(chain.len(), 21, "Chain must contain exactly 21 modules");
    chain
}

// ── Plugin struct ───────────────────────────────────────────────────────────

pub struct AkiFx {
    params: Arc<AkiFxParams>,
    chain: ModuleChain,
    /// Pre-allocated scratch buffers for the mono fallback path (avoids
    /// per-block heap allocation on the audio thread).
    mono_scratch_l: Vec<f32>,
    mono_scratch_r: Vec<f32>,
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
        }
    }
}

// ── nih-plug Plugin implementation ──────────────────────────────────────────

impl Plugin for AkiFx {
    const NAME: &'static str = "AkiFX";
    const VENDOR: &'static str = "Akiro";
    const URL: &'static str = "https://github.com/akiro/akifx";
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
        let mut entries = gui::state::build_ui_entries(&self.params);
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

        context.set_latency_samples(self.chain.latency_samples() as u32);
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
        // Collect MIDI input events for the block.
        // Since SysExMessage = (), next_event() already returns NoteEvent<()>.
        let mut note_events = Vec::new();
        while let Some(event) = context.next_event() {
            match event {
                NoteEvent::NoteOn { .. }
                | NoteEvent::NoteOff { .. }
                | NoteEvent::PolyPressure { .. }
                | NoteEvent::MidiPitchBend { .. } => {
                    note_events.push(event);
                }
                _ => {}
            }
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

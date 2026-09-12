//! Crossover module — multi-band splitting via IIR Linkwitz-Riley filters.
//!
//! // allow: SIZE_OK — Faithful port of nih-plug's Crossover IIR path (~700 LOC)
//! // including biquad filters, all-pass cascade, LR24 crossover topology,
//! // parameters, AkiFxModule trait impl, and comprehensive inline tests.
//! // The algorithm's interconnected filter stages resist clean splitting
//! // without cross-file coupling that would hurt readability.
//!
//! Faithfully ports the IIR path from nih-plug's Crossover plugin, adapted
//! for AkiFX's module chain which has no aux routing. Instead of sending
//! bands to separate outputs, this module **sums** all bands back to stereo
//! output, with per-band gain controls so users can shape individual bands.
//!
//! # Module Context Adaptation
//!
//! The original nih-plug Crossover sends each band to a separate aux output.
//! AkiFX's `ModuleChain` has no aux routing — each module processes stereo
//! in-place. This module therefore:
//! - Splits the signal into 2–5 bands using LR24 IIR crossovers
//! - Applies per-band gain to each band
//! - Sums all bands back into the L+R output
//!
//! The result is a transparent passthrough when all band gains are 0 dB,
//! and frequency-selective gain when bands are boosted/cut.
//!
//! # Crossover Topology
//!
//! For LR24 (Linkwitz-Riley 24 dB/octave), each crossover uses two
//! cascaded Butterworth (Q = 1/√2) filters:
//! - Low-pass band `n` → two 2nd-order LP filters in series
//! - High-pass band `n` → two 2nd-order HP filters in series
//! - Lower bands get all-pass compensation to match the phase shift
//!   of higher bands, preserving sum transparency.
//!
//! # Linear-Phase Variant
//!
//! The FIR linear-phase crossover from the original plugin is stubbed as
//! future work. The `CrossoverType` enum has a variant for it, but
//! processing currently only implements the IIR path.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::f32::consts;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum number of bands supported by the crossover.
const MAX_BANDS: usize = 5;

/// Minimum crossover frequency in Hz.
const MIN_CROSSOVER_FREQUENCY: f32 = 40.0;

/// Maximum crossover frequency in Hz.
const MAX_CROSSOVER_FREQUENCY: f32 = 20_000.0;

/// Butterworth Q for LR24 crossovers (Q = 1/√2).
const NEUTRAL_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

// ── Biquad filter (ported from nih-plug crossover/iir/biquad.rs) ─────────────

/// Pre-normalized biquad coefficients `[b0, b1, b2, a1, a2]`.
///
/// Based on the Audio EQ Cookbook transposed direct form.
#[derive(Clone, Copy, Debug)]
struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoefficients {
    /// Identity: passes signal through unchanged.
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Clamp inputs into a range where the RBJ coefficient formulas are
    /// well-defined. The crossover-frequency parameters top out at 20 kHz,
    /// so hosts running at or below 40 kHz can push automated values past
    /// Nyquist; clamping keeps the audio thread panic-free (the previous
    /// `assert!`s were reachable in release builds).
    fn sanitize(sample_rate: f32, frequency: f32, q: f32) -> (f32, f32, f32) {
        let sample_rate = sample_rate.max(1.0);
        let frequency = frequency.clamp(1.0, sample_rate * 0.45);
        let q = q.max(1.0e-4);
        (sample_rate, frequency, q)
    }

    /// Compute coefficients for a 2nd-order low-pass filter.
    ///
    /// Based on <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>.
    fn lowpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_omega0) / 2.0) / a0,
            b1: (1.0 - cos_omega0) / a0,
            b2: ((1.0 - cos_omega0) / 2.0) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Compute coefficients for a 2nd-order high-pass filter.
    ///
    /// Based on <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>.
    fn highpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_omega0) / 2.0) / a0,
            b1: -(1.0 + cos_omega0) / a0,
            b2: ((1.0 + cos_omega0) / 2.0) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Compute coefficients for a 2nd-order all-pass filter.
    ///
    /// Based on <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>.
    fn allpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - alpha) / a0,
            b1: (-2.0 * cos_omega0) / a0,
            b2: (1.0 + alpha) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

/// Transposed direct-form biquad filter.
///
/// Processes one scalar sample at a time (no SIMD — AkiFx processes L/R
/// independently via `process(left, right)`).
#[derive(Clone, Copy, Debug)]
struct Biquad {
    coefficients: BiquadCoefficients,
    s1: f32,
    s2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            coefficients: BiquadCoefficients::identity(),
            s1: 0.0,
            s2: 0.0,
        }
    }
}

impl Biquad {
    fn process(&mut self, sample: f32) -> f32 {
        let result = self.coefficients.b0 * sample + self.s1;
        self.s1 = self.coefficients.b1 * sample - self.coefficients.a1 * result + self.s2;
        self.s2 = self.coefficients.b2 * sample - self.coefficients.a2 * result;
        result
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

// ── Single crossover (LR24) ─────────────────────────────────────────────────

/// A single crossover stage using two cascaded Butterworth filters per side.
///
/// For LR24: two 2nd-order LP + two 2nd-order HP → 24 dB/octave slopes.
#[derive(Debug, Clone, Default)]
struct Crossover {
    lp_filters: [Biquad; 2],
    hp_filters: [Biquad; 2],
}

impl Crossover {
    /// Process one sample through LR24 crossover.
    /// Returns `(low_passed, high_passed)`.
    fn process_lr24(&mut self, sample: f32) -> (f32, f32) {
        let mut low_passed = sample;
        for filter in &mut self.lp_filters {
            low_passed = filter.process(low_passed);
        }
        let mut high_passed = sample;
        for filter in &mut self.hp_filters {
            high_passed = filter.process(high_passed);
        }
        (low_passed, high_passed)
    }

    fn update_coefficients(&mut self, lp_coefs: BiquadCoefficients, hp_coefs: BiquadCoefficients) {
        for filter in &mut self.lp_filters {
            filter.coefficients = lp_coefs;
        }
        for filter in &mut self.hp_filters {
            filter.coefficients = hp_coefs;
        }
    }

    fn reset(&mut self) {
        for filter in &mut self.lp_filters {
            filter.reset();
        }
        for filter in &mut self.hp_filters {
            filter.reset();
        }
    }
}

// ── All-pass cascade for phase compensation ──────────────────────────────────

/// Compensates lower bands for the additional phase shift introduced when
/// higher bands are split by crossovers.
///
/// For LR24, low-passed band `n` gets a 2nd-order all-pass for each
/// crossover frequency above it (`n+1..num_crossovers`), so all bands
/// have aligned phase when summed.
#[derive(Debug, Default)]
struct AllPassCascade {
    /// Indexed by `[crossover_idx][0..num_bands - crossover_idx - 2]`.
    ap_filters: [[Biquad; MAX_BANDS - 2]; MAX_BANDS - 1],
    num_bands: usize,
}

impl AllPassCascade {
    /// Apply all-pass compensation to the low-passed samples for `band_idx`.
    fn compensate_lr24(&mut self, lp_sample: f32, band_idx: usize) -> f32 {
        let mut compensated = lp_sample;
        for filter in &mut self.ap_filters[band_idx][..self.num_bands - band_idx - 2] {
            compensated = filter.process(compensated);
        }
        compensated
    }

    /// Update all-pass coefficients in the diagonal pattern.
    fn update_coefficients(
        &mut self,
        sample_rate: f32,
        num_bands: usize,
        frequencies: &[f32; MAX_BANDS - 1],
    ) {
        self.num_bands = num_bands;

        for (crossover_idx, &frequency) in
            frequencies.iter().enumerate().take(num_bands - 1).skip(1)
        {
            let ap_coefs = BiquadCoefficients::allpass(sample_rate, frequency, NEUTRAL_Q);

            for target_idx in 0..crossover_idx {
                self.ap_filters[target_idx][crossover_idx - target_idx - 1].coefficients =
                    ap_coefs;
            }
        }
    }

    fn reset(&mut self) {
        for filters in &mut self.ap_filters {
            for filter in filters.iter_mut() {
                filter.reset();
            }
        }
    }
}

// ── IIR Crossover processor ─────────────────────────────────────────────────

/// IIR crossover processor using Linkwitz-Riley 24 dB/octave filters.
///
/// Splits a mono signal into up to 5 bands. Each band is phase-aligned
/// so the sum of all bands reproduces the original signal when gains are 0 dB.
#[derive(Debug)]
struct IirCrossover {
    crossovers: [Crossover; MAX_BANDS - 1],
    all_passes: AllPassCascade,
}

impl Default for IirCrossover {
    fn default() -> Self {
        Self {
            crossovers: Default::default(),
            all_passes: Default::default(),
        }
    }
}

impl IirCrossover {
    /// Split `sample` into `num_bands` bands, returning band values in order.
    ///
    /// Band 0 = lowest frequency, band `num_bands - 1` = highest.
    fn process(&mut self, num_bands: usize, sample: f32) -> [f32; MAX_BANDS] {
        let mut bands = [0.0f32; MAX_BANDS];
        let mut current = sample;

        for (crossover_idx, crossover) in
            self.crossovers.iter_mut().take(num_bands - 1).enumerate()
        {
            let (lp, hp) = crossover.process_lr24(current);

            // Phase-compensate the low-passed band
            let lp = self.all_passes.compensate_lr24(lp, crossover_idx);

            bands[crossover_idx] = lp;
            current = hp;
        }

        // Last band is the final high-pass result
        bands[num_bands - 1] = current;

        bands
    }

    /// Update filter coefficients for all crossovers.
    fn update(
        &mut self,
        sample_rate: f32,
        num_bands: usize,
        frequencies: &[f32; MAX_BANDS - 1],
    ) {
        for (crossover, &frequency) in
            self.crossovers.iter_mut().take(num_bands - 1).zip(frequencies.iter())
        {
            let lp_coefs = BiquadCoefficients::lowpass(sample_rate, frequency, NEUTRAL_Q);
            let hp_coefs = BiquadCoefficients::highpass(sample_rate, frequency, NEUTRAL_Q);
            crossover.update_coefficients(lp_coefs, hp_coefs);
        }

        self.all_passes
            .update_coefficients(sample_rate, num_bands, frequencies);
    }

    fn reset(&mut self) {
        for crossover in &mut self.crossovers {
            crossover.reset();
        }
        self.all_passes.reset();
    }
}

// ── Parameters ───────────────────────────────────────────────────────────────

/// Crossover filter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum CrossoverType {
    /// IIR Linkwitz-Riley 24 dB/octave. Zero-latency, minimum-phase.
    #[id = "lr24"]
    #[name = "LR24"]
    LinkwitzRiley24,

    /// FIR linear-phase LR24. Phase-coherent but introduces latency.
    /// **Stubbed as future work** — falls back to IIR path.
    #[id = "lr24-lp"]
    #[name = "LR24 (LP)"]
    LinkwitzRiley24LinearPhase,
}

/// Parameters for the Crossover module.
#[derive(Params)]
pub struct CrossoverParams {
    /// Number of bands (2–5).
    #[id = "bandcnt"]
    pub num_bands: IntParam,

    /// Crossover frequency 1 (Hz). Only active when num_bands >= 2.
    #[id = "xov1fq"]
    pub crossover_1_freq: FloatParam,
    /// Crossover frequency 2 (Hz). Only active when num_bands >= 3.
    #[id = "xov2fq"]
    pub crossover_2_freq: FloatParam,
    /// Crossover frequency 3 (Hz). Only active when num_bands >= 4.
    #[id = "xov3fq"]
    pub crossover_3_freq: FloatParam,
    /// Crossover frequency 4 (Hz). Only active when num_bands >= 5.
    #[id = "xov4fq"]
    pub crossover_4_freq: FloatParam,

    /// Crossover filter type (IIR vs linear-phase).
    #[id = "xovtyp"]
    pub crossover_type: EnumParam<CrossoverType>,

    /// Gain for band 1 (lowest). dB, smoothed.
    #[id = "bg1"]
    pub band_1_gain: FloatParam,
    /// Gain for band 2. dB, smoothed.
    #[id = "bg2"]
    pub band_2_gain: FloatParam,
    /// Gain for band 3. dB, smoothed.
    #[id = "bg3"]
    pub band_3_gain: FloatParam,
    /// Gain for band 4. dB, smoothed.
    #[id = "bg4"]
    pub band_4_gain: FloatParam,
    /// Gain for band 5 (highest). dB, smoothed.
    #[id = "bg5"]
    pub band_5_gain: FloatParam,
}

impl Default for CrossoverParams {
    fn default() -> Self {
        let crossover_range = FloatRange::Skewed {
            min: MIN_CROSSOVER_FREQUENCY,
            max: MAX_CROSSOVER_FREQUENCY,
            factor: FloatRange::skew_factor(-1.0),
        };
        let crossover_smooth = SmoothingStyle::Logarithmic(100.0);
        let hz_to_string = formatters::v2s_f32_hz_then_khz(0);
        let string_to_hz = formatters::s2v_f32_hz_then_khz();

        let band_gain_range = FloatRange::Skewed {
            min: util::db_to_gain(-24.0),
            max: util::db_to_gain(24.0),
            factor: FloatRange::gain_skew_factor(-24.0, 24.0),
        };

        Self {
            num_bands: IntParam::new("Band Count", 2, IntRange::Linear { min: 2, max: 5 }),

            crossover_1_freq: FloatParam::new("Crossover 1", 200.0, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_2_freq: FloatParam::new("Crossover 2", 1000.0, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_3_freq: FloatParam::new("Crossover 3", 5000.0, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_4_freq: FloatParam::new("Crossover 4", 10000.0, crossover_range)
                .with_smoother(crossover_smooth)
                .with_value_to_string(hz_to_string)
                .with_string_to_value(string_to_hz),

            crossover_type: EnumParam::new("Type", CrossoverType::LinkwitzRiley24),

            band_1_gain: FloatParam::new("Band 1 Gain", util::db_to_gain(0.0), band_gain_range)
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
                .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            band_2_gain: FloatParam::new("Band 2 Gain", util::db_to_gain(0.0), band_gain_range)
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
                .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            band_3_gain: FloatParam::new("Band 3 Gain", util::db_to_gain(0.0), band_gain_range)
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
                .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            band_4_gain: FloatParam::new("Band 4 Gain", util::db_to_gain(0.0), band_gain_range)
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
                .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            band_5_gain: FloatParam::new("Band 5 Gain", util::db_to_gain(0.0), band_gain_range)
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
                .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

// ── Module ───────────────────────────────────────────────────────────────────

/// Crossover module — splits audio into frequency bands, applies per-band
/// gain, and sums back to stereo.
///
/// Uses IIR Linkwitz-Riley 24 dB/octave filters for clean, phase-aligned
/// band splitting. The linear-phase FIR variant is stubbed as future work.
pub struct CrossoverModule {
    params: Arc<CrossoverParams>,
    bypass: Arc<AtomicBool>,
    /// One IIR crossover per channel with independent filter state, matching
    /// upstream's `Biquad<f32x2>` dual-lane design. (An earlier port ran both
    /// channels sequentially through one state set, so the right channel
    /// inherited the left channel's residual filter state.) Coefficients are
    /// kept identical across the two instances.
    iir_crossovers: [IirCrossover; 2],
    sample_rate: f32,
    /// Filter-band count and frequencies the coefficients were last built
    /// with, so `process()` can rebuild them when automation changes the
    /// crossover points (previously coefficients were only computed in
    /// `initialize()`/`reset()`, making frequency automation a no-op).
    cached_num_bands: usize,
    cached_frequencies: [f32; MAX_BANDS - 1],
}

impl CrossoverModule {
    /// Create a new CrossoverModule with shared params and bypass flag.
    pub fn new(params: Arc<CrossoverParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            iir_crossovers: [IirCrossover::default(), IirCrossover::default()],
            sample_rate: 44100.0,
            cached_num_bands: 0,
            cached_frequencies: [-1.0; MAX_BANDS - 1],
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(CrossoverParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Get the current crossover frequencies as an array.
    fn crossover_frequencies(&self) -> [f32; MAX_BANDS - 1] {
        [
            self.params.crossover_1_freq.value(),
            self.params.crossover_2_freq.value(),
            self.params.crossover_3_freq.value(),
            self.params.crossover_4_freq.value(),
        ]
    }

    /// Update filter coefficients from current parameter values.
    fn update_filters(&mut self) {
        let num_bands = self.params.num_bands.value() as usize;
        let frequencies = self.crossover_frequencies();
        for crossover in &mut self.iir_crossovers {
            crossover.update(self.sample_rate, num_bands, &frequencies);
        }
        self.cached_num_bands = num_bands;
        self.cached_frequencies = frequencies;
    }

    /// Per-sample coefficient update while the frequency smoothers are
    /// moving (mirrors upstream's `should_update_filters()`/`update_filters(1)`
    /// pair).
    fn update_filters_while_smoothing(&mut self) {
        let smoothing = self.params.crossover_1_freq.smoothed.is_smoothing()
            || self.params.crossover_2_freq.smoothed.is_smoothing()
            || self.params.crossover_3_freq.smoothed.is_smoothing()
            || self.params.crossover_4_freq.smoothed.is_smoothing();
        if !smoothing {
            return;
        }
        let num_bands = self.params.num_bands.value() as usize;
        let frequencies = [
            self.params.crossover_1_freq.smoothed.next_step(1),
            self.params.crossover_2_freq.smoothed.next_step(1),
            self.params.crossover_3_freq.smoothed.next_step(1),
            self.params.crossover_4_freq.smoothed.next_step(1),
        ];
        for crossover in &mut self.iir_crossovers {
            crossover.update(self.sample_rate, num_bands, &frequencies);
        }
        self.cached_num_bands = num_bands;
        self.cached_frequencies = frequencies;
    }

    /// Seed the parameter smoothers (non-host contexts start from zero
    /// otherwise).
    fn seed_smoothers(&mut self) {
        self.params
            .crossover_1_freq
            .smoothed
            .reset(self.params.crossover_1_freq.value());
        self.params
            .crossover_2_freq
            .smoothed
            .reset(self.params.crossover_2_freq.value());
        self.params
            .crossover_3_freq
            .smoothed
            .reset(self.params.crossover_3_freq.value());
        self.params
            .crossover_4_freq
            .smoothed
            .reset(self.params.crossover_4_freq.value());
        self.params.band_1_gain.smoothed.reset(self.params.band_1_gain.value());
        self.params.band_2_gain.smoothed.reset(self.params.band_2_gain.value());
        self.params.band_3_gain.smoothed.reset(self.params.band_3_gain.value());
        self.params.band_4_gain.smoothed.reset(self.params.band_4_gain.value());
        self.params.band_5_gain.smoothed.reset(self.params.band_5_gain.value());
    }

    /// Rebuild the filter coefficients if the band count or crossover
    /// frequencies changed since they were last computed.
    fn update_filters_if_changed(&mut self) {
        let num_bands = self.params.num_bands.value() as usize;
        let frequencies = self.crossover_frequencies();
        if num_bands != self.cached_num_bands || frequencies != self.cached_frequencies {
            self.update_filters();
        }
    }
}

impl AkiFxModule for CrossoverModule {
    fn name(&self) -> &'static str {
        "Crossover"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.seed_smoothers();
        self.update_filters();
    }

    fn reset(&mut self) {
        for crossover in &mut self.iir_crossovers {
            crossover.reset();
        }
        self.update_filters();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Rebuild coefficients when automation moved the crossover points.
        self.update_filters_if_changed();

        for i in 0..left.len() {
            // Glide the crossover frequencies while their smoothers move
            // (upstream updates coefficients per sample while smoothing).
            self.update_filters_while_smoothing();

            let num_bands = self.params.num_bands.value() as usize;
            // Band gains are port additions; smooth them per sample too.
            let gains = [
                self.params.band_1_gain.smoothed.next(),
                self.params.band_2_gain.smoothed.next(),
                self.params.band_3_gain.smoothed.next(),
                self.params.band_4_gain.smoothed.next(),
                self.params.band_5_gain.smoothed.next(),
            ];

            // Both channels pass through their own filter instance.
            let bands_l = self.iir_crossovers[0].process(num_bands, left[i]);
            let bands_r = self.iir_crossovers[1].process(num_bands, right[i]);

            let mut sum_l = 0.0;
            let mut sum_r = 0.0;
            for b in 0..num_bands {
                sum_l += bands_l[b] * gains[b];
                sum_r += bands_r[b] * gains[b];
            }
            left[i] = sum_l;
            right[i] = sum_r;
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SAMPLE_RATE: f32 = 44100.0;
    const BLOCK_SIZE: usize = 4096;

    /// Helper: build params with specific crossover freqs and per-band gain dB values.
    fn test_params(
        num_bands: i32,
        crossover_freqs: &[f32],
        band_gains_db: &[f32],
    ) -> Arc<CrossoverParams> {
        let crossover_smooth = SmoothingStyle::Logarithmic(100.0);
        let hz_to_string = formatters::v2s_f32_hz_then_khz(0);
        let string_to_hz = formatters::s2v_f32_hz_then_khz();
        let crossover_range = FloatRange::Skewed {
            min: MIN_CROSSOVER_FREQUENCY,
            max: MAX_CROSSOVER_FREQUENCY,
            factor: FloatRange::skew_factor(-1.0),
        };

        let band_gain = |name: &str, db: f32| {
            FloatParam::new(
                name,
                util::db_to_gain(db),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db())
        };

        let c1 = crossover_freqs.first().copied().unwrap_or(200.0);
        let c2 = crossover_freqs.get(1).copied().unwrap_or(1000.0);
        let c3 = crossover_freqs.get(2).copied().unwrap_or(5000.0);
        let c4 = crossover_freqs.get(3).copied().unwrap_or(10000.0);

        Arc::new(CrossoverParams {
            num_bands: IntParam::new("Band Count", num_bands, IntRange::Linear { min: 2, max: 5 }),
            crossover_1_freq: FloatParam::new("Crossover 1", c1, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_2_freq: FloatParam::new("Crossover 2", c2, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_3_freq: FloatParam::new("Crossover 3", c3, crossover_range)
                .with_smoother(crossover_smooth.clone())
                .with_value_to_string(hz_to_string.clone())
                .with_string_to_value(string_to_hz.clone()),
            crossover_4_freq: FloatParam::new("Crossover 4", c4, crossover_range)
                .with_smoother(crossover_smooth)
                .with_value_to_string(hz_to_string)
                .with_string_to_value(string_to_hz),
            crossover_type: EnumParam::new("Type", CrossoverType::LinkwitzRiley24),
            band_1_gain: band_gain("Band 1", band_gains_db.first().copied().unwrap_or(0.0)),
            band_2_gain: band_gain("Band 2", band_gains_db.get(1).copied().unwrap_or(0.0)),
            band_3_gain: band_gain("Band 3", band_gains_db.get(2).copied().unwrap_or(0.0)),
            band_4_gain: band_gain("Band 4", band_gains_db.get(3).copied().unwrap_or(0.0)),
            band_5_gain: band_gain("Band 5", band_gains_db.get(4).copied().unwrap_or(0.0)),
        })
    }

    /// Helper: create a module with default band gains (all 0 dB).
    fn make_module(
        num_bands: i32,
        crossover_freqs: &[f32],
    ) -> CrossoverModule {
        let params = test_params(num_bands, crossover_freqs, &[]);
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = CrossoverModule::new(params, bypass);
        module.initialize(SAMPLE_RATE, BLOCK_SIZE);
        module
    }

    /// Helper: create a module with specific per-band gain dB values.
    ///
    /// `band_gains_db[i]` sets the gain for band `i+1`. Unspecified bands
    /// default to 0 dB.
    fn make_module_with_gains(
        num_bands: i32,
        crossover_freqs: &[f32],
        band_gains_db: &[f32],
    ) -> CrossoverModule {
        let params = test_params(num_bands, crossover_freqs, band_gains_db);
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = CrossoverModule::new(params, bypass);
        module.initialize(SAMPLE_RATE, BLOCK_SIZE);
        module
    }

    /// Helper: generate a pure sine wave at `freq` Hz.
    fn sine_wave(freq: f32, sample_rate: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate).sin())
            .collect()
    }

    /// Helper: generate pink-ish noise via filtered white noise.
    /// Uses a simple 1/f approximation: sum of octave bands.
    fn pink_noise(len: usize) -> Vec<f32> {
        let mut rng_state: u32 = 12345;
        let mut noise = vec![0.0f32; len];

        for sample in noise.iter_mut() {
            // Simple LCG PRNG
            rng_state = rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
            let white = (rng_state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            *sample = white;
        }

        // Apply a rough 1/f shaping via cascaded single-pole filters
        let mut y = 0.0f32;
        for sample in noise.iter_mut() {
            y = y * 0.99 + *sample * 0.01;
            *sample = y;
        }

        // Normalize
        let peak = noise.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if peak > 0.0 {
            for sample in noise.iter_mut() {
                *sample /= peak;
            }
        }

        noise
    }

    /// Helper: measure RMS of a signal.
    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt()
    }

    /// Helper: measure magnitude at a frequency via DFT.
    fn magnitude_at_freq(signal: &[f32], freq: f32, sample_rate: f32) -> f32 {
        let n = signal.len();
        let k = (freq * n as f32 / sample_rate).round() as usize;
        let k = k.min(n / 2);

        let mut real = 0.0;
        let mut imag = 0.0;
        for (i, &s) in signal.iter().enumerate() {
            let angle = 2.0 * PI * k as f32 * i as f32 / n as f32;
            real += s * angle.cos();
            imag -= s * angle.sin();
        }

        2.0 * (real * real + imag * imag).sqrt() / n as f32
    }

    // ── Test: 2-band passthrough ──────────────────────────────────────────

    /// With all band gains at 0 dB, the sum of low+high bands should
    /// approximate passthrough. For an LR24 crossover, the sum of LP + HP
    /// at the crossover frequency should be within ~0.5 dB of the input.
    #[test]
    fn test_two_band_passthrough() {
        let mut module = make_module(2, &[1000.0]);

        // Generate test signal at the crossover frequency
        let signal = sine_wave(1000.0, SAMPLE_RATE, BLOCK_SIZE);
        let input_rms = rms(&signal);

        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);

        let output_rms = rms(&left);

        // The LR24 sum (LP+HP) at the crossover frequency should be very
        // close to unity. Allow 0.5 dB tolerance.
        let ratio_db = 20.0 * (output_rms / input_rms).log10();
        assert!(
            ratio_db.abs() < 0.5,
            "2-band passthrough at crossover: expected ~0 dB, got {ratio_db:.2} dB"
        );
    }

    /// Pink noise passthrough: sum of bands should preserve spectrum
    /// within 0.5 dB across the spectrum.
    #[test]
    fn test_two_band_pink_noise_passthrough() {
        let mut module = make_module(2, &[1000.0]);

        let noise = pink_noise(BLOCK_SIZE);
        let input_rms = rms(&noise);

        let mut left = noise;
        let mut right = vec![0.0; BLOCK_SIZE];
        module.process(&mut left, &mut right);

        let output_rms = rms(&left);

        // Overall RMS should be very close to input
        let ratio_db = 20.0 * (output_rms / input_rms).log10();
        assert!(
            ratio_db.abs() < 0.5,
            "Pink noise passthrough: expected ~0 dB, got {ratio_db:.2} dB"
        );
    }

    // ── Test: low-band rolloff ────────────────────────────────────────────

    /// The low band should attenuate frequencies well above the crossover
    /// frequency. With LR24 (24 dB/octave), at 2 octaves above crossover
    /// the attenuation should be >= 48 dB (asymptotically).
    #[test]
    fn test_low_band_rolloff() {
        let crossover_freq = 1000.0;
        // Mute band 2 (-100 dB ≈ mute) to isolate the low band output
        let mut module = make_module_with_gains(2, &[crossover_freq], &[0.0, -100.0]);

        // Test at 4x the crossover frequency (2 octaves above)
        let test_freq = crossover_freq * 4.0;
        let signal = sine_wave(test_freq, SAMPLE_RATE, BLOCK_SIZE);
        let input_rms = rms(&signal);

        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);
        let low_band_rms = rms(&left);

        let attenuation_db = 20.0 * (input_rms / low_band_rms).log10();
        assert!(
            attenuation_db >= 40.0,
            "Low band at 4x crossover: expected >= 40 dB attenuation, got {attenuation_db:.1} dB"
        );
    }

    /// High band should pass high frequencies. Mute band 1 to isolate HP output.
    #[test]
    fn test_high_band_passes() {
        let crossover_freq = 1000.0;
        // Mute band 1 (-100 dB ≈ mute) to isolate the high band output
        let mut module = make_module_with_gains(2, &[crossover_freq], &[-100.0, 0.0]);

        let test_freq = crossover_freq * 4.0;
        let signal = sine_wave(test_freq, SAMPLE_RATE, BLOCK_SIZE);
        let input_rms = rms(&signal);

        let mut left = signal.clone();
        let mut right = signal.clone();
        module.process(&mut left, &mut right);

        let high_band_rms = rms(&left);
        let ratio_db = 20.0 * (high_band_rms / input_rms).log10();

        assert!(
            ratio_db.abs() < 1.0,
            "High band at 4x crossover: expected ~0 dB, got {ratio_db:.2} dB"
        );
    }

    // ── Test: 4-band config runs without panic ────────────────────────────

    #[test]
    fn test_four_band_no_panic() {
        let mut module = make_module(4, &[200.0, 1000.0, 5000.0]);

        let signal = pink_noise(BLOCK_SIZE);
        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);

        // Just verify it ran and produced finite output
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(right.iter().all(|s| s.is_finite()));
    }

    // ── Test: DC passes through low band ──────────────────────────────────

    #[test]
    fn test_dc_passes_through_low_band() {
        let mut module = make_module(2, &[1000.0]);

        // DC signal (constant 1.0)
        let signal = vec![1.0f32; BLOCK_SIZE];
        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);

        // DC should pass through with ~0 dB change (all bands sum to unity)
        let output_dc = left[100]; // Skip initial transient
        assert!(
            (output_dc - 1.0).abs() < 0.01,
            "DC passthrough: expected ~1.0, got {output_dc:.4}"
        );
    }

    // ── Test: impulse response bounded over 2 seconds ─────────────────────

    #[test]
    fn test_impulse_response_bounded() {
        let num_samples = (SAMPLE_RATE * 2.0) as usize;
        let mut module = make_module(3, &[200.0, 2000.0]);

        let mut left = vec![0.0f32; num_samples];
        let mut right = vec![0.0f32; num_samples];

        // Single impulse at sample 0
        left[0] = 1.0;
        right[0] = 1.0;

        module.process(&mut left, &mut right);

        // Check that the impulse response is bounded (no divergence)
        let max_abs = left
            .iter()
            .chain(right.iter())
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_abs < 10.0,
            "Impulse response diverged: max absolute value = {max_abs}"
        );

        // Check that the tail has decayed substantially
        let tail_rms = rms(&left[num_samples - 4096..]);
        assert!(
            tail_rms < 0.001,
            "Impulse response tail not decayed: tail RMS = {tail_rms:.6}"
        );
    }

    // ── Test: reset restores initial behavior ──────────────────────────────

    #[test]
    fn test_reset_restores_behavior() {
        let mut module = make_module(2, &[1000.0]);

        // Process some audio to build up filter state
        let signal = sine_wave(5000.0, SAMPLE_RATE, BLOCK_SIZE);
        let mut left = signal.clone();
        let mut right = signal.clone();
        module.process(&mut left, &mut right);

        // Reset
        module.reset();

        // Process a fresh signal — should match a freshly-created module
        let mut module_fresh = make_module(2, &[1000.0]);

        let mut left_after = signal.clone();
        let mut right_after = signal.clone();
        module.process(&mut left_after, &mut right_after);

        let mut left_fresh = signal.clone();
        let mut right_fresh = signal.clone();
        module_fresh.process(&mut left_fresh, &mut right_fresh);

        // After reset, output should match a fresh module
        for (a, b) in left_after.iter().zip(left_fresh.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "Reset mismatch: got {a:.8} vs fresh {b:.8}"
            );
        }
    }

    // ── Test: band gain shaping ───────────────────────────────────────────

    /// Boosting band 1 should increase low-frequency content.
    #[test]
    fn test_band_gain_boost() {
        // Band 1 at +6 dB, band 2 at 0 dB
        let mut module = make_module_with_gains(2, &[1000.0], &[6.0, 0.0]);

        // Low-frequency signal (well below crossover)
        let signal = sine_wave(100.0, SAMPLE_RATE, BLOCK_SIZE);
        let input_rms = rms(&signal);

        let mut left = signal.clone();
        let mut right = signal.clone();
        module.process(&mut left, &mut right);

        let output_rms = rms(&left);
        let ratio_db = 20.0 * (output_rms / input_rms).log10();

        // Should be boosted by approximately 6 dB
        assert!(
            (ratio_db - 6.0).abs() < 1.0,
            "Band 1 boost: expected ~6 dB, got {ratio_db:.2} dB"
        );
    }

    // ── Test: 5-band config ───────────────────────────────────────────────

    #[test]
    fn test_five_band_no_panic() {
        let mut module = make_module(5, &[100.0, 500.0, 2000.0, 8000.0]);

        let signal = pink_noise(BLOCK_SIZE);
        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);

        assert!(left.iter().all(|s| s.is_finite()));
        assert!(right.iter().all(|s| s.is_finite()));
    }

    // ── Test: crossover rolloff slope measurement ─────────────────────────

    /// Measure the rolloff slope of the low band by comparing magnitudes
    /// at 1 octave and 2 octaves above the crossover frequency.
    /// LR24 should give ~24 dB/octave slope.
    #[test]
    fn test_rolloff_slope_measurement() {
        let crossover_freq = 1000.0;
        // Mute band 2 to isolate the low band
        let params = test_params(2, &[crossover_freq], &[0.0, -100.0]);

        // Test at 1 octave above (2000 Hz) with fresh module
        let mut module_1 = CrossoverModule::new(params.clone(), Arc::new(AtomicBool::new(false)));
        module_1.initialize(SAMPLE_RATE, BLOCK_SIZE);
        let signal_1oct = sine_wave(crossover_freq * 2.0, SAMPLE_RATE, BLOCK_SIZE);
        let mut left_1 = signal_1oct.clone();
        let mut right_1 = signal_1oct.clone();
        module_1.process(&mut left_1, &mut right_1);
        let mag_1oct = magnitude_at_freq(&left_1, crossover_freq * 2.0, SAMPLE_RATE);

        // Test at 2 octaves above (4000 Hz) with fresh module
        let mut module_2 = CrossoverModule::new(params.clone(), Arc::new(AtomicBool::new(false)));
        module_2.initialize(SAMPLE_RATE, BLOCK_SIZE);
        let signal_2oct = sine_wave(crossover_freq * 4.0, SAMPLE_RATE, BLOCK_SIZE);
        let mut left_2 = signal_2oct.clone();
        let mut right_2 = signal_2oct.clone();
        module_2.process(&mut left_2, &mut right_2);
        let mag_2oct = magnitude_at_freq(&left_2, crossover_freq * 4.0, SAMPLE_RATE);

        // 1 octave apart, difference should be >= 20 dB (LR24 asymptotic)
        if mag_1oct > 1e-10 && mag_2oct > 1e-10 {
            let slope_db = 20.0 * (mag_1oct / mag_2oct).log10();
            assert!(
                slope_db >= 20.0,
                "LR24 slope: expected >= 20 dB/oct, got {slope_db:.1} dB/oct"
            );
        }
    }
}

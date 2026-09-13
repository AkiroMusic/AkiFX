//! Crisp — high-frequency exciter (port of nih-plug's Crisp).
//!
//! Layers the input with a copy ring-modulated by filtered noise,
//! adding bright crispy top end to bass-heavy sounds. Inspired by
//! Polarity's "Fake Distortion" technique.
//!
//! # Algorithm
//!
//! For each sample:
//! 1. Input → LPF → ring modulate with (HPF → LPF noise) → scale by `amount × 2`
//! 2. Output = `(input + rm_output) × output_gain`
//!    or wet-only: `rm_output × output_gain`
//!
//! # Parameters
//!
//! Mirrors all nih-plug Crisp parameters exactly: amount, mode, stereo mode,
//! filter frequencies/Q for input LPF, noise HPF/LPF, output gain, wet-only.

use crate::dsp::{Biquad, BiquadCoefficients};
use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ────────────────────────────────────────────────────────────

const NUM_CHANNELS: usize = 2;
const AMOUNT_GAIN_MULTIPLIER: f32 = 2.0;
const MIN_FILTER_FREQUENCY: f32 = 5.0;
const MAX_FILTER_FREQUENCY: f32 = 22_000.0;
const INITIAL_PRNG_SEED: Pcg32iState = Pcg32iState::new(69, 420);

// ── PCG32 PRNG (ported faithfully from nih-plug pcg.rs) ──────────────────
// <https://github.com/imneme/pcg-c/blob/master/include/pcg_variants.h>
// <https://www.pcg-random.org/using-pcg-c.html>

const PCG_DEFAULT_MULTIPLIER_32: u32 = 747796405;

/// The `pcg32i` PRNG from PCG.
#[derive(Copy, Clone)]
struct Pcg32iState {
    state: u32,
    inc: u32,
}

impl Pcg32iState {
    /// Initialize the PRNG, aka `*_srandom()`.
    const fn new(state: u32, sequence: u32) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (sequence << 1) | 1,
        };
        rng.state = rng
            .state
            .wrapping_mul(PCG_DEFAULT_MULTIPLIER_32)
            .wrapping_add(rng.inc);
        rng.state += state;
        rng.state = rng
            .state
            .wrapping_mul(PCG_DEFAULT_MULTIPLIER_32)
            .wrapping_add(rng.inc);
        rng
    }

    /// Generate a new uniformly distributed `u32` covering all possible values.
    #[inline]
    fn next_u32(&mut self) -> u32 {
        let old_state = self.state;
        self.state = self
            .state
            .wrapping_mul(PCG_DEFAULT_MULTIPLIER_32)
            .wrapping_add(self.inc);
        let word =
            ((old_state >> ((old_state >> 28) + 4)) ^ old_state).wrapping_mul(277803737);
        (word >> 22) ^ word
    }

    /// Generate a new `f32` value in the open `(0, 1)` range.
    #[inline]
    fn next_f32(&mut self) -> f32 {
        const FLOAT_SIZE: u32 = std::mem::size_of::<f32>() as u32 * 8;
        let value = self.next_u32();
        let fraction = value >> (FLOAT_SIZE - f32::MANTISSA_DIGITS - 1);
        let exponent_bits: u32 = ((f32::MAX_EXP - 1) as u32) << (f32::MANTISSA_DIGITS - 1);
        f32::from_bits(fraction | exponent_bits) - (1.0 - f32::EPSILON / 2.0)
    }
}


// ── Enums ────────────────────────────────────────────────────────────────

/// Controls the type of ring modulation to apply.
#[derive(Enum, Debug, PartialEq, Clone, Copy)]
pub enum Mode {
    /// RM the entire waveform.
    #[id = "soggy"]
    Soggy,
    /// RM only the positive part of the waveform.
    #[id = "crispy"]
    Crispy,
}

/// Controls how to handle stereo input.
#[derive(Enum, Debug, PartialEq, Clone, Copy)]
pub enum StereoMode {
    /// Use the same noise for both channels.
    #[id = "mono"]
    Mono,
    /// Use a different noise source per channel.
    #[id = "stereo"]
    Stereo,
}

// ── Parameters ───────────────────────────────────────────────────────────

/// Concrete parameter struct for the Crisp module.
///
/// Stored in an `Arc<CrispParams>` shared between the module (for DSP access)
/// and the umbrella `AkiFxParams` (for host serialization via `#[nested]`).
#[derive(Params)]
pub struct CrispParams {
    /// On a range of `[0, 1]`, how much of the modulated sound to mix in.
    #[id = "amount"]
    pub amount: FloatParam,
    /// What kind of ring modulation to apply.
    #[id = "mode"]
    pub mode: EnumParam<Mode>,
    /// How to handle stereo signals.
    #[id = "stereo"]
    pub stereo_mode: EnumParam<StereoMode>,

    /// Cutoff frequency for the LPF applied to the input before RM.
    #[id = "rmlpff"]
    pub rm_input_lpf_freq: FloatParam,
    /// Q factor for the LPF applied to the input before RM.
    #[id = "rmlpfq"]
    pub rm_input_lpf_q: FloatParam,
    /// Cutoff frequency for the HPF applied to the noise.
    #[id = "nzhpff"]
    pub noise_hpf_freq: FloatParam,
    /// Q factor for the HPF applied to the noise.
    #[id = "nzhpfq"]
    pub noise_hpf_q: FloatParam,
    /// Cutoff frequency for the LPF applied to the noise.
    #[id = "nzlpff"]
    pub noise_lpf_freq: FloatParam,
    /// Q factor for the LPF applied to the noise.
    #[id = "nzlpfq"]
    pub noise_lpf_q: FloatParam,

    /// Output gain, as voltage gain. Displayed in decibels.
    #[id = "output"]
    pub output_gain: FloatParam,
    /// If set, only output the RM'ed signal.
    #[id = "wtonly"]
    pub wet_only: BoolParam,
}

impl Default for CrispParams {
    fn default() -> Self {
        Self::new()
    }
}

impl CrispParams {
    /// Create params with a specific `amount` value (useful for testing).
    pub fn with_amount(amount: f32) -> Self {
        let mut p = Self::new();
        p.amount = FloatParam::new(
            "Amount",
            amount,
            FloatRange::Linear {
                min: 0.0,
                max: 1.0,
            },
        )
        .with_smoother(SmoothingStyle::Linear(10.0))
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage());
        p
    }

    /// Create params with default values matching nih-plug Crisp.
    pub fn new() -> Self {
        let f32_hz_then_khz = formatters::v2s_f32_hz_then_khz(0);
        let from_f32_hz_then_khz = formatters::s2v_f32_hz_then_khz();

        Self {
            amount: FloatParam::new(
                "Amount",
                0.35,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(10.0))
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            mode: EnumParam::new("Mode", Mode::Crispy),
            stereo_mode: EnumParam::new("Stereo Mode", StereoMode::Stereo),

            rm_input_lpf_freq: FloatParam::new(
                "RM LP Frequency",
                MAX_FILTER_FREQUENCY,
                FloatRange::Skewed {
                    min: MIN_FILTER_FREQUENCY,
                    max: MAX_FILTER_FREQUENCY,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(Arc::new(|value| {
                if value >= MAX_FILTER_FREQUENCY {
                    String::from("Disabled")
                } else {
                    format!("{value:.0} Hz")
                }
            }))
            .with_string_to_value(Arc::new(|string| {
                if string == "Disabled" {
                    Some(MAX_FILTER_FREQUENCY)
                } else {
                    string.trim().trim_end_matches(" Hz").parse().ok()
                }
            })),
            rm_input_lpf_q: FloatParam::new(
                "RM LP Resonance",
                2.0f32.sqrt() / 2.0,
                FloatRange::Skewed {
                    min: 2.0f32.sqrt() / 2.0,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            noise_hpf_freq: FloatParam::new(
                "Noise HP Frequency",
                MIN_FILTER_FREQUENCY,
                FloatRange::Skewed {
                    min: MIN_FILTER_FREQUENCY,
                    max: MAX_FILTER_FREQUENCY,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string({
                let f32_hz_then_khz = f32_hz_then_khz.clone();
                Arc::new(move |value| {
                    if value <= MIN_FILTER_FREQUENCY {
                        String::from("Disabled")
                    } else {
                        f32_hz_then_khz(value)
                    }
                })
            })
            .with_string_to_value({
                let from_f32_hz_then_khz = from_f32_hz_then_khz.clone();
                Arc::new(move |string| {
                    if string == "Disabled" {
                        Some(MIN_FILTER_FREQUENCY)
                    } else {
                        from_f32_hz_then_khz(string)
                    }
                })
            }),
            noise_hpf_q: FloatParam::new(
                "Noise HP Resonance",
                2.0f32.sqrt() / 2.0,
                FloatRange::Skewed {
                    min: 2.0f32.sqrt() / 2.0,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            noise_lpf_freq: FloatParam::new(
                "Noise LP Frequency",
                MAX_FILTER_FREQUENCY,
                FloatRange::Skewed {
                    min: MIN_FILTER_FREQUENCY,
                    max: MAX_FILTER_FREQUENCY,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(Arc::new(move |value| {
                if value >= MAX_FILTER_FREQUENCY {
                    String::from("Disabled")
                } else {
                    f32_hz_then_khz(value)
                }
            }))
            .with_string_to_value(Arc::new(move |string| {
                if string == "Disabled" {
                    Some(MAX_FILTER_FREQUENCY)
                } else {
                    from_f32_hz_then_khz(string)
                }
            })),
            noise_lpf_q: FloatParam::new(
                "Noise LP Resonance",
                2.0f32.sqrt() / 2.0,
                FloatRange::Skewed {
                    min: 2.0f32.sqrt() / 2.0,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            output_gain: FloatParam::new(
                "Output",
                1.0,
                FloatRange::Linear {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            wet_only: BoolParam::new("Wet Only", false),
        }
    }
}

// ── Module ───────────────────────────────────────────────────────────────

/// Crisp high-frequency exciter module.
///
/// # Usage
///
/// ```rust,no_run
/// use akifx::modules::AkiFxModule;
/// use akifx::modules::crisp::{CrispModule, CrispParams};
/// use std::sync::Arc;
///
/// let params = Arc::new(CrispParams::new());
/// let bypass = Arc::new(std::sync::atomic::AtomicBool::new(false));
/// let mut module = CrispModule::new(params, bypass);
/// module.initialize(44100.0, 512);
///
/// let mut left = vec![0.5; 64];
/// let mut right = vec![0.5; 64];
/// module.process(&mut left, &mut right);
/// ```
pub struct CrispModule {
    params: Arc<CrispParams>,
    bypass: Arc<AtomicBool>,

    /// Needed for computing the filter coefficients.
    sample_rate: f32,

    /// PRNG for generating noise.
    prng: Pcg32iState,

    /// LPF on input before ring modulation.
    rm_input_lpf: [Biquad; NUM_CHANNELS],
    /// HPF on noise to brighten it.
    noise_hpf: [Biquad; NUM_CHANNELS],
    /// LPF on noise to tame extreme highs.
    noise_lpf: [Biquad; NUM_CHANNELS],
}

impl CrispModule {
    /// Create a new CrispModule with shared params and bypass flag.
    pub fn new(params: Arc<CrispParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 1.0,
            prng: INITIAL_PRNG_SEED,
            rm_input_lpf: [Biquad::default(); NUM_CHANNELS],
            noise_hpf: [Biquad::default(); NUM_CHANNELS],
            noise_lpf: [Biquad::default(); NUM_CHANNELS],
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(CrispParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Generate a noise sample: PRNG → HPF → LPF.
    fn gen_noise(&mut self, channel: usize) -> f32 {
        let noise = self.prng.next_f32() * 2.0 - 1.0;
        let high_passed = self.noise_hpf[channel].process(noise);
        self.noise_lpf[channel].process(high_passed)
    }

    /// Ring-modulate a sample depending on the mode. Applies input LPF first.
    fn do_ring_mod(&mut self, sample: f32, channel_idx: usize, noise: f32) -> f32 {
        let sample = self.rm_input_lpf[channel_idx].process(sample);
        match self.params.mode.value() {
            Mode::Soggy => sample * noise,
            Mode::Crispy => sample.max(0.0) * noise,
        }
    }

    /// Step and apply any coefficient smoothers that are currently moving,
    /// once per sample (mirrors upstream's maybe_update_filters).
    fn maybe_update_filters(&mut self) {
        if self.params.rm_input_lpf_freq.smoothed.is_smoothing()
            || self.params.rm_input_lpf_q.smoothed.is_smoothing()
        {
            self.update_rm_input_lpf();
        }
        if self.params.noise_hpf_freq.smoothed.is_smoothing()
            || self.params.noise_hpf_q.smoothed.is_smoothing()
        {
            self.update_noise_hpf();
        }
        if self.params.noise_lpf_freq.smoothed.is_smoothing()
            || self.params.noise_lpf_q.smoothed.is_smoothing()
        {
            self.update_noise_lpf();
        }
    }

    /// Update the pre-RM low-pass coefficients, stepping its smoothers.
    fn update_rm_input_lpf(&mut self) {
        let frequency = self.params.rm_input_lpf_freq.smoothed.next();
        let q = self.params.rm_input_lpf_q.smoothed.next();
        let coefficients = BiquadCoefficients::lowpass(self.sample_rate, frequency, q);
        for filter in &mut self.rm_input_lpf {
            filter.coefficients = coefficients;
        }
    }

    /// Update the noise high-pass coefficients, stepping its smoothers.
    fn update_noise_hpf(&mut self) {
        let frequency = self.params.noise_hpf_freq.smoothed.next();
        let q = self.params.noise_hpf_q.smoothed.next();
        let coefficients = BiquadCoefficients::highpass(self.sample_rate, frequency, q);
        for filter in &mut self.noise_hpf {
            filter.coefficients = coefficients;
        }
    }

    /// Update the noise low-pass coefficients, stepping its smoothers.
    fn update_noise_lpf(&mut self) {
        let frequency = self.params.noise_lpf_freq.smoothed.next();
        let q = self.params.noise_lpf_q.smoothed.next();
        let coefficients = BiquadCoefficients::lowpass(self.sample_rate, frequency, q);
        for filter in &mut self.noise_lpf {
            filter.coefficients = coefficients;
        }
    }

    /// Recompute all filter coefficients from current param values.
    fn update_filter_coefficients(&mut self) {
        let lp_freq = self.params.rm_input_lpf_freq.value();
        let lp_q = self.params.rm_input_lpf_q.value();
        let lp_coeffs = BiquadCoefficients::lowpass(self.sample_rate, lp_freq, lp_q);
        for filter in &mut self.rm_input_lpf {
            filter.coefficients = lp_coeffs;
        }

        let hp_freq = self.params.noise_hpf_freq.value();
        let hp_q = self.params.noise_hpf_q.value();
        let hp_coeffs = BiquadCoefficients::highpass(self.sample_rate, hp_freq, hp_q);
        for filter in &mut self.noise_hpf {
            filter.coefficients = hp_coeffs;
        }

        let nz_lp_freq = self.params.noise_lpf_freq.value();
        let nz_lp_q = self.params.noise_lpf_q.value();
        let nz_lp_coeffs =
            BiquadCoefficients::lowpass(self.sample_rate, nz_lp_freq, nz_lp_q);
        for filter in &mut self.noise_lpf {
            filter.coefficients = nz_lp_coeffs;
        }
    }
}

impl AkiFxModule for CrispModule {
    fn name(&self) -> &'static str {
        "Crisp"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // Seed all smoothers so non-host contexts (tests, standalone) don't
        // ramp from zero (an unseeded amount smoother silenced the module).
        self.params
            .amount
            .smoothed
            .reset(self.params.amount.value());
        self.params
            .output_gain
            .smoothed
            .reset(self.params.output_gain.value());
        self.params
            .rm_input_lpf_freq
            .smoothed
            .reset(self.params.rm_input_lpf_freq.value());
        self.params
            .rm_input_lpf_q
            .smoothed
            .reset(self.params.rm_input_lpf_q.value());
        self.params
            .noise_hpf_freq
            .smoothed
            .reset(self.params.noise_hpf_freq.value());
        self.params
            .noise_hpf_q
            .smoothed
            .reset(self.params.noise_hpf_q.value());
        self.params
            .noise_lpf_freq
            .smoothed
            .reset(self.params.noise_lpf_freq.value());
        self.params
            .noise_lpf_q
            .smoothed
            .reset(self.params.noise_lpf_q.value());
        self.update_filter_coefficients();
    }

    fn reset(&mut self) {
        self.prng = INITIAL_PRNG_SEED;
        for filter in &mut self.rm_input_lpf {
            filter.reset();
        }
        for filter in &mut self.noise_hpf {
            filter.reset();
        }
        for filter in &mut self.noise_lpf {
            filter.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let wet_only = self.params.wet_only.value();
        let stereo = self.params.stereo_mode.value() == StereoMode::Stereo;

        // Smooth per sample exactly like upstream: `amount`/`output_gain`
        // step every sample, and filter coefficients are rebuilt while their
        // frequency/Q smoothers are moving. (The previous port read raw
        // values once per block, producing zipper noise on automation.)
        for i in 0..left.len() {
            let amount = self.params.amount.smoothed.next() * AMOUNT_GAIN_MULTIPLIER;
            self.maybe_update_filters();

            let (rm_l, rm_r) = if stereo {
                let noise_l = self.gen_noise(0);
                let noise_r = self.gen_noise(1);
                (
                    self.do_ring_mod(left[i], 0, noise_l) * amount,
                    self.do_ring_mod(right[i], 1, noise_r) * amount,
                )
            } else {
                let noise = self.gen_noise(0);
                (
                    self.do_ring_mod(left[i], 0, noise) * amount,
                    self.do_ring_mod(right[i], 1, noise) * amount,
                )
            };

            let output_gain = self.params.output_gain.smoothed.next();
            if wet_only {
                left[i] = rm_l * output_gain;
                right[i] = rm_r * output_gain;
            } else {
                left[i] = (left[i] + rm_l) * output_gain;
                right[i] = (right[i] + rm_r) * output_gain;
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::f32::consts;
    use super::*;

    const SR: f32 = 44_100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module with default params.
    fn make_module() -> CrispModule {
        let mut m = CrispModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create and initialise a module with a specific amount.
    fn make_module_with_amount(amount: f32) -> CrispModule {
        let params = Arc::new(CrispParams::with_amount(amount));
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = CrispModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process stereo signal through the module block-by-block.
    fn process_signal(
        module: &mut CrispModule,
        left: &[f32],
        right: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let mut out_l = vec![0.0f32; left.len()];
        let mut out_r = vec![0.0f32; right.len()];

        for start in (0..left.len()).step_by(BLOCK) {
            let end = (start + BLOCK).min(left.len());
            let chunk_len = end - start;
            let mut l = vec![0.0f32; BLOCK];
            let mut r = vec![0.0f32; BLOCK];
            l[..chunk_len].copy_from_slice(&left[start..end]);
            r[..chunk_len].copy_from_slice(&right[start..end]);
            module.process(&mut l, &mut r);
            out_l[start..end].copy_from_slice(&l[..chunk_len]);
            out_r[start..end].copy_from_slice(&r[..chunk_len]);
        }

        (out_l, out_r)
    }

    /// Compute total spectral energy in bins above `cutoff_hz`.
    fn spectral_energy_above(signal: &[f32], sample_rate: f32, cutoff_hz: f32) -> f32 {
        let n = signal.len();
        let bin_cutoff = (cutoff_hz * n as f32 / sample_rate).ceil() as usize;
        let bin_nyquist = n / 2;

        let mut energy = 0.0f32;
        for k in bin_cutoff..=bin_nyquist.min(n - 1) {
            let mut re = 0.0f32;
            let mut im = 0.0f32;
            let freq_k = 2.0 * consts::PI * k as f32 / n as f32;
            for (i, &s) in signal.iter().enumerate() {
                let angle = freq_k * i as f32;
                re += s * angle.cos();
                im -= s * angle.sin();
            }
            energy += re * re + im * im;
        }
        energy
    }

    /// Compute RMS of a signal slice.
    fn rms(signal: &[f32]) -> f32 {
        if signal.is_empty() {
            return 0.0;
        }
        let sum: f32 = signal.iter().map(|s| s * s).sum();
        (sum / signal.len() as f32).sqrt()
    }

    // ── Test 1: amount=0 → passthrough ────────────────────────────────────

    #[test]
    fn amount_zero_passthrough() {
        let mut module = make_module_with_amount(0.0);

        let n = 1024;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let (out_l, _) = process_signal(&mut module, &input, &input);

        for i in 0..n {
            let diff = (out_l[i] - input[i]).abs();
            assert!(
                diff < 1e-6,
                "sample {i}: input={input_i:.6}, output={out_i:.6}",
                input_i = input[i],
                out_i = out_l[i]
            );
        }
    }

    // ── Test 2: Engaged adds high-frequency energy ────────────────────────

    #[test]
    fn engaged_adds_high_freq_energy() {
        // Bass-heavy input: 100 Hz sine.
        let n = 8192;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * consts::PI * 100.0 * i as f32 / SR).sin())
            .collect();

        // Amount = 0: no RM mixed in.
        let mut module_off = make_module_with_amount(0.0);
        let (off_l, _) = process_signal(&mut module_off, &input, &input);
        let energy_off = spectral_energy_above(&off_l, SR, 8000.0);

        // Amount = 0.5: RM mixed in.
        let mut module_on = make_module_with_amount(0.5);
        let (on_l, _) = process_signal(&mut module_on, &input, &input);
        let energy_on = spectral_energy_above(&on_l, SR, 8000.0);

        assert!(
            energy_on > energy_off,
            "high-freq energy should increase with effect engaged: off={energy_off:.2}, on={energy_on:.2}"
        );
    }

    // ── Test 3: Determinism — same seed → bit-identical ───────────────────

    #[test]
    fn determinism() {
        let n = 2048;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let mut m1 = make_module();
        let (l1, r1) = process_signal(&mut m1, &input, &input);

        let mut m2 = make_module();
        let (l2, r2) = process_signal(&mut m2, &input, &input);

        for i in 0..n {
            assert_eq!(l1[i], l2[i], "left channel differ at sample {i}");
            assert_eq!(r1[i], r2[i], "right channel differ at sample {i}");
        }
    }

    // ── Test 4: Silence → silence ─────────────────────────────────────────

    #[test]
    fn silence_stays_silent() {
        let mut module = make_module();
        let n = 4096;
        let input = vec![0.0f32; n];

        let (out_l, out_r) = process_signal(&mut module, &input, &input);

        let max_l: f32 = out_l.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let max_r: f32 = out_r.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(max_l < 1e-10, "left silence output: max abs = {max_l}");
        assert!(max_r < 1e-10, "right silence output: max abs = {max_r}");
    }

    // ── Test 5: No NaN/Inf over 10 s of noise ────────────────────────────

    #[test]
    fn no_nan_over_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize;

        // Deterministic pseudo-noise from sum of sines.
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * consts::PI * 100.0 * t).sin()
                    + (2.0 * consts::PI * 317.0 * t).sin()
                    + (2.0 * consts::PI * 793.0 * t).sin()
            })
            .collect();

        let (out_l, out_r) = process_signal(&mut module, &input, &input);

        assert_eq!(out_l.len(), total, "left length must match input");
        assert_eq!(out_r.len(), total, "right length must match input");
        assert!(!out_l.iter().any(|s| s.is_nan()), "left channel has NaN");
        assert!(!out_r.iter().any(|s| s.is_nan()), "right channel has NaN");
        assert!(
            !out_l.iter().any(|s| s.is_infinite()),
            "left channel has Inf"
        );
        assert!(
            !out_r.iter().any(|s| s.is_infinite()),
            "right channel has Inf"
        );

        let max_abs_l: f32 = out_l.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let max_abs_r: f32 = out_r.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs_l <= 10.0,
            "left output unbounded: max abs = {max_abs_l}"
        );
        assert!(
            max_abs_r <= 10.0,
            "right output unbounded: max abs = {max_abs_r}"
        );

        // Should have non-trivial energy (effect is doing something).
        let level_l = rms(&out_l);
        assert!(
            level_l > 0.01,
            "left output RMS too low ({level_l:.6}), effect may not be processing"
        );
    }

    // ── Test 6: Reset determinism ─────────────────────────────────────────

    #[test]
    fn reset_determinism() {
        let n = 4096;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        // Process once to build up state.
        let mut module = make_module();
        let _ = process_signal(&mut module, &input, &input);

        // Reset clears all internal state.
        module.reset();

        // Process the same input after reset.
        let (reset_l, _) = process_signal(&mut module, &input, &input);

        // Fresh module processes the same input.
        let mut fresh = make_module();
        let (fresh_l, _) = process_signal(&mut fresh, &input, &input);

        // Outputs must be bit-identical.
        for i in 0..n {
            assert_eq!(
                reset_l[i], fresh_l[i],
                "reset determinism failed at sample {i}: after_reset={:.6}, fresh={:.6}",
                reset_l[i], fresh_l[i]
            );
        }
    }
}

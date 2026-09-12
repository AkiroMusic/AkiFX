//! Loudness War Winner — hard clipper with optional band-limited "win harder" path.
//!
//! Ports the nih-plug `loudness_war_winner` plugin into the AkiFx module framework.
//! Applies a hard clipper at a configurable threshold, with an optional cascaded
//! bandpass filter chain around 5.5 kHz for LUFS K-Weighting.
//!
//! # Parameters
//!
//! - **Threshold** (`#[id = "thresh"]`): Hard clip threshold in dBFS (default: 0 dB).
//! - **WIN HARDER** (`#[id = "win"]`): Enables cascaded bandpass filter chain.
//! - **Cutoff** (`#[id = "cutoff"]`): Bandpass center frequency in Hz (default: 5500 Hz).
//! - **Trim** (`#[id = "trim"]`): Output gain trim in dB (default: 0 dB / unity).

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::f32::consts;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Center frequency for the bandpass filter when WIN HARDER is engaged (Hz).
const BP_FREQUENCY: f32 = 5500.0;
/// Silence duration before fadeout begins (ms).
const SILENCE_FADEOUT_START_MS: f32 = 1000.0;
/// Total silence duration before output is fully muted (ms).
const SILENCE_FADEOUT_END_MS: f32 = SILENCE_FADEOUT_START_MS + 1000.0;

// ── Biquad filter (ported from nih-plug filter.rs) ──────────────────────────

/// Pre-normalized biquad coefficients `[b0, b1, b2, a1, a2]`.
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

    /// Bandpass coefficients from the Audio EQ Cookbook.
    fn bandpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let omega0 = consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

/// Transposed direct-form biquad filter.
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

// ── Parameters ──────────────────────────────────────────────────────────────

/// Parameters for the Loudness War Winner module.
#[derive(Params)]
pub struct LoudnessWarWinnerParams {
    /// The output gain, set to -24 dB by default because oof ouchie. This is
    /// a linear gain value; the hard clipper outputs `sign(x) * output_gain`.
    #[id = "output"]
    pub output_gain: FloatParam,

    /// When non-zero, this engages a bandpass filter around 5.5 kHz to help
    /// with the LUFS K-weighting. A fraction in `[0, 1]`; the filter's Q is
    /// `0.00001 + factor * 30` (upstream formula).
    #[id = "powah"]
    pub win_harder_factor: FloatParam,
}

impl Default for LoudnessWarWinnerParams {
    fn default() -> Self {
        Self {
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(-24.0),
                // Representing gain in decibels keeps the range logarithmic
                FloatRange::Linear {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            win_harder_factor: FloatParam::new(
                "WIN HARDER",
                0.0,
                // This ramps up hard, so keep the "usable" value range larger
                FloatRange::Skewed {
                    min: 0.0,
                    max: 1.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(30.0))
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }
}

pub struct LoudnessWarWinnerModule {
    params: Arc<LoudnessWarWinnerParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    /// 4 cascaded biquads per channel for the bandpass path.
    bp_filters: Vec<[Biquad; 4]>,
    num_silent_samples: u32,
    silence_fadeout_start_samples: u32,
    silence_fadeout_end_samples: u32,
    silence_fadeout_length_samples: u32,
}

impl LoudnessWarWinnerModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<LoudnessWarWinnerParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            bp_filters: Vec::new(),
            num_silent_samples: 0,
            silence_fadeout_start_samples: 0,
            silence_fadeout_end_samples: 0,
            silence_fadeout_length_samples: 0,
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(LoudnessWarWinnerParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Update the bandpass coefficients, stepping the WIN HARDER smoother.
    /// Only called during processing while that smoother is moving (upstream
    /// contract). Q = `0.00001 + factor * 30` — at factor 0 the filter is an
    /// extremely narrow spike, which is exactly the upstream behaviour.
    fn update_bp_filters(&mut self) {
        let q = 0.00001 + (self.params.win_harder_factor.smoothed.next() * 30.0);
        let coeffs = BiquadCoefficients::bandpass(self.sample_rate, BP_FREQUENCY, q);
        for filters in &mut self.bp_filters {
            for filter in filters {
                filter.coefficients = coeffs;
            }
        }
    }
}

impl AkiFxModule for LoudnessWarWinnerModule {
    fn name(&self) -> &'static str {
        "Loudness War Winner"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.bp_filters.resize(2, [Biquad::default(); 4]);
        // Seed the smoothers so non-host contexts don't ramp from zero.
        self.params
            .output_gain
            .smoothed
            .reset(self.params.output_gain.value());
        self.params
            .win_harder_factor
            .smoothed
            .reset(self.params.win_harder_factor.value());
        self.update_bp_filters();
        self.silence_fadeout_start_samples =
            (SILENCE_FADEOUT_START_MS / 1000.0 * sample_rate).round() as u32;
        self.silence_fadeout_end_samples =
            (SILENCE_FADEOUT_END_MS / 1000.0 * sample_rate).round() as u32;
        self.silence_fadeout_length_samples =
            self.silence_fadeout_end_samples - self.silence_fadeout_start_samples;
    }

    fn reset(&mut self) {
        for filters in &mut self.bp_filters {
            for filter in filters {
                filter.reset();
            }
        }
        self.num_silent_samples = self.silence_fadeout_end_samples;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Per-sample processing exactly like upstream (iter_samples):
        for i in 0..left.len() {
            let output_gain = self.params.output_gain.smoothed.next();

            // While WIN HARDER is moving, keep its bandpass coefficients
            // gliding (upstream steps the factor smoother here).
            if self.params.win_harder_factor.smoothed.is_smoothing() {
                self.update_bp_filters();
            }
            let apply_bp_filters = self.params.win_harder_factor.smoothed.previous_value() > 0.0;

            let mut l = left[i];
            let mut r = right[i];

            let input_silent = l == 0.0 && r == 0.0;

            // Optional 5.5 kHz bandpass cascade for extra K-weighting "pain"
            if apply_bp_filters {
                if let Some(filters) = self.bp_filters.get_mut(0) {
                    for filter in filters {
                        l = filter.process(l);
                    }
                }
                if let Some(filters) = self.bp_filters.get_mut(1) {
                    for filter in filters {
                        r = filter.process(r);
                    }
                }
            }

            // The "limiter": a sign hard clip scaled by the output gain.
            l = if l >= 0.0 { 1.0 } else { -1.0 } * output_gain;
            r = if r >= 0.0 { 1.0 } else { -1.0 } * output_gain;

            // Fade into silence after prolonged silence to avoid constant DC
            if input_silent {
                self.num_silent_samples += 1;
                if self.num_silent_samples >= self.silence_fadeout_end_samples {
                    l = 0.0;
                    r = 0.0;
                } else if self.num_silent_samples >= self.silence_fadeout_start_samples {
                    let fadeout_gain = 1.0
                        - ((self.num_silent_samples - self.silence_fadeout_start_samples) as f32
                            / self.silence_fadeout_length_samples as f32);
                    l *= fadeout_gain;
                    r *= fadeout_gain;
                }
            } else {
                self.num_silent_samples = 0;
            }

            left[i] = l;
            right[i] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// A module initialised with the default params (output gain -24 dB,
    /// WIN HARDER off), seeded like the host would.
    fn make_module() -> LoudnessWarWinnerModule {
        let mut m = LoudnessWarWinnerModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin() * amp)
            .collect()
    }

    /// Upstream behaviour: every non-silent sample becomes
    /// `sign(x) * output_gain` — a full-scale square wave at the output gain
    /// (default -24 dB), regardless of how quiet the input was.
    #[test]
    fn nonzero_input_becomes_square_at_output_gain() {
        let mut m = make_module();
        let mut left = sine(220.0, BLOCK, 0.125);
        let mut right = left.clone();
        m.process(&mut left, &mut right);

        let expected = util::db_to_gain(-24.0);
        for i in 1..BLOCK {
            assert!(
                (left[i].abs() - expected).abs() < 1e-4 || left[i] == 0.0,
                "sample {i} should be a +/-gain square, got {}",
                left[i]
            );
        }
    }

    /// After reset() the module starts in the "silent" state, so a silent
    /// input stays fully muted (no DC square).
    #[test]
    fn silence_fades_out_to_zero() {
        let mut m = make_module();
        m.reset();
        let mut left = vec![0.0f32; BLOCK];
        let mut right = vec![0.0f32; BLOCK];
        m.process(&mut left, &mut right);
        assert!(left.iter().all(|s| *s == 0.0), "silence must stay muted");
    }

    /// WIN HARDER engaged (factor > 0) still yields the square output; its
    /// bandpass only reshapes the sign pattern of the input.
    #[test]
    fn win_harder_still_outputs_square() {
        let params = Arc::new(LoudnessWarWinnerParams {
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(-24.0),
                FloatRange::Linear {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            win_harder_factor: FloatParam::new(
                "WIN HARDER",
                1.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 1.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = LoudnessWarWinnerModule::new(params, bypass);
        m.initialize(SR, BLOCK);

        let mut left = sine(440.0, BLOCK, 0.5);
        let mut right = left.clone();
        m.process(&mut left, &mut right);

        let expected = util::db_to_gain(-24.0);
        let squares = left
            .iter()
            .filter(|s| ((s.abs() - expected).abs() < 1e-4 || **s == 0.0))
            .count();
        assert!(
            squares > BLOCK * 9 / 10,
            "output should be almost entirely square, squares={squares}"
        );
    }

    /// reset() must re-enter the "silent" state so a freshly inserted,
    /// silent instance doesn't emit the DC square.
    #[test]
    fn reset_starts_silent() {
        let mut m = make_module();
        m.reset();
        assert_eq!(m.num_silent_samples, m.silence_fadeout_end_samples);
    }
}

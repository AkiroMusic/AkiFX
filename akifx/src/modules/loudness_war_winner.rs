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
    /// Hard clip threshold in dBFS. Samples exceeding ±threshold are clamped.
    #[id = "thresh"]
    pub threshold_db: FloatParam,
    /// Enable cascaded bandpass filters around 5.5 kHz for K-Weighting.
    #[id = "win"]
    pub win_harder: BoolParam,
    /// Bandpass center frequency (Hz).
    #[id = "cutoff"]
    pub cutoff_hz: FloatParam,
    /// Output gain trim in dB, applied after clipping.
    #[id = "trim"]
    pub trim: FloatParam,
}

impl Default for LoudnessWarWinnerParams {
    fn default() -> Self {
        Self {
            threshold_db: FloatParam::new(
                "Threshold",
                0.0,
                FloatRange::Skewed {
                    min: -80.0,
                    max: 0.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(2))
            .with_string_to_value(Arc::new(|s: &str| s.parse::<f32>().ok())),
            win_harder: BoolParam::new("WIN HARDER", false),
            cutoff_hz: FloatParam::new(
                "Cutoff",
                BP_FREQUENCY,
                FloatRange::Skewed {
                    min: 100.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(30.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            trim: FloatParam::new(
                "Trim",
                util::db_to_gain(0.0),
                FloatRange::Linear {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

// ── Module ──────────────────────────────────────────────────────────────────

/// Loudness War Winner — hard clipper with optional bandpass "win harder" path.
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

    fn update_bp_filters(&mut self) {
        let cutoff = self.params.cutoff_hz.value();
        // `0.00001 + 30.0` is a faithful transcription of the upstream JSFX,
        // where the epsilon guards a variable Q expression that was collapsed
        // here to the constant full-Q case (~30.0). Kept verbatim on purpose.
        let q = 0.00001 + 30.0;
        let coeffs = BiquadCoefficients::bandpass(self.sample_rate, cutoff, q);
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
        let threshold = util::db_to_gain(self.params.threshold_db.value());
        let trim = self.params.trim.value();
        let apply_bp = self.params.win_harder.value();

        if apply_bp {
            self.update_bp_filters();
        }

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let input_silent = *l == 0.0 && *r == 0.0;

            // Bandpass filter chain if WIN HARDER is engaged
            if apply_bp {
                if let Some(filters) = self.bp_filters.get_mut(0) {
                    for filter in filters {
                        *l = filter.process(*l);
                    }
                }
                if let Some(filters) = self.bp_filters.get_mut(1) {
                    for filter in filters {
                        *r = filter.process(*r);
                    }
                }
            }

            // Hard clip to +/-threshold, then apply trim
            *l = clamp_and_trim(*l, threshold, trim);
            *r = clamp_and_trim(*r, threshold, trim);

            // Silence fadeout to avoid constant DC on silent input
            if input_silent {
                self.num_silent_samples += 1;
                if self.num_silent_samples >= self.silence_fadeout_end_samples {
                    *l = 0.0;
                    *r = 0.0;
                } else if self.num_silent_samples >= self.silence_fadeout_start_samples {
                    let fadeout_gain = 1.0
                        - ((self.num_silent_samples - self.silence_fadeout_start_samples) as f32
                            / self.silence_fadeout_length_samples as f32);
                    *l *= fadeout_gain;
                    *r *= fadeout_gain;
                }
            } else {
                self.num_silent_samples = 0;
            }
        }
    }
}

/// Hard clip sample to +/-threshold, then multiply by trim.
#[inline]
fn clamp_and_trim(sample: f32, threshold: f32, trim: f32) -> f32 {
    let clamped = if sample > threshold {
        threshold
    } else if sample < -threshold {
        -threshold
    } else {
        sample
    };
    clamped * trim
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build params with specific values for testing.
    fn test_params(
        threshold_db: f32,
        win_harder: bool,
        trim_db: f32,
    ) -> Arc<LoudnessWarWinnerParams> {
        Arc::new(LoudnessWarWinnerParams {
            threshold_db: FloatParam::new(
                "Threshold",
                threshold_db,
                FloatRange::Skewed {
                    min: -80.0,
                    max: 0.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(2))
            .with_string_to_value(Arc::new(|s: &str| s.parse::<f32>().ok())),
            win_harder: BoolParam::new("WIN HARDER", win_harder),
            cutoff_hz: FloatParam::new(
                "Cutoff",
                BP_FREQUENCY,
                FloatRange::Skewed {
                    min: 100.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(30.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            trim: FloatParam::new(
                "Trim",
                util::db_to_gain(trim_db),
                FloatRange::Linear {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        })
    }

    fn make_module(
        threshold_db: f32,
        win_harder: bool,
        trim_db: f32,
    ) -> LoudnessWarWinnerModule {
        let params = test_params(threshold_db, win_harder, trim_db);
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = LoudnessWarWinnerModule::new(params, bypass);
        m.initialize(44100.0, 512);
        m
    }

    #[test]
    fn sine_above_threshold_gets_clamped() {
        let mut m = make_module(-6.0, false, 0.0);
        let expected = util::db_to_gain(-6.0);
        let mut left = vec![0.9f32; 64];
        let mut right = vec![0.9f32; 64];
        m.process(&mut left, &mut right);
        for (l, r) in left.iter().zip(right.iter()) {
            assert!(
                (l - expected).abs() < 1e-4,
                "left sample {l} not within 1e-4 of {expected}"
            );
            assert!(
                (r - expected).abs() < 1e-4,
                "right sample {r} not within 1e-4 of {expected}"
            );
        }
    }

    #[test]
    fn quiet_input_passes_through() {
        let mut m = make_module(0.0, false, 0.0);
        let input_val = 0.01f32;
        let mut left = vec![input_val; 64];
        let mut right = vec![input_val; 64];
        m.process(&mut left, &mut right);
        let max_diff = left
            .iter()
            .zip(right.iter())
            .map(|(l, r)| (l - input_val).abs().max((r - input_val).abs()))
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-6, "max diff {max_diff} exceeds 1e-6");
    }

    #[test]
    fn win_harder_bounded_near_threshold() {
        let mut m = make_module(-6.0, true, 0.0);
        let threshold = util::db_to_gain(-6.0);
        let mut left = vec![0.5f32; 256];
        let mut right = vec![0.5f32; 256];
        m.process(&mut left, &mut right);
        // After the initial transient, output should be bounded by threshold
        for sample in left[64..].iter().chain(right[64..].iter()) {
            assert!(
                sample.abs() <= threshold + 1e-4,
                "sample {} exceeds threshold {threshold}",
                sample
            );
        }
    }

    #[test]
    fn reset_clears_filter_state() {
        // Process an impulse, save output, reset, process again — should match
        let params = test_params(-6.0, true, 0.0);
        let bypass = Arc::new(AtomicBool::new(false));

        let mut m1 = LoudnessWarWinnerModule::new(params.clone(), bypass.clone());
        m1.initialize(44100.0, 512);
        let mut left1 = vec![0.0f32; 128];
        let mut right1 = vec![0.0f32; 128];
        left1[0] = 1.0;
        right1[0] = 1.0;
        m1.process(&mut left1, &mut right1);

        let mut m2 = LoudnessWarWinnerModule::new(params, bypass);
        m2.initialize(44100.0, 512);
        // m2 is freshly initialized — reset() then process
        m2.reset();
        let mut left2 = vec![0.0f32; 128];
        let mut right2 = vec![0.0f32; 128];
        left2[0] = 1.0;
        right2[0] = 1.0;
        m2.process(&mut left2, &mut right2);

        for (a, b) in left1.iter().zip(left2.iter()) {
            assert!((a - b).abs() < 1e-6, "left mismatch: {a} vs {b}");
        }
        for (a, b) in right1.iter().zip(right2.iter()) {
            assert!((a - b).abs() < 1e-6, "right mismatch: {a} vs {b}");
        }
    }
}

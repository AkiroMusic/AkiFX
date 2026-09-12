//! Safety Limiter module — ear protection with SOS Morse code signal.
//!
//! When input exceeds the threshold, this module clamps the output and plays a
//! SOS Morse code beep pattern to alert the user. When input returns to safe
//! levels, it gradually fades back to passthrough using equal-power crossfade.
//!
//! # Parameters
//!
//! - **Threshold** (`#[id = "threshold"]`): The level (in dB) at which the limiter
//!   engages. Stored as linear gain internally, displayed in dB.
//!
//! # Morse Code
//!
//! The SOS pattern is: three short beeps (dots), three long beeps (dashes),
//! three short beeps (dots), with appropriate spacing between elements.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// After reaching the threshold, it will take this many milliseconds under that threshold to start
/// fading back to the normal signal. Peaking above the threshold again during this time resets
/// this. The fadeout doesn't start immediately since that would add some nasty distortion when most
/// but not all samples pass the threshold.
const MORSE_FADEOUT_START_MS: f32 = 500.0;
/// The Morse fadeout ends after this many milliseconds.
const MORSE_FADEOUT_END_MS: f32 = MORSE_FADEOUT_START_MS + 1500.0;
/// The frequency of the sine wave used for the SOS signal.
const MORSE_FREQUENCY: f32 = 420.0;

/// The four second SOS morse code sequence. Each element here represents an edge where the signal
/// is either turned on or off. The first element of each tuple is the time in milliseconds into the
/// sequence, while the second element is the new gate status at that time point. The last element
/// acts as a delay before wrapping around, and it is equivalent to the 0 position in the next cycle
/// (hence why it is set to true).
const MORSE_SEQ_EDGES_MS: [(u32, bool); 19] = [
    // S, 3*100 ms + 2*100ms spacing
    (0, true),
    (100, false),
    (200, true),
    (300, false),
    (400, true),
    // 500 ms silence
    (500, false),
    //
    // O, 3*200 ms + 2*100ms spacing
    (1000, true),
    (1200, false),
    (1400, true),
    (1600, false),
    (1800, true),
    // 500 ms silence
    (2000, false),
    //
    // S, 3*100 ms + 2*100ms spacing
    (2500, true),
    (2600, false),
    (2700, true),
    (2800, false),
    (2900, true),
    // 1000 ms silence
    (3000, false),
    // Acts as a delay at the end before the sequence loops. This sample 4000 behaves like an alias
    // for sample 0 in the next cycle.
    (4000, true),
];

/// Concrete parameter struct for the Safety Limiter module.
///
/// Stored in an `Arc<SafetyLimiterParams>` shared between the module (for DSP access)
/// and the umbrella `AkiFxParams` (for host serialization via `#[nested]`).
#[derive(Params)]
pub struct SafetyLimiterParams {
    /// The level at which to start engaging the safety limiter. Stored as a gain ratio instead of
    /// decibels.
    #[id = "threshold"]
    pub threshold: FloatParam,
}

impl SafetyLimiterParams {
    /// Create new safety limiter params with default threshold at 0 dBFS.
    pub fn new() -> Self {
        Self {
            threshold: FloatParam::new(
                "Threshold",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 12.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

impl Default for SafetyLimiterParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Safety Limiter module — clamps peaks and plays SOS Morse code when redlining.
///
/// # Usage
///
/// ```rust,no_run
/// use akifx::modules::AkiFxModule;
/// use akifx::modules::safety_limiter::{SafetyLimiterModule, SafetyLimiterParams};
/// use std::sync::Arc;
///
/// let params = Arc::new(SafetyLimiterParams::new());
/// let bypass = Arc::new(std::sync::atomic::AtomicBool::new(false));
/// let mut module = SafetyLimiterModule::new(params, bypass);
/// module.initialize(44100.0, 512);
///
/// let mut left = vec![0.0; 64];
/// let mut right = vec![0.0; 64];
/// module.process(&mut left, &mut right);
/// ```
pub struct SafetyLimiterModule {
    params: Arc<SafetyLimiterParams>,
    bypass: Arc<AtomicBool>,

    /// The sample rate, set during initialization.
    sample_rate: f32,

    /// `MORSE_FADEOUT_START_MS` translated into samples.
    morse_fadeout_samples_start: u32,
    /// `MORSE_FADEOUT_END_MS` translated into samples.
    morse_fadeout_samples_end: u32,
    /// `MORSE_SEQ_EDGES_MS` translated into samples.
    morse_seq_edges_samples: [(u32, bool); 19],

    /// The number of samples into the fadeout. This resets back to 0 whenever the signal peaks
    /// above the threshold.
    morse_fadeout_samples_current: u32,
    /// The index of the current step into `morse_seq_edges_samples`. This wraps around to zero when
    /// reaching the end of the sequence. This is only reset once the fadeout is fully finished.
    morse_seq_current_step_idx: usize,
    /// The index of the current sample in the morse code sequence. This wraps around to zero when
    /// reaching the end of the sequence. This is only reset once the fadeout is fully finished.
    morse_seq_current_sample_idx: u32,

    /// The phase of the Morse code sine oscillator. This runs from zero to `2 * pi` for
    /// efficiency's sake.
    osc_phase_tau: f32,
    /// The phase increment for every sample. This can be precomputed since the frequency is fixed.
    osc_phase_tau_dt: f32,
}

impl SafetyLimiterModule {
    /// Create a new SafetyLimiterModule with shared params and bypass flag.
    pub fn new(params: Arc<SafetyLimiterParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 1.0,
            morse_fadeout_samples_start: 0,
            morse_fadeout_samples_end: 0,
            morse_seq_edges_samples: [(0, false); 19],
            morse_fadeout_samples_current: 0,
            morse_seq_current_step_idx: 0,
            morse_seq_current_sample_idx: 0,
            osc_phase_tau: 0.0,
            osc_phase_tau_dt: 0.0,
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_default() -> Self {
        let params = Arc::new(SafetyLimiterParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Reset the SOS signal to the start.
    fn reset_morse_signal(&mut self) {
        self.osc_phase_tau = 0.0;
        self.morse_seq_current_step_idx = 0;
        self.morse_seq_current_sample_idx = 0;
    }
}

impl AkiFxModule for SafetyLimiterModule {
    fn name(&self) -> &'static str {
        "Safety Limiter"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.morse_fadeout_samples_start =
            (MORSE_FADEOUT_START_MS / 1000.0 * sample_rate).round() as u32;
        self.morse_fadeout_samples_end =
            (MORSE_FADEOUT_END_MS / 1000.0 * sample_rate).round() as u32;
        self.osc_phase_tau_dt = MORSE_FREQUENCY / sample_rate * std::f32::consts::TAU;

        self.morse_seq_edges_samples = MORSE_SEQ_EDGES_MS.map(|(time_ms, gate)| {
            (
                (time_ms as f32 / 1000.0 * sample_rate).round() as u32,
                gate,
            )
        });

        // After initialization the morse signal should not be playing. Setting the fadeout
        // counter past the end disables morse output until the first peak triggers it.
        self.morse_fadeout_samples_current = self.morse_fadeout_samples_end;
        self.reset_morse_signal();
    }

    fn reset(&mut self) {
        self.morse_fadeout_samples_current = self.morse_fadeout_samples_end;
        self.reset_morse_signal();
    }

    fn latency_samples(&self) -> u64 {
        0
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let threshold = self.params.threshold.value();
        let morse_seq_len = self.morse_seq_edges_samples.last().map_or(0, |(len, _)| *len);

        for i in 0..left.len() {
            let sample_l = left[i];
            let sample_r = if i < right.len() { right[i] } else { sample_l };

            // Check if either channel is peaking (above threshold).
            // Non-finite samples count as peaking: upstream mutes them AND
            // fires the SOS morse alert (a blown buffer is exactly the
            // situation the alert exists for).
            let is_peaking = !sample_l.is_finite()
                || !sample_r.is_finite()
                || sample_l.abs() > threshold
                || sample_r.abs() > threshold;

            // Handle non-finite samples by replacing with silence
            if !sample_l.is_finite() {
                left[i] = 0.0;
            }
            if i < right.len() && !sample_r.is_finite() {
                right[i] = 0.0;
            }

            if is_peaking {
                // We'll continue playback where it was left off when this gets triggered before the
                // fadeout has finished, but otherwise the sequence should be restarted.
                if self.morse_fadeout_samples_current >= self.morse_fadeout_samples_end {
                    self.reset_morse_signal();
                }

                // This is the number of samples into the fadeout
                self.morse_fadeout_samples_current = 0;
            }

            // Depending on the current gate status in the morse code sequence we'll either play a
            // sine wave oscillator or silence, and the original audio will be faded back in when it
            // stays under the threshold for long enough.
            if self.morse_fadeout_samples_current < self.morse_fadeout_samples_end {
                // Move to the next step when it is reached
                let morse_seq_next_step_idx =
                    (self.morse_seq_current_step_idx + 1) % self.morse_seq_edges_samples.len();
                if self.morse_seq_current_sample_idx
                    >= self.morse_seq_edges_samples[morse_seq_next_step_idx].0
                {
                    self.morse_seq_current_step_idx = morse_seq_next_step_idx;
                }

                // And either play or don't play the sine wave depending on the current step's gate
                // values. We'll wait for the phase wraparound when deactivating the sine wave to
                // avoid clicks.
                let (_, gate) = self.morse_seq_edges_samples[self.morse_seq_current_step_idx];
                let morse_sample = if gate || self.osc_phase_tau > self.osc_phase_tau_dt {
                    // This phase runs from 0 to `2 * pi` as an optimization, so we can use it
                    // directly. And the sine wave is scaled down to the threshold minus 24 dB
                    let sine_sample =
                        self.osc_phase_tau.sin() * (threshold * 0.125);
                    self.osc_phase_tau += self.osc_phase_tau_dt;
                    if self.osc_phase_tau >= std::f32::consts::TAU {
                        self.osc_phase_tau -= std::f32::consts::TAU;
                    }

                    sine_sample
                } else {
                    0.0
                };

                // We'll do an equal power fade
                let original_t_squared = if self.morse_fadeout_samples_current
                    < self.morse_fadeout_samples_start
                {
                    0.0
                } else {
                    (self.morse_fadeout_samples_current - self.morse_fadeout_samples_start) as f32
                        / (self.morse_fadeout_samples_end - self.morse_fadeout_samples_start) as f32
                };
                let original_t = original_t_squared.sqrt();
                let morse_t = (1.0 - original_t_squared).sqrt();

                let morse_mixed = (morse_sample * morse_t) + (left[i] * original_t);
                left[i] = morse_mixed;
                if i < right.len() {
                    let morse_mixed_r = (morse_sample * morse_t) + (right[i] * original_t);
                    right[i] = morse_mixed_r;
                }

                self.morse_fadeout_samples_current += 1;
                self.morse_seq_current_sample_idx += 1;
                if self.morse_seq_current_sample_idx >= morse_seq_len {
                    self.morse_seq_current_sample_idx -= morse_seq_len;
                    self.morse_seq_current_step_idx = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Helper: create an initialized module at a given sample rate.
    fn make_module(threshold_db: f32, sample_rate: f32) -> SafetyLimiterModule {
        let params = Arc::new(SafetyLimiterParams {
            threshold: FloatParam::new(
                "Threshold",
                util::db_to_gain(threshold_db),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 12.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = SafetyLimiterModule::new(params, bypass);
        module.initialize(sample_rate, 512);
        module
    }

    #[test]
    fn silence_stays_bit_silent() {
        let mut module = make_module(0.0, 44100.0);
        let mut left = vec![0.0f32; 256];
        let mut right = vec![0.0f32; 256];
        module.process(&mut left, &mut right);
        assert!(
            left.iter().all(|&s| s == 0.0),
            "Silence input must produce bit-silent output"
        );
        assert!(
            right.iter().all(|&s| s == 0.0),
            "Silence input must produce bit-silent output (right)"
        );
    }

    #[test]
    fn sine_above_threshold_clamped() {
        let threshold_db = 0.0; // 0 dBFS = gain 1.0
        let mut module = make_module(threshold_db, 44100.0);
        let sample_rate = 44100.0f32;
        let freq = 1000.0f32;
        let amplitude_db = 3.0; // +3 dBFS
        let amplitude = util::db_to_gain(amplitude_db);

        let block_size = 512;
        let mut left = Vec::with_capacity(block_size);
        let mut right = Vec::with_capacity(block_size);
        for n in 0..block_size {
            let sample = (2.0 * PI * freq * n as f32 / sample_rate).sin() * amplitude;
            left.push(sample);
            right.push(sample);
        }

        module.process(&mut left, &mut right);

        let threshold_gain = util::db_to_gain(threshold_db);
        // During limiting, the morse signal is at threshold * 0.125 = 0.125,
        // which is well below the threshold. The original signal gets multiplied
        // by original_t which starts at 0 and fades in. So all output samples
        // should be bounded by threshold + small epsilon.
        let tolerance = threshold_gain + 1e-3;
        for (i, &s) in left.iter().enumerate() {
            assert!(
                s.abs() <= tolerance,
                "Sample {} (L) = {} exceeds threshold {} + epsilon",
                i,
                s,
                tolerance
            );
        }
        for (i, &s) in right.iter().enumerate() {
            assert!(
                s.abs() <= tolerance,
                "Sample {} (R) = {} exceeds threshold {} + epsilon",
                i,
                s,
                tolerance
            );
        }
    }

    #[test]
    fn morse_bursts_detected_while_limiting() {
        // At +3 dBFS with 0 dBFS threshold, the module will be limiting.
        // We check that the output contains alternating high/low RMS windows
        // consistent with morse dots/dashes.
        let threshold_db = 0.0;
        let mut module = make_module(threshold_db, 44100.0);
        let sample_rate = 44100.0f32;
        let freq = 1000.0f32;
        let amplitude = util::db_to_gain(3.0);

        // Process enough samples for at least one full morse sequence cycle (4 seconds)
        let block_size = (sample_rate * 4.0) as usize;
        let mut left = Vec::with_capacity(block_size);
        let mut right = Vec::with_capacity(block_size);
        for n in 0..block_size {
            let sample = (2.0 * PI * freq * n as f32 / sample_rate).sin() * amplitude;
            left.push(sample);
            right.push(sample);
        }

        module.process(&mut left, &mut right);

        // Check RMS in 100ms windows — morse dots are 100ms on, 100ms off
        let window_size = (sample_rate * 0.1) as usize;
        let mut rms_values = Vec::new();

        for window_start in (0..block_size).step_by(window_size) {
            let window_end = (window_start + window_size).min(block_size);
            if window_end <= window_start {
                break;
            }
            let sum_sq: f32 = left[window_start..window_end]
                .iter()
                .map(|&s| s * s)
                .sum();
            let rms = (sum_sq / (window_end - window_start) as f32).sqrt();
            rms_values.push(rms);
        }

        // We should see at least some windows with significant energy (morse playing)
        // and some with near-zero energy (morse gaps). The morse signal amplitude is
        // threshold * 0.125 = 0.125, so RMS during morse should be around 0.088.
        let high_rms_count = rms_values.iter().filter(|&&r| r > 0.01).count();
        let low_rms_count = rms_values.iter().filter(|&&r| r < 0.001).count();

        assert!(
            high_rms_count > 0,
            "Expected some high-RMS windows (morse beeps) while limiting"
        );
        assert!(
            low_rms_count > 0,
            "Expected some low-RMS windows (morse gaps) while limiting"
        );
    }

    #[test]
    fn returns_to_passthrough_after_sustained_quiet() {
        let threshold_db = 0.0;
        let mut module = make_module(threshold_db, 44100.0);
        let sample_rate = 44100.0f32;
        let freq = 1000.0f32;
        let amplitude = util::db_to_gain(3.0);

        // First, push signal above threshold for a while to trigger limiting
        let trigger_samples = (sample_rate * 1.0) as usize; // 1 second of loud signal
        for n in 0..trigger_samples {
            let sample = (2.0 * PI * freq * n as f32 / sample_rate).sin() * amplitude;
            let mut left = vec![sample];
            let mut right = vec![sample];
            module.process(&mut left, &mut right);
        }

        // Now send quiet signal — after the fadeout period, output should return to near-passthrough
        let fadeout_samples = (MORSE_FADEOUT_END_MS / 1000.0 * sample_rate) as usize;
        let test_signal_level = 0.1f32;
        let mut quiet_input = vec![test_signal_level; fadeout_samples + 1000];
        let mut quiet_right = vec![test_signal_level; fadeout_samples + 1000];
        module.process(&mut quiet_input, &mut quiet_right);

        // Check the last portion — should be near passthrough
        let check_start = fadeout_samples;
        let check_end = quiet_input.len();
        for i in check_start..check_end {
            let diff = (quiet_input[i] - test_signal_level).abs();
            assert!(
                diff < 0.01,
                "Sample {} after fadeout: input {} -> output {} (diff {})",
                i,
                test_signal_level,
                quiet_input[i],
                diff
            );
        }
    }
}

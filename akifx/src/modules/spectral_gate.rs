//! SpectralGate — spectral gate ported from SpectralSuite's SpectralGate.
//!
//! Faithfully ports the SpectralGate FFT processor as a per-bin spectral gate
//! with two thresholds (`gate_high` and `gate_low`) creating a "dead zone"
//! where bins are zeroed. Bins above `gate_high` pass through scaled by
//! `balance_strong_scale`, bins below `gate_low` pass through scaled by
//! `balance_weak_scale`, and bins in between are zeroed.
//!
//! # Parameters (mirroring source)
//!
//! - **Cutoff** (`#[id = "cutoff"]`): Gate threshold (0–1), default 0.6, displayed in dB.
//! - **Balance** (`#[id = "balance"]`): Weak/strong balance (0–1), default 0.7.
//! - **Tilt** (`#[id = "tilt"]`): Frequency tilt amount (0–1), default 0.5.
//! - **Enable Tilt** (`#[id = "enable_tilt"]`): Enable frequency-dependent tilt, default false.
//!
//! # Internal Parameters (recalculated from cutoff/balance)
//!
//! - `gate_high = cutoff^10` — upper threshold (sharp curve)
//! - `gate_low = cutoff * 0.6` — lower threshold
//! - `balance_strong_scale = balance^3` — gain for strong bins
//! - `balance_weak_scale = (1-balance)^4` — gain for weak bins
//!
//! # Gating Behavior
//!
//! For each frequency bin (except DC which passes through):
//! - `mag >= gate_high` → `mag * balance_strong_scale`
//! - `mag < gate_low` → `mag * balance_weak_scale`
//! - else → `0.0`
//!
//! Phase is always copied from input (untouched).

use crate::modules::AkiFxModule;
use crate::modules::spectral_common::SpectralFxCore;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the SpectralGate module.
///
/// Mirrors SpectralSuite's `SpectralGateParameters` parameter set.
#[derive(Params)]
pub struct SpectralGateParams {
    /// Gate cutoff threshold (0–1), displayed in dB.
    #[id = "cutoff"]
    pub cutoff: FloatParam,

    /// Weak/strong balance (0–1).
    #[id = "balance"]
    pub balance: FloatParam,

    /// Tilt amount (0–1).
    #[id = "tilt"]
    pub tilt: FloatParam,

    /// Enable frequency-dependent tilt.
    #[id = "enable_tilt"]
    pub enable_tilt: BoolParam,
}

impl SpectralGateParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            cutoff: FloatParam::new(
                "Cutoff",
                0.6,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|val: f32| {
                if val <= 0.0 {
                    "-inf dB".to_string()
                } else {
                    format!("{:.0} dB", 20.0 * val.log10())
                }
            })),
            balance: FloatParam::new(
                "Weak/Strong Balance",
                0.7,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            tilt: FloatParam::new(
                "Tilt",
                0.5,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            enable_tilt: BoolParam::new("Enable Tilt", false),
        }
    }
}

impl Default for SpectralGateParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// SpectralGate — per-bin spectral gating with two thresholds.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. The spectral
/// callback applies gating logic faithfully ported from the C++ source.
pub struct SpectralGateModule {
    params: Arc<SpectralGateParams>,
    bypass: Arc<AtomicBool>,
    #[allow(dead_code)]
    sample_rate: f32,
    core: SpectralFxCore,
    /// Cached parameter values for change detection.
    cached_cutoff: f32,
    cached_balance: f32,
    /// Recalculated internal gating parameters.
    gate_high: f32,
    gate_low: f32,
    balance_strong_scale: f32,
    balance_weak_scale: f32,
}

impl SpectralGateModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<SpectralGateParams>, bypass: Arc<AtomicBool>) -> Self {
        let mut m = Self {
            params,
            bypass,
            sample_rate: 44100.0,
            core: SpectralFxCore::new(),
            cached_cutoff: -1.0,
            cached_balance: -1.0,
            gate_high: 0.0,
            gate_low: 0.0,
            balance_strong_scale: 1.0,
            balance_weak_scale: 0.0,
        };
        m.recalculate_internal_params();
        m
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(SpectralGateParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Recalculate internal gating parameters from cutoff and balance.
    ///
    /// Faithful to C++ `recalculateInternalParameters()`.
    fn recalculate_internal_params(&mut self) {
        let cutoff = self.params.cutoff.value();
        let balance = self.params.balance.value();

        // Skip if values haven't changed
        if (cutoff - self.cached_cutoff).abs() < 1e-12
            && (balance - self.cached_balance).abs() < 1e-12
        {
            return;
        }

        self.cached_cutoff = cutoff;
        self.cached_balance = balance;

        // Faithful to C++ recalculateInternalParameters()
        self.gate_high = cutoff.powf(10.0);
        self.gate_low = cutoff * 0.6;
        self.balance_strong_scale = balance.powf(3.0);
        self.balance_weak_scale = (1.0 - balance).powf(4.0);
    }

}

impl AkiFxModule for SpectralGateModule {
    fn name(&self) -> &'static str {
        "Spectral Gate"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.core.build_engine();
    }

    fn reset(&mut self) {
        self.core.reset();
    }

    fn latency_samples(&self) -> u64 {
        self.core.latency_samples()
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Recalculate internal params if cutoff or balance changed
        self.recalculate_internal_params();

        // Capture cached parameters for the closure
        let gate_high = self.gate_high;
        let gate_low = self.gate_low;
        let strong_scale = self.balance_strong_scale;
        let weak_scale = self.balance_weak_scale;
        let tilt_enabled = self.params.enable_tilt.value();
        let tilt_raw = self.params.tilt.value();

        // Ensure the engine exists even if initialize() was not called
        // (tests / standalone edge paths).
        self.core.build_if_missing();

        self.core.process(left, right, &mut |num_bins, _chan, _overlap, polar| {
                // Faithful port of SpectralGateFFTProcessor::spectral_process
                // DC bin (0) passes through unchanged

                if tilt_enabled {
                    let tilt = (tilt_raw - 0.5) * 2.0;
                    for n in 1..num_bins {
                        let frac = n as f32 / num_bins as f32;
                        let high_scale_offset =
                            (frac - 0.5) * 2.0 * gate_high * tilt;
                        let gate_high_n = gate_high + high_scale_offset;
                        let low_scale_offset = high_scale_offset * 0.8;
                        let gate_low_n = gate_low + low_scale_offset;

                        let mag = polar[n].magnitude;
                        if mag >= gate_high_n {
                            polar[n].magnitude = mag * strong_scale;
                        } else if mag < gate_low_n {
                            polar[n].magnitude = mag * weak_scale;
                        } else {
                            polar[n].magnitude = 0.0;
                        }
                        // Phase unchanged (faithful to C++)
                    }
                } else {
                    for n in 1..num_bins {
                        let mag = polar[n].magnitude;
                        if mag >= gate_high {
                            polar[n].magnitude = mag * strong_scale;
                        } else if mag < gate_low {
                            polar[n].magnitude = mag * weak_scale;
                        } else {
                            polar[n].magnitude = 0.0;
                        }
                        // Phase unchanged (faithful to C++)
                    }
                }
            },
        );
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> SpectralGateModule {
        let mut m = SpectralGateModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with specific cutoff and balance.
    fn make_module_with_params(cutoff: f32, balance: f32) -> SpectralGateModule {
        let params = Arc::new(SpectralGateParams {
            cutoff: FloatParam::new(
                "Cutoff",
                cutoff,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            balance: FloatParam::new(
                "Balance",
                balance,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            tilt: FloatParam::new(
                "Tilt",
                0.5,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            enable_tilt: BoolParam::new("Enable Tilt", false),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = SpectralGateModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with tilt enabled.
    fn make_module_with_tilt(cutoff: f32, balance: f32, tilt: f32) -> SpectralGateModule {
        let params = Arc::new(SpectralGateParams {
            cutoff: FloatParam::new(
                "Cutoff",
                cutoff,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            balance: FloatParam::new(
                "Balance",
                balance,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            tilt: FloatParam::new(
                "Tilt",
                tilt,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            enable_tilt: BoolParam::new("Enable Tilt", true),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = SpectralGateModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut SpectralGateModule, input: &[f32]) -> Vec<f32> {
        let block_size = BLOCK;
        let num_blocks = input.len().div_ceil(block_size);
        let mut output = vec![0.0f32; input.len()];

        for b in 0..num_blocks {
            let start = b * block_size;
            let end = (start + block_size).min(input.len());
            let mut left = vec![0.0f32; block_size];
            let mut right = vec![0.0f32; block_size];
            let chunk_len = end - start;
            left[..chunk_len].copy_from_slice(&input[start..end]);
            right[..chunk_len].copy_from_slice(&input[start..end]);
            module.process(&mut left, &mut right);
            output[start..end].copy_from_slice(&left[..chunk_len]);
        }

        output
    }

    /// Helper: measure steady-state RMS of a sinusoidal output.
    fn steady_state_rms(output: &[f32], input_len: usize) -> f32 {
        let steady_start = input_len / 2;
        let n = output.len() - steady_start;
        let sum: f32 = output[steady_start..]
            .iter()
            .map(|s| s * s)
            .sum();
        (sum / n as f32).sqrt()
    }

    // ── Test 1: Gate fully open → identity ───────────────────────────────
    //
    // With cutoff=0 (gate_high=0, gate_low=0) and balance=1.0
    // (strong_scale=1.0), all bins pass through at unity.

    #[test]
    fn gate_fully_open_identity() {
        let mut module = make_module_with_params(0.0, 1.0);
        let total = 4096;

        // Impulse at sample 0
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;

        let output = process_signal(&mut module, &input);

        // Output should have a peak at the expected delay position
        let latency = module.latency_samples() as usize;
        // True signal delay is fft_size; the reported value adds a
        // conservative hop (matching the C++ plugins).
        let latency = latency - crate::modules::spectral_common::SPECTRAL_FFT_SIZE / 4;
        let peak_pos = output[latency..]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(i, _)| i + latency)
            .unwrap();

        // The impulse should be within a few samples of the latency position
        assert!(
            (peak_pos as isize - latency as isize).unsigned_abs() < 16,
            "impulse peak at {peak_pos}, expected near {latency}"
        );

        // The output should correlate highly with the delayed input
        let compare_start = latency;
        let compare_end = total - 256;
        let compare_len = compare_end - compare_start;
        let mut corr = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut energy_in = 0.0f32;
        for i in 0..compare_len {
            let o = output[compare_start + i];
            let inp = input[i];
            corr += o * inp;
            energy_out += o * o;
            energy_in += inp * inp;
        }
        let norm = (energy_out * energy_in).sqrt();
        let correlation = if norm > 1e-10 { corr / norm } else { 0.0 };

        assert!(
            correlation > 0.95,
            "identity reconstruction correlation: {correlation} (expected > 0.95)"
        );
    }

    // ── Test 2: Two-tone gating ─────────────────────────────────────────
    //
    // 200 Hz at -40 dB should be attenuated > 20 dB more than 2 kHz at -10 dB.

    #[test]
    fn two_tone_gating() {
        // Choose cutoff=0.7 so that gate_high=0.7^10≈0.028 sits between
        // the two signal levels, and gate_low=0.7*0.6=0.42.
        // With gate_high=0.028:
        // - 200 Hz at -40 dB (mag ~0.01): 0.01 < 0.028 → weak → scaled by (1-0.7)^4=0.0081
        // - 2 kHz at -10 dB (mag ~0.316): 0.316 >= 0.028 → strong → scaled by 0.7^3=0.343
        let mut module = make_module_with_params(0.7, 0.7);
        let total = 8192;

        // Process 200 Hz at -40 dB
        let amp_200 = 0.01; // -40 dB
        let input_200: Vec<f32> = (0..total)
            .map(|i| amp_200 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / SR).sin())
            .collect();
        let output_200 = process_signal(&mut module, &input_200);

        let rms_200 = steady_state_rms(&output_200, total);

        // Reset and process 2 kHz at -10 dB
        module.reset();
        let amp_2k = 0.316; // -10 dB
        let input_2k: Vec<f32> = (0..total)
            .map(|i| amp_2k * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / SR).sin())
            .collect();
        let output_2k = process_signal(&mut module, &input_2k);

        let rms_2k = steady_state_rms(&output_2k, total);

        // Calculate attenuation in dB
        let atten_200 = 20.0 * (rms_200 / amp_200).log10();
        let atten_2k = 20.0 * (rms_2k / amp_2k).log10();

        // 200 Hz should be attenuated > 20 dB more than 2 kHz
        let diff = atten_200 - atten_2k;
        assert!(
            diff < -20.0,
            "200 Hz attenuation ({atten_200:.1} dB) should be > 20 dB more than 2 kHz ({atten_2k:.1} dB), diff = {diff:.1} dB"
        );
    }

    // ── Test 3: Silence remains silent ───────────────────────────────────

    #[test]
    fn silence_remains_silent() {
        let mut module = make_module();
        let total: usize = 4096;
        let input = vec![0.0f32; total];
        let output = process_signal(&mut module, &input);

        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 1e-10,
            "silence must produce silence, got max abs = {max_abs}"
        );
    }

    // ── Test 4: Stability — 10 seconds of noise, bounded output, no NaN ─

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize;

        // Deterministic pseudo-noise (sine sum)
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 317.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 793.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        assert_eq!(output.len(), total, "output length must match input");

        let has_nan = output.iter().any(|s| s.is_nan());
        assert!(!has_nan, "output must not contain NaN");

        let has_inf = output.iter().any(|s| s.is_infinite());
        assert!(!has_inf, "output must not contain infinity");

        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 100.0,
            "output must be bounded, got max abs = {max_abs}"
        );
    }

    // ── Test 5: Reset restores state ─────────────────────────────────────

    #[test]
    fn reset_restores_state() {
        let mut module = make_module();

        // Process some audio to build up state
        let signal: Vec<f32> = (0..4096)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();
        let _ = process_signal(&mut module, &signal);

        // Reset
        module.reset();

        // Process silence after reset — output should be silent
        let silence = vec![0.0f32; 4096];
        let output = process_signal(&mut module, &silence);
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 1e-10,
            "after reset + silence input, output must be silent, got max abs = {max_abs}"
        );
    }

    // ── Test 6: Reset determinism ────────────────────────────────────────
    //
    // Two runs with the same input, reset between them, should produce
    // identical output.

    #[test]
    fn reset_determinism() {
        let mut module1 = make_module();
        let mut module2 = make_module();

        let total = 4096;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        // First run
        let output1 = process_signal(&mut module1, &input);

        // Reset and run again
        module2.reset();
        let output2 = process_signal(&mut module2, &input);

        // Outputs should be identical
        for (i, (a, b)) in output1.iter().zip(output2.iter()).enumerate() {
            assert!((a - b).abs() < 1e-10, "sample {i}: {a} != {b}");
        }
    }

    // ── Test 7: Latency is correct ───────────────────────────────────────

    #[test]
    fn latency_equals_fft_size() {
        let module = make_module();
        assert_eq!(
            module.latency_samples(),
            (crate::modules::spectral_common::SPECTRAL_FFT_SIZE
                + crate::modules::spectral_common::SPECTRAL_FFT_SIZE / 4) as u64,
            "latency should equal FFT size + hop size"
        );
    }

    // ── Test 8: Internal parameter recalculation ─────────────────────────

    #[test]
    fn internal_params_match_cpp() {
        let module = make_module_with_params(0.6, 0.7);

        // gate_high = 0.6^10 ≈ 0.0060466176
        let expected_gate_high = 0.6f32.powf(10.0);
        assert!(
            (module.gate_high - expected_gate_high).abs() < 1e-6,
            "gate_high: {} vs {}",
            module.gate_high,
            expected_gate_high
        );

        // gate_low = 0.6 * 0.6 = 0.36
        assert!(
            (module.gate_low - 0.36).abs() < 1e-6,
            "gate_low: {} vs 0.36",
            module.gate_low
        );

        // strong_scale = 0.7^3 = 0.343
        let expected_strong = 0.7f32.powf(3.0);
        assert!(
            (module.balance_strong_scale - expected_strong).abs() < 1e-6,
            "strong_scale: {} vs {}",
            module.balance_strong_scale,
            expected_strong
        );

        // weak_scale = 0.3^4 = 0.0081
        let expected_weak = 0.3f32.powf(4.0);
        assert!(
            (module.balance_weak_scale - expected_weak).abs() < 1e-6,
            "weak_scale: {} vs {}",
            module.balance_weak_scale,
            expected_weak
        );
    }

    // ── Test 9: Gate closed zeros signal ─────────────────────────────────
    //
    // With cutoff=1.0 and balance=1.0:
    // gate_high=1.0, gate_low=0.6, strong_scale=1.0, weak_scale=0.0
    // All bins with mag < 1.0 → either weak (mag * 0 = 0) or dead zone (0).
    // Only bins with mag >= 1.0 pass through. A signal at -3 dB (0.707)
    // has all bins below 1.0, so output should be strongly attenuated.

    #[test]
    fn gate_closed_attenuates() {
        let mut module = make_module_with_params(1.0, 1.0);
        let total = 4096;

        // Signal at -3 dB (amp=0.707)
        // gate_high=1.0, gate_low=0.6
        // All bins < 1.0 → weak (scale=0) or dead zone → zeroed
        let input: Vec<f32> = (0..total)
            .map(|i| {
                0.707 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin()
            })
            .collect();
        let output = process_signal(&mut module, &input);

        let input_rms = steady_state_rms(&input, total);
        let output_rms = steady_state_rms(&output, total);

        // Output should be significantly attenuated (> 20 dB)
        let atten_db = 20.0 * (output_rms / input_rms).log10();
        assert!(
            atten_db < -20.0,
            "gate should attenuate > 20 dB, got {atten_db:.1} dB (output RMS = {output_rms})"
        );
    }

    // ── Test 10: Tilt mode processes without panic ───────────────────────

    #[test]
    fn tilt_mode_no_panic() {
        let mut module = make_module_with_tilt(0.5, 0.7, 0.8);
        let total = 4096;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let output = process_signal(&mut module, &input);

        assert_eq!(output.len(), total);
        let has_nan = output.iter().any(|s| s.is_nan());
        assert!(!has_nan, "tilt mode output must not contain NaN");
    }

    // ── Test 11: Varying block sizes don't panic ─────────────────────────

    #[test]
    fn varying_block_sizes_no_panic() {
        let mut module = make_module();
        let total: usize = 4096;

        for block_size in [64, 128, 256, 512, 1024, 2048] {
            let input: Vec<f32> = (0..total)
                .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
                .collect();

            let num_blocks = total.div_ceil(block_size);
            let mut output = vec![0.0f32; total];
            for b in 0..num_blocks {
                let start = b * block_size;
                let end = (start + block_size).min(total);
                let mut left = vec![0.0f32; block_size];
                let mut right = vec![0.0f32; block_size];
                let chunk_len = end - start;
                left[..chunk_len].copy_from_slice(&input[start..end]);
                right[..chunk_len].copy_from_slice(&input[start..end]);
                module.process(&mut left, &mut right);
                output[start..end].copy_from_slice(&left[..chunk_len]);
            }

            assert!(
                output.iter().all(|s| s.is_finite()),
                "block_size {block_size}: output contains non-finite values"
            );
        }
    }
}

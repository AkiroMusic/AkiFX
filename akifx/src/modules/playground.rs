//! Playground — spectral processing shell ported from SpectralSuite's Playground.
//!
//! Faithfully ports the Playground FFT processor as a phase vocoder sandbox.
//! The default spectral callback is identity (bins pass through unchanged),
//! making this module transparent when PVOC is enabled. It serves as both a
//! working passthrough with configurable latency and a starting point for
//! spectral effect development.
//!
//! # Parameters (mirroring source)
//!
//! - **Mix** (`#[id = "mix"]`): Dry/wet mix (0–100 %), default 100 %.
//! - **Use PVOC** (`#[id = "use_pvoc"]`): Enable spectral processing, default true.
//! - **Num Overlaps** (`#[id = "num_overlaps"]`): STFT overlap count (1–8), default 4.
//! - **Do Window Rotate** (`#[id = "do_rotate"]`): PV window rotation, default true.
//! - **Threaded FFT** (`#[id = "threaded"]`): Multithread large FFT, default true.
//! - **Use Linear Hann** (`#[id = "linear_hann"]`): Use linear Hann window, default false.
//!
//! # Note
//!
//! Parameters `do_rotate`, `threaded`, and `linear_hann` are ported for
//! parameter-set fidelity with the source. In this implementation, `linear_hann`
//! selects the FFT window type (Rectangular when true, Hann when false), while
//! `threaded` is informational only (our SpectralEngine is single-threaded).
//! `do_rotate` is informational (our engine handles windowing internally).

use crate::modules::AkiFxModule;
use crate::stft::{SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.
const DEFAULT_FFT_SIZE: usize = 2048;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Playground module.
///
/// Mirrors SpectralSuite's `PlaygroundParameters` parameter set.
#[derive(Params)]
pub struct PlaygroundParams {
    /// Dry/wet mix (0 % = fully dry, 100 % = fully wet).
    #[id = "mix"]
    pub mix: FloatParam,

    /// Enable phase vocoder (spectral) processing.
    /// When false, audio passes through without FFT processing.
    #[id = "use_pvoc"]
    pub use_pvoc: BoolParam,

    /// Number of STFT overlap instances (1–8).
    /// Higher overlap improves time resolution at the cost of CPU.
    #[id = "num_overlaps"]
    pub num_overlaps: IntParam,

    /// PV window rotation (informational — our engine handles this internally).
    #[id = "do_rotate"]
    pub do_window_rotate: BoolParam,

    /// Multithread large FFT (informational — our engine is single-threaded).
    #[id = "threaded"]
    pub use_threaded_fft: BoolParam,

    /// Use linear Hann window (when true, selects Rectangular window).
    #[id = "linear_hann"]
    pub use_linear_hann: BoolParam,
}

impl PlaygroundParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            mix: FloatParam::new(
                "Mix",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            use_pvoc: BoolParam::new("Use Phase Vocoder", true),
            num_overlaps: IntParam::new(
                "Number of Overlaps",
                4,
                IntRange::Linear {
                    min: 1,
                    max: 8,
                },
            ),
            do_window_rotate: BoolParam::new("PV: Window Rotate", true),
            use_threaded_fft: BoolParam::new("PV: Multithread Large FFT", true),
            use_linear_hann: BoolParam::new("PV: Use Linear Hann", false),
        }
    }
}

impl Default for PlaygroundParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Playground — spectral processing shell with configurable PVOC parameters.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. The spectral
/// callback is identity by default, faithfully porting the C++ source's
/// `spectral_process` which copies input bins to output bins unchanged.
pub struct PlaygroundModule {
    params: Arc<PlaygroundParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    /// Dry signal buffers for mix blending.
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
    /// Cached overlap count for change detection.
    cached_overlaps: i32,
}

impl PlaygroundModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<PlaygroundParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
            cached_overlaps: -1,
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(PlaygroundParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild the spectral engine with current parameter values.
    fn build_engine(&mut self) {
        let overlap_count = self.params.num_overlaps.value() as usize;
        let window = if self.params.use_linear_hann.value() {
            WindowType::Rectangular
        } else {
            WindowType::Hann
        };
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count,
            window,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
        self.cached_overlaps = self.params.num_overlaps.value();
    }

    /// Ensure dry buffers are large enough for the current block size.
    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

impl AkiFxModule for PlaygroundModule {
    fn name(&self) -> &'static str {
        "Playground"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.build_engine();
        self.ensure_dry_buffers(self.fft_size);
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
    }

    fn latency_samples(&self) -> u64 {
        self.engine
            .as_ref()
            .map(|e| e.latency_samples() as u64)
            .unwrap_or(0)
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mix = self.params.mix.value() / 100.0;

        // Rebuild engine if overlap count changed at runtime
        if self.params.num_overlaps.value() != self.cached_overlaps {
            self.build_engine();
        }

        // Ensure engine exists
        if self.engine.is_none() {
            self.build_engine();
        }

        // When PVOC disabled: passthrough (scaled by mix)
        if !self.params.use_pvoc.value() {
            if mix < 1.0 {
                let dry_gain = 1.0 - mix;
                for s in left.iter_mut() {
                    *s *= dry_gain;
                }
                for s in right.iter_mut() {
                    *s *= dry_gain;
                }
            }
            return;
        }

        // Ensure dry buffers are large enough
        self.ensure_dry_buffers(left.len());

        // Save dry signal for mix blending
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        // Process through spectral engine with identity callback.
        // Faithfully ports PlaygroundFFTProcessor::spectral_process which
        // copies in[i] to out[i] for all bins (lines 29-33 of .h).
        let engine = self.engine.as_mut().expect("engine must be initialized");
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |_num_bins, _polar| {
                // Identity: bins pass through unchanged.
            },
        );

        // Apply dry/wet mix
        if mix < 1.0 {
            let dry_gain = 1.0 - mix;
            for (s, d) in left.iter_mut().zip(self.dry_buf_l.iter()) {
                *s = *s * mix + *d * dry_gain;
            }
            for (s, d) in right.iter_mut().zip(self.dry_buf_r.iter()) {
                *s = *s * mix + *d * dry_gain;
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> PlaygroundModule {
        let mut m = PlaygroundModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut PlaygroundModule, input: &[f32]) -> Vec<f32> {
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

    // ── Test 1: Identity reconstruction ──────────────────────────────────
    //
    // With default identity callback, the spectral engine should reconstruct
    // the input (delayed by latency_samples) with high correlation.

    #[test]
    fn identity_reconstruction() {
        let mut module = make_module();
        let fft_size = 2048;
        let total = fft_size * 8;

        // Test signal: sum of two sines (avoids zero-crossing relative error)
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 880.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        // After the engine fills (fft_size samples), output should track input.
        // Use cross-correlation to find the actual delay and gain.
        let latency = module.latency_samples() as usize;
        let compare_start = latency;
        let compare_end = total - fft_size;
        let compare_len = compare_end - compare_start;

        // Compute cross-correlation at the expected delay
        let mut corr = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut energy_in = 0.0f32;
        for i in 0..compare_len {
            let o = output[compare_start + i];
            let inp = input[i]; // input at t=0 corresponds to output at t=latency
            corr += o * inp;
            energy_out += o * o;
            energy_in += inp * inp;
        }

        // Normalized correlation coefficient
        let norm = (energy_out * energy_in).sqrt();
        let correlation = if norm > 1e-10 {
            corr / norm
        } else {
            0.0
        };

        assert!(
            correlation > 0.99,
            "identity reconstruction correlation: {correlation} (expected > 0.99)"
        );

        // Also verify the gain is reasonable (COLA gain for Hann + 4x overlap)
        let gain = corr / energy_in.max(1e-10);
        assert!(
            gain > 0.1 && gain < 100.0,
            "identity reconstruction gain: {gain} (expected reasonable COLA gain)"
        );
    }

    // ── Test 2: Silence remains silent ───────────────────────────────────

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

    // ── Test 3: Effect behavior — identity callback preserves signal ─────
    //
    // The default spectral callback is identity (faithful to C++ source).
    // Verify that the output waveform correlates highly with the delayed input.

    #[test]
    fn identity_callback_preserves_signal() {
        let mut module = make_module();
        let total = 8192;

        // Impulse train: one impulse every 256 samples
        let mut input = vec![0.0f32; total];
        for i in (0..total).step_by(256) {
            input[i] = 1.0;
        }

        let output = process_signal(&mut module, &input);

        // The output should have peaks at impulse locations (delayed by latency)
        let latency = module.latency_samples() as usize;

        // Check that output peaks align with input impulses (shifted by latency)
        for impulse_pos in (0..total).step_by(256) {
            let expected_peak_pos = impulse_pos + latency;
            if expected_peak_pos >= total {
                break;
            }

            // Find the actual peak near the expected position
            let search_start = expected_peak_pos.saturating_sub(16);
            let search_end = (expected_peak_pos + 16).min(total);
            let actual_peak = output[search_start..search_end]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
                .map(|(i, _)| i + search_start)
                .unwrap_or(expected_peak_pos);

            assert!(
                (actual_peak as isize - expected_peak_pos as isize).unsigned_abs() < 16,
                "impulse at {impulse_pos}: expected peak near {expected_peak_pos}, \
                 found at {actual_peak}"
            );
        }
    }

    // ── Test 4: Stability — 10 seconds of noise, bounded output, no NaN ─

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize; // 10 seconds

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

        // No NaN
        let has_nan = output.iter().any(|s| s.is_nan());
        assert!(!has_nan, "output must not contain NaN");

        // No infinity
        let has_inf = output.iter().any(|s| s.is_infinite());
        assert!(!has_inf, "output must not contain infinity");

        // Bounded output (max amplitude should be reasonable for overlap-add)
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 100.0,
            "output must be bounded, got max abs = {max_abs}"
        );
    }

    /// Helper: create a module with PVOC disabled.
    fn make_module_pvoc_off() -> PlaygroundModule {
        let params = Arc::new(PlaygroundParams {
            mix: FloatParam::new(
                "Mix",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            ),
            use_pvoc: BoolParam::new("Use Phase Vocoder", false),
            num_overlaps: IntParam::new(
                "Number of Overlaps",
                4,
                IntRange::Linear {
                    min: 1,
                    max: 8,
                },
            ),
            do_window_rotate: BoolParam::new("PV: Window Rotate", true),
            use_threaded_fft: BoolParam::new("PV: Multithread Large FFT", true),
            use_linear_hann: BoolParam::new("PV: Use Linear Hann", false),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = PlaygroundModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with a specific mix percentage.
    fn make_module_mix(mix_pct: f32) -> PlaygroundModule {
        let params = Arc::new(PlaygroundParams {
            mix: FloatParam::new(
                "Mix",
                mix_pct,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            ),
            use_pvoc: BoolParam::new("Use Phase Vocoder", true),
            num_overlaps: IntParam::new(
                "Number of Overlaps",
                4,
                IntRange::Linear {
                    min: 1,
                    max: 8,
                },
            ),
            do_window_rotate: BoolParam::new("PV: Window Rotate", true),
            use_threaded_fft: BoolParam::new("PV: Multithread Large FFT", true),
            use_linear_hann: BoolParam::new("PV: Use Linear Hann", false),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = PlaygroundModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    // ── Test 5: Use PVOC disabled → passthrough ─────────────────────────

    #[test]
    fn pvoc_disabled_passthrough() {
        let mut module = make_module_pvoc_off();

        let total = 1024;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();
        let output = process_signal(&mut module, &input);

        // With PVOC off and mix=100%, output must equal input
        for (i, (o, e)) in output.iter().zip(input.iter()).enumerate() {
            assert!(
                (o - e).abs() < 1e-6,
                "sample {i}: expected {e}, got {o}"
            );
        }
    }

    // ── Test 6: Mix parameter blends dry and wet ─────────────────────────

    #[test]
    fn mix_blends_dry_and_wet() {
        let mut module = make_module_mix(50.0);

        let total = 4096;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let output = process_signal(&mut module, &input);

        // Output should be between 0 and input amplitude (mix averages wet and dry)
        let input_peak = input.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let output_peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // With 50% mix, output peak should be roughly half the wet peak
        // (wet peak ≈ input peak × COLA gain, then mixed 50/50 with dry)
        assert!(
            output_peak < input_peak * 5.0,
            "50% mix output peak {output_peak} seems too large vs input {input_peak}"
        );
        assert!(output_peak > 0.0, "50% mix output must not be silent");
    }

    // ── Test 7: Reset restores state ─────────────────────────────────────

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

    // ── Test 8: Latency is correct ───────────────────────────────────────

    #[test]
    fn latency_equals_fft_size() {
        let module = make_module();
        assert_eq!(
            module.latency_samples(),
            DEFAULT_FFT_SIZE as u64,
            "latency should equal FFT size"
        );
    }

    // ── Test 9: Varying block sizes don't panic ──────────────────────────

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

            // Just verify no NaN/Inf
            assert!(
                output.iter().all(|s| s.is_finite()),
                "block_size {block_size}: output contains non-finite values"
            );
        }
    }
}

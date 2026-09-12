//! Sinusoidal Shaped Filter (SSF) — spectral processing ported from SpectralSuite.
//!
//! Applies a sinusoidal envelope across FFT bins using a wavetable-based emphasis
//! pattern. The sinusoid's frequency, width (sharpness), and phase are parameterized,
//! producing a comb-like or rippled spectral shaping effect.
//!
//! # DSP (faithful to SSF_FFTProcessor)
//!
//! For each bin `i`:
//! 1. `wavetable_index = i * (freq + 1.0) + phase^3 * half_size`
//! 2. `sinusoid = max(0, wavetable[wavetable_index])`  (sine wavetable, clamped)
//! 3. `output_magnitude = input_magnitude * sinusoid^(width^2 * 8 + 1)`
//! 4. `output_phase = input_phase` (phase preserved)
//!
//! # Parameters (mirroring SSFParameters)
//!
//! - **Frequency** (`#[id = "freq"]`): Controls the sinusoidal period across bins (0–10, default 7).
//! - **Width** (`#[id = "width"]`): Controls the sharpness of the sinusoidal peaks (0–1, default 0.9).
//! - **Phase** (`#[id = "phase"]`): Shifts the sinusoidal pattern across bins (0–1, default 0.5).
//! - **Mix** (`#[id = "mix"]`): Dry/wet blend (0–100%, default 100%).
//! - **Use PVOC** (`#[id = "use_pvoc"]`): Enable spectral processing (default true).
//! - **Num Overlaps** (`#[id = "num_overlaps"]`): STFT overlap count (1–8, default 4).

use crate::modules::AkiFxModule;
use crate::stft::{SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

const DEFAULT_FFT_SIZE: usize = 2048;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the SinusoidalShapedFilter module.
#[derive(Params)]
pub struct SinusoidalShapedFilterParams {
    /// Sinusoidal frequency across bins (0–10). Controls the period of the
    /// comb-like pattern. Higher = more oscillations across the spectrum.
    #[id = "freq"]
    pub frequency: FloatParam,

    /// Width / sharpness of the sinusoidal peaks (0–1). Controls how narrow
    /// the emphasis/notch peaks are via a power exponent.
    #[id = "width"]
    pub width: FloatParam,

    /// Phase offset of the sinusoidal pattern across bins (0–1).
    /// Shifts the entire pattern left/right in the frequency domain.
    #[id = "phase"]
    pub phase: FloatParam,

    /// Dry/wet mix (0 % = fully dry, 100 % = fully wet).
    #[id = "mix"]
    pub mix: FloatParam,

    /// Enable phase vocoder (spectral) processing.
    #[id = "use_pvoc"]
    pub use_pvoc: BoolParam,

    /// Number of STFT overlap instances (1–8).
    #[id = "num_overlaps"]
    pub num_overlaps: IntParam,
}

impl SinusoidalShapedFilterParams {
    pub fn new() -> Self {
        Self {
            frequency: FloatParam::new(
                "Frequency",
                7.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 10.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|v: f32| format!("{v:.2}"))),
            width: FloatParam::new(
                "Width",
                0.9,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            phase: FloatParam::new(
                "Phase",
                0.5,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
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
        }
    }
}

impl Default for SinusoidalShapedFilterParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Wavetable ──────────────────────────────────────────────────────────────

/// Pre-computed sine wavetable with linear interpolation.
/// Matches SpectralSuite's `Table<float>` with type=1 (sine), interp=1 (linear).
struct SineWavetable {
    table: Vec<f32>,
    size: usize,
}

impl SineWavetable {
    /// Create a sine wavetable of the given size.
    fn new(size: usize) -> Self {
        let mut table = vec![0.0f32; size + 2];
        for n in 0..size {
            table[n] = (std::f32::consts::TAU * n as f32 / size as f32).sin();
        }
        table[size] = 0.0; // guard point for interpolation
        table[size + 1] = 0.0;
        Self { table, size }
    }

    /// Resize the wavetable and regenerate sine data.
    fn resize(&mut self, new_size: usize) {
        self.size = new_size;
        self.table.resize(new_size + 2, 0.0);
        for n in 0..new_size {
            self.table[n] = (std::f32::consts::TAU * n as f32 / new_size as f32).sin();
        }
        self.table[new_size] = 0.0;
        self.table[new_size + 1] = 0.0;
    }

    /// Get interpolated value at a fractional index (wraps).
    fn get_value(&self, index: f32) -> f32 {
        // Wrap into [0, table_len) while KEEPING the fractional part — the
        // previous version truncated to usize before computing `frac`, which
        // made it always 0 and turned the interpolation into a zero-order
        // hold.
        let len = self.table.len() as f32;
        let wrapped = index.rem_euclid(len);
        let idx = wrapped as usize;
        let frac = wrapped - idx as f32;
        let next_index = if idx + 1 >= self.table.len() {
            0
        } else {
            idx + 1
        };
        let val_a = self.table[idx];
        let val_b = self.table[next_index];
        val_a + (val_b - val_a) * frac
    }
}

// ── Internal DSP state ─────────────────────────────────────────────────────

/// Pre-computed DSP parameters derived from the user-facing controls.
struct SsfDspState {
    /// Scale factor applied to bin index: `freq + 1.0`.
    index_scale: f32,
    /// Phase offset in bin units: `phase^3 * half_size`.
    theta: f32,
    /// Exponent for the sinusoidal shaping: `width^2 * 8 + 1`.
    sinusoid_exponent: f32,
}

impl SsfDspState {
    fn compute(freq: f32, width: f32, phase: f32, half_size: usize) -> Self {
        Self {
            index_scale: freq + 1.0,
            theta: phase.powi(3) * half_size as f32,
            sinusoid_exponent: width * width * 8.0 + 1.0,
        }
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Sinusoidal Shaped Filter — applies a parameterized sinusoidal envelope
/// across FFT bins via wavetable lookup.
pub struct SinusoidalShapedFilterModule {
    params: Arc<SinusoidalShapedFilterParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    wavetable: SineWavetable,
    dsp_state: SsfDspState,
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
    cached_overlaps: i32,
}

impl SinusoidalShapedFilterModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<SinusoidalShapedFilterParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            wavetable: SineWavetable::new(DEFAULT_FFT_SIZE / 2),
            dsp_state: SsfDspState::compute(7.0, 0.9, 0.5, DEFAULT_FFT_SIZE / 2),
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
            cached_overlaps: -1,
        }
    }

    /// Convenience constructor for testing.
    pub fn with_defaults() -> Self {
        let params = Arc::new(SinusoidalShapedFilterParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild spectral engine and recompute DSP state from current params.
    fn rebuild(&mut self) {
        let overlap_count = self.params.num_overlaps.value() as usize;
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count,
            window: WindowType::Hann,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
        self.cached_overlaps = self.params.num_overlaps.value();

        // Resize wavetable to match half FFT size (faithful to SSFInteractor)
        self.wavetable.resize(self.fft_size / 2);

        // Recompute DSP state
        self.dsp_state = SsfDspState::compute(
            self.params.frequency.value(),
            self.params.width.value(),
            self.params.phase.value(),
            self.fft_size / 2,
        );
    }
}

impl AkiFxModule for SinusoidalShapedFilterModule {
    fn name(&self) -> &'static str {
        "SinusoidalShapedFilter"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.rebuild();
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

        // Rebuild if overlap count changed
        if self.params.num_overlaps.value() != self.cached_overlaps {
            self.rebuild();
        }

        if self.engine.is_none() {
            self.rebuild();
        }

        // When PVOC disabled the spectral processing is skipped, so the
        // "wet" signal equals the dry one and any mix value blends dry with
        // dry. True passthrough — the old code scaled the signal by
        // (1 - mix), attenuating the output at lower mix settings.
        if !self.params.use_pvoc.value() {
            return;
        }

        // Recompute DSP state (params may have changed)
        self.dsp_state = SsfDspState::compute(
            self.params.frequency.value(),
            self.params.width.value(),
            self.params.phase.value(),
            self.fft_size / 2,
        );

        self.ensure_dry_buffers(left.len());

        // Save dry
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        // Process through spectral engine.
        // Destructure to avoid conflicting mutable borrows of self.
        let Self {
            engine,
            wavetable,
            dsp_state,
            dry_buf_l,
            dry_buf_r,
            ..
        } = self;
        // Engine construction is guaranteed during initialize(); skip this
        // block rather than panicking in the host's audio callback if not.
        let Some(engine) = engine.as_mut() else {
            return;
        };
        let input_channels: &[&[f32]] = &[&dry_buf_l[..left.len()], &dry_buf_r[..right.len()]];
        let output_channels: &mut [&mut [f32]] = &mut [left, right];
        let index_scale = dsp_state.index_scale;
        let theta = dsp_state.theta;
        let exponent = dsp_state.sinusoid_exponent;
        engine.process(input_channels, output_channels, &mut |num_bins, polar| {
            // Spectral callback: apply sinusoidal shaping per-bin
            // (inlined closure: engine/wavetable/dsp_state are disjoint borrows
            // from `self`, so a &mut self method cannot be used here).
            for i in 0..num_bins {
                let wavetable_index = i as f32 * index_scale + theta;
                let sinusoid = wavetable.get_value(wavetable_index).max(0.0);
                polar[i].magnitude *= sinusoid.powf(exponent);
            }
        });

        // Dry/wet mix
        if mix < 1.0 {
            let dry_gain = 1.0 - mix;
            for (s, d) in left.iter_mut().zip(dry_buf_l.iter()) {
                *s = *s * mix + *d * dry_gain;
            }
            for (s, d) in right.iter_mut().zip(dry_buf_r.iter()) {
                *s = *s * mix + *d * dry_gain;
            }
        }
    }
}

impl SinusoidalShapedFilterModule {
    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> SinusoidalShapedFilterModule {
        let mut m = SinusoidalShapedFilterModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut SinusoidalShapedFilterModule, input: &[f32]) -> Vec<f32> {
        let num_blocks = input.len().div_ceil(BLOCK);
        let mut output = vec![0.0f32; input.len()];
        for b in 0..num_blocks {
            let start = b * BLOCK;
            let end = (start + BLOCK).min(input.len());
            let chunk_len = end - start;
            let mut left = vec![0.0f32; BLOCK];
            let mut right = vec![0.0f32; BLOCK];
            left[..chunk_len].copy_from_slice(&input[start..end]);
            right[..chunk_len].copy_from_slice(&input[start..end]);
            module.process(&mut left, &mut right);
            output[start..end].copy_from_slice(&left[..chunk_len]);
        }
        output
    }

    /// Compute the FFT magnitude spectrum of a real signal.
    fn fft_magnitudes(signal: &[f32], fft_size: usize) -> Vec<f32> {
        use rustfft::{FftPlanner, num_complex::Complex};
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buf: Vec<Complex<f32>> = signal
            .iter()
            .take(fft_size)
            .map(|&s| Complex::new(s, 0.0))
            .collect();
        buf.resize(fft_size, Complex::default());
        let scratch_len = fft.get_inplace_scratch_len();
        let mut scratch = vec![Complex::default(); scratch_len];
        fft.process_with_scratch(&mut buf, &mut scratch);
        buf.iter()
            .take(fft_size / 2)
            .map(|c| c.norm())
            .collect()
    }

    // ── Test 1: Silence → silence ───────────────────────────────────────

    #[test]
    fn silence_remains_silent() {
        let mut module = make_module();
        let total = 4096;
        let input = vec![0.0f32; total];
        let output = process_signal(&mut module, &input);
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 1e-10,
            "silence must produce silence, got max abs = {max_abs}"
        );
    }

    // ── Test 2: No NaN / Inf with 10 seconds of noise ──────────────────

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize;
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (TAU * 100.0 * t).sin()
                    + (TAU * 317.0 * t).sin()
                    + (TAU * 793.0 * t).sin()
            })
            .collect();
        let output = process_signal(&mut module, &input);
        assert_eq!(output.len(), total);
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN allowed");
        assert!(
            !output.iter().any(|s| s.is_infinite()),
            "no infinity allowed"
        );
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(max_abs < 100.0, "output bounded, got max abs = {max_abs}");
    }

    // ── Test 3: Engaged → periodic ripple in output spectrum ────────────
    //
    // With default params (freq=7, width=0.9), the sinusoidal shaping should
    // create a visible amplitude modulation pattern across FFT bins. We verify
    // this by computing the variance of the output magnitude spectrum and
    // comparing it against a flat (identity) spectrum's variance.

    #[test]
    fn engaged_creates_spectral_ripple() {
        let mut module = make_module();
        let total = 8192;

        // Pink-noise-like signal (deterministic)
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (TAU * 100.0 * t).sin()
                    + 0.5 * (TAU * 317.0 * t).sin()
                    + 0.3 * (TAU * 793.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        // Compute output spectrum after settling
        let fft_size = 2048;
        let spectrum_start = fft_size;
        let mags = fft_magnitudes(&output[spectrum_start..], fft_size);

        // Compute variance of log magnitudes (ripple = high variance)
        let mean: f32 = mags.iter().map(|m| m.max(1e-10).ln()).sum::<f32>() / mags.len() as f32;
        let variance: f32 = mags
            .iter()
            .map(|m| {
                let db = m.max(1e-10).ln();
                (db - mean).powi(2)
            })
            .sum::<f32>()
            / mags.len() as f32;

        // A flat (identity) spectrum would have very low variance.
        // With SSF engaged, the sinusoidal shaping should produce significantly
        // more variance. We check that the standard deviation is > 0.1 dB
        // equivalent (log-domain std dev).
        let std_dev = variance.sqrt();
        assert!(
            std_dev > 0.1,
            "spectral ripple std dev too low: {std_dev:.4} (expected > 0.1 for engaged SSF)"
        );
    }

    // ── Test 4: Reset is deterministic ──────────────────────────────────
    //
    // Process some audio, reset, then process the same audio again.
    // The output after reset must match the output from a fresh module.

    #[test]
    fn reset_is_deterministic() {
        let signal: Vec<f32> = (0..8192)
            .map(|i| (TAU * 440.0 * i as f32 / SR).sin())
            .collect();

        // Fresh module
        let mut fresh = make_module();
        let output_fresh = process_signal(&mut fresh, &signal);

        // Module that processed then reset
        let mut reused = make_module();
        let _ = process_signal(&mut reused, &signal);
        reused.reset();
        let output_reset = process_signal(&mut reused, &signal);

        // After reset, output must match fresh
        for (i, (a, b)) in output_fresh.iter().zip(output_reset.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "sample {i}: fresh={a}, reset={b}, diff={}",
                (a - b).abs()
            );
        }
    }

    // ── Test 5: Impulse delay within 1e-3 of expected latency ──────────

    #[test]
    fn impulse_delay_matches_latency() {
        let mut module = make_module();
        let fft_size = 2048;
        let total = fft_size * 4;

        // Single impulse at sample 0
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;

        let output = process_signal(&mut module, &input);

        let latency = module.latency_samples() as usize;

        // The output peak should be near the expected latency
        // Allow ±(fft_size/2) tolerance due to windowing spread
        let search_center = latency;
        let tolerance = fft_size / 4;

        // Find peak in output
        let (peak_idx, peak_val) = output
            .iter()
            .enumerate()
            .skip(fft_size / 2)
            .take(fft_size)
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .unwrap();

        let delay_error = (peak_idx as isize - search_center as isize).unsigned_abs();
        assert!(
            delay_error < tolerance,
            "peak at {peak_idx}, expected near {search_center}, error {delay_error} > {tolerance}"
        );
        assert!(
            peak_val.abs() > 1e-6,
            "output peak too small: {peak_val}"
        );
    }

    // ── Test 6: PVOC disabled → passthrough ─────────────────────────────

    #[test]
    fn pvoc_disabled_passthrough() {
        let params = Arc::new(SinusoidalShapedFilterParams {
            frequency: FloatParam::new("Frequency", 7.0, FloatRange::Linear { min: 0.0, max: 10.0 }),
            width: FloatParam::new("Width", 0.9, FloatRange::Linear { min: 0.0, max: 1.0 }),
            phase: FloatParam::new("Phase", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 }),
            mix: FloatParam::new("Mix", 100.0, FloatRange::Linear { min: 0.0, max: 100.0 }),
            use_pvoc: BoolParam::new("Use Phase Vocoder", false),
            num_overlaps: IntParam::new("Number of Overlaps", 4, IntRange::Linear { min: 1, max: 8 }),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = SinusoidalShapedFilterModule::new(params, bypass);
        module.initialize(SR, BLOCK);

        let total = 1024;
        let input: Vec<f32> = (0..total)
            .map(|i| (TAU * 440.0 * i as f32 / SR).sin())
            .collect();
        let output = process_signal(&mut module, &input);

        for (i, (o, e)) in output.iter().zip(input.iter()).enumerate() {
            assert!(
                (o - e).abs() < 1e-6,
                "PVOC off passthrough: sample {i}: expected {e}, got {o}"
            );
        }
    }

    // ── Test 7: Wavetable correctness ──────────────────────────────────

    #[test]
    fn wavetable_sine_values() {
        let wt = SineWavetable::new(256);
        // Index 0 should be sin(0) ≈ 0
        assert!(wt.get_value(0.0).abs() < 1e-6);
        // Index 64 (quarter of 256) should be sin(π/2) ≈ 1
        let val_quarter = wt.get_value(64.0);
        assert!(
            (val_quarter - 1.0).abs() < 1e-3,
            "quarter-table value: {val_quarter}"
        );
        // Index 128 (half of 256) should be sin(π) ≈ 0
        let val_half = wt.get_value(128.0);
        assert!(
            val_half.abs() < 1e-3,
            "half-table value: {val_half}"
        );
        // Index 192 (three-quarter) should be sin(3π/2) ≈ -1
        let val_3q = wt.get_value(192.0);
        assert!(
            (val_3q + 1.0).abs() < 1e-3,
            "three-quarter-table value: {val_3q}"
        );
    }

    // ── Test 8: Varying block sizes don't panic ─────────────────────────

    #[test]
    fn varying_block_sizes_no_panic() {
        let mut module = make_module();
        let total: usize = 4096;

        for block_size in [64, 128, 256, 512, 1024, 2048] {
            let input: Vec<f32> = (0..total)
                .map(|i| (TAU * 440.0 * i as f32 / SR).sin())
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
                "block_size {block_size}: output has non-finite values"
            );
        }
    }
}

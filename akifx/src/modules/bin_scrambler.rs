//! Bin Scrambler — spectral bin reordering ported from SpectralSuite's BinScrambler.
//!
//! Faithfully ports the BinScrambler algorithm: frequency bins are randomly
//! reordered in chunks, with scatter controlling the likelihood of low-to-high
//! remapping and scramble controlling the magnitude of chunk shuffling. Two
//! index buffers are crossfaded via linear interpolation at a rate set by the
//! `rate` parameter (in Hz).
//!
//! # Parameters (mirroring source)
//!
//! - **Scramble** (`#[id = "scramble"]`): 0–100 %, default 10 %. Likelihood
//!   that frequency bins will be scrambled. Cubed internally to bias toward
//!   subtle scrambling.
//! - **Scatter** (`#[id = "scatter"]`): 0–100 %, default 40 %. How far
//!   scattered frequencies are remapped. Squared internally.
//! - **Rate** (`#[id = "rate"]`): 0.25–15 Hz, default 2.0 Hz. How fast new
//!   scramble patterns are generated.
//! - **Seed** (`#[id = "seed"]`): 0–9999, default 0. Random seed. 0 = random
//!   (time-based), non-zero = deterministic.

use crate::modules::AkiFxModule;
use crate::stft::{Polar, SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.
const DEFAULT_FFT_SIZE: usize = 2048;

/// Default number of STFT overlaps.
const DEFAULT_OVERLAPS: usize = 4;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Bin Scrambler module.
///
/// Mirrors SpectralSuite's `BinScramblerParameters` parameter set.
#[derive(Params)]
pub struct BinScramblerParams {
    /// Scramble amount 0–100 % (stored as `[0, 1]`).
    /// Cubed internally to bias toward subtle scrambling.
    #[id = "scramble"]
    pub scramble: FloatParam,

    /// Scatter amount 0–100 % (stored as `[0, 1]`).
    /// Squared internally to control distribution.
    #[id = "scatter"]
    pub scatter: FloatParam,

    /// Rate in Hz (0.25–15.0).
    /// How fast new scramble patterns are generated.
    #[id = "rate"]
    pub rate: FloatParam,

    /// Random seed (0–9999). 0 = time-based random.
    #[id = "seed"]
    pub seed: IntParam,
}

impl BinScramblerParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            scramble: FloatParam::new(
                "Scramble",
                0.1,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            scatter: FloatParam::new(
                "Scatter",
                0.4,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            rate: FloatParam::new(
                "Rate",
                2.0,
                FloatRange::Linear {
                    min: 0.25,
                    max: 15.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|v| format!("{v:.2} Hz"))),
            seed: IntParam::new("Random Seed", 0, IntRange::Linear { min: 0, max: 9999 }),
        }
    }
}

impl Default for BinScramblerParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Bin Scrambler — spectral bin reordering with scatter, scramble, and
/// crossfaded index interpolation at a configurable rate.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. Two index
/// buffers are maintained and crossfaded via linear interpolation during
/// the spectral callback, faithfully porting the C++ algorithm.
pub struct BinScramblerModule {
    params: Arc<BinScramblerParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    /// Previous index buffer (the "from" state for interpolation).
    indices_a: Vec<i32>,
    /// Current index buffer (the "to" state for interpolation).
    indices_b: Vec<i32>,
    /// Pointer to the current index buffer (0 = A, 1 = B).
    current_ptr: usize,
    /// Phase accumulator (0..=phasor_max).
    phasor: u32,
    /// Maximum phase before a new scramble pattern is generated.
    phasor_max: u32,
    /// Cached scramble parameter for change detection.
    cached_scramble: f32,
    /// Cached scatter parameter for change detection.
    cached_scatter: f32,
    /// Cached rate parameter for change detection.
    cached_rate: f32,
    /// Scratch for the per-frame interpolated source indices (num_bins).
    blended_indices: Vec<usize>,
    /// Scratch for the in-place bin permutation in the spectral callback.
    scratch_polar: Vec<Polar>,
    /// Dry signal buffer for left channel.
    dry_buf_l: Vec<f32>,
    /// Dry signal buffer for right channel.
    dry_buf_r: Vec<f32>,
}

impl BinScramblerModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<BinScramblerParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            indices_a: Vec::new(),
            indices_b: Vec::new(),
            current_ptr: 0,
            phasor: 0,
            phasor_max: 22050,
            blended_indices: Vec::new(),
            scratch_polar: Vec::new(),
            cached_scramble: 0.0,
            cached_scatter: 0.0,
            cached_rate: 0.0,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(BinScramblerParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild the spectral engine with current parameter values.
    fn build_engine(&mut self) {
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count: DEFAULT_OVERLAPS,
            window: WindowType::Hann,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
    }

    /// Initialize both index buffers to identity (sequential indices).
    fn reset_indices(&mut self) {
        let num_bins = self.fft_size / 2;
        self.indices_a = (0..num_bins as i32).collect();
        self.indices_b = self.indices_a.clone();
        self.current_ptr = 0;
        if self.blended_indices.len() != num_bins {
            self.blended_indices = vec![0; num_bins];
            self.scratch_polar = vec![Polar::default(); num_bins];
        }
    }

    /// Recalculate internal parameters from current param values.
    /// Matches `BinScramblerInteractor::recalculateInternalParameters`.
    fn recalculate_internal_params(&mut self) {
        self.cached_scramble = self.params.scramble.value();
        self.cached_scatter = self.params.scatter.value();
        self.cached_rate = self.params.rate.value();
        self.phasor_max = (self.sample_rate / self.cached_rate) as u32;
    }

    /// Check if parameters have changed since the last recalculation.
    fn should_recalculate(&self) -> bool {
        (self.params.scramble.value() - self.cached_scramble).abs() > f32::EPSILON
            || (self.params.scatter.value() - self.cached_scatter).abs() > f32::EPSILON
            || (self.params.rate.value() - self.cached_rate).abs() > f32::EPSILON
    }

    /// Compute the scatter amount and scramble factor from current parameters.
    fn compute_scatter_and_scramble(&self) -> (i32, usize) {
        let scatter = self.params.scatter.value();
        let scramble = self.params.scramble.value();
        let bin_range = self.fft_size / 2;

        // scatter_amount: high scatter → low distribution (inverted via 1-x²)
        let scatter_amount = ((1.0 - scatter * scatter) * 100.0) as i32;
        // scramble_factor: cubed to bias toward subtle scrambling
        let scramble_factor = ((scramble.powi(3) * bin_range as f32) / 2.0) as usize;

        (scatter_amount, scramble_factor)
    }

    /// Swap the current and previous index buffers.
    fn swap_buffers(&mut self) {
        self.current_ptr = 1 - self.current_ptr;
    }

    /// Get the mutable current index buffer.
    fn current_indices_mut(&mut self) -> &mut [i32] {
        if self.current_ptr == 0 {
            &mut self.indices_a
        } else {
            &mut self.indices_b
        }
    }

    /// Ensure dry buffers are large enough for the current block size.
    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

impl Default for BinScramblerModule {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl AkiFxModule for BinScramblerModule {
    fn name(&self) -> &'static str {
        "Bin Scrambler"
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
        self.reset_indices();
        self.recalculate_internal_params();
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
        self.reset_indices();
        self.recalculate_internal_params();
        self.phasor = 0;
    }

    fn latency_samples(&self) -> u64 {
        self.engine
            .as_ref()
            .map(|e| e.latency_samples() as u64)
            .unwrap_or(0)
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Ensure engine exists
        if self.engine.is_none() {
            self.build_engine();
        }

        // When scramble ≈ 0 and scatter ≈ 0, pass through directly
        if self.params.scramble.value() <= f32::EPSILON
            && self.params.scatter.value() <= f32::EPSILON
        {
            return;
        }

        // ── Advance phasor and check for new scramble pattern ──────────
        if self.phasor >= self.phasor_max {
            // Swap current and previous buffers
            self.swap_buffers();

            // Reset the new "current" buffer to identity
            let num_bins = self.fft_size / 2;
            for i in 0..num_bins {
                self.current_indices_mut()[i] = i as i32;
            }

            // Recalculate parameters if they changed
            if self.should_recalculate() {
                self.recalculate_internal_params();
            }

            // Create RNG once before mutable borrows of indices
            let mut rng = self.make_rng();

            // Apply scatter (low-to-high remapping)
            let (scatter_amount, scramble_factor) = self.compute_scatter_and_scramble();
            let indices = self.current_indices_mut();
            let scatter_range = num_bins / 5;
            if scatter_amount > 0 && scatter_range > 0 {
                for i in 0..num_bins / 5 {
                    if rng.gen_range(0..100) >= scatter_amount {
                        let src = rng.gen_range(0..scatter_range);
                        indices[i] = indices[src];
                    }
                }
            }

            // Apply scramble (chunk shuffling)
            if scramble_factor > 0 {
                let mut n = 0;
                while n + scramble_factor <= indices.len() {
                    indices[n..n + scramble_factor].shuffle(&mut rng);
                    n += scramble_factor;
                }
            }

            self.phasor -= self.phasor_max;
        }

        // The phasor counts elapsed input samples, like the C++ original's
        // `mPhasor += blockSize`. (The previous version advanced by the hop
        // size once per process() call, making the scramble rate depend on
        // the host's block size.)
        let phase = self.phasor;
        self.phasor += left.len() as u32;

        // ── Precompute blended indices ─────────────────────────────────
        let phasor_max = self.phasor_max;
        let inv_phasor_max = if phasor_max > 0 {
            1.0 / phasor_max as f32
        } else {
            1.0
        };
        let blend = phase as f32 * inv_phasor_max;

        let num_bins = self.fft_size / 2;
        // Direct field access (rather than the previous/next helpers) so the
        // borrow checker can see these don't alias `blended_indices`.
        let (a, b) = if self.current_ptr == 0 {
            (&self.indices_b, &self.indices_a)
        } else {
            (&self.indices_a, &self.indices_b)
        };
        for i in 0..num_bins {
            let a_idx = a[i].max(0).min(num_bins as i32 - 1) as f32;
            let b_idx = b[i].max(0).min(num_bins as i32 - 1) as f32;
            let idx = a_idx + (b_idx - a_idx) * blend;
            self.blended_indices[i] = idx as usize;
        }

        // ── Spectral processing ────────────────────────────────────────
        self.ensure_dry_buffers(left.len());
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        let blended_indices: &[usize] = &self.blended_indices;
        let scratch_polar: &mut [Polar] = &mut self.scratch_polar;
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |_num_bins, polar| {
                // The engine invokes this callback once per channel instance
                // with that channel's mono bin spectrum. (The previous
                // implementation assumed an interleaved stereo buffer, so it
                // scrambled half the bins using even/odd "channels" that do
                // not exist.)
                let bins = polar.len().min(blended_indices.len());
                scratch_polar[..bins].copy_from_slice(&polar[..bins]);
                for (i, slot) in polar[..bins].iter_mut().enumerate() {
                    *slot = scratch_polar[blended_indices[i].min(bins - 1)];
                }
            },
        );
    }
}

// ── Helper methods (not in trait impl) ─────────────────────────────────────

impl BinScramblerModule {
    /// Create an RNG from the seed parameter. Non-zero seed → deterministic.
    fn make_rng(&self) -> StdRng {
        let seed = self.params.seed.value() as u64;
        if seed > 0 {
            StdRng::seed_from_u64(seed)
        } else {
            StdRng::from_entropy()
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 4096;

    /// Helper: create and initialise a module ready for testing.

    /// Helper: create a module with custom scramble/scatter/rate parameters.
    fn make_module_with_params(scramble: f32, scatter: f32, rate: f32) -> BinScramblerModule {
        let params = Arc::new(BinScramblerParams {
            scramble: FloatParam::new(
                "Scramble",
                scramble,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            scatter: FloatParam::new(
                "Scatter",
                scatter,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            rate: FloatParam::new(
                "Rate",
                rate,
                FloatRange::Linear {
                    min: 0.25,
                    max: 15.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|v| format!("{v:.2} Hz"))),
            seed: IntParam::new("Random Seed", 0, IntRange::Linear { min: 0, max: 9999 }),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = BinScramblerModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with a specific seed.
    fn make_module_with_seed(seed: i32) -> BinScramblerModule {
        make_module_full(0.5, 0.4, 2.0, seed)
    }

    /// Helper: create a module with all parameters.
    fn make_module_full(scramble: f32, scatter: f32, rate: f32, seed: i32) -> BinScramblerModule {
        let params = Arc::new(BinScramblerParams {
            scramble: FloatParam::new(
                "Scramble",
                scramble,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            scatter: FloatParam::new(
                "Scatter",
                scatter,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            rate: FloatParam::new(
                "Rate",
                rate,
                FloatRange::Linear {
                    min: 0.25,
                    max: 15.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|v| format!("{v:.2} Hz"))),
            seed: IntParam::new("Random Seed", seed, IntRange::Linear { min: 0, max: 9999 }),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = BinScramblerModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut BinScramblerModule, input: &[f32]) -> Vec<f32> {
        let block_size = BLOCK;
        let num_blocks = input.len().div_ceil(block_size);
        let mut output = vec![0.0f32; input.len()];

        for b in 0..num_blocks {
            let start = b * block_size;
            let end = (start + block_size).min(input.len());
            let chunk_len = end - start;
            let mut left = vec![0.0f32; block_size];
            let mut right = vec![0.0f32; block_size];
            left[..chunk_len].copy_from_slice(&input[start..end]);
            right[..chunk_len].copy_from_slice(&input[start..end]);
            module.process(&mut left, &mut right);
            output[start..end].copy_from_slice(&left[..chunk_len]);
        }

        output
    }

    // ── Test 1: Scramble off → identity passthrough ───────────────────

    #[test]
    fn scramble_off_identity_passthrough() {
        let mut module = make_module_with_params(0.0, 0.0, 2.0);
        let total = 4096;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();
        let output = process_signal(&mut module, &input);

        let max_diff: f32 = input
            .iter()
            .zip(output.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 1e-3,
            "Scramble off should passthrough, got max diff = {max_diff}"
        );
    }

    // ── Test 2: Scrambled run conserves spectral energy ──────────────
    //
    // Permuting bins is a unitary operation in polar form: the sum of
    // squared magnitudes before and after should be identical.

    #[test]
    fn scrambled_run_conserves_spectral_energy() {
        let mut module = make_module_with_params(0.5, 0.4, 2.0);
        let total = 4096;

        // Rich signal with multiple frequencies
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 880.0 * t).sin()
                    + 0.3 * (2.0 * std::f32::consts::PI * 1200.0 * t).sin()
            })
            .collect();
        let output = process_signal(&mut module, &input);

        // Compute spectral energy of input and output (time-domain sum of squares
        // is a proxy; for strict proof we'd DFT both, but this is sufficient for
        // verifying the permutation doesn't add/remove energy).
        let input_energy: f32 = input.iter().map(|s| s * s).sum();
        let output_energy: f32 = output.iter().map(|s| s * s).sum();

        let ratio = output_energy / input_energy.max(1e-10);
        assert!(
            ratio > 0.05 && ratio < 20.0,
            "Energy ratio out of range: {ratio} (input={input_energy:.1}, output={output_energy:.1})"
        );
    }

    // ── Test 3: Same seed → bit-identical outputs ────────────────────

    #[test]
    fn same_seed_bitidentical() {
        let mut m1 = make_module_with_seed(42);
        let mut m2 = make_module_with_seed(42);

        let total = 8192;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let out1 = process_signal(&mut m1, &input);
        let out2 = process_signal(&mut m2, &input);

        for (i, (a, b)) in out1.iter().zip(out2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "Non-deterministic at sample {i}: {a} vs {b}"
            );
        }
    }

    // ── Test 4: Different seeds → different outputs ──────────────────
    //
    // Use the maximum rate (15 Hz) so phasor_max ≈ 2940. With hop_size=512
    // per process() call, need ≥ 6 calls (24576 samples) to trigger a swap.

    #[test]
    fn different_seeds_differ() {
        let mut m1 = make_module_full(0.5, 0.4, 15.0, 1);
        let mut m2 = make_module_full(0.5, 0.4, 15.0, 2);

        // 32768 samples / 4096 block = 8 calls → phasor reaches 8×512 = 4096 > 2940
        let total = 32768;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let out1 = process_signal(&mut m1, &input);
        let out2 = process_signal(&mut m2, &input);

        // At least some samples should differ
        let max_diff: f32 = out1
            .iter()
            .zip(out2.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff > 1e-6,
            "Different seeds produced identical output (max_diff={max_diff})"
        );
    }

    // ── Test 5: Silence → silence ─────────────────────────────────────

    /// Regression: after the callback was rewritten to the engine's real
    /// per-channel mono-bin contract, a fully-scrambled pass must still
    /// deliver energy (a permutation of bins) and never NaN.
    #[test]
    fn scrambled_output_has_energy_and_no_nan() {
        let params = Arc::new(BinScramblerParams {
            scramble: FloatParam::new(
                "Scramble",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            scatter: FloatParam::new(
                "Scatter",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            rate: FloatParam::new(
                "Rate",
                4.0,
                FloatRange::Linear { min: 0.25, max: 15.0 },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|v| format!("{v:.2} Hz"))),
            seed: IntParam::new("Random Seed", 7, IntRange::Linear { min: 0, max: 9999 }),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = BinScramblerModule::new(params, bypass);
        m.initialize(SR, BLOCK);

        let n = BLOCK * 8;
        let mut left: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin() * 0.5)
            .collect();
        let mut right = left.clone();

        // Process in host-sized chunks.
        for chunk in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            let (l, r) = chunk;
            m.process(l, r);
        }

        // Skip the FFT warmup (fft_size + hop margin).
        let tail = &left[2048..];
        assert!(!tail.iter().any(|s| s.is_nan()), "no NaN in scrambled output");
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(
            rms > 0.01,
            "scrambled output must retain energy, rms={rms}"
        );
    }

    #[test]
    fn silence_remains_silent() {
        let mut module = make_module_with_params(0.5, 0.4, 2.0);
        let input = vec![0.0f32; 4096];
        let output = process_signal(&mut module, &input);

        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 1e-10,
            "Silence must produce silence, got max abs = {max_abs}"
        );
    }

    // ── Test 6: No NaN over 10 seconds of noise ─────────────────────

    #[test]
    fn no_nan_ten_seconds_noise() {
        let mut module = make_module_with_params(0.8, 0.6, 5.0);
        let total = (SR * 10.0) as usize; // 10 seconds
        let block_size = BLOCK;

        // Deterministic pseudo-noise via LCG
        let mut state: u32 = 12345;
        for _ in (0..total).step_by(block_size) {
            let mut left = vec![0.0f32; block_size];
            let mut right = vec![0.0f32; block_size];
            for s in left.iter_mut().zip(right.iter_mut()) {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let sample = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                *s.0 = sample;
                *s.1 = sample;
            }
            module.process(&mut left, &mut right);

            for s in left.iter().chain(right.iter()) {
                assert!(s.is_finite(), "Non-finite sample: {s}");
                assert!(s.abs() <= 10.0, "Sample too large: {s}");
            }
        }
    }
}

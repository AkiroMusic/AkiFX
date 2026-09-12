//! Frequency Shift — spectral bin shifting ported from SpectralSuite's FrequencyShift.
//!
//! Shifts frequency content by moving spectral bins up or down by a configurable
//! number of Hertz, and optionally scales the frequency axis by a multiplier.
//!
//! # Parameters (mirroring source)
//!
//! - **Shift** (`#[id = "shift"]`): Frequency shift in Hz (-500 to 500), default 0.
//! - **Scale** (`#[id = "scale"]`): Frequency scale factor (0.25–3.0), default 1.0.
//!
//! # Algorithm
//!
//! The spectral callback implements two independent bin transformations:
//!
//! 1. **Scale**: For each input bin `i`, write to output bin `floor(i * scale)` if in range.
//!    When scale=1.0 this is identity; scale>1 expands frequencies; scale<1 compresses.
//!
//! 2. **Shift**: Computes `binShift = round(shiftHz * fftSize / sampleRate)` and copies
//!    input bins `[startIndex..endIndex]` to `out[i + binShift]`. The source range is
//!    chosen so shifted bins stay within `[0, halfFftSize)`:
//!    - binShift > 0: source range is `[0, halfSize - binShift)`
//!    - binShift < 0: source range is `[|binShift|, halfSize)`
//!
//! Both transformations are applied: scale runs first (all bins), then shift runs
//! (restricted range), matching the C++ `spectral_process` exactly.

use crate::modules::AkiFxModule;
use crate::stft::{Polar, SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.
const DEFAULT_FFT_SIZE: usize = 2048;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the FrequencyShift module.
///
/// Mirrors SpectralSuite's `FrequencyShiftPluginParameters` parameter set.
#[derive(Params)]
pub struct FrequencyShiftParams {
    /// Frequency shift in Hz. Positive shifts up, negative shifts down.
    #[id = "shift"]
    pub shift: FloatParam,

    /// Frequency scale factor. 1.0 = no scaling, <1 compresses, >1 expands.
    #[id = "scale"]
    pub scale: FloatParam,
}

impl FrequencyShiftParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            shift: FloatParam::new(
                "Frequency Shift",
                0.0,
                FloatRange::Linear {
                    min: -500.0,
                    max: 500.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            scale: FloatParam::new(
                "Frequency Scale",
                1.0,
                FloatRange::Skewed {
                    min: 0.25,
                    max: 3.0,
                    factor: 0.5,
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(3)),
        }
    }
}

impl Default for FrequencyShiftParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Frequency Shift — spectral bin shifting with configurable Hz offset and scale.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. The spectral callback
/// implements bin scaling and shifting per the C++ `FrequencyShiftFFTProcessor`.
pub struct FrequencyShiftModule {
    params: Arc<FrequencyShiftParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    /// Dry signal buffers for mix blending.
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
}

impl FrequencyShiftModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<FrequencyShiftParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(FrequencyShiftParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild the spectral engine.
    fn build_engine(&mut self) {
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count: 4,
            window: WindowType::Hann,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
    }

    /// Ensure dry buffers are large enough for the current block size.
    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

impl AkiFxModule for FrequencyShiftModule {
    fn name(&self) -> &'static str {
        "Frequency Shift"
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
        // Ensure engine exists
        if self.engine.is_none() {
            self.build_engine();
        }

        // Ensure dry buffers are large enough
        self.ensure_dry_buffers(left.len());

        // Save dry signal for wet-only processing (no mix param — always 100% wet)
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        let shift_hz = self.params.shift.value();
        let scale = self.params.scale.value();
        let sample_rate = self.sample_rate;
        let fft_size = self.fft_size;
        let half_size = fft_size / 2;

        // Pre-compute shift parameters (matching recalculateInternalParameters)
        let bin_shift = if sample_rate > 0.0 {
            let bin_width = fft_size as f32 / sample_rate;
            (shift_hz * bin_width).round() as i32
        } else {
            0
        };

        let (shift_start, shift_end) = if bin_shift > 0 {
            (0usize, (half_size as i32 - bin_shift).max(0) as usize)
        } else {
            ((-bin_shift).max(0) as usize, half_size)
        };

        // Process through spectral engine.
        // Engine construction is guaranteed during initialize(); skip this
        // block rather than panicking in the host's audio callback if not.
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |num_bins, polar| {
                frequency_shift_callback(polar, num_bins, bin_shift, shift_start, shift_end, scale);
            },
        );
    }
}

/// Spectral callback implementing the FrequencyShift bin transformations.
///
/// Faithfully ports `FrequencyShiftFFTProcessor::spectral_process` from C++.
fn frequency_shift_callback(
    polar: &mut [Polar],
    num_bins: usize,
    bin_shift: i32,
    shift_start: usize,
    shift_end: usize,
    scale: f32,
) {
    let scale_is_zero = (scale - 0.0).abs() < f32::EPSILON;

    // Fast path: no shift and scale is zero → identity copy
    if bin_shift == 0 && scale_is_zero {
        return; // polar already contains input (in-place from engine)
    }

    // We need a temporary buffer to avoid overwriting input during remapping
    let mut tmp = vec![Polar::new(0.0, 0.0); num_bins];

    // Step 1: Scale transformation (if scale is non-zero)
    // out[floor(i * scale)] = in[i] for all i where newIndex < numBins
    if !scale_is_zero {
        for i in 0..num_bins {
            let new_index = (i as f32 * scale) as usize;
            if new_index < num_bins {
                tmp[new_index] = polar[i];
            }
        }
    }

    // Step 2: Shift transformation (if bin_shift != 0)
    // out[i + binShift] = in[i] for i in [shiftStart, shiftEnd)
    if bin_shift != 0 {
        for i in shift_start..shift_end {
            let dest = (i as i32 + bin_shift) as usize;
            if dest < num_bins {
                tmp[dest] = polar[i];
            }
        }
    }

    // Copy result back to polar buffer
    polar[..num_bins].copy_from_slice(&tmp[..num_bins]);
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stft::Polar;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> FrequencyShiftModule {
        let mut m = FrequencyShiftModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with specific shift and scale values.
    fn make_module_with_params(shift: f32, scale: f32) -> FrequencyShiftModule {
        let params = Arc::new(FrequencyShiftParams {
            shift: FloatParam::new(
                "Frequency Shift",
                shift,
                FloatRange::Linear {
                    min: -500.0,
                    max: 500.0,
                },
            ),
            scale: FloatParam::new(
                "Frequency Scale",
                scale,
                FloatRange::Skewed {
                    min: 0.25,
                    max: 3.0,
                    factor: 0.5,
                },
            ),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = FrequencyShiftModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut FrequencyShiftModule, input: &[f32]) -> Vec<f32> {
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

    /// Helper: find the dominant frequency bin of a signal via FFT peak.
    /// Returns the bin index with highest magnitude.
    fn dominant_bin(signal: &[f32], fft_size: usize) -> usize {
        use rustfft::num_complex::Complex;
        use rustfft::FftPlanner;

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);

        let mut buf: Vec<Complex<f32>> = signal
            .iter()
            .take(fft_size)
            .map(|&s| Complex::new(s, 0.0))
            .collect();
        buf.resize(fft_size, Complex::new(0.0, 0.0));

        let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        fft.process_with_scratch(&mut buf, &mut scratch);

        // Only check first half (positive frequencies)
        let half = fft_size / 2;
        let (peak_bin, _) = buf[..half]
            .iter()
            .enumerate()
            .max_by(|a, b| {
                a.1.norm()
                    .partial_cmp(&b.1.norm())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        peak_bin
    }

    // ── Test 1: Shift=0 is identity (impulse delay within 1e-3) ────────

    #[test]
    fn shift_zero_is_identity() {
        let mut module = make_module(); // shift=0, scale=1.0
        let total = 4096;

        // Impulse at t=0
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;

        let output = process_signal(&mut module, &input);

        // With shift=0 and scale=1.0, the spectral callback is effectively identity
        // (scale copies all bins to same position, shift=0 does nothing).
        // Output should track input delayed by latency.
        let latency = module.latency_samples() as usize;

        // Find peak in output near where the input impulse should appear
        let search_start = latency.saturating_sub(32);
        let search_end = (latency + 32).min(total);
        let output_peak = output[search_start..search_end]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(i, v)| (i + search_start, *v))
            .unwrap_or((0, 0.0));

        assert!(
            (output_peak.0 as isize - latency as isize).unsigned_abs() < 32,
            "impulse peak should be near latency ({latency}), found at {}",
            output_peak.0
        );
        assert!(
            output_peak.1.abs() > 0.01,
            "impulse peak should be significant, got {}",
            output_peak.1
        );

        // No NaN
        assert!(
            output.iter().all(|s| s.is_finite()),
            "output must not contain NaN or Inf"
        );
    }

    // ── Test 2: Positive shift moves spectral peak UP ──────────────────

    #[test]
    fn positive_shift_moves_peak_up() {
        let fft_size = 2048;
        // Create a sine wave at a known frequency
        let freq = 1000.0f32;
        let shift_hz = 200.0f32;
        let total = fft_size * 4;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect();

        // Measure the dominant bin of the input
        let input_bin = dominant_bin(&input, fft_size);
        let expected_bin_width = fft_size as f32 / SR;
        let expected_input_bin = (freq * expected_bin_width).round() as usize;
        assert!(
            (input_bin as isize - expected_input_bin as isize).unsigned_abs() <= 1,
            "input dominant bin {input_bin} should be near expected {expected_input_bin}"
        );

        // Process with positive shift
        let mut module = make_module_with_params(shift_hz, 1.0);
        let output = process_signal(&mut module, &input);

        // Measure the dominant bin of the output (skip latency region)
        let skip = fft_size;
        let output_bin = dominant_bin(&output[skip..], fft_size);

        // Expected shift in bins: shift_hz * fft_size / sample_rate
        let expected_bin_shift = (shift_hz * expected_bin_width).round() as usize;

        assert!(
            output_bin >= input_bin,
            "positive shift should move peak up: input_bin={input_bin}, output_bin={output_bin}"
        );
        assert!(
            (output_bin as isize - input_bin as isize - expected_bin_shift as isize).unsigned_abs() <= 2,
            "shifted peak should be near input_bin + binShift: \
             input_bin={input_bin}, output_bin={output_bin}, expected_bin_shift={expected_bin_shift}"
        );
    }

    // ── Test 3: Negative shift symmetric ───────────────────────────────

    #[test]
    fn negative_shift_moves_peak_down() {
        let fft_size = 2048;
        let freq = 2000.0f32;
        let shift_hz = -300.0f32;
        let total = fft_size * 4;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect();

        let input_bin = dominant_bin(&input, fft_size);

        let mut module = make_module_with_params(shift_hz, 1.0);
        let output = process_signal(&mut module, &input);

        let skip = fft_size;
        let output_bin = dominant_bin(&output[skip..], fft_size);

        let expected_bin_width = fft_size as f32 / SR;
        let expected_bin_shift = (shift_hz.abs() * expected_bin_width).round() as usize;

        assert!(
            output_bin <= input_bin,
            "negative shift should move peak down: input_bin={input_bin}, output_bin={output_bin}"
        );
        assert!(
            (input_bin as isize - output_bin as isize - expected_bin_shift as isize).unsigned_abs() <= 2,
            "shifted peak should be near input_bin - |binShift|: \
             input_bin={input_bin}, output_bin={output_bin}, expected_bin_shift={expected_bin_shift}"
        );
    }

    // ── Test 4: Silence → silence ──────────────────────────────────────

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

    // ── Test 5: No NaN with 10 seconds of noise ───────────────────────

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module_with_params(150.0, 1.5);
        let total = (SR * 10.0) as usize;

        // Deterministic pseudo-noise (sum of sines at coprime-ish freqs)
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

        // Bounded output
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 100.0,
            "output must be bounded, got max abs = {max_abs}"
        );
    }

    // ── Test 6: Reset is deterministic ─────────────────────────────────

    #[test]
    fn reset_is_deterministic() {
        let mut module1 = make_module_with_params(200.0, 1.2);
        let mut module2 = make_module_with_params(200.0, 1.2);

        let total = 4096;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        // Process some audio to build up state
        let _ = process_signal(&mut module1, &input);
        let _ = process_signal(&mut module2, &input);

        // Reset both
        module1.reset();
        module2.reset();

        // Process same signal after reset — outputs must be identical
        let out1 = process_signal(&mut module1, &input);
        let out2 = process_signal(&mut module2, &input);

        for (i, (a, b)) in out1.iter().zip(out2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "sample {i}: out1={a}, out2={b} — reset must be deterministic"
            );
        }
    }

    // ── Test 7: Scale factor compresses/expands ────────────────────────

    #[test]
    fn scale_compresses_and_expands() {
        let fft_size = 2048;
        let freq = 1500.0f32;
        let total = fft_size * 4;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect();

        let input_bin = dominant_bin(&input, fft_size);

        // Scale > 1 should move peak to higher bin
        let mut module_up = make_module_with_params(0.0, 1.5);
        let output_up = process_signal(&mut module_up, &input);
        let bin_up = dominant_bin(&output_up[fft_size..], fft_size);

        // Scale < 1 should move peak to lower bin
        let mut module_down = make_module_with_params(0.0, 0.5);
        let output_down = process_signal(&mut module_down, &input);
        let bin_down = dominant_bin(&output_down[fft_size..], fft_size);

        let expected_up = (input_bin as f32 * 1.5).round() as usize;
        let expected_down = (input_bin as f32 * 0.5).round() as usize;

        assert!(
            bin_up > input_bin,
            "scale=1.5 should move peak up: input={input_bin}, up={bin_up}"
        );
        assert!(
            (bin_up as isize - expected_up as isize).unsigned_abs() <= 2,
            "scale=1.5 peak should be near {expected_up}, got {bin_up}"
        );

        assert!(
            bin_down < input_bin,
            "scale=0.5 should move peak down: input={input_bin}, down={bin_down}"
        );
        assert!(
            (bin_down as isize - expected_down as isize).unsigned_abs() <= 2,
            "scale=0.5 peak should be near {expected_down}, got {bin_down}"
        );
    }

    // ── Test 8: Latency equals FFT size ────────────────────────────────

    #[test]
    fn latency_equals_fft_size() {
        let module = make_module();
        assert_eq!(
            module.latency_samples(),
            DEFAULT_FFT_SIZE as u64,
            "latency should equal FFT size"
        );
    }

    // ── Test 9: Combined shift + scale ─────────────────────────────────

    #[test]
    fn combined_shift_and_scale() {
        let mut module = make_module_with_params(100.0, 1.2);
        let total = 8192;

        // Just verify no NaN/Inf with both parameters active
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 500.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 1200.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        assert!(
            output.iter().all(|s| s.is_finite()),
            "combined shift+scale must not produce NaN/Inf"
        );

        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(
            max_abs < 50.0,
            "combined output must be bounded, got max abs = {max_abs}"
        );
    }

    // ── Test 10: Callback unit tests ───────────────────────────────────

    #[test]
    fn callback_identity_when_no_shift_and_scale_1() {
        // When bin_shift=0 and scale=1.0: scale copies in[i] to out[i*1]=out[i],
        // shift does nothing. Should be identity.
        let num_bins = 8;
        let mut polar: Vec<Polar> = (0..num_bins)
            .map(|i| Polar::new(i as f32 + 1.0, i as f32 * 0.5))
            .collect();
        let original = polar.clone();

        frequency_shift_callback(&mut polar, num_bins, 0, 0, num_bins, 1.0);

        for (i, (got, expected)) in polar.iter().zip(original.iter()).enumerate() {
            assert!(
                (got.magnitude - expected.magnitude).abs() < 1e-6
                    && (got.phase - expected.phase).abs() < 1e-6,
                "bin {i}: expected {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn callback_scale_only() {
        // scale=2.0: out[i*2] = in[i]
        let num_bins = 8;
        let mut polar: Vec<Polar> = (0..num_bins)
            .map(|i| Polar::new(i as f32 + 1.0, 0.0))
            .collect();

        frequency_shift_callback(&mut polar, num_bins, 0, 0, num_bins, 2.0);

        // in[0] → out[0], in[1] → out[2], in[2] → out[4], in[3] → out[6]
        assert_eq!(polar[0].magnitude, 1.0);
        assert_eq!(polar[2].magnitude, 2.0);
        assert_eq!(polar[4].magnitude, 3.0);
        assert_eq!(polar[6].magnitude, 4.0);
        // Bins 1, 3, 5, 7 should be zero
        assert_eq!(polar[1].magnitude, 0.0);
        assert_eq!(polar[3].magnitude, 0.0);
    }

    #[test]
    fn callback_shift_only() {
        // bin_shift=+2, scale=0.0: out[i+2] = in[i] for i in [0, halfSize-2)
        let num_bins = 8;
        let mut polar: Vec<Polar> = (0..num_bins)
            .map(|i| Polar::new(i as f32 + 1.0, 0.0))
            .collect();

        // bin_shift=2, shift_start=0, shift_end=6 (halfSize - 2 = 8 - 2 = 6)
        frequency_shift_callback(&mut polar, num_bins, 2, 0, 6, 0.0);

        // in[0] → out[2], in[1] → out[3], ..., in[5] → out[7]
        assert_eq!(polar[2].magnitude, 1.0);
        assert_eq!(polar[3].magnitude, 2.0);
        assert_eq!(polar[4].magnitude, 3.0);
        assert_eq!(polar[5].magnitude, 4.0);
        assert_eq!(polar[6].magnitude, 5.0);
        assert_eq!(polar[7].magnitude, 6.0);
        // Bins 0, 1 should be zero
        assert_eq!(polar[0].magnitude, 0.0);
        assert_eq!(polar[1].magnitude, 0.0);
    }

    #[test]
    fn callback_negative_shift() {
        // bin_shift=-2, scale=0.0: out[i-2] = in[i] for i in [2, halfSize)
        let num_bins = 8;
        let mut polar: Vec<Polar> = (0..num_bins)
            .map(|i| Polar::new(i as f32 + 1.0, 0.0))
            .collect();

        // bin_shift=-2, shift_start=2, shift_end=8
        frequency_shift_callback(&mut polar, num_bins, -2, 2, 8, 0.0);

        // in[2] → out[0], in[3] → out[1], ..., in[7] → out[5]
        assert_eq!(polar[0].magnitude, 3.0);
        assert_eq!(polar[1].magnitude, 4.0);
        assert_eq!(polar[2].magnitude, 5.0);
        assert_eq!(polar[3].magnitude, 6.0);
        assert_eq!(polar[4].magnitude, 7.0);
        assert_eq!(polar[5].magnitude, 8.0);
        // Bins 6, 7 should be zero
        assert_eq!(polar[6].magnitude, 0.0);
        assert_eq!(polar[7].magnitude, 0.0);
    }

    // ── Test 11: Varying block sizes don't panic ───────────────────────

    #[test]
    fn varying_block_sizes_no_panic() {
        let mut module = make_module_with_params(100.0, 0.8);
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

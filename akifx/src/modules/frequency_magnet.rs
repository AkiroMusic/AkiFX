//! Frequency Magnet — spectral bin concentration effect ported from SpectralSuite.
//!
//! Faithfully ports `FrequencyMagnetFFTProcessor::spectral_process` from C++.
//! Dynamically merges all FFT bins toward a single target frequency bin.
//! At strength=0 (width=1 in C++) the spectrum passes through unchanged;
//! at strength=1 (width=0 in C++) all energy collapses to the target bin.
//!
//! # Parameters (mirroring source)
//!
//! - **Frequency** (`#[id = "freq"]`): Target frequency (20–2000 Hz), default 800 Hz.
//! - **Strength** (`#[id = "strength"]`): Magnet strength (0–100 %), default 50 %.
//!   Maps to C++ `width` as `1.0 - strength` so strength=0 is identity.
//! - **Width Bias** (`#[id = "bias"]`): Curve exponent for the width parameter
//!   (0–1), default 0.01. Controls how aggressively intermediate strengths compress.
//! - **Use Legacy Mode** (`#[id = "use_legacy"]`): Legacy high-frequency shift behaviour.
//!
//! # Algorithm
//!
//! Below the target bin, each input bin's position is remapped via a power curve
//! whose exponent is controlled by `width * 8 / 8` (i.e. just `width`). As
//! strength increases (width decreases), the exponent shrinks and bins pile up
//! near the target. Above the target, the same idea applies with a different
//! exponent formula from the C++ source. Magnitudes accumulate in a temp buffer;
//! phase is preserved from the input.

use crate::modules::AkiFxModule;
use crate::stft::{Polar, SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.
const DEFAULT_FFT_SIZE: usize = 2048;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Frequency Magnet module.
///
/// Mirrors SpectralSuite's `FrequencyMagnetParameters` parameter set.
#[derive(Params)]
pub struct FrequencyMagnetParams {
    /// Target frequency in Hz (20–2000, default 800).
    #[id = "freq"]
    pub frequency: FloatParam,

    /// Magnet strength (0–100 %, default 50 %).
    ///
    /// At 0 % the spectrum is unchanged; at 100 % all bins collapse to the
    /// target frequency. Maps to C++ `width` as `1.0 - strength`.
    #[id = "strength"]
    pub strength: FloatParam,

    /// Width bias — curve exponent for the width parameter (0–1, default 0.01).
    #[id = "bias"]
    pub width_bias: FloatParam,

    /// Use legacy high-frequency shift logic (default false).
    #[id = "use_legacy"]
    pub use_legacy: BoolParam,
}

impl FrequencyMagnetParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            frequency: FloatParam::new(
                "Frequency",
                800.0,
                FloatRange::Linear {
                    min: 20.0,
                    max: 2000.0,
                },
            )
            .with_unit(" Hz"),
            strength: FloatParam::new(
                "Strength",
                50.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            width_bias: FloatParam::new(
                "Width Bias",
                0.01,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            ),
            use_legacy: BoolParam::new("Use Legacy Mode", false),
        }
    }
}

impl Default for FrequencyMagnetParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Frequency Magnet — spectral bin concentration effect.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. The spectral
/// callback remaps bin magnitudes toward a target frequency, creating a
/// focused spectral "magnet" effect controlled by the strength parameter.
pub struct FrequencyMagnetModule {
    params: Arc<FrequencyMagnetParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    /// Engine input snapshots: the engine processes these and writes to the
    /// channel slices in place.
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
    /// Reused scratch for the spectral callback so FFT frames never
    /// allocate on the audio thread.
    scratch: MagnetScratch,
}

/// Scratch buffers for [`spectral_process_magnet`], reused across FFT frames.
struct MagnetScratch {
    in_mag: Vec<f32>,
    in_phase: Vec<f32>,
    temp: Vec<f32>,
}

impl MagnetScratch {
    fn new(num_bins: usize, fft_size: usize) -> Self {
        Self {
            in_mag: vec![0.0; num_bins],
            in_phase: vec![0.0; num_bins],
            temp: vec![0.0; fft_size],
        }
    }
}

impl FrequencyMagnetModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<FrequencyMagnetParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
            scratch: MagnetScratch::new(DEFAULT_FFT_SIZE / 2, DEFAULT_FFT_SIZE),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(FrequencyMagnetParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild the spectral engine with current parameter values.
    fn build_engine(&mut self) {
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count: 4,
            window: WindowType::Hann,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
        let num_bins = self.fft_size / 2;
        if self.scratch.in_mag.len() != num_bins {
            self.scratch = MagnetScratch::new(num_bins, self.fft_size);
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

impl AkiFxModule for FrequencyMagnetModule {
    fn name(&self) -> &'static str {
        "Frequency Magnet"
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

        // Save input for spectral processing
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        // Read parameters for the spectral callback
        let target_freq = self.params.frequency.value();
        // Map strength (0–100%) to C++ width (1–0): strength=0 → width=1 (identity),
        // strength=100 → width=0 (full collapse).
        let width = 1.0 - (self.params.strength.value() / 100.0);
        let width_bias_raw = self.params.width_bias.value();
        let use_legacy = self.params.use_legacy.value();

        // Process through spectral engine with frequency magnet callback.
        // Engine construction is guaranteed during initialize(); skip this
        // block rather than panicking in the host's audio callback if not.
        let sample_rate = self.sample_rate;
        let fft_size = self.fft_size;
        let scratch: &mut MagnetScratch = &mut self.scratch;
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |num_bins, _chan, _overlap, polar| {
                spectral_process_magnet(
                    polar,
                    num_bins,
                    fft_size,
                    sample_rate,
                    target_freq,
                    width,
                    width_bias_raw,
                    use_legacy,
                    scratch,
                );
            },
        );
    }
}

// ── Spectral callback ──────────────────────────────────────────────────────

/// Port of `FrequencyMagnetFFTProcessor::spectral_process`.
///
/// Remaps bin magnitudes toward a target frequency bin. The `polar` slice
/// contains `num_bins` elements (half the FFT size). A scratch buffer sized
/// to `fft_size` is used for accumulation to match the C++ out-of-range
/// indexing pattern (indices above `num_bins` in `out` are zeroed by the
/// engine's `pol2Car` step, so writes beyond `num_bins` are harmless).
fn spectral_process_magnet(
    polar: &mut [Polar],
    num_bins: usize,
    fft_size: usize,
    sample_rate: f32,
    target_freq: f32,
    width: f32,
    width_bias_raw: f32,
    use_legacy: bool,
    scratch: &mut MagnetScratch,
) {
    // Compute width bias exponent and lower limit (faithful to C++)
    let width_bias = width_bias_raw * 3.0;
    let width_limit = 1.0 - width_bias_raw.powf(0.125);

    // Target bin index (matches C++: freq / sampleRate * fftSize)
    let target_bin_f = (target_freq / sample_rate) * fft_size as f32;
    let target_bin = target_bin_f as usize;

    // If target bin exceeds available bins, pass through unchanged
    if target_bin > num_bins {
        return;
    }

    // Clamp and bias the width parameter
    let clamped_width = width.clamp(width_limit, 1.0);
    let biased_width = clamped_width.powf(width_bias);

    // Snapshot input magnitudes and phases. The C++ code has separate `in` and
    // `out` polar vectors, but the Rust engine callback provides a single
    // `&mut [Polar]` buffer, so we must read all inputs before writing outputs.
    // The scratch buffers are reused across frames (no allocation here).
    let in_mag = &mut scratch.in_mag;
    let in_phase = &mut scratch.in_phase;
    in_mag.clear();
    in_mag.resize(num_bins, 0.0);
    in_phase.clear();
    in_phase.resize(num_bins, 0.0);
    for (dst, src) in in_mag.iter_mut().zip(polar.iter()) {
        *dst = src.magnitude;
    }
    for (dst, src) in in_phase.iter_mut().zip(polar.iter()) {
        *dst = src.phase;
    }

    // Zero the output buffer (matches C++ `utilities::emptyPolar(out)`)
    for p in polar.iter_mut() {
        *p = Polar::new(0.0, 0.0);
    }

    // Accumulation buffer sized to fft_size to match C++ indexing pattern.
    // The C++ `temp` vector has `bins` elements, but the code indexes it with
    // values up to `fft_size - 1` via the `out.size()` clip, so we size to
    // `fft_size` for safety.
    let temp = &mut scratch.temp[..fft_size];
    temp.fill(0.0);

    // ── Below target bin ────────────────────────────────────────────────
    for i in 0..target_bin {
        let line = i as f32 / target_bin as f32;
        // width * 8 / 8 = width (faithful to C++ expression). The C++ keeps
        // the mapped index FRACTIONAL: utilities::interp_lin truncates the
        // index and blends temp[idx] with temp[idx + 1] by the remainder.
        // (The previous Rust code truncated before computing the blend
        // weight, so the fractional smear was dead.)
        let index_below = line.powf(biased_width) * target_bin as f32;

        let idx = (index_below as usize).min(fft_size - 1);
        temp[idx] += in_mag[i];

        // Phase at input index (faithful to C++: out[i].m_phase = in[i].m_phase)
        if i < num_bins {
            polar[i].phase = in_phase[i];
        }

        // out[idx].m_mag = interp_lin(temp[idx], temp[idx + 1], index_below).
        // C++ reads temp[idx + 1] one past the end when idx == bins - 1
        // (undefined behaviour); we clamp to the last valid slot instead.
        let next = (idx + 1).min(num_bins - 1);
        let t = index_below.fract();
        polar[idx].magnitude = temp[idx] + (temp[next] - temp[idx]) * t;
    }

    // ── Above target bin ────────────────────────────────────────────────
    let target_bin_safe = target_bin.max(1);
    let range = (num_bins - target_bin_safe) as f32;

    for i in target_bin_safe..num_bins {
        let norm_index = (i - target_bin_safe) as f32;
        let line = norm_index / range;

        let exponent = (1.0 - biased_width) * 7.0 + 1.0;
        let mut index_above = line.powf(exponent) * range;

        // Offset by target frequency (unless legacy mode)
        if !use_legacy {
            index_above += target_bin as f32;
        }

        index_above = index_above.clamp(1.0, (num_bins as f32) - 1.0);

        let idx = (index_above as usize).min(num_bins - 1);
        temp[idx] += in_mag[i];

        // Phase at input index (faithful to C++: out[i].m_phase = in[i].m_phase)
        polar[i].phase = in_phase[i];

        // out[idx].m_mag = interp_lin(temp[idx], temp[idx - 1], index_above):
        // the C++ blends toward the PREVIOUS bin by the fractional part.
        // (The previous Rust code used t = frac + 1, turning the blend into
        // an extrapolation that overshot and inverted the neighbour.)
        let prev = idx.saturating_sub(1);
        let t = index_above.fract();
        polar[idx].magnitude = temp[idx] + (temp[prev] - temp[idx]) * t;
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> FrequencyMagnetModule {
        let mut m = FrequencyMagnetModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with specific strength, frequency, and optional bias.
    fn make_module_with(strength_pct: f32, frequency: f32) -> FrequencyMagnetModule {
        make_module_with_bias(strength_pct, frequency, 0.01)
    }

    fn make_module_with_bias(strength_pct: f32, frequency: f32, bias: f32) -> FrequencyMagnetModule {
        let params = Arc::new(FrequencyMagnetParams {
            frequency: FloatParam::new("Frequency", frequency, FloatRange::Linear { min: 20.0, max: 2000.0 }),
            strength: FloatParam::new("Strength", strength_pct, FloatRange::Linear { min: 0.0, max: 100.0 }),
            width_bias: FloatParam::new("Width Bias", bias, FloatRange::Linear { min: 0.0, max: 1.0 }),
            use_legacy: BoolParam::new("Use Legacy Mode", false),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = FrequencyMagnetModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut FrequencyMagnetModule, input: &[f32]) -> Vec<f32> {
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

    /// Helper: compute the FFT magnitude spectrum of a signal.
    fn fft_magnitude(signal: &[f32], fft_size: usize) -> Vec<f32> {
        use rustfft::FftPlanner;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buf: Vec<Complex<f32>> = signal.iter()
            .take(fft_size)
            .map(|&s| Complex::new(s, 0.0))
            .collect();
        buf.resize(fft_size, Complex::default());
        let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        fft.process_with_scratch(&mut buf, &mut scratch);
        buf.iter()
            .take(fft_size / 2)
            .map(|c| c.norm())
            .collect()
    }

    /// Find the bin index with the maximum magnitude in an FFT spectrum.
    fn peak_bin(spectrum: &[f32]) -> usize {
        spectrum.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    // ── Test 1: strength=0 → identity (impulse delayed by latency) ──────

    #[test]
    fn strength_zero_identity() {
        let mut module = make_module_with(0.0, 800.0);
        let fft_size = DEFAULT_FFT_SIZE;
        let total = fft_size * 8;

        // Impulse at t=0
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;

        let output = process_signal(&mut module, &input);

        let latency = module.latency_samples() as usize;
        // True signal delay is fft_size; the reported value adds a
        // conservative hop (matching the C++ plugins).
        let latency = latency - module.fft_size / 4;

        // The output should contain a peak near the expected latency position
        let search_start = latency.saturating_sub(32);
        let search_end = (latency + 32).min(total);
        let peak = output[search_start..search_end]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(i, _)| i + search_start)
            .unwrap_or(0);

        let peak_error = (peak as isize - latency as isize).unsigned_abs();
        assert!(
            peak_error < 64,
            "strength=0: impulse peak at {peak}, expected near latency {latency}, error {peak_error}"
        );

        // Peak magnitude should be comparable to input (identity preserves energy)
        let peak_mag = output[peak];
        assert!(
            peak_mag.abs() > 0.01,
            "strength=0: output peak magnitude too small: {peak_mag}"
        );

        // Energy should be comparable (spectral processing with identity callback)
        let input_energy: f32 = input.iter().map(|s| s * s).sum();
        let output_energy: f32 = output.iter().map(|s| s * s).sum();
        let energy_ratio = output_energy / input_energy.max(1e-10);
        assert!(
            energy_ratio > 0.1 && energy_ratio < 10.0,
            "strength=0: energy ratio {energy_ratio} expected near 1.0"
        );
    }

    // ── Test 2: high strength concentrates energy near target ────────────

    #[test]
    fn high_strength_concentrates_energy() {
        // Create a tone at 440 Hz; set magnet to target 800 Hz with high strength
        // and moderate bias so the width limit allows strong concentration.
        let mut module = make_module_with_bias(95.0, 800.0, 0.5);
        let fft_size = DEFAULT_FFT_SIZE;
        let total = fft_size * 8;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let output = process_signal(&mut module, &input);

        // Compute FFT of input and output (after latency settling)
        let compare_start = fft_size * 2;
        let input_spectrum = fft_magnitude(&input[compare_start..], fft_size);
        let output_spectrum = fft_magnitude(&output[compare_start..], fft_size);

        let input_peak = peak_bin(&input_spectrum);
        let output_peak = peak_bin(&output_spectrum);

        // Convert peak bins to frequencies
        let input_freq = input_peak as f32 * SR / fft_size as f32;
        let output_freq = output_peak as f32 * SR / fft_size as f32;
        let target_freq = 800.0f32;

        // The output peak should be closer to the target than the input peak
        let input_dist = (input_freq - target_freq).abs();
        let output_dist = (output_freq - target_freq).abs();

        assert!(
            output_dist < input_dist,
            "high strength: input peak at {input_freq:.1} Hz (dist {input_dist:.1}), \
             output peak at {output_freq:.1} Hz (dist {output_dist:.1}); \
             output should be closer to target {target_freq:.1} Hz"
        );
    }

    // ── Test 3: silence → silence ───────────────────────────────────────

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

    // ── Test 4: no NaN over 10 seconds of noise ────────────────────────

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module_with(75.0, 500.0);
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

    // ── Test 5: reset() is deterministic ────────────────────────────────

    #[test]
    fn reset_deterministic() {
        let mut module = make_module_with(50.0, 440.0);

        // Process some audio to build up state
        let signal: Vec<f32> = (0..4096)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();
        let first_run = process_signal(&mut module, &signal);

        // Reset
        module.reset();

        // Process the same signal again — output should match first run
        let second_run = process_signal(&mut module, &signal);

        // After reset, the engine state is cleared so the second run
        // should produce the same output as a fresh run.
        let max_diff: f32 = first_run.iter()
            .zip(second_run.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);

        assert!(
            max_diff < 1e-6,
            "reset should restore deterministic state, max diff = {max_diff}"
        );
    }
}

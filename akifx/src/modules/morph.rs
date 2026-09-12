//! Morph — spectral frequency bin remapping ported from SpectralSuite's Morph plugin.
//!
//! Faithfully ports the Morph FFT processor as a frequency-domain remapping effect.
//! Each frequency bin's magnitude and phase are copied from an input position to an
//! output position determined by a morph curve. The morph curve is defined by control
//! points that are interpolated to generate the full FFT-size mapping.
//!
//! # Parameters (mirroring source)
//!
//! - **Mix** (`#[id = "mix"]`): Dry/wet mix (0–100 %), default 100 %.
//! - **Use PVOC** (`#[id = "use_pvoc"]`): Enable spectral processing, default true.
//! - **Num Overlaps** (`#[id = "num_overlaps"]`): STFT overlap count (1–8), default 4.
//! - **Control Points** (`#[id = "cp_0"]` through `#[id = "cp_15"]`): 16 normalized
//!   frequency mapping control points (0.0–1.0). Default values produce a diagonal
//!   (identity) mapping where each input bin maps to the same output position.
//!
//! # Spectral Domain Behavior
//!
//! The morph effect works by:
//! 1. Zeroing the output spectrum
//! 2. For each input bin `i`, copying `in[i]` to `out[morphPoints[i]]`
//!
//! The `morphPoints` array is generated from the control points by linear interpolation,
//! matching the C++ SplineHelper + MorphInteractor::controlPointsChanged logic.

use crate::modules::AkiFxModule;
use crate::stft::{SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default FFT size for the spectral engine.
const DEFAULT_FFT_SIZE: usize = 2048;

/// Number of control points defining the morph curve.
/// Mirrors the C++ SplineHelper resolution (128), but reduced to a manageable
/// parameter count. The full morph mapping is interpolated from these points.
const NUM_CONTROL_POINTS: usize = 16;

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Morph module.
///
/// Mirrors SpectralSuite's MorphPluginParameters parameter set.
/// The 16 control points define a normalized frequency mapping curve
/// (0.0 = lowest bin, 1.0 = highest bin). Default values produce a
/// diagonal (identity) mapping.
#[derive(Params)]
pub struct MorphParams {
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

    /// Control point 0: normalized frequency mapping (0.0–1.0).
    #[id = "cp_0"]
    pub cp_0: FloatParam,
    /// Control point 1.
    #[id = "cp_1"]
    pub cp_1: FloatParam,
    /// Control point 2.
    #[id = "cp_2"]
    pub cp_2: FloatParam,
    /// Control point 3.
    #[id = "cp_3"]
    pub cp_3: FloatParam,
    /// Control point 4.
    #[id = "cp_4"]
    pub cp_4: FloatParam,
    /// Control point 5.
    #[id = "cp_5"]
    pub cp_5: FloatParam,
    /// Control point 6.
    #[id = "cp_6"]
    pub cp_6: FloatParam,
    /// Control point 7.
    #[id = "cp_7"]
    pub cp_7: FloatParam,
    /// Control point 8.
    #[id = "cp_8"]
    pub cp_8: FloatParam,
    /// Control point 9.
    #[id = "cp_9"]
    pub cp_9: FloatParam,
    /// Control point 10.
    #[id = "cp_10"]
    pub cp_10: FloatParam,
    /// Control point 11.
    #[id = "cp_11"]
    pub cp_11: FloatParam,
    /// Control point 12.
    #[id = "cp_12"]
    pub cp_12: FloatParam,
    /// Control point 13.
    #[id = "cp_13"]
    pub cp_13: FloatParam,
    /// Control point 14.
    #[id = "cp_14"]
    pub cp_14: FloatParam,
    /// Control point 15.
    #[id = "cp_15"]
    pub cp_15: FloatParam,
}

/// Helper macro to create a control point FloatParam.
fn make_control_point(id_prefix: usize, default: f32) -> FloatParam {
    FloatParam::new(
        format!("CP {id_prefix}"),
        default,
        FloatRange::Linear {
            min: 0.0,
            max: 1.0,
        },
    )
}

impl MorphParams {
    /// Create default parameters matching the source plugin.
    ///
    /// Default control points produce a diagonal (identity) mapping where
    /// each input frequency bin maps to the same output position.
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
            // Diagonal (identity) mapping: cp[i] = i / (N-1)
            cp_0: make_control_point(0, 0.0),
            cp_1: make_control_point(1, 1.0 / 15.0),
            cp_2: make_control_point(2, 2.0 / 15.0),
            cp_3: make_control_point(3, 3.0 / 15.0),
            cp_4: make_control_point(4, 4.0 / 15.0),
            cp_5: make_control_point(5, 5.0 / 15.0),
            cp_6: make_control_point(6, 6.0 / 15.0),
            cp_7: make_control_point(7, 7.0 / 15.0),
            cp_8: make_control_point(8, 8.0 / 15.0),
            cp_9: make_control_point(9, 9.0 / 15.0),
            cp_10: make_control_point(10, 10.0 / 15.0),
            cp_11: make_control_point(11, 11.0 / 15.0),
            cp_12: make_control_point(12, 12.0 / 15.0),
            cp_13: make_control_point(13, 13.0 / 15.0),
            cp_14: make_control_point(14, 14.0 / 15.0),
            cp_15: make_control_point(15, 1.0),
        }
    }

    /// Read all control point values into a fixed-size array.
    fn control_point_values(&self) -> [f32; NUM_CONTROL_POINTS] {
        [
            self.cp_0.value(),
            self.cp_1.value(),
            self.cp_2.value(),
            self.cp_3.value(),
            self.cp_4.value(),
            self.cp_5.value(),
            self.cp_6.value(),
            self.cp_7.value(),
            self.cp_8.value(),
            self.cp_9.value(),
            self.cp_10.value(),
            self.cp_11.value(),
            self.cp_12.value(),
            self.cp_13.value(),
            self.cp_14.value(),
            self.cp_15.value(),
        ]
    }
}

impl Default for MorphParams {
    fn default() -> Self {
        Self::new()
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Morph — spectral frequency bin remapping with configurable morph curve.
///
/// Owns a [`SpectralEngine`] for stereo FFT/IFFT processing. The spectral
/// callback applies frequency bin remapping based on a morph curve defined
/// by control points, faithfully porting the C++ MorphFFTProcessor behavior.
pub struct MorphModule {
    params: Arc<MorphParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    engine: Option<SpectralEngine>,
    fft_size: usize,
    /// Dry signal buffers for mix blending.
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
    /// Cached overlap count for change detection.
    cached_overlaps: i32,
    /// Morph mapping: morph_points[i] = destination bin for input bin i.
    /// Matches C++ `Array<int>` in MorphInteractor.
    morph_points: Vec<i32>,
    /// Cached control point values for change detection.
    cached_cp: [f32; NUM_CONTROL_POINTS],
}

impl MorphModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<MorphParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            engine: None,
            fft_size: DEFAULT_FFT_SIZE,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
            cached_overlaps: -1,
            morph_points: Vec::new(),
            cached_cp: [0.0; NUM_CONTROL_POINTS],
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(MorphParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Rebuild the spectral engine with current parameter values.
    fn build_engine(&mut self) {
        let overlap_count = self.params.num_overlaps.value() as usize;
        let config = SpectralConfig {
            fft_size: self.fft_size,
            overlap_count,
            window: WindowType::Hann,
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

    /// Rebuild the morph points array from current control point values.
    ///
    /// Faithfully ports MorphInteractor::controlPointsChanged:
    /// - Control points are normalized float values (0.0–1.0)
    /// - Each is scaled to FFT half-size indices: `value * fft_half_size`
    /// - Linear interpolation expands control points to full FFT size
    fn rebuild_morph_points(&mut self, half_size: usize) {
        let cp = self.params.control_point_values();
        self.cached_cp = cp;

        if half_size == 0 {
            self.morph_points.clear();
            return;
        }

        // The C++ code uses SplineHelper to generate128 values from the GUI points,
        // then resamples to FFT half-size. We skip the spline (no GUI) and
        // interpolate directly from our16 control points to half_size bins.
        self.morph_points = interpolate_control_points(&cp, half_size);
    }

    /// Check if control points have changed and rebuild morph points if needed.
    fn update_morph_points_if_needed(&mut self) {
        let half_size = self.fft_size / 2;
        let cp = self.params.control_point_values();
        if cp != self.cached_cp || self.morph_points.len() != half_size {
            self.rebuild_morph_points(half_size);
        }
    }
}

impl AkiFxModule for MorphModule {
    fn name(&self) -> &'static str {
        "Morph"
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
        self.rebuild_morph_points(self.fft_size / 2);
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

        // Update morph points if control points changed
        self.update_morph_points_if_needed();

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

        // Process through spectral engine with morph callback.
        // Faithfully ports MorphFFTProcessor::spectral_process:
        //   for each input bin i: out[morphPoints[i]] = in[i]
        let morph_points = &self.morph_points;
        // Engine construction is guaranteed during initialize(); skip this
        // block rather than panicking in the host's audio callback if not.
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |num_bins, polar| {
                let bin_count = num_bins.min(polar.len());

                // Build temporary copy of input bins
                let input_bins: Vec<crate::stft::Polar> =
                    polar[..bin_count].to_vec();

                // Zero output (matching C++ spectral_process lines 9-12)
                for bin in polar[..bin_count].iter_mut() {
                    *bin = crate::stft::Polar::new(0.0, 0.0);
                }

                // Remap bins (matching C++ lines 20-23)
                let mp_len = morph_points.len();
                for i in 0..bin_count {
                    if i < mp_len {
                        let dest = morph_points[i] as usize;
                        if dest < bin_count {
                            polar[dest] = input_bins[i];
                        }
                    }
                }
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

// ── Interpolation ──────────────────────────────────────────────────────────

/// Linearly interpolate control points to generate a morph mapping of `target_len` bins.
///
/// Each control point represents a normalized frequency position (0.0–1.0).
/// The output is a vector of integer bin indices suitable for frequency remapping.
///
/// This mirrors the C++ MorphInteractor::controlPointsChanged logic:
/// - Control points are spaced evenly across the input range
/// - Linear interpolation expands to target_len values
/// - Values are scaled to bin indices (clamped to 0..target_len-1)
fn interpolate_control_points(control_points: &[f32; NUM_CONTROL_POINTS], target_len: usize) -> Vec<i32> {
    if target_len == 0 {
        return Vec::new();
    }

    let n = control_points.len();
    if n == 0 {
        return (0..target_len as i32).collect();
    }

    let mut result = Vec::with_capacity(target_len);
    let max_bin = (target_len as f32) - 1.0;

    for i in 0..target_len {
        // Map output index to control point position
        let cp_pos = (i as f32) / max_bin * ((n - 1) as f32);
        let idx_a = (cp_pos as usize).min(n - 1);
        let idx_b = (idx_a + 1).min(n - 1);
        let frac = cp_pos - (idx_a as f32);

        // Linear interpolation between adjacent control points
        let value = control_points[idx_a] + (control_points[idx_b] - control_points[idx_a]) * frac;

        // Clamp and scale to bin index (matching C++ `value * fftSize`)
        let bin_index = (value * max_bin).round() as i32;
        let bin_index = bin_index.max(0).min(max_bin as i32);
        result.push(bin_index);
    }

    result
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> MorphModule {
        let mut m = MorphModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create a module with specific control points.
    fn make_module_with_cp(cp: [f32; NUM_CONTROL_POINTS]) -> MorphModule {
        let params = Arc::new(MorphParams {
            mix: FloatParam::new(
                "Mix",
                100.0,
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
            cp_0: make_control_point(0, cp[0]),
            cp_1: make_control_point(1, cp[1]),
            cp_2: make_control_point(2, cp[2]),
            cp_3: make_control_point(3, cp[3]),
            cp_4: make_control_point(4, cp[4]),
            cp_5: make_control_point(5, cp[5]),
            cp_6: make_control_point(6, cp[6]),
            cp_7: make_control_point(7, cp[7]),
            cp_8: make_control_point(8, cp[8]),
            cp_9: make_control_point(9, cp[9]),
            cp_10: make_control_point(10, cp[10]),
            cp_11: make_control_point(11, cp[11]),
            cp_12: make_control_point(12, cp[12]),
            cp_13: make_control_point(13, cp[13]),
            cp_14: make_control_point(14, cp[14]),
            cp_15: make_control_point(15, cp[15]),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = MorphModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut MorphModule, input: &[f32]) -> Vec<f32> {
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

    // ── Test 1: Default identity morph → impulse delay within 1e-3 ─────
    //
    // With default diagonal control points, the morph mapping is identity
    // (bin i → bin i). The output should be a delayed copy of the input
    // with high correlation (>0.99).

    #[test]
    fn identity_morph_impulse_delay() {
        let mut module = make_module();
        let fft_size = 2048;
        let total = fft_size * 8;

        // Impulse at the start
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;

        let output = process_signal(&mut module, &input);

        // The output should have a peak near the impulse position (delayed by latency)
        let latency = module.latency_samples() as usize;

        // Find the peak in the output
        let peak_pos = output
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);

        // Peak should be near the impulse + latency
        let delay_error = (peak_pos as isize - latency as isize).unsigned_abs();
        assert!(
            delay_error < 32,
            "identity morph: impulse peak at {peak_pos}, expected near {latency}, \
             error {delay_error} samples (must be < 32)"
        );

        // Correlation should be high after accounting for delay
        let compare_start = latency;
        let compare_end = total - fft_size;
        if compare_end > compare_start {
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
                correlation > 0.99,
                "identity morph: correlation {correlation} (expected > 0.99)"
            );
        }
    }

    // ── Test 2: Engaged curve remaps energy measurably ─────────────────
    //
    // With a non-identity morph curve (e.g., reversed), the spectrum centroid
    // should shift. A reversed mapping (low bins → high bins, high → low)
    // should shift energy from high frequencies to low frequencies.

    #[test]
    fn engaged_curve_remaps_energy() {
        // Reversed morph curve: cp[i] = 1.0 - i/15.0
        // This maps low bins to high bins and vice versa
        let reversed_cp: [f32; NUM_CONTROL_POINTS] = [
            1.0, 14.0 / 15.0, 13.0 / 15.0, 12.0 / 15.0, 11.0 / 15.0, 10.0 / 15.0,
            9.0 / 15.0, 8.0 / 15.0, 7.0 / 15.0, 6.0 / 15.0, 5.0 / 15.0, 4.0 / 15.0,
            3.0 / 15.0, 2.0 / 15.0, 1.0 / 15.0, 0.0,
        ];

        let mut module_rev = make_module_with_cp(reversed_cp);
        let mut module_id = make_module();

        let fft_size = 2048;
        let total = fft_size * 8;

        // High-frequency signal: 8800 Hz sine (near Nyquist)
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * PI * 8800.0 * t).sin()
            })
            .collect();

        let output_rev = process_signal(&mut module_rev, &input);
        let output_id = process_signal(&mut module_id, &input);

        // Compute spectral centroid for both outputs (after settling)
        let settle = fft_size * 2;
        let compare_end = total - fft_size;

        let centroid_rev = spectral_centroid(&output_rev[settle..compare_end], fft_size);
        let centroid_id = spectral_centroid(&output_id[settle..compare_end], fft_size);

        // The reversed mapping should shift the centroid measurably
        // (high freq energy moved to low freq bins → lower centroid)
        assert!(
            (centroid_rev - centroid_id).abs() > 1.0,
            "reversed morph should shift centroid measurably: \
             rev={centroid_rev:.2}, id={centroid_id:.2}, diff={:.2}",
            (centroid_rev - centroid_id).abs()
        );
    }

    /// Compute average spectral centroid of a signal.
    fn spectral_centroid(signal: &[f32], fft_size: usize) -> f32 {
        use rustfft::{FftPlanner, num_complex::Complex};

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let half = fft_size / 2;

        let mut total_weighted = 0.0f32;
        let mut total_mag = 0.0f32;

        let hop = fft_size / 4;
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * PI * i as f32 / fft_size as f32).cos())
            })
            .collect();

        let mut pos = 0;
        while pos + fft_size <= signal.len() {
            let mut buf: Vec<Complex<f32>> = signal[pos..pos + fft_size]
                .iter()
                .zip(window.iter())
                .map(|(s, w)| Complex::new(s * w, 0.0))
                .collect();

            fft.process(&mut buf);

            for k in 1..half {
                let mag = buf[k].norm();
                total_weighted += mag * k as f32;
                total_mag += mag;
            }
            pos += hop;
        }

        if total_mag > 1e-10 {
            total_weighted / total_mag
        } else {
            0.0
        }
    }

    // ── Test 3: Silence remains silent ─────────────────────────────────

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

    // ── Test 4: Stability — 10 seconds of noise, no NaN ───────────────

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize; // 10 seconds

        // Deterministic pseudo-noise (sine sum)
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * PI * 100.0 * t).sin()
                    + (2.0 * PI * 317.0 * t).sin()
                    + (2.0 * PI * 793.0 * t).sin()
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

    // ── Test 5: Reset restores state ───────────────────────────────────

    #[test]
    fn reset_restores_state() {
        let mut module = make_module();

        // Process some audio to build up state
        let signal: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / SR).sin())
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

    // ── Test 6: Latency is correct ─────────────────────────────────────

    #[test]
    fn latency_equals_fft_size() {
        let module = make_module();
        assert_eq!(
            module.latency_samples(),
            DEFAULT_FFT_SIZE as u64,
            "latency should equal FFT size"
        );
    }

    // ── Test 7: PVOC disabled → passthrough ────────────────────────────

    #[test]
    fn pvoc_disabled_passthrough() {
        let params = Arc::new(MorphParams {
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
            cp_0: make_control_point(0, 0.0),
            cp_1: make_control_point(1, 1.0 / 15.0),
            cp_2: make_control_point(2, 2.0 / 15.0),
            cp_3: make_control_point(3, 3.0 / 15.0),
            cp_4: make_control_point(4, 4.0 / 15.0),
            cp_5: make_control_point(5, 5.0 / 15.0),
            cp_6: make_control_point(6, 6.0 / 15.0),
            cp_7: make_control_point(7, 7.0 / 15.0),
            cp_8: make_control_point(8, 8.0 / 15.0),
            cp_9: make_control_point(9, 9.0 / 15.0),
            cp_10: make_control_point(10, 10.0 / 15.0),
            cp_11: make_control_point(11, 11.0 / 15.0),
            cp_12: make_control_point(12, 12.0 / 15.0),
            cp_13: make_control_point(13, 13.0 / 15.0),
            cp_14: make_control_point(14, 14.0 / 15.0),
            cp_15: make_control_point(15, 1.0),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = MorphModule::new(params, bypass);
        module.initialize(SR, BLOCK);

        let total = 1024;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / SR).sin())
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

    // ── Test 8: Mix parameter blends dry and wet ───────────────────────

    #[test]
    fn mix_blends_dry_and_wet() {
        let params = Arc::new(MorphParams {
            mix: FloatParam::new(
                "Mix",
                50.0,
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
            cp_0: make_control_point(0, 0.0),
            cp_1: make_control_point(1, 1.0 / 15.0),
            cp_2: make_control_point(2, 2.0 / 15.0),
            cp_3: make_control_point(3, 3.0 / 15.0),
            cp_4: make_control_point(4, 4.0 / 15.0),
            cp_5: make_control_point(5, 5.0 / 15.0),
            cp_6: make_control_point(6, 6.0 / 15.0),
            cp_7: make_control_point(7, 7.0 / 15.0),
            cp_8: make_control_point(8, 8.0 / 15.0),
            cp_9: make_control_point(9, 9.0 / 15.0),
            cp_10: make_control_point(10, 10.0 / 15.0),
            cp_11: make_control_point(11, 11.0 / 15.0),
            cp_12: make_control_point(12, 12.0 / 15.0),
            cp_13: make_control_point(13, 13.0 / 15.0),
            cp_14: make_control_point(14, 14.0 / 15.0),
            cp_15: make_control_point(15, 1.0),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = MorphModule::new(params, bypass);
        module.initialize(SR, BLOCK);

        let total = 4096;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let output = process_signal(&mut module, &input);

        // Output should be non-zero and bounded
        let output_peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            output_peak < 5.0,
            "50% mix output peak {output_peak} seems too large"
        );
        assert!(output_peak > 0.0, "50% mix output must not be silent");
    }

    // ── Test 9: Varying block sizes don't panic ────────────────────────

    #[test]
    fn varying_block_sizes_no_panic() {
        let mut module = make_module();
        let total: usize = 4096;

        for block_size in [64, 128, 256, 512, 1024, 2048] {
            let input: Vec<f32> = (0..total)
                .map(|i| (2.0 * PI * 440.0 * i as f32 / SR).sin())
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

    // ── Test 10: Non-identity morph produces different output ───────────

    #[test]
    fn non_identity_cp_produces_different_output() {
        // Gentle compression: cp[i] = i/15 * 0.8 (compressed to 80% of range)
        // Maps each bin to a lower position but spreads energy across spectrum
        let compressed_cp: [f32; NUM_CONTROL_POINTS] =
            std::array::from_fn(|i| (i as f32 / 15.0) * 0.8);
        let mut module_cp = make_module_with_cp(compressed_cp);
        let mut module_id = make_module();

        let total = 8192;
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * PI * 440.0 * t).sin() + 0.5 * (2.0 * PI * 880.0 * t).sin()
            })
            .collect();

        let output_cp = process_signal(&mut module_cp, &input);
        let output_id = process_signal(&mut module_id, &input);

        // Both should be finite
        assert!(
            output_cp.iter().all(|s| s.is_finite()),
            "compressed CP output contains non-finite values"
        );
        assert!(
            output_id.iter().all(|s| s.is_finite()),
            "identity output contains non-finite values"
        );

        // The outputs should be different (different spectral content)
        let settle = 2048;
        let compare_end = total - 2048;
        let mut diff = 0.0f32;
        for i in settle..compare_end {
            diff += (output_cp[i] - output_id[i]).abs();
        }
        let avg_diff = diff / (compare_end - settle) as f32;
        assert!(
            avg_diff > 0.001,
            "non-identity CP should produce different output than identity, avg_diff = {avg_diff}"
        );
    }

    // ── Test 11: Control point interpolation correctness ───────────────

    #[test]
    fn interpolate_control_points_identity() {
        // Default diagonal CPs should produce identity mapping
        let default_cp = MorphParams::new().control_point_values();
        let half_size = DEFAULT_FFT_SIZE / 2;
        let morph = interpolate_control_points(&default_cp, half_size);

        assert_eq!(morph.len(), half_size);

        // Each bin should map to approximately itself
        for i in 0..half_size {
            let error = (morph[i] - i as i32).unsigned_abs();
            assert!(
                error <= 1,
                "identity interpolation: bin {i} maps to {}, expected {i}",
                morph[i]
            );
        }
    }

    #[test]
    fn interpolate_control_points_reversed() {
        // Reversed CPs should produce reverse mapping
        let reversed_cp: [f32; NUM_CONTROL_POINTS] = [
            1.0, 14.0 / 15.0, 13.0 / 15.0, 12.0 / 15.0, 11.0 / 15.0, 10.0 / 15.0,
            9.0 / 15.0, 8.0 / 15.0, 7.0 / 15.0, 6.0 / 15.0, 5.0 / 15.0, 4.0 / 15.0,
            3.0 / 15.0, 2.0 / 15.0, 1.0 / 15.0, 0.0,
        ];
        let half_size = 1024;
        let morph = interpolate_control_points(&reversed_cp, half_size);

        assert_eq!(morph.len(), half_size);

        // First bin should map near the last bin, last near the first
        let last_bin = (half_size - 1) as i32;
        assert!(
            (morph[0] - last_bin).unsigned_abs() <= 2,
            "reversed: first bin maps to {}, expected ~{last_bin}",
            morph[0]
        );
        assert!(
            (morph[half_size - 1]).unsigned_abs() <= 2,
            "reversed: last bin maps to {}, expected ~0",
            morph[half_size - 1]
        );
    }

    // ── Test 12: Repeated process calls are deterministic ──────────────

    #[test]
    fn repeated_process_deterministic() {
        let mut m1 = make_module();
        let mut m2 = make_module();

        let total = 4096;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let out1 = process_signal(&mut m1, &input);
        let out2 = process_signal(&mut m2, &input);

        // Same module, same input → same output
        for (i, (a, b)) in out1.iter().zip(out2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "determinism: sample {i} differs: {a} vs {b}"
            );
        }
    }
}

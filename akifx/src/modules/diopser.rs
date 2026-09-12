//! Diopser — phase-rotation filter using cascaded all-pass biquads.
//!
//! Port of nih-plug's Diopser. Cascades up to 512 first-order all-pass
//! biquad sections to rotate phase around a center frequency. The spread
//! parameter offsets each stage's frequency, broadening the rotation band.
//!
//! # Parameters
//!
//! - **Filter Stages** (`#[id = "stages"]`): Number of all-pass sections (0..512).
//! - **Frequency** (`#[id = "cutoff"]`): Center frequency in Hz (5..20 000).
//! - **Resonance** (`#[id = "res"]`): Q factor (0.01..30.0).
//! - **Spread** (`#[id = "spread"]`): Octave offset per stage (−5..5).
//! - **Spread Style** (`#[id = "spstyl"]`): Octaves or Linear distribution.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::f32::consts;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of all-pass filter stages.
pub const MAX_NUM_FILTERS: usize = 512;

/// Minimum allowed frequency (must never reach 0).
const MIN_FREQUENCY: f32 = 5.0;

// ── Spread style ───────────────────────────────────────────────────────────

/// How the frequency spread between stages is distributed.
#[derive(Enum, Debug, Clone, Copy, PartialEq)]
pub enum SpreadStyle {
    /// Exponential spread in octaves — most musical.
    #[id = "octaves"]
    Octaves,
    /// Linear frequency spread — useful for sound design.
    #[id = "linear"]
    Linear,
}

// ── Biquad (ported from nih-plug filter.rs) ────────────────────────────────

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
    /// Identity (passthrough) coefficients.
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Compute all-pass filter coefficients.
    ///
    /// Based on the Audio EQ Cookbook formula. At the given `frequency`, the
    /// filter shifts phase by approximately −90° while preserving magnitude.
    fn allpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        debug_assert!(sample_rate > 0.0);
        debug_assert!(frequency > 0.0);
        debug_assert!(frequency < sample_rate / 2.0);
        debug_assert!(q > 0.0);

        let omega0 = consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - alpha) / a0,
            b1: (-2.0 * cos_omega0) / a0,
            b2: (1.0 + alpha) / a0,
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
    /// Process a single sample through the all-pass filter.
    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
        let result = self.coefficients.b0 * sample + self.s1;
        self.s1 = self.coefficients.b1 * sample - self.coefficients.a1 * result + self.s2;
        self.s2 = self.coefficients.b2 * sample - self.coefficients.a2 * result;
        result
    }

    /// Reset filter state to zero.
    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Diopser module.
#[derive(Params)]
pub struct DiopserParams {
    /// Number of all-pass filter stages (0 = passthrough, up to 512).
    #[id = "stages"]
    pub filter_stages: IntParam,

    /// Center frequency in Hz. Filters are spread around this value.
    #[id = "cutoff"]
    pub filter_frequency: FloatParam,

    /// Q factor for each all-pass stage.
    #[id = "res"]
    pub filter_resonance: FloatParam,

    /// Frequency spread between stages in octaves.
    #[id = "spread"]
    pub filter_spread_octaves: FloatParam,

    /// How the spread is distributed (Octaves or Linear).
    #[id = "spstyl"]
    pub filter_spread_style: EnumParam<SpreadStyle>,
}

impl DiopserParams {
    /// Create default parameters matching the source plugin.
    pub fn new() -> Self {
        Self {
            filter_stages: IntParam::new(
                "Filter Stages",
                0,
                IntRange::Linear {
                    min: 0,
                    max: MAX_NUM_FILTERS as i32,
                },
            ),
            filter_frequency: FloatParam::new(
                "Filter Frequency",
                200.0,
                FloatRange::Skewed {
                    min: 5.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(formatters::v2s_f32_hz_then_khz_with_note_name(0, true))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            filter_resonance: FloatParam::new(
                "Filter Resonance",
                0.5,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 30.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(100.0))
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            filter_spread_octaves: FloatParam::new(
                "Filter Spread",
                0.0,
                FloatRange::SymmetricalSkewed {
                    min: -5.0,
                    max: 5.0,
                    factor: FloatRange::skew_factor(-1.0),
                    center: 0.0,
                },
            )
            .with_unit(" octaves")
            .with_step_size(0.01)
            .with_smoother(SmoothingStyle::Linear(100.0)),
            filter_spread_style: EnumParam::new("Filter Spread Style", SpreadStyle::Octaves),
        }
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Diopser — cascaded all-pass phase rotation filter.
pub struct DiopserModule {
    params: Arc<DiopserParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,

    /// Per-channel biquad chains: `[left_stages, right_stages]`.
    /// Each channel holds up to `MAX_NUM_FILTERS` stages.
    filters: [Vec<Biquad>; 2],

    /// Whether coefficients need recomputation on next process() call.
    should_update_filters: bool,

    /// Cached parameter values for change detection.
    cached_stages: i32,
    cached_frequency: f32,
    cached_resonance: f32,
    cached_spread: f32,
    cached_spread_style: SpreadStyle,
}

impl DiopserModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<DiopserParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            filters: [
                vec![Biquad::default(); MAX_NUM_FILTERS],
                vec![Biquad::default(); MAX_NUM_FILTERS],
            ],
            should_update_filters: true,
            cached_stages: -1,
            cached_frequency: 0.0,
            cached_resonance: 0.0,
            cached_spread: f32::NAN,
            cached_spread_style: SpreadStyle::Octaves,
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(DiopserParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Return a reference to the current filter coefficients for spectrum analysis.
    ///
    /// Returns `(num_active_stages, frequency, resonance, spread_octaves)`.
    /// The GUI/analyzer can use this to draw the phase response.
    pub fn spectrum_data(&self) -> (usize, f32, f32, f32) {
        let stages = self.params.filter_stages.value() as usize;
        let freq = self.params.filter_frequency.value();
        let res = self.params.filter_resonance.value();
        let spread = self.params.filter_spread_octaves.value();
        (stages.min(MAX_NUM_FILTERS), freq, res, spread)
    }

    /// Recompute all-pass filter coefficients for both channels.
    ///
    /// Faithfully ports `Diopser::update_filters()` from the source, including
    /// the spread-style distribution math. Called lazily when parameters change.
    fn update_filters(&mut self) {
        self.update_filters_impl(
            self.params.filter_frequency.value(),
            self.params.filter_resonance.value(),
            self.params.filter_spread_octaves.value(),
        );
    }

    /// Update coefficients from the smoothers' next step, for per-sample
    /// gliding while they are moving (upstream steps these every sample at
    /// the default automation precision).
    fn update_filters_smoothed(&mut self) {
        self.update_filters_impl(
            self.params.filter_frequency.smoothed.next_step(1),
            self.params.filter_resonance.smoothed.next_step(1),
            self.params.filter_spread_octaves.smoothed.next_step(1),
        );
    }

    fn update_filters_impl(&mut self, raw_frequency: f32, raw_resonance: f32, raw_spread: f32) {
        let sample_rate = self.sample_rate;
        let frequency = raw_frequency.max(MIN_FREQUENCY);
        let resonance = raw_resonance.max(0.01);
        let spread_octaves = raw_spread;
        let spread_style = self.params.filter_spread_style.value();
        let num_stages = self.params.filter_stages.value() as usize;

        let max_frequency = sample_rate / 2.05;

        // Linear spread offset — clamped so range never dips below 0
        let max_octave_spread = if spread_octaves >= 0.0 {
            frequency - (frequency * 2.0f32.powf(-spread_octaves))
        } else {
            (frequency * 2.0f32.powf(spread_octaves)) - frequency
        };

        for filter_idx in 0..num_stages {
            // Normalized position in [-1, 1]
            let filter_proportion =
                (filter_idx as f32 / num_stages as f32) * 2.0 - 1.0;

            let stage_frequency = match spread_style {
                SpreadStyle::Octaves => {
                    frequency * 2.0f32.powf(spread_octaves * filter_proportion)
                }
                SpreadStyle::Linear => frequency + (max_octave_spread * filter_proportion),
            }
            .clamp(MIN_FREQUENCY, max_frequency);

            let coefficients =
                BiquadCoefficients::allpass(sample_rate, stage_frequency, resonance);

            for channel in &mut self.filters {
                channel[filter_idx].coefficients = coefficients;
            }
        }

        // Update cache
        self.cached_stages = self.params.filter_stages.value();
        self.cached_frequency = frequency;
        self.cached_resonance = resonance;
        self.cached_spread = spread_octaves;
        self.cached_spread_style = spread_style;
        self.should_update_filters = false;
    }

    /// Check if any parameter changed since last coefficient update.
    fn params_changed(&self) -> bool {
        self.should_update_filters
            || self.params.filter_stages.value() != self.cached_stages
            || self.params.filter_frequency.value() != self.cached_frequency
            || self.params.filter_resonance.value() != self.cached_resonance
            || self.params.filter_spread_octaves.value() != self.cached_spread
            || self.params.filter_spread_style.value() != self.cached_spread_style
    }
}

impl AkiFxModule for DiopserModule {
    fn name(&self) -> &'static str {
        "Diopser"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.should_update_filters = true;
        // Seed the smoothers so non-host contexts don't glide from zero.
        self.params
            .filter_frequency
            .smoothed
            .reset(self.params.filter_frequency.value());
        self.params
            .filter_resonance
            .smoothed
            .reset(self.params.filter_resonance.value());
        self.params
            .filter_spread_octaves
            .smoothed
            .reset(self.params.filter_spread_octaves.value());
    }

    fn reset(&mut self) {
        for channel in &mut self.filters {
            for filter in channel.iter_mut() {
                filter.reset();
            }
        }
        self.should_update_filters = true;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Recompute coefficients if parameters changed
        if self.params_changed() {
            self.update_filters();
        }

        let num_stages = (self.params.filter_stages.value() as usize).min(MAX_NUM_FILTERS);

        // Process each sample through the cascade. While a parameter's
        // smoother is moving, coefficients are rebuilt every sample so the
        // filter glides instead of jumping (upstream behaviour; the
        // previously-declared smoothers were never stepped at all).
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.params.filter_frequency.smoothed.is_smoothing()
                || self.params.filter_resonance.smoothed.is_smoothing()
                || self.params.filter_spread_octaves.smoothed.is_smoothing()
            {
                self.update_filters_smoothed();
            }

            // Left channel
            let mut sample_l = *l;
            for filter in self.filters[0][..num_stages].iter_mut() {
                sample_l = filter.process(sample_l);
            }
            *l = sample_l;

            // Right channel
            let mut sample_r = *r;
            for filter in self.filters[1][..num_stages].iter_mut() {
                sample_r = filter.process(sample_r);
            }
            *r = sample_r;
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Helper: create a test module with specific parameter values.
    fn make_module(stages: i32, frequency: f32, resonance: f32) -> DiopserModule {
        make_module_full(stages, frequency, resonance, 0.0, SpreadStyle::Octaves)
    }

    /// Helper: create a test module with all parameter values specified.
    fn make_module_full(
        stages: i32,
        frequency: f32,
        resonance: f32,
        spread_octaves: f32,
        spread_style: SpreadStyle,
    ) -> DiopserModule {
        let params = Arc::new(DiopserParams {
            filter_stages: IntParam::new(
                "Filter Stages",
                stages,
                IntRange::Linear {
                    min: 0,
                    max: MAX_NUM_FILTERS as i32,
                },
            ),
            filter_frequency: FloatParam::new(
                "Filter Frequency",
                frequency,
                FloatRange::Skewed {
                    min: 5.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            ),
            filter_resonance: FloatParam::new(
                "Filter Resonance",
                resonance,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 30.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            ),
            filter_spread_octaves: FloatParam::new(
                "Filter Spread",
                spread_octaves,
                FloatRange::SymmetricalSkewed {
                    min: -5.0,
                    max: 5.0,
                    factor: FloatRange::skew_factor(-1.0),
                    center: 0.0,
                },
            )
            .with_step_size(0.01),
            filter_spread_style: EnumParam::new("Filter Spread Style", spread_style),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = DiopserModule::new(params, bypass);
        module.initialize(48000.0, 512);
        module
    }

    /// filter_count=0 must produce bit-identical passthrough.
    #[test]
    fn passthrough_when_zero_stages() {
        let mut module = make_module(0, 1000.0, 0.5);
        let original_l: Vec<f32> = (0..256).map(|i| (i as f32 * 0.01).sin()).collect();
        let original_r: Vec<f32> = (0..256).map(|i| (i as f32 * 0.02).cos()).collect();
        let mut left = original_l.clone();
        let mut right = original_r.clone();

        module.process(&mut left, &mut right);

        for (l, ol) in left.iter().zip(original_l.iter()) {
            assert_eq!(l, ol, "left channel must be bit-identical at 0 stages");
        }
        for (r, or_) in right.iter().zip(original_r.iter()) {
            assert_eq!(r, or_, "right channel must be bit-identical at 0 stages");
        }
    }

    /// Single all-pass at frequency f: magnitude ~unity, phase ~−90° at f.
    ///
    /// We generate a 1 kHz sine, pass it through one all-pass centered at 1 kHz,
    /// and verify via correlation that (a) magnitude ≈ 1.0 and (b) phase ≈ −90°.
    #[test]
    fn allpass_magnitude_and_phase_at_center_frequency() {
        let sample_rate: f32 = 48000.0;
        let center_freq: f32 = 1000.0;
        let test_freq: f32 = 1000.0;
        let num_samples = 4096;
        let skip = 512; // skip transient

        let mut module = make_module(1, center_freq, 0.5);

        // Generate test sine
        let signal: Vec<f32> = (0..num_samples)
            .map(|i| (2.0 * consts::PI * test_freq * i as f32 / sample_rate).sin())
            .collect();
        let mut left = signal.clone();
        let mut right = signal.clone();

        module.process(&mut left, &mut right);

        // Measure magnitude: ratio of output RMS to input RMS (after transient)
        let input_rms: f32 = (signal[skip..]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / (num_samples - skip) as f32)
            .sqrt();
        let output_rms: f32 = (left[skip..]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / (num_samples - skip) as f32)
            .sqrt();
        let magnitude_ratio = output_rms / input_rms;
        assert!(
            (magnitude_ratio - 1.0).abs() < 0.01,
            "all-pass magnitude should be ~unity, got ratio = {magnitude_ratio}"
        );

        // Measure phase via correlation
        // Correlate output with sin(ωt) and cos(ωt) at the test frequency
        let mut corr_sin = 0.0_f32;
        let mut corr_cos = 0.0_f32;
        for i in skip..num_samples {
            let phase = 2.0 * consts::PI * test_freq * i as f32 / sample_rate;
            corr_sin += left[i] * phase.sin();
            corr_cos += left[i] * phase.cos();
        }
        let n = (num_samples - skip) as f32;
        corr_sin /= n;
        corr_cos /= n;

        // A second-order all-pass biquad shifts phase by −π (−180°) at center freq
        // corr_sin = A²/2 * cos(φ), corr_cos = A²/2 * sin(φ)
        // where A ≈ 1.0 and φ ≈ −π
        // So corr_sin ≈ −A²/2, corr_cos ≈ 0
        let phase = corr_cos.atan2(corr_sin);
        assert!(
            (phase - (-consts::PI)).abs() < 0.15,
            "phase shift at center should be ~−180° (−π), got {phase} rad ({}°)",
            phase * 180.0 / consts::PI
        );
    }

    /// Impulse response must decay: max abs < 1e-6 after 2 seconds at 48 kHz.
    #[test]
    fn impulse_response_decays() {
        let sample_rate: f32 = 48000.0;
        let num_samples = (2.0 * sample_rate) as usize; // 2 seconds
        let mut module = make_module(64, 1000.0, 0.5);

        // Inject single impulse
        let mut left = vec![0.0_f32; num_samples];
        let mut right = vec![0.0_f32; num_samples];
        left[0] = 1.0;
        right[0] = 1.0;

        module.process(&mut left, &mut right);

        // Check that the tail has decayed
        let tail_start = num_samples - 4800; // last 100ms
        let max_abs_tail: f32 = left[tail_start..]
            .iter()
            .chain(right[tail_start..].iter())
            .map(|s| s.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_abs_tail < 1e-6,
            "impulse response tail max abs should be < 1e-6, got {max_abs_tail}"
        );
    }

    /// reset() must restore identical impulse response.
    #[test]
    fn reset_restores_impulse_response() {
        let _sample_rate: f32 = 48000.0;
        let num_samples = 2048;
        let mut module = make_module(32, 2000.0, 0.75);

        // First impulse response
        let mut left1 = vec![0.0_f32; num_samples];
        let mut right1 = vec![0.0_f32; num_samples];
        left1[0] = 1.0;
        right1[0] = 1.0;
        module.process(&mut left1, &mut right1);
        let ir1_l = left1.clone();
        let ir1_r = right1.clone();

        // Process some noise to change state
        let mut noise_l: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut noise_r = noise_l.clone();
        module.process(&mut noise_l, &mut noise_r);

        // Reset and re-inject impulse
        module.reset();
        let mut left2 = vec![0.0_f32; num_samples];
        let mut right2 = vec![0.0_f32; num_samples];
        left2[0] = 1.0;
        right2[0] = 1.0;
        module.process(&mut left2, &mut right2);

        // Must match first impulse response exactly
        for (a, b) in ir1_l.iter().zip(left2.iter()) {
            assert!(
                (a - b).abs() < 1e-10,
                "reset() must restore identical IR (left): {a} vs {b}"
            );
        }
        for (a, b) in ir1_r.iter().zip(right2.iter()) {
            assert!(
                (a - b).abs() < 1e-10,
                "reset() must restore identical IR (right): {a} vs {b}"
            );
        }
    }

    /// Verify that spectrum_data() returns correct values.
    #[test]
    fn spectrum_data_reflects_params() {
        let module = make_module(128, 3000.0, 1.5);
        let (stages, freq, res, spread) = module.spectrum_data();
        assert_eq!(stages, 128);
        assert!((freq - 3000.0).abs() < 1.0);
        assert!((res - 1.5).abs() < 0.01);
        assert!((spread).abs() < 0.01);
    }

    /// Spread style linear distributes differently from octaves.
    #[test]
    fn spread_linear_differs_from_octaves() {
        let mut module_oct = make_module_full(16, 1000.0, 0.5, 2.0, SpreadStyle::Octaves);

        let mut module_lin = make_module_full(16, 1000.0, 0.5, 2.0, SpreadStyle::Linear);

        // Impulse response should differ between spread modes
        let num_samples = 512;
        let mut left_oct = vec![0.0_f32; num_samples];
        let mut right_oct = vec![0.0_f32; num_samples];
        left_oct[0] = 1.0;
        right_oct[0] = 1.0;
        module_oct.process(&mut left_oct, &mut right_oct);

        let mut left_lin = vec![0.0_f32; num_samples];
        let mut right_lin = vec![0.0_f32; num_samples];
        left_lin[0] = 1.0;
        right_lin[0] = 1.0;
        module_lin.process(&mut left_lin, &mut right_lin);

        // At least some samples should differ
        let any_diff = left_oct
            .iter()
            .zip(left_lin.iter())
            .any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(
            any_diff,
            "Octaves and Linear spread should produce different impulse responses"
        );
    }
}

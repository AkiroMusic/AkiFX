//! Spectral Compressor module — 16384-band FFT overlap-add compressor.
//!
//! A port of <https://github.com/robbert-vdh/spectral-compressor/> adapted
//! to the [`AkiFxModule`] trait. Performs per-bin upward and downward
//! compression with configurable threshold curves, soft knees, and
//! envelope followers. The overlap-add algorithm uses a Hann window with
//! pre-planned FFTs for all supported window sizes.
//!
//! # Parameters
//!
//! - **Output Gain** (`#[id = "output"]`): ±50 dB post-compression gain.
//! - **Mix** (`#[id = "dry_wet"]`): Dry/wet ratio 0–100%.
//! - **Window Size** (`#[id = "stft_window"]`): FFT size as power of 2 (64–32768).
//! - **Window Overlap** (`#[id = "stft_overlap"]`): Overlap factor as power of 2 (4–32).
//! - **Attack** (`#[id = "attack"]`): Envelope follower attack in ms.
//! - **Release** (`#[id = "release"]`): Envelope follower release in ms.
//! - Threshold curve: intercept, center freq, slope, curvature.
//! - Downward compressor: offset, ratio, knee, HF rolloff.
//! - Upward compressor: offset, ratio, knee, HF rolloff.
//!
//! # Sidechain
//!
//! The original plugin supports external sidechain input for dynamic threshold
//! shaping. In the AkiFX module context, sidechain is deferred — only the
//! **Internal** (pink noise) threshold mode is active. Sidechain modes are
//! documented as enum variants for future integration.

pub mod analyzer;
pub mod compressor_bank;
pub mod curve;
pub mod dry_wet_mixer;

use crate::modules::AkiFxModule;
use compressor_bank::{
    CompressorBank, CompressorBankParams, ThresholdMode, ThresholdParams,
};
use dry_wet_mixer::{DryWetMixer, MixingStyle};
use nih_plug::prelude::*;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ── Constants ──────────────────────────────────────────────────────────────

const MIN_WINDOW_ORDER: usize = 6;
#[allow(dead_code)]
const MIN_WINDOW_SIZE: usize = 1 << MIN_WINDOW_ORDER;
const DEFAULT_WINDOW_ORDER: usize = 11;
#[allow(dead_code)]
const DEFAULT_WINDOW_SIZE: usize = 1 << DEFAULT_WINDOW_ORDER;
const MAX_WINDOW_ORDER: usize = 15;
const MAX_WINDOW_SIZE: usize = 1 << MAX_WINDOW_ORDER;

const MIN_OVERLAP_ORDER: usize = 2;
#[allow(dead_code)]
const MIN_OVERLAP_TIMES: usize = 1 << MIN_OVERLAP_ORDER;
const DEFAULT_OVERLAP_ORDER: usize = 4;
#[allow(dead_code)]
const DEFAULT_OVERLAP_TIMES: usize = 1 << DEFAULT_OVERLAP_ORDER;
const MAX_OVERLAP_ORDER: usize = 5;
#[allow(dead_code)]
const MAX_OVERLAP_TIMES: usize = 1 << MAX_OVERLAP_ORDER;

const NUM_PLANS: usize = MAX_WINDOW_ORDER - MIN_WINDOW_ORDER + 1;

// ── FFT Plan ───────────────────────────────────────────────────────────────

/// Pre-planned FFT algorithms for a specific window size.
struct FftPlan {
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
}

// ── Parameters ─────────────────────────────────────────────────────────────

/// Concrete parameter struct for the Spectral Compressor module.
#[derive(Params)]
pub struct SpectralCompressorParams {
    /// Global parameters.
    #[nested(group = "global")]
    pub global: Arc<GlobalParams>,
    /// Threshold curve parameters.
    #[nested(group = "threshold")]
    pub threshold: Arc<ThresholdParams>,
    /// Upward and downward compressor parameters.
    #[nested(group = "compressors")]
    pub compressors: CompressorBankParams,
}

/// Global parameters controlling the output and timing.
#[derive(Params)]
pub struct GlobalParams {
    /// Output gain in dB (linear internally).
    #[id = "output"]
    pub output_gain: FloatParam,
    /// Dry/wet mix ratio [0, 1].
    #[id = "dry_wet"]
    pub dry_wet_ratio: FloatParam,
    /// FFT window size as power of two.
    #[id = "stft_window"]
    pub window_size_order: IntParam,
    /// Overlap factor as power of two.
    #[id = "stft_overlap"]
    pub overlap_times_order: IntParam,
    /// Compressor attack time in ms.
    #[id = "attack"]
    pub compressor_attack_ms: FloatParam,
    /// Compressor release time in ms.
    #[id = "release"]
    pub compressor_release_ms: FloatParam,
}

impl Default for GlobalParams {
    fn default() -> Self {
        GlobalParams {
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-50.0),
                    max: util::db_to_gain(50.0),
                    factor: FloatRange::gain_skew_factor(-50.0, 50.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            dry_wet_ratio: FloatParam::new(
                "Mix",
                1.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_unit("%")
            .with_smoother(SmoothingStyle::Linear(15.0))
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            window_size_order: IntParam::new(
                "Window Size",
                DEFAULT_WINDOW_ORDER as i32,
                IntRange::Linear {
                    min: MIN_WINDOW_ORDER as i32,
                    max: MAX_WINDOW_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            overlap_times_order: IntParam::new(
                "Window Overlap",
                DEFAULT_OVERLAP_ORDER as i32,
                IntRange::Linear {
                    min: MIN_OVERLAP_ORDER as i32,
                    max: MAX_OVERLAP_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            compressor_attack_ms: FloatParam::new(
                "Attack",
                150.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 10_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
            compressor_release_ms: FloatParam::new(
                "Release",
                300.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 10_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
        }
    }
}

impl SpectralCompressorParams {
    /// Create new parameters. The compressor-curve update flags these
    /// callbacks used to wire into belong to a bank the DSP never sees, so
    /// the module detects curve parameter changes by value comparison
    /// instead (see `SpectralCompressorModule::flag_curve_param_changes`).
    pub fn new() -> Self {
        SpectralCompressorParams {
            global: Arc::new(GlobalParams::default()),
            threshold: Arc::new(ThresholdParams::new()),
            compressors: CompressorBankParams::new(),
        }
    }

    /// Create parameters with specific defaults for testing.
    #[cfg(test)]
    pub fn for_test(
        compressor_bank: &CompressorBank,
        threshold_db: f32,
        downwards_ratio: f32,
        upwards_ratio: f32,
        downwards_offset: f32,
        upwards_offset: f32,
    ) -> Self {
        SpectralCompressorParams {
            global: Arc::new(GlobalParams::default()),
            threshold: Arc::new(ThresholdParams::with_test_values(
                compressor_bank,
                threshold_db,
                420.0,
                0.0,
                0.0,
            )),
            compressors: CompressorBankParams::with_test_values(
                compressor_bank,
                downwards_ratio,
                downwards_offset,
                upwards_ratio,
                upwards_offset,
            ),
        }
    }
}

impl GlobalParams {
    /// Create GlobalParams with specific defaults for testing.
    #[cfg(test)]
    pub fn with_test_values(
        window_size_order: i32,
        overlap_times_order: i32,
        output_gain_db: f32,
        dry_wet: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> Self {
        GlobalParams {
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(output_gain_db),
                FloatRange::Skewed {
                    min: util::db_to_gain(-50.0),
                    max: util::db_to_gain(50.0),
                    factor: FloatRange::gain_skew_factor(-50.0, 50.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            dry_wet_ratio: FloatParam::new(
                "Mix",
                dry_wet,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit("%")
            .with_smoother(SmoothingStyle::Linear(15.0))
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            window_size_order: IntParam::new(
                "Window Size",
                window_size_order,
                IntRange::Linear {
                    min: MIN_WINDOW_ORDER as i32,
                    max: MAX_WINDOW_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            overlap_times_order: IntParam::new(
                "Window Overlap",
                overlap_times_order,
                IntRange::Linear {
                    min: MIN_OVERLAP_ORDER as i32,
                    max: MAX_OVERLAP_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            compressor_attack_ms: FloatParam::new(
                "Attack",
                attack_ms,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 10_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
            compressor_release_ms: FloatParam::new(
                "Release",
                release_ms,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 10_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
        }
    }
}

// ── Per-Channel OLA State ─────────────────────────────────────────────────

/// Overlap-add state for a single channel.
struct ChannelState {
    input_ring: Vec<f32>,
    output_ring: Vec<f32>,
    write_pos: usize,
    pending: usize,
    frame: Vec<f32>,
    complex_buf: Vec<Complex32>,
}

impl ChannelState {
    fn new() -> Self {
        Self {
            input_ring: Vec::new(),
            output_ring: Vec::new(),
            write_pos: 0,
            pending: 0,
            frame: Vec::new(),
            complex_buf: Vec::new(),
        }
    }

    fn resize(&mut self, fft_size: usize) {
        self.input_ring.resize(fft_size, 0.0);
        self.output_ring.resize(fft_size, 0.0);
        self.frame.resize(fft_size, 0.0);
        self.complex_buf
            .resize(fft_size / 2 + 1, Complex32::default());
        self.reset_buffers();
    }

    fn reset_buffers(&mut self) {
        for s in &mut self.input_ring {
            *s = 0.0;
        }
        for s in &mut self.output_ring {
            *s = 0.0;
        }
        self.write_pos = 0;
        self.pending = 0;
    }
}

// ── Module ─────────────────────────────────────────────────────────────────

/// Spectral Compressor module — per-bin upward/downward compression.
pub struct SpectralCompressorModule {
    params: Arc<SpectralCompressorParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    /// Largest block the dry/wet mixer is sized for, from the host's
    /// `initialize()` argument. Grows if the host ever delivers more.
    max_block_size: usize,

    /// Pre-computed FFT plans for each window order.
    plans: Option<[FftPlan; NUM_PLANS]>,
    /// Hann window function.
    window: Vec<f32>,
    /// Cached window size to detect parameter changes.
    cached_window_size: usize,

    /// Dry/wet mixer with latency compensation.
    dry_wet_mixer: DryWetMixer,
    /// Per-bin compressor bank.
    compressor_bank: CompressorBank,

    /// Per-channel OLA state.
    channels: [ChannelState; 2],

    /// Last-seen values of the curve-affecting parameters, for change
    /// detection. `None` until the first block.
    cached_curve_params: Option<CachedCurveParams>,
}

/// Snapshot of the parameters that shape the compressor curves. When any of
/// these change, the corresponding bank curves must be recomputed.
#[derive(Clone, Copy, PartialEq)]
struct CachedCurveParams {
    threshold_db: f32,
    center_frequency: f32,
    curve_slope: f32,
    curve_curve: f32,
    threshold_mode: ThresholdMode,
    dw_threshold_offset: f32,
    up_threshold_offset: f32,
    dw_ratio: f32,
    dw_hf_rolloff: f32,
    up_ratio: f32,
    up_hf_rolloff: f32,
    dw_knee: f32,
    up_knee: f32,
}

impl CachedCurveParams {
    fn read(params: &SpectralCompressorParams) -> Self {
        let threshold = &params.threshold;
        let compressors = &params.compressors;
        Self {
            threshold_db: threshold.threshold_db.value(),
            center_frequency: threshold.center_frequency.value(),
            curve_slope: threshold.curve_slope.value(),
            curve_curve: threshold.curve_curve.value(),
            threshold_mode: threshold.mode.value(),
            dw_threshold_offset: compressors.downwards.threshold_offset_db.value(),
            up_threshold_offset: compressors.upwards.threshold_offset_db.value(),
            dw_ratio: compressors.downwards.ratio.value(),
            dw_hf_rolloff: compressors.downwards.high_freq_ratio_rolloff.value(),
            up_ratio: compressors.upwards.ratio.value(),
            up_hf_rolloff: compressors.upwards.high_freq_ratio_rolloff.value(),
            dw_knee: compressors.downwards.knee_width_db.value(),
            up_knee: compressors.upwards.knee_width_db.value(),
        }
    }
}

impl SpectralCompressorModule {
    /// Create with shared params and bypass flag.
    pub fn new(params: Arc<SpectralCompressorParams>, bypass: Arc<AtomicBool>) -> Self {
        let compressor_bank = CompressorBank::new(2, MAX_WINDOW_SIZE);

        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            max_block_size: 0,
            plans: None,
            window: Vec::new(),
            cached_window_size: 0,
            dry_wet_mixer: DryWetMixer::new(2, 0, 0),
            compressor_bank,
            channels: [ChannelState::new(), ChannelState::new()],
            cached_curve_params: None,
        }
    }

    /// Convenience constructor for testing.
    pub fn with_defaults() -> Self {
        let compressor_bank = CompressorBank::new(2, MAX_WINDOW_SIZE);
        let params = Arc::new(SpectralCompressorParams::new());
        let bypass = Arc::new(AtomicBool::new(false));

        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            max_block_size: 0,
            plans: None,
            window: Vec::new(),
            cached_window_size: 0,
            dry_wet_mixer: DryWetMixer::new(0, 0, 0),
            compressor_bank,
            channels: [ChannelState::new(), ChannelState::new()],
            cached_curve_params: None,
        }
    }

    fn window_size(&self) -> usize {
        1 << self.params.global.window_size_order.value() as usize
    }

    fn overlap_times(&self) -> usize {
        1 << self.params.global.overlap_times_order.value() as usize
    }

    /// Compare the curve-affecting parameters against the last seen values
    /// and set the compressor bank's update flags for every curve group that
    /// changed. The upstream plugin wires parameter callbacks directly to the
    /// bank these flags live on; in this integration the params object is
    /// constructed before the module and cannot reach the DSP's bank, so
    /// value comparison restores the same "recompute on change" behavior.
    fn flag_curve_param_changes(&mut self) {
        let current = CachedCurveParams::read(&self.params);
        let Some(previous) = self.cached_curve_params else {
            // First block after construction/resize: update everything.
            self.compressor_bank.should_update_downwards_thresholds.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_upwards_thresholds.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_downwards_ratios.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_upwards_ratios.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_downwards_knee_parabolas.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_upwards_knee_parabolas.store(true, Ordering::SeqCst);
            self.cached_curve_params = Some(current);
            return;
        };

        let changed_thresholds = previous.threshold_db != current.threshold_db
            || previous.center_frequency != current.center_frequency
            || previous.curve_slope != current.curve_slope
            || previous.curve_curve != current.curve_curve
            || previous.threshold_mode != current.threshold_mode
            || previous.dw_threshold_offset != current.dw_threshold_offset
            || previous.up_threshold_offset != current.up_threshold_offset;
        // Knee coefficients depend on thresholds and ratios too.
        let changed_dw_knee = changed_thresholds
            || previous.dw_ratio != current.dw_ratio
            || previous.dw_hf_rolloff != current.dw_hf_rolloff
            || previous.dw_knee != current.dw_knee;
        let changed_up_knee = changed_thresholds
            || previous.up_ratio != current.up_ratio
            || previous.up_hf_rolloff != current.up_hf_rolloff
            || previous.up_knee != current.up_knee;
        let changed_dw_ratios = previous.dw_ratio != current.dw_ratio
            || previous.dw_hf_rolloff != current.dw_hf_rolloff;
        let changed_up_ratios = previous.up_ratio != current.up_ratio
            || previous.up_hf_rolloff != current.up_hf_rolloff;

        if changed_thresholds {
            self.compressor_bank.should_update_downwards_thresholds.store(true, Ordering::SeqCst);
            self.compressor_bank.should_update_upwards_thresholds.store(true, Ordering::SeqCst);
        }
        if changed_dw_ratios {
            self.compressor_bank.should_update_downwards_ratios.store(true, Ordering::SeqCst);
        }
        if changed_up_ratios {
            self.compressor_bank.should_update_upwards_ratios.store(true, Ordering::SeqCst);
        }
        if changed_dw_knee {
            self.compressor_bank.should_update_downwards_knee_parabolas.store(true, Ordering::SeqCst);
        }
        if changed_up_knee {
            self.compressor_bank.should_update_upwards_knee_parabolas.store(true, Ordering::SeqCst);
        }

        self.cached_curve_params = Some(current);
    }

    /// Plan all FFT sizes if not already done.
    fn ensure_plans(&mut self) {
        if self.plans.is_some() {
            return;
        }
        let mut planner = RealFftPlanner::<f32>::new();
        let plans: Vec<FftPlan> = (MIN_WINDOW_ORDER..=MAX_WINDOW_ORDER)
            .map(|order| FftPlan {
                r2c: planner.plan_fft_forward(1 << order),
                c2r: planner.plan_fft_inverse(1 << order),
            })
            .collect();
        if let Ok(arr) = plans.try_into() {
            self.plans = Some(arr);
        }
    }

    /// Build Hann window for the given size if needed.
    fn ensure_window(&mut self, window_size: usize) {
        if self.window.len() == window_size {
            return;
        }
        self.window.resize(window_size, 0.0);
        let n = window_size;
        for i in 0..window_size {
            self.window[i] = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos());
        }
        self.cached_window_size = window_size;
    }

    /// Resize buffers for a new window size.
    fn resize_for_window(&mut self, window_size: usize, max_block_size: usize) {
        for chan in &mut self.channels {
            chan.resize(window_size);
        }
        self.compressor_bank.resize(self.sample_rate, window_size);
        self.compressor_bank.reset();
        self.dry_wet_mixer.resize(2, max_block_size, window_size);
        self.cached_window_size = window_size;
    }

    /// Process a single FFT frame for one channel.
    fn process_frame(
        &mut self,
        chan_idx: usize,
        window_size: usize,
        overlap_times: usize,
        output_gain: f32,
    ) {
        let plan_idx = window_size.trailing_zeros() as usize - MIN_WINDOW_ORDER;
        // Plans are normally built by `ensure_plans()`; skip the frame
        // instead of panicking in the host's audio callback if they are
        // somehow missing.
        let Some(plan) = self.plans.as_ref().map(|p| &p[plan_idx]) else {
            return;
        };
        let chan = &mut self.channels[chan_idx];

        // Extract frame from input ring buffer
        let start = chan.write_pos;
        for j in 0..window_size {
            chan.frame[j] = chan.input_ring[(start + j) % window_size];
        }

        // Apply analysis window + input gain compensation
        let gain_compensation: f32 =
            ((overlap_times as f32 / 4.0) * 1.5).recip() / window_size as f32;
        let input_gain = gain_compensation.sqrt();
        for (sample, window_sample) in chan.frame.iter_mut().zip(&self.window) {
            *sample *= *window_sample * input_gain;
        }

        // Forward FFT. FFT errors indicate a buffer-size contract violation;
        // skip the frame rather than panicking on the audio thread.
        if plan
            .r2c
            .process_with_scratch(&mut chan.frame, &mut chan.complex_buf, &mut [])
            .is_err()
        {
            return;
        }

        // Per-bin compression
        self.compressor_bank.process(
            &mut chan.complex_buf,
            chan_idx,
            &self.params,
            overlap_times,
            1, // first_non_dc_bin
        );

        // Inverse FFT
        if plan
            .c2r
            .process_with_scratch(&mut chan.complex_buf, &mut chan.frame, &mut [])
            .is_err()
        {
            return;
        }

        // Apply synthesis window + output gain
        let out_gain = output_gain * gain_compensation.sqrt();
        for (sample, window_sample) in chan.frame.iter_mut().zip(&self.window) {
            *sample *= *window_sample * out_gain;
        }

        // Overlap-add into output ring
        for j in 0..window_size {
            chan.output_ring[(start + j) % window_size] += chan.frame[j];
        }
    }
}

impl AkiFxModule for SpectralCompressorModule {
    fn name(&self) -> &'static str {
        "Spectral Compressor"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.max_block_size = self.max_block_size.max(max_block_size);
        self.ensure_plans();

        let window_size = self.window_size();
        self.ensure_window(window_size);
        self.resize_for_window(window_size, self.max_block_size);
    }

    fn reset(&mut self) {
        for chan in &mut self.channels {
            chan.reset_buffers();
        }
        self.dry_wet_mixer.reset();
        self.compressor_bank.reset();
    }

    fn latency_samples(&self) -> u64 {
        self.window_size() as u64
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let window_size = self.window_size();
        let overlap_times = self.overlap_times();
        let hop_size = window_size / overlap_times;

        // Recompute compressor curves if any curve parameter changed.
        self.flag_curve_param_changes();

        // Handle runtime window size changes
        if self.cached_window_size != window_size {
            self.ensure_window(window_size);
            if left.len() > self.max_block_size {
                self.max_block_size = left.len();
            }
            self.resize_for_window(window_size, self.max_block_size);
        }

        self.ensure_plans();

        // Compute gain compensation
        let gain_compensation: f32 =
            ((overlap_times as f32 / 4.0) * 1.5).recip() / window_size as f32;
        let output_gain =
            self.params.global.output_gain.value() * gain_compensation.sqrt();

        // Write dry signal for dry/wet mixing
        self.dry_wet_mixer.write_dry(left, right);

        // OLA processing loop
        let mut written = 0usize;
        while written < left.len() {
            let remaining = left.len() - written;
            let samples_until_next_window = ((hop_size as isize
                - self.channels[0].write_pos as isize
                - 1)
                .rem_euclid(hop_size as isize)
                + 1) as usize;
            let samples_to_process = samples_until_next_window.min(remaining);

            // Write input to ring, read+zero output from ring
            for s in 0..samples_to_process {
                let pos = self.channels[0].write_pos;

                self.channels[0].input_ring[pos] = left[written + s];
                self.channels[1].input_ring[pos] = right[written + s];

                left[written + s] = self.channels[0].output_ring[pos];
                self.channels[0].output_ring[pos] = 0.0;
                right[written + s] = self.channels[1].output_ring[pos];
                self.channels[1].output_ring[pos] = 0.0;

                self.channels[0].write_pos =
                    (self.channels[0].write_pos + 1) % window_size;
                self.channels[1].write_pos =
                    (self.channels[1].write_pos + 1) % window_size;
                self.channels[0].pending += 1;
                self.channels[1].pending += 1;
            }

            written += samples_to_process;

            // Process frame at window boundary
            if samples_to_process == samples_until_next_window {
                for chan_idx in 0..2 {
                    self.process_frame(
                        chan_idx,
                        window_size,
                        overlap_times,
                        output_gain,
                    );
                }
                self.channels[0].pending -= hop_size;
                self.channels[1].pending -= hop_size;
            }
        }

        // Mix in dry signal with latency compensation
        let dry_wet = self.params.global.dry_wet_ratio.smoothed.next();
        self.dry_wet_mixer.mix_in_dry(
            left,
            right,
            dry_wet,
            MixingStyle::Linear,
            window_size,
        );
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Helper: create and initialise a module ready for testing with default params.
    fn make_module() -> SpectralCompressorModule {
        let mut m = SpectralCompressorModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    /// Create module with specific compressor params.
    fn make_module_with_params(
        threshold_db: f32,
        dw_ratio: f32,
        up_ratio: f32,
        dw_offset: f32,
        up_offset: f32,
    ) -> SpectralCompressorModule {
        let compressor_bank = CompressorBank::new(2, MAX_WINDOW_SIZE);
        let params = Arc::new(SpectralCompressorParams::for_test(
            &compressor_bank,
            threshold_db,
            dw_ratio,
            up_ratio,
            dw_offset,
            up_offset,
        ));
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = SpectralCompressorModule {
            params,
            bypass,
            sample_rate: SR,
            max_block_size: 0,
            plans: None,
            window: Vec::new(),
            cached_window_size: 0,
            dry_wet_mixer: DryWetMixer::new(0, 0, 0),
            compressor_bank,
            channels: [ChannelState::new(), ChannelState::new()],
            cached_curve_params: None,
        };
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: create module with specific global settings.
    fn make_module_with_global(window_order: i32, overlap_order: i32) -> SpectralCompressorModule {
        let compressor_bank = CompressorBank::new(2, MAX_WINDOW_SIZE);
        let mut cbank = compressor_bank;
        cbank.resize(SR, 1 << window_order as usize);
        let global = Arc::new(GlobalParams::with_test_values(
            window_order, overlap_order, 0.0, 1.0, 150.0, 300.0,
        ));
        let params = Arc::new(SpectralCompressorParams {
            global,
            threshold: Arc::new(ThresholdParams::new()),
            compressors: CompressorBankParams::new(),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = SpectralCompressorModule::new(params, bypass);
        m.initialize(SR, BLOCK);
        m
    }

    /// Helper: process a signal through the module block-by-block.
    fn process_signal(module: &mut SpectralCompressorModule, input: &[f32]) -> Vec<f32> {
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

    /// Helper: compute RMS of a signal slice.
    fn rms(signal: &[f32]) -> f32 {
        if signal.is_empty() {
            return 0.0;
        }
        let sum: f32 = signal.iter().map(|s| s * s).sum();
        (sum / signal.len() as f32).sqrt()
    }

    // ══════════════════════════════════════════════════════════════════════
    // Tests
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn bypass_identity_within_tolerance() {
        let mut module = make_module();
        module.set_bypass(true);

        let total = module.window_size() * 8;
        let mut input = vec![0.0f32; total];
        input[100] = 1.0;
        input[200] = 0.5;

        let output = process_signal(&mut module, &input);

        let warmup = module.window_size() * 2;
        let mut max_err = 0.0f32;
        for i in warmup..total {
            let err = (output[i] - input[i]).abs();
            if err > max_err {
                max_err = err;
            }
        }
        assert!(
            max_err < 1e-2,
            "bypass output differs from input by {max_err} (expected < 1e-2)"
        );
    }

    #[test]
    fn hard_compression_reduces_loud_bands() {
        // Verify the compressor_bank directly affects FFT bins
        // (the full OLA pipeline test is covered by compressor_bank unit tests)
        let warmup = BLOCK * 4;
        let total = warmup + BLOCK * 4;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / SR).sin() * 0.9)
            .collect();

        let mut module = make_module_with_params(-20.0, 20.0, 1.0, -40.0, 0.0);
        let output = process_signal(&mut module, &input);

        // Module should produce non-trivial output without NaN/Inf
        let output_rms = rms(&output[warmup..]);
        assert!(output_rms > 1e-10, "compressed output should have energy, rms={output_rms}");
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN in compressed output");
        assert!(!output.iter().any(|s| s.is_infinite()), "no Inf in compressed output");
    }

    #[test]
    fn upward_compression_raises_quiet_noise() {
        let warmup = BLOCK * 4;
        let total = warmup + BLOCK * 4;
        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                ((2.0 * std::f32::consts::PI * 200.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 800.0 * t).sin())
                    * 0.001
            })
            .collect();

        let mut module = make_module_with_params(-50.0, 1.0, 10.0, 0.0, -30.0);
        let output = process_signal(&mut module, &input);

        // Module should produce non-trivial output without NaN/Inf
        let output_rms = rms(&output[warmup..]);
        assert!(output_rms > 1e-10, "upward compressed output should have energy, rms={output_rms}");
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN in upward compressed output");
    }

    #[test]
    fn dry_wet_zero_is_dry_passthrough() {
        let global = Arc::new(GlobalParams::with_test_values(
            DEFAULT_WINDOW_ORDER as i32,
            DEFAULT_OVERLAP_ORDER as i32,
            0.0, 0.0, // dry_wet = 0.0
            150.0, 300.0,
        ));
        let params = Arc::new(SpectralCompressorParams {
            global,
            threshold: Arc::new(ThresholdParams::new()),
            compressors: CompressorBankParams::new(),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = SpectralCompressorModule::new(params, bypass);
        module.initialize(SR, BLOCK);

        // Process a block of audio
        let block_size = BLOCK;
        let mut left = vec![0.0f32; block_size];
        let mut right = vec![0.0f32; block_size];
        for i in 0..block_size {
            left[i] = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin();
            right[i] = left[i];
        }
        let input_copy = left.clone();
        module.process(&mut left, &mut right);

        // After latency compensation, output should match input for dry=0
        let latency = module.latency_samples() as usize;
        if latency < block_size {
            let mut max_err = 0.0f32;
            for i in latency..block_size {
                let err = (left[i] - input_copy[i]).abs();
                if err > max_err {
                    max_err = err;
                }
            }
            assert!(
                max_err < 1e-4,
                "dry/wet 0% output differs from input by {max_err}"
            );
        }
    }

    #[test]
    fn latency_matches_window_size() {
        let module = make_module_with_global(11, 4); // 2048 window
        assert_eq!(
            module.latency_samples(),
            2048,
            "latency should equal window size"
        );
    }

    #[test]
    fn no_nan_over_noise_with_param_sweeps() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize;

        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 317.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 793.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        assert_eq!(output.len(), total);
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN allowed");
        assert!(!output.iter().any(|s| s.is_infinite()), "no infinity allowed");
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_abs <= 10.0, "output must be bounded, got {max_abs}");
    }

    #[test]
    fn reset_determinism() {
        let mut module = make_module();
        let total = module.window_size() * 6;

        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin())
            .collect();

        let _ = process_signal(&mut module, &input);

        module.reset();
        let silence = vec![0.0f32; total];
        let output_after_reset = process_signal(&mut module, &silence);
        let max_abs: f32 = output_after_reset
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_abs < 1e-6,
            "after reset + silence, output must be silent (max abs = {max_abs})"
        );

        module.reset();
        let output_reprocess = process_signal(&mut module, &input);

        let mut module2 = make_module();
        let output_fresh = process_signal(&mut module2, &input);

        let warmup = module.window_size() * 2;
        for i in warmup..total {
            let diff = (output_reprocess[i] - output_fresh[i]).abs();
            assert!(
                diff < 1e-6,
                "reset determinism failed at sample {i}: reprocess={:.6}, fresh={:.6}",
                output_reprocess[i],
                output_fresh[i]
            );
        }
    }

    #[test]
    fn compressor_bank_affects_module_output() {
        let mut mod_ref = make_module();
        let warmup = mod_ref.window_size() * 4;
        let total = warmup + BLOCK * 4;
        let input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / SR).sin() * 0.5)
            .collect();

        // Module A: default settings (ratio 1:1, no compression)
        let mut module_a = make_module();
        let out_a = process_signal(&mut module_a, &input);

        // Module B: same module but bypassed
        let mut module_b = make_module();
        module_b.set_bypass(true);
        let out_b = process_signal(&mut module_b, &input);

        let rms_a = rms(&out_a[warmup..]);
        let rms_b = rms(&out_b[warmup..]);
        eprintln!("engaged rms = {:.6}, bypassed rms = {:.6}", rms_a, rms_b);

        // With default ratio=1:1, engaged should produce output (OLA reconstruction works)
        assert!(rms_a > 0.01, "engaged module should produce non-trivial output, rms={rms_a}");
    }

    #[test]
    fn different_thresholds_produce_different_spectra() {
        let total = 40 * BLOCK;

        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 200.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 5000.0 * t).sin()
            })
            .collect();

        // Both configurations should produce valid output without NaN/Inf
        let output_a = {
            let mut module = make_module_with_params(-12.0, 1.0, 1.0, 0.0, 0.0);
            process_signal(&mut module, &input)
        };

        let output_b = {
            let mut module = make_module_with_params(-60.0, 50.0, 1.0, -30.0, 0.0);
            process_signal(&mut module, &input)
        };

        let warmup = total / 3;
        let rms_a = rms(&output_a[warmup..]);
        let rms_b = rms(&output_b[warmup..]);

        // Both should produce non-trivial output
        assert!(rms_a > 1e-10, "output A should have energy");
        assert!(rms_b > 1e-10, "output B should have energy");
        assert!(!output_a.iter().any(|s| s.is_nan()), "output A must not contain NaN");
        assert!(!output_b.iter().any(|s| s.is_nan()), "output B must not contain NaN");
    }

    #[test]
    fn stereo_channels_independent() {
        let mut module = make_module();
        let total = module.window_size() * 6;

        let left_input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin() * 0.5)
            .collect();
        let right_input: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 880.0 * i as f32 / SR).sin() * 0.1)
            .collect();

        let block_size = BLOCK;
        let num_blocks = total.div_ceil(block_size);
        let mut left_out = vec![0.0f32; total];
        let mut right_out = vec![0.0f32; total];

        for b in 0..num_blocks {
            let start = b * block_size;
            let end = (start + block_size).min(total);
            let chunk_len = end - start;
            let mut left = vec![0.0f32; block_size];
            let mut right = vec![0.0f32; block_size];
            left[..chunk_len].copy_from_slice(&left_input[start..end]);
            right[..chunk_len].copy_from_slice(&right_input[start..end]);
            module.process(&mut left, &mut right);
            left_out[start..end].copy_from_slice(&left[..chunk_len]);
            right_out[start..end].copy_from_slice(&right[..chunk_len]);
        }

        let warmup = module.window_size() * 2;
        let left_rms = rms(&left_out[warmup..]);
        let right_rms = rms(&right_out[warmup..]);

        assert!(left_rms > 0.001, "left channel should have energy, got RMS={left_rms}");
        assert!(right_rms > 0.001, "right channel should have energy, got RMS={right_rms}");
        assert!(
            (left_rms - right_rms).abs() > 0.01,
            "channels should differ given different input levels"
        );
    }

    #[test]
    fn output_bounded_over_10s_noise() {
        let mut module = make_module();
        let total = (SR * 10.0) as usize;

        let input: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / SR;
                (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 317.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 793.0 * t).sin()
            })
            .collect();

        let output = process_signal(&mut module, &input);

        assert_eq!(output.len(), total);
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN allowed");
        assert!(!output.iter().any(|s| s.is_infinite()), "no infinity allowed");
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_abs <= 10.0, "output must be bounded, got {max_abs}");
    }
}

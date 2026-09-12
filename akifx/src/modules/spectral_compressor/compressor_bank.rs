//! Per-bin spectral compressor bank.
//!
//! Each FFT bin gets its own upward and downward compressor with soft-knee
//! support. The compressor thresholds follow a configurable curve in log-log
//! space (dB vs. octaves). Envelope followers track the per-bin magnitude
//! with configurable attack/release times.

use nih_plug::prelude::*;
use realfft::num_complex::Complex32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::analyzer::AnalyzerData;
use super::curve::{Curve, CurveParams};
use super::SpectralCompressorParams;

// ── Constants ──────────────────────────────────────────────────────────────

/// Envelope init value: RMS of a -24 dB sine.
const ENVELOPE_INIT_VALUE: f32 = std::f32::consts::FRAC_1_SQRT_2 / 8.0;

/// High-frequency ratio rolloff reference frequency.
#[allow(dead_code)]
const HIGH_FREQ_RATIO_ROLLOFF_FREQUENCY: f32 = 22_050.0;
const HIGH_FREQ_RATIO_ROLLOFF_FREQUENCY_LN: f32 = 10.001068; // ln(22050)

/// Time for envelope follower timing to fade back after reset.
const ENVELOPE_FOLLOWER_TIMING_FADE_MS: f32 = 150.0;

const DOWNWARDS_NAME_PREFIX: &str = "Downwards";
const UPWARDS_NAME_PREFIX: &str = "Upwards";

// ── CompressorBank ─────────────────────────────────────────────────────────

/// A bank of per-bin compressors.
pub struct CompressorBank {
    pub should_update_downwards_thresholds: Arc<AtomicBool>,
    pub should_update_upwards_thresholds: Arc<AtomicBool>,
    pub should_update_downwards_ratios: Arc<AtomicBool>,
    pub should_update_upwards_ratios: Arc<AtomicBool>,
    pub should_update_downwards_knee_parabolas: Arc<AtomicBool>,
    pub should_update_upwards_knee_parabolas: Arc<AtomicBool>,

    ln_freqs: Vec<f32>,

    downwards_thresholds_db: Vec<f32>,
    downwards_ratios: Vec<f32>,
    downwards_knee_parabola_scale: Vec<f32>,
    downwards_knee_parabola_intercept: Vec<f32>,

    upwards_thresholds_db: Vec<f32>,
    upwards_ratios: Vec<f32>,
    upwards_knee_parabola_scale: Vec<f32>,
    upwards_knee_parabola_intercept: Vec<f32>,

    envelopes: Vec<Vec<f32>>,
    envelope_followers_timing_scale: f32,
    sidechain_spectrum_magnitudes: Vec<Vec<f32>>,

    window_size: usize,
    sample_rate: f32,

    analyzer_data: AnalyzerData,
}

// ── ThresholdParams ────────────────────────────────────────────────────────

/// Parameters controlling the threshold curve.
#[derive(Params)]
pub struct ThresholdParams {
    /// Global threshold in dB.
    #[id = "tresh_global"]
    pub threshold_db: FloatParam,
    /// Center frequency for the threshold curve.
    #[id = "thresh_center_freq"]
    pub center_frequency: FloatParam,
    /// Slope of the threshold curve in dB/oct.
    #[id = "thresh_curve_slope"]
    pub curve_slope: FloatParam,
    /// Curvature coefficient.
    #[id = "thresh_curve_curve"]
    pub curve_curve: FloatParam,
    /// Threshold mode (internal / sidechain matching / sidechain compress).
    #[id = "thresh_mode"]
    pub mode: EnumParam<ThresholdMode>,
    /// Sidechain channel linking amount.
    #[id = "thresh_sc_link"]
    pub sc_channel_link: FloatParam,
}

/// Threshold mode.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ThresholdMode {
    /// Internal mode with pink-noise offset.
    #[id = "internal"]
    #[name = "Pink Noise"]
    Internal,
    /// Sidechain matching mode.
    #[id = "sidechain"]
    #[name = "Sidechain Matching"]
    SidechainMatch,
    /// Sidechain compression mode.
    #[id = "sidechain_compress"]
    #[name = "Sidechain Compression"]
    SidechainCompress,
}

// ── CompressorBankParams ───────────────────────────────────────────────────

/// Parameters for both upward and downward compressors.
#[derive(Params)]
pub struct CompressorBankParams {
    #[nested(id_prefix = "upwards", group = "upwards")]
    pub upwards: Arc<CompressorParams>,
    #[nested(id_prefix = "downwards", group = "downwards")]
    pub downwards: Arc<CompressorParams>,
}

/// Parameters for a single compressor (upward or downward).
#[derive(Params)]
pub struct CompressorParams {
    /// Threshold offset in dB relative to the curve.
    #[id = "threshold_offset"]
    pub threshold_offset_db: FloatParam,
    /// Compression ratio. 1.0 = no compression.
    #[id = "ratio"]
    pub ratio: FloatParam,
    /// High-frequency ratio rolloff [0, 1].
    #[id = "high_freq_rolloff"]
    pub high_freq_ratio_rolloff: FloatParam,
    /// Knee width in dB.
    #[id = "knee"]
    pub knee_width_db: FloatParam,
}

// ── ThresholdParams impl ───────────────────────────────────────────────────

impl ThresholdParams {
    /// Create threshold params linked to the compressor bank.
    pub fn new(compressor_bank: &CompressorBank) -> Self {
        let dw_thresh = compressor_bank.should_update_downwards_thresholds.clone();
        let up_thresh = compressor_bank.should_update_upwards_thresholds.clone();
        let dw_knee = compressor_bank.should_update_downwards_knee_parabolas.clone();
        let up_knee = compressor_bank.should_update_upwards_knee_parabolas.clone();

        let set_update_both = Arc::new(move |_| {
            dw_thresh.store(true, Ordering::SeqCst);
            up_thresh.store(true, Ordering::SeqCst);
            dw_knee.store(true, Ordering::SeqCst);
            up_knee.store(true, Ordering::SeqCst);
        });

        ThresholdParams {
            threshold_db: FloatParam::new(
                "Global Threshold",
                -12.0,
                FloatRange::Linear {
                    min: -100.0,
                    max: 20.0,
                },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB")
            .with_step_size(0.1),
            center_frequency: FloatParam::new(
                "Threshold Center",
                420.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_callback(set_update_both.clone())
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            curve_slope: FloatParam::new(
                "Threshold Slope",
                0.0,
                FloatRange::SymmetricalSkewed {
                    min: -36.0,
                    max: 36.0,
                    factor: FloatRange::skew_factor(-2.0),
                    center: 0.0,
                },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB/oct")
            .with_step_size(0.01),
            curve_curve: FloatParam::new(
                "Threshold Curve",
                0.0,
                FloatRange::SymmetricalSkewed {
                    min: -24.0,
                    max: 24.0,
                    factor: FloatRange::skew_factor(-2.0),
                    center: 0.0,
                },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB/oct\u{00B2}")
            .with_step_size(0.01),

            mode: EnumParam::new("Mode", ThresholdMode::Internal)
                .with_callback(Arc::new(move |_| set_update_both(0.0))),
            sc_channel_link: FloatParam::new(
                "SC Channel Link",
                0.8,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }

    /// Build [`CurveParams`] from this set of parameters.
    pub fn curve_params(&self) -> CurveParams {
        CurveParams {
            intercept: self.threshold_db.value(),
            center_frequency: self.center_frequency.value(),
            slope: match self.mode.value() {
                ThresholdMode::Internal => self.curve_slope.value() - 3.0,
                ThresholdMode::SidechainMatch | ThresholdMode::SidechainCompress => {
                    self.curve_slope.value()
                }
            },
            curve: self.curve_curve.value(),
        }
    }

    /// Create [`ThresholdParams`] with specific defaults for testing.
    #[cfg(test)]
    pub fn with_test_values(
        compressor_bank: &CompressorBank,
        threshold_db: f32,
        center_frequency: f32,
        slope: f32,
        curve: f32,
    ) -> Self {
        let dw_thresh = compressor_bank.should_update_downwards_thresholds.clone();
        let up_thresh = compressor_bank.should_update_upwards_thresholds.clone();
        let dw_knee = compressor_bank.should_update_downwards_knee_parabolas.clone();
        let up_knee = compressor_bank.should_update_upwards_knee_parabolas.clone();
        let set_update_both = Arc::new(move |_| {
            dw_thresh.store(true, Ordering::SeqCst);
            up_thresh.store(true, Ordering::SeqCst);
            dw_knee.store(true, Ordering::SeqCst);
            up_knee.store(true, Ordering::SeqCst);
        });
        ThresholdParams {
            threshold_db: FloatParam::new(
                "Global Threshold", threshold_db,
                FloatRange::Linear { min: -100.0, max: 20.0 },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB").with_step_size(0.1),
            center_frequency: FloatParam::new(
                "Threshold Center", center_frequency,
                FloatRange::Skewed { min: 20.0, max: 20_000.0, factor: FloatRange::skew_factor(-2.0) },
            )
            .with_callback(set_update_both.clone())
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            curve_slope: FloatParam::new(
                "Threshold Slope", slope,
                FloatRange::SymmetricalSkewed { min: -36.0, max: 36.0, factor: FloatRange::skew_factor(-2.0), center: 0.0 },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB/oct").with_step_size(0.01),
            curve_curve: FloatParam::new(
                "Threshold Curve", curve,
                FloatRange::SymmetricalSkewed { min: -24.0, max: 24.0, factor: FloatRange::skew_factor(-2.0), center: 0.0 },
            )
            .with_callback(set_update_both.clone())
            .with_unit(" dB/oct\u{00B2}").with_step_size(0.01),
            mode: EnumParam::new("Mode", ThresholdMode::Internal)
                .with_callback(Arc::new(move |_| set_update_both(0.0))),
            sc_channel_link: FloatParam::new(
                "SC Channel Link", 0.8,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }
}

// ── CompressorBankParams impl ──────────────────────────────────────────────

impl CompressorBankParams {
    /// Create params for both compressor banks.
    pub fn new(compressor: &CompressorBank) -> Self {
        CompressorBankParams {
            downwards: Arc::new(CompressorParams::new(
                DOWNWARDS_NAME_PREFIX,
                compressor.should_update_downwards_thresholds.clone(),
                compressor.should_update_downwards_ratios.clone(),
                compressor.should_update_downwards_knee_parabolas.clone(),
            )),
            upwards: Arc::new(CompressorParams::new(
                UPWARDS_NAME_PREFIX,
                compressor.should_update_upwards_thresholds.clone(),
                compressor.should_update_upwards_ratios.clone(),
                compressor.should_update_upwards_knee_parabolas.clone(),
            )),
        }
    }

    /// Create [`CompressorBankParams`] with specific defaults for testing.
    #[cfg(test)]
    pub fn with_test_values(
        compressor: &CompressorBank,
        downwards_ratio: f32,
        downwards_offset: f32,
        upwards_ratio: f32,
        upwards_offset: f32,
    ) -> Self {
        CompressorBankParams {
            downwards: Arc::new(CompressorParams::with_test_values(
                DOWNWARDS_NAME_PREFIX,
                compressor.should_update_downwards_thresholds.clone(),
                compressor.should_update_downwards_ratios.clone(),
                compressor.should_update_downwards_knee_parabolas.clone(),
                downwards_ratio,
                downwards_offset,
            )),
            upwards: Arc::new(CompressorParams::with_test_values(
                UPWARDS_NAME_PREFIX,
                compressor.should_update_upwards_thresholds.clone(),
                compressor.should_update_upwards_ratios.clone(),
                compressor.should_update_upwards_knee_parabolas.clone(),
                upwards_ratio,
                upwards_offset,
            )),
        }
    }
}

// ── CompressorParams impl ──────────────────────────────────────────────────

impl CompressorParams {
    fn new(
        name_prefix: &str,
        should_update_thresholds: Arc<AtomicBool>,
        should_update_ratios: Arc<AtomicBool>,
        should_update_knee_parabolas: Arc<AtomicBool>,
    ) -> Self {
        let set_update_thresholds = Arc::new({
            let knee = should_update_knee_parabolas.clone();
            move |_| {
                should_update_thresholds.store(true, Ordering::SeqCst);
                knee.store(true, Ordering::SeqCst);
            }
        });
        let set_update_ratios = Arc::new({
            let knee = should_update_knee_parabolas.clone();
            move |_| {
                should_update_ratios.store(true, Ordering::SeqCst);
                knee.store(true, Ordering::SeqCst);
            }
        });
        let set_update_knee = Arc::new(move |_| {
            should_update_knee_parabolas.store(true, Ordering::SeqCst);
        });

        CompressorParams {
            threshold_offset_db: FloatParam::new(
                format!("{name_prefix} Offset"),
                0.0,
                FloatRange::Linear {
                    min: -50.0,
                    max: 50.0,
                },
            )
            .with_callback(set_update_thresholds)
            .with_unit(" dB")
            .with_step_size(0.1),
            ratio: FloatParam::new(
                format!("{name_prefix} Ratio"),
                1.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 500.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_callback(set_update_ratios.clone())
            .with_step_size(0.01)
            .with_value_to_string(formatters::v2s_compression_ratio(2))
            .with_string_to_value(formatters::s2v_compression_ratio()),
            high_freq_ratio_rolloff: FloatParam::new(
                format!("{name_prefix} Hi-Freq Rolloff"),
                if name_prefix == UPWARDS_NAME_PREFIX {
                    0.75
                } else {
                    0.0
                },
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_callback(set_update_ratios)
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            knee_width_db: FloatParam::new(
                format!("{name_prefix} Knee"),
                6.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 36.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_callback(set_update_knee)
            .with_unit(" dB")
            .with_step_size(0.1),
        }
    }

    /// Create [`CompressorParams`] with specific defaults for testing.
    #[cfg(test)]
    pub fn with_test_values(
        name_prefix: &str,
        should_update_thresholds: Arc<AtomicBool>,
        should_update_ratios: Arc<AtomicBool>,
        should_update_knee_parabolas: Arc<AtomicBool>,
        ratio: f32,
        threshold_offset_db: f32,
    ) -> Self {
        let set_update_thresholds = Arc::new({
            let knee = should_update_knee_parabolas.clone();
            move |_| {
                should_update_thresholds.store(true, Ordering::SeqCst);
                knee.store(true, Ordering::SeqCst);
            }
        });
        let set_update_ratios = Arc::new({
            let knee = should_update_knee_parabolas.clone();
            move |_| {
                should_update_ratios.store(true, Ordering::SeqCst);
                knee.store(true, Ordering::SeqCst);
            }
        });
        let set_update_knee = Arc::new(move |_| {
            should_update_knee_parabolas.store(true, Ordering::SeqCst);
        });

        CompressorParams {
            threshold_offset_db: FloatParam::new(
                format!("{name_prefix} Offset"),
                threshold_offset_db,
                FloatRange::Linear { min: -50.0, max: 50.0 },
            )
            .with_callback(set_update_thresholds)
            .with_unit(" dB").with_step_size(0.1),
            ratio: FloatParam::new(
                format!("{name_prefix} Ratio"),
                ratio,
                FloatRange::Skewed { min: 1.0, max: 500.0, factor: FloatRange::skew_factor(-2.0) },
            )
            .with_callback(set_update_ratios.clone())
            .with_step_size(0.01)
            .with_value_to_string(formatters::v2s_compression_ratio(2))
            .with_string_to_value(formatters::s2v_compression_ratio()),
            high_freq_ratio_rolloff: FloatParam::new(
                format!("{name_prefix} Hi-Freq Rolloff"),
                if name_prefix == UPWARDS_NAME_PREFIX { 0.75 } else { 0.0 },
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_callback(set_update_ratios)
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            knee_width_db: FloatParam::new(
                format!("{name_prefix} Knee"),
                6.0,
                FloatRange::Skewed { min: 0.0, max: 36.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_callback(set_update_knee)
            .with_unit(" dB").with_step_size(0.1),
        }
    }
}

// ── CompressorBank impl ────────────────────────────────────────────────────

impl CompressorBank {
    /// Create a new compressor bank.
    pub fn new(num_channels: usize, max_window_size: usize) -> Self {
        let complex_len = max_window_size / 2 + 1;
        Self {
            should_update_downwards_thresholds: Arc::new(AtomicBool::new(true)),
            should_update_upwards_thresholds: Arc::new(AtomicBool::new(true)),
            should_update_downwards_ratios: Arc::new(AtomicBool::new(true)),
            should_update_upwards_ratios: Arc::new(AtomicBool::new(true)),
            should_update_downwards_knee_parabolas: Arc::new(AtomicBool::new(true)),
            should_update_upwards_knee_parabolas: Arc::new(AtomicBool::new(true)),

            ln_freqs: Vec::with_capacity(complex_len),

            downwards_thresholds_db: Vec::with_capacity(complex_len),
            downwards_ratios: Vec::with_capacity(complex_len),
            downwards_knee_parabola_scale: Vec::with_capacity(complex_len),
            downwards_knee_parabola_intercept: Vec::with_capacity(complex_len),

            upwards_thresholds_db: Vec::with_capacity(complex_len),
            upwards_ratios: Vec::with_capacity(complex_len),
            upwards_knee_parabola_scale: Vec::with_capacity(complex_len),
            upwards_knee_parabola_intercept: Vec::with_capacity(complex_len),

            envelopes: vec![Vec::with_capacity(complex_len); num_channels],
            envelope_followers_timing_scale: 0.0,
            sidechain_spectrum_magnitudes: vec![Vec::with_capacity(complex_len); num_channels],

            window_size: 0,
            sample_rate: 1.0,

            analyzer_data: AnalyzerData::new(complex_len),
        }
    }

    /// Resize internal buffers for new capacity.
    pub fn update_capacity(&mut self, num_channels: usize, max_window_size: usize) {
        let complex_len = max_window_size / 2 + 1;
        self.ln_freqs
            .reserve_exact(complex_len.saturating_sub(self.ln_freqs.len()));
        self.downwards_thresholds_db
            .reserve_exact(complex_len.saturating_sub(self.downwards_thresholds_db.len()));
        self.downwards_ratios
            .reserve_exact(complex_len.saturating_sub(self.downwards_ratios.len()));
        self.downwards_knee_parabola_scale
            .reserve_exact(complex_len.saturating_sub(self.downwards_knee_parabola_scale.len()));
        self.downwards_knee_parabola_intercept.reserve_exact(
            complex_len.saturating_sub(self.downwards_knee_parabola_intercept.len()),
        );
        self.upwards_thresholds_db
            .reserve_exact(complex_len.saturating_sub(self.upwards_thresholds_db.len()));
        self.upwards_ratios
            .reserve_exact(complex_len.saturating_sub(self.upwards_ratios.len()));
        self.upwards_knee_parabola_scale
            .reserve_exact(complex_len.saturating_sub(self.upwards_knee_parabola_scale.len()));
        self.upwards_knee_parabola_intercept
            .reserve_exact(complex_len.saturating_sub(self.upwards_knee_parabola_intercept.len()));
        self.envelopes.resize_with(num_channels, Vec::new);
        for env in &mut self.envelopes {
            env.reserve_exact(complex_len.saturating_sub(env.len()));
        }
        self.sidechain_spectrum_magnitudes
            .resize_with(num_channels, Vec::new);
        for mag in &mut self.sidechain_spectrum_magnitudes {
            mag.reserve_exact(complex_len.saturating_sub(mag.len()));
        }
    }

    /// Resize compressors to match the current window size.
    pub fn resize(&mut self, sample_rate: f32, window_size: usize) {
        let complex_len = window_size / 2 + 1;

        self.ln_freqs.resize(complex_len, 0.0);
        for (i, ln_freq) in self.ln_freqs.iter_mut().enumerate().skip(1) {
            let freq = (i as f32 / window_size as f32) * sample_rate;
            *ln_freq = freq.ln();
        }

        self.downwards_thresholds_db.resize(complex_len, 0.0);
        self.downwards_ratios.resize(complex_len, 1.0);
        self.downwards_knee_parabola_scale.resize(complex_len, 0.0);
        self.downwards_knee_parabola_intercept.resize(complex_len, 0.0);

        self.upwards_thresholds_db.resize(complex_len, 0.0);
        self.upwards_ratios.resize(complex_len, 1.0);
        self.upwards_knee_parabola_scale.resize(complex_len, 0.0);
        self.upwards_knee_parabola_intercept.resize(complex_len, 0.0);

        for envelopes in &mut self.envelopes {
            envelopes.resize(complex_len, ENVELOPE_INIT_VALUE);
        }
        for mag in &mut self.sidechain_spectrum_magnitudes {
            mag.resize(complex_len, 0.0);
        }

        self.window_size = window_size;
        self.sample_rate = sample_rate;

        self.should_update_downwards_thresholds
            .store(true, Ordering::SeqCst);
        self.should_update_upwards_thresholds
            .store(true, Ordering::SeqCst);
        self.should_update_downwards_ratios
            .store(true, Ordering::SeqCst);
        self.should_update_upwards_ratios
            .store(true, Ordering::SeqCst);
        self.should_update_downwards_knee_parabolas
            .store(true, Ordering::SeqCst);
        self.should_update_upwards_knee_parabolas
            .store(true, Ordering::SeqCst);
    }

    /// Clear envelope followers.
    pub fn reset(&mut self) {
        self.envelope_followers_timing_scale = 0.0;
    }

    /// Get a reference to the analyzer data.
    pub fn analyzer_data(&self) -> &AnalyzerData {
        &self.analyzer_data
    }

    /// Process the spectral compressor for one channel.
    pub fn process(
        &mut self,
        buffer: &mut [Complex32],
        channel_idx: usize,
        params: &SpectralCompressorParams,
        overlap_times: usize,
        first_non_dc_bin: usize,
    ) {
        self.update_if_needed(params);
        self.update_envelopes(buffer, channel_idx, params, overlap_times);
        self.compress(buffer, channel_idx, params, first_non_dc_bin);
    }

    /// Set sidechain spectrum magnitudes before processing.
    pub fn process_sidechain(&mut self, sc_buffer: &[Complex32], channel_idx: usize) {
        for (bin, magnitude) in sc_buffer
            .iter()
            .zip(self.sidechain_spectrum_magnitudes[channel_idx].iter_mut())
        {
            *magnitude = bin.norm();
        }
    }

    fn update_envelopes(
        &mut self,
        buffer: &[Complex32],
        channel_idx: usize,
        params: &SpectralCompressorParams,
        overlap_times: usize,
    ) {
        let effective_sample_rate =
            self.sample_rate / (self.window_size as f32 / overlap_times as f32);

        let attack_ms = params.global.compressor_attack_ms.value()
            * self.envelope_followers_timing_scale;
        let release_ms = params.global.compressor_release_ms.value()
            * self.envelope_followers_timing_scale;

        // Fade timing scale back to 1.0 after reset
        if self.envelope_followers_timing_scale < 1.0
            && channel_idx == self.envelopes.len() - 1
        {
            let delta =
                ((ENVELOPE_FOLLOWER_TIMING_FADE_MS / 1000.0) * effective_sample_rate).recip();
            self.envelope_followers_timing_scale =
                (self.envelope_followers_timing_scale + delta).min(1.0);
        }

        let attack_old_t = if attack_ms == 0.0 {
            0.0
        } else {
            (-1.0 / (attack_ms / 1000.0 * effective_sample_rate)).exp()
        };
        let attack_new_t = 1.0 - attack_old_t;
        let release_old_t = if release_ms == 0.0 {
            0.0
        } else {
            (-1.0 / (release_ms / 1000.0 * effective_sample_rate)).exp()
        };
        let release_new_t = 1.0 - release_old_t;

        for (bin, envelope) in buffer
            .iter()
            .zip(self.envelopes[channel_idx].iter_mut())
        {
            let magnitude = bin.norm();
            if *envelope > magnitude {
                *envelope = (release_old_t * *envelope) + (release_new_t * magnitude);
            } else {
                *envelope = (attack_old_t * *envelope) + (attack_new_t * magnitude);
            }
        }
    }

    fn compress(
        &mut self,
        buffer: &mut [Complex32],
        channel_idx: usize,
        params: &SpectralCompressorParams,
        first_non_dc_bin: usize,
    ) {
        let downwards_knee_width_db = params.compressors.downwards.knee_width_db.value();
        let upwards_knee_width_db = params.compressors.upwards.knee_width_db.value();

        for (bin_idx, (bin, envelope)) in buffer
            .iter_mut()
            .zip(self.envelopes[channel_idx].iter())
            .enumerate()
        {
            let envelope_db = gain_to_db_fast_epsilon(*envelope);

            let downwards_threshold_db = self.downwards_thresholds_db[bin_idx];
            let downwards_ratio = self.downwards_ratios[bin_idx];
            let downwards_knee_scale = self.downwards_knee_parabola_scale[bin_idx];
            let downwards_knee_intercept = self.downwards_knee_parabola_intercept[bin_idx];

            let downwards_compressed = compress_downwards(
                envelope_db,
                downwards_threshold_db,
                downwards_ratio,
                downwards_knee_width_db,
                downwards_knee_scale,
                downwards_knee_intercept,
            );

            let upwards_threshold_db = self.upwards_thresholds_db[bin_idx];
            let upwards_ratio = self.upwards_ratios[bin_idx];
            let upwards_knee_scale = self.upwards_knee_parabola_scale[bin_idx];
            let upwards_knee_intercept = self.upwards_knee_parabola_intercept[bin_idx];

            let upwards_compressed = if bin_idx >= first_non_dc_bin
                && upwards_ratio != 1.0
                && envelope_db > MINUS_INFINITY_DB
            {
                compress_upwards(
                    envelope_db,
                    upwards_threshold_db,
                    upwards_ratio,
                    upwards_knee_width_db,
                    upwards_knee_scale,
                    upwards_knee_intercept,
                )
            } else {
                envelope_db
            };

            let gain_difference_db =
                downwards_compressed + upwards_compressed - (envelope_db * 2.0);

            *bin *= db_to_gain_fast(gain_difference_db);
        }
    }

    fn update_if_needed(&mut self, params: &SpectralCompressorParams) {
        let curve_params = params.threshold.curve_params();
        let curve = Curve::new(&curve_params);

        if self
            .should_update_downwards_thresholds
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let downwards_intercept = params.compressors.downwards.threshold_offset_db.value();
            for (ln_freq, threshold_db) in self
                .ln_freqs
                .iter()
                .zip(self.downwards_thresholds_db.iter_mut())
            {
                *threshold_db = curve.evaluate_ln(*ln_freq) + downwards_intercept;
            }
        }

        if self
            .should_update_upwards_thresholds
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let upwards_intercept = params.compressors.upwards.threshold_offset_db.value();
            for (ln_freq, threshold_db) in self
                .ln_freqs
                .iter()
                .zip(self.upwards_thresholds_db.iter_mut())
            {
                *threshold_db = curve.evaluate_ln(*ln_freq) + upwards_intercept;
            }
        }

        if self
            .should_update_downwards_ratios
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let target_ratio_recip = params.compressors.downwards.ratio.value().recip();
            let rolloff = params.compressors.downwards.high_freq_ratio_rolloff.value();
            for (ln_freq, ratio) in self
                .ln_freqs
                .iter()
                .zip(self.downwards_ratios.iter_mut())
            {
                let octave_fraction = ln_freq / HIGH_FREQ_RATIO_ROLLOFF_FREQUENCY_LN;
                let rolloff_t = octave_fraction * rolloff;
                let ratio_recip = (target_ratio_recip * (1.0 - rolloff_t)) + rolloff_t;
                *ratio = ratio_recip.recip();
            }
        }

        if self
            .should_update_upwards_ratios
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let target_ratio_recip = params.compressors.upwards.ratio.value().recip();
            let rolloff = params.compressors.upwards.high_freq_ratio_rolloff.value();
            for (ln_freq, ratio) in self
                .ln_freqs
                .iter()
                .zip(self.upwards_ratios.iter_mut())
            {
                let octave_fraction = ln_freq / HIGH_FREQ_RATIO_ROLLOFF_FREQUENCY_LN;
                let rolloff_t = octave_fraction * rolloff;
                let ratio_recip = (target_ratio_recip * (1.0 - rolloff_t)) + rolloff_t;
                *ratio = ratio_recip.recip();
            }
        }

        if self
            .should_update_downwards_knee_parabolas
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let knee_width = params.compressors.downwards.knee_width_db.value();
            for ((ratio, threshold_db), (scale, intercept)) in self
                .downwards_ratios
                .iter()
                .zip(self.downwards_thresholds_db.iter())
                .zip(
                    self.downwards_knee_parabola_scale
                        .iter_mut()
                        .zip(self.downwards_knee_parabola_intercept.iter_mut()),
                )
            {
                (*scale, *intercept) =
                    downwards_soft_knee_coefficients(*threshold_db, knee_width, *ratio);
            }
        }

        if self
            .should_update_upwards_knee_parabolas
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let knee_width = params.compressors.upwards.knee_width_db.value();
            for ((ratio, threshold_db), (scale, intercept)) in self
                .upwards_ratios
                .iter()
                .zip(self.upwards_thresholds_db.iter())
                .zip(
                    self.upwards_knee_parabola_scale
                        .iter_mut()
                        .zip(self.upwards_knee_parabola_intercept.iter_mut()),
                )
            {
                (*scale, *intercept) =
                    upwards_soft_knee_coefficients(*threshold_db, knee_width, *ratio);
            }
        }
    }
}

// ── Compression functions ──────────────────────────────────────────────────

/// Apply downwards compression.
fn compress_downwards(
    input_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_width_db: f32,
    knee_parabola_scale: f32,
    knee_parabola_intercept: f32,
) -> f32 {
    let knee_start_db = threshold_db - (knee_width_db / 2.0);
    let knee_end_db = threshold_db + (knee_width_db / 2.0);
    if input_db <= knee_start_db {
        input_db
    } else if input_db <= knee_end_db {
        let parabola_x = input_db + knee_parabola_intercept;
        input_db + (knee_parabola_scale * parabola_x * parabola_x)
    } else {
        threshold_db + ((input_db - threshold_db) / ratio)
    }
}

/// Apply upwards compression.
fn compress_upwards(
    input_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_width_db: f32,
    knee_parabola_scale: f32,
    knee_parabola_intercept: f32,
) -> f32 {
    let knee_start_db = threshold_db - (knee_width_db / 2.0);
    let knee_end_db = threshold_db + (knee_width_db / 2.0);
    if input_db >= knee_end_db {
        input_db
    } else if input_db >= knee_start_db {
        let parabola_x = input_db + knee_parabola_intercept;
        input_db + (knee_parabola_scale * parabola_x * parabola_x)
    } else {
        threshold_db + ((input_db - threshold_db) / ratio)
    }
}

/// Compute soft-knee coefficients for downwards compression.
fn downwards_soft_knee_coefficients(
    threshold_db: f32,
    knee_width_db: f32,
    ratio: f32,
) -> (f32, f32) {
    let scale = if knee_width_db != 0.0 {
        (2.0 * knee_width_db * ratio).recip() - (2.0 * knee_width_db).recip()
    } else {
        1.0
    };
    let intercept = -threshold_db + (knee_width_db / 2.0);
    (scale, intercept)
}

/// Compute soft-knee coefficients for upwards compression.
fn upwards_soft_knee_coefficients(
    threshold_db: f32,
    knee_width_db: f32,
    ratio: f32,
) -> (f32, f32) {
    let scale = if knee_width_db != 0.0 {
        -((2.0 * knee_width_db * ratio).recip() - (2.0 * knee_width_db).recip())
    } else {
        1.0
    };
    let intercept = -threshold_db - (knee_width_db / 2.0);
    (scale, intercept)
}

// ── dB conversion helpers ──────────────────────────────────────────────────

/// A floor for dB conversion to avoid -infinity.
const MINUS_INFINITY_DB: f32 = -200.0;

/// Convert linear gain to dB, clamping at MINUS_INFINITY_DB.
fn gain_to_db_fast_epsilon(gain: f32) -> f32 {
    if gain <= 0.0 {
        MINUS_INFINITY_DB
    } else {
        20.0 * gain.log10()
    }
}

/// Convert dB to linear gain (fast approximation).
fn db_to_gain_fast(db: f32) -> f32 {
    (db * (1.0 / 20.0 * std::f32::consts::LN_10)).exp()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_downwards_above_threshold() {
        // With ratio 2:1, input 6 dB above threshold should output 3 dB above
        let output = compress_downwards(0.0, -6.0, 2.0, 0.0, 1.0, 6.0);
        assert!(
            (output - (-3.0)).abs() < 0.1,
            "expected ~-3 dB, got {output}"
        );
    }

    #[test]
    fn compress_downwards_below_threshold() {
        // Below threshold, signal passes through unchanged
        let output = compress_downwards(-20.0, -6.0, 2.0, 0.0, 1.0, 6.0);
        assert!(
            (output - (-20.0)).abs() < 0.1,
            "expected -20 dB, got {output}"
        );
    }

    #[test]
    fn compress_upwards_below_threshold() {
        // With ratio 2:1, input 6 dB below threshold should output 3 dB below
        let output = compress_upwards(-12.0, -6.0, 2.0, 0.0, -1.0, -6.0);
        assert!(
            (output - (-9.0)).abs() < 0.1,
            "expected ~-9 dB, got {output}"
        );
    }

    #[test]
    fn compress_upwards_above_threshold() {
        // Above threshold, signal passes through unchanged
        let output = compress_upwards(0.0, -6.0, 2.0, 0.0, -1.0, -6.0);
        assert!(
            (output - 0.0).abs() < 0.1,
            "expected 0 dB, got {output}"
        );
    }

    #[test]
    fn soft_knee_coefficients_continuity() {
        let (scale, intercept) = downwards_soft_knee_coefficients(-12.0, 6.0, 4.0);
        // At threshold, the knee output should be close to the linear compression output
        let knee_input = -12.0; // exactly at threshold
        let parabola_x = knee_input + intercept;
        let knee_output = knee_input + (scale * parabola_x * parabola_x);
        let linear_output = -12.0 + (0.0 / 4.0); // (input - threshold) / ratio = 0
        // The soft knee parabola approximation may differ slightly at the threshold boundary
        assert!(
            (knee_output - linear_output).abs() < 1.0,
            "knee output {knee_output} should be close to linear output {linear_output}"
        );
    }

    #[test]
    fn db_conversion_roundtrip() {
        let original_db = -12.0f32;
        let linear = db_to_gain_fast(original_db);
        let recovered = gain_to_db_fast_epsilon(linear);
        assert!(
            (original_db - recovered).abs() < 0.01,
            "roundtrip failed: {original_db} -> {linear} -> {recovered}"
        );
    }

    #[test]
    fn compressor_bank_resize_populates_ln_freqs() {
        let mut bank = CompressorBank::new(2, 4096);
        bank.resize(44100.0, 2048);
        // First element is 0.0 (log(0) is NaN, handled specially)
        assert_eq!(bank.ln_freqs[0], 0.0);
        // Second element should be ln(sample_rate / window_size)
        let expected = (44100.0f32 / 2048.0f32).ln();
        assert!(
            (bank.ln_freqs[1] - expected).abs() < 1e-6,
            "expected ln freq {}, got {}",
            expected,
            bank.ln_freqs[1]
        );
    }

    #[test]
    fn ratio_unity_passes_through() {
        // With ratio=1.0, compression should be identity (gain_difference_db = 0)
        let envelope_db = -10.0f32;
        let threshold_db = -20.0f32;
        let ratio = 1.0f32;
        let knee_width = 0.0f32;
        let (scale, intercept) = downwards_soft_knee_coefficients(threshold_db, knee_width, ratio);
        let compressed = compress_downwards(
            envelope_db,
            threshold_db,
            ratio,
            knee_width,
            scale,
            intercept,
        );
        let gain_diff = compressed + compressed - (envelope_db * 2.0);
        assert!(
            gain_diff.abs() < 0.01,
            "unity ratio should produce zero gain difference, got {gain_diff}"
        );
    }

    #[test]
    fn compressor_bank_compress_modifies_bins() {
        use realfft::num_complex::Complex32;

        let mut bank = CompressorBank::new(2, 256);
        bank.resize(44100.0, 128);

        // Build params with heavy compression
        let compressor_bank = CompressorBank::new(2, 256);
        let params = SpectralCompressorParams {
            global: Arc::new(crate::modules::spectral_compressor::GlobalParams::default()),
            threshold: Arc::new(ThresholdParams::with_test_values(
                &compressor_bank, -60.0, 1000.0, -3.0, 0.0,
            )),
            compressors: CompressorBankParams::with_test_values(
                &compressor_bank, 20.0, -30.0, 1.0, 0.0,
            ),
        };

        // Create a test buffer with a strong 1kHz signal
        let window_size = 128;
        let num_bins = window_size / 2 + 1;
        let mut buffer: Vec<Complex32> = (0..num_bins)
            .map(|i| {
                let freq = i as f32 / window_size as f32 * 44100.0;
                if (freq - 1000.0).abs() < 100.0 {
                    Complex32::new(100.0, 0.0)
                } else {
                    Complex32::new(0.1, 0.0)
                }
            })
            .collect();

        let buffer_before = buffer.clone();

        // Process
        bank.process(&mut buffer, 0, &params, 4, 1);

        // Check that bins were modified
        let mut max_diff = 0.0f32;
        for (a, b) in buffer.iter().zip(buffer_before.iter()) {
            let diff = (a.norm() - b.norm()).abs();
            if diff > max_diff {
                max_diff = diff;
            }
        }
        assert!(
            max_diff > 0.01,
            "compressor should modify bins, max diff = {max_diff}"
        );
    }
}

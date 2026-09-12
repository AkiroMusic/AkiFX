//! Soft Vacuum module — Airwindows Hard Vacuum port with Lanczos3 oversampling.
//!
//! Faithfully ports the Hard Vacuum distortion algorithm (drive, warmth/sag, aura/bias)
//! with configurable up-to-16x Lanczos3 oversampling. Each channel maintains independent
//! processor and oversampler state.
//!
//! # Parameters
//!
//! - **Drive** (`#[id = "drive"]`): 0–200%, above 100% adds up to 4 distortion stages
//! - **Warmth** (`#[id = "warmth"]`): DC bias/sag, 0–100%
//! - **Aura** (`#[id = "aura"]`): Extra input gain/bias, 0–100% (mapped to `[0, π]`)
//! - **Output Gain** (`#[id = "output_gain"]`): −40 to 0 dB
//! - **Mix** (`#[id = "dry_wet_ratio"]`): Dry/wet 0–100%
//! - **Oversampling** (`#[id = "oversampling_factor"]`): 1x–16x

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ── Hard Vacuum Algorithm ───────────────────────────────────────────────

/// Presumed typo from original plugin: `pi/2` ≈ 1.5708 but original uses 1.55708.
const ALMOST_FRAC_PI_2: f32 = 1.557_079_7;

/// Parameters for the Hard Vacuum distortion algorithm.
pub struct HardVacuumParams {
    /// Drive in `[0, 2]`. Above 1.0, distortion stages multiply quadratically.
    pub drive: f32,
    /// Warmth (DC bias) in `[0, 1]`.
    pub warmth: f32,
    /// Aura (extra input gain) in `[0, π]`.
    pub aura: f32,
}

/// Single-channel Hard Vacuum processor. Maintains one `last_sample` for slew computation.
#[derive(Debug, Default)]
struct HardVacuum {
    last_sample: f32,
}

impl HardVacuum {
    fn reset(&mut self) {
        self.last_sample = 0.0;
    }

    /// Compute the slew (discrete derivative) for a sample.
    fn compute_slew(&mut self, input: f32) -> f32 {
        let skew = input - self.last_sample;
        self.last_sample = input;
        skew
    }

    /// Process with an externally computed slew. Matches source `process_with_slew()`.
    fn process_with_slew(&self, input: f32, params: &HardVacuumParams, slew: f32) -> f32 {
        let scaled_warmth = params.warmth / FRAC_PI_2;
        let inverse_warmth = 1.0 - params.warmth;

        let skew = {
            let skew = slew;
            let bridge_rectifier = skew.abs().min(PI).sin();
            skew.signum() * bridge_rectifier * params.aura * input * ALMOST_FRAC_PI_2
        };

        let mut remaining_distortion_stages = if params.drive > 1.0 {
            params.drive * params.drive
        } else {
            params.drive
        };

        let mut output = input;
        while remaining_distortion_stages > 0.0 {
            let drive = if remaining_distortion_stages > 1.0 {
                ALMOST_FRAC_PI_2
            } else {
                remaining_distortion_stages
                    * (1.0 + ((ALMOST_FRAC_PI_2 - 1.0) * inverse_warmth))
            };

            let bridge_rectifier = (output.abs() + skew).min(FRAC_PI_2).sin();
            let bridge_rectifier = bridge_rectifier.mul_add(drive, skew).min(FRAC_PI_2).sin();
            output = if output > 0.0 {
                let positive = drive - scaled_warmth;
                (output * (1.0 - positive + skew)) + (bridge_rectifier * (positive + skew))
            } else {
                let negative = drive + scaled_warmth;
                (output * (1.0 - negative + skew)) - (bridge_rectifier * (negative + skew))
            };

            remaining_distortion_stages -= 1.0;
        }

        output
    }
}

// ── Lanczos3 Oversampler ────────────────────────────────────────────────

/// Lanczos a=3 upsampling kernel (11 taps, outer zero points omitted).
const LANCZOS3_UP_KERNEL: [f32; 11] = [
    0.02431708, -0.0, -0.13509491, 0.0, 0.6079271, 1.0, 0.6079271, 0.0, -0.13509491, -0.0,
    0.02431708,
];

/// Lanczos a=3 downsampling kernel (half-amplitude for unity gain roundtrip).
const LANCZOS3_DOWN_KERNEL: [f32; 11] = [
    0.01215854, -0.0, -0.06754746, 0.0, 0.30396355, 0.5, 0.30396355, 0.0, -0.06754746, -0.0,
    0.01215854,
];

/// Kernel latency (half the kernel length).
const KERNEL_LATENCY: usize = LANCZOS3_UP_KERNEL.len() / 2;

/// Convolve a ring buffer with a kernel starting at `pos`.
fn convolve_rb(rb: &[f32], kernel: &[f32], pos: usize) -> f32 {
    let mut total = 0.0;
    let until_wrap = (rb.len() - pos).min(kernel.len());
    for (i, &k) in kernel.iter().rev().take(until_wrap).enumerate() {
        total += k * rb[pos + i];
    }
    for (i, &k) in kernel.iter().rev().skip(until_wrap).enumerate() {
        total += k * rb[i];
    }
    total
}

/// A single 2× oversampling stage with Lanczos3 half-band filter.
#[derive(Debug, Clone)]
struct Lanczos3Stage {
    oversampling_amount: usize,
    up_rb: Vec<f32>,
    up_write_pos: usize,
    additional_up_latency: usize,
    down_rb: [f32; LANCZOS3_DOWN_KERNEL.len()],
    down_write_pos: usize,
    scratch: Vec<f32>,
}

impl Lanczos3Stage {
    fn new(max_block: usize, stage_num: usize) -> Self {
        let os_amt = 2usize.pow(stage_num as u32 + 1);
        let uncomp = KERNEL_LATENCY + KERNEL_LATENCY;
        let extra = (-(uncomp as isize)).rem_euclid(os_amt as isize) as usize;
        Self {
            oversampling_amount: os_amt,
            up_rb: vec![0.0; LANCZOS3_UP_KERNEL.len() + extra],
            up_write_pos: 0,
            additional_up_latency: extra,
            down_rb: [0.0; LANCZOS3_DOWN_KERNEL.len()],
            down_write_pos: 0,
            scratch: vec![0.0; max_block * os_amt],
        }
    }

    fn reset(&mut self) {
        self.up_rb.fill(0.0);
        self.up_write_pos = 0;
        self.down_rb.fill(0.0);
        self.down_write_pos = 0;
    }

    fn effective_latency(&self) -> u32 {
        let uncomp = KERNEL_LATENCY + KERNEL_LATENCY;
        let total = uncomp + self.additional_up_latency;
        let eff = total as f32 / self.oversampling_amount as f32;
        debug_assert_eq!(eff.fract(), 0.0);
        eff as u32
    }

    fn upsample_from(&mut self, block: &[f32]) {
        let out_len = block.len() * 2;
        debug_assert!(out_len <= self.scratch.len());

        // Zero-stuff
        for (i, &s) in block.iter().enumerate() {
            self.scratch[i * 2] = s;
            self.scratch[i * 2 + 1] = 0.0;
        }

        let mut direct_read =
            (self.up_write_pos + KERNEL_LATENCY) % self.up_rb.len();
        for idx in 0..out_len {
            self.up_rb[self.up_write_pos] = self.scratch[idx];
            self.up_write_pos += 1;
            if self.up_write_pos == self.up_rb.len() {
                self.up_write_pos = 0;
            }
            direct_read += 1;
            if direct_read == self.up_rb.len() {
                direct_read = 0;
            }

            self.scratch[idx] = if idx % 2 == (KERNEL_LATENCY % 2) {
                self.up_rb[direct_read]
            } else {
                convolve_rb(&self.up_rb, &LANCZOS3_UP_KERNEL, self.up_write_pos)
            };
        }
    }

    fn downsample_to(&mut self, block: &mut [f32]) {
        let in_len = block.len() * 2;
        debug_assert!(in_len <= self.scratch.len());

        for idx in 0..in_len {
            self.down_rb[self.down_write_pos] = self.scratch[idx];
            self.down_write_pos += 1;
            if self.down_write_pos == LANCZOS3_DOWN_KERNEL.len() {
                self.down_write_pos = 0;
            }
            if idx % 2 == 0 {
                block[idx / 2] = convolve_rb(
                    &self.down_rb,
                    &LANCZOS3_DOWN_KERNEL,
                    self.down_write_pos,
                );
            }
        }
    }
}

/// Multi-stage Lanczos3 oversampler. Factor 0 = 1× (bypass), 1 = 2×, 2 = 4×, etc.
#[derive(Debug)]
struct Lanczos3Oversampler {
    stages: Vec<Lanczos3Stage>,
    latencies: Vec<u32>,
}

impl Lanczos3Oversampler {
    fn new(max_block: usize, max_factor: usize) -> Self {
        let stages: Vec<Lanczos3Stage> = (0..max_factor)
            .map(|i| Lanczos3Stage::new(max_block, i))
            .collect();
        let latencies: Vec<u32> = stages
            .iter()
            .map(|s| s.effective_latency())
            .scan(0, |acc, l| {
                *acc += l;
                Some(*acc)
            })
            .collect();
        Self { stages, latencies }
    }

    fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
    }

    fn latency(&self, factor: usize) -> u32 {
        if factor == 0 {
            0
        } else {
            self.latencies[factor - 1]
        }
    }

    fn process(&mut self, block: &mut [f32], factor: usize, f: impl FnOnce(&mut [f32])) {
        if factor == 0 {
            f(block);
            return;
        }
        debug_assert!(factor <= self.stages.len());
        let up = self.upsample(block, factor);
        f(up);
        self.downsample(block, factor);
    }

    fn upsample(&mut self, block: &[f32], factor: usize) -> &mut [f32] {
        debug_assert_ne!(factor, 0);
        debug_assert!(factor <= self.stages.len());
        self.stages[0].upsample_from(block);
        let mut prev_len = block.len() * 2;
        for to_idx in 1..factor {
            let ([.., from], [to, ..]) = self.stages.split_at_mut(to_idx) else {
                unreachable!()
            };
            to.upsample_from(&from.scratch[..prev_len]);
            prev_len *= 2;
        }
        &mut self.stages[factor - 1].scratch[..prev_len]
    }

    fn downsample(&mut self, block: &mut [f32], factor: usize) {
        debug_assert_ne!(factor, 0);
        debug_assert!(factor <= self.stages.len());
        let mut next_len = block.len() * 2usize.pow(factor as u32 - 1);
        for to_idx in (1..factor).rev() {
            let ([.., to], [from, ..]) = self.stages.split_at_mut(to_idx) else {
                unreachable!()
            };
            from.downsample_to(&mut to.scratch[..next_len]);
            next_len /= 2;
        }
        debug_assert_eq!(next_len, block.len());
        self.stages[0].downsample_to(block);
    }
}

// ── Parameters ──────────────────────────────────────────────────────────

const MAX_OVERSAMPLING_FACTOR: usize = 4; // 16×
const MAX_OVERSAMPLING_TIMES: usize = 1 << MAX_OVERSAMPLING_FACTOR;
const DEFAULT_OVERSAMPLING_FACTOR: usize = 1; // 2×
const MAX_BLOCK_SIZE: usize = 512;
const MAX_OVERSAMPLED_BLOCK: usize = MAX_BLOCK_SIZE * MAX_OVERSAMPLING_TIMES;

/// Parameters mirroring the source Soft Vacuum plugin.
#[derive(Params)]
pub struct SoftVacuumParams {
    /// Drive 0–200% (stored as `[0, 2]`).
    #[id = "drive"]
    pub drive: FloatParam,
    /// Warmth (DC bias/sag) 0–100%.
    #[id = "warmth"]
    pub warmth: FloatParam,
    /// Aura (extra input gain) 0–100%, mapped to `[0, π]`.
    #[id = "aura"]
    pub aura: FloatParam,
    /// Output gain in dB.
    #[id = "output_gain"]
    pub output_gain: FloatParam,
    /// Dry/wet mix 0–100%.
    #[id = "dry_wet_ratio"]
    pub dry_wet_ratio: FloatParam,
    /// Oversampling factor (log₂), 0 = 1×, 1 = 2×, ..., 4 = 16×.
    #[id = "oversampling_factor"]
    pub oversampling_factor: IntParam,
}

impl Default for SoftVacuumParams {
    fn default() -> Self {
        Self {
            drive: FloatParam::new("Drive", 0.0, FloatRange::Linear { min: 0.0, max: 2.0 })
                .with_unit("%")
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            warmth: FloatParam::new("Warmth", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_smoother(SmoothingStyle::Linear(10.0))
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            aura: FloatParam::new("Aura", 0.0, FloatRange::Linear { min: 0.0, max: PI })
                .with_unit("%")
                .with_smoother(SmoothingStyle::Linear(10.0))
                .with_value_to_string({
                    let f = formatters::v2s_f32_percentage(0);
                    Arc::new(move |v| f(v / PI))
                })
                .with_string_to_value({
                    let f = formatters::s2v_f32_percentage();
                    Arc::new(move |s| f(s).map(|v| v * PI))
                }),
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(0.0),
                    factor: FloatRange::gain_skew_factor(-40.0, 0.0),
                },
            )
            .with_unit(" dB")
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            dry_wet_ratio: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_smoother(SmoothingStyle::Linear(10.0))
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            oversampling_factor: IntParam::new(
                "Oversampling",
                DEFAULT_OVERSAMPLING_FACTOR as i32,
                IntRange::Linear {
                    min: 0,
                    max: MAX_OVERSAMPLING_FACTOR as i32,
                },
            )
            .with_unit("x")
            .with_value_to_string(Arc::new(|v| {
                (1usize << v as u32).to_string()
            }))
            .with_string_to_value(Arc::new(|s| {
                let t: usize = s.parse().ok()?;
                Some(t.ilog2() as i32)
            })),
        }
    }
}

/// Scratch buffers for smoothed parameter rendering during process.
struct ScratchBuffers {
    drive: Vec<f32>,
    warmth: Vec<f32>,
    aura: Vec<f32>,
    output_gain: Vec<f32>,
    dry_wet_ratio: Vec<f32>,
}

impl ScratchBuffers {
    fn new() -> Self {
        Self {
            drive: vec![0.0; MAX_OVERSAMPLED_BLOCK],
            warmth: vec![0.0; MAX_OVERSAMPLED_BLOCK],
            aura: vec![0.0; MAX_OVERSAMPLED_BLOCK],
            output_gain: vec![0.0; MAX_OVERSAMPLED_BLOCK],
            dry_wet_ratio: vec![0.0; MAX_OVERSAMPLED_BLOCK],
        }
    }
}

// ── Module ──────────────────────────────────────────────────────────────

/// Per-channel state: distortion processor + audio oversampler + slew oversampler.
struct ChannelState {
    hard_vacuum: HardVacuum,
    oversampler: Lanczos3Oversampler,
    slew_oversampler: Lanczos3Oversampler,
}

impl ChannelState {
    fn new() -> Self {
        Self {
            hard_vacuum: HardVacuum::default(),
            oversampler: Lanczos3Oversampler::new(MAX_BLOCK_SIZE, MAX_OVERSAMPLING_FACTOR),
            slew_oversampler: Lanczos3Oversampler::new(MAX_BLOCK_SIZE, MAX_OVERSAMPLING_FACTOR),
        }
    }

    fn reset(&mut self) {
        self.hard_vacuum.reset();
        self.oversampler.reset();
        self.slew_oversampler.reset();
    }
}

pub struct SoftVacuumModule {
    params: Arc<SoftVacuumParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    channels: Vec<ChannelState>,
    scratch: Box<ScratchBuffers>,
}

impl SoftVacuumModule {
    pub fn new(params: Arc<SoftVacuumParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 44100.0,
            channels: Vec::new(),
            scratch: Box::new(ScratchBuffers::new()),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(
            Arc::new(SoftVacuumParams::default()),
            Arc::new(AtomicBool::new(false)),
        )
    }
}

impl AkiFxModule for SoftVacuumModule {
    fn name(&self) -> &'static str {
        "Soft Vacuum"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // Ensure we have at least 2 channels (stereo)
        while self.channels.len() < 2 {
            self.channels.push(ChannelState::new());
        }
    }

    fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.reset();
        }
    }

    fn latency_samples(&self) -> u64 {
        let factor = self.params.oversampling_factor.value() as usize;
        if self.channels.is_empty() {
            return 0;
        }
        self.channels[0].oversampler.latency(factor) as u64
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let os_factor = self.params.oversampling_factor.value() as usize;
        let os_times = 1usize << os_factor as u32;
        let block_len = left.len().max(right.len());
        let upsampled_len = block_len * os_times;

        // Process a single channel slice with its own ChannelState
        fn process_channel(
            ch: &mut ChannelState,
            block: &mut [f32],
            os_factor: usize,
            upsampled_len: usize,
            params: &SoftVacuumParams,
            scratch: &mut ScratchBuffers,
        ) {
            let block_len = block.len();

            // Render smoothed parameters for this channel
            params
                .drive
                .smoothed
                .next_block(&mut scratch.drive, upsampled_len);
            params
                .warmth
                .smoothed
                .next_block(&mut scratch.warmth, upsampled_len);
            params
                .aura
                .smoothed
                .next_block(&mut scratch.aura, upsampled_len);
            params
                .output_gain
                .smoothed
                .next_block(&mut scratch.output_gain, upsampled_len);
            params
                .dry_wet_ratio
                .smoothed
                .next_block(&mut scratch.dry_wet_ratio, upsampled_len);

            // Compute slews at base rate, then upsample the slew signal
            let mut slews = vec![0.0f32; block_len];
            for (i, &s) in block.iter().enumerate() {
                slews[i] = ch.hard_vacuum.compute_slew(s);
            }

            // When os_factor is 0 (1x), use slews directly; otherwise upsample
            let upsampled_slews = if os_factor == 0 {
                slews
            } else {
                ch.slew_oversampler.upsample(&mut slews, os_factor).to_vec()
            };

            // Oversample the audio, apply distortion, downsample
            ch.oversampler.process(block, os_factor, |upsampled| {
                debug_assert_eq!(upsampled.len(), upsampled_len);

                for (i, (sample, &slew)) in
                    upsampled.iter_mut().zip(upsampled_slews.iter()).enumerate()
                {
                    let hv_params = HardVacuumParams {
                        drive: scratch.drive[i],
                        warmth: scratch.warmth[i],
                        aura: scratch.aura[i],
                    };
                    let distorted = ch.hard_vacuum.process_with_slew(*sample, &hv_params, slew);
                    let og = scratch.output_gain[i];
                    let dw = scratch.dry_wet_ratio[i];
                    *sample = (distorted * og * dw) + (*sample * (1.0 - dw));
                }
            });
        }

        // Ensure we have at least 2 channels
        while self.channels.len() < 2 {
            self.channels.push(ChannelState::new());
        }

        // Process left channel with channels[0]
        process_channel(
            &mut self.channels[0],
            left,
            os_factor,
            upsampled_len,
            &self.params,
            &mut self.scratch,
        );

        // Process right channel with channels[1]
        process_channel(
            &mut self.channels[1],
            right,
            os_factor,
            upsampled_len,
            &self.params,
            &mut self.scratch,
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create params with specific values (nih_plug params are set at construction).
    fn make_params(drive: f32, warmth: f32, aura: f32, os_factor: i32) -> Arc<SoftVacuumParams> {
        Arc::new(SoftVacuumParams {
            drive: FloatParam::new("Drive", drive, FloatRange::Linear { min: 0.0, max: 2.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            warmth: FloatParam::new("Warmth", warmth, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            aura: FloatParam::new("Aura", aura, FloatRange::Linear { min: 0.0, max: PI })
                .with_unit("%")
                .with_value_to_string({
                    let f = formatters::v2s_f32_percentage(0);
                    Arc::new(move |v| f(v / PI))
                })
                .with_string_to_value({
                    let f = formatters::s2v_f32_percentage();
                    Arc::new(move |s| f(s).map(|v| v * PI))
                }),
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(0.0),
                    factor: FloatRange::gain_skew_factor(-40.0, 0.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            dry_wet_ratio: FloatParam::new(
                "Mix",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            oversampling_factor: IntParam::new(
                "Oversampling",
                os_factor,
                IntRange::Linear {
                    min: 0,
                    max: MAX_OVERSAMPLING_FACTOR as i32,
                },
            )
            .with_unit("x")
            .with_value_to_string(Arc::new(|v| (1usize << v as u32).to_string()))
            .with_string_to_value(Arc::new(|s| {
                let t: usize = s.parse().ok()?;
                Some(t.ilog2() as i32)
            })),
        })
    }

    /// Helper: create module, initialize, process a buffer, return output.
    fn process_with_settings(
        drive: f32,
        warmth: f32,
        aura: f32,
        os_factor: i32,
        input: &[f32],
    ) -> Vec<f32> {
        let params = make_params(drive, warmth, aura, os_factor);
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = SoftVacuumModule::new(params, bypass);
        module.initialize(44100.0, input.len());
        module.reset();

        let mut left = input.to_vec();
        let mut right = input.to_vec();
        module.process(&mut left, &mut right);
        left
    }

    #[test]
    fn drive_zero_bias_zero_sag_zero_passthrough() {
        // Given: all parameters at zero (no distortion, no bias, no sag, no mix change)
        let input: Vec<f32> = (0..512)
            .map(|i| (i as f32 * 0.01).sin())
            .collect();
        let output = process_with_settings(0.0, 0.0, 0.0, 0, &input);

        // When: drive=0, warmth=0, aura=0 with dry_wet=1.0 (default)
        // Then: output should be near-unity (max abs diff < 0.01 after warmup)
        // The algorithm with drive=0 skips all distortion stages, output = input
        let warmup = 64; // oversampler warmup
        let max_diff: f32 = input[warmup..]
            .iter()
            .zip(output[warmup..].iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 0.01,
            "Passthrough diff too large: {max_diff}"
        );
    }

    #[test]
    fn high_drive_bounded_output() {
        // Given: high drive (200%) with some bias
        let input: Vec<f32> = (0..2048)
            .map(|i| (i as f32 * 0.1).sin() * 0.8)
            .collect();
        let output = process_with_settings(2.0, 0.5, 1.0, 0, &input);

        // Then: output bounded |sample| <= 2.0 (distortion shouldn't explode)
        let max_abs = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_abs <= 2.0, "Output unbounded: {max_abs}");
    }

    #[test]
    fn silence_stays_silent() {
        // Given: silence input
        let input = vec![0.0f32; 1024];
        let output = process_with_settings(1.0, 0.5, 1.0, 0, &input);

        // Then: output should be near-zero (algorithm with zero input produces near-zero)
        let max_abs = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs < 1e-6,
            "Silence not preserved: max_abs = {max_abs}"
        );
    }

    #[test]
    fn oversampling_makes_difference() {
        // Given: same input processed at 1x vs 4x oversampling
        let input: Vec<f32> = (0..512)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect();
        let out_1x = process_with_settings(1.5, 0.3, 0.8, 0, &input);
        let out_4x = process_with_settings(1.5, 0.3, 0.8, 2, &input);

        // Then: outputs should differ (oversampling active)
        let max_diff: f32 = out_1x
            .iter()
            .zip(out_4x.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff > 0.001,
            "Oversampling made no difference: {max_diff}"
        );

        // And: both bounded
        let max_1x = out_1x.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let max_4x = out_4x.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_1x <= 2.0, "1x output unbounded: {max_1x}");
        assert!(max_4x <= 2.0, "4x output unbounded: {max_4x}");
    }

    #[test]
    fn reset_deterministic() {
        // Given: two identical module instances with same params
        let params1 = make_params(1.0, 0.3, 0.5, 1);
        let bypass1 = Arc::new(AtomicBool::new(false));
        let mut m1 = SoftVacuumModule::new(params1, bypass1);
        m1.initialize(44100.0, 256);
        m1.reset();

        let params2 = make_params(1.0, 0.3, 0.5, 1);
        let bypass2 = Arc::new(AtomicBool::new(false));
        let mut m2 = SoftVacuumModule::new(params2, bypass2);
        m2.initialize(44100.0, 256);
        m2.reset();

        // When: same input to both
        let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut l1 = input.clone();
        let mut r1 = input.clone();
        m1.process(&mut l1, &mut r1);

        let mut l2 = input.clone();
        let mut r2 = input.clone();
        m2.process(&mut l2, &mut r2);

        // Then: identical output (deterministic)
        for (a, b) in l1.iter().zip(l2.iter()) {
            assert!(
                (a - b).abs() < 1e-10,
                "Non-deterministic: {a} vs {b}"
            );
        }
    }

    #[test]
    fn no_nan_over_noise() {
        // Given: pseudo-random noise input for 10 seconds at 44100 Hz
        let total_samples = 44100 * 10;
        let params = make_params(2.0, 0.8, PI, 2);
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = SoftVacuumModule::new(params, bypass);
        module.initialize(44100.0, 512);
        module.reset();

        // Simple LCG noise
        let mut state: u32 = 12345;
        let chunk_size = 512;
        for _ in (0..total_samples).step_by(chunk_size) {
            let mut left = Vec::with_capacity(chunk_size);
            let mut right = Vec::with_capacity(chunk_size);
            for _ in 0..chunk_size {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let sample = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                left.push(sample);
                right.push(sample);
            }
            module.process(&mut left, &mut right);

            // Then: no NaN or Inf
            for s in left.iter().chain(right.iter()) {
                assert!(s.is_finite(), "Non-finite sample: {s}");
                assert!(s.abs() <= 4.0, "Sample too large: {s}");
            }
        }
    }
}

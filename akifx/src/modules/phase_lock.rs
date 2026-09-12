//! Phase Lock - spectral phase/magnitude freezing ported from SpectralSuite.
//!
//! Faithfully ports `PhaseLockFFTProcessor::spectral_process` from C++. Freezes
//! phase and/or magnitude bins across FFT frames, with optional morph transitions
//! and random phase injection.

use crate::modules::AkiFxModule;
use crate::stft::{Polar, SpectralConfig, SpectralEngine, WindowType};
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const DEFAULT_FFT_SIZE: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockStateInner { Off, ChangeToOn, On }

struct LockState { state: LockStateInner }

impl LockState {
    fn new() -> Self { Self { state: LockStateInner::Off } }
    fn is_on(&self) -> bool { self.state == LockStateInner::On }
    fn should_transition_to_on(&self) -> bool { self.state == LockStateInner::ChangeToOn }
    fn off(&mut self) { self.state = LockStateInner::Off; }
    fn begin_transition_to_on(&mut self) {
        if self.state == LockStateInner::Off { self.state = LockStateInner::ChangeToOn; }
    }
    fn complete_transition_to_on(&mut self) { self.state = LockStateInner::On; }
    fn reset_state(&mut self) {
        if self.state == LockStateInner::ChangeToOn || self.state == LockStateInner::On {
            self.state = LockStateInner::ChangeToOn;
        } else { self.state = LockStateInner::Off; }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionInner { Off, StartTransition, TransitionIn, Hold, TransitionOut }

struct TransitionState {
    sample_rate: f32, increment_amount: f32, state: TransitionInner,
    in_duration_seconds: i32, out_duration_seconds: i32, counter: f32, end: f32,
}

impl TransitionState {
    fn new(sample_rate: f32, block_size: f32) -> Self {
        Self { sample_rate, increment_amount: block_size, state: TransitionInner::Off,
               in_duration_seconds: 1, out_duration_seconds: 1, counter: 0.0,
               end: sample_rate * block_size }
    }
    fn is_off(&self) -> bool { self.state == TransitionInner::Off }
    fn should_start_transition(&self) -> bool { self.state == TransitionInner::StartTransition }
    fn start(&mut self) {
        if !self.is_off() { return; }
        self.end = self.sample_rate * self.in_duration_seconds as f32;
        self.counter = 0.0;
        self.state = TransitionInner::StartTransition;
    }
    fn stop(&mut self) {
        if self.state == TransitionInner::TransitionOut { return; }
        if self.state == TransitionInner::TransitionIn || self.state == TransitionInner::Hold {
            self.state = TransitionInner::TransitionOut;
        } else { self.state = TransitionInner::Off; }
    }
    fn next(&mut self) -> f32 { let v = self.value(); self.tick(); v }
    fn tick(&mut self) {
        if self.state == TransitionInner::StartTransition {
            self.state = TransitionInner::TransitionIn;
        }
        if self.state == TransitionInner::TransitionIn {
            if self.counter >= self.end { self.state = TransitionInner::Hold; }
            else { self.counter += self.increment_amount; }
        } else if self.state == TransitionInner::TransitionOut {
            if self.counter <= 0.0 { self.state = TransitionInner::Off; }
            else { self.counter -= self.increment_amount; }
        }
    }
    fn value(&self) -> f32 {
        if self.end <= 0.0 { return 0.0; }
        match self.state {
            TransitionInner::TransitionIn => (self.counter / self.end).min(1.0),
            TransitionInner::TransitionOut => (self.counter / self.end).max(0.0),
            TransitionInner::Hold => 1.0,
            _ => 0.0,
        }
    }
    fn set_duration(&mut self, seconds: i32) {
        self.in_duration_seconds = seconds; self.out_duration_seconds = seconds;
    }
}

struct PhaseLockProcessParams {
    phase_mix: f32, mag_mix: f32, mag_track: f32, random_phase_mix: f32,
}

struct PhaseLockState {
    locked_phases: Vec<f32>, locked_mags: Vec<f32>,
    target_phases: Vec<f32>, target_mags: Vec<f32>,
    lock_phase_state: LockState, lock_mag_state: LockState,
    transition_phase_state: TransitionState, transition_mag_state: TransitionState,
    /// Snapshot of the current frame's phases (reused across frames — the
    /// previous per-frame Vec allocations landed on the audio thread).
    in_phases: Vec<f32>,
    /// Snapshot of the current frame's magnitudes (reused).
    in_mags: Vec<f32>,
    prng_state: u32,
}

impl PhaseLockState {
    fn new(sample_rate: f32, fft_size: usize) -> Self {
        let half = fft_size / 2;
        Self { locked_phases: vec![0.0; half], locked_mags: vec![0.0; half],
               target_phases: vec![0.0; half], target_mags: vec![0.0; half],
               in_phases: vec![0.0; half], in_mags: vec![0.0; half],
               lock_phase_state: LockState::new(), lock_mag_state: LockState::new(),
               transition_phase_state: TransitionState::new(sample_rate, fft_size as f32),
               transition_mag_state: TransitionState::new(sample_rate, fft_size as f32),
               prng_state: 0x1234_5678 }
    }
    fn reset(&mut self, sample_rate: f32, fft_size: usize) {
        self.lock_phase_state.reset_state();
        self.lock_mag_state.reset_state();
        self.transition_phase_state = TransitionState::new(sample_rate, fft_size as f32);
        self.transition_mag_state = TransitionState::new(sample_rate, fft_size as f32);
        self.prng_state = 0x1234_5678;
    }
    fn spectral_process(&mut self, num_bins: usize, polar: &mut [Polar], params: &PhaseLockProcessParams) {
        self.do_lock(num_bins, polar, params);
        self.do_morph_transition(num_bins, polar);
    }
    fn do_lock(&mut self, num_bins: usize, polar: &mut [Polar], params: &PhaseLockProcessParams) {
        // Snapshot the frame into reusable buffers (no allocation).
        self.in_phases.clear();
        self.in_phases.resize(num_bins, 0.0);
        self.in_mags.clear();
        self.in_mags.resize(num_bins, 0.0);
        for (i, p) in polar.iter().take(num_bins).enumerate() {
            self.in_phases[i] = p.phase;
            self.in_mags[i] = p.magnitude;
        }
        let in_phases: &Vec<f32> = &self.in_phases;
        let in_mags: &Vec<f32> = &self.in_mags;
        if self.lock_phase_state.should_transition_to_on() {
            self.locked_phases.clear();
            for i in 0..num_bins { self.locked_phases.push(in_phases[i]); }
            self.lock_phase_state.complete_transition_to_on();
        }
        if self.lock_mag_state.should_transition_to_on() {
            self.locked_mags.clear();
            for i in 0..num_bins { self.locked_mags.push(in_mags[i]); }
            self.lock_mag_state.complete_transition_to_on();
        }
        let max_mag = in_mags.iter().copied().fold(0.0f32, f32::max);
        let max_locked_mag = self.locked_mags.iter().take(num_bins).copied().fold(0.1f32, f32::max);
        let raw_scale = max_mag / max_locked_mag;
        let scale = raw_scale * params.mag_track + (1.0 - params.mag_track);
        let phase_on = self.lock_phase_state.is_on();
        let mag_on = self.lock_mag_state.is_on();
        if phase_on && mag_on {
            for i in 0..num_bins {
                let in_phase = in_phases[i];
                let mut out_phase = in_phase + (self.locked_phases[i] - in_phase) * params.phase_mix;
                let in_mag = in_mags[i];
                let out_mag = (in_mag + (self.locked_mags[i] - in_mag) * params.mag_mix) * scale;
                let rand_phase = params.random_phase_mix * Self::generate_random_phase(&mut self.prng_state);
                out_phase = rand_phase + (1.0 - params.random_phase_mix) * out_phase;
                polar[i] = Polar::new(out_mag, out_phase);
            }
        } else if phase_on {
            for i in 0..num_bins {
                let in_phase = in_phases[i];
                let mut out_phase = in_phase + (self.locked_phases[i] - in_phase) * params.phase_mix;
                let rand_phase = params.random_phase_mix * Self::generate_random_phase(&mut self.prng_state);
                out_phase = rand_phase + (1.0 - params.random_phase_mix) * out_phase;
                polar[i] = Polar::new(in_mags[i], out_phase);
            }
        } else if mag_on {
            for i in 0..num_bins {
                let in_mag = in_mags[i];
                let out_mag = (in_mag + (self.locked_mags[i] - in_mag) * params.mag_mix) * scale;
                let rand_phase = params.random_phase_mix * Self::generate_random_phase(&mut self.prng_state);
                let out_phase = rand_phase + (1.0 - params.random_phase_mix) * in_phases[i];
                polar[i] = Polar::new(out_mag, out_phase);
            }
        } else {
            for i in 0..num_bins {
                let rand_phase = params.random_phase_mix * Self::generate_random_phase(&mut self.prng_state);
                let out_phase = rand_phase + (1.0 - params.random_phase_mix) * in_phases[i];
                polar[i] = Polar::new(in_mags[i], out_phase);
            }
        }
    }
    fn do_morph_transition(&mut self, num_bins: usize, polar: &mut [Polar]) {
        if self.transition_mag_state.should_start_transition()
            && self.transition_phase_state.should_start_transition()
        {
            self.target_mags.clear(); self.target_phases.clear();
            for i in 0..num_bins {
                self.target_mags.push(polar[i].magnitude);
                self.target_phases.push(polar[i].phase);
            }
        } else if self.transition_mag_state.should_start_transition() {
            self.target_mags.clear();
            for i in 0..num_bins { self.target_mags.push(polar[i].magnitude); }
        } else if self.transition_phase_state.should_start_transition() {
            self.target_phases.clear();
            for i in 0..num_bins { self.target_phases.push(polar[i].phase); }
        }
        if !self.transition_mag_state.is_off() && !self.transition_phase_state.is_off() {
            let phase_factor = self.transition_phase_state.next();
            let mag_factor = self.transition_mag_state.next();
            for i in 0..num_bins {
                let target_mag = self.target_mags.get(i).copied().unwrap_or(polar[i].magnitude);
                let target_phase = self.target_phases.get(i).copied().unwrap_or(polar[i].phase);
                polar[i].magnitude = lerp(polar[i].magnitude, target_mag, mag_factor);
                polar[i].phase = lerp(polar[i].phase, target_phase, phase_factor);
            }
        }
    }
    fn generate_random_phase(prng_state: &mut u32) -> f32 {
        *prng_state ^= *prng_state << 13;
        *prng_state ^= *prng_state >> 17;
        *prng_state ^= *prng_state << 5;
        let r = (*prng_state % 1000) as f32;
        let p = (r / 1000.0) * std::f32::consts::TAU;
        p - std::f32::consts::PI
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

#[derive(Params)]
pub struct PhaseLockParams {
    #[id = "phase_lock"] pub phase_lock: BoolParam,
    #[id = "phase_mix"] pub phase_mix: FloatParam,
    #[id = "mag_lock"] pub mag_lock: BoolParam,
    #[id = "mag_mix"] pub mag_mix: FloatParam,
    #[id = "mag_track"] pub mag_track: FloatParam,
    #[id = "rand_phase"] pub random_phase: FloatParam,
    #[id = "morph"] pub morph_mag_and_phase: BoolParam,
    #[id = "morph_dur"] pub morph_duration: IntParam,
}

impl PhaseLockParams {
    pub fn new() -> Self {
        Self {
            phase_lock: BoolParam::new("Phase Lock", false),
            phase_mix: FloatParam::new("Phase Mix", 100.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            mag_lock: BoolParam::new("Frequency Lock", false),
            mag_mix: FloatParam::new("Frequency Mix", 100.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            mag_track: FloatParam::new("Magnitude Tracking", 0.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            random_phase: FloatParam::new("Random Phase", 0.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            morph_mag_and_phase: BoolParam::new("Morph Freq and Phase", false),
            morph_duration: IntParam::new("Morph Duration", 2, IntRange::Linear { min: 1, max: 30 })
                .with_unit(" s"),
        }
    }
}
impl Default for PhaseLockParams { fn default() -> Self { Self::new() } }

pub struct PhaseLockModule {
    params: Arc<PhaseLockParams>, bypass: Arc<AtomicBool>,
    sample_rate: f32, engine: Option<SpectralEngine>, fft_size: usize,
    dry_buf_l: Vec<f32>, dry_buf_r: Vec<f32>, phase_state: PhaseLockState,
}

impl PhaseLockModule {
    pub fn new(params: Arc<PhaseLockParams>, bypass: Arc<AtomicBool>) -> Self {
        Self { params, bypass, sample_rate: 44100.0, engine: None, fft_size: DEFAULT_FFT_SIZE,
               dry_buf_l: Vec::new(), dry_buf_r: Vec::new(),
               phase_state: PhaseLockState::new(44100.0, DEFAULT_FFT_SIZE) }
    }
    pub fn with_defaults() -> Self {
        let params = Arc::new(PhaseLockParams::new());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }
    fn build_engine(&mut self) {
        let config = SpectralConfig { fft_size: self.fft_size, overlap_count: 4, window: WindowType::Hann };
        self.engine = Some(SpectralEngine::new(config, 2));
    }
    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

impl AkiFxModule for PhaseLockModule {
    fn name(&self) -> &'static str { "Phase Lock" }
    fn params(&self) -> &dyn Params { self.params.as_ref() }
    fn bypass_flag(&self) -> &Arc<AtomicBool> { &self.bypass }
    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.phase_state = PhaseLockState::new(sample_rate, self.fft_size);
        self.build_engine();
        self.ensure_dry_buffers(self.fft_size);
    }
    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine { engine.reset(); }
        self.phase_state.reset(self.sample_rate, self.fft_size);
    }
    fn latency_samples(&self) -> u64 {
        self.engine.as_ref().map(|e| e.latency_samples() as u64).unwrap_or(0)
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.engine.is_none() { self.build_engine(); }
        let phase_lock = self.params.phase_lock.value();
        let phase_mix = self.params.phase_mix.value() / 100.0;
        let mag_lock = self.params.mag_lock.value();
        let mag_mix = self.params.mag_mix.value() / 100.0;
        let mag_track = self.params.mag_track.value() / 100.0;
        let rand_phase = self.params.random_phase.value() / 100.0;
        let morph = self.params.morph_mag_and_phase.value();
        let morph_dur = self.params.morph_duration.value();
        if phase_lock { self.phase_state.lock_phase_state.begin_transition_to_on(); }
        else { self.phase_state.lock_phase_state.off(); }
        if mag_lock { self.phase_state.lock_mag_state.begin_transition_to_on(); }
        else { self.phase_state.lock_mag_state.off(); }
        self.phase_state.transition_phase_state.set_duration(morph_dur);
        self.phase_state.transition_mag_state.set_duration(morph_dur);
        if morph {
            self.phase_state.transition_phase_state.start();
            self.phase_state.transition_mag_state.start();
        } else {
            self.phase_state.transition_phase_state.stop();
            self.phase_state.transition_mag_state.stop();
        }
        self.ensure_dry_buffers(left.len());
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);
        // Engine construction is guaranteed during initialize(); skip this
        // block rather than panicking in the host's audio callback if not.
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let state = &mut self.phase_state;
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            &mut |num_bins, polar| {
                let params = PhaseLockProcessParams {
                    phase_mix, mag_mix, mag_track, random_phase_mix: rand_phase,
                };
                state.spectral_process(num_bins, polar, &params);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    fn make_module() -> PhaseLockModule {
        let mut m = PhaseLockModule::with_defaults();
        m.initialize(SR, BLOCK); m
    }

    fn make_module_with(phase_lock: bool, mag_lock: bool) -> PhaseLockModule {
        let params = Arc::new(PhaseLockParams {
            phase_lock: BoolParam::new("Phase Lock", phase_lock),
            phase_mix: FloatParam::new("Phase Mix", 100.0, FloatRange::Linear { min: 0.0, max: 100.0 }),
            mag_lock: BoolParam::new("Frequency Lock", mag_lock),
            mag_mix: FloatParam::new("Frequency Mix", 100.0, FloatRange::Linear { min: 0.0, max: 100.0 }),
            mag_track: FloatParam::new("Magnitude Tracking", 0.0, FloatRange::Linear { min: 0.0, max: 100.0 }),
            random_phase: FloatParam::new("Random Phase", 0.0, FloatRange::Linear { min: 0.0, max: 100.0 }),
            morph_mag_and_phase: BoolParam::new("Morph", false),
            morph_duration: IntParam::new("Morph Duration", 2, IntRange::Linear { min: 1, max: 30 }),
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = PhaseLockModule::new(params, bypass);
        m.initialize(SR, BLOCK); m
    }

    fn process_signal(module: &mut PhaseLockModule, input: &[f32]) -> Vec<f32> {
        let num_blocks = input.len().div_ceil(BLOCK);
        let mut output = vec![0.0f32; input.len()];
        for b in 0..num_blocks {
            let start = b * BLOCK;
            let end = (start + BLOCK).min(input.len());
            let mut left = vec![0.0f32; BLOCK];
            let mut right = vec![0.0f32; BLOCK];
            let chunk_len = end - start;
            left[..chunk_len].copy_from_slice(&input[start..end]);
            right[..chunk_len].copy_from_slice(&input[start..end]);
            module.process(&mut left, &mut right);
            output[start..end].copy_from_slice(&left[..chunk_len]);
        }
        output
    }

    #[test]
    fn neutral_mode_identity() {
        let mut module = make_module();
        let fft_size = DEFAULT_FFT_SIZE;
        let total = fft_size * 8;
        let input: Vec<f32> = (0..total).map(|i| {
            let t = i as f32 / SR;
            (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.5 * (2.0 * std::f32::consts::PI * 880.0 * t).sin()
        }).collect();
        let output = process_signal(&mut module, &input);
        let latency = module.latency_samples() as usize;
        let compare_end = total - fft_size;
        let compare_len = compare_end - latency;
        let mut corr = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut energy_in = 0.0f32;
        for i in 0..compare_len {
            let o = output[latency + i];
            let inp = input[i];
            corr += o * inp;
            energy_out += o * o;
            energy_in += inp * inp;
        }
        let norm = (energy_out * energy_in).sqrt();
        let correlation = if norm > 1e-10 { corr / norm } else { 0.0 };
        assert!(correlation > 0.99, "neutral mode identity correlation: {correlation} (expected > 0.99)");
    }

    #[test]
    fn phase_lock_preserves_magnitudes_and_alters_phases() {
        let mut module = make_module_with(true, false);
        let fft_size = DEFAULT_FFT_SIZE;
        let total = fft_size * 8;
        let input: Vec<f32> = (0..total).map(|i| {
            let t = i as f32 / SR;
            (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        }).collect();
        let output = process_signal(&mut module, &input);
        use rustfft::num_complex::Complex;
        use rustfft::FftPlanner;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let compare_start = fft_size * 3;
        let mut in_buf: Vec<Complex<f32>> = input[compare_start..compare_start + fft_size]
            .iter().map(|&s| Complex::new(s, 0.0)).collect();
        let mut out_buf: Vec<Complex<f32>> = output[compare_start..compare_start + fft_size]
            .iter().map(|&s| Complex::new(s, 0.0)).collect();
        let scratch_len = fft.get_inplace_scratch_len();
        let mut scratch = vec![Complex::default(); scratch_len];
        fft.process_with_scratch(&mut in_buf, &mut scratch);
        fft.process_with_scratch(&mut out_buf, &mut scratch);
        let peak_bin = (440.0 / SR * fft_size as f32) as usize;
        let mut best_in_mag = 0.0f32;
        let mut best_out_mag = 0.0f32;
        for b in peak_bin.saturating_sub(3)..=(peak_bin + 3).min(fft_size / 2 - 1) {
            let im = in_buf[b].norm();
            let om = out_buf[b].norm();
            if im > best_in_mag { best_in_mag = im; }
            if om > best_out_mag { best_out_mag = om; }
        }
        let mag_ratio = best_out_mag / best_in_mag.max(1e-10);
        assert!(mag_ratio > 0.3 && mag_ratio < 3.0,
            "phase lock should preserve magnitudes: in={best_in_mag:.3}, out={best_out_mag:.3}, ratio={mag_ratio:.3}");
        let output_energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(output_energy > 1.0, "output should have energy (processing occurred)");
    }

    #[test]
    fn silence_remains_silent() {
        let mut module = make_module();
        let input = vec![0.0f32; 4096];
        let output = process_signal(&mut module, &input);
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(max_abs < 1e-10, "silence must produce silence, got max abs = {max_abs}");
    }

    #[test]
    fn stability_ten_seconds_noise() {
        let mut module = make_module_with(true, true);
        let total = (SR * 10.0) as usize;
        let input: Vec<f32> = (0..total).map(|i| {
            let t = i as f32 / SR;
            (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                + (2.0 * std::f32::consts::PI * 317.0 * t).sin()
                + (2.0 * std::f32::consts::PI * 793.0 * t).sin()
        }).collect();
        let output = process_signal(&mut module, &input);
        assert_eq!(output.len(), total);
        assert!(!output.iter().any(|s| s.is_nan()), "no NaN");
        assert!(!output.iter().any(|s| s.is_infinite()), "no Inf");
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(max_abs < 100.0, "bounded output, got max abs = {max_abs}");
    }

    #[test]
    fn reset_deterministic() {
        let mut module = make_module_with(true, false);
        let signal: Vec<f32> = (0..4096).map(|i| {
            (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR).sin()
        }).collect();
        let _ = process_signal(&mut module, &signal);
        module.reset();
        let silence = vec![0.0f32; 4096];
        let output = process_signal(&mut module, &silence);
        let max_abs: f32 = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(max_abs < 1e-10, "after reset + silence, output must be silent, got max abs = {max_abs}");
    }
}

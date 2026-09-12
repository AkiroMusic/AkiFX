//! Sine generator module — MIDI-controlled sine test tone.
//!
//! Generates a sine wave whose frequency follows the last MIDI note received,
//! with exponential smoothing for frequency transitions. When no voice is
//! active, the output is bit-silent.
//!
//! Ported from nih-plug's `sine` example plugin.
//!
//! # Parameters
//!
//! - **Level** (`#[id = "sine_level"]`): Output level in dB (-24..+6, default -12).
//! - **Fallback Frequency** (`#[id = "sine_freq"]`): Initial frequency in Hz
//!   (20..20000, log scale, default 440). Used to seed the frequency smoother
//!   before any MIDI note is received.
//!
//! # MIDI Behavior
//!
//! | Event        | Action                                      |
//! |--------------|---------------------------------------------|
//! | `NoteOn`     | Sets target frequency, activates voice       |
//! | `NoteOff`    | Deactivates voice → silence                  |
//! | No events    | Output is 0.0 (bit-silent)                  |

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::f64::consts::TAU;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Concrete parameter struct for the Sine Generator module.
#[derive(Params)]
pub struct SineGenParams {
    /// Output level in dB, converted to linear gain at process time.
    #[id = "sine_level"]
    pub level_db: FloatParam,

    /// Fallback frequency in Hz, used to seed the smoother before any MIDI.
    #[id = "sine_freq"]
    pub fallback_frequency: FloatParam,
}

impl SineGenParams {
    /// Create params with the given defaults.
    pub fn new(default_level_db: f32, default_freq: f32) -> Self {
        Self {
            level_db: FloatParam::new(
                "Level",
                default_level_db,
                FloatRange::Linear {
                    min: -24.0,
                    max: 6.0,
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB"),
            fallback_frequency: FloatParam::new(
                "Fallback Frequency",
                default_freq,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(10.0))
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
        }
    }
}

impl Default for SineGenParams {
    fn default() -> Self {
        Self::new(-12.0, 440.0)
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// Sine generator module — phase-accumulator oscillator driven by MIDI notes.
pub struct SineGenModule {
    params: Arc<SineGenParams>,
    bypass: Arc<AtomicBool>,
    /// Host sample rate, set during `initialize()`.
    sample_rate: f32,
    /// Phase accumulator, always in [0, 1).
    phase: f64,
    /// Whether a voice is currently active (NoteOn received, no NoteOff yet).
    /// Note gain envelope (velocity/pressure controlled, 5 ms linear) that
    /// prevents clicks on note on/off, matching the upstream sine example.
    midi_note_gain: Smoother<f32>,
    /// MIDI note ID of the currently active note.
    midi_note_id: u8,
    /// Exponential smoother for frequency transitions between notes.
    frequency_smoother: Smoother<f32>,
}

impl SineGenModule {
    /// Create a new module with shared params and bypass flag.
    pub fn new(params: Arc<SineGenParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 1.0,
            phase: 0.0,
            midi_note_gain: Smoother::new(SmoothingStyle::Linear(5.0)),
            midi_note_id: 0,
            frequency_smoother: Smoother::new(SmoothingStyle::Logarithmic(10.0)),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(SineGenParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Generate one sine sample at the current phase and advance the accumulator.
    fn calculate_sine(&mut self, frequency: f64) -> f32 {
        let sine = (self.phase * TAU).sin();
        let phase_delta = frequency / self.sample_rate as f64;
        self.phase += phase_delta;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sine as f32
    }
}

impl AkiFxModule for SineGenModule {
    fn name(&self) -> &'static str {
        "Sine Generator"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // Seed smoothers to their default plain values for standalone testing.
        self.params
            .level_db
            .smoothed
            .reset(self.params.level_db.value());
        self.frequency_smoother
            .reset(self.params.fallback_frequency.value());
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.midi_note_gain.reset(0.0);
        self.midi_note_id = 0;
        self.frequency_smoother
            .reset(self.params.fallback_frequency.value());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // No MIDI context — delegate with empty event list.
        self.process_with_midi(left, right, &[]);
    }

    fn process_with_midi(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        note_events: &[NoteEvent<()>],
    ) {
        let mut event_idx = 0;
        let num_events = note_events.len();

        for (sample_idx, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            // Drain all MIDI events whose timing ≤ current sample.
            while event_idx < num_events
                && note_events[event_idx].timing() <= sample_idx as u32
            {
                match note_events[event_idx] {
                    NoteEvent::NoteOn { note, velocity, .. } => {
                        self.midi_note_id = note;
                        let freq = util::midi_note_to_freq(note);
                        self.frequency_smoother
                            .set_target(self.sample_rate, freq);
                        self.midi_note_gain.set_target(self.sample_rate, velocity);
                    }
                    NoteEvent::NoteOff { note, .. } if note == self.midi_note_id => {
                        // Ramp to silence instead of hard-gating to avoid
                        // the release click (upstream uses the same 5 ms
                        // linear gain envelope).
                        self.midi_note_gain.set_target(self.sample_rate, 0.0);
                    }
                    NoteEvent::PolyPressure { note, pressure, .. }
                        if note == self.midi_note_id =>
                    {
                        self.midi_note_gain.set_target(self.sample_rate, pressure);
                    }
                    _ => {}
                }
                event_idx += 1;
            }

            // Always rendered: the gain envelope sits at 0 until a NoteOn,
            // and its 5 ms ramp prevents clicks on both note edges (upstream
            // model). The previous port hard-gated to 0.0, which clicked on
            // every note release.
            let freq = self.frequency_smoother.next() as f64;
            let sine = self.calculate_sine(freq);
            let level_gain = util::db_to_gain(self.params.level_db.smoothed.next());
            let sample = sine * self.midi_note_gain.next() * level_gain;

            *l = sample;
            *r = sample;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a module ready for testing at 44100 Hz with default params.
    fn make_module() -> SineGenModule {
        let mut m = SineGenModule::with_defaults();
        m.initialize(44100.0, 512);
        m
    }

    /// Create a module with a specific level_db value.
    fn make_module_with_level(level_db: f32) -> SineGenModule {
        let params = Arc::new(SineGenParams::new(level_db, 440.0));
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = SineGenModule::new(params, bypass);
        m.initialize(44100.0, 512);
        m
    }

    /// NoteOn at timing 0.
    fn note_on(note: u8) -> NoteEvent<()> {
        NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note,
            velocity: 1.0,
        }
    }

    /// NoteOff at the given timing.
    fn note_off(note: u8, timing: u32) -> NoteEvent<()> {
        NoteEvent::NoteOff {
            timing,
            voice_id: None,
            channel: 0,
            note,
            velocity: 0.0,
        }
    }

    // ---- No notes → bit-silent output ----

    #[test]
    fn silence_with_no_notes() {
        let mut module = make_module();
        let mut left = vec![1.0; 256];
        let mut right = vec![1.0; 256];

        module.process_with_midi(&mut left, &mut right, &[]);

        for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert!(
                l.abs() < f32::EPSILON,
                "left[{i}] should be 0.0, got {l}"
            );
            assert!(
                r.abs() < f32::EPSILON,
                "right[{i}] should be 0.0, got {r}"
            );
        }
    }

    // ---- Note-on A4 → zero crossings over exactly 44100 samples = ~880 ±5% ----

    #[test]
    fn zero_crossings_for_a4() {
        let mut module = make_module();
        let events = vec![note_on(69)]; // A4 = MIDI 69 = 440 Hz
        let block_size = 44100; // exactly 1 second at 44100 Hz
        let mut left = vec![0.0; block_size];
        let mut right = vec![0.0; block_size];

        module.process_with_midi(&mut left, &mut right, &events);

        // Count zero crossings (sign changes).
        let mut crossings = 0usize;
        for window in left.windows(2) {
            if (window[0] >= 0.0 && window[1] < 0.0) || (window[0] < 0.0 && window[1] >= 0.0) {
                crossings += 1;
            }
        }

        // At 440 Hz, expect 880 crossings (2 per cycle). Allow ±5%.
        let expected = 880.0f64;
        let tolerance = expected * 0.05;
        assert!(
            (crossings as f64 - expected).abs() <= tolerance,
            "Expected ~{expected} zero crossings (±5%), got {crossings}"
        );
    }

    // ---- Amplitude matches db_to_gain(level_db) within 0.01 ----

    #[test]
    fn amplitude_matches_level_db() {
        let level_db = -12.0f32;
        let mut module = make_module_with_level(level_db);

        let events = vec![note_on(69)]; // A4 = 440 Hz
        let block_size = 44100;
        let mut left = vec![0.0; block_size];
        let mut right = vec![0.0; block_size];

        module.process_with_midi(&mut left, &mut right, &events);

        let expected_gain = util::db_to_gain(level_db);
        let peak = left.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));

        assert!(
            (peak - expected_gain).abs() < 0.01,
            "Peak amplitude {peak:.4} should match db_to_gain({level_db}) = {expected_gain:.4} ± 0.01"
        );
    }

    // ---- Block-boundary continuity: max discontinuity < 0.05 ----

    #[test]
    fn block_boundary_continuity() {
        let mut module = make_module();
        let events = vec![note_on(69)]; // A4 = 440 Hz

        let block_size = 512;
        let mut left_a = vec![0.0; block_size];
        let mut right_a = vec![0.0; block_size];
        let mut left_b = vec![0.0; block_size];
        let mut right_b = vec![0.0; block_size];

        // First block: NoteOn at sample 0.
        module.process_with_midi(&mut left_a, &mut right_a, &events);

        // Second block: no events (voice stays active).
        module.process_with_midi(&mut left_b, &mut right_b, &[]);

        let discontinuity = (left_a[block_size - 1] - left_b[0]).abs();
        assert!(
            discontinuity < 0.05,
            "Block boundary discontinuity {discontinuity:.6} should be < 0.05"
        );
    }

    // ---- Note-off → silent afterwards ----

    #[test]
    fn note_off_silences_output() {
        let mut module = make_module();

        // Block 1: NoteOn at sample 0.
        let events_on = vec![note_on(69)];
        let mut left_a = vec![0.0; 256];
        let mut right_a = vec![0.0; 256];
        module.process_with_midi(&mut left_a, &mut right_a, &events_on);

        // Verify that we do get output.
        let peak_a = left_a.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
        assert!(peak_a > 0.0, "Should produce output after NoteOn");

        // Block 2: NoteOff at sample 0.
        let events_off = vec![note_off(69, 0)];
        let mut left_b = vec![1.0; 256];
        let mut right_b = vec![1.0; 256];
        module.process_with_midi(&mut left_b, &mut right_b, &events_off);

        // The anti-click envelope ramps to silence over ~5 ms (~220 samples
        // at 44.1 kHz), so the block's tail must be (near) silent rather than
        // hard-cut at the NoteOff instant (the old hard gate clicked).
        for (i, (l, r)) in left_b.iter().zip(right_b.iter()).enumerate() {
            if i < 240 {
                continue; // 5 ms release ramp (~220 samples) still decaying
            }
            assert!(
                l.abs() < 1e-3,
                "left[{i}] should have ramped to silence after NoteOff, got {l}"
            );
            assert!(
                r.abs() < 1e-3,
                "right[{i}] should have ramped to silence after NoteOff, got {r}"
            );
        }
    }

    // ---- Reset zeroes phase and voices ----

    #[test]
    fn reset_clears_state() {
        let mut module = make_module();

        // Activate a voice.
        let events = vec![note_on(69)];
        let mut left = vec![0.0; 128];
        let mut right = vec![0.0; 128];
        module.process_with_midi(&mut left, &mut right, &events);

        // Reset.
        module.reset();

        // After reset, no voice should be active → silent.
        let mut left = vec![1.0; 128];
        let mut right = vec![1.0; 128];
        module.process_with_midi(&mut left, &mut right, &[]);

        for (i, l) in left.iter().enumerate() {
            assert!(
                l.abs() < f32::EPSILON,
                "left[{i}] should be silent after reset, got {l}"
            );
        }
    }
}

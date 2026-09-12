//! Buffr Glitch module — MIDI-controlled buffer repeater.
//!
//! Faithfully ported from nih-plug's Buffr Glitch by Robbert van der Helm.
//! Continuously records incoming audio into a ring buffer; on MIDI NoteOn, it
//! captures one period corresponding to the note frequency and loops it as a
//! waveform cycle per voice (up to 8 polyphonic voices). An AR envelope shapes
//! attack/release, and a dry/wet mix blends the original signal.
//!
//! # Parameters (mirroring source)
//!
//! - **Dry Level** (`#[id = "dry_mix"]`): 0–1 (gain-skewed), default 1.0.
//! - **Velocity Sensitive** (`#[id = "velocity_sensitive"]`): bool, default false.
//! - **Octave Shift** (`#[id = "octave_shift"]`): ±2, default 0.
//! - **Attack** (`#[id = "attack_ms"]`): 0–50 ms, default 2.0 ms.
//! - **Release** (`#[id = "release_ms"]`): 0–50 ms, default 2.0 ms.
//! - **Crossfade** (`#[id = "crossfade_ms"]`): 0–50 ms, default 2.0 ms.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum octave shift for buffer sizing (matches source).
const MAX_OCTAVE_SHIFT: i32 = 2;

/// Maximum number of simultaneous voices.
const MAX_VOICES: usize = 8;

// ---------------------------------------------------------------------------
// Ring Buffer — ported from nih-plug buffr_glitch buffer.rs
// ---------------------------------------------------------------------------

/// Ring buffer that records audio until full, then loops the recorded content.
/// First pass records input; subsequent passes play back the captured period.
#[derive(Debug)]
struct RingBuffer {
    sample_rate: f32,
    /// Per-channel sample storage, resized to match note period.
    audio_buffers: Vec<Vec<f32>>,
    /// Current read/write position.
    next_sample_pos: usize,
    /// Crossfade length in samples (0 = no crossfade).
    crossfade_length: usize,
    /// Buffer state machine.
    status: BufferStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferStatus {
    Recording,
    Crossfading,
    Ready,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self {
            sample_rate: 1.0,
            audio_buffers: Vec::new(),
            next_sample_pos: 0,
            crossfade_length: 0,
            status: BufferStatus::Recording,
        }
    }
}

impl RingBuffer {
    /// Allocate storage for `num_channels` at the given sample rate.
    /// Capacity is sized for the lowest possible note (MIDI 0 shifted down by
    /// `MAX_OCTAVE_SHIFT`), rounded up to the next power of two.
    fn resize(&mut self, num_channels: usize, sample_rate: f32) {
        debug_assert!(num_channels >= 1);
        debug_assert!(sample_rate > 0.0);

        let lowest_freq = util::midi_note_to_freq(0u8) / 2.0f32.powi(MAX_OCTAVE_SHIFT);
        let lowest_period = (lowest_freq.recip() * sample_rate).ceil() as usize;
        let buffer_len = lowest_period.next_power_of_two();

        self.sample_rate = sample_rate;
        self.audio_buffers.resize(num_channels, Vec::new());
        for buf in self.audio_buffers.iter_mut() {
            buf.resize(buffer_len, 0.0);
        }
    }

    /// Whether `resize()` has run and the buffers are safe to play back.
    fn is_allocated(&self) -> bool {
        !self.audio_buffers.is_empty() && !self.audio_buffers[0].is_empty()
    }

    /// Clear playback state without deallocating the storage. Deallocating
    /// here would leave the voice unusable after a host reset (the next
    /// note-on would index an empty buffer and panic on the audio thread).
    fn reset_state(&mut self) {
        self.next_sample_pos = 0;
        self.crossfade_length = 0;
        self.status = BufferStatus::Recording;
    }

    /// Prepare for playback at the given frequency. Sets the active buffer
    /// length to one period and resets the write position for recording.
    fn prepare_playback(&mut self, frequency: f32, crossfade_ms: f32) {
        debug_assert!(frequency > 0.0);
        debug_assert!(crossfade_ms >= 0.0);

        let period_samples = (frequency.recip() * self.sample_rate).ceil() as usize;
        debug_assert!(period_samples <= self.audio_buffers[0].capacity());

        for buf in self.audio_buffers.iter_mut() {
            buf.resize(period_samples, 0.0);
        }

        self.next_sample_pos = 0;
        self.crossfade_length =
            ((crossfade_ms * self.sample_rate).ceil() as usize).min(period_samples);
        self.status = BufferStatus::Recording;
    }

    /// Read/write one sample for the given channel. Returns the output value.
    /// On the first loop iteration this records the input; afterwards it plays
    /// back the captured waveform.
    fn next_sample(&mut self, channel_idx: usize, input_sample: f32) -> f32 {
        match self.status {
            BufferStatus::Recording => {
                self.audio_buffers[channel_idx][self.next_sample_pos] = input_sample;
            }
            BufferStatus::Crossfading if self.next_sample_pos < self.crossfade_length => {
                let crossfade_t = self.next_sample_pos as f32
                    / (self.crossfade_length - 1).max(1) as f32;
                let new_t = (1.0 - crossfade_t).sqrt();
                let existing_t = crossfade_t.sqrt();
                self.audio_buffers[channel_idx][self.next_sample_pos] =
                    (input_sample * new_t)
                        + (self.audio_buffers[channel_idx][self.next_sample_pos] * existing_t);
            }
            _ => {}
        }

        let result = self.audio_buffers[channel_idx][self.next_sample_pos];

        // Advance position after the last channel is processed
        if channel_idx == self.audio_buffers.len() - 1 {
            self.next_sample_pos += 1;
            if self.next_sample_pos >= self.audio_buffers[0].len() {
                self.next_sample_pos = 0;
                self.status = match self.status {
                    BufferStatus::Recording if self.crossfade_length > 0 => {
                        BufferStatus::Crossfading
                    }
                    _ => BufferStatus::Ready,
                };
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// AR Envelope — ported from nih-plug buffr_glitch envelope.rs
// ---------------------------------------------------------------------------

/// First-order IIR attack/release envelope. Output in `[0, 1]`.
#[derive(Debug)]
struct AREnvelope {
    state: f32,
    attack_retain_t: f32,
    release_retain_t: f32,
    releasing: bool,
}

impl Default for AREnvelope {
    fn default() -> Self {
        Self {
            state: 0.0,
            attack_retain_t: 0.0,
            release_retain_t: 0.0,
            releasing: false,
        }
    }
}

impl AREnvelope {
    fn set_attack_time(&mut self, sample_rate: f32, time_ms: f32) {
        self.attack_retain_t = (-1.0 / (time_ms / 1000.0 * sample_rate)).exp();
    }

    fn set_release_time(&mut self, sample_rate: f32, time_ms: f32) {
        self.release_retain_t = (-1.0 / (time_ms / 1000.0 * sample_rate)).exp();
    }

    fn reset(&mut self) {
        self.state = 0.0;
        self.releasing = false;
    }

    fn current(&self) -> f32 {
        self.state
    }

    /// Compute the next envelope value (single sample).
    fn next_sample(&mut self) -> f32 {
        let (target, t) = if self.releasing {
            (0.0, self.release_retain_t)
        } else {
            (1.0, self.attack_retain_t)
        };
        self.state = (self.state * t) + (target * (1.0 - t));
        self.state
    }

    fn start_release(&mut self) {
        self.releasing = true;
    }

    fn is_releasing(&self) -> bool {
        self.releasing && self.state >= 0.001
    }
}

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------

/// A single polyphonic voice. Owns a ring buffer and AR envelope.
struct Voice {
    buffer: RingBuffer,
    midi_note_id: Option<u8>,
    velocity_gain: f32,
    amp_envelope: AREnvelope,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            buffer: RingBuffer::default(),
            midi_note_id: None,
            velocity_gain: 1.0,
            amp_envelope: AREnvelope::default(),
        }
    }
}

impl Voice {
    fn reset(&mut self) {
        self.buffer.reset_state();
        self.midi_note_id = None;
        self.amp_envelope.reset();
    }

    fn note_on(&mut self, params: &BuffrGlitchParams, note: u8, velocity: f32) {
        if !self.buffer.is_allocated() {
            // No storage yet (host reset before initialize()): ignore the
            // note instead of indexing an unallocated buffer.
            return;
        }
        self.midi_note_id = Some(note);
        self.velocity_gain = if params.velocity_sensitive.value() {
            velocity / (100.0 / 127.0)
        } else {
            1.0
        };
        self.amp_envelope.reset();

        let note_frequency = util::midi_note_to_freq(note)
            * 2.0f32.powi(params.octave_shift.value());
        self.buffer
            .prepare_playback(note_frequency, params.crossfade_ms.value());
    }

    fn note_off(&mut self) {
        self.amp_envelope.start_release();
        self.midi_note_id = None;
    }

    fn is_active(&self) -> bool {
        self.midi_note_id.is_some() || self.amp_envelope.is_releasing()
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Concrete parameter struct for the Buffr Glitch module.
#[derive(Params)]
pub struct BuffrGlitchParams {
    /// Dry signal mix level (0–1, gain-skewed). Defaults to 1.0 (full dry).
    #[id = "dry_mix"]
    pub dry_level: FloatParam,
    /// Velocity-sensitive mode.
    #[id = "velocity_sensitive"]
    pub velocity_sensitive: BoolParam,
    /// Octave shift for the captured period (±2).
    #[id = "octave_shift"]
    pub octave_shift: IntParam,
    /// Attack time in milliseconds.
    #[id = "attack_ms"]
    pub attack_ms: FloatParam,
    /// Release time in milliseconds.
    #[id = "release_ms"]
    pub release_ms: FloatParam,
    /// Crossfade length in milliseconds.
    #[id = "crossfade_ms"]
    pub crossfade_ms: FloatParam,
}

impl Default for BuffrGlitchParams {
    fn default() -> Self {
        Self {
            dry_level: FloatParam::new(
                "Dry Level",
                1.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 1.0,
                    factor: FloatRange::gain_skew_factor(util::MINUS_INFINITY_DB, 0.0),
                },
            )
            .with_smoother(SmoothingStyle::Exponential(10.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            velocity_sensitive: BoolParam::new("Velocity Sensitive", false),
            octave_shift: IntParam::new(
                "Octave Shift",
                0,
                IntRange::Linear {
                    min: -MAX_OCTAVE_SHIFT,
                    max: MAX_OCTAVE_SHIFT,
                },
            ),
            attack_ms: FloatParam::new(
                "Attack",
                2.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 50.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.001),
            release_ms: FloatParam::new(
                "Release",
                2.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 50.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.001),
            crossfade_ms: FloatParam::new(
                "Crossfade",
                2.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 50.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_unit(" ms")
            .with_step_size(0.001),
        }
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// Buffr Glitch module — MIDI-controlled buffer repeater with polyphonic voice
/// allocation, ring-buffer looping, and AR envelope shaping.
pub struct BuffrGlitchModule {
    params: Arc<BuffrGlitchParams>,
    bypass: Arc<AtomicBool>,
    sample_rate: f32,
    voices: Vec<Voice>,
}

impl BuffrGlitchModule {
    /// Create a new module with shared params and bypass flag.
    pub fn new(params: Arc<BuffrGlitchParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 1.0,
            voices: (0..MAX_VOICES).map(|_| Voice::default()).collect(),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(BuffrGlitchParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Find the voice slot for a new note: prefer unused voices, then the
    /// quietest releasing voice, then the quietest overall.
    fn alloc_voice(&self) -> usize {
        // Prefer an inactive voice
        for (i, v) in self.voices.iter().enumerate() {
            if !v.is_active() {
                return i;
            }
        }
        // Prefer the quietest releasing voice
        let mut best = 0;
        let mut best_amp = f32::INFINITY;
        for (i, v) in self.voices.iter().enumerate() {
            if v.amp_envelope.is_releasing() {
                let amp = v.amp_envelope.current();
                if amp < best_amp {
                    best_amp = amp;
                    best = i;
                }
            }
        }
        if best_amp < f32::INFINITY {
            return best;
        }
        // Fall back to quietest voice overall
        let mut best = 0;
        let mut best_amp = f32::INFINITY;
        for (i, v) in self.voices.iter().enumerate() {
            let amp = v.amp_envelope.current();
            if amp < best_amp {
                best_amp = amp;
                best = i;
            }
        }
        best
    }
}

impl AkiFxModule for BuffrGlitchModule {
    fn name(&self) -> &'static str {
        "Buffr Glitch"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        for voice in &mut self.voices {
            voice.buffer.resize(2, sample_rate);
        }
        // Seed the dry-level smoother so non-host contexts (tests,
        // standalone) don't ramp from silence.
        self.params
            .dry_level
            .smoothed
            .reset(self.params.dry_level.value());
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_with_midi(left, right, &[]);
    }

    fn process_with_midi(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        note_events: &[NoteEvent<()>],
    ) {
        let num_samples = left.len();
        debug_assert_eq!(left.len(), right.len());

        // --- Sort events by timing and build a cursor ---
        // We process sample-by-sample, applying events at their timing offset.
        let mut event_idx = 0;

        // Cache param values (read once per block, matching source behaviour).
        // dry_level is smoothed per sample below (its declared exponential
        // smoother used to be ignored, making dry/wet changes step abruptly).
        let attack_ms = self.params.attack_ms.value();
        let release_ms = self.params.release_ms.value();

        // The envelope coefficients only depend on block-level values, so set
        // them once per block instead of once per sample per voice (exp() is
        // not free, and voices can be up to MAX_POLYPHON).
        for voice in &mut self.voices {
            voice
                .amp_envelope
                .set_attack_time(self.sample_rate, attack_ms);
            voice
                .amp_envelope
                .set_release_time(self.sample_rate, release_ms);
        }

        for sample_idx in 0..num_samples {
            // --- Process any events at this sample offset ---
            while event_idx < note_events.len() {
                let ev = &note_events[event_idx];
                if ev.timing() as usize > sample_idx {
                    break;
                }
                match ev {
                    NoteEvent::NoteOn {
                        note, velocity, ..
                    } => {
                        let vid = self.alloc_voice();
                        self.voices[vid].note_on(&self.params, *note, *velocity);
                    }
                    NoteEvent::NoteOff { note, .. } => {
                        for voice in &mut self.voices {
                            if voice.midi_note_id == Some(*note) {
                                voice.note_off();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                event_idx += 1;
            }

            // --- Read input before voices overwrite the buffer ---
            let in_l = left[sample_idx];
            let in_r = right[sample_idx];

            // --- Clear output and sum active voices ---
            let mut out_l = 0.0f32;
            let mut out_r = 0.0f32;
            let mut max_envelope = 0.0f32;

            for voice in &mut self.voices {
                if !voice.is_active() {
                    continue;
                }
                let env = voice.amp_envelope.next_sample();
                let amp = voice.velocity_gain * env;
                max_envelope = max_envelope.max(env);

                out_l += voice.buffer.next_sample(0, in_l) * amp;
                out_r += voice.buffer.next_sample(1, in_r) * amp;
            }

            // --- Mix dry signal (ducked by active voice envelope) ---
            let dry_gain = (1.0 - max_envelope) * self.params.dry_level.smoothed.next();
            left[sample_idx] = out_l + in_l * dry_gain;
            right[sample_idx] = out_r + in_r * dry_gain;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 512;

    /// Generate a sine wave at the given frequency.
    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect()
    }

    /// Create a NoteOn event.
    fn note_on_event(note: u8, timing: u32) -> NoteEvent<()> {
        NoteEvent::NoteOn {
            timing,
            voice_id: None,
            channel: 0,
            note,
            velocity: 1.0,
        }
    }

    /// Create a NoteOff event.
    fn note_off_event(note: u8, timing: u32) -> NoteEvent<()> {
        NoteEvent::NoteOff {
            timing,
            voice_id: None,
            channel: 0,
            note,
            velocity: 0.0,
        }
    }

    /// Helper: create and initialise a module ready for testing.
    fn make_module() -> BuffrGlitchModule {
        let mut m = BuffrGlitchModule::with_defaults();
        m.initialize(SR, BLOCK);
        m
    }

    // ---- Test 1: No notes → passthrough (dry path, bit-comparable) ----

    #[test]
    fn no_notes_passthrough() {
        let mut module = make_module();
        let n = 256;
        let left = sine(440.0, n);
        let right = sine(440.0, n);
        let left_expected = left.clone();
        let right_expected = right.clone();
        let mut left_out = left;
        let mut right_out = right;

        module.process_with_midi(&mut left_out, &mut right_out, &[]);

        // With default dry_level=1.0 and no active voices, output must equal input
        for i in 0..n {
            assert!(
                (left_out[i] - left_expected[i]).abs() < 1e-6,
                "left sample {i}: expected {}, got {}",
                left_expected[i],
                left_out[i]
            );
            assert!(
                (right_out[i] - right_expected[i]).abs() < 1e-6,
                "right sample {i}: expected {}, got {}",
                right_expected[i],
                right_out[i]
            );
        }
    }

    // ---- Test 2: Note-on captures a loop whose fundamental repeats at the
    //              note frequency (zero-crossing check) ----

    #[test]
    fn note_on_captures_loop_at_correct_frequency() {
        let mut module = make_module();
        let freq = 220.0_f32;
        // MIDI note for 220 Hz: 57 (A3)
        let midi_note = 57u8;
        let period_samples = (SR / freq).round() as usize; // ~200

        // We need enough samples: fill the buffer (one period) then loop for
        // at least a few more periods to check zero crossings.
        let warmup = period_samples; // first period is recorded
        let check_len = period_samples * 4; // check 4 looped periods
        let total = warmup + check_len;
        let input = sine(freq, total);

        let events = vec![note_on_event(midi_note, 0)];
        let mut left = input.clone();
        let mut right = input.clone();

        module.process_with_midi(&mut left, &mut right, &events);

        // After warmup, the output should be the looped buffer * envelope.
        // The envelope is still rising (attack=2ms ≈ 88 samples at 44100),
        // so by sample 200 it's close to 1.0. Check zero crossings in the
        // looped region to verify the fundamental.
        let check_start = warmup + 10; // skip a few samples for envelope ramp
        let check_end = (check_start + period_samples * 3).min(total);

        // Count zero crossings (sign changes) in the left channel
        let mut crossings = 0usize;
        for i in (check_start + 1)..check_end {
            if (left[i - 1] >= 0.0 && left[i] < 0.0)
                || (left[i - 1] < 0.0 && left[i] >= 0.0)
            {
                crossings += 1;
            }
        }

        // A pure sine at frequency f has 2 zero crossings per period.
        // Over 3 periods we expect 6 crossings (give or take 1 for edge effects).
        let expected_crossings = 6;
        assert!(
            crossings >= expected_crossings - 1 && crossings <= expected_crossings + 1,
            "expected ~{expected_crossings} zero crossings for {freq} Hz over 3 periods, got {crossings}"
        );
    }

    // ---- Test 3: Two overlapping notes create two voices ----

    #[test]
    fn two_notes_create_two_voices() {
        let mut module = make_module();

        // Send two NoteOn events at different notes
        let events = vec![
            note_on_event(60, 0), // C4
            note_on_event(64, 0), // E4
        ];
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];

        module.process_with_midi(&mut left, &mut right, &events);

        // Count active voices
        let active = module.voices.iter().filter(|v| v.is_active()).count();
        assert_eq!(active, 2, "expected 2 active voices after 2 NoteOns");
    }

    // ---- Test 4: 9th note steals oldest (max 8 voices) ----

    #[test]
    fn ninth_note_steals_oldest_voice() {
        let mut module = make_module();

        // Fill all 8 voices
        let mut events: Vec<NoteEvent<()>> = (0..8u8)
            .map(|i| note_on_event(48 + i, 0))
            .collect();
        let mut left = vec![0.0; 64];
        let mut right = vec![0.0; 64];
        module.process_with_midi(&mut left, &mut right, &events);

        let active_before = module.voices.iter().filter(|v| v.is_active()).count();
        assert_eq!(active_before, 8, "should have 8 voices after 8 NoteOns");

        // Send a 9th note
        events = vec![note_on_event(72, 0)];
        module.process_with_midi(&mut left, &mut right, &events);

        let active_after = module.voices.iter().filter(|v| v.is_active()).count();
        assert_eq!(active_after, 8, "should still have exactly 8 voices");

        // The 9th note should be assigned to one of the voices
        let has_note_72 = module
            .voices
            .iter()
            .any(|v| v.midi_note_id == Some(72));
        assert!(has_note_72, "one voice should now hold note 72");
    }

    // ---- Test 5: Release fades to silence without clicks ----

    #[test]
    fn release_fades_without_clicks() {
        let mut module = make_module();
        let freq = 440.0_f32;
        let midi_note = 69u8; // A4 = 440 Hz
        let period_samples = (SR / freq).round() as usize;

        // Record a full period then loop for a bit
        let record_and_loop = period_samples * 3;
        let input = sine(freq, record_and_loop + 2048);

        let mut left = input.clone();
        let mut right = input.clone();

        // NoteOn at sample 0
        let events_on = vec![note_on_event(midi_note, 0)];
        module.process_with_midi(&mut left, &mut right, &events_on);

        // Now send NoteOff after the buffer has looped a few times
        let note_off_sample = record_and_loop as u32;
        let mut left2 = vec![0.0; 2048];
        let mut right2 = vec![0.0; 2048];
        let events_off = vec![note_off_event(midi_note, note_off_sample)];
        module.process_with_midi(&mut left2, &mut right2, &events_off);

        // After the release, check for smooth fade (no discontinuity > 0.1
        // between consecutive samples)
        let release_region = &left2;
        let mut max_discontinuity = 0.0f32;
        for i in 1..release_region.len() {
            let diff = (release_region[i] - release_region[i - 1]).abs();
            if diff > max_discontinuity {
                max_discontinuity = diff;
            }
        }

        // Wait for the envelope to actually be releasing — the release starts
        // at sample `note_off_sample` but our output slice starts at 0, so
        // only samples after the release onset matter. We check the tail.
        // The release region for the second process call is effectively all of
        // left2 since the NoteOff is at timing 0 relative to this call.
        // Actually, the events_off timing is relative to this process call, so
        // the note-off fires at sample 0 of left2.
        // After release completes, the envelope should fade smoothly.
        // With release_ms=2.0, the fade takes ~88 samples. Check the tail
        // (after ~200 samples) for the actual fade.
        let fade_start = 88; // after release has had time to act
        let fade_end = release_region.len();
        let mut max_fade_discontinuity = 0.0f32;
        for i in (fade_start + 1)..fade_end {
            let diff =
                (release_region[i] - release_region[i - 1]).abs();
            if diff > max_fade_discontinuity {
                max_fade_discontinuity = diff;
            }
        }

        assert!(
            max_fade_discontinuity < 0.1,
            "release fade has click: max consecutive sample diff = {max_fade_discontinuity:.6}"
        );

        // Also verify the output eventually reaches near-silence
        let tail_start = fade_end - 200;
        let tail_rms: f32 = (release_region[tail_start..]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / 200.0)
            .sqrt();
        assert!(
            tail_rms < 0.01,
            "output should be near-silence after release, got RMS={tail_rms:.6}"
        );
    }

    // ---- Test 6: State reset clears voices ----

    #[test]
    fn reset_clears_all_voices() {
        let mut module = make_module();

        // Activate a voice
        let events = vec![note_on_event(60, 0)];
        let mut left = vec![0.0; 64];
        let mut right = vec![0.0; 64];
        module.process_with_midi(&mut left, &mut right, &events);

        assert!(
            module.voices.iter().any(|v| v.is_active()),
            "should have an active voice before reset"
        );

        module.reset();

        let active = module.voices.iter().filter(|v| v.is_active()).count();
        assert_eq!(active, 0, "no voices should be active after reset");
    }
}

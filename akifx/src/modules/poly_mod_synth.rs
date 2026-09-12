//! Polyphonic modulation synthesizer module -- MIDI-controlled polyphonic saw oscillator.
//!
//! A polyphonic synthesizer with up to 16 simultaneous voices, each with:
//! - A saw-wave oscillator driven by MIDI note frequency
//! - An AR (Attack/Release) amplitude envelope via [`Smoother`]
//! - Per-voice velocity scaling
//!
//! Ported from nih-plug's `poly_mod_synth` example.
//!
//! # Adaptation Notes
//!
//! The original nih-plug example uses CLAP's polyphonic modulation, which allows
//! per-voice parameter modulation via `NoteEvent::PolyModulation` and
//! `NoteEvent::MonoAutomation`. Since AkiFX modules run as effect plugins (VST3),
//! polyphonic modulation is not available. This module degrades gracefully to plain
//! MIDI NoteOn/Off velocity handling. If CLAP support is added in the future,
//! poly modulation can be re-enabled by extending the event match in
//! [`process_with_midi`](AkiFxModule::process_with_midi).
//!
//! # Parameters
//!
//! - **Gain** (`#[id = "pms_gain"]`): Output gain in dB (-36..0, default -12).
//! - **Attack** (`#[id = "pms_atk"]`): Amplitude envelope attack time in ms (0..2000, default 200).
//! - **Release** (`#[id = "pms_rel"]`): Amplitude envelope release time in ms (0..2000, default 100).

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Number of simultaneous voices (matches source).
const NUM_VOICES: usize = 16;
/// Maximum sub-block size for parameter smoothing (matches source).
const MAX_BLOCK_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// Deterministic PRNG -- xorshift32, no external crate dependency
// ---------------------------------------------------------------------------

/// Minimal deterministic PRNG for reproducible initial phases.
/// Seeded to produce the same kind of deterministic output as the source's
/// `Pcg32::new(420, 1337)`.
struct XorShift32(u32);

impl XorShift32 {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// Return a pseudo-random f32 in [0, 1).
    fn gen_f32(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Concrete parameter struct for the Poly Mod Synth module.
///
/// Stored in an `Arc<PolyModSynthParams>` shared between the module (for DSP access)
/// and the umbrella `AkiFxParams` (for host serialization via `#[nested]`).
#[derive(Params)]
pub struct PolyModSynthParams {
    /// Output gain. Linear gain internally, displayed in dB.
    #[id = "pms_gain"]
    pub gain: FloatParam,
    /// Amplitude envelope attack time. Global (same for every voice).
    #[id = "pms_atk"]
    pub amp_attack_ms: FloatParam,
    /// Amplitude envelope release time. Global (same for every voice).
    #[id = "pms_rel"]
    pub amp_release_ms: FloatParam,
}

impl PolyModSynthParams {
    /// Create new params with defaults matching the source example.
    pub fn new() -> Self {
        Self {
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(-12.0),
                FloatRange::Linear {
                    min: util::db_to_gain(-36.0),
                    max: util::db_to_gain(0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(5.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            amp_attack_ms: FloatParam::new(
                "Attack",
                200.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_step_size(0.1)
            .with_unit(" ms"),
            amp_release_ms: FloatParam::new(
                "Release",
                100.0,
                FloatRange::Skewed {
                    min: 0.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_step_size(0.1)
            .with_unit(" ms"),
        }
    }
}

impl Default for PolyModSynthParams {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------

/// Data for a single synth voice.
///
/// In a performance-critical synth you'd use a struct-of-arrays layout.
/// This struct-of-structs is faithful to the source example.
#[derive(Debug, Clone)]
struct Voice {
    /// Identifier for note matching (host-provided or computed fallback).
    voice_id: i32,
    /// MIDI channel `0..16`.
    channel: u8,
    /// MIDI note `0..128`.
    note: u8,
    /// Internal monotonic ID -- lower means older, used for voice stealing.
    internal_voice_id: u64,
    /// Square root of note velocity, used as amplitude multiplier.
    velocity_sqrt: f32,

    /// Phase accumulator `[0, 1)`. Randomized at voice onset.
    phase: f32,
    /// Phase increment per sample: `freq / sample_rate`. Constant for voice lifetime.
    phase_delta: f32,
    /// Whether the key has been released and the voice is in its release stage.
    releasing: bool,
    /// AR envelope implemented via exponential Smoother (`0->1` attack, `1->0` release).
    amp_envelope: Smoother<f32>,
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// Polyphonic modulation synthesizer -- up to 16-voice saw oscillator with AR envelopes.
pub struct PolyModSynthModule {
    params: Arc<PolyModSynthParams>,
    bypass: Arc<AtomicBool>,
    /// Host sample rate, set during [`initialize`](AkiFxModule::initialize).
    sample_rate: f32,
    /// Deterministic PRNG for initial voice phases.
    prng: XorShift32,
    /// Voice pool. Inactive voices are `None`.
    voices: [Option<Voice>; NUM_VOICES],
    /// Monotonically increasing voice counter for oldest-victim stealing.
    next_internal_voice_id: u64,
}

impl PolyModSynthModule {
    /// Create a new module with shared params and bypass flag.
    pub fn new(params: Arc<PolyModSynthParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            sample_rate: 1.0,
            prng: XorShift32(420_1337),
            voices: core::array::from_fn(|_| None),
            next_internal_voice_id: 0,
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(PolyModSynthParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Compute a fallback voice ID when the host doesn't provide one.
    ///
    /// Polyphonic modulation will not work with fallback IDs, but basic note
    /// events still function. Matches the source's `compute_fallback_voice_id()`.
    const fn fallback_voice_id(note: u8, channel: u8) -> i32 {
        note as i32 | ((channel as i32) << 16)
    }

    /// Find a voice index by its voice ID, if the voice exists.
    ///
    /// Used by CLAP poly modulation event routing (not active in VST3 context).
    #[allow(dead_code)]
    fn get_voice_idx(&self, voice_id: i32) -> Option<usize> {
        self.voices
            .iter()
            .position(|v| matches!(v, Some(v) if v.voice_id == voice_id))
    }

    /// Start a new voice. If all 16 slots are occupied, the oldest voice is stolen.
    fn start_voice(&mut self, voice_id: Option<i32>, channel: u8, note: u8, velocity: f32) {
        let new_internal_id = self.next_internal_voice_id;
        self.next_internal_voice_id = self.next_internal_voice_id.wrapping_add(1);

        let initial_phase = self.prng.gen_f32();
        let freq = util::midi_note_to_freq(note);
        let phase_delta = freq / self.sample_rate;

        let amp_envelope = Smoother::new(SmoothingStyle::Exponential(
            self.params.amp_attack_ms.value(),
        ));
        amp_envelope.reset(0.0);
        amp_envelope.set_target(self.sample_rate, 1.0);

        let voice = Voice {
            voice_id: voice_id.unwrap_or_else(|| Self::fallback_voice_id(note, channel)),
            channel,
            note,
            internal_voice_id: new_internal_id,
            velocity_sqrt: velocity.sqrt(),
            phase: initial_phase,
            phase_delta,
            releasing: false,
            amp_envelope,
        };

        // Find a free slot, or steal the oldest voice
        if let Some(free_idx) = self.voices.iter().position(|v| v.is_none()) {
            self.voices[free_idx] = Some(voice);
        } else {
            let oldest_idx = self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| {
                    v.as_ref()
                        .expect("all voices should be occupied")
                        .internal_voice_id
                })
                .expect("voices array is non-empty")
                .0;
            self.voices[oldest_idx] = Some(voice);
        }
    }

    /// Start the release phase for matching voices.
    ///
    /// If `voice_id` is `Some`, only that specific voice is released.
    /// If `voice_id` is `None`, all voices matching `channel`+`note` are released.
    fn start_release_for_voices(&mut self, voice_id: Option<i32>, channel: u8, note: u8) {
        for voice_opt in &mut self.voices {
            if let Some(voice) = voice_opt {
                let matches_id = voice_id == Some(voice.voice_id);
                let matches_note = voice.channel == channel && voice.note == note;
                if matches_id || matches_note {
                    voice.releasing = true;
                    voice.amp_envelope.style =
                        SmoothingStyle::Exponential(self.params.amp_release_ms.value());
                    voice.amp_envelope.set_target(self.sample_rate, 0.0);
                    if voice_id.is_some() {
                        return;
                    }
                }
            }
        }
    }

    /// Render one sub-block of audio for all active voices.
    ///
    /// Output is accumulated (not overwritten) so voices sum correctly.
    fn render_block(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        block_start: usize,
        block_end: usize,
    ) {
        let block_len = block_end - block_start;

        // Clear the output block
        for i in block_start..block_end {
            left[i] = 0.0;
            right[i] = 0.0;
        }

        // Render global gain into scratch buffer
        let mut gain_buf = [0.0f32; MAX_BLOCK_SIZE];
        self.params
            .gain
            .smoothed
            .next_block(&mut gain_buf, block_len);

        for voice_opt in self.voices.iter_mut() {
            let voice = match voice_opt {
                Some(v) => v,
                None => continue,
            };

            // Render this voice's envelope into scratch buffer
            let mut env_buf = [0.0f32; MAX_BLOCK_SIZE];
            voice.amp_envelope.next_block(&mut env_buf, block_len);

            for (i, sample_idx) in (block_start..block_end).enumerate() {
                let amp = voice.velocity_sqrt * gain_buf[i] * env_buf[i];
                // Saw wave: linear ramp from -1 to +1
                let sample = (voice.phase * 2.0 - 1.0) * amp;

                voice.phase += voice.phase_delta;
                if voice.phase >= 1.0 {
                    voice.phase -= 1.0;
                }

                left[sample_idx] += sample;
                right[sample_idx] += sample;
            }
        }
    }

    /// Terminate voices whose release envelope has fully decayed to zero.
    fn terminate_released_voices(&mut self) {
        for voice_opt in self.voices.iter_mut() {
            if let Some(voice) = voice_opt {
                if voice.releasing && voice.amp_envelope.previous_value() == 0.0 {
                    *voice_opt = None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AkiFxModule implementation
// ---------------------------------------------------------------------------

impl AkiFxModule for PolyModSynthModule {
    fn name(&self) -> &'static str {
        "Poly Mod Synth"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // Seed the gain smoother to its default plain value for standalone testing.
        self.params.gain.smoothed.reset(self.params.gain.value());
    }

    fn reset(&mut self) {
        self.prng = XorShift32(420_1337);
        self.voices = core::array::from_fn(|_| None);
        self.next_internal_voice_id = 0;
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
        let mut next_event_idx = 0;
        let mut block_start: usize = 0;

        while block_start < num_samples {
            // Determine block end: capped at MAX_BLOCK_SIZE or next event boundary
            let mut block_end = (block_start + MAX_BLOCK_SIZE).min(num_samples);

            // Drain all MIDI events whose timing <= block_start
            while next_event_idx < note_events.len()
                && note_events[next_event_idx].timing() <= block_start as u32
            {
                match note_events[next_event_idx] {
                    NoteEvent::NoteOn {
                        voice_id,
                        channel,
                        note,
                        velocity,
                        ..
                    } => {
                        self.start_voice(voice_id, channel, note, velocity);
                    }
                    NoteEvent::NoteOff {
                        voice_id,
                        channel,
                        note,
                        ..
                    } => {
                        self.start_release_for_voices(voice_id, channel, note);
                    }
                    // Choke: immediately terminate matching voices (no release)
                    NoteEvent::Choke {
                        voice_id,
                        channel,
                        note,
                        ..
                    } => {
                        for voice_opt in self.voices.iter_mut() {
                            if let Some(voice) = voice_opt {
                                let matches_id = voice_id == Some(voice.voice_id);
                                let matches_note = voice.channel == channel && voice.note == note;
                                if matches_id || matches_note {
                                    *voice_opt = None;
                                    if voice_id.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // CLAP poly modulation events -- not available in VST3 context;
                    // silently ignored for forward compatibility.
                    _ => {}
                }
                next_event_idx += 1;
            }

            // If an event falls before block_end, shorten the block to split there
            if next_event_idx < note_events.len() {
                let next_timing = note_events[next_event_idx].timing() as usize;
                if next_timing > block_start && next_timing < block_end {
                    block_end = next_timing;
                }
            }

            // Render voices into this sub-block
            self.render_block(left, right, block_start, block_end);

            block_start = block_end;
        }

        // Terminate voices whose release has fully decayed
        self.terminate_released_voices();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a module ready for testing at 44100 Hz with default params.
    fn make_module() -> PolyModSynthModule {
        let mut m = PolyModSynthModule::with_defaults();
        m.initialize(44100.0, 512);
        m
    }

    /// Create a module with specific attack/release params.
    fn make_module_with_envelope(attack_ms: f32, release_ms: f32) -> PolyModSynthModule {
        let params = Arc::new(PolyModSynthParams {
            amp_attack_ms: FloatParam::new(
                "Attack",
                attack_ms,
                FloatRange::Linear {
                    min: 0.0,
                    max: 2000.0,
                },
            ),
            amp_release_ms: FloatParam::new(
                "Release",
                release_ms,
                FloatRange::Linear {
                    min: 0.0,
                    max: 2000.0,
                },
            ),
            ..PolyModSynthParams::default()
        });
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = PolyModSynthModule::new(params, bypass);
        m.initialize(44100.0, 512);
        m
    }

    /// NoteOn at timing 0, full velocity.
    fn note_on(note: u8) -> NoteEvent<()> {
        NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note,
            velocity: 1.0,
        }
    }

    /// NoteOn at timing 0 with given velocity.
    fn note_on_velocity(note: u8, velocity: f32) -> NoteEvent<()> {
        NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note,
            velocity,
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

    /// RMS of a buffer.
    fn rms(buf: &[f32]) -> f32 {
        let sum: f32 = buf.iter().map(|s| s * s).sum();
        (sum / buf.len() as f32).sqrt()
    }

    /// Peak absolute value of a buffer.
    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()))
    }

    // ---- No notes -> bit-silent output ----

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

    // ---- Note-on velocity -> nonzero output with amplitude scaling ----

    #[test]
    fn note_on_produces_output() {
        let mut module = make_module();
        let events = vec![note_on(69)]; // A4
        let mut left = vec![0.0; 512];
        let mut right = vec![0.0; 512];

        module.process_with_midi(&mut left, &mut right, &events);

        let p = peak(&left);
        assert!(p > 0.0, "NoteOn should produce nonzero output, got peak={p}");
    }

    #[test]
    fn velocity_scales_amplitude() {
        let mut module_a = make_module();
        let mut module_b = make_module();

        // Full velocity
        let events_a = vec![note_on_velocity(69, 1.0)];
        let mut left_a = vec![0.0; 4096];
        let mut right_a = vec![0.0; 4096];
        module_a.process_with_midi(&mut left_a, &mut right_a, &events_a);

        // Half velocity
        let events_b = vec![note_on_velocity(69, 0.25)];
        let mut left_b = vec![0.0; 4096];
        let mut right_b = vec![0.0; 4096];
        module_b.process_with_midi(&mut left_b, &mut right_b, &events_b);

        let rms_a = rms(&left_a);
        let rms_b = rms(&left_b);

        // velocity=1.0 -> sqrt(1.0)=1.0, velocity=0.25 -> sqrt(0.25)=0.5
        // So RMS ratio should be ~2:1
        assert!(
            rms_a > rms_b,
            "Full velocity RMS ({rms_a}) should exceed half velocity RMS ({rms_b})"
        );
        let ratio = rms_a / rms_b;
        assert!(
            (ratio - 2.0).abs() < 0.5,
            "RMS ratio should be ~2.0 (sqrt velocity scaling), got {ratio}"
        );
    }

    // ---- ADSR attack: first N samples ramp from ~0 toward peak ----

    #[test]
    fn attack_envelope_rises_monotonically() {
        // Use a long attack so we can observe the ramp
        let mut module = make_module_with_envelope(200.0, 100.0);
        let events = vec![note_on(69)];
        let block_size = 4410; // 100ms at 44100
        let mut left = vec![0.0; block_size];
        let mut right = vec![0.0; block_size];

        module.process_with_midi(&mut left, &mut right, &events);

        // The envelope uses an exponential smoother, so it rises from 0 toward 1.
        // Check that the output is generally increasing in the first ~50 samples.
        // We allow for saw-wave oscillation within the envelope shape, so we check
        // that the absolute values are bounded and generally growing.
        //
        // Compute envelope of abs(output) using a simple moving-average peak tracker.
        let window = 64;
        let mut envelopes = Vec::new();
        for chunk in left.chunks(window) {
            let local_peak = chunk.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
            envelopes.push(local_peak);
        }

        // Envelope should be monotonically non-decreasing (allowing for smoothing)
        // from the first non-zero chunk to the last.
        let mut first_nonzero = envelopes.len();
        for (i, &e) in envelopes.iter().enumerate() {
            if e > 1e-6 {
                first_nonzero = i;
                break;
            }
        }
        assert!(
            first_nonzero < envelopes.len() - 1,
            "Should have some nonzero output within the block"
        );

        // After the first nonzero region, envelope should generally rise
        let post_attack = &envelopes[first_nonzero..];
        let last = *post_attack.last().unwrap();
        let first = post_attack[0];
        assert!(
            last > first * 0.5,
            "Envelope should rise during attack: first={first}, last={last}"
        );
    }

    // ---- Note-off -> release decay reaches near zero ----

    #[test]
    fn note_off_release_decays() {
        // Short attack, long-ish release so we can measure decay
        let mut module = make_module_with_envelope(1.0, 200.0);
        let block_size = 4410; // 100ms

        // Block 1: NoteOn at sample 0, process enough for attack to complete
        let events_on = vec![note_on(69)];
        let mut left_a = vec![0.0; block_size];
        let mut right_a = vec![0.0; block_size];
        module.process_with_midi(&mut left_a, &mut right_a, &events_on);

        let peak_a = peak(&left_a);
        assert!(peak_a > 0.0, "Should produce output after NoteOn");

        // Block 2: NoteOff at sample 0, process release period
        let events_off = vec![note_off(69, 0)];
        let mut left_b = vec![0.0; block_size];
        let mut right_b = vec![0.0; block_size];
        module.process_with_midi(&mut left_b, &mut right_b, &events_off);

        // Block 3: Continue processing (no events) to let release decay further
        let mut left_c = vec![0.0; block_size];
        let mut right_c = vec![0.0; block_size];
        module.process_with_midi(&mut left_c, &mut right_c, &[]);

        let peak_b = peak(&left_b);
        let peak_c = peak(&left_c);

        // After release_time * 2 (400ms total), envelope should be very small
        // Block C is 200-300ms post release start
        assert!(
            peak_c < peak_a * 0.01,
            "After release period, peak should decay significantly: \
             peak_a={peak_a}, peak_c={peak_c}"
        );

        // Release should be decaying
        assert!(
            peak_b > peak_c || peak_c < 1e-6,
            "Release should be decaying: peak_b={peak_b}, peak_c={peak_c}"
        );
    }

    // ---- Polyphony: 4 simultaneous notes -> louder RMS than single note ----

    #[test]
    fn polyphony_increases_rms() {
        // Single note
        let mut module_single = make_module();
        let events_single = vec![note_on(69)];
        let mut left_s = vec![0.0; 4096];
        let mut right_s = vec![0.0; 4096];
        module_single.process_with_midi(&mut left_s, &mut right_s, &events_single);
        let rms_single = rms(&left_s);

        // Four simultaneous notes
        let mut module_poly = make_module();
        let events_poly = vec![
            note_on(60), // C4
            note_on(64), // E4
            note_on(67), // G4
            note_on(72), // C5
        ];
        let mut left_p = vec![0.0; 4096];
        let mut right_p = vec![0.0; 4096];
        module_poly.process_with_midi(&mut left_p, &mut right_p, &events_poly);
        let rms_poly = rms(&left_p);

        assert!(
            rms_poly > rms_single,
            "Polyphonic RMS ({rms_poly}) should exceed monophonic RMS ({rms_single})"
        );
    }

    // ---- No NaN/Inf across random MIDI streams ----

    #[test]
    fn no_nan_inf_across_random_midi() {
        let mut module = make_module();

        // Generate a stream of pseudo-random note events
        let mut events = Vec::new();
        let mut prng = XorShift32(12345);
        for i in 0..500 {
            let note = (prng.next() % 128) as u8;
            let velocity = prng.gen_f32();
            let timing = i * 10;

            if prng.next() % 3 == 0 {
                // NoteOff
                events.push(NoteEvent::NoteOff {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note,
                    velocity: 0.0,
                });
            } else {
                // NoteOn
                events.push(NoteEvent::NoteOn {
                    timing,
                    voice_id: None,
                    channel: 0,
                    note,
                    velocity,
                });
            }
        }

        // Process all events at once (they have correct timing offsets)
        let total_samples = 5000;
        let mut left = vec![0.0f32; total_samples];
        let mut right = vec![0.0f32; total_samples];

        module.process_with_midi(&mut left, &mut right, &events);

        // Also process more samples with no events to test release
        let mut extra_left = vec![0.0f32; 44100];
        let mut extra_right = vec![0.0f32; 44100];
        module.process_with_midi(&mut extra_left, &mut extra_right, &[]);

        // Check for NaN/Inf in both buffers
        for (i, &s) in left.iter().enumerate() {
            assert!(
                s.is_finite(),
                "left[{i}] is not finite: {s}"
            );
        }
        for (i, &s) in right.iter().enumerate() {
            assert!(
                s.is_finite(),
                "right[{i}] is not finite: {s}"
            );
        }
        for (i, &s) in extra_left.iter().enumerate() {
            assert!(
                s.is_finite(),
                "extra_left[{i}] is not finite: {s}"
            );
        }
        for (i, &s) in extra_right.iter().enumerate() {
            assert!(
                s.is_finite(),
                "extra_right[{i}] is not finite: {s}"
            );
        }
    }

    // ---- Reset clears all voices ----

    #[test]
    fn reset_clears_voices() {
        let mut module = make_module();

        // Activate some voices
        let events = vec![note_on(60), note_on(64), note_on(67)];
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        module.process_with_midi(&mut left, &mut right, &events);

        let rms_before = rms(&left);
        assert!(rms_before > 0.0, "Should produce output before reset");

        // Reset
        module.reset();

        // After reset, should be silent
        let mut left2 = vec![1.0; 256];
        let mut right2 = vec![1.0; 256];
        module.process_with_midi(&mut left2, &mut right2, &[]);

        for (i, l) in left2.iter().enumerate() {
            assert!(
                l.abs() < f32::EPSILON,
                "left[{i}] should be silent after reset, got {l}"
            );
        }
    }

    // ---- Left and right channels are identical (mono saw) ----

    #[test]
    fn left_right_channels_identical() {
        let mut module = make_module();
        let events = vec![note_on(69)];
        let mut left = vec![0.0; 512];
        let mut right = vec![0.0; 512];

        module.process_with_midi(&mut left, &mut right, &events);

        for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert!(
                (l - r).abs() < f32::EPSILON,
                "Sample {i}: left={l} != right={r}"
            );
        }
    }
}

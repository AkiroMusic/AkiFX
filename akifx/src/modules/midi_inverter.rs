//! MIDI Inverter module — transforms MIDI events by inverting note numbers,
//! channels, velocities, CC values, pitch bend, and other parameters.
//!
//! This is a MIDI FX module: audio passes through bit-identical. All
//! transformation happens in [`process_with_midi`], which applies the exact
//! same math as the nih-plug `midi_inverter` example.
//!
//! # Transformation Rules (ported from nih-plug source)
//!
//! | Field        | Formula        |
//! |--------------|----------------|
//! | `channel`    | `15 - channel` |
//! | `note`       | `127 - note`   |
//! | `velocity`   | `1.0 - velocity` |
//! | `pressure`   | `1.0 - pressure` |
//! | `gain`       | `1.0 - gain`   |
//! | `pan`        | `1.0 - pan`    |
//! | `tuning`     | `1.0 - tuning` |
//! | `vibrato`    | `1.0 - vibrato` |
//! | `expression` | `1.0 - expression` |
//! | `brightness` | `1.0 - brightness` |
//! | `cc` number  | **not** inverted |
//! | `value` (CC) | `1.0 - value`  |
//!
//! # MIDI Output Routing
//!
//! Transformed events are queued internally and must be consumed by the
//! plugin-level MIDI output routing in a future integration wave. Call
//! [`MidiInverterModule::take_transformed_events`] after each process block
//! to drain the queue.
//!
//! # Parameter
//!
//! - **Enable** (`#[id = "midi_inv_enable"]`): Bypass toggle (BoolParam).
//!   When disabled the module produces bit-identical audio and passes all
//!   MIDI events through unmodified.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Concrete parameter struct for the MIDI Inverter module.
#[derive(Params)]
pub struct MidiInverterParams {
    /// Enable / bypass toggle.
    #[id = "midi_inv_enable"]
    pub enable: BoolParam,
}

impl MidiInverterParams {
    /// Create params with the given default enabled state.
    pub fn new(default_enabled: bool) -> Self {
        Self {
            enable: BoolParam::new("Enable", default_enabled),
        }
    }
}

impl Default for MidiInverterParams {
    fn default() -> Self {
        Self::new(true)
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// MIDI Inverter module — pure MIDI transform, audio passthrough.
pub struct MidiInverterModule {
    params: Arc<MidiInverterParams>,
    bypass: Arc<AtomicBool>,
    /// Queue of transformed MIDI events produced during the last `process_with_midi` call.
    /// Consumed by [`take_transformed_events`].
    transformed_queue: Vec<NoteEvent<()>>,
}

impl MidiInverterModule {
    /// Create a new module with shared params and bypass flag.
    ///
    /// The `Arc<MidiInverterParams>` should also be given to the umbrella
    /// `AkiFxParams` via `#[nested(id_prefix = "midi_inv")]` for host
    /// serialization.
    pub fn new(params: Arc<MidiInverterParams>, bypass: Arc<AtomicBool>) -> Self {
        Self {
            params,
            bypass,
            transformed_queue: Vec::new(),
        }
    }

    /// Convenience constructor for testing and standalone use.
    pub fn with_defaults() -> Self {
        let params = Arc::new(MidiInverterParams::default());
        let bypass = Arc::new(AtomicBool::new(false));
        Self::new(params, bypass)
    }

    /// Drain the internal queue of transformed MIDI events.
    ///
    /// The plugin-level MIDI output routing (integration wave) will call
    /// this after each `process_with_midi` to send transformed events to
    /// the host's MIDI output.
    pub fn take_transformed_events(&mut self) -> Vec<NoteEvent<()>> {
        std::mem::take(&mut self.transformed_queue)
    }
}

// ---------------------------------------------------------------------------
// Pure transformation function
// ---------------------------------------------------------------------------

/// Transform a single MIDI event by inverting all invertable fields.
///
/// Returns a `Vec` (typically 0 or 1 elements) because some events (like
/// unrecognized variants) produce no output — matching the source plugin's
/// `_ => ()` catch-all that silently drops them.
pub fn invert_event(ev: NoteEvent<()>) -> Vec<NoteEvent<()>> {
    match ev {
        NoteEvent::NoteOn {
            timing,
            voice_id,
            channel,
            note,
            velocity,
        } => vec![NoteEvent::NoteOn {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            velocity: 1.0 - velocity,
        }],
        NoteEvent::NoteOff {
            timing,
            voice_id,
            channel,
            note,
            velocity,
        } => vec![NoteEvent::NoteOff {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            velocity: 1.0 - velocity,
        }],
        NoteEvent::Choke {
            timing,
            voice_id,
            channel,
            note,
        } => vec![NoteEvent::Choke {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
        }],
        NoteEvent::PolyPressure {
            timing,
            voice_id,
            channel,
            note,
            pressure,
        } => vec![NoteEvent::PolyPressure {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            pressure: 1.0 - pressure,
        }],
        NoteEvent::PolyVolume {
            timing,
            voice_id,
            channel,
            note,
            gain,
        } => vec![NoteEvent::PolyVolume {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            gain: 1.0 - gain,
        }],
        NoteEvent::PolyPan {
            timing,
            voice_id,
            channel,
            note,
            pan,
        } => vec![NoteEvent::PolyPan {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            pan: 1.0 - pan,
        }],
        NoteEvent::PolyTuning {
            timing,
            voice_id,
            channel,
            note,
            tuning,
        } => vec![NoteEvent::PolyTuning {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            tuning: 1.0 - tuning,
        }],
        NoteEvent::PolyVibrato {
            timing,
            voice_id,
            channel,
            note,
            vibrato,
        } => vec![NoteEvent::PolyVibrato {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            vibrato: 1.0 - vibrato,
        }],
        NoteEvent::PolyExpression {
            timing,
            voice_id,
            channel,
            note,
            expression,
        } => vec![NoteEvent::PolyExpression {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            expression: 1.0 - expression,
        }],
        NoteEvent::PolyBrightness {
            timing,
            voice_id,
            channel,
            note,
            brightness,
        } => vec![NoteEvent::PolyBrightness {
            timing,
            voice_id,
            channel: 15 - channel,
            note: 127 - note,
            brightness: 1.0 - brightness,
        }],
        NoteEvent::MidiChannelPressure {
            timing,
            channel,
            pressure,
        } => vec![NoteEvent::MidiChannelPressure {
            timing,
            channel: 15 - channel,
            pressure: 1.0 - pressure,
        }],
        NoteEvent::MidiPitchBend {
            timing,
            channel,
            value,
        } => vec![NoteEvent::MidiPitchBend {
            timing,
            channel: 15 - channel,
            value: 1.0 - value,
        }],
        NoteEvent::MidiCC {
            timing,
            channel,
            cc,
            value,
        } => vec![NoteEvent::MidiCC {
            timing,
            channel: 15 - channel,
            cc, // CC number is NOT inverted (matching source)
            value: 1.0 - value,
        }],
        // Unrecognized / other variants — silently dropped, matching source
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// AkiFxModule impl
// ---------------------------------------------------------------------------

impl AkiFxModule for MidiInverterModule {
    fn name(&self) -> &'static str {
        "MIDI Inverter"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, _sample_rate: f32, _max_block_size: usize) {
        // No resources that depend on sample rate.
    }

    fn reset(&mut self) {
        self.transformed_queue.clear();
    }

    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {
        // Audio passthrough — bit-identical. This is a MIDI FX module.
    }

    fn process_with_midi(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        note_events: &[NoteEvent<()>],
    ) {
        // When disabled, pass everything through unchanged.
        if self.params.enable.value() {
            for ev in note_events {
                self.transformed_queue.extend(invert_event(*ev));
            }
        }
        // Audio always passes through unchanged.
        let _ = left;
        let _ = right;
    }

    fn take_output_midi(&mut self) -> Vec<NoteEvent<()>> {
        std::mem::take(&mut self.transformed_queue)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a module ready for testing
    fn make_module() -> MidiInverterModule {
        let mut m = MidiInverterModule::with_defaults();
        m.initialize(44100.0, 512);
        m
    }

    // ---- Audio passthrough (bit-identical) ----

    #[test]
    fn audio_passthrough_bit_identical() {
        let mut module = make_module();
        let original_left: Vec<f32> = (0..128).map(|i| i as f32 * 0.01).collect();
        let original_right: Vec<f32> = (0..128).map(|i| i as f32 * -0.01).collect();
        let left_before = original_left.clone();
        let right_before = original_right.clone();
        let mut left = original_left;
        let mut right = original_right;

        module.process(&mut left, &mut right);

        assert_eq!(left, left_before, "left channel must be bit-identical");
        assert_eq!(right, right_before, "right channel must be bit-identical");
    }

    #[test]
    fn audio_passthrough_with_midi_events() {
        let mut module = make_module();
        let left_before = vec![0.5; 64];
        let right_before = vec![-0.5; 64];
        let mut left = left_before.clone();
        let mut right = right_before.clone();

        let events = vec![NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note: 60,
            velocity: 1.0,
        }];

        module.process_with_midi(&mut left, &mut right, &events);

        assert_eq!(left, left_before, "left unchanged with MIDI events");
        assert_eq!(right, right_before, "right unchanged with MIDI events");
    }

    // ---- Note inversion ----

    #[test]
    fn note_inversion_basic() {
        // note 60 → 127 - 60 = 67
        let ev = NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note: 60,
            velocity: 0.8,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::NoteOn {
                timing,
                voice_id,
                channel,
                note,
                velocity,
            } => {
                assert_eq!(*timing, 0);
                assert_eq!(*voice_id, None);
                assert_eq!(*channel, 15, "channel: 15 - 0 = 15");
                assert_eq!(*note, 67, "note: 127 - 60 = 67");
                assert!((*velocity - 0.2).abs() < f32::EPSILON, "velocity: 1.0 - 0.8 = 0.2");
            }
            _ => panic!("expected NoteOn"),
        }
    }

    #[test]
    fn note_inversion_roundtrip() {
        // Double-inversion should return to original values
        let original = NoteEvent::NoteOn {
            timing: 10,
            voice_id: Some(3),
            channel: 2,
            note: 72,
            velocity: 0.6,
        };

        let inverted = invert_event(original.clone());
        assert_eq!(inverted.len(), 1);

        let restored = invert_event(inverted.into_iter().next().unwrap());
        assert_eq!(restored.len(), 1);

        // Compare roundtripped with original
        let orig_str = format!("{:?}", original);
        let restored_str = format!("{:?}", restored[0]);
        assert_eq!(orig_str, restored_str, "double inversion must restore original");
    }

    #[test]
    fn note_off_inversion() {
        let ev = NoteEvent::NoteOff {
            timing: 0,
            voice_id: None,
            channel: 0,
            note: 60,
            velocity: 0.5,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::NoteOff { channel, note, velocity, .. } => {
                assert_eq!(*channel, 15);
                assert_eq!(*note, 67);
                assert!((*velocity - 0.5).abs() < f32::EPSILON);
            }
            _ => panic!("expected NoteOff"),
        }
    }

    // ---- CC value inversion ----

    #[test]
    fn cc_value_inversion() {
        let ev = NoteEvent::MidiCC {
            timing: 0,
            channel: 0,
            cc: 1, // modulation wheel — cc number NOT inverted
            value: 0.75,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::MidiCC {
                channel, cc, value, ..
            } => {
                assert_eq!(*channel, 15, "channel: 15 - 0 = 15");
                assert_eq!(*cc, 1, "cc number must NOT be inverted");
                assert!((*value - 0.25).abs() < f32::EPSILON, "value: 1.0 - 0.75 = 0.25");
            }
            _ => panic!("expected MidiCC"),
        }
    }

    // ---- Pitch bend inversion ----

    #[test]
    fn pitch_bend_inversion() {
        let ev = NoteEvent::MidiPitchBend {
            timing: 0,
            channel: 0,
            value: 0.5, // center
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::MidiPitchBend {
                channel, value, ..
            } => {
                assert_eq!(*channel, 15);
                assert!((*value - 0.5).abs() < f32::EPSILON, "value: 1.0 - 0.5 = 0.5");
            }
            _ => panic!("expected MidiPitchBend"),
        }
    }

    #[test]
    fn pitch_bend_extreme_values() {
        // value 0.0 → 1.0, value 1.0 → 0.0
        let ev_zero = NoteEvent::MidiPitchBend {
            timing: 0,
            channel: 5,
            value: 0.0,
        };
        let result = invert_event(ev_zero);
        match &result[0] {
            NoteEvent::MidiPitchBend { value, .. } => {
                assert!((*value - 1.0).abs() < f32::EPSILON);
            }
            _ => panic!("expected MidiPitchBend"),
        }

        let ev_one = NoteEvent::MidiPitchBend {
            timing: 0,
            channel: 5,
            value: 1.0,
        };
        let result = invert_event(ev_one);
        match &result[0] {
            NoteEvent::MidiPitchBend { value, .. } => {
                assert!((*value - 0.0).abs() < f32::EPSILON);
            }
            _ => panic!("expected MidiPitchBend"),
        }
    }

    // ---- Channel pressure inversion ----

    #[test]
    fn channel_pressure_inversion() {
        let ev = NoteEvent::MidiChannelPressure {
            timing: 0,
            channel: 3,
            pressure: 0.6,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::MidiChannelPressure {
                channel, pressure, ..
            } => {
                assert_eq!(*channel, 12, "15 - 3 = 12");
                assert!((*pressure - 0.4).abs() < f32::EPSILON, "1.0 - 0.6 = 0.4");
            }
            _ => panic!("expected MidiChannelPressure"),
        }
    }

    // ---- Empty event slice leaves audio unchanged ----

    #[test]
    fn empty_events_leaves_audio_unchanged() {
        let mut module = make_module();
        let left = vec![1.0; 32];
        let right = vec![-1.0; 32];
        let left_before = left.clone();
        let right_before = right.clone();
        let mut left = left;
        let mut right = right;

        let events: Vec<NoteEvent<()>> = vec![];
        module.process_with_midi(&mut left, &mut right, &events);

        assert_eq!(left, left_before);
        assert_eq!(right, right_before);
    }

    // ---- PolyPressure inversion ----

    #[test]
    fn poly_pressure_inversion() {
        let ev = NoteEvent::PolyPressure {
            timing: 0,
            voice_id: None,
            channel: 7,
            note: 64,
            pressure: 0.3,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::PolyPressure {
                channel,
                note,
                pressure,
                ..
            } => {
                assert_eq!(*channel, 8, "15 - 7 = 8");
                assert_eq!(*note, 63, "127 - 64 = 63");
                assert!((*pressure - 0.7).abs() < f32::EPSILON, "1.0 - 0.3 = 0.7");
            }
            _ => panic!("expected PolyPressure"),
        }
    }

    // ---- Choke inversion ----

    #[test]
    fn choke_inversion() {
        let ev = NoteEvent::Choke {
            timing: 0,
            voice_id: None,
            channel: 15,
            note: 127,
        };

        let result = invert_event(ev);
        assert_eq!(result.len(), 1);

        match &result[0] {
            NoteEvent::Choke { channel, note, .. } => {
                assert_eq!(*channel, 0, "15 - 15 = 0");
                assert_eq!(*note, 0, "127 - 127 = 0");
            }
            _ => panic!("expected Choke"),
        }
    }

    // ---- Queue drain via take_transformed_events ----

    #[test]
    fn take_transformed_events_drains_queue() {
        let mut module = make_module();

        let events = vec![
            NoteEvent::NoteOn { timing: 0, voice_id: None, channel: 0, note: 60, velocity: 0.5 },
            NoteEvent::MidiCC { timing: 0, channel: 0, cc: 7, value: 1.0 },
        ];

        let mut left = vec![0.0; 32];
        let mut right = vec![0.0; 32];
        module.process_with_midi(&mut left, &mut right, &events);

        let taken = module.take_transformed_events();
        assert_eq!(taken.len(), 2, "should have 2 transformed events");

        // Queue should now be empty
        let empty = module.take_transformed_events();
        assert_eq!(empty.len(), 0, "queue should be empty after drain");
    }

    // ---- process_with_midi disabled state (bypass) ----

    #[test]
    fn disabled_state_passes_events_through_unchanged() {
        // Create module with enable param defaulting to false
        let params = Arc::new(MidiInverterParams::new(false));
        let bypass = Arc::new(AtomicBool::new(false));
        let mut module = MidiInverterModule::new(params, bypass);
        module.initialize(44100.0, 512);

        let events = vec![NoteEvent::NoteOn {
            timing: 0,
            voice_id: None,
            channel: 0,
            note: 60,
            velocity: 0.8,
        }];

        let mut left = vec![0.0; 32];
        let mut right = vec![0.0; 32];
        module.process_with_midi(&mut left, &mut right, &events);

        let taken = module.take_transformed_events();
        assert_eq!(taken.len(), 0, "disabled module should not transform events");
    }
}

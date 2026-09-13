//! Core module abstraction for AkiFX.
//!
//! Each audio effect module implements [`AkiFxModule`], which provides a uniform interface
//! for processing, parameter access, bypass control, and lifecycle management.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  AkiFx Plugin (lib.rs)                                     │
//! │  ┌────────────────────┐  ┌──────────────────────────────┐  │
//! │  │  ModuleChain        │  │  AkiFxParams (umbrella)      │  │
//! │  │  ┌──────────────┐  │  │  #[nested(id_prefix="gain")] │  │
//! │  │  │ GainModule    │──┼──│  pub gain: Arc<GainParams>   │  │
//! │  │  │ PhaserModule  │  │  │  // future modules...        │  │
//! │  │  │ ...           │  │  └──────────────────────────────┘  │
//! │  │  └──────────────┘  │                                    │
//! │  └────────────────────┘                                    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Bypass Contract
//!
//! - A bypassed module **must** produce bit-identical output to its input.
//! - The chain checks [`AkiFxModule::is_bypassed()`] and skips calling
//!   [`AkiFxModule::process()`] entirely, guaranteeing zero-cost passthrough.
//! - Each module holds an `Arc<AtomicBool>` bypass flag that the GUI can toggle.
//!
//! # Process Contract
//!
//! - `process()` mutates audio **in place**.
//! - All methods must be safe to call from the audio thread (no allocation, no blocking).

pub mod chain;
pub mod gain;
pub mod registry;
pub mod sine_gen;
pub mod safety_limiter;
pub mod crisp;
pub mod buffr_glitch;
pub mod diopser;
pub mod soft_vacuum;
pub mod crossover;
pub mod spectral_gate;
pub mod frequency_shift;
pub mod spectral_common;
pub mod spectral_compressor;

pub use chain::ModuleChain;

use nih_plug::prelude::Params;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Core trait that all AkiFX audio modules must implement.
///
/// # Implementors
///
/// Each module should:
/// 1. Define a concrete `XxxParams` struct with `#[derive(Params)]`.
/// 2. Store params in an `Arc<XxxParams>` for sharing with the umbrella.
/// 3. Store bypass state in an `Arc<AtomicBool>`.
///
/// # Thread Safety
///
/// This trait is `Send + Sync` to allow modules to be stored in the processing chain
/// and accessed from both the audio thread and the GUI thread.
pub trait AkiFxModule: Send + Sync {
    /// Human-readable module name (e.g., "Gain", "Phaser").
    ///
    /// Used for logging and GUI display. Must return a stable `'static` string.
    fn name(&self) -> &'static str;

    /// Access to the module's parameters via the nih-plug `Params` trait object.
    ///
    /// This allows generic parameter access without knowing the concrete type.
    /// The returned reference must remain valid for the module's lifetime.
    fn params(&self) -> &dyn Params;

    /// The `Arc<AtomicBool>` backing this module's bypass state.
    ///
    /// The chain uses this to skip processing when bypassed. The GUI can toggle
    /// this flag directly. Modules must provide this; the default `set_bypass()`
    /// and `is_bypassed()` implementations use it.
    fn bypass_flag(&self) -> &Arc<AtomicBool>;

    /// Set the bypass state.
    ///
    /// Default implementation stores to the [`Self::bypass_flag()`] atomic.
    fn set_bypass(&self, on: bool) {
        self.bypass_flag().store(on, Ordering::Relaxed);
    }

    /// Check if the module is currently bypassed.
    ///
    /// Default implementation loads from the [`Self::bypass_flag()`] atomic.
    fn is_bypassed(&self) -> bool {
        self.bypass_flag().load(Ordering::Relaxed)
    }

    /// Called once when the plugin is initialized with the host's sample rate
    /// and maximum block size.
    ///
    /// Modules should allocate any resources that depend on sample rate here.
    /// This is called before the first `process()` call.
    fn initialize(&mut self, sample_rate: f32, max_block_size: usize);

    /// Called when the host resets the audio processing state (e.g., transport restart,
    /// sample rate change).
    ///
    /// Modules **must** clear internal state to avoid clicks or pops.
    /// The caller guarantees that `process()` will be called with fresh audio
    /// after this returns.
    fn reset(&mut self);

    /// Latency introduced by this module, in samples.
    ///
    /// Returns `0` by default. Override this if the module introduces latency
    /// (e.g., linear-phase filters, look-ahead compressors).
    fn latency_samples(&self) -> u64 {
        0
    }

    /// Process audio in place.
    ///
    /// # Arguments
    /// * `left` - Left channel samples. Mutated in place.
    /// * `right` - Right channel samples. Mutated in place.
    ///
    /// # Bypass
    ///
    /// The chain will **not** call this method when the module is bypassed.
    /// Do not implement bypass logic inside `process()`.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);

    /// Process audio in place with access to the block's MIDI note events.
    ///
    /// Default implementation ignores the events and delegates to [`Self::process()`].
    /// MIDI-driven modules (synths, buffer glitchers, pitch shifters) override this.
    ///
    /// Event timing: events carry sample offsets within the block; modules that
    /// care about exact timing must handle sub-block scheduling themselves.
    fn process_with_midi(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        note_events: &[nih_plug::midi::NoteEvent<()>],
    ) {
        let _ = note_events;
        self.process(left, right)
    }

    /// Drain any output MIDI events produced during the last `process_with_midi` call.
    ///
    /// Default implementation returns an empty vec. Modules that transform or generate
    /// MIDI (e.g., [`MidiInverterModule`]) override this to return their output queue.
    ///
    /// The plugin calls this after each process block to forward events to the host.
    fn take_output_midi(&mut self) -> Vec<nih_plug::midi::NoteEvent<()>> {
        Vec::new()
    }
}

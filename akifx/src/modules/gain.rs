//! Gain module — the exemplar implementation of `AkiFxModule`.
//!
//! Applies linear gain (stored in dB) to stereo audio. This module serves as the
//! template for all future AkiFX modules.
//!
//! # Parameter
//!
//! - **Gain** (`#[id = "gain"]`): ±24 dB range, logarithmic smoothing (50 ms),
//!   stored as linear gain internally, displayed in dB.

use crate::modules::AkiFxModule;
use nih_plug::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Concrete parameter struct for the Gain module.
///
/// Stored in an `Arc<GainParams>` shared between the module (for DSP access)
/// and the umbrella `AkiFxParams` (for host serialization via `#[nested]`).
#[derive(Params)]
pub struct GainParams {
    /// Gain in dB, stored as linear gain internally.
    #[id = "gain"]
    pub gain: FloatParam,
}

impl GainParams {
    /// Create new gain params with the given default value in dB.
    pub fn new(default_db: f32) -> Self {
        Self {
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(default_db),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

/// Gain module — applies smoothed gain to stereo audio.
///
/// # Usage
///
/// ```rust,no_run
/// use akifx::modules::AkiFxModule;
/// use akifx::modules::gain::{GainModule, GainParams};
/// use std::sync::Arc;
///
/// let params = Arc::new(GainParams::new(0.0));
/// let bypass = Arc::new(std::sync::atomic::AtomicBool::new(false));
/// let mut module = GainModule::new(params, bypass);
/// module.initialize(44100.0, 512);
///
/// let mut left = vec![0.5; 64];
/// let mut right = vec![0.5; 64];
/// module.process(&mut left, &mut right);
/// ```
pub struct GainModule {
    params: Arc<GainParams>,
    bypass: Arc<AtomicBool>,
}

impl GainModule {
    /// Create a new GainModule with shared params and bypass flag.
    ///
    /// The `Arc<GainParams>` should also be given to the umbrella `AkiFxParams`
    /// via `#[nested(id_prefix = "gain")]` for host serialization.
    pub fn new(params: Arc<GainParams>, bypass: Arc<AtomicBool>) -> Self {
        Self { params, bypass }
    }

    /// Convenience constructor for testing and standalone use.
    ///
    /// Creates fresh params and bypass flag. Returns the module; the caller
    /// does not need to manage the `Arc`s unless building an umbrella.
    pub fn with_db(default_db: f32) -> Self {
        let params = Arc::new(GainParams::new(default_db));
        let bypass = Arc::new(AtomicBool::new(false));
        Self { params, bypass }
    }
}

impl AkiFxModule for GainModule {
    fn name(&self) -> &'static str {
        "Gain"
    }

    fn params(&self) -> &dyn Params {
        self.params.as_ref()
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, _sample_rate: f32, _max_block_size: usize) {
        // Seed the smoother with the current value: the nih-plug wrapper
        // initializes smoothers for us in a host, but tests and the
        // standalone binary construct modules directly, and an unseeded
        // smoother would ramp from 0 (silence).
        self.params.gain.smoothed.reset(self.params.gain.value());
    }

    fn reset(&mut self) {
        // Nothing to clear — gain has no internal filter state.
        // The smoother is managed by the nih-plug wrapper.
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Step the smoother once per sample so parameter changes ramp
        // instead of zipper-crackling (the smoother used to be declared but
        // never stepped, leaving the DSP on the raw value).
        for (sample, r) in left.iter_mut().zip(right.iter_mut()) {
            let gain = self.params.gain.smoothed.next();
            *sample *= gain;
            *r *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_at_gain_db(db: f32) -> GainModule {
        let params = Arc::new(GainParams::new(db));
        let bypass = Arc::new(AtomicBool::new(false));
        let mut m = GainModule::new(params, bypass);
        m.initialize(44100.0, 512);
        m
    }

    #[test]
    fn zero_db_is_identity() {
        let mut m = module_at_gain_db(0.0);
        let mut left: Vec<f32> = (0..64).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut right = left.clone();
        let (l0, r0) = (left.clone(), right.clone());
        m.process(&mut left, &mut right);
        for i in 0..64 {
            assert!((left[i] - l0[i]).abs() < 1e-6, "0 dB must be unity at {i}");
            assert!((right[i] - r0[i]).abs() < 1e-6, "0 dB must be unity at {i}");
        }
    }

    #[test]
    fn minus_six_db_halves_amplitude() {
        let mut m = module_at_gain_db(-6.0);
        let mut left = vec![0.5f32; 64];
        let mut right = vec![0.5f32; 64];
        m.process(&mut left, &mut right);
        let expected = 0.5 * util::db_to_gain(-6.0);
        for i in 0..64 {
            assert!(
                (left[i] - expected).abs() < 1e-4,
                "-6 dB must halve amplitude at {i}: {} vs {}",
                left[i], expected
            );
            assert!((right[i] - expected).abs() < 1e-4);
        }
    }
}

//! Spectrum view data for the Spectral Compressor's GUI.
//!
//! The audio thread publishes a [`SpectrumSnapshot`] into a [`SpectrumView`]
//! holder at display rate (~30 Hz); the editor reads the latest snapshot
//! each frame. The holder is a mutex over an `Arc` so publication never
//! blocks the GUI and reads never block the audio thread.

use parking_lot::Mutex;
use std::sync::Arc;

/// One frame of analyzer data for the spectrum display.
#[derive(Debug, Clone, Default)]
pub struct SpectrumSnapshot {
    /// Input spectrum (envelope follower magnitudes) in dBFS, one entry per
    /// FFT bin (window_size / 2 + 1).
    pub magnitudes_db: Vec<f32>,
    /// The downwards threshold curve in dB, same bin layout — drawn as the
    /// overlay the user tunes against.
    pub thresholds_db: Vec<f32>,
    /// Sample rate at publish time (for the frequency axis).
    pub sample_rate: f32,
    /// FFT window size at publish time.
    pub window_size: usize,
}

impl SpectrumSnapshot {
    /// The bin index a frequency maps to, or None when out of range.
    pub fn bin_for_frequency(&self, freq: f32) -> Option<usize> {
        if self.window_size == 0 || self.sample_rate <= 0.0 {
            return None;
        }
        let bin_width = self.sample_rate / self.window_size as f32;
        let idx = (freq / bin_width).round() as i64;
        if idx < 0 {
            Some(0)
        } else if (idx as usize) < self.magnitudes_db.len() {
            Some(idx as usize)
        } else {
            None
        }
    }
}

/// Shared holder for the latest snapshot. The audio thread stores a fresh
/// `Arc` (at display rate, not per block); the GUI clones the `Arc` and draws
/// without holding the lock.
#[derive(Clone, Default)]
pub struct SpectrumView(Arc<Mutex<Arc<SpectrumSnapshot>>>);

impl SpectrumView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a snapshot (audio thread). Replaces the previous one.
    pub fn publish(&self, snapshot: SpectrumSnapshot) {
        *self.0.lock() = Arc::new(snapshot);
    }

    /// Read the latest snapshot (GUI thread). Returns an owned `Arc` so the
    /// lock is released immediately.
    pub fn latest(&self) -> Arc<SpectrumSnapshot> {
        self.0.lock().clone()
    }
}

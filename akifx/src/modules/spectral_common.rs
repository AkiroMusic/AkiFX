//! Shared engine wrapper for the SpectralSuite-derived spectral modules.
//!
//! Owns the STFT engine and the dry-signal snapshots both modules need, so
//! each module only carries its parameters and its spectral callback. All
//! modules here run the same fixed configuration: 2048-point FFT, 4×
//! overlap, Hann window (the SpectralSuite defaults).

use crate::stft::{SpectralConfig, SpectralEngine, WindowType};

/// FFT size for all spectral modules.
pub const SPECTRAL_FFT_SIZE: usize = 2048;
/// Overlap count for all spectral modules.
pub const SPECTRAL_OVERLAPS: usize = 4;

/// Engine + dry-buffer plumbing shared by the spectral modules.
pub struct SpectralFxCore {
    engine: Option<SpectralEngine>,
    dry_buf_l: Vec<f32>,
    dry_buf_r: Vec<f32>,
}

impl Default for SpectralFxCore {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectralFxCore {
    pub fn new() -> Self {
        Self {
            engine: None,
            dry_buf_l: Vec::new(),
            dry_buf_r: Vec::new(),
        }
    }

    /// (Re)build the engine with the fixed spectral configuration.
    pub fn build_engine(&mut self) {
        let config = SpectralConfig {
            fft_size: SPECTRAL_FFT_SIZE,
            overlap_count: SPECTRAL_OVERLAPS,
            window: WindowType::Hann,
        };
        self.engine = Some(SpectralEngine::new(config, 2));
    }

    pub fn build_if_missing(&mut self) {
        if self.engine.is_none() {
            self.build_engine();
        }
    }

    pub fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
    }

    pub fn latency_samples(&self) -> u64 {
        self.engine
            .as_ref()
            .map(|e| e.latency_samples() as u64)
            .unwrap_or(0)
    }

    pub fn num_bins(&self) -> usize {
        SPECTRAL_FFT_SIZE / 2
    }

    /// Snapshot `left`/`right` into the dry buffers and run the engine, with
    /// the given spectral callback invoked per bin frame.
    pub fn process<F>(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        callback: &mut F,
    ) where
        F: FnMut(usize, usize, usize, &mut [crate::stft::Polar]),
    {
        self.ensure_dry_buffers(left.len());
        self.dry_buf_l[..left.len()].copy_from_slice(left);
        self.dry_buf_r[..right.len()].copy_from_slice(right);

        let Some(engine) = self.engine.as_mut() else {
            // Engine construction is guaranteed during initialize(); skip
            // this block rather than panicking in the host's audio callback
            // if not.
            return;
        };
        engine.process(
            &[&self.dry_buf_l[..left.len()], &self.dry_buf_r[..right.len()]],
            &mut [left, right],
            callback,
        );
    }

    fn ensure_dry_buffers(&mut self, block_size: usize) {
        if self.dry_buf_l.len() < block_size {
            self.dry_buf_l.resize(block_size, 0.0);
            self.dry_buf_r.resize(block_size, 0.0);
        }
    }
}

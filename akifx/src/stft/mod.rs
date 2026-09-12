//! Shared spectral STFT engine ported from SpectralSuite.
//!
//! # Quick Start
//!
//! ```ignore
//! use akifx::stft::{SpectralConfig, SpectralEngine, Polar, WindowType};
//!
//! let config = SpectralConfig {
//!     fft_size: 1024,
//!     overlap_count: 4,
//!     window: WindowType::Hann,
//! };
//! let mut engine = SpectralEngine::new(config, 2); // stereo
//!
//! // In your plugin's process callback:
//! engine.process(&[&input_L, &input_R], &mut [&mut output_L, &mut output_R],
//!     &mut |num_bins, polar| {
//!         for bin in polar.iter_mut().take(num_bins) {
//!             // e.g. spectral gate:
//!             if bin.magnitude < 0.01 { bin.magnitude = 0.0; }
//!         }
//!     },
//! );
//! ```

mod engine;
mod polar;
mod window;

pub use engine::{SpectralConfig, SpectralEngine};
pub use polar::Polar;
pub use window::WindowType;

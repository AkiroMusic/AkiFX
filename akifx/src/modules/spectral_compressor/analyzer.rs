//! Minimal analyzer data stub for the spectral compressor.
//!
//! In the original nih-plug plugin, this data feeds a triple-buffered spectrum
//! analyzer for the GUI. Here we provide the data structure only — actual GUI
//! integration is deferred to a future phase.

use super::curve::CurveParams;

/// Data used for the spectrum analyzer display.
///
/// This contains the envelope follower magnitudes and gain reduction data,
/// both accumulated during processing. The GUI will consume this via
/// a lock-free channel or atomic pointer.
#[derive(Debug, Clone)]
pub struct AnalyzerData {
    /// The parameters for the global threshold curve.
    pub curve_params: CurveParams,
    /// Upwards and downwards threshold offsets for drawing.
    pub curve_offsets_db: (f32, f32),
    /// Number of active bins.
    pub num_bins: usize,
    /// Per-bin envelope follower magnitudes (linear).
    pub envelope_followers: Vec<f32>,
    /// Per-bin gain difference in dB (positive = boost, negative = reduction).
    pub gain_difference_db: Vec<f32>,
}

impl AnalyzerData {
    /// Create a new analyzer data buffer with the given capacity.
    pub fn new(max_bins: usize) -> Self {
        Self {
            curve_params: CurveParams::default(),
            curve_offsets_db: (0.0, 0.0),
            num_bins: 0,
            envelope_followers: vec![0.0; max_bins],
            gain_difference_db: vec![0.0; max_bins],
        }
    }
}

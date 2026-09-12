//! Threshold curve evaluation for the spectral compressor.
//!
//! The threshold curve is a quadratic polynomial evaluated in log-log space
//! (octaves on the x-axis, dB on the y-axis). This module provides the same
//! curve used by both the compressor bank and the GUI analyzer display.

/// Parameters for a threshold curve.
#[derive(Debug, Default, Clone, Copy)]
pub struct CurveParams {
    /// The compressor threshold at the center frequency (intercept).
    pub intercept: f32,
    /// The center frequency for the curve.
    pub center_frequency: f32,
    /// The slope for the curve, in dB/oct.
    pub slope: f32,
    /// The curvature coefficient (parabolic behavior).
    pub curve: f32,
}

/// Evaluates the quadratic threshold curve.
pub struct Curve<'a> {
    params: &'a CurveParams,
    ln_center_frequency: f32,
}

impl<'a> Curve<'a> {
    pub fn new(params: &'a CurveParams) -> Self {
        Self {
            params,
            ln_center_frequency: params.center_frequency.ln(),
        }
    }

    /// Evaluate the curve for the natural logarithm of the frequency.
    #[inline]
    pub fn evaluate_ln(&self, ln_freq: f32) -> f32 {
        let offset = ln_freq - self.ln_center_frequency;
        self.params.intercept
            + (self.params.slope * offset)
            + (self.params.curve * offset * offset)
    }

    /// Evaluate the curve for a frequency in Hertz.
    #[inline]
    #[allow(unused)]
    pub fn evaluate_linear(&self, freq: f32) -> f32 {
        self.evaluate_ln(freq.ln())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_curve_at_center_frequency() {
        let params = CurveParams {
            intercept: -12.0,
            center_frequency: 1000.0,
            slope: 0.0,
            curve: 0.0,
        };
        let curve = Curve::new(&params);
        let result = curve.evaluate_ln(1000.0f32.ln());
        assert!(
            (result - (-12.0)).abs() < 1e-6,
            "at center frequency the curve should equal the intercept"
        );
    }

    #[test]
    fn slope_3db_octave() {
        let params = CurveParams {
            intercept: 0.0,
            center_frequency: 1000.0,
            slope: -3.0,
            curve: 0.0,
        };
        let curve = Curve::new(&params);
        let one_octave_above = curve.evaluate_ln(2000.0f32.ln());
        // Curve evaluates slope * ln(freq/center_freq) = -3.0 * ln(2) ≈ -2.079
        let expected = -3.0 * 2.0f32.ln();
        assert!(
            (one_octave_above - expected).abs() < 1e-5,
            "one octave above center: expected {expected:.4}, got {one_octave_above}"
        );
    }

    #[test]
    fn parabolic_curve_symmetry() {
        let params = CurveParams {
            intercept: 0.0,
            center_frequency: 1000.0,
            slope: 0.0,
            curve: 1.0,
        };
        let curve = Curve::new(&params);
        let above = curve.evaluate_ln(2000.0f32.ln());
        let below = curve.evaluate_ln(500.0f32.ln());
        assert!(
            (above - below).abs() < 1e-5,
            "parabolic curve should be symmetric: above={above}, below={below}"
        );
    }
}

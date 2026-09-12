//! Polar coordinate representation matching SpectralSuite's `Polar.h` and
//! `Polar.cpp`.  Used as the currency type between the STFT engine and the
//! user's spectral callback.

use std::ops::{Mul, MulAssign};
use rustfft::num_complex::Complex;

/// A magnitude/phase pair — mirrors the C++ `Polar<T>` template.
///
/// Fields are named `magnitude` / `phase` rather than `m_mag` / `m_phase`
/// for idiomatic Rust, but the semantics are identical.
#[derive(Debug, Clone, Copy, Default)]
pub struct Polar {
    pub magnitude: f32,
    pub phase: f32,
}

impl Polar {
    /// `Polar(mag, phase)` — matches `Polar(const T&, const T&)`.
    #[inline]
    pub fn new(magnitude: f32, phase: f32) -> Self {
        Self { magnitude, phase }
    }

    /// Convert from a complex value: `m_mag = abs(c); m_phase = arg(c)`.
    ///
    /// Matches `Polar(const std::complex<T>&)` in Polar.h line 19.
    #[inline]
    pub fn from_complex(c: Complex<f32>) -> Self {
        Self {
            magnitude: c.norm(),
            phase: c.arg(),
        }
    }

    /// Convert back to rectangular form: `polar(mag, phase)`.
    #[inline]
    pub fn to_complex(self) -> Complex<f32> {
        Complex::from_polar(self.magnitude, self.phase)
    }
}

// ── Polar<T>::operator*(T)  —  Polar.cpp lines 4-10 ──────────────
// Both magnitude AND phase are scaled (unusual but faithful to source).

impl Mul<f32> for Polar {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self {
            magnitude: self.magnitude * rhs,
            phase: self.phase * rhs,
        }
    }
}

impl MulAssign<f32> for Polar {
    #[inline]
    fn mul_assign(&mut self, rhs: f32) {
        self.magnitude *= rhs;
        self.phase *= rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_complex_roundtrip() {
        let c = Complex::new(3.0, 4.0);
        let p = Polar::from_complex(c);
        assert!((p.magnitude - 5.0).abs() < 1e-6);
        let back = p.to_complex();
        assert!((back.re - 3.0).abs() < 1e-5);
        assert!((back.im - 4.0).abs() < 1e-5);
    }

    #[test]
    fn operator_mul_scales_both() {
        let p = Polar::new(2.0, 1.0);
        let q = p * 3.0;
        assert!((q.magnitude - 6.0).abs() < 1e-9);
        assert!((q.phase - 3.0).abs() < 1e-9);
    }
}

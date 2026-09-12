//! FFT window functions matching SpectralSuite's `FftWindowType.h` and
//! `StandardFFTProcessor.cpp` coefficient math **exactly**.
//!
//! Deviation documented: the C++ Hamming window uses `size + 1` in the
//! denominator (line 125 of StandardFFTProcessor.cpp), which differs from
//! the textbook Hamming formula that uses `size - 1`.

/// Window type identifiers matching SpectralSuite's `FftWindowType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowType {
    /// All ones — no windowing. Not in SpectralSuite; added for COLA testing.
    Rectangular,
    /// Hann (raised cosine) window.
    Hann,
    /// Hamming window.
    Hamming,
    /// Blackman window.
    Blackman,
    /// 4-term Blackman-Harris window.
    BlackmanHarris,
}

/// C++ `TWOPI` constant: `6.28318531` (from StandardFFTProcessor.h line 25).
const TWOPI: f32 = 6.28318531;

impl WindowType {
    /// Generate a window table of `len` samples.
    ///
    /// Coefficients and denominator choices are copied verbatim from
    /// `StandardFFTProcessor.cpp` lines 100–146.
    pub fn generate(&self, len: usize) -> Vec<f32> {
        match self {
            Self::Rectangular => vec![1.0; len],
            Self::Hann => Self::gen_hann(len),
            Self::Hamming => Self::gen_hamming(len),
            Self::Blackman => Self::gen_blackman(len),
            Self::BlackmanHarris => Self::gen_blackman_harris(len),
        }
    }

    // ── Hann ──────────────────────────────────────────────────────────
    // StandardFFTProcessor.cpp lines 100-111
    // `max = (float)size - 1.f`
    // `w = TWOPI * (float(n) / max)`
    // `table[n] = 0.5f * (1.f - cosf(w))`
    fn gen_hann(len: usize) -> Vec<f32> {
        let max = (len as f32) - 1.0;
        (0..len)
            .map(|n| {
                let w = TWOPI * (n as f32 / max);
                0.5 * (1.0 - w.cos())
            })
            .collect()
    }

    // ── Hamming ───────────────────────────────────────────────────────
    // StandardFFTProcessor.cpp lines 124-130
    // NOTE: denominator is `size + 1` (differs from textbook `size - 1`).
    // `max = (float)size + 1.f`
    // `w = TWOPI * (float(n) / max)`
    // `table[n] = 0.53836f - 0.46164f * cosf(w)`
    fn gen_hamming(len: usize) -> Vec<f32> {
        let max = (len as f32) + 1.0;
        (0..len)
            .map(|n| {
                let w = TWOPI * (n as f32 / max);
                0.53836 - 0.46164 * w.cos()
            })
            .collect()
    }

    // ── Blackman ──────────────────────────────────────────────────────
    // StandardFFTProcessor.cpp lines 113-122
    // `max = (float)size - 1.f`
    // `table[n] = 0.42f - (0.5f*cosf(TWOPI*n/max) + 0.08f*cosf(TWOPI*2*n/max))`
    fn gen_blackman(len: usize) -> Vec<f32> {
        let max = (len as f32) - 1.0;
        (0..len)
            .map(|n| {
                let ratio = n as f32 / max;
                let w1 = TWOPI * ratio;
                let w2 = TWOPI * 2.0 * ratio;
                0.42 - (0.5 * w1.cos() + 0.08 * w2.cos())
            })
            .collect()
    }

    // ── Blackman-Harris ───────────────────────────────────────────────
    // StandardFFTProcessor.cpp lines 132-146
    // `max = (float)size - 1.f`
    // `table[n] = 0.35875 - 0.48829*cos(w1) + 0.14128*cos(w2) - 0.01168*cos(w3)`
    // where w1 = TWOPI*inc, w2 = TWOPI*2*inc, w3 = TWOPI*6*inc
    fn gen_blackman_harris(len: usize) -> Vec<f32> {
        let max = (len as f32) - 1.0;
        (0..len)
            .map(|n| {
                let inc = n as f32 / max;
                let w1 = TWOPI * inc;
                let w2 = TWOPI * 2.0 * inc;
                let w3 = TWOPI * 6.0 * inc;
                0.35875 - 0.48829 * w1.cos() + 0.14128 * w2.cos() - 0.01168 * w3.cos()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_are_zero() {
        let w = WindowType::Hann.generate(512);
        assert!((w[0]).abs() < 1e-7);
        assert!((w[511]).abs() < 1e-7);
    }

    #[test]
    fn hamming_endpoints_are_nonzero() {
        let w = WindowType::Hamming.generate(512);
        assert!(w[0] > 0.05);
        assert!(w[511] > 0.05);
    }

    #[test]
    fn blackman_harris_endpoints_are_near_zero() {
        let w = WindowType::BlackmanHarris.generate(512);
        assert!(w[0].abs() < 1e-4);
        assert!(w[511].abs() < 1e-4);
    }
}

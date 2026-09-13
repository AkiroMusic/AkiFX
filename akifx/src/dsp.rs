//! Shared DSP building blocks.

/// A biquad filter implemented in Transposed Direct Form II.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    pub coefficients: BiquadCoefficients,
    s1: f32,
    s2: f32,
}

impl Biquad {
    pub fn update_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coefficients = coefficients;
    }

    /// Process a single sample.
    pub fn process(&mut self, sample: f32) -> f32 {
        let result = self.coefficients.b0 * sample + self.s1;
        self.s1 = self.coefficients.b1 * sample - self.coefficients.a1 * result + self.s2;
        self.s2 = self.coefficients.b2 * sample - self.coefficients.a2 * result;
        result
    }

    /// Reset the state to zero.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// RBJ biquad coefficient formulas.
///
/// <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadCoefficients {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoefficients {
    /// Clamp inputs into a range where the RBJ formulas are well-defined.
    /// Crossover-frequency parameters can reach 20 kHz, so hosts running at
    /// or below 40 kHz can push automated values past Nyquist; clamping
    /// keeps the audio thread panic-free.
    fn sanitize(sample_rate: f32, frequency: f32, q: f32) -> (f32, f32, f32) {
        let sample_rate = sample_rate.max(1.0);
        let frequency = frequency.clamp(1.0, sample_rate * 0.45);
        let q = q.max(1.0e-4);
        (sample_rate, frequency, q)
    }

    /// Compute coefficients for a 2nd-order low-pass filter.
    pub fn lowpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = std::f32::consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_omega0) / 2.0) / a0,
            b1: (1.0 - cos_omega0) / a0,
            b2: ((1.0 - cos_omega0) / 2.0) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Compute coefficients for a 2nd-order high-pass filter.
    pub fn highpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = std::f32::consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_omega0) / 2.0) / a0,
            b1: -(1.0 + cos_omega0) / a0,
            b2: ((1.0 + cos_omega0) / 2.0) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Compute coefficients for a 2nd-order all-pass filter.
    pub fn allpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let (sample_rate, frequency, q) = Self::sanitize(sample_rate, frequency, q);

        let omega0 = std::f32::consts::TAU * (frequency / sample_rate);
        let cos_omega0 = omega0.cos();
        let alpha = omega0.sin() / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - alpha) / a0,
            b1: -2.0 * cos_omega0 / a0,
            b2: (1.0 + alpha) / a0,
            a1: (-2.0 * cos_omega0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_attenuates_highs() {
        let mut b = Biquad::default();
        b.update_coefficients(BiquadCoefficients::lowpass(44100.0, 500.0, 0.707));
        // 10 kHz sine should be strongly attenuated
        let mut energy_out = 0.0;
        let mut energy_in = 0.0;
        for i in 0..4410 {
            let x = (2.0 * std::f32::consts::PI * 10_000.0 * i as f32 / 44100.0).sin();
            let y = b.process(x);
            if i > 441 {
                energy_in += x * x;
                energy_out += y * y;
            }
        }
        assert!(
            (energy_out / energy_in).sqrt() < 0.1,
            "10 kHz must be attenuated by a 500 Hz lowpass"
        );
    }

    #[test]
    fn allpass_keeps_amplitude() {
        let mut b = Biquad::default();
        b.update_coefficients(BiquadCoefficients::allpass(44100.0, 1000.0, 0.5));
        let mut peak = 0.0f32;
        for i in 0..4410 {
            let x = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / 44100.0).sin();
            peak = peak.max(b.process(x).abs());
        }
        assert!(
            (peak - 1.0).abs() < 0.05,
            "allpass must preserve amplitude, peak={peak}"
        );
    }

    #[test]
    fn clamps_out_of_range_inputs() {
        // Must not panic or produce NaN at absurd values
        let c = BiquadCoefficients::lowpass(44100.0, 1.0e9, -1.0);
        assert!(c.b0.is_finite() && c.a1.is_finite());
    }
}

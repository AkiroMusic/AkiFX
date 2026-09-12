//! Dry/wet mixer with latency compensation.
//!
//! Ports the dry-wet mixing logic from nih-plug's Spectral Compressor.
//! Stores a circular delay line of the dry signal, then mixes it back
//! into the processed output with the appropriate latency offset.

/// Mixing style for dry/wet blending.
#[derive(Debug, Clone, Copy)]
pub enum MixingStyle {
    /// Linear crossfade: wet * ratio + dry * (1 - ratio).
    Linear,
    /// Equal-power crossfade: sqrt-based.
    EqualPower,
}

/// Dry-wet mixer with a per-channel circular delay line.
pub struct DryWetMixer {
    /// Delay line indexed by `[channel][sample]`.
    delay_line: Vec<Vec<f32>>,
    /// Next write position in the delay line.
    next_write_position: usize,
}

impl DryWetMixer {
    /// Create a new mixer with the given channel count and max capacities.
    pub fn new(num_channels: usize, max_block_size: usize, max_latency: usize) -> Self {
        let delay_len = (max_block_size + max_latency).next_power_of_two();
        Self {
            delay_line: vec![vec![0.0; delay_len]; num_channels],
            next_write_position: 0,
        }
    }

    /// Resize internal buffers for new parameters.
    pub fn resize(&mut self, num_channels: usize, max_block_size: usize, max_latency: usize) {
        let delay_len = (max_block_size + max_latency).next_power_of_two();
        self.delay_line.resize_with(num_channels, Vec::new);
        for buf in &mut self.delay_line {
            buf.resize(delay_len, 0.0);
            buf.fill(0.0);
        }
        self.next_write_position = 0;
    }

    /// Clear all buffers.
    pub fn reset(&mut self) {
        for buf in &mut self.delay_line {
            buf.fill(0.0);
        }
        self.next_write_position = 0;
    }

    /// Write the dry signal into the delay line. Call at the start of process().
    ///
    /// # Panics
    ///
    /// Panics if the buffer is larger than the delay line capacity.
    pub fn write_dry(&mut self, left: &[f32], right: &[f32]) {
        if self.delay_line.is_empty() {
            return;
        }
        let delay_len = self.delay_line[0].len();
        let block_size = left.len();
        assert!(block_size <= delay_len);

        let num_before_wrap = block_size.min(delay_len - self.next_write_position);
        let num_after_wrap = block_size - num_before_wrap;

        let channels: &[&[f32]] = if self.delay_line.len() >= 2 {
            &[left, right]
        } else {
            &[left]
        };

        for (chan_buf, input) in self.delay_line.iter_mut().zip(channels) {
            chan_buf[self.next_write_position..self.next_write_position + num_before_wrap]
                .copy_from_slice(&input[..num_before_wrap]);
            chan_buf[..num_after_wrap]
                .copy_from_slice(&input[num_before_wrap..]);
        }

        self.next_write_position =
            (self.next_write_position + block_size) % delay_len;
    }

    /// Mix the dry signal back into the processed output.
    ///
    /// - `ratio`: 0.0 = all dry, 1.0 = all wet.
    /// - `latency`: number of samples of delay to compensate for.
    ///
    /// # Panics
    ///
    /// Panics if block_size + latency exceeds delay line capacity.
    pub fn mix_in_dry(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        ratio: f32,
        style: MixingStyle,
        latency: usize,
    ) {
        if self.delay_line.is_empty() {
            return;
        }
        let ratio = ratio.clamp(0.0, 1.0);
        if ratio == 1.0 {
            return;
        }

        let (wet_t, dry_t) = match style {
            MixingStyle::Linear => (ratio, 1.0 - ratio),
            MixingStyle::EqualPower => (ratio.sqrt(), (1.0 - ratio).sqrt()),
        };

        let delay_len = self.delay_line[0].len();
        let block_size = left.len();
        assert!(block_size + latency <= delay_len);

        let read_pos =
            (self.next_write_position + delay_len - block_size - latency) % delay_len;
        let num_before_wrap = block_size.min(delay_len - read_pos);
        let num_after_wrap = block_size - num_before_wrap;

        let output_channels: &mut [&mut [f32]] = if self.delay_line.len() >= 2 {
            &mut [left, right]
        } else {
            &mut [left]
        };

        if ratio == 0.0 {
            // Pure dry passthrough
            for (chan_buf, output) in self.delay_line.iter().zip(output_channels.iter_mut()) {
                output[..num_before_wrap]
                    .copy_from_slice(&chan_buf[read_pos..read_pos + num_before_wrap]);
                output[num_before_wrap..]
                    .copy_from_slice(&chan_buf[..num_after_wrap]);
            }
        } else {
            for (chan_buf, output) in self.delay_line.iter().zip(output_channels.iter_mut()) {
                for (o, &d) in output[..num_before_wrap]
                    .iter_mut()
                    .zip(&chan_buf[read_pos..read_pos + num_before_wrap])
                {
                    *o = (*o * wet_t) + (d * dry_t);
                }
                for (o, &d) in output[num_before_wrap..]
                    .iter_mut()
                    .zip(&chan_buf[..num_after_wrap])
                {
                    *o = (*o * wet_t) + (d * dry_t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_passthrough_ratio_zero() {
        let mut mixer = DryWetMixer::new(2, 256, 4096);
        let left = vec![1.0f32; 64];
        let right = vec![0.5f32; 64];

        mixer.write_dry(&left, &right);

        // Process some wet content
        let mut wet_left = vec![0.0f32; 64];
        let mut wet_right = vec![0.0f32; 64];
        for (i, l) in wet_left.iter_mut().enumerate() {
            *l = (i as f32 * 0.1).sin();
        }
        for (i, r) in wet_right.iter_mut().enumerate() {
            *r = (i as f32 * 0.1).cos();
        }

        mixer.mix_in_dry(&mut wet_left, &mut wet_right, 0.0, MixingStyle::Linear, 0);

        // At ratio=0.0, output should be the dry signal
        for i in 0..64 {
            assert!(
                (wet_left[i] - left[i]).abs() < 1e-6,
                "dry passthrough failed at left[{i}]: expected {}, got {}",
                left[i],
                wet_left[i]
            );
            assert!(
                (wet_right[i] - right[i]).abs() < 1e-6,
                "dry passthrough failed at right[{i}]: expected {}, got {}",
                right[i],
                wet_right[i]
            );
        }
    }

    #[test]
    fn wet_only_ratio_one() {
        let mut mixer = DryWetMixer::new(2, 256, 4096);
        let left = vec![1.0f32; 64];
        let right = vec![0.5f32; 64];

        mixer.write_dry(&left, &right);

        let mut wet_left = vec![0.75f32; 64];
        let mut wet_right = vec![0.25f32; 64];
        let wet_left_orig = wet_left.clone();
        let _wet_right_orig = wet_right.clone();

        mixer.mix_in_dry(&mut wet_left, &mut wet_right, 1.0, MixingStyle::Linear, 0);

        // At ratio=1.0, output should be unchanged wet signal
        for i in 0..64 {
            assert!(
                (wet_left[i] - wet_left_orig[i]).abs() < 1e-6,
                "wet-only passthrough failed at left[{i}]"
            );
        }
    }

    #[test]
    fn reset_clears_buffers() {
        let mut mixer = DryWetMixer::new(2, 256, 4096);
        let left = vec![1.0f32; 64];
        let right = vec![1.0f32; 64];

        mixer.write_dry(&left, &right);
        mixer.reset();

        let mut wet_left = vec![0.0f32; 64];
        let mut wet_right = vec![0.0f32; 64];
        mixer.mix_in_dry(&mut wet_left, &mut wet_right, 0.0, MixingStyle::Linear, 0);

        for i in 0..64 {
            assert!(
                wet_left[i].abs() < 1e-6,
                "after reset, dry signal should be zero"
            );
        }
    }
}

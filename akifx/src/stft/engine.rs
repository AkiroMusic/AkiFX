//! Core STFT engine ported from SpectralSuite's `StandardFFTProcessor` and
//! `SpectralAudioProcessorInteractor`.
//!
//! ## Architecture
//!
//! Each channel has `overlap_count` independent STFT processor instances,
//! each with its own input/output buffer and offset — matching the C++
//! multi-instance-per-channel design exactly.  The host output accumulates
//! across all instances via `+=`.
//!
//! ## Normalisation (faithful to C++)
//!
//! * Forward FFT is **unscaled** (standard DFT, same as kissfft).
//! * First `half_size` bins are multiplied by `2 / fft_size`.
//! * `pol2Car` fills bins `0..half_size` from polar, zeros `half_size..fft_size`.
//! * Inverse FFT is **unscaled** (rustfft default), exactly like kissfft.
//!   No explicit `1/N` is applied — the single-sided `2/N` forward scale plus
//!   the double analysis/synthesis window reconstructs amplitude via COLA.
//! * The window is applied **twice** (analysis + synthesis).
//!
//! ## Deviations from C++
//!
//! 1. **No DC zero** — The original `m_ifftin[0] = 0.f` (line 38) is omitted
//!    so downstream spectral modules can access the DC bin.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::polar::Polar;
use super::window::WindowType;

// ── Public config ───────────────────────────────────────────────────────

/// Configuration for [`SpectralEngine`].
#[derive(Debug, Clone)]
pub struct SpectralConfig {
    pub fft_size: usize,
    pub overlap_count: usize,
    pub window: WindowType,
}

// ── Per-instance state (matches C++ StandardFFTProcessor) ───────────────

struct StftInstance {
    input_buf: Vec<f32>,
    output_buf: Vec<f32>,
    offset: usize,
    // scratch
    fft_buf: Vec<Complex<f32>>,
    ifft_buf: Vec<Complex<f32>>,
    polar_buf: Vec<Polar>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
}

// ── SpectralEngine ──────────────────────────────────────────────────────

pub struct SpectralEngine {
    fft_size: usize,
    half_size: usize,
    hop_size: usize,
    window: Vec<f32>,
    num_channels: usize,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    /// channels[chan][overlap_idx]
    instances: Vec<Vec<StftInstance>>,
}

impl SpectralEngine {
    pub fn new(config: SpectralConfig, num_channels: usize) -> Self {
        assert!(config.fft_size.is_power_of_two());
        assert!(config.fft_size >= 128 && config.fft_size <= 16384);
        assert!(config.overlap_count >= 1 && config.overlap_count <= 8);
        assert!(config.fft_size % config.overlap_count == 0);

        let fft_size = config.fft_size;
        let half_size = fft_size / 2;
        let hop_size = fft_size / config.overlap_count;
        let window = config.window.generate(fft_size);

        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(fft_size);
        let fft_inv = planner.plan_fft_inverse(fft_size);
        let scratch_fwd_len = fft_fwd.get_inplace_scratch_len();
        let scratch_inv_len = fft_inv.get_inplace_scratch_len();

        // channels[chan][overlap] — each instance has its own offset
        // matching SpectralAudioProcessorInteractor::setFftSize:
        //   overlap i gets offset = hopSize * (i % numOverlaps)
        let instances: Vec<Vec<StftInstance>> = (0..num_channels)
            .map(|_chan| {
                (0..config.overlap_count)
                    .map(|overlap_idx| {
                        StftInstance {
                            input_buf: vec![0.0; fft_size],
                            output_buf: vec![0.0; fft_size],
                            offset: hop_size * (overlap_idx % config.overlap_count),
                            fft_buf: vec![Complex::default(); fft_size],
                            ifft_buf: vec![Complex::default(); fft_size],
                            polar_buf: vec![Polar::default(); half_size],
                            scratch_fwd: vec![Complex::default(); scratch_fwd_len],
                            scratch_inv: vec![Complex::default(); scratch_inv_len],
                        }
                    })
                    .collect()
            })
            .collect();

        Self {
            fft_size,
            half_size,
            hop_size,
            window,
            num_channels,
            fft_fwd,
            fft_inv,
            instances,
        }
    }

    /// Reports `fft_size + hop_size`, matching the C++ plugins
    /// (SpectralAudioPlugin reports `fftSize + hopSize`).
    #[inline]
    pub fn latency_samples(&self) -> usize {
        self.fft_size + self.hop_size
    }

    #[inline]
    pub fn num_bins(&self) -> usize {
        self.half_size
    }

    #[inline]
    pub fn hop_size(&self) -> usize {
        self.hop_size
    }

    pub fn reset(&mut self) {
        let hop = self.hop_size;
        let oc = self.instances[0].len();
        for chan_instances in &mut self.instances {
            for (idx, inst) in chan_instances.iter_mut().enumerate() {
                inst.input_buf.iter_mut().for_each(|s| *s = 0.0);
                inst.output_buf.iter_mut().for_each(|s| *s = 0.0);
                inst.offset = hop * (idx % oc);
            }
        }
    }

    /// Process a block of audio for all channels.
    ///
    /// Each channel's output is the sum of all overlap instances' outputs,
    /// matching the C++ SpectralAudioProcessorInteractor::process loop.
    /// The callback is invoked once per FFT frame per instance.
    ///
    /// Unlike the C++ (which steps in `hop_size` chunks), samples are
    /// advanced one at a time so instance offsets hit `fft_size` exactly.
    /// The chunked version silently dropped input/output samples whenever a
    /// host delivered blocks that are not multiples of the hop size (e.g.
    /// 480-sample blocks with a 512-sample hop), causing periodic dropouts.
    pub fn process<F>(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        callback: &mut F,
    ) where
        F: FnMut(usize, usize, usize, &mut [Polar]),
    {
        // `callback(num_bins, channel_idx, overlap_idx, polar)` — the C++
        // creates one spectral processor per (channel, overlap) instance, so
        // the callback receives the instance identity for modules (like
        // Phase Lock) that keep per-instance state.
        let fft_size = self.fft_size;

        for chan in 0..self.num_channels {
            let block_size = input[chan].len();

            // Zero output before accumulating across instances
            for s in output[chan].iter_mut() {
                *s = 0.0;
            }

            // Each overlap instance processes the same input independently
            for (overlap_idx, inst) in self.instances[chan].iter_mut().enumerate() {
                // fill_in_passOut, one sample at a time (C++ lines 164-176):
                // write input at the instance offset, read the matching
                // output, fire the FFT the moment the buffer is exactly full.
                for i in 0..block_size {
                    let off = inst.offset;
                    if off < fft_size {
                        inst.input_buf[off] = input[chan][i];
                        output[chan][i] += inst.output_buf[off];
                    }

                    inst.offset += 1;
                    if inst.offset >= fft_size {
                        process_instance(inst, chan, overlap_idx, &*self.fft_fwd,
                            &*self.fft_inv, &self.window, fft_size, self.half_size,
                            callback);
                        inst.offset = 0;
                    }
                }
            }
        }
    }
}

/// Process one STFT frame for a single overlap instance.
/// Matches StandardFFTProcessor::process when offset >= fftSize.
fn process_instance<F>(
    inst: &mut StftInstance,
    chan: usize,
    overlap_idx: usize,
    fft_fwd: &dyn Fft<f32>,
    fft_inv: &dyn Fft<f32>,
    window: &[f32],
    fft_size: usize,
    half_size: usize,
    callback: &mut F,
) where
    F: FnMut(usize, usize, usize, &mut [Polar]),
{
    // ① applyWindow (line 24)
    for i in 0..fft_size {
        inst.input_buf[i] *= window[i];
    }

    // ② float2Cpx (line 28)
    for i in 0..fft_size {
        inst.fft_buf[i] = Complex::new(inst.input_buf[i], 0.0);
    }

    // ③ Forward FFT (line 30)
    fft_fwd.process_with_scratch(&mut inst.fft_buf, &mut inst.scratch_fwd);

    // ④ normalise — first halfSize bins × 2/N (lines 198-208)
    let scale = 2.0 / fft_size as f32;
    for c in inst.fft_buf[..half_size].iter_mut() {
        c.re *= scale;
        c.im *= scale;
    }

    // ⑤ car2Pol (utilities.cpp lines 17-22)
    for i in 0..half_size {
        inst.polar_buf[i] = Polar::from_complex(inst.fft_buf[i]);
    }

    // ⑥ User spectral callback
    callback(half_size, chan, overlap_idx, &mut inst.polar_buf);

    // ⑦ pol2Car — first half from polar, mirror zeroed (lines 24-40)
    for i in 0..half_size {
        inst.ifft_buf[i] = inst.polar_buf[i].to_complex();
    }
    for i in half_size..fft_size {
        inst.ifft_buf[i] = Complex::new(0.0, 0.0);
    }

    // ⑧ m_ifftin[0] = 0.f  — DEVIATION: omitted (see module doc)

    // ⑨ Inverse FFT (line 40) — kissfft inverse is unscaled, rustfft too
    fft_inv.process_with_scratch(&mut inst.ifft_buf, &mut inst.scratch_inv);

    // ⑩ cpx2Float (line 42)
    for i in 0..fft_size {
        inst.output_buf[i] = inst.ifft_buf[i].re;
    }

    // ⑪ Synthesis window (line 43)
    for i in 0..fft_size {
        inst.output_buf[i] *= window[i];
    }
}

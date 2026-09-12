//! Integration tests for the STFT spectral engine.
//!
//! Run with: cargo test -p akifx stft -- --nocapture

use akifx::stft::{SpectralConfig, SpectralEngine, WindowType};

// ---------------------------------------------------------------------------
// Test 1: Identity callback reconstructs signal within tolerance
// ---------------------------------------------------------------------------
#[test]
fn identity_reconstruction() {
    let fft_size = 512;
    let config = SpectralConfig {
        fft_size,
        overlap_count: 4,
        window: WindowType::Hann,
    };
    let mut engine = SpectralEngine::new(config, 1);
    let latency = engine.latency_samples();
    assert_eq!(latency, fft_size);

    // Feed a smooth signal (low-frequency sine) through identity callback
    let total = latency + fft_size * 4;
    let sample_rate = 44100.0f32;
    let freq = 100.0f32;
    let input: Vec<f32> = (0..total)
        .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / sample_rate).sin())
        .collect();
    let mut output = vec![0.0f32; total];

    engine.process(
        &[&input],
        &mut [&mut output],
        &mut |_num_bins, _chan, _overlap, _polar| { /* identity */ },
    );

    // Compare output to input (output is delayed by latency_samples)
    // output[n] ≈ gain * input[n - latency]
    let trim_start = latency;
    let trim_end = output.len() - fft_size;
    if trim_start >= trim_end {
        return;
    }
    let out_slice = &output[trim_start..trim_end];
    let in_slice = &input[0..out_slice.len()];

    let rms_out: f32 = (out_slice.iter().map(|s| s * s).sum::<f32>() / out_slice.len() as f32).sqrt();
    let rms_in: f32 = (in_slice.iter().map(|s| s * s).sum::<f32>() / in_slice.len() as f32).sqrt();

    if rms_in < 1e-10 || rms_out < 1e-10 {
        panic!("Signal energy too low for correlation test");
    }

    let gain = rms_out / rms_in;

    // RMSE of gain-normalized output vs input
    let mut sse = 0.0f32;
    for (a, b) in out_slice.iter().zip(in_slice.iter()) {
        let diff = a / gain - b;
        sse += diff * diff;
    }
    let rmse = (sse / out_slice.len() as f32).sqrt();

    assert!(
        rmse < 0.05,
        "Identity STFT RMSE {} too large (gain={})",
        rmse,
        gain
    );
}

// ---------------------------------------------------------------------------
// Test 2: Silence -> silence (< -120 dBFS residual)
// ---------------------------------------------------------------------------
#[test]
fn silence_remains_silent() {
    let config = SpectralConfig {
        fft_size: 512,
        overlap_count: 4,
        window: WindowType::Hann,
    };
    let mut engine = SpectralEngine::new(config, 2);

    let block_size = 256;
    let num_blocks = 16;

    let input = vec![0.0f32; block_size];
    let mut output0 = vec![0.0f32; block_size];
    let mut output1 = vec![0.0f32; block_size];

    for _ in 0..num_blocks {
        engine.process(
            &[&input, &input],
            &mut [&mut output0, &mut output1],
            &mut |_bins, _chan, _overlap, _pol| {},
        );
    }

    let max0 = output0.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let max1 = output1.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let max_abs = max0.max(max1);
    let dbfs = if max_abs > 0.0 {
        20.0 * max_abs.log10()
    } else {
        f32::NEG_INFINITY
    };
    assert!(
        dbfs < -120.0 || max_abs == 0.0,
        "silence residual {} dBFS should be < -120 dBFS",
        dbfs
    );
}

// ---------------------------------------------------------------------------
// Test 3: 440 Hz sine correlates >= 0.99 with input
// ---------------------------------------------------------------------------
#[test]
fn sine_440hz_correlation() {
    let fft_size = 1024;
    let config = SpectralConfig {
        fft_size,
        overlap_count: 4,
        window: WindowType::Hann,
    };
    let mut engine = SpectralEngine::new(config, 1);

    let sample_rate = 44100.0f32;
    let freq = 440.0f32;
    let block_size = 256;
    let num_blocks = 64;
    let latency = engine.latency_samples();

    let mut all_output = Vec::new();
    let mut sample_offset = 0usize;

    for _ in 0..num_blocks {
        let input: Vec<f32> = (0..block_size)
            .map(|i| {
                let t = (sample_offset + i) as f32 / sample_rate;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let mut output = vec![0.0f32; block_size];

        engine.process(
            &[&input],
            &mut [&mut output],
            &mut |_bins, _chan, _overlap, _pol| {},
        );

        all_output.extend_from_slice(&output);
        sample_offset += block_size;
    }

    let trim = latency + fft_size;
    let end = all_output.len().saturating_sub(fft_size);
    if trim >= end {
        return;
    }
    let out_slice = &all_output[trim..end];

    let ref_signal: Vec<f32> = (0..out_slice.len())
        .map(|i| {
            let t = (trim + i - latency) as f32 / sample_rate;
            (2.0 * std::f32::consts::PI * freq * t).sin()
        })
        .collect();

    let n = out_slice.len() as f32;
    let mean_out = out_slice.iter().sum::<f32>() / n;
    let mean_ref = ref_signal.iter().sum::<f32>() / n;

    let mut cov = 0.0f32;
    let mut var_out = 0.0f32;
    let mut var_ref = 0.0f32;
    for (a, b) in out_slice.iter().zip(ref_signal.iter()) {
        let da = a - mean_out;
        let db = b - mean_ref;
        cov += da * db;
        var_out += da * da;
        var_ref += db * db;
    }

    let corr = if var_out > 0.0 && var_ref > 0.0 {
        cov / (var_out * var_ref).sqrt()
    } else {
        0.0
    };

    assert!(
        corr >= 0.99,
        "440 Hz sine correlation {} should be >= 0.99",
        corr
    );
}

// ---------------------------------------------------------------------------
// Test 4: Window tables match closed-form values at sample points
// ---------------------------------------------------------------------------
#[test]
fn window_table_values() {
    // Hann: midpoint near 1.0 (C++ uses size-1 denominator, not exact)
    let hann = WindowType::Hann.generate(512);
    let mid = hann[256];
    assert!(
        (mid - 1.0).abs() < 1e-4,
        "Hann midpoint {} should be ~1.0",
        mid
    );
    assert!(hann[0].abs() < 1e-7, "Hann[0] = {} should be ~0", hann[0]);
    assert!(hann[511].abs() < 1e-7, "Hann[511] = {} should be ~0", hann[511]);

    // Blackman: midpoint is ~0.84 (not 1.0 — by formula: 0.42 + 0.5 - 0.08 = 0.84)
    let blackman = WindowType::Blackman.generate(512);
    let bm_mid = blackman[256];
    assert!(
        (bm_mid - 0.84).abs() < 1e-4,
        "Blackman midpoint {} should be ~0.84",
        bm_mid
    );

    // Hamming endpoints non-zero by design (~0.08)
    let hamming = WindowType::Hamming.generate(512);
    assert!(hamming[0] > 0.05 && hamming[0] < 0.12, "Hamming[0] = {}", hamming[0]);

    // BlackmanHarris midpoint ~ 0.977 (not 1.0 — by formula with size-1 denom)
    let bh = WindowType::BlackmanHarris.generate(512);
    assert!(
        (bh[256] - 0.977).abs() < 0.001,
        "BH midpoint {} should be ~0.977",
        bh[256]
    );
}

// ---------------------------------------------------------------------------
// Test 5: fft_size 512 / overlap 4 processes without allocation panic
//         across random blocks of varying size (128..1024)
// ---------------------------------------------------------------------------
#[test]
fn varying_block_sizes_no_panic() {
    let config = SpectralConfig {
        fft_size: 512,
        overlap_count: 4,
        window: WindowType::Hann,
    };
    let mut engine = SpectralEngine::new(config, 2);

    let mut rng_state: u32 = 12345;
    let mut next_block_size = || -> usize {
        rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        let range = 1024 - 128 + 1;
        (rng_state / 65536) as usize % range + 128
    };

    for _ in 0..50 {
        let block_size = next_block_size();
        let input: Vec<f32> = (0..block_size).map(|i| i as f32 * 0.001).collect();
        let mut out0 = vec![0.0f32; block_size];
        let mut out1 = vec![0.0f32; block_size];

        engine.process(
            &[&input, &input],
            &mut [&mut out0, &mut out1],
            &mut |num_bins, _chan, _overlap, polar| {
                for bin in polar.iter_mut().take(num_bins) {
                    bin.phase *= 1.0;
                }
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Regression: blocks that are not multiples of the hop size must not drop
// samples. The chunked implementation used to lose input/output samples
// whenever instance offsets overshot fft_size (e.g. 480-sample blocks with a
// 128-sample hop), producing periodic dropouts. With a DC-ish input and an
// identity callback, a correct engine reconstructs the constant everywhere —
// a dropped segment would appear as a run of (near-)zeros.
// ---------------------------------------------------------------------------
#[test]
fn non_hop_multiple_blocks_conserve_signal() {
    let fft_size = 512;
    let config = SpectralConfig {
        fft_size,
        overlap_count: 4,
        window: WindowType::Hann,
    };
    let mut engine = SpectralEngine::new(config, 2);

    let amplitude = 0.25f32;
    let block_size = 480; // not a multiple of hop = 128

    let mut min_mag = f32::INFINITY;
    let mut max_mag = 0.0f32;
    let mut saw_nan = false;

    // Two seconds' worth of blocks; skip the fft_size warmup.
    let mut produced: Vec<f32> = Vec::new();
    for _ in 0..200 {
        let input = vec![amplitude; block_size];
        let mut out0 = vec![0.0f32; block_size];
        let mut out1 = vec![0.0f32; block_size];
        engine.process(
            &[&input, &input],
            &mut [&mut out0, &mut out1],
            &mut |num_bins, _chan, _overlap, polar| {
                let _ = num_bins;
                let _ = polar;
                // identity
            },
        );
        produced.extend_from_slice(&out0);
    }

    for &sample in &produced[fft_size..] {
        saw_nan |= sample.is_nan();
        min_mag = min_mag.min(sample.abs());
        max_mag = max_mag.max(sample.abs());
    }

    assert!(!saw_nan, "no NaN in steady-state output");
    assert!(
        min_mag > 0.15,
        "steady-state output must stay near {amplitude} (min |out| = {min_mag});          near-zero runs indicate dropped samples"
    );
    assert!(
        max_mag < 1.0,
        "steady-state output must stay bounded (max |out| = {max_mag})"
    );
}

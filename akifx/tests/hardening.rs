//! Chain-hardening tests: stereo consistency, latency-aligned null
//! reconstruction through the spectral core, and extreme-parameter
//! stability. Companion to `full_chain.rs`.

use akifx::modules::spectral_common::SpectralFxCore;
use akifx::modules::AkiFxModule;
use akifx::AkiFxParams;

/// Noise-like test signal (deterministic, no external rng).
fn test_signal(n: usize, seed: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32;
            0.5 * (t * 0.043 + seed).sin()
                + 0.3 * (t * 0.0171 + seed * 2.1).cos()
                + 0.15 * (t * 0.089 + seed * 0.7).sin()
        })
        .collect()
}

/// Same signal on both channels through the fully enabled chain must come out
/// bit-identical on both channels — catches any shared-state / per-channel
/// divergence bug in the chain plumbing (guide: stereo consistency check).
#[test]
fn stereo_consistency_identical_inputs() {
    let params = AkiFxParams::default();
    let mut chain = akifx::create_default_chain(&params);
    for module in chain.modules_mut().iter_mut() {
        module.set_bypass(false);
    }
    chain.initialize_all(44100.0, 512);

    // Several blocks, identical input on both channels
    for block in 0..6 {
        let mut left = test_signal(512, block as f32 * 1.7);
        let mut right = test_signal(512, block as f32 * 1.7);
        chain.process(&mut left, &mut right);
        for i in 0..512 {
            assert_eq!(
                left[i].to_bits(),
                right[i].to_bits(),
                "block {block} sample {i}: channels diverged ({} vs {})",
                left[i],
                right[i]
            );
        }
    }
}

/// The spectral core must reconstruct its input when the per-bin callback
/// does nothing. The engine REPORTS fft+hop latency (the conservative value
/// hosts use for PDC) while the true signal delay is fft_size — the null is
/// taken at the true delay. This is the latency-aligned null test.
#[test]
fn latency_aligned_null_spectral_core() {
    let mut core = SpectralFxCore::new();
    core.build_engine();
    core.build_if_missing();

    // Reported-latency contract: fft (2048) + hop (512), conservative.
    assert_eq!(
        core.latency_samples(),
        (akifx::modules::spectral_common::SPECTRAL_FFT_SIZE
            + akifx::modules::spectral_common::SPECTRAL_FFT_SIZE / 4) as u64
    );
    let true_delay = akifx::modules::spectral_common::SPECTRAL_FFT_SIZE;

    let identity = &mut |_: usize, _: usize, _: usize, _: &mut [akifx::stft::Polar]| {};

    let block_size = 512;
    let blocks = 12;
    let total = block_size * blocks;

    let input = test_signal(total, 0.42);
    let mut output = input.clone();
    for chunk in output.chunks_mut(block_size) {
        let mut l = chunk.to_vec();
        let mut r = vec![0.0f32; chunk.len()];
        core.process(&mut l, &mut r, identity);
        chunk.copy_from_slice(&l);
    }

    // output[t] ≈ gain · input[t - true_delay] over the steady region (both
    // edges trimmed by one fft). The engine's OLA normalization is a fixed
    // scale factor, measured as an RMS ratio exactly like the engine-level
    // identity test; the null is the gain-normalized RMSE.
    let trim_start = true_delay;
    let trim_end = total - true_delay;
    let out_slice = &output[trim_start..trim_end];
    let in_slice = &input[0..out_slice.len()];

    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    let rms_out = rms(out_slice);
    let rms_in = rms(in_slice);
    assert!(rms_in > 1e-6 && rms_out > 1e-6, "test signal energy too low");

    let gain = rms_out / rms_in;
    let mut sse = 0.0f32;
    for (a, b) in out_slice.iter().zip(in_slice.iter()) {
        let d = a / gain - b;
        sse += d * d;
    }
    let rmse = (sse / out_slice.len() as f32).sqrt();
    println!(
        "latency-aligned null: reported latency={}, true_delay={true_delay}, scale={gain:.4}, rmse={rmse:.6}",
        core.latency_samples()
    );
    assert!(
        rmse < 0.05,
        "identity callback did not reconstruct input at the true delay (rmse {rmse})"
    );
}

/// Extreme parameter settings must stay finite and bounded. The master gain
/// is the one module whose extremes are constructible from outside nih-plug
/// (params themselves are not writable outside a host); ±full-scale boosts
/// exercise the loudest path through the output chain.
#[test]
fn extreme_gain_settings_stay_finite() {
    use akifx::modules::gain::GainModule;

    for db in [-30.0f32, 30.0] {
        let mut module = GainModule::with_db(db);
        module.initialize(44100.0, 512);
        let mut left = test_signal(512, 0.9);
        let mut right = test_signal(512, 0.9);
        module.process(&mut left, &mut right);
        for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert!(!l.is_nan() && !r.is_nan(), "sample {i} NaN at {db} dB");
            assert!(
                l.is_finite() && r.is_finite(),
                "sample {i} non-finite at {db} dB"
            );
        }
    }
}

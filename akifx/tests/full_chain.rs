//! Integration test for the full AkiFX module chain.
//!
//! Verifies that all 21 modules are wired correctly, bypass works,
//! gain works, and the entire active chain produces no NaN.

use akifx::modules::ModuleChain;
use akifx::AkiFxParams;
use nih_plug::prelude::*;
use std::sync::Arc;

/// Build the real module set and return (chain, params).
fn build_chain() -> (ModuleChain, Arc<AkiFxParams>) {
    let params = Arc::new(AkiFxParams::default());
    let chain = akifx::create_default_chain(&params);
    (chain, params)
}

// ---------------------------------------------------------------------------
// Core structural tests
// ---------------------------------------------------------------------------

#[test]
fn chain_contains_21_modules() {
    let (chain, _params) = build_chain();
    assert_eq!(chain.len(), 21, "Expected exactly 21 modules in the chain");
}

#[test]
fn all_bypassed_produces_bit_identical_passthrough() {
    let (mut chain, params) = build_chain();

    // Bypass every module
    for module in chain.modules_mut().iter() {
        module.set_bypass(true);
    }

    let block_size = 512;
    let mut left: Vec<f32> = (0..block_size).map(|i| (i as f32 * 0.01).sin()).collect();
    let mut right: Vec<f32> = (0..block_size).map(|i| (i as f32 * 0.01).cos()).collect();
    let input_left = left.clone();
    let input_right = right.clone();

    chain.process(&mut left, &mut right);

    for (i, (actual, expected)) in left.iter().zip(input_left.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Left sample {i}: bypassed chain modified audio ({expected} -> {actual})"
        );
    }
    for (i, (actual, expected)) in right.iter().zip(input_right.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Right sample {i}: bypassed chain modified audio ({expected} -> {actual})"
        );
    }
    let _ = params; // keep alive
}

#[test]
fn master_gain_minus_6db_halves_amplitude() {
    // Verify gain module works by creating a chain with gain-only processing.
    // We can't set param values from outside nih-plug (set_plain_value is pub(crate)),
    // so we test via a dedicated GainModule with the desired dB value.
    use akifx::modules::gain::GainModule;
    use akifx::modules::AkiFxModule;

    let mut gain_module = GainModule::with_db(-6.0);
    gain_module.initialize(44100.0, 512);

    let block_size = 256;
    let mut left = vec![1.0f32; block_size];
    let mut right = vec![1.0f32; block_size];

    gain_module.process(&mut left, &mut right);

    // -6 dB ≈ 0.501187... linear
    let expected = nih_plug::util::db_to_gain(-6.0);
    for (i, sample) in left.iter().enumerate() {
        assert!(
            (sample - expected).abs() < 1e-4,
            "Left sample {i}: expected ~{expected}, got {sample}"
        );
    }
    for (i, sample) in right.iter().enumerate() {
        assert!(
            (sample - expected).abs() < 1e-4,
            "Right sample {i}: expected ~{expected}, got {sample}"
        );
    }
}

#[test]
fn no_nan_through_active_chain_with_noise() {
    let (mut chain, _params) = build_chain();

    // T2: defaults all-off — enable every module for the active-chain test.
    for module in chain.modules_mut().iter_mut() {
        module.set_bypass(false);
    }

    // Initialize all modules before processing (required for modules with internal buffers)
    chain.initialize_all(44100.0, 512);

    // All modules active with default params (neutral-ish settings)
    let block_size = 512;
    let mut left: Vec<f32> = (0..block_size)
        .map(|i| ((i as f32 * 0.1).sin() * 0.5 + (i as f32 * 0.037).cos() * 0.3))
        .collect();
    let mut right: Vec<f32> = (0..block_size)
        .map(|i| ((i as f32 * 0.13).sin() * 0.4 + (i as f32 * 0.041).cos() * 0.35))
        .collect();

    chain.process(&mut left, &mut right);

    for (i, sample) in left.iter().enumerate() {
        assert!(
            !sample.is_nan(),
            "Left sample {i} is NaN through active chain"
        );
    }
    for (i, sample) in right.iter().enumerate() {
        assert!(
            !sample.is_nan(),
            "Right sample {i} is NaN through active chain"
        );
    }
}

#[test]
fn latency_sums_non_bypassed_modules() {
    let (mut chain, _params) = build_chain();

    // T2: defaults all-off — enable all modules so latency is measurable.
    for module in chain.modules_mut().iter_mut() {
        module.set_bypass(false);
    }

    // PubertySimulator has 2048 samples latency — all others are 0.
    // Latency is the sum of non-bypassed modules (= 2048 in identity order).
    let latency = chain.latency_samples();
    assert!(
        latency >= 2048,
        "Expected chain latency >= 2048 (PubertySimulator), got {latency}"
    );
}

#[test]
fn initialize_and_reset_do_not_panic() {
    let (mut chain, _params) = build_chain();
    chain.initialize_all(44100.0, 512);
    chain.reset_all();
    // Verify we can still process after reset
    let mut left = vec![0.5f32; 256];
    let mut right = vec![0.5f32; 256];
    chain.process(&mut left, &mut right);
}

#[test]
fn module_names_match_expected() {
    let (chain, _params) = build_chain();
    let names: Vec<&str> = chain.modules().iter().map(|m| m.name()).collect();

    // Check that key modules are present
    assert!(names.contains(&"Sine Generator"), "Missing Sine Generator");
    assert!(names.contains(&"MIDI Inverter"), "Missing MIDI Inverter");
    assert!(names.contains(&"Gain"), "Missing Gain");
    assert!(names.contains(&"Safety Limiter"), "Missing Safety Limiter");
    assert!(
        names.contains(&"Spectral Compressor"),
        "Missing Spectral Compressor"
    );
    assert!(names.contains(&"Puberty Simulator"), "Missing Puberty Simulator");
    assert!(
        names.contains(&"Poly Mod Synth"),
        "Missing Poly Mod Synth"
    );
    assert!(names.contains(&"Playground"), "Missing Playground");
}

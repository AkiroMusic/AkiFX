//! Tests for the module chain abstraction.
//!
//! These tests use mock modules to verify chain behavior, then test the real GainModule.

use akifx::modules::{AkiFxModule, ModuleChain};
use akifx::modules::gain::GainModule;
use nih_plug::prelude::Params;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Mock modules for chain behaviour tests
// ---------------------------------------------------------------------------

/// Records the order in which modules process.
struct OrderTracker {
    id: usize,
    order_log: Arc<AtomicUsize>,
    params: OrderParams,
    bypass: Arc<AtomicBool>,
}

#[derive(Params)]
struct OrderParams {
    #[id = "dummy"]
    dummy: nih_plug::prelude::FloatParam,
}

impl Default for OrderParams {
    fn default() -> Self {
        Self {
            dummy: nih_plug::prelude::FloatParam::new("Dummy", 0.0, nih_plug::prelude::FloatRange::Linear { min: 0.0, max: 1.0 }),
        }
    }
}

impl OrderTracker {
    fn new(id: usize, order_log: Arc<AtomicUsize>) -> Self {
        Self {
            id,
            order_log,
            params: OrderParams::default(),
            bypass: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AkiFxModule for OrderTracker {
    fn name(&self) -> &'static str {
        "OrderTracker"
    }

    fn params(&self) -> &dyn Params {
        &self.params
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn reset(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Record our position in the processing order
        self.order_log.store(self.id, Ordering::SeqCst);
        // Apply a trivial transform: add our id as f32 to mark we were here
        let marker = self.id as f32;
        for sample in left.iter_mut() {
            *sample += marker;
        }
        for sample in right.iter_mut() {
            *sample += marker;
        }
    }
}

/// A mock module that multiplies audio by a fixed gain value.
struct MockGain {
    gain: f32,
    params: MockGainParams,
    bypass: Arc<AtomicBool>,
}

#[derive(Params)]
struct MockGainParams {
    #[id = "mgain"]
    gain: nih_plug::prelude::FloatParam,
}

impl Default for MockGainParams {
    fn default() -> Self {
        Self {
            gain: nih_plug::prelude::FloatParam::new("MockGain", 1.0, nih_plug::prelude::FloatRange::Linear { min: 0.0, max: 10.0 }),
        }
    }
}

impl MockGain {
    fn new(gain: f32) -> Self {
        Self {
            gain,
            params: MockGainParams::default(),
            bypass: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AkiFxModule for MockGain {
    fn name(&self) -> &'static str {
        "MockGain"
    }

    fn params(&self) -> &dyn Params {
        &self.params
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn reset(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for sample in left.iter_mut() {
            *sample *= self.gain;
        }
        for sample in right.iter_mut() {
            *sample *= self.gain;
        }
    }
}

/// A mock module that introduces latency.
struct MockLatency {
    latency: u64,
    params: MockLatencyParams,
    bypass: Arc<AtomicBool>,
}

#[derive(Params)]
struct MockLatencyParams {
    #[id = "mlat"]
    dummy: nih_plug::prelude::FloatParam,
}

impl Default for MockLatencyParams {
    fn default() -> Self {
        Self {
            dummy: nih_plug::prelude::FloatParam::new("Dummy", 0.0, nih_plug::prelude::FloatRange::Linear { min: 0.0, max: 1.0 }),
        }
    }
}

impl MockLatency {
    fn new(latency: u64) -> Self {
        Self {
            latency,
            params: MockLatencyParams::default(),
            bypass: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AkiFxModule for MockLatency {
    fn name(&self) -> &'static str {
        "MockLatency"
    }

    fn params(&self) -> &dyn Params {
        &self.params
    }

    fn bypass_flag(&self) -> &Arc<AtomicBool> {
        &self.bypass
    }

    fn initialize(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn reset(&mut self) {}

    fn latency_samples(&self) -> u64 {
        self.latency
    }

    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn chain_processes_modules_in_insertion_order() {
    let order_log = Arc::new(AtomicUsize::new(0));
    let mut chain = ModuleChain::new();
    chain.push(Box::new(OrderTracker::new(1, order_log.clone())));
    chain.push(Box::new(OrderTracker::new(2, order_log.clone())));
    chain.push(Box::new(OrderTracker::new(3, order_log.clone())));

    let mut left = vec![0.0; 4];
    let mut right = vec![0.0; 4];
    chain.process(&mut left, &mut right);

    // The last module to process stores its id; should be 3 (last inserted)
    assert_eq!(order_log.load(Ordering::SeqCst), 3);

    // Verify the accumulated markers: 1 + 2 + 3 = 6.0
    for sample in &left {
        assert!((sample - 6.0).abs() < 1e-6, "Expected 6.0, got {sample}");
    }
    for sample in &right {
        assert!((sample - 6.0).abs() < 1e-6, "Expected 6.0, got {sample}");
    }
}

#[test]
fn bypassed_module_produces_bit_identical_passthrough() {
    let mut chain = ModuleChain::new();
    let gain_module = MockGain::new(2.0);
    gain_module.bypass.store(true, Ordering::SeqCst);
    chain.push(Box::new(gain_module));

    let input_left = vec![0.5, -0.5, 1.0, 0.0];
    let input_right = vec![0.25, -0.25, 0.75, 0.0];
    let mut left = input_left.clone();
    let mut right = input_right.clone();
    chain.process(&mut left, &mut right);

    // Bit-identical: bypassed module must not modify audio
    for (actual, expected) in left.iter().zip(input_left.iter()) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Bypassed module modified audio: expected {expected}, got {actual}"
        );
    }
    for (actual, expected) in right.iter().zip(input_right.iter()) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Bypassed module modified audio: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn two_chained_gains_multiply() {
    let mut chain = ModuleChain::new();
    chain.push(Box::new(MockGain::new(2.0)));
    chain.push(Box::new(MockGain::new(3.0)));

    let mut left = vec![1.0; 4];
    let mut right = vec![1.0; 4];
    chain.process(&mut left, &mut right);

    // 1.0 * 2.0 * 3.0 = 6.0
    for sample in &left {
        assert!((sample - 6.0).abs() < 1e-6, "Expected 6.0, got {sample}");
    }
    for sample in &right {
        assert!((sample - 6.0).abs() < 1e-6, "Expected 6.0, got {sample}");
    }
}

#[test]
fn latency_aggregation_returns_sum() {
    let mut chain = ModuleChain::new();
    chain.push(Box::new(MockLatency::new(0)));
    chain.push(Box::new(MockLatency::new(128)));
    chain.push(Box::new(MockLatency::new(64)));

    // Latency is now the SUM of all non-bypassed modules (series-correct).
    // 0 + 128 + 64 = 192
    assert_eq!(chain.latency_samples(), 192);
}

#[test]
fn empty_chain_latency_is_zero() {
    let chain = ModuleChain::new();
    assert_eq!(chain.latency_samples(), 0);
}

#[test]
fn reset_all_resets_all_modules() {
    // Verify that calling reset_all doesn't panic and runs without error
    let mut chain = ModuleChain::new();
    chain.push(Box::new(MockGain::new(2.0)));
    chain.push(Box::new(MockGain::new(3.0)));
    chain.reset_all();
}

#[test]
fn initialize_all_runs_on_all_modules() {
    let mut chain = ModuleChain::new();
    chain.push(Box::new(MockGain::new(2.0)));
    chain.push(Box::new(MockGain::new(3.0)));
    chain.initialize_all(44100.0, 512);
}

#[test]
fn gain_module_applies_gain_in_place() {
    let mut gain_module = GainModule::with_db(6.0);
    gain_module.initialize(44100.0, 512);

    let input_left = vec![1.0, 0.5, -0.5];
    let input_right = vec![0.25, -0.25, 1.0];
    let mut left = input_left.clone();
    let mut right = input_right.clone();
    gain_module.process(&mut left, &mut right);

    // Gain of 6.0 dB ≈ 1.99526... linear
    let expected_gain = nih_plug::util::db_to_gain(6.0);
    for (actual, input) in left.iter().zip(input_left.iter()) {
        let expected = input * expected_gain;
        assert!(
            (actual - expected).abs() < 1e-4,
            "Expected {expected}, got {actual}"
        );
    }
    for (actual, input) in right.iter().zip(input_right.iter()) {
        let expected = input * expected_gain;
        assert!(
            (actual - expected).abs() < 1e-4,
            "Expected {expected}, got {actual}"
        );
    }
}

#[test]
fn gain_module_bypass_via_chain() {
    // Bypass is a chain concern: the chain checks is_bypassed() and skips process().
    // Test that the chain correctly skips a bypassed GainModule.
    let mut chain = ModuleChain::new();
    let gain_module = GainModule::with_db(12.0);
    gain_module.set_bypass(true);
    chain.push(Box::new(gain_module));

    let input_left = vec![0.5, -0.5, 1.0];
    let input_right = vec![0.25, -0.25, 0.75];
    let mut left = input_left.clone();
    let mut right = input_right.clone();
    chain.process(&mut left, &mut right);

    // Bypassed module → bit-identical passthrough
    for (actual, expected) in left.iter().zip(input_left.iter()) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Bypassed GainModule modified audio"
        );
    }
    for (actual, expected) in right.iter().zip(input_right.iter()) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "Bypassed GainModule modified audio"
        );
    }
}

#[test]
fn gain_module_doubles_amplitude() {
    let params = Arc::new(akifx::modules::gain::GainParams::new(
        20.0 * 2.0f32.log10(),
    ));
    let bypass = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut module = akifx::modules::gain::GainModule::new(params, bypass);
    module.initialize(44100.0, 512);

    let mut left: Vec<f32> = vec![0.25, -0.5, 0.75, -1.0];
    let mut right: Vec<f32> = vec![0.25, -0.5, 0.75, -1.0];
    module.process(&mut left, &mut right);

    let expected: Vec<f32> = vec![0.5, -1.0, 1.5, -2.0];
    for (actual, exp) in left.iter().zip(&expected) {
        let exp: f32 = *exp;
        let actual: f32 = *actual;
        assert!((actual - exp).abs() < 1e-6, "Expected {exp}, got {actual}");
    }
    for (actual, exp) in right.iter().zip(&expected) {
        let exp: f32 = *exp;
        let actual: f32 = *actual;
        assert!((actual - exp).abs() < 1e-6, "Expected {exp}, got {actual}");
    }
}

#[test]
fn chain_is_empty_by_default() {
    let chain = ModuleChain::new();
    assert!(chain.is_empty());
    assert_eq!(chain.len(), 0);
}

#[test]
fn chain_len_reflects_pushed_modules() {
    let mut chain = ModuleChain::new();
    chain.push(Box::new(MockGain::new(1.0)));
    chain.push(Box::new(MockGain::new(2.0)));
    chain.push(Box::new(MockGain::new(3.0)));
    assert_eq!(chain.len(), 3);
    assert!(!chain.is_empty());
}

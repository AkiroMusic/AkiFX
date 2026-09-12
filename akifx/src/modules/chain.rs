//! Processing chain that owns and iterates over modules.
//!
//! The chain supports runtime-reorderable module processing via [`SharedOrder`].
//! A `SharedOrder` is an `Arc<Mutex<Vec<usize>>>` shared between the audio thread
//! (which reads the order during processing) and the GUI/init thread (which writes
//! new orders via [`ModuleChain::set_order`]). The mutex hold time on the audio
//! thread is limited to cloning a `Vec<usize>` — microseconds.

use super::AkiFxModule;

/// Shared processing order: a permutation of module indices `[0, len)`.
///
/// The audio thread clones the inner `Vec` under the lock (microseconds),
/// then processes modules in the cloned order without holding the lock.
pub type SharedOrder = std::sync::Arc<parking_lot::Mutex<Vec<usize>>>;

/// A sequential chain of audio effect modules.
///
/// Modules are processed in the order given by the current [`SharedOrder`].
/// By default this is the identity permutation (insertion order). The chain
/// manages bypass checks, latency aggregation, and lifecycle calls
/// (`initialize_all`, `reset_all`).
///
/// # Thread Safety
///
/// `ModuleChain` is **not** `Sync`. It is designed to be owned by a single
/// thread (the audio thread). The GUI accesses individual modules' bypass flags
/// via `Arc<AtomicBool>` shared through the module trait, and the processing
/// order via the [`SharedOrder`] mutex.
pub struct ModuleChain {
    modules: Vec<Box<dyn AkiFxModule>>,
    /// Current processing order. `order[i]` is the index of the module
    /// that processes audio in position `i`.
    order: SharedOrder,
    /// Audio-thread local copy of the order. Refreshed from `order` only
    /// when the shared order actually differs, so steady-state processing
    /// performs a lock + element-wise compare instead of a Vec clone.
    cached_order: Vec<usize>,
}

impl ModuleChain {
    /// Create an empty processing chain with identity order.
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            order: std::sync::Arc::new(parking_lot::Mutex::new(Vec::new())),
            cached_order: Vec::new(),
        }
    }

    /// Append a module to the end of the chain.
    ///
    /// Processing order matches insertion order: the first module pushed
    /// processes audio first (when using the identity permutation).
    pub fn push(&mut self, module: Box<dyn AkiFxModule>) {
        self.modules.push(module);
        // Extend identity order to include the new module.
        let idx = self.modules.len() - 1;
        self.order.lock().push(idx);
        self.cached_order.push(idx);
    }

    /// Replace the processing order.
    ///
    /// The `order` vector must:
    /// - have the same length as the chain
    /// - be a valid permutation of `0..len()` (each index appears exactly once)
    ///
    /// If validation fails the order is left unchanged and `()` is returned.
    pub fn set_order(&self, order: Vec<usize>) {
        let len = self.modules.len();
        if order.len() != len {
            return;
        }
        // Validate permutation: every index 0..len must appear exactly once.
        let mut seen = vec![false; len];
        for &idx in &order {
            if idx >= len || seen[idx] {
                return;
            }
            seen[idx] = true;
        }
        *self.order.lock() = order;
    }

    /// Whether the chain contains no modules.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Number of modules in the chain.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Process audio through all non-bypassed modules in the current order.
    ///
    /// Each module receives the same stereo buffer and mutates it in place.
    /// Bypassed modules are skipped entirely (bit-identical passthrough).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_with_midi(left, right, &[]);
    }

    /// Same as [`ModuleChain::process()`] but forwards MIDI note events to
    /// every active module (each module decides whether it uses them).
    ///
    /// The shared order is compared against the audio thread's cached copy
    /// under the lock (no allocation); the cached copy is refreshed only
    /// when the GUI/init thread actually changed the order.
    pub fn process_with_midi(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        note_events: &[nih_plug::midi::NoteEvent<()>],
    ) {
        // Refresh the local order cache if the shared order changed. The
        // lock is held only for the (allocation-free) comparison.
        {
            let shared = self.order.lock();
            if shared.len() != self.cached_order.len()
                || shared.iter().ne(self.cached_order.iter())
            {
                self.cached_order.clear();
                self.cached_order.extend_from_slice(&shared);
            }
        }

        for &idx in &self.cached_order {
            // Defensive: skip invalid indices.
            if let Some(module) = self.modules.get_mut(idx) {
                if !module.is_bypassed() {
                    module.process_with_midi(left, right, note_events);
                }
            }
        }
    }

    /// Sum of latencies of non-bypassed modules in current order.
    ///
    /// For series-cascaded modules, the total latency is the *sum* of
    /// individual latencies: each module's latency adds to the pipeline
    /// delay. Currently only `PubertySimulator` introduces latency (2048),
    /// so this equals 2048 when active and 0 when bypassed — same as the
    /// old `max()` semantics in the single-latency case, but correct for
    /// the general multi-latency future.
    pub fn latency_samples(&self) -> u64 {
        // Sum under the lock (no clone/allocation; the lock is uncontended
        // in practice and held for a handful of atomic loads).
        let order = self.order.lock();
        order
            .iter()
            .filter_map(|&idx| self.modules.get(idx))
            .filter(|m| !m.is_bypassed())
            .map(|m| m.latency_samples())
            .sum()
    }

    /// Call `initialize()` on every module in the chain.
    pub fn initialize_all(&mut self, sample_rate: f32, max_block_size: usize) {
        for module in self.modules.iter_mut() {
            module.initialize(sample_rate, max_block_size);
        }
    }

    /// Call `reset()` on every module in the chain.
    pub fn reset_all(&mut self) {
        for module in self.modules.iter_mut() {
            module.reset();
        }
    }

    /// Iterate over modules immutably (for GUI parameter access, etc.).
    pub fn modules(&self) -> &[Box<dyn AkiFxModule>] {
        &self.modules
    }

    /// Iterate over modules mutably.
    pub fn modules_mut(&mut self) -> &mut [Box<dyn AkiFxModule>] {
        &mut self.modules
    }

    /// Access the shared processing order (for GUI drag-to-reorder, etc.).
    pub fn order(&self) -> &SharedOrder {
        &self.order
    }
}

impl Default for ModuleChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nih_plug::prelude::Params;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Mock module that records invocation order via a shared atomic counter.
    struct OrderRecorder {
        id: usize,
        counter: Arc<AtomicUsize>,
        params: DummyParams,
        bypass: Arc<AtomicBool>,
    }

    #[derive(Params)]
    struct DummyParams {
        #[id = "dum"]
        dummy: nih_plug::prelude::FloatParam,
    }

    impl Default for DummyParams {
        fn default() -> Self {
            Self {
                dummy: nih_plug::prelude::FloatParam::new(
                    "Dummy",
                    0.0,
                    nih_plug::prelude::FloatRange::Linear {
                        min: 0.0,
                        max: 1.0,
                    },
                ),
            }
        }
    }

    impl OrderRecorder {
        fn new(id: usize, counter: Arc<AtomicUsize>) -> Self {
            Self {
                id,
                counter,
                params: DummyParams::default(),
                bypass: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl AkiFxModule for OrderRecorder {
        fn name(&self) -> &'static str {
            "OrderRecorder"
        }
        fn params(&self) -> &dyn Params {
            &self.params
        }
        fn bypass_flag(&self) -> &Arc<AtomicBool> {
            &self.bypass
        }
        fn initialize(&mut self, _: f32, _: usize) {}
        fn reset(&mut self) {}
        fn process(&mut self, _: &mut [f32], _: &mut [f32]) {
            self.counter.store(self.id, Ordering::SeqCst);
        }
    }

    /// Mock module that introduces latency.
    struct LatencyModule {
        latency: u64,
        params: DummyParams,
        bypass: Arc<AtomicBool>,
    }

    impl LatencyModule {
        fn new(latency: u64) -> Self {
            Self {
                latency,
                params: DummyParams::default(),
                bypass: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl AkiFxModule for LatencyModule {
        fn name(&self) -> &'static str {
            "LatencyModule"
        }
        fn params(&self) -> &dyn Params {
            &self.params
        }
        fn bypass_flag(&self) -> &Arc<AtomicBool> {
            &self.bypass
        }
        fn initialize(&mut self, _: f32, _: usize) {}
        fn reset(&mut self) {}
        fn latency_samples(&self) -> u64 {
            self.latency
        }
        fn process(&mut self, _: &mut [f32], _: &mut [f32]) {}
    }

    // ── Reorder tests ────────────────────────────────────────────────────

    #[test]
    fn reordered_chain_processes_in_permuted_order() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut chain = ModuleChain::new();
        // Push modules 0, 1, 2 in insertion order.
        chain.push(Box::new(OrderRecorder::new(0, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(1, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(2, counter.clone())));

        // Reverse order: [2, 1, 0].
        chain.set_order(vec![2, 1, 0]);

        let mut left = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        chain.process(&mut left, &mut right);

        // The last module to run stores its id; with reversed order, module 0
        // runs last (index 2 in the permuted order).
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn identity_order_processes_in_insertion_order() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut chain = ModuleChain::new();
        chain.push(Box::new(OrderRecorder::new(10, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(20, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(30, counter.clone())));

        // No set_order — identity is the default.
        let mut left = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        chain.process(&mut left, &mut right);

        // Last module to run is 30 (insertion order = identity).
        assert_eq!(counter.load(Ordering::SeqCst), 30);
    }

    #[test]
    fn set_order_with_invalid_length_is_ignored() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut chain = ModuleChain::new();
        chain.push(Box::new(OrderRecorder::new(0, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(1, counter.clone())));

        // Too short.
        chain.set_order(vec![1]);
        let mut left = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        chain.process(&mut left, &mut right);

        // Still identity — module 1 runs last.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn set_order_with_duplicate_index_is_ignored() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut chain = ModuleChain::new();
        chain.push(Box::new(OrderRecorder::new(0, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(1, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(2, counter.clone())));

        // Duplicate index 0.
        chain.set_order(vec![0, 0, 1]);
        let mut left = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        chain.process(&mut left, &mut right);

        // Still identity — module 2 runs last.
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn set_order_with_out_of_bounds_index_is_ignored() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut chain = ModuleChain::new();
        chain.push(Box::new(OrderRecorder::new(0, counter.clone())));
        chain.push(Box::new(OrderRecorder::new(1, counter.clone())));

        // Index 5 is out of bounds.
        chain.set_order(vec![5, 0]);
        let mut left = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        chain.process(&mut left, &mut right);

        // Still identity — module 1 runs last.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ── Latency sum tests ────────────────────────────────────────────────

    #[test]
    fn latency_sums_active_modules() {
        let mut chain = ModuleChain::new();
        chain.push(Box::new(LatencyModule::new(100)));
        chain.push(Box::new(LatencyModule::new(200)));
        chain.push(Box::new(LatencyModule::new(0)));

        assert_eq!(chain.latency_samples(), 300);
    }

    #[test]
    fn latency_excludes_bypassed_modules() {
        let mut chain = ModuleChain::new();
        chain.push(Box::new(LatencyModule::new(100)));
        let m2 = LatencyModule::new(200);
        m2.set_bypass(true);
        chain.push(Box::new(m2));
        chain.push(Box::new(LatencyModule::new(50)));

        // 100 + 0 (bypassed) + 50 = 150
        assert_eq!(chain.latency_samples(), 150);
    }

    #[test]
    fn latency_zero_for_empty_chain() {
        let chain = ModuleChain::new();
        assert_eq!(chain.latency_samples(), 0);
    }

    /// The plugin re-reports latency to the host from process(); this test
    /// verifies the chain's value actually moves when a module is toggled,
    /// which is what keeps DAW delay compensation in sync.
    #[test]
    fn latency_updates_when_module_toggles() {
        let mut chain = ModuleChain::new();
        chain.push(Box::new(LatencyModule::new(100)));
        chain.push(Box::new(LatencyModule::new(2048)));

        assert_eq!(chain.latency_samples(), 2148);

        // Toggle the high-latency module off via its shared flag.
        chain.modules_mut()[1]
            .bypass_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(chain.latency_samples(), 100);

        // And back on.
        chain.modules_mut()[1]
            .bypass_flag()
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(chain.latency_samples(), 2148);
    }

    #[test]
    fn latency_all_bypassed_is_zero() {
        let mut chain = ModuleChain::new();
        let m1 = LatencyModule::new(100);
        m1.set_bypass(true);
        chain.push(Box::new(m1));
        let m2 = LatencyModule::new(200);
        m2.set_bypass(true);
        chain.push(Box::new(m2));

        assert_eq!(chain.latency_samples(), 0);
    }
}

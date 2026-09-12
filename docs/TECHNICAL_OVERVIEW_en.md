# AkiFX Technical Overview

**Version 0.2.2** · For developers and contributors

This document describes the complete inner workings of AkiFX: framework choices, the module system, the STFT engine, the DSP algorithms behind each effect, host integration, the real-time safety model, and the testing strategy. Everything documented here has been calibrated line-by-line against the upstream sources (SpectralSuite @ `5dfd294`, nih-plug @ `f36931f`).

---

## Table of Contents

1. [Tech Stack & Project Layout](#1-tech-stack--project-layout)
2. [Module System & Signal Chain](#2-module-system--signal-chain)
3. [The STFT Engine](#3-the-stft-engine)
4. [DSP Implementation Notes per Effect](#4-dsp-implementation-notes-per-effect)
5. [Parameter System & State Persistence](#5-parameter-system--state-persistence)
6. [Host Integration (VST3/CLAP/Standalone)](#6-host-integration)
7. [Real-Time Safety Model](#7-real-time-safety-model)
8. [GUI Implementation](#8-gui-implementation)
9. [Testing & Verification Strategy](#9-testing--verification-strategy)

---

## 1. Tech Stack & Project Layout

| Component | Choice | Notes |
|---|---|---|
| Language | Rust 2021 (stable, MSVC) | No GC, no runtime on the audio thread |
| Plugin framework | [nih-plug](https://github.com/robbert-vdh/nih-plug) @ `f36931f` (**rev-pinned**) | VST3 + CLAP export, standalone (WASAPI/JACK), params derive macro |
| GUI | nih-plug-egui (egui 0.31 + baseview) | Parented into the host-provided window |
| FFT | rustfft (complex FFT, SpectralSuite engine) + realfft (real FFT, Puberty/Spectral Compressor) | Both lazily planned |
| Other | parking_lot (locks), rand/StdRng (Bin Scrambler), atomic_float (oversampling-aware smoothing) | |

```
Cargo.toml (workspace: akifx + xtask)
akifx/
  src/
    lib.rs            # nih_plug::Plugin impl, parameter tree, chain build, MIDI routing
    main.rs           # standalone entry point (nih_export_standalone)
    modules/          # 21 modules + chain.rs + mod.rs (trait definition)
      spectral_compressor/   # multi-file module (bank/curve/mixer/analyzer)
    stft/             # SpectralSuite STFT engine (engine/window/polar)
    gui/              # egui editor (mod/state/theme/descriptions)
  assets/fonts/       # Inter ×2, Noto Sans SC (CJK fallback), Cormorant Garamond
xtask/                # nih_plug_xtask bundler (cargo xtask bundle)
```

`lib.rs` exports `nih_export_vst3!` / `nih_export_clap!`; `main.rs` exports the standalone — the canonical nih-plug layout: `[lib] crate-type = ["cdylib", "lib"]`, with the host loading the cdylib and the standalone binary linking the rlib.

---

## 2. Module System & Signal Chain

### 2.1 The `AkiFxModule` trait (`modules/mod.rs`)

```rust
pub trait AkiFxModule: Send {
    fn name(&self) -> &'static str;
    fn params(&self) -> &dyn Params;
    fn bypass_flag(&self) -> &Arc<AtomicBool>;     // chain-level bypass (shared with the GUI)
    fn initialize(&mut self, sample_rate: f32, max_block_size: usize);
    fn reset(&mut self);
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    // Optional overrides:
    fn latency_samples(&self) -> u64 { 0 }
    fn process_with_midi(&mut self, l, r, events: &[NoteEvent<()>]) { self.process(l, r) }
    fn take_output_midi(&mut self) -> Vec<NoteEvent<()>> { Vec::new() }
}
```

Design points:

- **In-place processing**: every module rewrites the stereo slices it receives; the chain is zero-copy. Modules that need a dry reference (Morph, etc.) keep their own `dry_buf` snapshot.
- **Bypass = skip**: `ModuleChain::process_with_midi` only calls non-bypassed modules. A skipped module never touches the buffers, making bypass a **bit-identical passthrough** (in contrast to parameter-style bypass with smooth crossfades, this is the zero-cost hard switch).
- **One parameter set per module**: a single `Arc<XxxParams>` is mounted both on the umbrella parameter tree (host automation/persistence) and inside the module instance (DSP reads); both point at the same `FloatParam`s — the host writes values and the audio thread reads them via smoothers/atomics with no extra synchronisation.

### 2.2 `ModuleChain` (`modules/chain.rs`)

```rust
pub struct ModuleChain {
    modules: Vec<Box<dyn AkiFxModule>>,
    order: SharedOrder,          // Arc<Mutex<Vec<usize>>>, written by the GUI
    cached_order: Vec<usize>,    // audio-thread local copy
}
```

- **Runtime reordering**: the GUI commits a new permutation into `order` on drag; the audio thread performs an **element-wise comparison** under the lock each block (allocation-free) and refreshes its local copy only when the order actually changed. The parking_lot mutex is held for microseconds and is uncontended in practice.
- **Latency aggregation**: `latency_samples()` = Σ latencies of non-bypassed modules (summed under the lock, no clone). Order changes are reflected immediately.
- **Validity**: `set_order` validates the permutation; persisted orders restored in `initialize` are re-checked via `is_valid_permutation`, falling back to identity when invalid.

### 2.3 The four 21-element lists

The umbrella parameter tree (`AkiFxParams` in `lib.rs`), the chain-build macro (`push!`), the GUI entry macro (`entry!`) and the test name table must stay in the same order. Guarded by `assert_eq!(chain.len(), 21)` and the GUI tests. Adding a module means updating all four places.

---

## 3. The STFT Engine

`stft/engine.rs` is the port of SpectralSuite's `shared/StandardFFTProcessor.cpp` + `SpectralAudioProcessorInteractor.cpp`, shared as **code** by the eight spectral modules (each module owns an independent engine instance, mirroring "each original plugin runs on its own").

### 3.1 Data path (per instance)

```
input ──window(analysis)──► FFT (unscaled) ──first half × 2/N──► polar ──► module callback (per-bin edits)
      ◄──window(synthesis)── IFFT (unscaled) ◄──mirror zeroed ◄── cartesian ◄──┘
output = Σ outputs of all overlap instances (accumulated with +=)
```

This mirrors the 11 steps of the C++ `process()` (the doc comments annotate the C++ line numbers step by step):

1. **Double windowing**: Hann applied once for analysis and once for synthesis (`hann²`).
2. **Normalisation**: the forward transform is **unscaled** (rustfft behaves like kissfft); only the first `half_size` bins are scaled by `2/N` — the standard single-sided-spectrum amplitude factor.
3. **Mirror zeroing**: bins `[half_size, fft_size)` are zeroed after `pol2Car` (the C++ mirrors them via `invertedIndex`, which is equivalent — those bins are already dropped by the single-sided strategy). *Intentional deviation*: the C++ `m_ifftin[0] = 0.f` on line 38 clears the DC bin; AkiFX keeps DC accessible to the modules.
4. **Polar conversion**: the `Polar { magnitude, phase }` round-trip matches `utilities::car2Pol/pol2Car` formula-for-formula.

### 3.2 Overlap instances and the advancement model

- Each channel owns `overlap_count` independent instances; instance `i` writes at `offset = hop × (i % overlaps)` (matching `SpectralAudioProcessorInteractor::setFftSize`), and the FFT fires the instant `offset >= fft_size`, mirroring the `fill_in_passOut` guard.
- **Deviation from the C++ (intentional, fixes a behavioural defect)**: the C++ advances `offset` in `hop_size`-sized chunks; when a host delivers blocks that are not hop multiples (e.g. 480 @ hop 512) the offset overshoots `fft_size` and the `if off < fft_size` guard **periodically drops samples**. AkiFX advances **one sample at a time**: the offset increments per sample and the FFT fires the moment the buffer is exactly full. For hop-aligned blocks the timing is identical to the C++; for misaligned blocks nothing is dropped.
- **Callback signature**: `FnMut(num_bins, chan_idx, overlap_idx, &mut [Polar])`. The C++ `createSpectralProcess`s one independent processor object per (channel × overlap) instance; AkiFX's callback carries the instance identity so stateful modules (Phase Lock) can maintain **per-instance** state.
- **Preallocation**: every instance's `fft_buf/ifft_buf/polar_buf/scratch_fwd/scratch_inv` are allocated at construction; the FFTs run through `process_with_scratch` using private scratch — zero allocations on the audio thread.
- **Send/Sync**: the engine contains only `Vec`s and `Arc<dyn Fft>` (rustfft's `Fft` trait already carries `Send + Sync` bounds), so the auto traits suffice — no hand-written `unsafe impl`.

### 3.3 Window functions (`stft/window.rs`)

Closed-form generation for eight windows (Hann/Hamming/Blackman/…). The Hamming variant preserves the C++ `size + 1` denominator quirk (annotated). `latency_samples() = fft_size + hop_size`, matching what the C++ `SpectralAudioPlugin` reports (the true signal delay is `fft_size`; the reported value adds one hop of conservatism — PDC over-compensation does not break alignment).

---

## 4. DSP Implementation Notes per Effect

### 4.1 SpectralSuite family (shared engine; the callback *is* the algorithm)

Each module implements `fn spectral_callback(num_bins, polar)`, invoked by the engine once per frame per instance:

| Module | Callback algorithm (upstream source) |
|---|---|
| **Spectral Gate** | Per-bin gating: energy < threshold → zero; DC passes through; Tilt slants the threshold by `±(frac−0.5)·2·tilt`. Internal parameters (`cutoff¹⁰`, `balance³`, …) are recomputed per block with ε change detection |
| **Frequency Shift** | First remaps by `floor(i·scale)` (compression/expansion), then overwrites with `i+binShift` (same order as the C++, collisions resolve to the shift). The binShift is **truncated twice** (Hz → int, then Hz × binWidth → int), faithfully to upstream. Rust adds a guard for the C++'s latent out-of-bounds (`size_t` underflow when `binShift > halfFft`) |
| **Frequency Magnet** | Energy below the target bin is pulled upward along a `line^width` curve, energy above pulled downward along `(1−width)·7+1`, accumulated into `temp`; mapped positions keep their **fractional part** for sub-bin interpolation (below: blend toward `temp[idx+1]`, above: toward `temp[idx−1]`, weighted by `frac` — the `utilities::interp_lin` semantics). Rust clamps the C++'s out-of-bounds `temp[idx+1]` read |
| **Bin Scrambler** | Dual index buffers A/B (old/new permutation); the callback produces `out[i] = interp_lin(in[A[i]], in[B[i]], phase/maxPhase)` — magnitude and phase are **separately** linearly interpolated. New permutations: identity → sprinkle (`in[(i+size)/5] = in[rand%size/5]`, destinations in `[size/5, 6·size/25)`) → chunk shuffle (strict `n < size−chunk` bound). The phasor accumulates processed samples (equivalent to the C++ `mPhasor += blockSize`) and swaps patterns on wrap. RNG: a non-zero seed yields a reproducible `StdRng::seed_from_u64` stream |
| **Morph** | 16 control points → `morph_points[i]` recomputed per frame (0–1 normalised mapping onto bins); the callback performs the permutation `out[map[i]] = in[i]` (destinations ≥ bins are dropped — the C++ writes them into the ignored mirror region). The curve is a linear-interpolation simplification of upstream's spline, documented in code |
| **Phase Lock** | `LockState` (on/off state machine with a one-frame transition) and `TransitionState` (time-based morph between live and locked) transcribed formula-for-formula. **Each (channel × overlap) instance owns** its `locked_phases/locked_mags/target_*` vectors (matching the C++ per-instance `createSpectralProcess`). Magnitude tracking: `scale = t + (1−t)·(max_live/max_locked)`; PRNG is xorshift32 |
| **Sinusoidal Shaped Filter** | A 1024-point sine table (with guard point); `index = i·(freq+1) + phase³·halfSize`; magnitude ×= `value^(width²·8+1)`; phase untouched. **Intentional deviation**: the C++ table lookup is effectively zero-order hold (its "interpolation" re-uses an already-truncated integer as the index); Rust performs true linear interpolation — annotated as a fix in code |
| **Playground** | Identity callback (reconstruction RMSE within numerical tolerance). Upstream's "Linear Hann" toggle is never read there either; AkiFX likewise always uses the Hann window |

### 4.2 Soft Vacuum (Hard Vacuum port)

- **Core**: `bridge_rectifier = sin(min(|output|+skew, π/2))` two-stage sin shaping with half-wave-asymmetric mixing; `drive > 1` cascades `drive²` stages. `ALMOST_FRAC_PI_2 = 1.5570797` (upstream notes it as a typo of π/2 in the original plugin).
- **Slew separation**: the slew `slew[n] = x[n] − x[n−1]` is computed at base rate and upsampled through its own Lanczos3 oversampler before feeding the distortion core — this is what keeps the oversampled and non-oversampled timbres aligned.
- **Oversampler**: multi-stage 11-tap Lanczos3 half-band kernels (coefficients bit-identical to upstream); kernel latency compensated to integer samples via `(2·KERNEL_LATENCY).rem_euclid(2ⁿ)`. Scratch buffers are sized from the **host's max_buffer_size** (not hard-coded) and grow at runtime if a misbehaving host delivers more (resetting filter state rather than panicking).
- **Smoothing**: all five parameters use `SmoothingStyle::OversamplingAware(Arc<AtomicF32>, inner)` — step counts scale with `sample_rate × oversampling` so the smoothing **wall-clock time** stays constant; the `Arc` is updated by the Oversampling `IntParam` callback. Smoothed values are **rendered once per block** and shared by both channels (upstream semantics; rendering per channel would double the smoothing rate and diverge L/R).

### 4.3 Crisp

- **Noise source**: a PCG32 integer stream (seeds 69/420, bit-identical to upstream) → RBJ high-pass → RBJ low-pass.
- **Per-sample smoothing**: `amount` and `output_gain` step `smoothed.next()` every sample; the three two-parameter filter groups (pre-RM low-pass, noise high/low-pass) step and rebuild their coefficients per sample **while any member smoother is smoothing** (`maybe_update_filters`, mirroring upstream). When settled, nothing steps (`is_smoothing() == false`).
- **Modes**: `do_ring_mod` branches on Mode; Crispy and CrispyNegated are both `max(0.0)` upstream (an upstream quirk, preserved and annotated).

### 4.4 Puberty Simulator & Spectral Compressor (realfft OLA pipeline)

Both bypass the shared engine and implement their own overlap-add on top of `realfft`, structurally mirroring nih-plug's `StftHelper::process_overlap_add`:

- **Ring-buffer advancement**: `samples_until_next_window = ((hop − write_pos − 1).rem_euclid(hop) + 1)`; per sample, "write input ring → read and zero output ring", and at window boundaries take the **oldest `window_size` samples** for frame processing. The read/write ordering matches upstream's StftHelper line by line.
- **Window**: symmetric Hann (`τ/(N−1)`, the `util::window::hann_window` variant).
- **Puberty**: frame = window → R2C → whole-spectrum bin shift (forward iteration when `multiplier ≥ 1`, reverse otherwise; `floor/ceil` neighbour interpolation by `frac`; optional polar mode) → `×3×gain_compensation` (upstream's documented "random extra gain") → C2R → window → OLA. The pitch parameter steps **once per FFT frame** via `smoothed.next_step(hop)`. Gain compensation `((overlap/4)·1.5)⁻¹/window`. FFT errors skip the frame instead of panicking.
- **Spectral Compressor**: frame = window×`√gc` → R2C → **CompressorBank** → C2R → window×`out_gain`. Key details:
  - **Curve**: `threshold_db(f) = intercept + slope·Δ + curve·Δ²` with `Δ = ln f − ln(center)`; Pink Noise mode bakes −3 dB/oct into the slope.
  - **Envelopes**: per-bin magnitude envelope (first-order attack/release IIR) at effective sample rate `sr/(window/overlap)`; after a reset the timing coefficient linearly recovers over 150 ms (as upstream).
  - **Compression**: soft-knee parabolas (Giannoulis coefficients); `gain_diff = down + up − 2·env`; upward gating on `bin ≥ first_non_dc_bin && ratio ≠ 1 && env_db > −100 dB` (`gain_to_db` clamps at 1e-5 — `util::MINUS_INFINITY_GAIN`).
  - **Curve updates**: upstream wires parameter callbacks directly to the bank's update atomics; AkiFX's parameter object is created before the module and cannot reach the DSP-side bank, so the module **compares a cached snapshot by value** (`CachedCurveParams`, 13 curve parameters) and flags the corresponding curve groups on change — recomputation timing is equivalent to upstream (within one block) and the throwaway bank allocation upstream needs is avoided entirely.
  - **Dry/wet**: a `DryWetMixer` ring delay line aligns with the STFT latency; capacity is `(max_block + window).next_power_of_two()`, grown lazily rather than asserted if a misbehaving host delivers larger blocks. The Mix smoother advances per block via `next_step(block_len)`.

### 4.5 Crossover

- **LR4 topology**: each crossover stage is 2× cascaded low-pass + 2× cascaded high-pass (RBJ formulas, `NEUTRAL_Q = 1/√2`); low-passed bands pass through an **all-pass cascade** (`ap_filters[target][crossover − target − 1]` matrix) for phase compensation. Coefficient updates are written to both channel instances simultaneously.
- **Channel state**: `iir_crossovers: [IirCrossover; 2]` — independent state (upstream achieves the same with a single `Biquad<f32x2>` coefficient set and dual state lanes).
- **Smoothing**: the four crossover frequencies step `next_step(1)` per sample while smoothing with coefficient rebuilds; band gains (an AkiFX addition) smooth per sample.

### 4.6 Diopser

Cascaded RBJ all-pass biquads (TDF-II implementation, formula-identical to upstream's `filter.rs`). Spread fans the stages along `freq·2^(oct·proportion)` (octaves) or `freq + Δ·proportion` (linear), clamped to `[5 Hz, sr/2.05]`. Frequency/resonance/spread smooth per sample with coefficient rebuilds while smoothing. Each channel owns independent filter state (the equivalent split of upstream's `f32x2` lanes).

### 4.7 Loudness War Winner

A faithful port: `output[n] = sign(x) · output_gain` (sign hard-clip); when the WIN HARDER factor f > 0, a 5.5 kHz × 4-stage bandpass engages with `Q = 1e-5 + f·30` (coefficients recomputed only while the factor is smoothing); `output_gain` smooths per sample. Silence detection (all channels simultaneously zero) accumulates for 1 s, then fades linearly and fully mutes at 2 s; `reset()` starts in the "silent" state so a freshly inserted, silent instance does not emit a DC square.

### 4.8 Buffr Glitch

- **Ring buffer**: capacity is the period of MIDI note 0 at maximum octave shift, rounded up to a power of two; `prepare_playback` sets the active length to one period. State machine Recording → (equal-power crossfade, `√t` blend) → Ready. `reset()` clears state **without deallocating** (deallocating would make the first NoteOn after a host reset index an empty buffer — a fixed defect); `note_on` defensively ignores unallocated buffers.
- **Voices**: 8-voice polyphony with a three-tier steal policy; the AR envelope is a first-order IIR (`exp(−1/(t·sr))`) whose coefficients are computed once per block instead of per sample per voice.
- **PolyVolume**: a per-voice 5 ms linear-smoother note-expression gain, multiplied into `velocity_gain × expr × env`.

### 4.9 Safety Limiter

A 19-edge SOS Morse table (times/gates, including a 4-second wrap alias edge); a 420 Hz sine at `threshold × 0.125` amplitude; phase-wrap click avoidance via a `looked_at` flag; equal-power fades (`√` curves). **Non-finite samples (NaN/Inf) are muted and trigger the alert** (upstream behaviour). Smoothed gain recovery; any over-threshold sample re-arms the alarm.

### 4.10 Source & MIDI modules

- **Sine Generator**: f64 phase accumulator; NoteOn velocity sets a 5 ms linear gain envelope target, PolyPressure likewise, NoteOff ramps to zero — the envelope always renders (no hard gate), eliminating release clicks.
- **MIDI Inverter**: mirrored transforms for 13 `NoteEvent` variants (`15−ch`, `127−note`, `1−v`); unrecognised variants are silently dropped (as upstream); zero output when disabled. Transformed events reach the host via `take_output_midi()` in the plugin layer.
- **Poly Mod Synth**: 16 voices; events split the block into sub-blocks at their `timing` (MAX 64 samples); envelope/gain step `next_block` per sub-block; with CLAP polyphonic modulation unavailable it degrades to standard behaviour (as upstream VST3). The PRNG is an XorShift32 (standing in for upstream's Pcg32 — different initial phases, same character, annotated).

---

## 5. Parameter System & State Persistence

- **Parameter tree**: the `#[derive(Params)]` umbrella `AkiFxParams` mounts the 21 module parameter sets via `#[nested(id_prefix = "...")]` → the host sees `AkiFX/:module prefix/:parameter ID` namespacing. The dual mounting of each `Arc` (tree + module) is nih-plug's standard sharing pattern.
- **Persistence**: three `#[persist]` fields, deserialised by nih-plug on state restore (before `initialize()` runs again):
  - `module_order: Mutex<Vec<usize>>` — processing order;
  - `ui_zoom: Mutex<f32>` — UI zoom;
  - `module_enabled: Mutex<Vec<bool>>` — per-module power states. `initialize()` applies them to the modules' bypass atomics (`!enabled → bypassed`); a fresh load defaults to all-false (all bypassed).
- **Value-change detection**: where a callback `Arc` cannot be held (Spectral Compressor curves), a module-side snapshot comparison substitutes: one 13-field f32 comparison per block.

---

## 6. Host Integration

```rust
impl Plugin for AkiFx {
    const AUDIO_IO_LAYOUTS: stereo + mono;
    const MIDI_INPUT/OUTPUT: MidiConfig::MidiCCs;   // all events
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
}
```

- **process()**:
  1. **Latency re-report**: `chain.latency_samples()` is compared against `reported_latency`; on change `context.set_latency_samples()` runs — module toggles and window/oversampling changes take effect within the same block (required for DAW PDC).
  2. **MIDI collection**: `next_event()` drains everything into a reused buffer (no allocation) and the events are **broadcast** to the chain.
  3. **Audio**: stereo processes in place via `split_first_mut`; mono is duplicated into preallocated scratch, processed, and copied back.
  4. **MIDI out**: each module's `take_output_midi()` → `context.send_event()`.
- **editor()**: `build_ui_entries(params, chain.modules())` reads latencies from the live modules → `wire_bypass_flags` shares the modules' bypass `Arc<AtomicBool>` with the GUI (critical: the GUI power toggles and the audio thread read/write **the same atomics**; a regression test guards this) → `create_egui_editor`.
- **Packaging**: `bundler.toml` sets the product name `AkiFX`; `cargo xtask bundle` produces `.vst3/Contents/x86_64-win/AkiFX.vst3`, `.clap`, and `AkiFX.exe`.

---

## 7. Real-Time Safety Model

Hard constraints for the audio thread and how they are met:

| Constraint | Implementation |
|---|---|
| No heap allocation | Spectral callbacks use module-preallocated scratch exclusively (`scratch_polar`, `temp`, snapshot vectors, `slew_buf`, …); the chain order uses "compare under lock + local cache"; the event buffer is reused. The only permitted allocation is a **host contract violation** (blocks larger than declared at `initialize`) triggering lazy growth (soft_vacuum/sc/mixer) — allocate rather than panic |
| No panics | All `unwrap/expect` replaced with let-else skip/passthrough fallbacks; FFT `Result` errors skip the frame; coefficient-generation `assert!`s replaced with clamping (RBJ frequency > Nyquist, etc.); the dry/wet mixer grows lazily |
| No unbounded waits | The single lock (order mutex) is held only for a comparison/sum, no nested locks; bypass toggles are relaxed atomic loads |
| State consistency | Bypass = whole-module skip (bit-identical); order changes take effect per block and are validated element-wise |

---

## 8. GUI Implementation

- **Framework**: `nih_plug_egui::create_egui_editor` (egui 0.31 + baseview), `EguiState::from_size(1100×720)` plus a custom `ResizableWindow` (minimum 900×600). Everything is drawn as vectors (LEDs, knobs and the power pill are painter primitives); there are no bitmap assets.
- **Fonts**: embedded via `include_bytes!` — Inter Regular/Medium (body), Cormorant Garamond SemiBold (display serif), Noto Sans SC (CJK fallback). Custom `FontFamily::Name(...)` values must be explicitly bound — an unbound family panics in egui.
- **Parameter widgets**: iterating `param_map()`'s `ParamPtr` set rendered with `ParamSlider::for_param` (covering Float/Int/Bool/Enum pointers); the GUI never writes values directly — it goes through the nih-plug setter, i.e. the automation path.
- **Power toggles**: write the shared atomics (`Relaxed`) and then call `sync_persisted_enabled`, mirroring the 21 toggles into the `#[persist]` state so the host captures them on save.
- **Latency display**: read live from the modules' `latency_samples()` (no static table); the bottom bar sums non-bypassed modules.

---

## 9. Testing & Verification Strategy

**277 tests** (`cargo test -p akifx`) in four layers:

1. **Engine** (`tests/stft_engine.rs`): identity-reconstruction RMSE, silence floor < −120 dBFS, 440 Hz correlation ≥ 0.99, closed-form window values, 128–1024 varying blocks, and **non-hop-multiple blocks (480) dropping nothing** (a steady-state DC input stays bounded away from zero; the old chunked driver floored it to zero in dropped segments).
2. **Chain** (`tests/chain_test.rs`, `tests/full_chain.rs`): insertion order, bit-identical bypass, latency summation, **latency changing when a module toggles** (the precondition for PDC sync), 21-module structure, all-active NaN sweeps.
3. **Module** (`#[cfg(test)]` in each file, ~244 tests): identity per module (parameters at zero = passthrough), noise/sine sweeps without NaN, determinism (same seed = bit-identical), and key regressions (Bin Scrambler conserving energy at full scramble, Gain halving at −6 dB, Crossover rebuilding coefficients on frequency change).
4. **GUI** (inline in `gui/`): entry count/name/prefix alignment, bypass-atomic sharing (ptr_eq), persistence mirroring, description completeness.

**Fidelity verification method**: formula-level audits against the upstreams across four dimensions (formulas, constants, defaults, control flow). Upstream quirks are preserved explicitly and annotated (CrispyNegated ≡ Crispy, the Hamming window's `size+1` denominator, Puberty's ×3 gain); intentional deviations (per-sample advancement instead of hop chunking, true passthrough instead of the PVOC-off scaling, …) are documented in code comments and in §4 of this document.

---

*AkiFX © Akiro · GPLv3*

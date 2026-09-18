# AkiFX Technical Overview (English)

**Version 0.3.0** · For developers and contributors

This document describes the inner workings of AkiFX: the framework, the declarative module registry, the STFT engine, the DSP algorithms behind each effect, host integration, the real-time safety model, and the testing strategy. The DSP has been calibrated line-by-line against the upstream sources (SpectralSuite @ `5dfd294`, nih-plug @ `f36931f`).

---

## Table of Contents

1. [Tech Stack & Project Layout](#1-tech-stack--project-layout)
2. [Declarative Module Registry](#2-declarative-module-registry)
3. [The Module Chain](#3-the-module-chain)
4. [The STFT Engine](#4-the-stft-engine)
5. [DSP Implementation Notes per Effect](#5-dsp-implementation-notes-per-effect)
6. [Parameters & Persistence](#6-parameters--persistence)
7. [Host Integration](#7-host-integration)
8. [Real-Time Safety Model](#8-real-time-safety-model)
9. [GUI Implementation](#9-gui-implementation)
10. [Testing & Verification Strategy](#10-testing--verification-strategy)

---

## 1. Tech Stack & Project Layout

| Component | Choice | Notes |
|---|---|---|
| Language | Rust 2021 (stable, MSVC) | No GC, no runtime on the audio thread |
| Plugin framework | [nih-plug](https://github.com/robbert-vdh/nih-plug) @ `f36931f` (**rev-pinned**) | VST3 + CLAP export, standalone (WASAPI/JACK), params derive macro |
| GUI | nih-plug-egui (egui 0.31 + baseview) | Parented into the host-provided window |
| FFT | rustfft (complex FFT, spectral engine) + realfft (real FFT, Spectral Compressor) | Both lazily planned |
| Other | parking_lot (locks), rand/StdRng, atomic_float (oversampling-aware smoothing) | |

```
Cargo.toml (workspace: akifx + xtask)
akifx/src/
  lib.rs        # define_modules! table, Plugin impl, process(), metering
  main.rs       # standalone entry (period-size guard)
  registry.rs?  — (macro definition lives in modules/registry.rs)
  modules/
    mod.rs             # AkiFxModule trait
    registry.rs        # define_modules! macro definition
    chain.rs           # ModuleChain + SharedOrder
    dsp.rs             # shared Biquad
    spectral_common.rs # SpectralFxCore engine wrapper
    spectral_compressor/  # mod + bank + curve + mixer + analyzer
    ...                # one file per remaining module
  stft/          # SpectralSuite STFT engine (engine/window/polar)
  gui/           # egui editor (mod/state/theme/descriptions)
akifx/assets/fonts/  # Inter ×2, Noto Sans SC, Cormorant Garamond
xtask/               # nih_plug_xtask bundler
```

`lib.rs` exports `nih_export_vst3!` / `nih_export_clap!`; `main.rs` exports the standalone — the canonical nih-plug layout (`cdylib` + `lib`).

---

## 2. Declarative Module Registry

The core of the v0.3.0 refactor. `define_modules!` (defined in `modules/registry.rs`, invoked once in `lib.rs`) takes a single table:

```rust
crate::define_modules! {
    sine_gen,  SineGenModule,  SineGenParams,  "sine_gen",  "Sine Generator";
    ...
}
```

and generates everything that used to be four hand-maintained lists:

1. the umbrella `AkiFxParams` tree (`#[nested(id_prefix)]` fields),
2. `Default for AkiFxParams` (every params type implements `Default`),
3. `create_default_chain()` — chain construction in table order, all modules bypassed,
4. `MODULE_NAMES` / `MODULE_PREFIXES` / `MODULE_COUNT`,
5. `params_for_module(params, idx)` — typed access via a getter table.

The GUI entries and the tests consume these generated tables; adding a module is one table row plus its own file. Registry integrity tests pin the static tables to the live chain (`registry_tables_match_chain`) and cover parameter access (`params_for_module_covers_all`).

The umbrella also carries the **global bypass** `BoolParam` (`"global_bypass"`), host-automatable and persisted, plus the three `#[persist]` fields (order, zoom, per-module enabled states).

---

## 3. The Module Chain

```rust
pub trait AkiFxModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn params(&self) -> &dyn Params;
    fn bypass_flag(&self) -> &Arc<AtomicBool>;
    fn initialize(&mut self, sample_rate: f32, max_block_size: usize);
    fn reset(&mut self);
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    fn latency_samples(&self) -> u64 { 0 }
    fn process_with_midi(&mut self, l, r, events) { self.process(l, r) }
    fn take_output_midi(&mut self) -> Vec<NoteEvent<()>> { Vec::new() }
    fn spectrum_view(&self) -> Option<SpectrumView> { None }
}
```

- **In-place processing**; modules needing a dry reference keep their own snapshots.
- **Bypass = skip**: the chain never calls a bypassed module — bit-identical passthrough at zero cost. The **global bypass** in `process()` skips the entire chain the same way.
- **Runtime reordering** via `SharedOrder` (`Arc<Mutex<Vec<usize>>>`): the audio thread compares under the lock against a local cache (allocation-free) and refreshes only on change.
- **Latency aggregation** sums non-bypassed module latencies.
- **MIDI broadcast**: every event reaches every enabled module; each module's `take_output_midi()` is drained and sent to the host.

---

## 4. The STFT Engine

`stft/engine.rs` ports SpectralSuite's `StandardFFTProcessor` + `SpectralAudioProcessorInteractor`, shared as code by Spectral Gate and Frequency Shift (via `SpectralFxCore`, §5).

- **Data path**: window (analysis) → unscaled FFT → first-half × 2/N → polar → module callback → cartesian (mirror zeroed) → unscaled IFFT → window (synthesis); outputs sum across overlap instances. The DC-clearing `m_ifftin[0] = 0.f` of the C++ is intentionally omitted so modules can access DC.
- **Instances**: `overlap_count` per channel, offsets staggered `hop × (i % overlaps)`; the FFT fires the instant the buffer fills.
- **Per-sample advancement**: the C++ advances offsets in hop-sized chunks and silently drops samples when a host block is not a hop multiple (e.g. 480 @ hop 512). AkiFX advances one sample at a time — identical timing for aligned blocks, no drops otherwise.
- **Callback signature**: `FnMut(num_bins, chan, overlap, polar)` — instances are identified so stateful modules can keep per-instance state.
- **Fixed configuration**: 2048-point FFT, 4× overlap, Hann; `latency_samples() = fft_size + hop_size` (matching the C++ plugin's conservative report).
- **Validation**: font-style asset corruption taught us to validate inputs — the engine asserts its config at construction and all audio-path errors degrade to frame skips, never panics.

---

## 5. DSP Implementation Notes per Effect

### 5.1 Shared layers

- **`dsp.rs`**: one TDF-II `Biquad` with RBJ coefficient constructors (low-pass/high-pass/all-pass) and input clamping (Nyquist, minimum Q). Used by Crisp, Crossover and Diopser — three divergent copies collapsed into one.
- **`spectral_common.rs`**: `SpectralFxCore` owns the STFT engine and dry-signal snapshots for Spectral Gate and Frequency Shift; each module only carries parameters and its spectral callback.

### 5.2 Sine Generator
f64 phase accumulator; NoteOn velocity → 5 ms linear gain envelope, PolyPressure likewise, NoteOff ramps to zero; the envelope always renders (no hard gate).

### 5.3 Soft Vacuum
Multi-stage diode-bridge waveshaping (`bridge_rectifier = sin(min(|out|+skew, π/2))` two-stage shaping, stage count = drive² above 1.0). The slew signal is computed at base rate and oversampled through its own Lanczos3 cascade, keeping oversampled timbre aligned. Oversampler scratch is sized from the host's `max_buffer_size` and grows lazily on contract violations. All five parameters use `SmoothingStyle::OversamplingAware` bound to the oversampling `IntParam`; smoothed values render once per block and are shared by both channels.

### 5.4 Crisp
PCG32 white noise (seeds 69/420) → RBJ HP → RBJ LP, ring-modulated against the low-passed input. `amount`/`output_gain` step per sample; the three filter groups rebuild coefficients per sample **while their smoothers move** (`maybe_update_filters`).

### 5.5 Spectral Gate
Per-bin gating against `cutoff¹⁰`-derived thresholds with tilt `±(frac−0.5)·2·tilt`; DC passes; internal params recomputed on ε-change.

### 5.6 Frequency Shift
First remaps bins by `floor(i·scale)`, then overwrites with `i+binShift` (upstream order). The binShift is truncated twice (Hz → int, × binWidth → int), faithfully to the C++. Rust guards the C++'s latent `size_t` underflow.

### 5.7 Spectral Compressor
realfft OLA pipeline mirroring nih-plug's `StftHelper`: per-sample ring in/out, frame at window boundaries with oldest-first extraction. Symmetric Hann. Per-bin envelopes at effective sample rate `sr/(window/overlap)` with a 150 ms timing-scale recovery after reset. Soft-knee parabolas (Giannoulis), `gain_diff = down + up − 2·env`; upward gating on `bin ≥ first_non_dc_bin && env_db > −100 dB` (gain→dB clamps at 1e-5). `first_non_dc_bin` is computed from the ~20 Hz bin index. Curve parameters drive recomputation via module-side **value-change detection** (13-field snapshot per block) — the params object cannot reach the DSP-side bank, so comparison restores upstream's recompute-on-change timing. Dry/wet mixer aligns with the STFT latency; capacity grows lazily on host contract violations; the Mix smoother advances `next_step(block_len)` per block. GUI spectrum publishing is described in §9.

### 5.8 Crossover
LR4 (two cascaded Butterworth LP/HP per split) with an all-pass cascade phase-compensating lower bands. One `IirCrossover` per channel (independent state). Crossover frequencies glide per sample while smoothing; band gains smooth per sample (an AkiFX addition).

### 5.9 Diopser
Cascaded RBJ all-pass biquads (up to 512), spread fan along octave/linear rules clamped to `[5 Hz, sr/2.05]`. Frequency/resonance/spread smooth per sample with coefficient rebuilds while smoothing.

### 5.10 Buffr Glitch
Ring buffer sized for MIDI note 0 at max octave shift (power of two); Recording → equal-power crossfade → Ready state machine. `reset()` clears state without deallocating (a previous defect made the first NoteOn after a host reset panic). 8 voices with three-tier stealing; AR envelope coefficients computed once per block. PolyVolume per-note gain via 5 ms linear smoothers.

### 5.11 Gain / Safety Limiter
Gain: clean stage with 50 ms logarithmic smoothing (seeded in `initialize` for non-host contexts). Safety Limiter: 19-edge SOS Morse table on a 420 Hz sine at `threshold × 0.125`, phase-wrap click avoidance, equal-power fades; non-finite samples are muted and trigger the alarm.

---

## 6. Parameters & Persistence

`AkiFxParams` (generated by the registry) carries the module tree plus:

- `global_bypass: BoolParam` — chain-wide bypass, automatable, persisted by the host;
- `module_order: Mutex<Vec<usize>>` — processing order;
- `ui_zoom: Mutex<f32>` — UI zoom;
- `module_enabled: Mutex<Vec<bool>>` — per-module power states, applied to the live bypass atomics in `initialize()` (all-off on a fresh load).

Where a callback `Arc` cannot reach the DSP-side state (Spectral Compressor curves), modules detect changes by **value comparison** against a cached snapshot — recomputation timing matches upstream within one block.

---

## 7. Host Integration

`process()` in order:

1. **Global bypass gate** — engaged: publish meters, drain nothing further, return (bit-identical passthrough).
2. **Latency re-report** — chain latency compared to `reported_latency`; on change `context.set_latency_samples()` runs in the same block.
3. **MIDI collection** — all events into a reused buffer, broadcast to the chain.
4. **Audio** — stereo in place via `split_first_mut`; mono via preallocated scratch.
5. **Peak metering** — per-channel absolute peaks into `Arc<AtomicF32>` handles shared with the editor.
6. **MIDI out** — each module's `take_output_midi()` sent to the host.

`editor()` assembles UI entries from the registry, wires the live bypass atomics into them, grabs the peak-meter handles and any module `spectrum_view()`, then builds the egui editor. `main.rs` raises the standalone's default period size to 2048 (some WASAPI devices deliver packets larger than the 512 request, which trips an assert inside nih-plug's cpal backend; an explicit `--period-size` still wins).

---

## 8. Real-Time Safety Model

| Constraint | Implementation |
|---|---|
| No heap allocation | Spectral callbacks use preallocated scratch; the chain order uses compare-under-lock + local cache; the event buffer is reused. The only permitted allocation is a host contract violation (oversized blocks) triggering lazy growth — allocate rather than panic |
| No panics | `unwrap/expect` replaced with let-else skip/passthrough; FFT errors skip frames; coefficient inputs clamped; the dry/wet mixer grows lazily; **fonts are validated by sfnt magic before registration** (a corrupt font asset once panicked hosts inside epaint) |
| No unbounded waits | One mutex (order) held only for a comparison/sum; bypass flags are relaxed atomics |
| State consistency | Bypass = whole-module skip (bit-identical); order changes validated element-wise; the global bypass skips everything |

---

## 9. GUI Implementation

- **Framework**: `nih_plug_egui::create_egui_editor` (egui 0.31 + baseview) with a custom resizable window. Everything is painter-drawn; no bitmap assets.
- **Fonts**: embedded via `include_bytes!` — Inter Regular/Medium (body), Cormorant Garamond SemiBold (display serif), Noto Sans SC (CJK fallback). Every file is validated by sfnt magic (`is_font_file`) and skipped when invalid; custom `FontFamily::Name(...)` values are explicitly bound (unbound families panic in egui).
- **Parameter widgets**: floats/ints/enums render as `ParamSlider`s; **bools render as single toggle buttons** (pressed = on), driven through the nih-plug setter (the automation path).
- **Global bypass button + peak meters** live in the bottom strip: the bypass is a parameter button; the meters draw L/R peaks from the shared atomics with a dBFS readout and over-scale highlight.
- **Spectrum view** (Spectral Compressor): the audio thread publishes `SpectrumSnapshot`s (envelope magnitudes + downward threshold curve, dB) into a `SpectrumView` holder at ~30 Hz; the editor draws spectrum (jade) and threshold curve (amber) on a log-frequency axis with octave gridlines.
- **Persistence**: the power toggles mirror into `module_enabled` on every click, so hosts capture the state on save.

---

## 10. Testing & Verification Strategy

**~200 tests** in four layers:

1. **Engine** (`tests/stft_engine.rs`): identity reconstruction, silence floor, sine correlation, closed-form windows, varying blocks, **non-hop-multiple blocks dropping nothing**.
2. **Chain** (`tests/chain_test.rs`, `tests/full_chain.rs`): insertion order, bit-identical bypass, latency summation and toggle dynamics, the 11-module structure, the global-bypass parameter contract, all-active NaN sweeps.
3. **Module** (~165 inline tests): identity at neutral parameters, NaN sweeps, determinism, and targeted regressions (crossover coefficient rebuilds, smoothing stepping, gains).
4. **Registry/GUI**: registry tables vs. the live chain, parameter access coverage, UI entry counts/names/prefixes against the generated tables, bypass-atomic sharing (ptr_eq), persistence mirroring, description completeness for all 11 modules.

**Fidelity method**: formula-level audits against the upstreams (formulas, constants, defaults, control flow). Upstream quirks are preserved and annotated; intentional deviations (per-sample STFT advancement, trims listed in the manual) are documented in code and in the user manual §7.

---

*AkiFX © Akiro · GPLv3*

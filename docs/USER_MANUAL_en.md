# AkiFX User Manual (English)

**Version 0.3.0 · VST3 / CLAP / Standalone**

AkiFX is an all-in-one effects rack: it consolidates the core effects from two well-known open-source projects — Andrew Reeman's **SpectralSuite** (a C++/JUCE spectral processing suite) and Robbert van der Helm's **nih-plug** framework with its official plugin collection — into a **single chain with a single interface**, exported as VST3 and CLAP.

> **v0.3.0 breaking change**: the plugin has been streamlined from 21 modules to **11 core effects**, and adds a global bypass, output peak meters, and a live spectrum view for the Spectral Compressor. **Projects saved with v0.2.x load with module states, ordering and parameters reset to defaults** — an intentional 0.x-stage break, not migrated.

---

## Table of Contents

1. [Installation & Loading](#1-installation--loading)
2. [Interface Overview](#2-interface-overview)
3. [Signal Chain & Module Ordering](#3-signal-chain--module-ordering)
4. [Module Reference](#4-module-reference)
5. [MIDI Support](#5-midi-support)
6. [Latency & Plug-in Delay Compensation (PDC)](#6-latency--plug-in-delay-compensation-pdc)
7. [Differences from the Original Plugins](#7-differences-from-the-original-plugins)
8. [FAQ](#8-faq)

---

## 1. Installation & Loading

| Format | Location | Installation |
|---|---|---|
| **VST3** | `target/bundled/AkiFX.vst3/` | Copy to the system VST3 folder (Windows: `C:\Program Files\Common Files\VST3`; macOS: `/Library/Audio/Plug-Ins/VST3`) |
| **CLAP** | `target/bundled/AkiFX.clap` | Copy to the CLAP folder (Windows: `C:\Program Files\Common Files\CLAP`) |
| Standalone | `target/bundled/AkiFX.exe` | Run directly (WASAPI/JACK audio) for host-free testing |

On first load, **all 11 modules are bypassed** — the plugin is equivalent to a straight wire. This is intentional: the spectral modules consume significant CPU, so you enable only what you need. **Module power states are saved with your project and restored automatically.**

Build from source: `cargo xtask bundle akifx --release` (requires the Rust stable MSVC toolchain).

---

## 2. Interface Overview

AkiFX uses a two-pane "rack + parameter panel" layout:

- **Rack (left)**: the 11 modules are listed vertically in chain order. Each row provides:
  - **LED power toggle**: switches between ACTIVE / BYPASSED. Bypassed modules are skipped entirely (zero CPU) with a guaranteed **bit-identical passthrough**.
  - **Latency badge**: the sample delay introduced by that module (e.g. `2560 samples`), shown only when non-zero.
  - **Drag handle**: grab the handle on the left of a row to **drag-reorder** the processing chain in real time.
- **Parameter panel (right)**: all parameters of the selected module:
  - **On/off parameters** render as a single toggle button (pressed = on);
  - **Numeric parameters** are value-displaying sliders;
  - Every control has a detailed tooltip (including Chinese descriptions).
- **Spectral Compressor spectrum view**: with the Spectral Compressor selected, the top of the panel shows the **live input spectrum with the threshold curve overlaid** on a logarithmic frequency axis — the core feedback surface for threshold tuning.
- **Bottom status bar**, left to right:
  - **GLOBAL BYPASS**: one button bypasses the entire chain (bit-identical passthrough, host-automatable);
  - the **master gain slider**;
  - **stereo peak meters**: post-processing L/R peaks (dBFS readout, amber above full scale);
  - live total latency, UI zoom, About.

---

## 3. Signal Chain & Module Ordering

Default processing order:

```
1. Sine Generator    →  2. Soft Vacuum  →  3. Crisp
→  4. Spectral Gate  →  5. Frequency Shift
→  6. Spectral Compressor
→  7. Crossover      →  8. Diopser
→  9. Buffr Glitch   →  10. Gain  →  11. Safety Limiter
```

The rationale: sources first → tone shaping (saturation/excitation) → frequency-domain processing → per-band dynamics → time-domain splitting and phase effects → buffer glitching → master gain and safety limiting.

**The order is fully adjustable**; reorderings persist with the project.

---

## 4. Module Reference

### 4.1 Sine Generator
- **Origin**: nih-plug `sine` example.
- **Principle**: MIDI-triggered sine oscillator. f64 phase accumulator with glide; level follows **note-on velocity** and **polyphonic pressure**; a 5 ms linear anti-click envelope covers note on and off.
- **Parameters**: Level (−24 to +6 dB, default −12); Fallback Frequency (20–20000 Hz, default 440).
- **Usage**: first-slot test source, monitoring calibration, clean exciter for the spectral modules. Silent without MIDI input.

### 4.2 Soft Vacuum
- **Origin**: nih-plug `soft_vacuum` (algorithm from Chris Johnson / Airwindows' Hard Vacuum).
- **Principle**: a multi-stage **diode-bridge waveshaping** saturator. Drive above 100 % cascades distortion stages; Warmth sets half-wave asymmetry; Aura adds bias gain. The **slew signal is computed at base rate and oversampled separately**, keeping oversampled and dry-run timbres aligned.
- **Parameters**: Drive (0–200 %, default 0); Warmth (0–100 %, default 0); Aura (0–100 %, default 0); Output Gain (−40 to 0 dB); Mix (100 %); Oversampling (1x–16x, default 2x, Lanczos3).
- **Usage**: drum-bus warming, vocal tube character, bass excitation. At high Drive prefer ≥4x oversampling. All parameters use **oversampling-aware smoothing** — zipper-free automation.

### 4.3 Crisp
- **Origin**: nih-plug `crisp`.
- **Principle**: a noise ring-modulation exciter. PCG32 white noise, shaped by high-/low-pass filters, ring-modulates the low-passed input to extract and re-inject high-frequency content.
- **Parameters**: Amount (0–100 %, default 35 %); Mode (Soggy = full waveform / Crispy = positive half); Stereo Mode (default Stereo); RM Input LPF / Noise HPF / Noise LPF; Output Gain; Wet Only.
- **Usage**: air and presence for vocals, acoustic guitar, snare. Low Amount + Wet Only makes a parallel exciter.

### 4.4 Spectral Gate
- **Origin**: SpectralSuite, same name.
- **Principle**: frequency-domain gating. An STFT (2048-point, 4× overlap, Hann) resolves 1024 bands; bands below threshold are zeroed or partially retained via the Tilt curve. Weak/Strong Balance sets the energy weighting of retained bands.
- **Parameters**: Cutoff (0–1 displayed in dB, default 0.6); Weak/Strong Balance (default 0.7); Tilt + Enable Tilt (default 0.5 / Off).
- **Usage**: robotic vocals, spectral drum restructuring. Balance sets the character, Cutoff the intensity.

### 4.5 Frequency Shift
- **Origin**: SpectralSuite, same name.
- **Principle**: shifts every spectral bin by a fixed amount (a true frequency shift — harmonic spacing is destroyed for metallic, inharmonic textures), or **scales** the spectrum (scale > 1 spreads harmonics, < 1 compresses them).
- **Parameters**: Frequency Shift (±500 Hz, default 0); Frequency Scale (0.25–3.0, default 1.0).
- **Usage**: small shifts for metallic sheen; Scale to granulate drums or stretch them. Combinable.

### 4.6 Spectral Compressor
- **Origin**: nih-plug `spectral_compressor`.
- **Principle**: up to 16384 bands of independent upward + downward compression: downward tames loud bands (harsh resonances), upward lifts quiet ones (noise floor and air); combined they approximate arbitrary spectral-envelope shaping. The threshold is a **frequency-tilted curve** (Pink Noise mode includes −3 dB/oct). Bins below ~20 Hz are excluded from upward compression; bins below −100 dB are never upward-compressed.
- **GUI**: the **live spectrum view** at the top of the panel shows the input spectrum (green) with the downward threshold curve overlaid (amber) on a log-frequency axis — bands above the curve get compressed.
- **Parameters** (grouped):
  | Group | Parameter | Default |
  |---|---|---|
  | Global | Output Gain / Mix / Window Size / Overlap / Attack / Release | 0 dB / 100 % / 2048 / 16x / 150 ms / 300 ms |
  | Threshold | Global Threshold / Center / Slope / Curve | −12 dB / 420 Hz / 0 / 0 |
  | Upwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 75 % / 6 dB |
  | Downwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 0 % / 6 dB |
- **Usage**: low ratios at high overlap for transparent mastering colour; high upward ratios to turn whispers into roars; high downward ratio with a narrow curve for precise harshness removal. All curve parameters automate in real time (the spectrum view follows).

### 4.7 Crossover
- **Origin**: nih-plug `crossover` (IIR section).
- **Principle**: a **Linkwitz-Riley 24 dB/oct (LR4)** multiband splitter with cascaded Butterworth sections and all-pass phase compensation; bands sum flat at the crossover points. AkiFX adds an independent per-band output gain.
- **Parameters**: Bands (2–5, default 2); Crossover 1–4 Freq (40 Hz–20 kHz, defaults 200/1000/5000/10000); Band 1–5 Gain (±24 dB, default 0).
- **Usage**: multiband routing, band soloing, five "band faders" for extreme sculpting.

### 4.8 Diopser
- **Origin**: nih-plug `diopser`.
- **Principle**: up to 512 cascaded **all-pass filters**. All-pass stages leave magnitude untouched while rotating phase; many stages create huge phase delay around the tuned frequency. Spread fans the stages into a filter cloud. Parameters glide per sample — click-free sweeps.
- **Parameters**: Filter Stages (0–512, default 0 = passthrough); Filter Frequency (5 Hz–20 kHz, default 200); Filter Resonance (0.01–30, default 0.5); Filter Spread (±5 oct, default 0); Spread Style (Octaves/Linear).
- **Usage**: 32–128 stages + resonance 5–20 + a sweep = the classic "Diopser roar".

### 4.9 Buffr Glitch
- **Origin**: nih-plug `buffr_glitch`.
- **Principle**: MIDI-triggered buffer repeats/stutter. On NoteOn a voice records exactly one period (pitch = note × octave shift) and loops it with an equal-power crossfade. 8 voices; PolyVolume per-note volume supported. The dry signal is ducked by the active voice envelope.
- **Parameters**: Dry Mix (default 1.0); Velocity Sensitive (Off); Octave Shift (±2, default 0); Attack/Release (2 ms); Crossfade (2 ms).
- **Usage**: mount on vocals or snare and "freeze-repeat" from a keyboard.

### 4.10 Gain
- **Origin**: nih-plug `gain` example.
- **Principle**: a clean digital gain stage with 50 ms logarithmic smoothing.
- **Parameters**: Gain (−30 to +30 dB, default 0).

### 4.11 Safety Limiter
- **Origin**: nih-plug `safety_limiter`.
- **Principle**: a **fuse**, not a musical limiter: any sample above threshold triggers a **SOS Morse-code** alarm on a 420 Hz sine and immediately pulls the level down; after 1 s of silence it returns to passthrough. NaN/Inf samples are muted **and trigger the alarm**.
- **Parameters**: Threshold (−24 to +12 dB, default 0).
- **Usage**: last slot on the master at ~+6 dB as a blowout fuse; if you hear SOS, something upstream is clipping.

---

## 5. MIDI Support

All host MIDI events (notes, CC, pitch bend, polyphonic pressure, …) are broadcast to every enabled module:

| Module | MIDI consumed |
|---|---|
| Sine Generator | Note On/Off (velocity → gain), Poly Pressure (→ gain) |
| Buffr Glitch | Note On/Off, PolyVolume (per-note volume) |

---

## 6. Latency & Plug-in Delay Compensation (PDC)

| Module | Latency |
|---|---|
| Spectral Gate / Frequency Shift | 2560 samples (FFT 2048 + hop 512, matching the originals) |
| Spectral Compressor | window length (default 2048) |
| Soft Vacuum | oversampler kernel latency (~10 samples at 2x) |

Latency changes are re-reported to the host **within the same processing block** (module toggles, window/oversampling changes). The global bypass does not affect the report. No manual compensation required.

---

## 7. Differences from the Original Plugins

AkiFX is calibrated against the upstreams (SpectralSuite @ `5dfd294`, nih-plug @ `f36931f`); with default parameters the sound matches the originals. Known intentional differences:

1. **Integration-driven**: the single-plugin chain, chain/global bypass (bit-identical), drag reordering, broadcast MIDI routing.
2. **Trimmed features**:
   - SpectralSuite's FFT-size/window selection and PVOC phase-vocoder mode are not exposed (fixed at 2048 / 4× / Hann — the originals' defaults);
   - Crossover's linear-phase FIR variant has been removed (only the implemented IIR LR24 remains);
   - Spectral Compressor's sidechain threshold modes have been removed (no sidechain input is routed); Pink Noise is the only mode;
   - Crisp's Crispy (alt) mode has been removed (identical to Crispy upstream);
   - Diopser's automation-precision parameter and spectrum-analyser GUI are not ported (default precision = per-sample smoothing, behaviour identical).
3. **Upstream quirks preserved**: the retained modules preserve upstream behaviour formula-for-formula (quantified quirks documented in the technical overview).
4. **AkiFX enhancements**: real-time automation of Spectral Compressor's threshold curves + the spectrum view, band gains (Crossover), global bypass, peak meters, drag reordering, allocation-free audio processing.

---

## 8. FAQ

**Q: No sound after loading?**
All modules start bypassed. Light an LED on the rack (or at least Gain). Hearing SOS? That's the Safety Limiter's over-threshold alarm.

**Q: My v0.2.x project lost its module states after upgrading?**
v0.3.0 is a breaking release; old states are not migrated and reset to defaults.

**Q: High CPU usage?**
Each spectral module owns an independent 2048-point FFT engine. Bypassed modules cost nothing; the global bypass skips everything.

---

*AkiFX © Akiro · Licensed GPLv3. SpectralSuite (Unlicense); nih-plug and its plugins (ISC/GPL-3.0); the Hard Vacuum algorithm © Chris Johnson (Airwindows).*

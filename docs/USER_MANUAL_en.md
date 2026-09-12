# AkiFX User Manual (English)

**Version 0.2.1 · VST3 / CLAP / Standalone**

AkiFX is an all-in-one effects rack: it consolidates the complete functional plugin sets of two well-known open-source projects — Andrew Reeman's **SpectralSuite** (a C++/JUCE spectral processing suite) and Robbert van der Helm's **nih-plug** framework with its official plugin collection — into a **single chain with a single interface**, exported as VST3 and CLAP for use in any major DAW.

This document is the authoritative reference for all 21 effect modules: their **origin, signal-processing principle, parameters, and application guidance**.

---

## Table of Contents

1. [Installation & Loading](#1-installation--loading)
2. [Interface Overview](#2-interface-overview)
3. [Signal Chain & Module Ordering](#3-signal-chain--module-ordering)
4. [Module Reference](#4-module-reference)
   - Sources & MIDI (Sine Generator / MIDI Inverter / Poly Mod Synth)
   - Saturation & Excitation (Soft Vacuum / Crisp)
   - Spectral Effects (Spectral Gate / Frequency Shift / Frequency Magnet / Bin Scrambler / Morph / Phase Lock / Sinusoidal Shaped Filter / Playground)
   - Pitch & Splitting (Puberty Simulator / Crossover / Diopser)
   - Dynamics (Loudness War Winner / Spectral Compressor)
   - Glitch & Mastering (Buffr Glitch / Gain / Safety Limiter)
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

On first load, **all 21 modules are bypassed** — the plugin is equivalent to a straight wire. This is intentional: the spectral modules consume significant CPU, so you enable only what you need. **Module power states are saved with your project and restored automatically.**

Build from source: `cargo xtask bundle akifx --release` (requires the Rust stable MSVC toolchain).

---

## 2. Interface Overview

AkiFX uses a two-pane "rack + parameter panel" layout:

- **Rack (left)**: the 21 modules are listed vertically in chain order. Each row provides:
  - **LED power toggle**: switches between ACTIVE / BYPASSED. Bypassed modules are skipped entirely (zero CPU) with a guaranteed **bit-identical passthrough** — no audible difference whatsoever.
  - **Latency badge**: the sample delay introduced by that module (e.g. `2560 samples`), shown only when non-zero.
  - **Drag handle**: grab the handle on the left of a row to **drag-reorder** the processing chain in real time.
- **Parameter panel (right)**: all parameters of the selected module, with detailed tooltips (including Chinese descriptions).
- **Bottom status bar**: the effective total latency (sum of all enabled modules' latencies).
- **Window scaling**: the UI zoom factor is persisted with the project.

---

## 3. Signal Chain & Module Ordering

Default processing order:

```
Sources/MIDI → Saturation/Excitation → Spectral → Pitch → Split/Phase → Dynamics → Glitch → Master
 1. Sine Generator    5. Soft Vacuum      6. Spectral Gate        14. Puberty Simulator   17. Loudness War Winner   18. Buffr Glitch   20. Gain
 2. MIDI Inverter     6. Crisp            7. Frequency Shift                                                                            21. Safety Limiter
 3. Poly Mod Synth                        8. Frequency Magnet
 4. Playground                            9. Bin Scrambler
                                         10. Morph
                                         11. Phase Lock
                                         12. Sinusoidal Shaped Filter
                                         13. Spectral Compressor*
```

*See the rack for the exact numbering; Spectral Compressor sits in the dynamics section (slot 18).*

The rationale: sources first → tone shaping (saturation/excitation) → frequency-domain processing (increasing complexity) → pitch shifting → time-domain splitting and phase effects → dynamics → buffer glitching → master gain and safety limiting.

**The order is fully adjustable**: drag Crisp after the spectral modules, or move Gain mid-chain as a fader. Reorderings are saved with the project.

---

## 4. Module Reference

### 4.1 Sine Generator

- **Origin**: ported from the nih-plug `sine` example.
- **Principle**: a MIDI-triggered sine oscillator. The phase accumulator runs at f64 precision and frequency changes glide smoothly. Output level follows **note-on velocity** and can be modulated in real time via **polyphonic pressure**; both note-on and note-off pass through a 5 ms linear anti-click envelope.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Level | −24 to +6 dB | −12 dB | Output level |
  | Fallback Frequency | 20 to 20000 Hz | 440 Hz | Initial value of the frequency smoother |
- **Usage**: place first in the chain as a test source, for monitoring calibration, or as a clean exciter feeding the spectral modules. Completely silent without MIDI input.

### 4.2 MIDI Inverter

- **Origin**: ported from the nih-plug `midi_inverter` example, transformation rules matched line by line.
- **Principle**: a **pure MIDI effect** (audio is bit-identical passthrough). It "mirrors" MIDI events: channel `ch → 15−ch`, note `n → 127−n`, velocity/pressure/CC value `v → 1−v` (CC numbers are not inverted), across Note On/Off, polyphonic pressure, pitch bend, CC, Choke, and more. Transformed events are sent back to the host.
- **Parameters**: Enable.
- **Usage**: an experimental MIDI transformer. With the host track's MIDI thru engaged, the mirrored output stacks against the original performance for "two hands" effects; disabling Enable restores normality. When disabled the module **emits no events** (avoiding duplication with the host's thru).

### 4.3 Poly Mod Synth

- **Origin**: ported from the nih-plug `poly_mod_synth` example.
- **Principle**: a 16-voice subtractive synthesizer whose saw/pulse oscillators start from pseudo-random phases. Under CLAP hosts it supports **per-voice polyphonic modulation** (note-level gain/pitch automation); under VST3 it degrades to standard MIDI behaviour, exactly like upstream. Voice allocation prefers free slots and otherwise steals the oldest voice.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Gain | −36 to 0 dB | −12 dB | Output level |
  | Attack | 0 to 2000 ms | 200 ms | Linear attack |
  | Release | 0 to 2000 ms | 100 ms | Exponential release; voices recycle once fully decayed |
- **Usage**: place first as a sound source; the downstream spectral and filter modules become its effect chain.

### 4.4 Soft Vacuum

- **Origin**: ported from the nih-plug plugin `soft_vacuum`, whose algorithm derives from Chris Johnson's (Airwindows) **Hard Vacuum**, rewritten with permission and extended with oversampling.
- **Principle**: a multi-stage **diode-bridge waveshaping** saturator. Above 100 % Drive the algorithm cascades multiple distortion stages (count scales with drive²); Warmth controls the positive/negative half-wave asymmetry (DC bias and "sag"); Aura adds bias/input gain. The **slew signal is computed at base rate and oversampled separately from the audio** so the oversampled and non-oversampled versions track the same timbre — the defining detail of this port.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Drive | 0 to 200 % | 0 % | Distortion amount; >100 % engages cascaded stages |
  | Warmth | 0 to 100 % | 0 % | Half-wave asymmetry / tube warmth |
  | Aura | 0 to 100 % | 0 % | Extra bias gain (internally mapped to [0, π]) |
  | Output Gain | −40 to 0 dB | 0 dB | Output trim |
  | Mix | 0 to 100 % | 100 % | Linear dry/wet |
  | Oversampling | 1x / 2x / 4x / 8x / 16x | 2x | Multi-stage Lanczos3; 16x adds 80 samples of latency |
- **Usage**: drum bus warming, vocal "tube" character, bass harmonic excitation. At high Drive, 4x oversampling or more reduces aliasing. All parameters use **oversampling-aware smoothing** for zipper-free automation.

### 4.5 Crisp

- **Origin**: ported from the nih-plug plugin `crisp`.
- **Principle**: a noise ring-modulation exciter. A deterministic PCG32 generator produces white noise, shaped by adjustable high-/low-pass filters, and ring-modulates the input (pre-filtered by an adjustable low-pass) — extracting and re-injecting the signal's high-frequency content without altering pitch.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Amount | 0 to 100 % | 35 % | Excitation depth |
  | Mode | Soggy / Crispy / Crispy (alt) | Crispy | RM waveform selection (Soggy = full waveform; Crispy = positive half; Crispy (alt) is identical to Crispy — as it is upstream) |
  | Stereo Mode | Mono / Stereo | Stereo | Mono shares a single noise source |
  | RM Input LPF | up to 20 kHz | 22000 Hz (Disabled) | Pre-ring-mod low-pass |
  | Noise HPF / LPF | adjustable | 5 Hz / 22000 Hz | Noise shaping |
  | Output Gain | −24 to 0 dB | 0 dB | Output level |
  | Wet Only | switch | Off | Output the excitation only |
- **Usage**: add "air" and presence to vocals, acoustic guitar, or snare. Low Amount (10–30 %) with Wet Only makes a parallel exciter.

### 4.6 Spectral Gate

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: frequency-domain gating. An STFT (2048-point FFT, 4× overlap, Hann window) resolves the signal into 1024 bands whose energies are compared against a threshold: bands below it are zeroed (or partially retained via the Tilt curve), bands above pass. The Weak/Strong Balance control determines how retained bands are weighted by energy — from an extreme resonant "only the loudest bands survive" effect to near-transparent expansion.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Cutoff | 0 to 1 (displayed in dB) | 0.6 (−4 dB) | Threshold |
  | Weak/Strong Balance | 0 to 1 | 0.7 | Retention of weak bands (0 = strongest only, 1 = near-transparent) |
  | Tilt | 0 to 1 | 0.5 | Threshold tilt across frequency |
  | Enable Tilt | switch | Off | Enables the tilt |
- **Usage**: robotic/underwater vocals, spectral drum restructuring, formant extraction from ambience. Balance defines the character; Cutoff defines the intensity.

### 4.7 Frequency Shift

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: shifts all spectral bins by a fixed amount (a true frequency shift, not pitch shifting — harmonic spacing is destroyed, producing metallic/inharmonic textures), or **scales** the spectrum: scale > 1 spreads harmonics apart, scale < 1 compresses them into denser inharmonic structures.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Frequency Shift | −500 to +500 Hz | 0 Hz | Uniform shift (quantised to bins, matching upstream) |
  | Frequency Scale | 0.25 to 3.0 | 1.0 | Spectral scaling |
- **Usage**: small positive/negative shifts for ring-modulator metallic sheen; scale < 1 to "granulate" drums; scale > 1 for brass-like spectral stretching. Both may be combined.

### 4.8 Frequency Magnet

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: "magnetises" spectral energy toward a **target frequency**: bins below the target are pulled upward along an exponential curve, bins above pulled downward along another, forming an energy funnel centred on the target. Strength sets the pull intensity (inverted internally into a width), and Width Bias shapes the curve.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Frequency | 20 to 2000 Hz | 800 Hz | Magnet target |
  | Strength | 0 to 100 % | 50 % | Pull intensity (0 = passthrough) |
  | Width Bias | 0 to 1 | 0.01 | Curve bias |
  | Use Legacy Mode | switch | Off | Upstream's legacy curve (no target-frequency offset) |
- **Usage**: pseudo-formant focusing on polyphonic material, or pushing noise into specific bands for wind/whistle textures. Moderate Strength is the most musical; full Strength yields heavy metallic coloration.

### 4.9 Bin Scrambler

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: periodically shuffles the **ordering** of the 1024 spectral bins and linearly **crossfades each bin's magnitude and phase** between the old and new permutations (t = phase/period) for smooth "spectral shuffling" at a musical rate. Scatter sprinkles low-bin indices into the lower-mid region; Scramble shuffles in chunks.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Scramble | 0 to 100 % | 10 % | Shuffle amount (0 = passthrough) |
  | Scatter | 0 to 100 % | 40 % | Low-bin sprinkle amount |
  | Rate | 0.25 to 15 Hz | 2 Hz | Reshuffle rate |
  | Random Seed | 0 to 9999 | 0 | 0 = random each session; non-zero = reproducible |
- **Usage**: fractured drum loops, granular ambience. Sync Rate to the project tempo (integer or fractional ratios) for rhythmic results.

### 4.10 Morph

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: spectral **permutation mapping** — bin *i*'s energy is relocated to a destination computed from a 16-control-point curve, stretching or compressing the spectral structure (e.g. pulling the harmonic series apart). The default diagonal curve is passthrough.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Mix | 0 to 100 % | 100 % | Blend of processed vs. dry signal |
  | Number of Overlaps | 1 / 2 / 4 / 8 | 4 | STFT overlap (higher = smoother, more CPU) |
  | Use PVOC | switch | On | Off = complete passthrough (bypasses the frequency domain) |
  | Control Points ×16 | 0 to 1 | linear | Bin mapping curve |
- **Usage**: stretch cymbal spectra into metallic storms; relocate vocal formants. Move a mid control point to begin morphing.

### 4.11 Phase Lock

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: captures and "freezes" a snapshot of the spectral **phases and magnitudes**: subsequent frames have their phases pulled toward the locked values (Frequency Lock + Phase Mix) and optionally their magnitudes too (Magnitude Lock + Magnitude Mix), producing the signature metallic spectral-freeze sustain. Morph oscillates between locked and live states over an adjustable duration; Random Phase injects jitter. **Each channel and each STFT overlap instance keeps an independent snapshot** (as upstream), preserving the stereo image.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Frequency Lock | switch | Off | Engages phase locking (with transition) |
  | Phase Mix | 0 to 100 % | 100 % | Amount of phase pulled toward the lock |
  | Magnitude Lock | switch | Off | Engages magnitude locking |
  | Magnitude Mix / Tracking | 0 to 100 % | 100 % / 0 % | Lock ratio / follow live magnitude |
  | Random Phase | 0 to 100 % | 0 % | Random phase jitter |
  | Morph | switch + Duration 1–30 s | Off / 2 s | Locked↔live transition |
- **Usage**: frozen vocal sustains, metallic pads. Solo a phrase, engage Frequency Lock, then sweep Phase Mix from 0 to 100 %.

### 4.12 Sinusoidal Shaped Filter

- **Origin**: ported from the SpectralSuite plugin of the same name.
- **Principle**: shapes spectral magnitudes bin-by-bin with a 1024-point sine lookup table (true linear interpolation): each bin reads the table at its index-derived position and its magnitude is scaled by `value^(width²·8+1)` — equivalent to a comb-filter bank whose shape is dictated by a sine wave. Frequency shifts the read position; Phase offsets the table start.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Frequency | 0 to 10 | 7 | Table read step (comb density) |
  | Width | 0 to 1 | 0.9 | Shaping depth |
  | Phase | 0 to 1 | 0.5 | Table phase offset |
  | Mix / Use PVOC / Overlaps | — | 100 % / On / 4 | As in Morph |
- **Usage**: tonal comb resonances (a frequency-domain auto-wah), synth-like harmonic reshaping of vocals.

### 4.13 Spectral Compressor

- **Origin**: ported from the nih-plug plugin `spectral_compressor`.
- **Principle**: splits the signal into up to 16384 bands and applies **independent upward + downward compression per band**: the downward compressor tames loud bands (taming harsh resonances) while the upward compressor lifts quiet ones (pulling up the noise floor and air); combined they approximate arbitrary spectral-envelope shaping. The threshold is not a scalar but a **frequency-tilted curve** (Pink Noise mode includes the −3 dB/oct compensation). Bins below ~20 Hz are excluded from upward compression (avoiding DC-leakage amplification), and bins below −100 dB are not upward-compressed (upstream's noise-floor guard).
- **Parameters** (grouped):
  | Group | Parameter | Default | Description |
  |---|---|---|---|
  | Global | Output Gain | 0 dB | Output trim |
  | Global | Mix | 100 % | Dry/wet (15 ms smoothing) |
  | Global | Window Size / Overlap | 2048 / 16x | STFT configuration (64–32768 / 4–32x) |
  | Global | Attack / Release | 150 / 300 ms | Envelope times |
  | Threshold | Global Threshold | −12 dB | Base threshold |
  | Threshold | Center / Slope / Curve | 420 Hz / 0 / 0 | Threshold curve shape (Pink Noise mode includes −3 dB/oct) |
  | Upwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 75 % / 6 dB | Upward compressor |
  | Downwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 0 % / 6 dB | Downward compressor |
- **Usage**: "spectral glue" — low ratios at high overlap for transparent mastering colour; high upward ratios to turn whispers into roars; high downward ratio with a narrow curve = precise harshness removal. All curve parameters support **real-time automation**.

### 4.14 Puberty Simulator

- **Origin**: ported from the nih-plug plugin `puberty_simulator`.
- **Principle**: an FFT phase-vocoder **pitch-down** shifter (default −1 octave, hence the name). Each frame is windowed, transformed, shifted to lower bins, and reconstructed via overlap-add. Rectangular and polar interpolation modes are provided. Latency equals the window length (default 1024 samples) and is reported automatically.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Pitch | −5 to +5 octaves | −1 | Pitch shift (negative = down) |
  | Window Size | 64 to 32768 | 1024 | STFT window |
  | Window Overlap | 4x to 32x | 8x | Overlap factor |
  | Mode | Rectangular / Polar | Rectangular | Bin interpolation method |
- **Usage**: deep/monster vocals, bass octave doubles. Upstream notes that the algorithm intentionally retains "characteristic artifacts" — it is an effect, not a transparent transposer.

### 4.15 Crossover

- **Origin**: ported from the nih-plug plugin `crossover` (IIR section).
- **Principle**: a **Linkwitz-Riley 24 dB/oct (LR4)** multiband splitter: each crossover stage is two cascaded Butterworth low-pass/high-pass sections, phase-aligned at the crossover point so the bands sum flat; low-passed bands pass through an **all-pass cascade phase compensation**. AkiFX adds an independent per-band output gain on top of upstream.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Bands | 2 to 5 | 2 | Number of bands |
  | Crossover 1–4 Freq | 40 Hz to 20 kHz | 200 / 1000 / 5000 / 10000 Hz | Crossover points |
  | Band 1–5 Gain | −24 to +24 dB | 0 dB | Per-band output gain |
  | Crossover Type | LR24 / LR24 (LP) | LR24 | The linear-phase FIR variant is a placeholder (see §7) |
- **Usage**: multiband routing (with host sidechaining), monitor-style band soloing, or five "band faders" for extreme spectral sculpting.

### 4.16 Diopser

- **Origin**: ported from the nih-plug plugin `diopser`.
- **Principle**: a **cascaded all-pass filter** bank (up to 512 stages). All-pass filters leave magnitude untouched while rotating phase; many cascaded stages create enormous phase delay around the tuned frequency, and sweeping it yields a flanger-like but much "thicker" resonance. Spread fans the stages' resonant frequencies into a filter cloud along octave or linear rules.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Filter Stages | 0 to 512 | 0 (passthrough) | Stage count |
  | Filter Frequency | 5 Hz to 20 kHz | 200 Hz | Base frequency |
  | Filter Resonance | 0.01 to 30 | 0.5 | Resonance sharpness |
  | Filter Spread | ±5 octaves | 0 | Stage frequency spread |
  | Spread Style | Octaves / Linear | Octaves | Spread law |
- **Usage**: 32–128 stages + resonance 5–20 + a sweep = the classic "Diopser roar". All parameters glide smoothly under automation.

### 4.17 Loudness War Winner

- **Origin**: ported from the nih-plug plugin `loudness_war_winner` — a **joke plugin** (official tagline: "Win the loudness war with ease"; VST3 subcategory "Pain").
- **Principle**: not a compressor at all: **every non-silent sample is hard-output as `sign(x) × Output Gain`** — any signal becomes a square wave at the output gain (default −24 dBFS), maxing LUFS and ruining the listen. WIN HARDER engages a 5.5 kHz four-stage bandpass whose Q rises with the parameter (Q = 0.00001 + f×30; a nod to the LUFS K-weighting high-frequency shelf), making the square even more punishing. To avoid an endless DC square, silence begins fading after 1 s and is fully muted after 2 s.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Output Gain | −24 to 0 dB | −24 dB | Square-wave level |
  | WIN HARDER | 0 to 100 % | 0 % | Bandpass torment intensity |
- **Usage**: almost exclusively for testing monitor chains, pranking colleagues, or demonstrating why loudness-wars mastering is absurd. Do not engage on real material unless pain is the goal.

### 4.18 Buffr Glitch

- **Origin**: ported from the nih-plug plugin `buffr_glitch`.
- **Principle**: a MIDI-triggered **buffer repeat/stutter** effect. On NoteOn a voice records exactly one period of the input (pitch = note frequency × octave shift), then loops it with an equal-power crossfade to hide the seam. Up to 8 voices, allocated free-slot-first with quietest-releasing stealing. Supports **PolyVolume** per-note volume automation. The dry signal is ducked by the active voice envelope.
- **Parameters**:
  | Parameter | Range | Default | Description |
  |---|---|---|---|
  | Dry Mix | 0 to 1 | 1.0 | Dry level (ducked by active voices) |
  | Velocity Sensitive | switch | Off | Maps velocity to voice gain |
  | Octave Shift | −2 to +2 | 0 | Buffer playback pitch shift |
  | Attack / Release | 0 to 50 ms | 2 ms | Voice envelope |
  | Crossfade | 0 to 50 ms | 2 ms | Loop-seam crossfade |
- **Usage**: mount on vocals or snare and "freeze-repeat" it from a keyboard — a performance-oriented effect.

### 4.19 Gain

- **Origin**: ported from the nih-plug `gain` example.
- **Principle**: a clean digital gain stage with 50 ms logarithmic smoothing for click-free adjustment.
- **Parameters**: Gain, −30 to +30 dB, default 0 dB.
- **Usage**: level trimming anywhere in the chain; the smoothed law makes it safe for fine automation rides.

### 4.20 Safety Limiter

- **Origin**: ported from the nih-plug plugin `safety_limiter`.
- **Principle**: not a "musical" limiter but a **fuse**: any sample above threshold triggers a **SOS Morse-code** alert played on a 420 Hz sine and immediately pulls the level down; after 1 s of silence it returns to passthrough, and sustained over-threshold keeps alarming. NaN/Inf samples (digital blowups) are muted **and trigger the alert** — its core value: put one on the master and you will *hear* when something breaks.
- **Parameters**: Threshold, −24 to +12 dB (gain-skewed), default 0 dB.
- **Usage**: last slot on the master chain at around +6 dB as a blowout fuse; if you hear SOS, something upstream is clipping.

### 4.21 Playground

- **Origin**: ported from SpectralSuite's Playground.
- **Principle**: an **identity spectral processor** — the full STFT→FFT→identity-callback→IFFT→OLA pipeline with no spectral modification (reconstruction error within numerical tolerance). It exists as a developer/learner template for spectral plugins and as a neutral test point for the pipeline itself.
- **Parameters**: Mix (100 %), Use PVOC (On), Number of Overlaps (4), plus two **inert** compatibility toggles (unused upstream as well).
- **Usage**: use it as a passthrough reference: toggling Playground bypassed vs. active verifies that host PDC is aligned.

---

## 5. MIDI Support

AkiFX declares MIDI input/output at the `MidiCCs` level: **all host MIDI events (notes, CC, pitch bend, polyphonic pressure, Choke, …) are broadcast to every enabled module** in the chain, and each module consumes what it understands:

| Module | MIDI consumed |
|---|---|
| Sine Generator | Note On/Off (velocity → gain), Poly Pressure (→ gain) |
| Poly Mod Synth | Note On/Off, Choke, (CLAP) polyphonic modulation |
| Buffr Glitch | Note On/Off, PolyVolume (per-note volume) |
| MIDI Inverter | All event types (transformed and returned to the host) |

Events produced by MIDI Inverter are sent to the host through the plugin's MIDI output.

---

## 6. Latency & Plug-in Delay Compensation (PDC)

Several modules carry **algorithmic latency**; the plugin re-reports it to the host **within the same processing block** whenever it changes:

| Module | Latency |
|---|---|
| SpectralSuite family (Gate/Shift/Magnet/Scrambler/Morph/PhaseLock/SSF/Playground) | 2560 samples (FFT 2048 + hop 512, matching the C++ originals) |
| Spectral Compressor | window length (default 2048) |
| Puberty Simulator | window length (default 1024) |
| Soft Vacuum | oversampler kernel latency (~10 samples at 2x) |

The host uses this for automatic delay compensation. Toggling modules, changing window sizes, or changing oversampling updates the report immediately — **no manual adjustment required**. If your host's PDC is deficient, use the total shown in the bottom status bar.

---

## 7. Differences from the Original Plugins

AkiFX was calibrated line-by-line against the upstreams (nih-plug @ `f36931f`, SpectralSuite @ `5dfd294`); with default parameters the sound matches the originals. Known intentional differences:

1. **Integration-driven (by design)**: the single-plugin chain, chain-level bypass (bit-identical passthrough), drag-to-reorder, broadcast MIDI routing, and MIDI Inverter emitting nothing when disabled (avoiding duplication with the host's thru).
2. **Trimmed features**:
   - SpectralSuite's FFT-size selection (2^7–2^20), window-type selection, and PVOC phase-vocoder mode are not exposed (fixed at 2048 / 4× / Hann — the originals' defaults);
   - Frequency Magnet's MIDI triggers (note = frequency, CC1 = width) are not ported;
   - Spectral Compressor's sidechain modes (Sidechain Match/Compress) are present as enum values but not wired — only Pink Noise is functional;
   - Crossover's linear-phase FIR variant is a placeholder (selecting LR24 (LP) uses the IIR LR24);
   - Diopser's automation-precision parameter and built-in spectrum analyser GUI are not ported (default precision = per-sample smoothing, behaviour identical).
3. **Upstream quirks preserved**: Crisp's Crispy (alt) mode is identical to Crispy (as upstream); Loudness War Winner's square-waving and 5.5 kHz WIN HARDER bandpass are ported verbatim.
4. **Enhancements not present upstream**: real-time automation of Spectral Compressor's threshold curves (a fix over the original's callback wiring), band gains (Crossover), chain-level persisted bypass, drag reordering, and allocation-free audio processing in every module.

---

## 8. FAQ

**Q: No sound after loading?**
All modules start bypassed. Light an LED on the rack (or at least Gain). Still silent? Check whether Safety Limiter is playing SOS — that is the over-threshold alarm.

**Q: Why do only some modules show latency badges?**
Only STFT/oversampling modules have algorithmic latency. The host compensates automatically; the badges are informational.

**Q: Are module power states remembered?**
Yes, they are saved with the project (since v0.2.1). Projects saved with v0.2.0 or earlier load all-bypassed once.

**Q: Spectral Compressor ignores my threshold changes?**
v0.2.0 had a defect where threshold automation never applied; please upgrade. Curve changes take effect on the following processing block.

**Q: High CPU usage?**
Each spectral module owns an independent 2048-point FFT engine (exactly as each original plugin would). CPU scales linearly with the number of lit spectral modules; bypassed modules cost nothing.

---

*AkiFX © Akiro · Licensed GPLv3. SpectralSuite (Unlicense); nih-plug and its plugins (ISC/GPL-3.0); the Hard Vacuum algorithm © Chris Johnson (Airwindows).*

# AkiFX

**A free, open-source all-in-one effects rack for your DAW** — 11 core effect modules consolidated into a single VST3/CLAP plugin with one chain, one interface.

AkiFX unifies the core effects of two well-known open-source projects into a single instrument-grade effect chain: **[SpectralSuite](https://github.com/andrewreeman/SpectralSuite)** by Andrew Reeman (a C++/JUCE spectral processing suite) and the **[nih-plug](https://github.com/robbert-vdh/nih-plug)** framework's official plugin collection by Robbert van der Helm. Every module has been calibrated against its upstream source so that, at default settings, it sounds like the original.

- **Formats**: VST3 · CLAP · Standalone (WASAPI/JACK)
- **Platforms**: Windows (macOS/Linux buildable — the DSP is pure Rust)
- **License**: GPLv3

---

## Highlights

- **One plugin, 11 effects** — spectral processors, saturation, dynamics, a resonance sweep, glitch and mastering tools in one insert slot.
- **Free module ordering** — drag-and-drop the processing chain in real time; reorderings persist with your project.
- **Per-module power switches** — bypassed modules are skipped entirely (zero CPU) with a bit-identical passthrough guarantee; power states are saved with the session.
- **Global bypass** — one host-automatable button turns the whole plugin into a straight wire.
- **Live output metering** — stereo peak meters in the bottom strip, fed from the audio thread.
- **Spectrum view** — the Spectral Compressor draws its live input spectrum with the threshold curve overlaid on a log-frequency axis; tune thresholds by eye.
- **Correct plug-in delay compensation** — algorithmic latency is reported dynamically and updates within the same processing block whenever it changes.
- **Full MIDI support** — notes, CC, pitch bend, polyphonic pressure and per-note expression are broadcast to every enabled module.
- **Real-time safe** — no heap allocations, no panics and no unbounded waits on the audio thread.
- **Bilingual UI & docs** — English/Chinese interface with per-parameter tooltips; manuals in both languages.

> **Upgrading from v0.2.x?** v0.3.0 streamlines the module set from 21 to 11 and is a breaking release: saved module states, ordering and parameters reset to defaults.

---

## The Modules

| # | Module | Origin | Description |
|---|---|---|---|
| 1 | **Sine Generator** | nih-plug `sine` | MIDI-triggered sine source with velocity/pressure gain control and anti-click envelopes |
| 2 | **Soft Vacuum** | nih-plug `soft_vacuum` (Airwindows Hard Vacuum) | Multi-stage diode-bridge saturator with up to 16× Lanczos3 oversampling |
| 3 | **Crisp** | nih-plug `crisp` | Noise ring-modulation high-frequency exciter |
| 4 | **Spectral Gate** | SpectralSuite | Frequency-domain gate with energy-weighted band retention and threshold tilt |
| 5 | **Frequency Shift** | SpectralSuite | Uniform bin shift and spectral scaling for metallic, inharmonic textures |
| 6 | **Spectral Compressor** | nih-plug `spectral_compressor` | Up to 16384 bands of independent upward/downward compression, live spectrum + threshold curve view |
| 7 | **Crossover** | nih-plug `crossover` | Linkwitz-Riley 24 dB/oct 2–5-band splitter with phase compensation and per-band gains |
| 8 | **Diopser** | nih-plug `diopser` | Up to 512 cascaded all-pass filters — a phase-rotation resonance sweep |
| 9 | **Buffr Glitch** | nih-plug `buffr_glitch` | MIDI-triggered buffer repeats/stutter with 8 voices and per-note volume automation |
| 10 | **Gain** | nih-plug `gain` | Clean ±30 dB gain stage with click-free smoothing |
| 11 | **Safety Limiter** | nih-plug `safety_limiter` | The fuse: over-threshold or NaN/Inf triggers a SOS Morse-code alarm |

---

## Installation

Build from source (Rust stable + MSVC on Windows; first build fetches nih-plug from the pinned revision):

```shell
git clone <this repository>
cd AkiFX
cargo xtask bundle akifx --release
```

Artifacts land in `target/bundled/`:

| Format | File | Install to |
|---|---|---|
| VST3 | `AkiFX.vst3/` | `C:\Program Files\Common Files\VST3` (Windows) · `/Library/Audio/Plug-Ins/VST3` (macOS) |
| CLAP | `AkiFX.clap` | `C:\Program Files\Common Files\CLAP` |
| Standalone | `AkiFX.exe` | Run directly, no DAW required |

Run the test suite:

```shell
cargo test -p akifx
```

### Quick Start

1. Load **AkiFX** on a track. All modules start **bypassed** — the plugin is a straight wire.
2. Click the **LEDs** on the rack to enable the modules you need (states persist with the project).
3. Select a module to edit its **parameters** on the right; hover any control for a tooltip. Toggle-style parameters are single buttons; numeric parameters are sliders.
4. **Drag the handles** on the left of each row to reorder the chain in real time.
5. Use **GLOBAL BYPASS** in the bottom strip to A/B the whole chain, and the **peak meters** to watch your output level.

> No sound? Light at least one module. Hearing SOS Morse code? That's the Safety Limiter telling you something upstream is clipping.

---

## Latency & PDC

Spectral modules carry algorithmic latency which is reported to the host and re-reported **within the same processing block** whenever it changes (toggling modules, resizing STFT windows, changing oversampling). No manual compensation required.

| Modules | Latency |
|---|---|
| Spectral Gate / Frequency Shift | 2560 samples (FFT 2048 + hop 512) |
| Spectral Compressor | window length (default 2048) |
| Soft Vacuum | oversampler kernel latency (~10 samples at 2×) |

---

## Documentation

| Document | Languages |
|---|---|
| [User Manual — all 11 modules: origin, principle, parameters, usage](docs/USER_MANUAL_en.md) | [中文版](docs/USER_MANUAL_zh.md) |
| [Technical Overview — architecture, registry, STFT engine, DSP internals, real-time safety](docs/TECHNICAL_OVERVIEW_en.md) | [中文版](docs/TECHNICAL_OVERVIEW_zh.md) |

---

## Fidelity & Attribution

AkiFX is calibrated against the upstream repositories (SpectralSuite @ `5dfd294`, nih-plug @ `f36931f`): formulas, constants, default values and control flow were audited module by module, and upstream quirks are preserved on purpose. Intentional integration differences and trimmed upstream features are documented in the user manual.

Standing on the shoulders of:

- **[SpectralSuite](https://github.com/andrewreeman/SpectralSuite)** by Andrew Reeman — Unlicense
- **[nih-plug](https://github.com/robbert-vdh/nih-plug)** by Robbert van der Helm and its plugins — ISC / GPL-3.0
- **Hard Vacuum** algorithm by Chris Johnson ([Airwindows](https://www.airwindows.com/))

Heartfelt thanks to both authors for releasing their work as free software.

## License

AkiFX is licensed under the **[GNU GPLv3](LICENSE)** (required by the VST3 binding of nih-plug). See [LICENSE](LICENSE).

---

<!-- 中文文档 ↓ -->

# AkiFX（中文说明）

**免费开源的 DAW 全家桶效果器机架** —— 11 个核心效果模块整合为单个 VST3/CLAP 插件，一条链、一个界面。

AkiFX 将两个知名开源项目的核心效果整合到一条乐器级效果链中：Andrew Reeman 的 **SpectralSuite**（C++/JUCE 频谱处理套件）与 Robbert van der Helm 的 **nih-plug** 框架及其官方插件集。所有模块均对照上游源码逐行校准，默认参数下与原版插件听感一致。

- **格式**：VST3 · CLAP · 独立运行版（WASAPI/JACK）
- **平台**：Windows（macOS/Linux 可自行构建——DSP 为纯 Rust）
- **许可证**：GPLv3

---

## 特性

- **一个插件，11 个效果**——频谱处理、饱和、动态、共鸣扫频、故障与母带工具，只占一个插入槽位。
- **模块顺序自由**——实时拖拽重排处理链，顺序随工程保存。
- **逐模块电源开关**——旁路模块完全跳过（零 CPU），保证位一致直通；开关状态随工程持久化。
- **全局旁路**——一个可在宿主中自动化的按钮，把整个插件变成直通线。
- **实时输出表**——底部条内置立体声峰值表，数据来自音频线程。
- **频谱视图**——Spectral Compressor 实时绘制输入频谱与门限曲线（对数频率轴），调门限所见即所得。
- **正确的自动延迟补偿**——算法延迟动态上报宿主，变化时同一处理块内更新。
- **完整 MIDI 支持**——音符、CC、弯音、复音压力、逐音符表达全部广播给已启用模块。
- **实时安全**——音频线程无堆分配、无 panic、无无限等待。
- **中英双语界面与文档**——全参数悬停提示；手册提供中英双语。

> **从 v0.2.x 升级？** v0.3.0 将模块集从 21 个精简到 11 个，是破坏性版本：已保存的模块状态、顺序与参数会重置为默认。

## 模块一览

| # | 模块 | 一句话说明 |
|---|---|---|
| 1 | Sine Generator | MIDI 正弦音源，力度/压力控增益，防 click 包络 |
| 2 | Soft Vacuum | 二极管桥式多级饱和，最高 16x Lanczos3 过采样 |
| 3 | Crisp | 噪声环调高频激励器 |
| 4 | Spectral Gate | 频域门限，能量加权保留 + 门限倾斜 |
| 5 | Frequency Shift | 频谱整体搬移/缩放，金属非谐波质感 |
| 6 | Spectral Compressor | 最多 16384 频段独立上下行压缩，实时频谱 + 门限曲线视图 |
| 7 | Crossover | LR4 24 dB/oct 2–5 段分频 + 频段增益 |
| 8 | Diopser | 最多 512 级全通共鸣扫频 |
| 9 | Buffr Glitch | MIDI 触发缓冲口吃，8 复音 + 逐音符音量 |
| 10 | Gain | ±30 dB 纯净增益 |
| 11 | Safety Limiter | 超阈值/NaN 播报 SOS 的保险丝 |

每个模块的详细说明（由来、原理、参数、用法）见[用户手册](docs/USER_MANUAL_zh.md)。

## 安装

从源码构建（Windows 需 Rust stable + MSVC；首次构建会拉取锁定版本的 nih-plug）：

```shell
git clone <本仓库>
cd AkiFX
cargo xtask bundle akifx --release
```

产物位于 `target/bundled/`：

| 格式 | 文件 | 安装位置 |
|---|---|---|
| VST3 | `AkiFX.vst3/` | `C:\Program Files\Common Files\VST3`（Windows）· `/Library/Audio/Plug-Ins/VST3`（macOS） |
| CLAP | `AkiFX.clap` | `C:\Program Files\Common Files\CLAP` |
| 独立版 | `AkiFX.exe` | 直接运行，无需宿主 |

运行测试：

```shell
cargo test -p akifx
```

### 快速上手

1. 在音轨上加载 **AkiFX**。所有模块默认**旁路**——插件等价于直通线。
2. 点击机架上的 **LED** 点亮需要的模块（状态随工程保存）。
3. 选中模块后在右侧编辑**参数**：开关型参数是单个按钮，数值参数是滑条，悬停可看提示。
4. **拖拽**每行左侧的把手实时重排处理链。
5. 底部条的 **GLOBAL BYPASS** 一键对比整条链，**峰值表**监视输出电平。

> 没有声音？先点亮至少一个模块。听到 SOS 摩尔斯电码？那是 Safety Limiter 在提示上游削波了。

## 延迟与 PDC

频谱模块带有算法延迟，插件会动态上报宿主，并在开关模块、调整 STFT 窗口或过采样倍数时**同一处理块内**更新——无需手动补偿。

| 模块 | 延迟 |
|---|---|
| Spectral Gate / Frequency Shift | 2560 samples（FFT 2048 + hop 512） |
| Spectral Compressor | 窗口长度（默认 2048） |
| Soft Vacuum | 过采样核延迟（2x 时约 10 samples） |

## 文档

| 文档 | 语言 |
|---|---|
| [用户手册 —— 11 个模块的由来/原理/参数/用法](docs/USER_MANUAL_zh.md) | [English](docs/USER_MANUAL_en.md) |
| [技术文档 —— 架构、注册表、STFT 引擎、DSP 实现、实时安全](docs/TECHNICAL_OVERVIEW_zh.md) | [English](docs/TECHNICAL_OVERVIEW_en.md) |

## 上游与致谢

AkiFX 对照上游仓库（SpectralSuite @ `5dfd294`、nih-plug @ `f36931f`）逐行校准：公式、常数、默认值与控制流均经模块级审计，上游怪癖被有意保留。集成性差异与裁剪的功能在用户手册中有完整说明。

站在以下项目的肩膀上：

- **[SpectralSuite](https://github.com/andrewreeman/SpectralSuite)**（Andrew Reeman）— Unlicense
- **[nih-plug](https://github.com/robbert-vdh/nih-plug)** 及其插件（Robbert van der Helm）— ISC / GPL-3.0
- **Hard Vacuum** 算法 — Chris Johnson（[Airwindows](https://www.airwindows.com/)）

向两位作者以自由软件发布作品致以诚挚感谢。

## 许可证

AkiFX 以 **[GNU GPLv3](LICENSE)** 授权（nih-plug 的 VST3 绑定所要求），详见 [LICENSE](LICENSE)。

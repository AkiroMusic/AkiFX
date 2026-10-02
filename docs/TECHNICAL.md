# AkiFX Technical Overview

*How AkiFX is built: signal chain, DSP internals, state model, UI architecture, and the test inventory.*

---

## 1 · Topology

```
 MIDI ──┐
        ▼
[Sine Generator]      test tone / MIDI voice          (0 latency)
   ▼
[Soft Vacuum]         oversampled tube saturation      (0)
   ▼
[Crisp]               RM + noise exciter               (0)
   ▼
[Spectral Gate]       per-bin gate      ┐
   ▼                                     ├ STFT 2048 / 4× overlap / Hann
[Frequency Shift]     per-bin shift     ┘   each reports 2560 samples
   ▼
[Spectral Compressor] realfft OLA, 16384 bands         (reports 2560)
   ▼
[Crossover]           Linkwitz-Riley IIR LR24          (0)
   ▼
[Diopser]             cascaded resonators              (0)
   ▼
[Buffr Glitch]        MIDI buffer stutter              (0)
   ▼
[Gain]                master gain                      (0)
   ▼
[Safety Limiter]      brickwall                        (0)
   ▼
   output (peak meters → global bypass gate → host)
```

Ordering is data, not code: `define_modules!` in `src/modules/registry.rs` is
the single table of record. One row per module generates the umbrella
parameter tree (`AkiFxParams`), the default chain, the GUI entries, and the
completeness tests. Processing order lives in a `SharedOrder` permutation
persisted as `module_order`; the audio thread reads it under a lock per block
and caches it allocation-free.

The global bypass is a `BoolParam`: when engaged, `AkiFx::process()` returns
before touching the buffer — a bit-identical passthrough by construction, and
host-automatable like any parameter.

## 2 · Spectral engine

Spectral Gate, Frequency Shift and the Spectral Compressor's analysis stage
share one STFT design (the SpectralSuite lineage):

- 2048-point FFT, 4× overlap (hop 512), Hann analysis and synthesis windows.
- The engine advances per-sample (not per hop-chunk), so stereo blocks never
  lose samples or misalign frames; each channel owns its own FIFO state.
- Single-sided polar spectra with 2/N normalization; overlap-add scales by
  Σw² (Hann at 4× measures ≈ 1.5, verified by the identity null test).
- **Latency**: the engine reports `fft + hop` (2560 samples) — conservative,
  matching the upstream C++ plugins' report — while the true signal delay is
  `fft` (2048). Hosts compensating 2560 samples per module are always safe.
- With a neutral per-bin callback, reconstruction at the true delay measures
  RMSE ≈ 4.7×10⁻⁴ (window ripple floor, ≈ 1/fft_size — inaudible). The
  symmetric-Hann COLA ripple is inherent and intentionally left alone.

The Spectral Compressor itself runs the realfft overlap-add pipeline from the
upstream port: per-bin downward and upward compressors over 16384 bands
(8192 bins × 2 channels) with a configurable log-log threshold curve, plus
per-block parameter-change flagging and a dry/wet crossfade.

## 3 · Module notes

- **Soft Vacuum** renders its smoothing block once per audio block and shares
  it across channels; the oversampling choice (2×/4×/8×) is an `IntParam`
  whose callback publishes an `Arc<AtomicF32>` consumed by the per-channel
  processors. Block buffers pre-allocate at 512 and grow on demand — the
  audio thread never allocates.
- **Crisp** smooths its filter coefficients per sample with TDF-II biquads
  (RBJ coefficients with clamped, sanitized inputs; a shared `dsp::Biquad`).
- **Crossover** keeps one IIR crossover per channel and smooths band gains
  per sample.
- **Diopser** switches between raw and smoothed filter updates per sample
  while parameters move.
- **Buffr Glitch** voices own per-voice gain smoothers; `note_on` guards
  against unallocated buffers.
- **Safety Limiter** treats any non-finite sample as an instant peak.
- Smoothing rates follow the module each effect was ported from; they were
  calibrated against upstream behavior and are documented in the port notes
  rather than re-tuned.

## 4 · Parameters, presets and state

Everything the host saves lives in `AkiFxParams` (plus the chain's
`module_order`/`module_enabled` and `ui_zoom`): parameters, module power
state and rack order round-trip through the host's state chunks. Parameter
IDs are stable; presets address them by id.

Factory presets (`src/presets.rs`) are a compile-time table of normalized
(0..1) value patches. Two guarantees are structural:

1. **Red-line modules are unaddressable.** `is_red_line_module` matches the
   Gain and Safety Limiter rows by registry prefix; `apply_to_sink` re-checks
   before every write, and tests fail if a preset ever references them.
   Presets therefore cannot move your output gain, your limiter, or the
   global bypass.
2. **Presets only power the modules they patch.** Modules outside a recipe
   keep the user's power state. `Init` is the single exception that resets
   every effect module to defaults and bypasses it.

At runtime the GUI applies a preset through a `PatchSink` that drives
nih-plug's `ParamSetter` (begin / set-normalized / end gesture groups, so
host automation sees coherent moves) and then mirrors module power into the
persisted enable flags.

## 5 · GUI implementation

The editor is egui (via `nih_plug_egui`) in the Aki "Aurora Glass" design
language. Architecture notes:

- **One color pack, zero hardcoded colors.** `gui/theme.rs` defines
  `ColorPack` (surfaces, text tiers, accents, status, the three-color gradient
  ramp, aurora intensities, liquid-glass material tokens, shadow tint). The
  active pack is *Mint Fresh* (the family light theme, §3.4); adding a
  theme means adding one struct —
  component code reads `pal()` only. Derived colors go through `mix()` /
  `with_alpha()` (the `color-mix()` equivalents).
- **Three font families**, all embedded OFL TTFs validated by sfnt magic
  before registration (an unparsable font would panic inside the host):
  Plus Jakarta Sans for UI text, Fraunces 72pt for display/brand, IBM Plex
  Mono for readouts; Noto Sans SC is the CJK fallback. Zoom (60–200%) is
  implemented at font-size level because the baseview renderer ignores
  `pixels_per_point`.
- **Materials.** Content cards are liquid glass: translucent fill over the
  aurora curtain, 1 px specular line on top, 1 px inner shade below, outer
  border plus an inset (5 px) double-bezel line, soft tinted shadow. The
  aurora curtain itself is four elliptical gradient meshes along the ramp
  plus a deterministic grain-noise tile (cached `TextureHandle`).
- **Flow borders and glow** are budgeted: one shimmer (the global-bypass
  pill while engaged), two neon edges (selected rack pill, spectrum panel),
  one star with a bloom-in animation, one signature quote in About. Glow is
  only drawn on active elements, using gradient meshes rather than CPU blur.
- **Performance gates.** The spectrum panel redraws its curves only while
  the published snapshot carries energy (silent frames skip tessellation and
  poll at 10 Hz); curves are max-pooled to ≤ 420 points; meters decay in the
  GUI at a fixed per-frame rate with instant attack; the only continuous
  repaints while idle are the bypass shimmer's 30 Hz and the meter decay.
- **Audio-thread data.** Peaks travel through `Arc<AtomicF32>`; the spectrum
  snapshot publishes through a mutex-swapped `Arc` every third block. The GUI
  thread never touches module state directly except through the bypass
  atomics and `ParamSetter`.

## 6 · Test inventory

`cargo test --workspace` runs 181 unit + 32 integration tests. The load-bearing ones:

| Test | Asserts |
|---|---|
| `all_bypassed_produces_bit_identical_passthrough` | bypassed chain is bit-exact |
| `stereo_consistency_identical_inputs` | identical L/R inputs → bit-identical L/R outputs through the fully enabled chain |
| `latency_aligned_null_spectral_core` | reported latency == fft+hop; identity reconstruction at the true delay, gain-normalized RMSE < 0.05 (measured 4.7×10⁻⁴) |
| `no_nan_through_active_chain_with_noise` | finite output through the active chain |
| `extreme_gain_settings_stay_finite` | ±30 dB master gain stays finite |
| `stft_engine::*` | identity reconstruction, silence, sine correlation, block-size invariance (non-hop-multiple blocks conserve signal) |
| `presets::*` | table integrity, id resolution per module, table→sink write roundtrip, Init full-reset contract (defaults + bypass, red lines untouched) |
| `registry` / `full_chain` structural tests | the module table, the default chain, GUI entries and tests stay in lockstep |
| module unit tests | per-module behavior (envelopes, filters, voices, metering) |

Parameter values are deliberately not writable outside a host (nih-plug
keeps `ParamMut` crate-private), so chain-level tests run at default
parameter settings and the preset roundtrip is asserted at the table→sink
boundary; the GUI's `ParamSetter` path is the host-notifying write.

---

# AkiFX 技术文档（中文）

*AkiFX 的构成方式：信号链、DSP 内部、状态模型、UI 架构与测试清单。*

## 1 · 拓扑

```
 MIDI ──┐
        ▼
[Sine Generator]      测试音源 / MIDI 声部            （无延迟）
   ▼
[Soft Vacuum]         过采样真空管饱和                 （无）
   ▼
[Crisp]               环调 + 噪声激励器                （无）
   ▼
[Spectral Gate]       逐 bin 门限       ┐
   ▼                                     ├ STFT 2048 / 4× 重叠 / Hann
[Frequency Shift]     逐 bin 搬移       ┘   各上报 2560 采样
   ▼
[Spectral Compressor] realfft OLA，16384 频带        （上报 2560）
   ▼
[Crossover]           Linkwitz-Riley IIR LR24          （无）
   ▼
[Diopser]             级联共振器                       （无）
   ▼
[Buffr Glitch]        MIDI 缓冲卡顿                    （无）
   ▼
[Gain]                主增益                           （无）
   ▼
[Safety Limiter]      砖墙限制器                       （无）
   ▼
   输出（峰值表 → 全局旁路门 → 宿主）
```

顺序即数据：`src/modules/registry.rs` 中的 `define_modules!` 是唯一事实表，
每模块一行，生成伞参数树（`AkiFxParams`）、默认链、GUI 条目与完整性测试。
处理顺序存于 `SharedOrder` 置换（持久化为 `module_order`），音频线程每块加锁
读取并无分配缓存。全局旁路是 `BoolParam`：接通时 `AkiFx::process()` 在触碰
缓冲前直接返回 —— 构造上即位一致直通，且可被宿主自动化。

## 2 · 频谱引擎

Spectral Gate、Frequency Shift 与 Spectral Compressor 的分析级共享同一 STFT
设计（SpectralSuite 血统）：

- 2048 点 FFT、4× 重叠（hop 512）、Hann 分析/合成双窗。
- 引擎逐采样推进（而非逐 hop 块），立体声块不丢样本、帧不错位；每声道
  独立 FIFO 状态。
- 单边极坐标频谱，2/N 归一化；重叠相加按 Σw² 缩放（Hann 4× 实测 ≈1.5，
  由恒等零测试验证）。
- **延迟**：引擎上报 `fft + hop`（2560 采样）—— 保守值，与上游 C++ 插件
  一致；真实信号延迟为 `fft`（2048）。宿主按每模块 2560 补偿永远安全。
- 中性回调下，真实延迟处的重构 RMSE ≈ 4.7×10⁻⁴（窗纹波底线，≈1/fft_size，
  不可闻）。对称 Hann 的 COLA 微纹波是固有特性，有意保留。

Spectral Compressor 本体运行上游移植的 realfft 重叠相加管线：对 16384 个
频带（8192 bin × 2 声道）做逐 bin 上/下行压缩，门限遵循可配置的 log-log
曲线；逐块标记参数变更，干湿交叉淡化。

## 3 · 模块要点

- **Soft Vacuum**：平滑块每块渲染一次、声道间共享；过采样档位是 `IntParam`，
  回调经 `Arc<AtomicF32>` 发布给各声道处理器。块缓冲按 512 预分配、按需
  增长 —— 音频线程零分配。
- **Crisp**：滤波系数逐样本平滑，TDF-II 双二阶（RBJ 系数 + 输入清理钳位；
  共享 `dsp::Biquad`）。
- **Crossover**：每声道一个 IIR 分频器，频带增益逐样本平滑。
- **Diopser**：参数移动期间逐样本在原始/平滑滤波更新间切换。
- **Buffr Glitch**：每声部独立增益平滑器；`note_on` 对未分配缓冲设防。
- **Safety Limiter**：任何非有限样本立即视为过峰。
- 平滑速率沿用各效果的上游移植来源；这些数值是对上游行为的校准结果，
  记录在移植说明中，不做重新调音。

## 4 · 参数、预设与状态

宿主保存的一切都在 `AkiFxParams`（加上链的 `module_order` /
`module_enabled` 与 `ui_zoom`）：参数、模块电源状态与机架顺序随宿主状态块
完整往返。参数 ID 稳定，预设按 ID 寻址。

工厂预设（`src/presets.rs`）是归一化（0..1）数值补丁的编译期表。两条结构
性保证：

1. **红线模块不可寻址。** `is_red_line_module` 按注册表前缀匹配 Gain 与
   Safety Limiter；`apply_to_sink` 每次写入前重新校验，任何预设若引用它们
   测试即失败。因此预设动不了你的输出增益、限制器与全局旁路。
2. **预设只点亮它补丁里的模块。** 配方之外的模块保持用户的电源状态。
   `Init` 是唯一例外：把所有效果模块重置为默认并旁路。

运行时 GUI 经 `PatchSink` 应用预设：驱动 nih-plug 的 `ParamSetter`
（begin / set-normalized / end 手势组，宿主自动化看到的是连贯动作），随后
把模块电源镜像到持久化的使能参数。

## 5 · GUI 实现

编辑器为 egui（经 `nih_plug_egui`），采用 Aki「Aurora Glass」设计语言：

- **一个配色包，零裸色值。** `gui/theme.rs` 定义 `ColorPack`（表面、三级
  文字、强调色、状态色、三色渐变坡道、极光光强、液态玻璃材质令牌、阴影
  色调）。当前启用 *Mint Fresh*（家族浅色主题，§3.4）；新增主题 = 增加一个 struct —— 组件代码
  只读 `pal()`。派生色一律经 `mix()` / `with_alpha()`（`color-mix()` 等价物）。
- **三字体家族**，全部内嵌 OFL TTF，注册前以 sfnt magic 校验（坏字体会
  在宿主进程内 panic）：Plus Jakarta Sans（UI 文字）、Fraunces 72pt
  （展示/品牌）、IBM Plex Mono（读数）；Noto Sans SC 作 CJK 回退。缩放
  （60–200%）在字号层实现，因为 baseview 渲染器忽略 `pixels_per_point`。
- **材质。** 内容卡为液态玻璃：极光幕布上的半透明填充、顶部 1 px 镜面高
  光、底部 1 px 内阴影、外描边 + 内缩 5 px 的双镶线、柔和有色阴影。极光
  幕布是沿坡道的四团椭圆渐变网格 + 确定性噪点纹理（缓存 `TextureHandle`）。
- **流光与光晕按预算**：shimmer ×1（全局旁路接通时）、neon ×2（选中机架
  药丸、频谱面板）、星芒 ×1（开窗绽放）、About 格言 ×1。光晕只出现在活跃
  元素上，用渐变网格而非 CPU 模糊。
- **性能门控。** 频谱面板只在发布快照携带能量时重绘曲线（静音帧跳过网格
  重建并以 10 Hz 轮询）；曲线按像素列 max-pool 至 ≤420 点；电平表在 GUI 侧
  即攻缓放衰减；空闲时仅旁路 shimmer 的 30 Hz 与电平表衰减在重绘。
- **音频线程数据。** 峰值经 `Arc<AtomicF32>`；频谱快照每三块经互斥锁交换
  的 `Arc` 发布。GUI 线程除旁路原子与 `ParamSetter` 外不直接触碰模块状态。

## 6 · 测试清单

`cargo test --workspace` 共 181 个单元 + 32 个集成测试。承重项：

| 测试 | 断言 |
|---|---|
| `all_bypassed_produces_bit_identical_passthrough` | 旁路链位精确 |
| `stereo_consistency_identical_inputs` | 相同双声道输入经全开链后输出位一致 |
| `latency_aligned_null_spectral_core` | 上报延迟 == fft+hop；真实延迟处恒等重构，增益归一化 RMSE < 0.05（实测 4.7×10⁻⁴） |
| `no_nan_through_active_chain_with_noise` | 激活链输出有限 |
| `extreme_gain_settings_stay_finite` | ±30 dB 主增益有限 |
| `stft_engine::*` | 恒等重构、静音、正弦相关性、块长不变性（非 hop 整除块长同样守恒） |
| `presets::*` | 表完整性、逐模块 ID 解析、表→sink 写入往返、Init 全重置契约（默认值 + 旁路，红线不触碰） |
| `registry` / `full_chain` 结构测试 | 模块表、默认链、GUI 条目与测试保持同步 |
| 各模块单元测试 | 各模块行为（包络、滤波、声部、计量） |

参数值在宿主之外刻意不可写（nih-plug 将 `ParamMut` 保持为 crate 私有），
因此链级测试运行于默认参数，预设往返在表→sink 边界断言；GUI 的
`ParamSetter` 路径是通知宿主的写入口。

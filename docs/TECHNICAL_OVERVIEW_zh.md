# AkiFX 技术文档（底层实现）

**版本 0.3.0** · 面向开发者与贡献者

本文档描述 AkiFX 的完整底层实现：框架选型、声明式模块注册表、STFT 引擎、各效果器的 DSP 算法、宿主集成、实时安全模型与测试策略。DSP 已对照上游源码（SpectralSuite @ `5dfd294`、nih-plug @ `f36931f`）逐行校准。

---

## 目录

1. [技术栈与工程结构](#1-技术栈与工程结构)
2. [声明式模块注册表](#2-声明式模块注册表)
3. [模块链](#3-模块链)
4. [STFT 频谱引擎](#4-stft-频谱引擎)
5. [各效果器的 DSP 实现](#5-各效果器的-dsp-实现)
6. [参数系统与持久化](#6-参数系统与持久化)
7. [宿主集成](#7-宿主集成)
8. [实时安全模型](#8-实时安全模型)
9. [GUI 实现](#9-gui-实现)
10. [测试与验证策略](#10-测试与验证策略)

---

## 1. 技术栈与工程结构

| 组件 | 选型 | 说明 |
|---|---|---|
| 语言 | Rust 2021（stable, MSVC） | 音频线程无 GC、无运行时 |
| 插件框架 | [nih-plug](https://github.com/robbert-vdh/nih-plug) @ `f36931f`（**rev 锁定**） | VST3 + CLAP 导出、standalone、参数派生宏 |
| GUI | nih-plug-egui（egui 0.31 + baseview） | 宿主父窗口内渲染 |
| FFT | rustfft（复数 FFT，频谱引擎）+ realfft（实数 FFT，Spectral Compressor） | 均为懒规划 |
| 其他 | parking_lot、rand/StdRng、atomic_float（过采样感知平滑） | |

```
Cargo.toml (workspace: akifx + xtask)
akifx/src/
  lib.rs        # define_modules! 表、Plugin 实现、process()、峰值表
  main.rs       # standalone 入口（period-size 防护）
  modules/
    mod.rs             # AkiFxModule trait
    registry.rs        # define_modules! 宏定义
    chain.rs           # ModuleChain + SharedOrder
    dsp.rs             # 共享 Biquad
    spectral_common.rs # SpectralFxCore 引擎包装
    spectral_compressor/  # mod + bank + curve + mixer + analyzer
    ...                # 其余模块每模块一文件
  stft/          # SpectralSuite STFT 引擎（engine/window/polar）
  gui/           # egui 编辑器（mod/state/theme/descriptions）
akifx/assets/fonts/  # Inter ×2、Noto Sans SC、Cormorant Garamond
xtask/               # nih_plug_xtask 打包器
```

`lib.rs` 导出 `nih_export_vst3!` / `nih_export_clap!`；`main.rs` 导出 standalone——nih-plug 标准布局（`cdylib` + `lib`）。

---

## 2. 声明式模块注册表

v0.3.0 重构的核心。`define_modules!`（定义于 `modules/registry.rs`，在 `lib.rs` 中唯一调用）接受一张表：

```rust
crate::define_modules! {
    sine_gen,  SineGenModule,  SineGenParams,  "sine_gen",  "Sine Generator";
    ...
}
```

生成过去需要 4 处手工同步的全部内容：

1. 伞参数树 `AkiFxParams`（`#[nested(id_prefix)]` 字段）；
2. `Default for AkiFxParams`（每个参数类型实现 `Default`）；
3. `create_default_chain()`——按表序构建链，全部模块初始旁路；
4. `MODULE_NAMES` / `MODULE_PREFIXES` / `MODULE_COUNT`；
5. `params_for_module(params, idx)`——经 getter 表的按下标类型化访问。

GUI 条目与测试消费这些生成表。**新增模块 = 表里加一行 + 自己的文件**。注册表完整性测试把静态表钉在活体链上（`registry_tables_match_chain`），并覆盖参数访问（`params_for_module_covers_all`）。

伞结构还携带**全局旁路** `BoolParam`（`"global_bypass"`，可自动化、宿主持久化），以及三个 `#[persist]` 字段（顺序、缩放、模块开关状态）。

---

## 3. 模块链

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

- **原位处理**；需要干信号的模块自持快照。
- **旁路 = 跳过**：链从不调用旁路模块——零开销位一致直通。`process()` 里的**全局旁路**以同样方式跳过整条链。
- **运行时重排**经 `SharedOrder`（`Arc<Mutex<Vec<usize>>>`）：音频线程锁内与本地缓存逐元素比较（无分配），变化时才刷新。
- **延迟聚合**：非旁路模块延迟求和。
- **MIDI 广播**：全部事件到达全部已启用模块；各模块 `take_output_midi()` 由插件层排空并发送宿主。

---

## 4. STFT 频谱引擎

`stft/engine.rs` 移植 SpectralSuite 的 `StandardFFTProcessor` + `SpectralAudioProcessorInteractor`，经 `SpectralFxCore`（§5）供 Spectral Gate 与 Frequency Shift 共享。

- **数据通路**：窗（分析）→ 无缩放 FFT → 前半 ×2/N → 极坐标 → 模块回调 → 直角坐标（镜像清零）→ 无缩放 IFFT → 窗（合成）；输出按重叠实例累加。C++ 的 `m_ifftin[0] = 0.f`（清 DC）有意省略，使模块可访问 DC。
- **实例**：每声道 `overlap_count` 个，偏移交错 `hop × (i % overlaps)`；缓冲恰好填满瞬间触发 FFT。
- **逐样本推进**：C++ 按 hop 分块推进 offset，宿主给非 hop 倍数块（如 480 @ hop 512）时会**周期性丢样**。AkiFX 逐样本推进——对齐块时序完全一致，非对齐块不丢样。
- **回调签名**：`FnMut(num_bins, chan, overlap, polar)`——携带实例身份，供有状态模块维护每实例状态。
- **固定配置**：2048 点 FFT、4 倍重叠、Hann 窗；`latency_samples() = fft_size + hop_size`（与 C++ 插件的保守上报一致）。
- **输入验证**：坏字体资产曾让宿主在 epaint 内 panic——引擎构造时断言配置合法性，音频路径所有错误一律降级为跳帧，绝不 panic。

---

## 5. 各效果器的 DSP 实现

### 5.1 共享层

- **`dsp.rs`**：唯一的 TDF-II `Biquad` + RBJ 系数构造器（低通/高通/全通）+ 输入钳位（Nyquist、最小 Q）。Crisp、Crossover、Diopser 三份复制粘贴归一于此。
- **`spectral_common.rs`**：`SpectralFxCore` 持有 STFT 引擎与干信号缓冲，Spectral Gate 与 Frequency Shift 各自只剩参数与频谱回调。

### 5.2 Sine Generator
f64 相位累加；NoteOn 力度 → 5 ms 线性增益包络，PolyPressure 同理，NoteOff 斜坡归零；包络恒渲染（无硬门，无释放爆音）。

### 5.3 Soft Vacuum
多级二极管桥式整形（`bridge_rectifier = sin(min(|out|+skew, π/2))`，drive>1 时级数 = drive²）。slew 信号基带计算、经独立 Lanczos3 级联上采样，保证过采样音色对齐。过采样器 scratch 按**宿主 max_buffer_size** 分配，宿主违约时惰性扩容（宁分配不 panic）。5 个参数全部 `SmoothingStyle::OversamplingAware`（绑定过采样 IntParam）；平滑值**每块渲染一次**、双声道共享。

### 5.4 Crisp
PCG32 白噪声（seed 69/420，与上游逐位一致）→ RBJ 高通 → RBJ 低通，与低通后的输入环调。`amount`/`output_gain` 逐样本步进；三组滤波器在**成员 smoother 平滑中**逐样本重建系数（`maybe_update_filters`）。

### 5.5 Spectral Gate
逐 bin 门限：`cutoff¹⁰` 派生阈值 + tilt `±(frac−0.5)·2·tilt`；DC 直通；内部参数 ε 变化检测按块重算。

### 5.6 Frequency Shift
先 `floor(i·scale)` 重映射，再 `i+binShift` 平移覆盖（上游顺序）。binShift **双重截断**（Hz→int、×binWidth→int）忠实 C++。Rust 补上了 C++ 的 `size_t` 下溢隐患。

### 5.7 Spectral Compressor
realfft OLA 管线，逐行对应 nih-plug 的 `StftHelper`：逐样本环形写入/读清，窗口边界取最旧帧。对称 Hann。逐 bin 包络（有效采样率 `sr/(window/overlap)`，reset 后 150 ms 时间系数回卷）。软拐点抛物线（Giannoulis 系数），`gain_diff = down + up − 2·env`；向上压缩门控 `bin ≥ first_non_dc_bin && env_db > −100 dB`（gain→dB 在 1e-5 夹持）。`first_non_dc_bin` 按 ~20 Hz bin 下标计算。曲线参数经模块侧**值变化检测**（每块 13 字段快照）驱动 bank 重算——参数对象无法触达 DSP 侧 bank，值比较恢复了上游"变化即重算"的时机。干湿混合器对齐 STFT 延迟；容量按宿主违约惰性扩容；Mix 平滑按块 `next_step(block_len)`。GUI 频谱发布见 §9。

### 5.8 Crossover
LR4（每级 2×级联 Butterworth LP/HP）+ 全通级联相位补偿（低频段）。每声道一个 `IirCrossover`（状态独立）。分频频率平滑中逐样本滑动；频段增益（AkiFX 增强）逐样本平滑。

### 5.9 Diopser
RBJ 全通 biquad 级联（最多 512 级），按八度/线性规律展开并钳位 `[5 Hz, sr/2.05]`。频率/谐振/展开逐样本平滑，平滑中重建系数。

### 5.10 Buffr Glitch
环形缓冲按 MIDI 音符 0 × 最大八度移位的周期取 2 的幂；Recording → 等功率交叉淡化 → Ready 状态机。`reset()` 清状态**不释放内存**（历史上的缺陷：宿主 reset 后首个 NoteOn 越界 panic）。8 复音三级窃取；AR 包络系数每块计算一次。PolyVolume 逐音符增益经 5 ms 线性 smoother。

### 5.11 Gain / Safety Limiter
Gain：纯净增益级 + 50 ms 对数平滑（`initialize` 中播种，非宿主上下文不从零滑动）。Safety Limiter：19 边沿 SOS 摩尔斯表、420 Hz 正弦 `threshold × 0.125`、相位回卷防 click、等功率淡入淡出；非有限样本静音**并触发警报**。

---

## 6. 参数系统与持久化

`AkiFxParams`（registry 生成）携带模块树 plus：

- `global_bypass: BoolParam`——链级旁路，可自动化，宿主持久化；
- `module_order: Mutex<Vec<usize>>`——处理顺序；
- `ui_zoom: Mutex<f32>`——界面缩放；
- `module_enabled: Mutex<Vec<bool>>`——模块开关，`initialize()` 时应用到旁路原子（全新加载全关）。

回调 Arc 无法触达 DSP 状态的场景（Spectral Compressor 曲线），模块以**值比较**快照替代——重算时机与上游等价（误差 ≤ 一个处理块）。

---

## 7. 宿主集成

`process()` 顺序：

1. **全局旁路门**——开启：发布峰值表后直接返回（位一致直通）。
2. **延迟重报**——链延迟与 `reported_latency` 比较，变化即 `context.set_latency_samples()`（同一块内生效）。
3. **MIDI 收集**——全部事件进复用缓冲，广播给链。
4. **音频**——立体声 `split_first_mut` 原位；单声道经预分配 scratch。
5. **峰值表**——各声道绝对峰值写入与编辑器共享的 `Arc<AtomicF32>`。
6. **MIDI 回送**——各模块 `take_output_midi()` 发送宿主。

`editor()` 从 registry 组装 UI 条目、接活体旁路原子、取峰值表句柄与模块 `spectrum_view()`，构建 egui 编辑器。`main.rs` 将 standalone 默认 period size 提到 2048（部分 WASAPI 设备交付大于 512 请求的包会触发 nih-plug cpal 后端的断言；显式 `--period-size` 仍优先）。

---

## 8. 实时安全模型

| 约束 | 实现 |
|---|---|
| 无堆分配 | 频谱回调全部预分配 scratch；链顺序锁内比较 + 本地缓存；事件缓冲复用。唯一允许的分配是宿主违约（超声明大块）时惰性扩容——宁分配不 panic |
| 无 panic | `unwrap/expect` 全部 let-else 跳过/直通；FFT 错误跳帧；系数输入钳位；干湿混合器惰性扩容；**字体按 sfnt 魔数验证后才注册**（坏字体资产曾让宿主在 epaint 内崩溃） |
| 无无限等待 | 唯一的锁（order）仅持有比较/求和时间；旁路开关为 relaxed 原子 |
| 状态一致 | 旁路 = 整模块跳过（位一致）；顺序变化逐元素校验；全局旁路跳过一切 |

---

## 9. GUI 实现

- **框架**：`nih_plug_egui::create_egui_editor`（egui 0.31 + baseview）+ 自定义可缩放窗口。全部矢量绘制，无位图资源。
- **字体**：`include_bytes!` 内嵌 Inter Regular/Medium（正文）、Cormorant Garamond SemiBold（标题衬线）、Noto Sans SC（CJK 回退）。每个文件按 sfnt 魔数验证（`is_font_file`），无效则跳过；自定义 `FontFamily::Name(...)` 显式绑定（未绑定在 egui 中 panic）。
- **参数控件**：浮点/整数/枚举渲染为 `ParamSlider`；**布尔渲染为单个切换按钮**（按下=开），经 nih-plug setter（自动化路径）驱动。
- **全局旁路按钮 + 峰值表**位于底部条：旁路是参数按钮；峰值表从共享原子绘制 L/R 峰值，带 dBFS 读数与过刻度高亮。
- **频谱视图**（Spectral Compressor）：音频线程以 ~30 Hz 将 `SpectrumSnapshot`（包络幅度 + 向下门限曲线，dB）发布进 `SpectrumView`；编辑器在对数频率轴上绘制频谱（玉绿）与门限曲线（琥珀），带倍频程网格线。
- **持久化**：电源开关每次点击镜像进 `module_enabled`，宿主保存工程即捕获。

---

## 10. 测试与验证策略

**约 200 项测试**，四层：

1. **引擎**（`tests/stft_engine.rs`）：恒等重构、静音本底、正弦相关性、闭式窗值、变块、**非 hop 倍数块不丢样**。
2. **链**（`tests/chain_test.rs`、`tests/full_chain.rs`）：插入顺序、位一致旁路、延迟求合与开关动态、11 模块结构、全局旁路参数契约、全激活 NaN 扫描。
3. **模块**（~165 项内联测试）：中性参数恒等、NaN 扫描、确定性、针对性回归（crossover 系数重建、平滑步进、增益）。
4. **Registry/GUI**：registry 表 vs 活体链、参数访问覆盖、UI 条目数/名称/前缀对生成表、旁路原子共享（ptr_eq）、持久化镜像、11 模块描述完整性。

**保真度方法**：对照上游逐式审计（公式、常数、默认值、控制流）。上游怪癖显式保留并注释；有意偏差（逐样本 STFT 推进、功能裁剪等）在代码注释与用户手册 §7 记录。

---

*AkiFX © Akiro · GPLv3*

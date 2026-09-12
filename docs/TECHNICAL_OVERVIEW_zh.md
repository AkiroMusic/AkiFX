# AkiFX 技术文档（底层实现）

**版本 0.2.1** · 面向开发者与贡献者

本文档描述 AkiFX 的完整底层实现：框架选型、模块系统、STFT 引擎、各效果器的 DSP 算法、宿主集成、实时安全模型与测试策略。所有结论均对照上游源码（SpectralSuite @ `5dfd294`、nih-plug @ `f36931f`）校准。

---

## 目录

1. [技术栈与工程结构](#1-技术栈与工程结构)
2. [模块系统与信号链](#2-模块系统与信号链)
3. [STFT 频谱引擎](#3-stft-频谱引擎)
4. [各效果器的 DSP 实现](#4-各效果器的-dsp-实现)
5. [参数系统与状态持久化](#5-参数系统与状态持久化)
6. [宿主集成（VST3/CLAP/独立版）](#6-宿主集成)
7. [实时安全模型](#7-实时安全模型)
8. [GUI 实现](#8-gui-实现)
9. [测试与验证策略](#9-测试与验证策略)

---

## 1. 技术栈与工程结构

| 组件 | 选型 | 说明 |
|---|---|---|
| 语言 | Rust 2021（stable, MSVC） | 音频线程无 GC、无运行时 |
| 插件框架 | [nih-plug](https://github.com/robbert-vdh/nih-plug) @ `f36931f`（**rev 锁定**） | VST3 + CLAP 导出、standalone（WASAPI/JACK）、参数派生宏 |
| GUI | nih-plug-egui（egui 0.31 + baseview） | 由框架传入宿主父窗口 |
| FFT | rustfft（复数 FFT，SpectralSuite 引擎）+ realfft（实数 FFT，Puberty/Spectral Compressor） | 两者均为懒规划（lazy planner） |
| 其他 | parking_lot（锁）、rand/StdRng（Bin Scrambler）、atomic_float（过采样感知平滑） | |

```
Cargo.toml (workspace: akifx + xtask)
akifx/
  src/
    lib.rs            # nih_plug::Plugin 实现、参数树、链构建、MIDI 路由
    main.rs           # standalone 入口（nih_export_standalone）
    modules/          # 21 个模块 + chain.rs + mod.rs（trait 定义）
      spectral_compressor/   # 多文件模块（bank/curve/mixer/analyzer）
    stft/             # SpectralSuite STFT 引擎（engine/window/polar）
    gui/              # egui 编辑器（mod/state/theme/descriptions）
  assets/fonts/       # Inter ×2、Noto Sans SC（CJK 回退）、Cormorant Garamond
xtask/                # nih_plug_xtask 打包器（cargo xtask bundle）
```

`lib.rs` 导出 `nih_export_vst3!` / `nih_export_clap!`，`main.rs` 导出 standalone——这是 nih-plug 的标准布局：`[lib] crate-type = ["cdylib", "lib"]`，宿主加载 cdylib，独立版由 bin 链接 rlib。

---

## 2. 模块系统与信号链

### 2.1 AkiFxModule trait（`modules/mod.rs`）

```rust
pub trait AkiFxModule: Send {
    fn name(&self) -> &'static str;
    fn params(&self) -> &dyn Params;
    fn bypass_flag(&self) -> &Arc<AtomicBool>;     // 链级旁路开关（GUI 共享）
    fn initialize(&mut self, sample_rate: f32, max_block_size: usize);
    fn reset(&mut self);
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    // 可选覆盖：
    fn latency_samples(&self) -> u64 { 0 }
    fn process_with_midi(&mut self, l, r, events: &[NoteEvent<()>]) { self.process(l, r) }
    fn take_output_midi(&mut self) -> Vec<NoteEvent<()>> { Vec::new() }
}
```

设计要点：

- **原位处理**：每个模块直接改写传入的立体声切片，链上零拷贝。需要干信号的模块（Morph 等）自己维护 `dry_buf` 快照。
- **旁路 = 跳过**：`ModuleChain::process_with_midi` 只调非旁路模块。被跳过的模块不触碰缓冲，因此是**位一致直通**（与 nih-plug 参数式 bypass 的平滑交叉淡化不同，这里选择零开销硬切；模块内部不保留状态泄漏问题）。
- **每模块一份参数**：`Arc<XxxParams>` 同时挂在伞参数树（宿主自动化/持久化）与模块实例（DSP 读取）上，指向同一批 `FloatParam`——宿主写值、音频线程经 smoother/原子读取，无额外同步。

### 2.2 ModuleChain（`modules/chain.rs`）

```rust
pub struct ModuleChain {
    modules: Vec<Box<dyn AkiFxModule>>,
    order: SharedOrder,          // Arc<Mutex<Vec<usize>>>，GUI 可写
    cached_order: Vec<usize>,    // 音频线程本地缓存
}
```

- **运行时重排**：GUI 拖拽提交新排列写入 `order`；音频线程每块在锁内做**逐元素比较**（无分配），仅在顺序真正变化时刷新本地缓存。锁为 parking_lot Mutex，音频线程持有时间为微秒级且无竞争。
- **延迟聚合**：`latency_samples()` = Σ 非旁路模块延迟（锁内求和，不克隆）。顺序变化立即反映。
- **合法性**：`set_order` 校验排列；`initialize` 恢复持久化顺序时经 `is_valid_permutation` 复核，非法则回退恒等。

### 2.3 四份"21 元素表"的同步约束

伞参数树（`lib.rs` 的 `AkiFxParams`）、链构建宏（`push!`）、GUI 条目宏（`entry!`）与测试名称表必须同序。由 `assert_eq!(chain.len(), 21)` 与 GUI 测试守卫。新增模块时四处同步修改。

---

## 3. STFT 频谱引擎

`stft/engine.rs` 是 SpectralSuite `shared/StandardFFTProcessor.cpp` + `SpectralAudioProcessorInteractor.cpp` 的移植，供 8 个频谱模块共享**代码**（每个模块持有独立引擎实例，与"每个原版插件独立运行"一致）。

### 3.1 数据通路（每实例）

```
input ──窗(分析)──► FFT(无缩放) ──前半×2/N──► 极坐标 ──► 模块回调(逐bin修改)
      ◄──窗(合成)── IFFT(无缩放) ◄──镜像清零 ◄── 直角坐标 ◄──┘
输出 = Σ 各重叠实例的输出（+= 累加）
```

对应 C++ `process()` 的 11 个步骤（文档注释中逐步标注了 C++ 行号）：

1. **双窗**：分析与合成各乘一次 Hann（`hann²`）。
2. **归一化**：正变换**不缩放**（rustfft 与 kissfft 行为一致），仅对前 `half_size` 个 bin 乘 `2/N`——这是单边谱（single-sided）幅度重建的标准因子。
3. **镜像清零**：`pol2Car` 后 `[half_size, fft_size)` 全部清零（C++ 用 `invertedIndex` 镜像写入，效果等价——这些 bin 本就被 `2/N` 只处理前半的策略丢弃）。*有意的偏差*：C++ 第 38 行 `m_ifftin[0] = 0.f` 会清空 DC bin，AkiFX 保留 DC 以便模块访问。
4. **极坐标**：`Polar { magnitude, phase }` 的往返转换与 C++ `utilities::car2Pol/pol2Car` 逐公式对应。

### 3.2 重叠实例与推进模型

- 每声道 `overlap_count` 个独立实例，实例 `i` 的写偏移 `offset = hop × (i % overlaps)`（对应 `SpectralAudioProcessorInteractor::setFftSize`），FFT 在 `offset >= fft_size` 的瞬间触发并复位——与 C++ `fill_in_passOut` 的守卫语义一致。
- **与 C++ 的偏差（有意，已修正行为差异）**：C++ 按 `hop_size` 分块推进 `offset`；宿主给非 hop 倍数块（如 480 @ hop 512）时 offset 会越过 `fft_size`，`if off < fft_size` 守卫导致**周期性丢样**。AkiFX 改为**逐样本推进**：offset 每样本 +1，恰好填满即触发 FFT。对齐块下与 C++ 时序完全等价，非对齐块下不再丢样。
- **回调签名**：`FnMut(num_bins, chan_idx, overlap_idx, &mut [Polar])`。C++ 为每个（声道 × 重叠）实例 `createSpectralProcess` 一个独立处理器对象；AkiFX 的回调携带实例身份，供有状态模块（Phase Lock）维护**每实例**状态。
- **预分配**：每实例的 `fft_buf/ifft_buf/polar_buf/scratch_fwd/scratch_inv` 在构造时分配；FFT 经 `process_with_scratch` 使用私有 scratch，音频线程零分配。
- **Send/Sync**：引擎只含 `Vec` 与 `Arc<dyn Fft>`（rustfft 的 `Fft` trait 自带 `Send + Sync` 上界），自动获得线程安全，无需手写 `unsafe impl`。

### 3.3 窗函数（`stft/window.rs`）

Hann/Hamming/Blackman 等 8 种窗的闭式生成，其中 Hamming 保留了 C++ `size + 1` 的分母怪癖（已注释）。`latency_samples() = fft_size + hop_size`，与 C++ `SpectralAudioPlugin` 的上报值一致（真实信号延迟为 `fft_size`，上报值含一 hop 的保守余量——PDC 过补偿不影响对齐）。

---

## 4. 各效果器的 DSP 实现

### 4.1 SpectralSuite 系列（共享引擎，回调即全部算法）

每个模块实现 `fn spectral_callback(num_bins, polar)`，引擎在每帧每实例上调用：

| 模块 | 回调算法（对应 C++ 文件） |
|---|---|
| **Spectral Gate** | 逐 bin 门限：能量 < 门限 → 置零；DC 直通；Tilt 让门限随频率倾斜 `±(frac−0.5)·2·tilt`。内部参数（`cutoff¹⁰`、`balance³` 等）以 ε 变化检测按块重算 |
| **Frequency Shift** | 先按 `floor(i·scale)` 压缩/拉伸重映射，再按 `i+binShift` 平移覆盖（顺序与 C++ 一致，重叠时后者胜）。binShift **双重截断**（Hz→int，Hz×binWidth→int）忠实上游。Rust 补上了 C++ 的潜在越界（`binShift > halfFft` 时 C++ 的 `size_t` 下溢） |
| **Frequency Magnet** | 目标 bin 以下的能量沿 `line^width` 曲线向上收拢、以上沿 `(1−width)·7+1` 次幂向下收拢，累加进 `temp`；映射位置保留**小数部分**做亚 bin 插值（下段向 `temp[idx+1]`、上段向 `temp[idx−1]` 按 `frac` 混合——`utilities::interp_lin` 语义）。Rust 对 C++ 的 `temp[idx+1]` 越界读做了钳位 |
| **Bin Scrambler** | 双索引缓冲 A/B（旧/新排列），回调输出 `out[i] = interp_lin(in[A[i]], in[B[i]], phase/maxPhase)`——幅度与相位**分别**线性插值。新排列生成：identity → 播撒（`in[(i+size)/5] = in[rand%size/5]`，目标区 `[size/5, 6size/25)`）→ 分块洗牌（严格 `n < size−chunk` 边界）。phasor 按实际处理样本数累计（与 C++ `mPhasor += blockSize` 等价），归零时切换排列。RNG：非零 seed 时用 `StdRng::seed_from_u64` 保证可复现（上游仅播撒路径可用 `srand`） |
| **Morph** | 16 控制点曲线 → 每帧重算 `morph_points[i]`（0–1 归一化映射到 bin），回调做 `out[map[i]] = in[i]` 的置换（目标 ≥ bins 丢弃——C++ 写入被忽略的镜像区）。曲线为线性插值简化版（上游为样条），已在代码注释声明 |
| **Phase Lock** | `LockState`（带 1 帧过渡的开关状态机）与 `TransitionState`（按时长从实时值向锁定值 morph）逐公式移植。**每（声道×重叠）实例独立持有** `locked_phases/locked_mags/target_*` 向量（对应 C++ 每实例 `createSpectralProcess`）。幅轨 `scale = t + (1−t)·(max_live/max_locked)`；PRNG 为 xorshift32 |
| **Sinusoidal Shaped Filter** | 1024 点正弦查找表（含 guard point），`index = i·(freq+1) + phase³·halfSize`，幅度 ×= `value^(width²·8+1)`，相位不动。**有意偏差**：C++ 查表实际是零阶保持（其"插值"把已截断的整数再当索引用），Rust 做了真线性插值——代码注释注明这是修复 |
| **Playground** | 恒等回调（重构 RMSE < 数值容差）。上游"Linear Hann"开关从未被读取，AkiFX 同样不读（恒 Hann 窗） |

### 4.2 Soft Vacuum（Hard Vacuum 移植）

- **核心**：`bridge_rectifier = sin(min(|output|+skew, π/2))` 两级 sin 整形 + 半周不对称混合；`drive > 1` 时级数 = `drive²`。`ALMOST_FRAC_PI_2 = 1.5570797`（上游注释：原插件笔误的 π/2）。
- **slew 分离**：压摆率 `slew[n] = x[n] − x[n−1]` 在基带计算，经独立的 Lanczos3 过采样器上采样后送入失真核——保证 16x 过采样与非过采样音色一致。
- **过采样器**：11 抽头 Lanczos3 半带核（系数与上游逐位相同）多级级联；核延迟 `(2·KERNEL_LATENCY).rem_euclid(2ⁿ)` 补偿为整数采样。scratch 按**宿主 max_buffer_size** 分配（而非硬编码），宿主违约给大块时运行时扩容（仅重置滤波状态，不 panic）。
- **平滑**：5 个参数使用 `SmoothingStyle::OversamplingAware(Arc<AtomicF32>, inner)`——步数按 `sample_rate × 过采样倍数` 计算，保证平滑的**墙钟时长**恒定；`Arc` 由 Oversampling IntParam 的回调更新。平滑值**每块渲染一次**、双声道共享（上游语义；每声道渲染会使平滑速率翻倍且左右轨迹分歧）。

### 4.3 Crisp

- **噪声源**：PCG32 整数流（seed 69/420，与上游 bit-identical）→ RBJ 高通 → RBJ 低通。
- **逐样本平滑**：`amount`、`output_gain` 每样本 `smoothed.next()`；三个双参数滤波器组（RM 前低通、噪声高通/低通）在**任一成员 smoother 处于平滑中**时逐样本步进并重建系数（`maybe_update_filters`，镜像上游）。静止时不步进（`is_smoothing() == false`）。
- **模式**：`do_ring_mod` 按 Mode 分支；Crispy/CrispyNegated 在上游即同为 `max(0.0)`（上游怪癖，保留并注释）。

### 4.4 Puberty Simulator 与 Spectral Compressor（realfft OLA 管线）

两者绕过共享引擎，各自实现基于 `realfft` 的重叠相加（OLA），结构对应 nih-plug 的 `StftHelper::process_overlap_add`：

- **环形缓冲推进**：`samples_until_next_window = ((hop − write_pos − 1).rem_euclid(hop) + 1)`，逐样本"写输入环 → 读并清零输出环"，到窗口边界时取**最旧的 window_size 样本**做帧处理。与上游 StftHelper 的读写次序逐行对应。
- **窗**：对称 Hann（`τ/(N−1)`，`util::window::hann_window` 同款）。
- **Puberty**：帧处理 = 窗 → R2C → 频段整体搬移（正向 `multiplier ≥ 1` 顺序遍历、反向时逆序遍历，`floor/ceil` 两邻 bin 按 `frac` 插值，幅度极坐标模式可选）→ `×3×gain_compensation`（上游注释的"随机额外增益"）→ C2R → 窗 → OLA。音分参数**每帧步进一次** `smoothed.next_step(hop)`。增益补偿 `((overlap/4)·1.5)⁻¹/window`。FFT 错误路径：跳过该帧而非 panic。
- **Spectral Compressor**：帧处理 = 窗×`√gc` → R2C → **CompressorBank** → C2R → 窗×`out_gain`。要点：
  - **曲线**：`threshold_db(f) = intercept + slope·Δ + curve·Δ²`，`Δ = ln f − ln(center)`；Pink Noise 模式 slope 内置 −3 dB/oct。
  - **包络**：逐 bin 幅度包络（attack/release 的一阶 IIR），有效采样率 = `sr/(window/overlap)`，reset 后 150 ms 内时间系数线性回卷（与上游一致）。
  - **压缩**：软拐点抛物线（Giannoulis 系数），`gain_diff = down + up − 2·env`；向上压缩的门控条件 `bin ≥ first_non_dc_bin && ratio ≠ 1 && env_db > −100 dB`（`gain_to_db` 在 1e-5 处夹持——与 `util::MINUS_INFINITY_GAIN` 一致）。
  - **曲线更新**：上游由参数回调直接置位 bank 的更新原子；AkiFX 的参数对象在模块构建前创建、无法触达 DSP 侧 bank，故模块**按值比较**缓存快照（`CachedCurveParams`，13 个曲线参数），变化时置位对应曲线组的更新标志——重算时机与上游等价（误差 ≤ 一个处理块），且顺带消除了上游不需要的一次性 bank 分配。
  - **干湿**：`DryWetMixer` 环形延迟线对齐 STFT 延迟；容量 `(max_block + window).next_power_of_two()`，宿主违约给大块时惰性扩容而非 assert。Mix 平滑按块 `next_step(block_len)`。

### 4.5 Crossover

- **LR4 拓扑**：每级分频 = 2×级联低通 + 2×级联高通（RBJ 公式，`NEUTRAL_Q = 1/√2`）；低通频段经**全通级联**（`ap_filters[target][crossover − target − 1]` 矩阵）做相位补偿。系数更新对两个声道实例同步写入。
- **声道状态**：`iir_crossovers: [IirCrossover; 2]`——独立状态（上游用 `Biquad<f32x2>` 双通道单系数组实现，等价）。
- **平滑**：分频频率的 4 个 smoother 在平滑中时**逐样本** `next_step(1)` 并重建系数；频段增益（AkiFX 增强）逐样本平滑。

### 4.6 Diopser

级联 RBJ 全通 biquad（TDF-II 实现，与上游 `filter.rs` 逐式一致）。Spread 按 `freq·2^(oct·proportion)`（八度）或 `freq + Δ·proportion`（线性）展开各级，钳位 `[5 Hz, sr/2.05]`。频率/谐振/展开三参数逐样本平滑 + 平滑中重建系数。每声道独立滤波器状态（上游 `f32x2` 的等价拆分）。

### 4.7 Loudness War Winner

忠实移植：`output[n] = sign(x) · output_gain`（符号硬削波）；WIN HARDER 因子 f>0 时启用 5.5 kHz × 4 级带通（Q = `1e-5 + f·30`，仅在因子平滑中重算系数）；`output_gain` 逐样本平滑。静音检测（所有声道同时为 0）累计 1 s 后线性淡出、2 s 全静音，`reset()` 以"已静音"状态起步避免插入即输出 DC 方波。

### 4.8 Buffr Glitch

- **环形缓冲**：容量按 MIDI 音符 0 × 最大八度移位的周期上取 2 的幂；`prepare_playback` 把活动长度设为一个周期。状态机 Recording →（等功率交叉淡化 `√t` 混合）→ Ready。`reset()` 只清状态**不释放内存**（释放会导致宿主 reset 后首个 NoteOn 越界——修复点），`note_on` 对未分配缓冲做防御性忽略。
- **声部**：8 复音，三级窃取策略；AR 包络一阶 IIR（`exp(−1/(t·sr))`，系数每块计算一次而非每样本每声部）。
- **PolyVolume**：每声部 5 ms 线性 smoother 的音符表达增益，乘入 `velocity_gain × expr × env`。

### 4.9 Safety Limiter

MOS 6502 风格的 SOS 摩尔斯表（19 个边沿时刻/门位，含 4 s 环回的别名边沿）；420 Hz 正弦、幅度 `threshold × 0.125`；相位回卷处 `wrap.looked_at = false` 防爆音；等功率淡入淡出（`√` 曲线）。**非有限样本（NaN/Inf）→ 置零并触发警报**（上游行为）。增益平滑恢复、超阈即触发。

### 4.10 音源与 MIDI 模块

- **Sine Generator**：f64 相位累加；NoteOn 力度 → 5 ms 线性增益包络目标，PolyPressure 同理，NoteOff 归零——包络恒渲染（无硬门），消除释放爆音。
- **MIDI Inverter**：13 种 NoteEvent 变体的镜像变换（`15−ch`、`127−note`、`1−v`），未识别变体静默丢弃（同上游）；Enable 关闭时零输出。变换事件经 `take_output_midi()` 由插件层回送宿主。
- **Poly Mod Synth**：16 声部；事件按 `timing` 切分子块（MAX 64 样本）；包络/增益每子块 `next_block`；CLAP 复音调制缺失时自动退化为标准行为（同上游 VST3）。PRNG 为 XorShift32（上游 Pcg32 的替代，初相不同但特性一致，已注释）。

---

## 5. 参数系统与状态持久化

- **参数树**：`#[derive(Params)]` 的伞结构 `AkiFxParams` 以 `#[nested(id_prefix = "...")]` 挂载 21 组模块参数 → 宿主看到 `AkiFX/:模块前缀/:参数 ID` 的命名空间。参数 `Arc` 双挂载（树 + 模块）是 nih-plug 的标准共享模式。
- **持久化**：三个 `#[persist]` 字段，由 nih-plug 在宿主恢复状态后（重新调用 `initialize()` 前）反序列化：
  - `module_order: Mutex<Vec<usize>>` — 处理顺序；
  - `ui_zoom: Mutex<f32>` — 界面缩放；
  - `module_enabled: Mutex<Vec<bool>>` — 各模块开关。`initialize()` 将其应用到各模块的旁路原子（`!enabled → bypassed`），首次加载缺省全 false（全旁路）。
- **值变化检测**：无法持有回调 Arc 的场景（Spectral Compressor 曲线）用模块侧快照比较替代，每块一次 13 项 f32 比较。

---

## 6. 宿主集成

```rust
impl Plugin for AkiFx {
    const AUDIO_IO_LAYOUTS: stereo + mono;
    const MIDI_INPUT/OUTPUT: MidiConfig::MidiCCs;   // 全事件
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
}
```

- **process()**：
  1. **延迟重报**：`chain.latency_samples()` 与 `reported_latency` 比较，变化即 `context.set_latency_samples()`——模块开关、窗口/过采样参数变化在同一块内生效（DAW PDC 所需）。
  2. **MIDI 收集**：`next_event()` 全量收进复用缓冲（无分配），**广播**给链。
  3. **音频**：立体声走 `split_first_mut` 原位处理；单声道经预分配 scratch 复制为双声道处理再拷回。
  4. **MIDI 回送**：逐模块 `take_output_midi()` → `context.send_event()`。
- **editor()**：`build_ui_entries(params, chain.modules())` 从活体模块读延迟 → `wire_bypass_flags` 把模块的旁路 `Arc<AtomicBool>` 共享给 GUI（关键：GUI 的电源开关与音频线程读写**同一个原子**，有回归测试守卫）→ `create_egui_editor`。
- **打包**：`bundler.toml` 指定产品名 `AkiFX`；`cargo xtask bundle` 产出 `.vst3/Contents/x86_64-win/AkiFX.vst3`、`.clap`、`AkiFX.exe`。

---

## 7. 实时安全模型

音频线程的硬性约束与实现：

| 约束 | 实现 |
|---|---|
| 无堆分配 | 频谱回调全部使用模块预分配 scratch（`scratch_polar`、`temp`、快照向量、`slew_buf` 等）；链顺序用"锁内比较 + 本地缓存"；事件缓冲复用；唯一允许的分配是**宿主违约**（块大于 initialize 声明值）时的惰性扩容（soft_vacuum/sc/mixer，宁可分配不可 panic） |
| 无 panic | 所有 `unwrap/expect` 已替换为 let-else 跳过/直通回退；FFT Result 错误 → 跳帧；`assert!` 系数生成改为钳位（RBJ 频率 > Nyquist 等）；dry_wet mixer 容量惰性增长 |
| 无无限等待 | 唯一的锁（order mutex）持有时间为比较/求和，无嵌套锁；旁路开关为原子 relaxed load |
| 状态一致 | 旁路 = 整模块跳过（位一致）；顺序变化按块生效且逐元素校验 |

---

## 8. GUI 实现

- **框架**：`nih_plug_egui::create_egui_editor`（egui 0.31 + baseview），`EguiState::from_size(1100×720)` + 自定义 `ResizableWindow`（最小 900×600）。全部绘制为矢量（LED、旋钮、电源 pill 均为 painter 图元），无位图资源。
- **字体**：`include_bytes!` 内嵌 Inter Regular/Medium（正文）、Cormorant Garamond SemiBold（标题衬线）、Noto Sans SC（CJK 回退，`FontFamily::Name(...)` 必须显式绑定——未绑定的自定义 family 在 egui 中 panic）。
- **参数控件**：遍历 `param_map()` 的 `ParamPtr`，用 `ParamSlider::for_param` 渲染（覆盖 Float/Int/Bool/Enum 四种指针）；GUI 不直接改值，经 nih-plug setter 走自动化路径。
- **电源开关**：写共享原子（`Relaxed`）后调用 `sync_persisted_enabled` 把 21 个开关镜像进 `#[persist]` 状态——宿主保存工程时捕获。
- **延迟显示**：从模块 `latency_samples()` 实时读取（无静态表）；底部条汇总非旁路模块。

---

## 9. 测试与验证策略

**277 项测试**（`cargo test -p akifx`）分四层：

1. **引擎层**（`tests/stft_engine.rs`）：恒等重构 RMSE、静音本底 < −120 dBFS、440 Hz 相关性 ≥ 0.99、窗闭式值、128–1024 变块、**非 hop 倍数块（480）不丢样**（稳态 DC 输出远离零——旧分块实现会在丢样段触零）。
2. **链路层**（`tests/chain_test.rs`、`tests/full_chain.rs`）：插入顺序、位一致旁路、延迟求合、**延迟随开关切换变化**（PDC 同步的前提）、21 模块结构、全激活 NaN 扫描。
3. **模块层**（各文件 `#[cfg(test)]`，~244 项）：每模块的恒等性（参数归零=直通）、噪声/正弦扫描无 NaN、确定性（同 seed 位一致）、关键回归（如 bin scrambler 满打散后能量守恒、gain −6 dB 减半、crossover 改频率后系数重建）。
4. **GUI 层**（`gui/` 内联）：条目数/名称/前缀对齐、旁路原子共享（ptr_eq）、持久化镜像、描述完整性。

**保真度验证方法**：对照上游逐式审计（公式、常数、默认值、控制流四个维度），上游怪癖（如 CrispyNegated≡Crispy、Hamming 窗 `size+1` 分母、Puberty 的 ×3 增益）显式保留并注释；有意偏差（逐样本推进替代 hop 分块、恒等直通替代 PVOC-off 缩放等）在代码注释与本文件 §4 标注。

---

*AkiFX © Akiro · GPLv3*

# AkiFX 用户手册（中文版）

**版本 0.3.0 · VST3 / CLAP / 独立运行版**

AkiFX 是一款"全家桶"式效果器机架插件：它将两个著名开源项目——Andrew Reeman 的 **SpectralSuite**（C++/JUCE 频谱效果器套件）与 Robbert van der Helm 的 **nih-plug** 及其官方插件集——中的核心效果器整合到**单一条链、单一界面**中，以 VST3 与 CLAP 格式导出。

> **v0.3.0 重要变更**：插件从 21 个模块精简到 **11 个核心效果器**，并新增全局旁路、输出峰值表和 Spectral Compressor 实时频谱视图。**旧工程（v0.2.x）加载 v0.3.0 时，模块开关、顺序与参数将重置为默认**——0.x 阶段的破坏性变更，恕不迁移。

---

## 目录

1. [安装与加载](#1-安装与加载)
2. [界面概览](#2-界面概览)
3. [信号链与模块顺序](#3-信号链与模块顺序)
4. [模块详解](#4-模块详解)
5. [MIDI 支持](#5-midi-支持)
6. [延迟与自动延迟补偿（PDC）](#6-延迟与自动延迟补偿pdc)
7. [与原版插件的差异说明](#7-与原版插件的差异说明)
8. [常见问题](#8-常见问题)

---

## 1. 安装与加载

| 格式 | 位置 | 安装方式 |
|---|---|---|
| **VST3** | `target/bundled/AkiFX.vst3/` | 拷贝到系统 VST3 目录（Windows：`C:\Program Files\Common Files\VST3`；macOS：`/Library/Audio/Plug-Ins/VST3`） |
| **CLAP** | `target/bundled/AkiFX.clap` | 拷贝到 CLAP 目录（Windows：`C:\Program Files\Common Files\CLAP`） |
| 独立运行版 | `target/bundled/AkiFX.exe` | 双击直接运行（WASAPI/JACK 音频），无需宿主即可试用 |

首次加载时，**全部 11 个模块处于旁路（BYPASSED）状态**——插件等价于一根"直通线"。这是有意设计：频谱类模块消耗较多 CPU，用户按需点亮所需模块即可。**模块开关状态随工程保存并自动恢复**。

从源码构建：`cargo xtask bundle akifx --release`（依赖 Rust stable MSVC 工具链）。

---

## 2. 界面概览

AkiFX 采用"机架 + 参数面板"双区布局：

- **左侧机架（Rack）**：11 个模块按信号链顺序垂直排列。每一行包含：
  - **LED 电源开关**：点击切换 ACTIVE / BYPASSED。旁路的模块完全不参与处理（零 CPU 占用），且保证**位一致（bit-identical）直通**。
  - **延迟徽标**：该模块引入的采样延迟（如 `2560 samples`），仅在有延迟时显示。
  - **拖拽手柄**：按住模块行左侧的把手可**拖拽重排**处理顺序，实时生效。
- **右侧参数面板**：显示当前选中模块的全部参数：
  - **开/关型参数**渲染为单个切换按钮（按下=开）；
  - **数值参数**为带数值显示的滑条；
  - 悬停任意控件有详细提示（含中文说明）。
- **Spectral Compressor 频谱视图**：选中 Spectral Compressor 时，参数面板顶部显示**实时输入频谱 + 门限曲线叠加**（对数频率轴）——调门限的核心反馈界面。
- **底部状态条**：从左到右依次是
  - **GLOBAL BYPASS**：全局旁路按钮。开启时整个插件位一致直通（可在宿主中自动化）；
  - **母带增益滑条**；
  - **立体声峰值表**：处理后的 L/R 峰值电平（dBFS 读数，超过满刻度变琥珀色）；
  - 实时总延迟、界面缩放、About。

---

## 3. 信号链与模块顺序

默认处理顺序：

```
1. Sine Generator    →  2. Soft Vacuum  →  3. Crisp
→  4. Spectral Gate  →  5. Frequency Shift
→  6. Spectral Compressor
→  7. Crossover      →  8. Diopser
→  9. Buffr Glitch   →  10. Gain  →  11. Safety Limiter
```

排序逻辑：音源在前 → 音色塑造（饱和/激励）→ 频域处理 → 频段动态 → 时域分频与相位效果 → 缓冲故障 → 母带增益与安全限制。

**顺序完全可自由调整**，重排结果随工程保存。

---

## 4. 模块详解

### 4.1 Sine Generator（正弦发生器）

- **由来**：移植自 nih-plug 官方示例 `sine`。
- **原理**：MIDI 触发的正弦波音源。f64 相位累加器，频率平滑滑音；音量受**按键力度（velocity）**与**复音压力（PolyPressure）**控制；音符开/关均经过 5 ms 线性防 click 包络。
- **参数**：Level（−24 ~ +6 dB，默认 −12）；Fallback Frequency（20 ~ 20000 Hz，默认 440）。
- **用法**：链首测试音源、校准监听电平、频谱模块的干净激励源。无 MIDI 音符时静音。

### 4.2 Soft Vacuum（柔和真空管饱和）

- **由来**：移植自 nih-plug 官方插件 `soft_vacuum`（算法源自 Chris Johnson / Airwindows 的 Hard Vacuum）。
- **原理**：多级**二极管桥式波形整形**饱和器。Drive > 100% 进入多级级联失真；Warmth 控制半周不对称；Aura 为额外偏置增益。**压摆率（slew）信号基带计算、独立过采样**，保证过采样与非过采样音色一致。
- **参数**：Drive（0–200%，默认 0）；Warmth（0–100%，默认 0）；Aura（0–100%，默认 0）；Output Gain（−40 ~ 0 dB）；Mix（默认 100%）；Oversampling（1x–16x，默认 2x，Lanczos3）。
- **用法**：鼓组暖化、人声电子管质感、贝斯激励。高 Drive 建议 4x 以上过采样。参数全部为**过采样感知平滑**，自动化无 zipper 噪声。

### 4.3 Crisp（高频激励器）

- **由来**：移植自 nih-plug 官方插件 `crisp`。
- **原理**：噪声 Ring Modulation 激励器。PCG32 白噪声经高通/低通整形后，与低通后的输入做环形调制，抽取并回注高频成分。
- **参数**：Amount（0–100%，默认 35%）；Mode（Soggy 全波形 / Crispy 仅正半周）；Stereo Mode（默认 Stereo）；RM Input LPF / Noise HPF / Noise LPF（噪声整形）；Output Gain；Wet Only。
- **用法**：人声/木吉他/军鼓的高频空气感。低 Amount + Wet Only 可做并联激励。

### 4.4 Spectral Gate（频谱门限）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：频域门限。STFT（2048 点、4 倍重叠、Hann 窗）化为 1024 频段，低于门限的置零或按 Tilt 曲线部分保留；Weak/Strong Balance 决定保留频段的能量加权。
- **参数**：Cutoff（0–1 按 dB 显示，默认 0.6）；Weak/Strong Balance（默认 0.7）；Tilt + Enable Tilt（默认 0.5 / Off）。
- **用法**：人声"机器人化"、鼓组频谱重组。先 Balance 定性格，再 Cutoff 定强度。

### 4.5 Frequency Shift（频谱搬移）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：全部频段整体平移固定 bin 数（非音高移调，产生金属/非谐波质感），或按比例缩放频谱（scale > 1 散开谐波，< 1 聚拢）。
- **参数**：Frequency Shift（±500 Hz，默认 0）；Frequency Scale（0.25–3.0，默认 1.0）。
- **用法**：微量正负偏移做金属感；Scale 对鼓组做颗粒聚拢/拉伸。二者可叠加。

### 4.6 Spectral Compressor（频谱压缩器）

- **由来**：移植自 nih-plug 官方插件 `spectral_compressor`。
- **原理**：多至 16384 个频段各自独立做向上+向下压缩：向下压刺耳共振，向上抬噪声底与空气感，叠加可实现任意频谱包络塑形。门限是一条随频率倾斜的**曲线**（Pink Noise 模式内置 −3 dB/oct 补偿）。20 Hz 以下频段不参与向上压缩；−100 dB 以下不参与向上增益。
- **GUI**：参数面板顶部的**实时频谱视图**显示输入频谱（绿色）与向下门限曲线（琥珀色），对数频率轴——曲线之上的频段被压缩，一目了然。
- **参数**（分组）：
  | 组 | 参数 | 默认 |
  |---|---|---|
  | Global | Output Gain / Mix / Window Size / Overlap / Attack / Release | 0 dB / 100% / 2048 / 16x / 150 ms / 300 ms |
  | Threshold | Global Threshold / Center / Slope / Curve | −12 dB / 420 Hz / 0 / 0 |
  | Upwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 75% / 6 dB |
  | Downwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 0% / 6 dB |
- **用法**：低比率+高重叠做透明母带染色；高向上比率把耳语推成轰鸣；Downwards 高比率+窄曲线精准去刺耳。所有曲线参数支持实时自动化（频谱视图同步刷新）。

### 4.7 Crossover（分频器）

- **由来**：移植自 nih-plug 官方插件 `crossover`（IIR 部分）。
- **原理**：**Linkwitz-Riley 24 dB/oct（LR4）**多频段分频，级联 Butterworth 滤波器 + 全通相位补偿，分频点相位对齐、总和平坦。AkiFX 为每个频段增加独立输出增益。
- **参数**：Bands（2–5，默认 2）；Crossover 1–4 Freq（40 Hz–20 kHz，默认 200/1000/5000/10000）；Band 1–5 Gain（±24 dB，默认 0）。
- **用法**：多段处理路由、频段独听、五路"频段推子"塑形。

### 4.8 Diopser（全通旋转滤波器）

- **由来**：移植自 nih-plug 官方插件 `diopser`。
- **原理**：最多 512 级**全通滤波器**级联。全通不改幅度只旋相位，多级级联在谐振频率产生巨大相位延迟；Spread 将各级频率展开成"滤波器云"。参数逐样本平滑，扫频无 click。
- **参数**：Filter Stages（0–512，默认 0 直通）；Filter Frequency（5 Hz–20 kHz，默认 200）；Filter Resonance（0.01–30，默认 0.5）；Filter Spread（±5 oct，默认 0）；Spread Style（Octaves/Linear）。
- **用法**：32–128 级 + Resonance 5–20 + 扫频 = 经典"Diopser 咆哮"。

### 4.9 Buffr Glitch（缓冲故障）

- **由来**：移植自 nih-plug 官方插件 `buffr_glitch`。
- **原理**：MIDI 触发的缓冲 repeats/口吃。NoteOn 录入恰好一个周期（音高=音符频率×八度移位）后循环回放，等功率交叉淡化消除接缝。8 复音，支持 PolyVolume 逐音符音量。干信号按声部包络反向闪避。
- **参数**：Dry Mix（默认 1.0）；Velocity Sensitive（Off）；Octave Shift（±2，默认 0）；Attack/Release（默认 2 ms）；Crossfade（默认 2 ms）。
- **用法**：人声/军鼓上挂载，键盘触发"冻结重复"。

### 4.10 Gain（增益）

- **由来**：移植自 nih-plug 官方示例 `gain`。
- **原理**：纯净数字增益级，50 ms 对数平滑。
- **参数**：Gain（−30 ~ +30 dB，默认 0）。

### 4.11 Safety Limiter（安全限制器）

- **由来**：移植自 nih-plug 官方插件 `safety_limiter`。
- **原理**：**保险丝**而非音乐性限制器：任何采样超阈值即触发 420 Hz 正弦播报的 **SOS 摩尔斯电码**并即时压低电平；静音 1 秒后恢复直通。NaN/Inf 数字爆音同样静音并触发警报。
- **参数**：Threshold（−24 ~ +12 dB，默认 0）。
- **用法**：母带末端 +6 dB 做"爆破头"保险；听到 SOS 即上游削波。

---

## 5. MIDI 支持

宿主发来的所有 MIDI 事件（音符、CC、弯音、复音压力等）广播给全部已启用模块：

| 模块 | 消费的 MIDI |
|---|---|
| Sine Generator | Note On/Off（力度→增益）、Poly Pressure（→增益） |
| Buffr Glitch | Note On/Off、PolyVolume（逐音符音量） |

---

## 6. 延迟与自动延迟补偿（PDC）

| 模块 | 延迟 |
|---|---|
| Spectral Gate / Frequency Shift | 2560 samples（FFT 2048 + hop 512，与原版一致） |
| Spectral Compressor | 窗口长度（默认 2048） |
| Soft Vacuum | 过采样核延迟（2x 约 10 samples） |

延迟变化在**同一处理块内**重新上报宿主（开关模块、改窗口/过采样均触发）。全局旁路不影响上报值。无需手动补偿。

---

## 7. 与原版插件的差异说明

对照上游（SpectralSuite @ `5dfd294`、nih-plug @ `f36931f`）逐行校准，默认参数下与原版一致。已知有意差异：

1. **集成性差异**：单插件多模块链、链级/全局旁路（位一致）、拖拽重排、全 MIDI 广播路由。
2. **裁剪的功能**：
   - SpectralSuite 的 FFT 尺寸/窗口选择与 PVOC 相位声码器模式未开放（固定 2048/4×/Hann，即原插件默认值）；
   - Crossover 的线性相位 FIR 变体已移除（仅保留实现过的 IIR LR24）；
   - Spectral Compressor 的侧链门限模式已移除（无侧链输入路由），仅 Pink Noise 模式；
   - Crisp 的 Crispy (alt) 模式已移除（上游中与 Crispy 完全相同）；
   - Diopser 的自动化精度参数与频谱分析 GUI 未移植（默认精度=逐样本平滑，行为一致）。
3. **上游怪癖的保留**：保留模块的上游行为逐式保留（含已知的量化怪癖，见技术文档）。
4. **AkiFX 增强**：Spectral Compressor 门限曲线实时自动化 + 频谱视图、Crossover 频段增益、全局旁路、峰值表、模块重排、音频线程零分配。

---

## 8. 常见问题

**Q：加载后没有声音？**
默认全部旁路。点亮机架 LED（或至少 Gain）。听到 SOS？那是 Safety Limiter 的超阈值警报。

**Q：v0.2.x 的工程升级后模块开关/顺序丢了？**
v0.3.0 是破坏性版本，旧状态不迁移，重置为默认。

**Q：CPU 占用高？**
每个频谱模块独立持有 2048 点 FFT 引擎。旁路模块零开销；全局旁路时全部跳过。

---

*AkiFX © Akiro · GPLv3 授权。SpectralSuite（Unlicense）；nih-plug 及其插件（ISC/GPL-3.0）；Hard Vacuum 算法 © Chris Johnson（Airwindows）。*

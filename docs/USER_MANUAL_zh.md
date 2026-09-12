# AkiFX 用户手册（中文版）

**版本 0.2.1 · VST3 / CLAP / 独立运行版**

AkiFX 是一款"全家桶"式效果器机架插件：它将两个著名开源项目——Andrew Reeman 的 **SpectralSuite**（C++/JUCE 频谱效果器套件）与 Robbert van der Helm 的 **nih-plug** 及其官方插件集——的全部功能性效果器整合到**单一条链路、单一界面**中，以 VST3 与 CLAP 格式导出，可在任意主流宿主（DAW）中加载。

本文档是 AkiFX 全部 21 个效果模块的权威参考：每个模块的**由来、声音原理、参数说明与使用建议**。

---

## 目录

1. [安装与加载](#1-安装与加载)
2. [界面概览](#2-界面概览)
3. [信号链与模块顺序](#3-信号链与模块顺序)
4. [模块详解](#4-模块详解)
   - 音源与 MIDI 类（Sine Generator / MIDI Inverter / Poly Mod Synth）
   - 饱和与激励类（Soft Vacuum / Crisp）
   - 频谱效果类（Spectral Gate / Frequency Shift / Frequency Magnet / Bin Scrambler / Morph / Phase Lock / Sinusoidal Shaped Filter / Playground）
   - 音高与分频类（Puberty Simulator / Crossover / Diopser）
   - 动态类（Loudness War Winner / Spectral Compressor）
   - 故障与母带类（Buffr Glitch / Gain / Safety Limiter）
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

首次加载时，**全部 21 个模块处于旁路（BYPASSED）状态**——插件等价于一根"直通线"。这是有意设计：频谱类模块消耗较多 CPU，用户按需点亮所需模块即可。**模块开关状态会随工程保存并自动恢复**。

从源码构建：`cargo xtask bundle akifx --release`（依赖 Rust stable MSVC 工具链）。

---

## 2. 界面概览

AkiFX 采用"机架 + 参数面板"双区布局：

- **左侧机架（Rack）**：21 个模块按信号链顺序垂直排列。每一行包含：
  - **LED 电源开关**：点击切换 ACTIVE / BYPASSED。旁路的模块完全不参与处理（零 CPU 占用），且保证**位一致（bit-identical）直通**——不会引入任何可听差异。
  - **延迟徽标**：该模块引入的采样延迟（如 `2560 samples`），仅在有延迟时显示。
  - **拖拽手柄**：按住模块行左侧的把手可**拖拽重排**处理顺序，实时生效。
- **右侧参数面板**：显示当前选中模块的全部参数，悬停任意控件有详细提示（含中文说明）。
- **底部状态条**：显示当前实际生效的总延迟（所有已启用模块延迟之和）。
- **窗口缩放**：界面支持自由缩放，缩放比例随工程保存。

---

## 3. 信号链与模块顺序

默认处理顺序（编号即默认链位）：

```
音源/MIDI → 饱和/激励 → 频谱效果 → 音高 → 分频/相位 → 动态 → 故障 → 母带
 1.Sine Generator   5.Soft Vacuum    6.Spectral Gate          14.Puberty Simulator  17.Loudness War Winner  18.Buffr Glitch  20.Gain
 2.MIDI Inverter    6.Crisp          7.Frequency Shift                                                     21.Safety Limiter
 3.Poly Mod Synth                    8.Frequency Magnet
 4.Playground                        9.Bin Scrambler
                                    10.Morph
                                    11.Phase Lock
                                    12.Sinusoidal Shaped Filter
                                    13.Spectral Compressor*
```

*实际编号见机架；Spectral Compressor 位于动态段（第 18 位）。

排序逻辑：音源在前 → 音色塑造（饱和/激励）→ 频域处理（按复杂度递增）→ 音高变换 → 时域分频与相位效果 → 动态控制 → 缓冲故障 → 母带增益与安全限制。

**顺序完全可自由调整**：例如把 Crisp 拖到频谱模块之后、把 Gain 拖到链条中间做推子使用。重排结果随工程保存。

---

## 4. 模块详解

### 4.1 Sine Generator（正弦发生器）

- **由来**：移植自 nih-plug 官方示例 `sine`。
- **原理**：MIDI 触发的正弦波音源。相位累加器以 f64 精度运行，输出频率支持平滑滑音。音量受**按键力度（velocity）**控制，并支持**多音_pressure（PolyPressure）**实时调制音量；音符开/关均经过 5 ms 线性防 clicks 包络。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Level | −24 ~ +6 dB | −12 dB | 输出电平 |
  | Fallback Frequency | 20 ~ 20000 Hz | 440 Hz | 频率平滑器的初始值 |
- **用法**：置于链首作为测试音源、校准监听电平、或作为频谱效果的"干净"激励源。无 MIDI 音符时完全静音。

### 4.2 MIDI Inverter（MIDI 反转器）

- **由来**：移植自 nih-plug 官方示例 `midi_inverter`，变换公式逐条对应。
- **原理**：**纯 MIDI 效果器**（音频位一致直通）。将 MIDI 事件做"镜像"变换：通道 `ch → 15−ch`、音高 `note → 127−note`、力度/压力/CC 值 `v → 1−v`（CC 编号不变），覆盖 Note On/Off、复音压力、弯音、CC、Choke 等全部事件类型。变换后的事件发送回宿主。
- **参数**：Enable（启用开关）。
- **用法**：实验性 MIDI 变换。典型用法：宿主轨道开启 MIDI 直通（thru）时，反转器输出的镜像事件可与原始事件叠加产生"双手演奏"效果；关闭 Enable 即恢复原状。注意：模块关闭时**不产出任何事件**（避免与宿主直通重复）。

### 4.3 Poly Mod Synth（复音调制合成器）

- **由来**：移植自 nih-plug 官方示例 `poly_mod_synth`。
- **原理**：16 复音减法合成器，每个声部由伪随机初始相位的锯齿/脉冲振荡器构成。CLAP 宿主下支持**复音调制（Poly Modulation）**——每个声部的增益/音高可由宿主逐音符自动化（VST3 下自动退化为标准 MIDI 行为，与上游一致）。声部分配采用"空闲槽位优先、否则替换最旧声部"策略。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Gain | −36 ~ 0 dB | −12 dB | 输出电平 |
  | Attack | 0 ~ 2000 ms | 200 ms | 音头（线性提升） |
  | Release | 0 ~ 2000 ms | 100 ms | 释音（指数衰减，精确归零后回收声部） |
- **用法**：置于链首作音源；后续频谱/滤波模块可作为它的"效果器"形成完整合成链。

### 4.4 Soft Vacuum（柔和真空管饱和）

- **由来**：移植自 nih-plug 官方插件 `soft_vacuum`，其算法本身源自 Chris Johnson（Airwindows）的 **Hard Vacuum**，经 Robbert van der Helm 授权重写并加入过采样。
- **原理**：多级**二极管桥式波形整形**饱和器。Drive 超过 100% 后进入多级级联失真（级数按 drive² 递增）；Warmth 控制正/负半周的不对称度（直流偏置与"塌陷"感）；Aura 为额外的输入增益/偏置项。**slew（压摆率）信号在基带单独计算后与音频分开过采样**，使过采样版本与非过采样版本音色一致——这是该移植的关键细节。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Drive | 0 ~ 200 % | 0 % | 失真量；>100% 进入多级级联 |
  | Warmth | 0 ~ 100 % | 0 % | 半周不对称/电子管"温暖感" |
  | Aura | 0 ~ 100 % | 0 % | 额外偏置增益（内部映射 [0, π]） |
  | Output Gain | −40 ~ 0 dB | 0 dB | 输出补偿 |
  | Mix | 0 ~ 100 % | 100 % | 干湿混合（线性） |
  | Oversampling | 1x / 2x / 4x / 8x / 16x | 2x | Lanczos3 多级过采样；16x 时延迟 80 samples |
- **用法**：鼓组总线暖化、人声"电子管"质感、贝斯谐波激励。高 Drive 时建议开启 4x 以上过采样以减少混叠。参数全部经过**过采样感知平滑（OversamplingAware）**，自动化时不会产生 zipper 噪声。

### 4.5 Crisp（"脆"高频激励器）

- **由来**：移植自 nih-plug 官方插件 `crisp`。
- **原理**：噪声 Ring Modulation（环形调制）激励器。内部以确定性 PCG32 伪随机数发生器产生白噪声，经可调高通/低滤整形后，与输入信号（先经可调低通）做环形调制——**仅"抽取"信号的高频成分**并叠加回来，以此在不改变音调的前提下补充高频空气感。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Amount | 0 ~ 100 % | 35 % | 激励量 |
  | Mode | Soggy / Crispy / Crispy (alt) | Crispy | RM 波形选择（Soggy 全波形；Crispy 仅正半周；Crispy (alt) 与上游一致，实际同 Crispy——上游即为如此） |
  | Stereo Mode | Mono / Stereo | Stereo | Mono 共用一路噪声源 |
  | RM Input LPF | 20 kHz 上限 | 22000 Hz（Disabled） | 环调前低通 |
  | Noise HPF / LPF | 可调 | 5 Hz / 22000 Hz | 噪声整形 |
  | Output Gain | −24 ~ 0 dB | 0 dB | 输出电平 |
  | Wet Only | 开关 | Off | 仅输出激励成分 |
- **用法**：人声、原声吉他、军鼓的高频"存在感"提升。Amount 低（10–30%）+ Wet Only 可做成并联激励。

### 4.6 Spectral Gate（频谱门限）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：在**频域**做门限处理。STFT（2048 点 FFT、4 倍重叠、Hann 窗）将信号化为 1024 个频段，按各频段能量与门限比较：低于门限的频段置零（或按 Tilt 曲线部分保留），高于门限的频段保留。"强/弱平衡"决定通带内频段按能量加权的保留程度——从"只留最响频段"的极端共振效果到近乎透明的扩张。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Cutoff | 0 ~ 1（按 dB 显示） | 0.6（−4 dB） | 门限 |
  | Weak/Strong Balance | 0 ~ 1 | 0.7 | 弱频段保留度（0 = 只留最强，1 = 接近直通） |
  | Tilt | 0 ~ 1 | 0.5 | 门限随频率倾斜 |
  | Enable Tilt | 开关 | Off | 启用倾斜 |
- **用法**：人声"机器人化/水声"、鼓组频谱重组、环境声提取共振峰。Cutoff 与 Balance 的组合是音色关键——先调 Balance 定"性格"，再调 Cutoff 定强度。

### 4.7 Frequency Shift（频谱搬移）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：将全部频段**整体平移**固定 bin 数（非音高移调——谐波间距被破坏，产生金属/铃声质感），或按**比例缩放**（scale）压缩/拉伸频谱——scale > 1 使谐波"散开"，< 1 使谐波"聚拢"并产生非谐波结构。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Frequency Shift | −500 ~ +500 Hz | 0 Hz | 整体频移（按 bin 取整，与上游一致） |
  | Frequency Scale | 0.25 ~ 3.0 | 1.0 | 频谱缩放 |
- **用法**：Shift 少量正负偏移做 ring-mod 式金属感；Scale < 1 对鼓组做"颗粒聚拢"，Scale > 1 做镀金铜管般的频谱拉伸。二者可叠加。

### 4.8 Frequency Magnet（频率吸附）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：把频谱各频段的能量向**目标频率**"吸附"重排：目标以下的频段按指数曲线向上收拢，以上的按另一条指数曲线向下收拢，形成一条以目标频率为谷（或峰）的能量漏斗。Strength 决定收拢强度（内部反转为宽度），Width Bias 塑造曲线偏置。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Frequency | 20 ~ 2000 Hz | 800 Hz | 吸附目标频率 |
  | Strength | 0 ~ 100 % | 50 % | 吸附强度（0 = 直通） |
  | Width Bias | 0 ~ 1 | 0.01 | 曲线偏置（影响上下两段的形状） |
  | Use Legacy Mode | 开关 | Off | 启用上游的旧版吸附曲线（不加目标频率偏移） |
- **用法**：对复音素材做"伪共振峰聚焦"，或把噪声推向特定频段制造"风声/哨声"。Strength 中等时最音乐化，满强度会产生强烈金属染色。

### 4.9 Bin Scrambler（频段打乱器）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：周期性地把 1024 个频段的**排列顺序**打乱，并在新旧两套排列之间做**逐 bin 的幅度/相位线性交叉淡化**（t = 相位/周期），实现平滑的"频谱洗牌"节拍。Scatter 把低频段的索引向中低频区"播撒"，Scramble 则按块打乱排列。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Scramble | 0 ~ 100 % | 10 % | 打乱量（按块洗牌，0 = 直通） |
  | Scatter | 0 ~ 100 % | 40 % | 低频播撒量 |
  | Rate | 0.25 ~ 15 Hz | 2 Hz | 重排节拍速率 |
  | Random Seed | 0 ~ 9999 | 0 | 0 = 每次随机；非零 = 可复现 |
- **用法**：鼓循环的"碎频"节奏效果、氛围垫的颗粒化流动。Rate 与工程 BPM 成整数/分数关系时最"节拍化"。

### 4.10 Morph（频谱形变）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：频谱**排列映射**——把第 i 个频段的能量"搬运"到由 16 个控制点样条映射出的目标频段，实现频谱结构的拉伸/压缩形变（如把泛音列整体拉开）。控制点曲线在 0–1 归一化频率轴上编辑。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Mix | 0 ~ 100 % | 100 % | 处理后信号与干信号的混合 |
  | Number of Overlaps | 1 / 2 / 4 / 8 | 4 | STFT 重叠数（越大越平滑，CPU 越高） |
  | Use PVOC | 开关 | On | 关闭时完全直通（旁路频域处理） |
  | Control Points ×16 | 0 ~ 1 | 线性 | 频段映射曲线 |
- **用法**：把镲片频谱"拉开"成金属风暴、把人声共振峰搬家。控制点默认为对角线（直通）——拖动中间点开始形变。

### 4.11 Phase Lock（相位锁定）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：捕获某一刻的频谱**相位/幅度快照**并"冻结"：之后所有帧的相位被拉向锁定值（Frequency Lock + Phase Mix），幅度亦可锁定（Magnitude Lock + Magnitude Mix），产生标志性的"频谱冻结/金属化延音"。Morph 功能可在锁定态与实时态之间按可调时长往复过渡；Random Phase 注入随机相位抖动。注意：**每个声道、每个 STFT 重叠实例持有独立快照**（与上游一致），因此立体声声像被完整保留。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Frequency Lock | 开关 | Off | 启动相位锁定（带过渡） |
  | Phase Mix | 0 ~ 100 % | 100 % | 相位拉向锁定的比例 |
  | Magnitude Lock | 开关 | Off | 启动幅度锁定 |
  | Magnitude Mix / Tracking | 0 ~ 100 % | 100 % / 0 % | 幅度锁定比例 / 跟随实时幅度 |
  | Random Phase | 0 ~ 100 % | 0 % | 随机相位抖动 |
  | Morph | 开关 + Duration 1–30 s | Off / 2 s | 锁定↔实时往复过渡 |
- **用法**：人声延音"冻结"、实现类似 spectral freeze 的金属质感Pad。先 solo 一句素材、开 Frequency Lock、再把 Mix 从 0 推到 100 听过渡。

### 4.12 Sinusoidal Shaped Filter（正弦整形滤波器）

- **由来**：移植自 SpectralSuite 同名插件。
- **原理**：以一条 1024 点正弦查找表对频谱幅度做**逐频段整形**：每个频段按其在频谱中的索引位置查表取值（支持真线性插值），再按 `value^(width²·8+1)` 幂次缩放幅度——等效于一组形状由正弦波决定的多峰梳状滤波器组。Frequency 平移查找表读取位置，Phase 控制起始相位。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Frequency | 0 ~ 10 | 7 | 表读取步进（梳齿密度） |
  | Width | 0 ~ 1 | 0.9 | 整形深度 |
  | Phase | 0 ~ 1 | 0.5 | 查表起始相位 |
  | Mix / Use PVOC / Overlaps | — | 100 % / On / 4 | 同 Morph |
- **用法**：制造乐音化梳状共鸣（类似自动 wah 的频域版）、对人声做"合成器化"谐波重塑。

### 4.13 Spectral Compressor（频谱压缩器）

- **由来**：移植自 nih-plug 官方插件 `spectral_compressor`。
- **原理**：把信号分成多达 16384 个频段，**每个频段独立做向上+向下压缩**：向下压缩器"压"过响的频段（驯服刺耳共振），向上压缩器"抬"过弱的频段（把噪声底/空气感拉上来），二者叠加可实现近乎任意的频谱包络塑形。门限不是单值而是一条**随频率倾斜的曲线**（Pink Noise 模式内置 −3 dB/oct 补偿）。20 Hz 以下频段不参与向上压缩（避免直流泄漏被放大），−100 dB 以下的频段不参与向上增益（上游同款防噪声放大设计）。
- **参数**（分组）：
  | 组 | 参数 | 默认 | 说明 |
  |---|---|---|---|
  | Global | Output Gain | 0 dB | 输出补偿 |
  | Global | Mix | 100 % | 干湿（15 ms 平滑） |
  | Global | Window Size / Overlap | 2048 / 16x | STFT 配置（窗口 64–32768，重叠 4–32x） |
  | Global | Attack / Release | 150 / 300 ms | 包络时间 |
  | Threshold | Global Threshold | −12 dB | 曲线基准门限 |
  | Threshold | Center / Slope / Curve | 420 Hz / 0 / 0 | 门限曲线形状（Pink Noise 模式含 −3 dB/oct） |
  | Upwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 75 % / 6 dB | 向上压缩器 |
  | Downwards | Offset / Ratio / Hi-Freq Rolloff / Knee | 0 dB / 1.0 / 0 % / 6 dB | 向下压缩器 |
- **用法**："频谱胶水"——低比率 + 高重叠做近乎透明的母带染色；高向上比率把耳语推成轰鸣；Downwards 高比率 + 窄曲线 = 精准去刺耳。所有曲线参数支持**实时自动化**。

### 4.14 Puberty Simulator（变声模拟器）

- **由来**：移植自 nih-plug 官方插件 `puberty_simulator`。
- **原理**：FFT 相位声码器**降调**器（默认 −1 个八度，故名"变声"）。按帧做窗口化 FFT，把各频段整体搬到更低频段后 OLA（重叠相加）重建；提供矩形/极坐标两种插值模式。延迟 = 窗口长度（默认 1024 samples），随窗口参数自动上报宿主。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Pitch | −5 ~ +5 octaves | −1 | 音高移位（负值 = 降调） |
  | Window Size | 64 ~ 32768 | 1024 | STFT 窗口 |
  | Window Overlap | 4x ~ 32x | 8x | 重叠倍数 |
  | Mode | Rectangular / Polar | Rectangular | 频段插值方式 |
- **用法**：人声"低沉/怪物"效果、贝斯副八度。上游作者注明：该算法故意保留了"独特的 artifacts"——它是效果器而非透明变调工具。

### 4.15 Crossover（分频器）

- **由来**：移植自 nih-plug 官方插件 `crossover`（IIR 部分）。
- **原理**：**Linkwitz-Riley 24 dB/oct（LR4）**多频段分频：每级分频由两级级联 Butterworth 低通/高通构成，相邻频段在分频点相位对齐、总和平坦；低通频段经过**全通级联相位补偿**以保持各频段相位一致。AkiFX 在上游基础上为每个频段增加了独立的输出增益（可做静态多段 EQ/多段动态的前级）。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Bands | 2 ~ 5 | 2 | 频段数 |
  | Crossover 1–4 Freq | 40 Hz ~ 20 kHz | 200 / 1000 / 5000 / 10000 Hz | 各分频点 |
  | Band 1–5 Gain | −24 ~ +24 dB | 0 dB | 各频段输出增益 |
  | Crossover Type | LR24 / LR24 (LP) | LR24 | 线性相位 FIR 变体为预留占位（见 §7） |
- **用法**：多段处理路由（配合宿主侧链）、扬声器管理式频段独听、或把 5 个频段当 5 路"频段推子"做极端塑形。

### 4.16 Diopser（全通旋转滤波器）

- **由来**：移植自 nih-plug 官方插件 `diopser`。
- **原理**：**级联全通滤波器**（最多 512 级）。全通滤波器不改变幅度频响，只旋转相位——多级级联在特定频率产生巨大相位延迟，配合自动化扫频可得到类似镶边（flanger）但更"厚"的共鸣效果。Spread 使各级滤波器的谐振频率按八度/线性规律展开成"滤波器云"。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Filter Stages | 0 ~ 512 | 0（直通） | 级数 |
  | Filter Frequency | 5 Hz ~ 20 kHz | 200 Hz | 基准频率 |
  | Filter Resonance | 0.01 ~ 30 | 0.5 | 谐振锐度 |
  | Filter Spread | ±5 octaves | 0 | 各级频率展开量 |
  | Spread Style | Octaves / Linear | Octaves | 展开规律 |
- **用法**：Stages 32–128 + Resonance 5–20 + 手动/自动扫频 = 经典"Diopser 咆哮"。所有参数平滑滑动，自动化无 click。

### 4.17 Loudness War Winner（响度战争赢家）

- **由来**：移植自 nih-plug 官方插件 `loudness_war_winner`——一个**玩笑式**插件（官方标语 "Win the loudness war with ease"，VST3 子分类含 "Pain"）。
- **原理**：根本不是压缩器：**把每个非静音采样硬性输出为 `sign(x) × Output Gain`**——即把任何信号变成输出增益电平的方波（默认 −24 dBFS），LUFS 瞬间拉满、听感瞬间报废。WIN HARDER 启用一个 Q 值随参数上升（Q = 0.00001 + f×30）的 5.5 kHz 四级带通（致敬 LUFS K 计权的高频台肩），让方波更加刺耳。为避免持续输出直流方波，静音 1 秒后开始线性淡出，2 秒后完全静音。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Output Gain | −24 ~ 0 dB | −24 dB | 方波电平 |
  | WIN HARDER | 0 ~ 100 % | 0 % | 带通折磨强度 |
- **用法**：真·用途极少：测试监听链、整蛊同事、或作为"为什么不能只拼响度"的教学演示。请勿在正式作品上启用（除非目的就是 pain）。

### 4.18 Buffr Glitch（缓冲故障效果）

- **由来**：移植自 nih-plug 官方插件 `buffr_glitch`。
- **原理**：MIDI 触发的**缓冲 repeats/口吃**效果。NoteOn 时为该音符分配一个声部，以输入信号录制恰好一个周期（音高对应音符频率 × 八度移位），随后循环回放该缓冲并做等功率交叉淡化消除接缝。最多 8 复音，按"空闲 → 最安静的释放中声部 → 最安静声部"策略分配。支持 **PolyVolume 逐音符音量**（音符表达）自动化。干信号按声部包络反向闪避。
- **参数**：
  | 参数 | 范围 | 默认 | 说明 |
  |---|---|---|---|
  | Dry Mix | 0 ~ 1 | 1.0 | 干信号电平（被活跃声部包络闪避） |
  | Velocity Sensitive | 开关 | Off | 力度映射声部增益 |
  | Octave Shift | −2 ~ +2 | 0 | 缓冲回放音高移位 |
  | Attack / Release | 0 ~ 50 ms | 2 ms | 声部包络 |
  | Crossfade | 0 ~ 50 ms | 2 ms | 循环接缝交叉淡化 |
- **用法**：在人声/军鼓上挂载，用键盘触发"冻结重复"；现场表演型效果器。

### 4.19 Gain（增益）

- **由来**：移植自 nih-plug 官方示例 `gain`。
- **原理**：纯净的数字增益级，50 ms 对数平滑消除调节爆音。
- **参数**：Gain，−30 ~ +30 dB，默认 0 dB。
- **用法**：链条中任意位置的电平微调；默认 −30 dB 上限以内可安全用于精细的自动化推子。

### 4.20 Safety Limiter（安全限制器）

- **由来**：移植自 nih-plug 官方插件 `safety_limiter`。
- **原理**：不是"好听的"限制器，而是**保险丝**：任何采样超阈值即触发 420 Hz 正弦播报的 **SOS 摩尔斯电码**警报，并即时拉低电平；连续 1 秒无声后恢复正常直通，连续超峰会持续报警。NaN/Inf（数字爆音）同样被静音**并触发警报**——这正是它的核心价值：在母带上挂一个，混音出问题时你会立刻"听到"。
- **参数**：Threshold，−24 ~ +12 dB（增益斜偏），默认 0 dB。
- **用法**：挂在母带链末端、增益设 +6 dB 左右做"爆破头"保险；只要听到 SOS 就说明上游某处 clip 了。

### 4.21 Playground（频谱实验场）

- **由来**：移植自 SpectralSuite 的 Playground。
- **原理**：**恒等频谱处理器**——完整的 STFT→FFT→恒等回调→IFFT→OLA 管线，但对频谱不做任何修改（重构误差在数值容差内）。它存在的意义：作为开发者/学习者的"频谱插件模板"，以及验证链路本身的中性测试点。
- **参数**：Mix（默认 100 %）、Use PVOC（On）、Number of Overlaps（默认 4）及两个**惰性**兼容开关（上游同样未使用）。
- **用法**：把它当作"直通参考"：对比启用/旁路 Playground 可验证宿主 PDC 是否正确对齐。

---

## 5. MIDI 支持

AkiFX 以 `MidiCCs` 级别声明 MIDI 输入/输出：**宿主发来的所有 MIDI 事件（音符、CC、弯音、复音压力、Choke 等）都会广播给链上全部已启用模块**，各模块按需消费：

| 模块 | 消费的 MIDI |
|---|---|
| Sine Generator | Note On/Off（力度→增益）、Poly Pressure（→增益） |
| Poly Mod Synth | Note On/Off、Choke、（CLAP 下）复音调制 |
| Buffr Glitch | Note On/Off、PolyVolume（逐音符音量） |
| MIDI Inverter | 全部事件类型（变换后回送宿主） |

MIDI Inverter 产生的事件（以及 Poly Mod Synth 的行为）会通过插件的 MIDI 输出回送宿主。

---

## 6. 延迟与自动延迟补偿（PDC）

多个模块具有**算法延迟**，插件会在延迟发生变化的**同一处理块内**重新上报宿主：

| 模块 | 延迟 |
|---|---|
| SpectralSuite 系（Gate/Shift/Magnet/Scrambler/Morph/PhaseLock/SSF/Playground） | 2560 samples（FFT 2048 + hop 512，与原版 C++ 一致） |
| Spectral Compressor | = 窗口长度（默认 2048） |
| Puberty Simulator | = 窗口长度（默认 1024） |
| Soft Vacuum | 过采样核延迟（2x 时约 10 samples） |

宿主据此做自动延迟补偿（PDC）。切换模块开关、调整窗口大小或过采样倍数都会即时更新上报值。**无需手动微调**。若你的宿主 PDC 有缺陷，可参考底部状态条的总延迟手动补偿。

---

## 7. 与原版插件的差异说明

AkiFX 与上游逐行对照校准（对照 nih-plug @ `f36931f` 与 SpectralSuite @ `5dfd294`），默认参数下的声音行为与原版一致。已知的有意差异：

1. **集成性差异（设计使然）**：单插件多模块链路、链级旁路（位一致直通）、模块顺序可调、全 MIDI 广播路由、MIDI Inverter 关闭时不回显事件（避免与宿主直通重复）。
2. **裁剪的功能**：
   - SpectralSuite 的 FFT 尺寸选择（2^7–2^20）、窗口类型选择与 PVOC 相位声码器模式未开放（固定 2048/4×/Hann，即各原插件的默认值）；
   - Frequency Magnet 的 MIDI 触发（note 选频率、CC1 控宽度）未移植；
   - Spectral Compressor 的侧链模式（Sidechain Match/Compress）保留枚举但未接线，仅 Pink Noise 模式可用；
   - Crossover 的线性相位 FIR 变体为占位（选择 LR24 (LP) 时实际使用 IIR LR24）；
   - Diopser 的自动化精度参数与内置频谱分析 GUI 未移植（默认精度=逐样本平滑，行为一致）。
3. **上游怪癖的保留**：Crisp 的 Crispy (alt) 模式与 Crispy 完全相同（上游如此）；Loudness War Winner 的"方波化"与 5.5 kHz WIN HARDER 带通按原样移植。
4. **上游没有的增强**：Spectral Compressor 门限曲线实时自动化（原版为回调接线缺陷的修复性设计）、频段增益（Crossover）、链级 bypass 持久化、模块重排、全部模块的音频线程零分配处理。

---

## 8. 常见问题

**Q：加载后没有声音？**
默认所有模块旁路。点亮机架上的 LED（或至少 Gain）即可。若仍无声，检查 Safety Limiter 是否正在播 SOS——那是超阈值警报。

**Q：为什么有些模块有延迟徽标，有些没有？**
只有 STFT/过采样类模块有算法延迟。宿主会自动补偿；徽标数字仅供参考。

**Q：模块开关为什么每次重开工程都保留了？**
开关状态随工程保存（v0.2.2 起）。旧工程（v0.2.0 及更早）不含此数据，首次加载仍为全旁路。

**Q：Spectral Compressor 调门限没反应？**
确认 Threshold 组参数在变化（曲线参数变化会在下一个处理块生效）。v0.2.0 存在门限自动化失效的缺陷，请升级。

**Q：CPU 占用高？**
每个频谱模块独立持有 2048 点 FFT 引擎（与原版插件各自独立运行一致）。同时点亮多个频谱模块时 CPU 线性叠加；旁路模块零开销。

---

*AkiFX © Akiro · GPLv3 授权。SpectralSuite（Unlicense）；nih-plug 及其插件（ISC/GPL-3.0）；Hard Vacuum 算法 © Chris Johnson（Airwindows）。*

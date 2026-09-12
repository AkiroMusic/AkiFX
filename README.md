# AkiFX

**面向音乐制作人的全能 VST3 效果器插件** —— 将 [SpectralSuite](https://github.com/andrewreeman/SpectralSuite) 与 [nih-plug](https://github.com/robbert-vdh/nih-plug) 两个项目的全部功能性音频插件整合为一个强大的模块链总插件。

## 插件格式

| 格式 | 位置 |
|---|---|
| **VST3** | `target/bundled/AkiFX.vst3/` |
| **CLAP** | `target/bundled/AkiFX.clap` |
| 独立运行版 | `target/bundled/akifx-standalone.exe`（WASAPI/JACK，可用于无 DAW 测试） |

## 内置模块（21 个）

### 来自 SpectralSuite（共享 STFT 频谱引擎）
- **SpectralGate** — 频谱门限
- **FrequencyShift** — 频谱搬移
- **FrequencyMagnet** — 频率吸附
- **BinScrambler** — 频段打乱
- **Morph** — 频谱形变
- **PhaseLock** — 相位锁定
- **SinusoidalShapedFilter** — 正弦整形滤波
- **Playground** — 频谱实验场

### 来自 nih-plug
- **Soft Vacuum** — Airwindows Hard Vacuum 饱和移植（16x 过采样）
- **Buffr Glitch** — MIDI 触发缓冲故障
- **Crisp** — 高频激励器
- **Crossover** — 2–5 段 Linkwitz-Riley 分频
- **Diopser** — 全通相位旋转滤波器
- **Loudness War Winner** — 响度战争最大化器
- **Puberty Simulator** — 八度下移变调
- **Safety Limiter** — 安全限制器（超阈值播放 SOS 摩尔斯码）
- **Spectral Compressor** — 16384 频段频谱压缩器
- **Gain / Sine Gen / MIDI Inverter / Poly Mod Synth** — 工具与音源模块

## 处理顺序

音源 → MIDI 变换 → 饱和/激励 → 频谱效果 → 音高 → 分频 → 相位 → 动态 → 故障 → 主增益 → 安全限制器。每个模块可独立旁路；旁路时位一致直通。

## 构建

```shell
cargo xtask bundle akifx --release   # 生成 VST3 + CLAP 到 target/bundled/
cargo test -p akifx                  # 运行全部测试
```

依赖：Rust stable (MSVC) + VS2022 Build Tools（Windows）。首次构建需联网拉取 nih-plug。

## 许可证

AkiFX 以 **GPLv3** 发布（VST3 绑定所必需）。致谢：SpectralSuite（Unlicense/公有领域）、NIH-plug 及其插件（ISC/GPLv3）、Airwindows Hard Vacuum（Chris Johnson）。详见 GUI 关于页。

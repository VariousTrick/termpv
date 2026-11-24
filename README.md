# termpv (Terminal MPV)

> 🚀 **基于 Rust 构建的高性能、多模式、低延迟终端视频播放器。**  
> 兼具现代终端 GPU 硬件级高清协议（Kitty / Sixel）与经典字符美学（TrueColor 半块像素 / ASCII），提供类似 MPV 的极致丝滑交互与音画体验。

---

## ✨ 核心特性

- 🎯 **多渲染后端无缝热切换**：
  - **Kitty Graphics Protocol**：针对 Ghostty、Kitty 等现代终端，直接传输原始 RGBA 帧，显卡硬件加速，达到原生播放器级别的 **60 FPS 无损画质**。
  - **24-bit TrueColor 半块像素（`▀`）**：每个字符格容纳上下两个独立像素，结合 ANSI 颜色压缩与黑边居中自适应，在任意终端（如 VSCode 内置终端、tmux、Windows Terminal）均可呈现高清细腻画面。
  - **经典 ASCII 字符灰度矩阵**：感知亮度映射阶梯字符（` .:-=+*#%@`），保留黑客帝国复古字符美学。
- ⚡ **原生 FFmpeg 解码与 libswscale 硬件加速**：
  - 调用底层 `libavcodec` 与 `libswscale`，直接在解码层以 SIMD 汇编完成色空间转换与缩放。
- ⏱️ **刚性主时钟与自适应丢帧调度（Zero Lag）**：
  - 彻底解决传统终端播放器“积压卡死”的通病。当检测到终端 I/O 滞后超过 30ms 时，自适应果断丢帧追赶时钟，音画严格保真。
- 🎹 **类 MPV 的全套交互体验**：
  - 支持即时暂停、快进快退（Seek）、音量调节、底行 OSD 状态面板（实时 FPS、播放进度条、丢帧统计）。
- 🛡️ **健壮的终端保护守卫（RAII）**：
  - 自动接管备用屏（Alternate Screen）与 Raw Mode；程序无论是正常退出、按 `Ctrl+C` 还是发生 Panic，均可 100% 自动无损复原终端光标与历史。

---

## ⌨️ 快捷键列表

| 按键 | 功能描述 |
| :--- | :--- |
| <kbd>Space</kbd> | 暂停 / 恢复播放 |
| <kbd>↑</kbd> / <kbd>↓</kbd> | **大步跨越快进 60 秒 / 快退 60 秒** |
| <kbd>→</kbd> / <kbd>l</kbd> | 快进 5 秒 / 10 秒 |
| <kbd>←</kbd> / <kbd>h</kbd> | 快退 5 秒 / 10 秒 |
| <kbd>+</kbd> / <kbd>=</kbd> / <kbd>0</kbd> | 音量增加 5% |
| <kbd>-</kbd> / <kbd>_</kbd> / <kbd>9</kbd> | 音量减小 5% |
| <kbd>x</kbd> | 一键静音 / 恢复音量 |
| <kbd>m</kbd> | **实时无缝轮换渲染模式**（Kitty ⇄ Half-Block ⇄ ASCII） |
| <kbd>Tab</kbd> / <kbd>o</kbd> | 开启 / 关闭底部 OSD 极简 HUD 进度条 |
| <kbd>q</kbd> / <kbd>Esc</kbd> | 安全退出播放器 |

---

## 🛠️ 环境准备与快速上手

### 系统依赖（Arch Linux 为例）

```bash
# 安装 FFmpeg 开发库与 Clang（供 Rust 生成底层绑定）
sudo pacman -S ffmpeg clang
```

### 编译与运行

```bash
# 克隆/进入项目目录
cd /home/arch/Downloads/vscode/termpv

# 启动本地播放（支持 MP4、MKV、AVI、FLV、WebM 等任意格式，自动跟随系统默认音频设备）
cargo run --release -- /home/arch/Downloads/test.mp4

# 播放网络直链流（支持 PikPak、Amane 刮削源、HTTP/HTTPS/RTSP/RTMP，智能 Range 分片与 300 包防抖缓冲）
cargo run --release -- "https://example.com/video.mp4"

# 指定初始渲染模式（可选：auto, kitty, sixel, half-block, ascii）
cargo run --release -- /home/arch/Downloads/test.mp4 -m half-block

# 开启循环播放
cargo run --release -- /home/arch/Downloads/test.mp4 --loop
```

---

## 📁 模块化项目结构

```
termpv/
├── src/
│   ├── main.rs          # 极简程序入口与参数传递
│   ├── app.rs           # 核心控制器：驱动解码流、事件排空与主渲染循环
│   ├── config.rs        # 命令行参数定义与 RenderMode 枚举
│   ├── error.rs         # 统一强类型错误枚举 (TermpvError)
│   ├── core/            # 底层媒体解码与同步
│   │   ├── clock.rs     # 刚性主时钟、A/V 评估与实时 FPS 统计
│   │   ├── decoder.rs   # FFmpeg 视频流解码与 libswscale 色彩缩放
│   │   └── demuxer.rs   # 容器解封装与 Seek 控制
│   ├── render/          # 多后端渲染体系
│   │   ├── mod.rs       # 终端环境自动嗅探与渲染引擎派发
│   │   ├── buffer.rs    # RgbFrame 帧缓冲结构
│   │   ├── halfblock.rs # 24-bit TrueColor 半块 (▀) 渲染器（含 ANSI 压缩与居中）
│   │   ├── ascii.rs     # ASCII 字符矩阵渲染器
│   │   └── kitty.rs     # Kitty Graphics Protocol 分块 Base64 硬件直出
│   └── tui/             # 终端控制与交互
│       ├── guard.rs     # RAII 终端状态守卫与崩溃保护
│       ├── event.rs     # 非阻塞键盘事件批处理排空（彻底杜绝按键卡死）
│       └── osd.rs       # MPV 风格 OSD 状态栏与进度条
```

---

## 📄 许可说明

本项目基于 MIT 协议开源。

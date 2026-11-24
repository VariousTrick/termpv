# termpv 开发指南与技术说明文档 (DEVELOPMENT.md)

本文档面向维护与开发 `termpv` 的工程师，阐释系统的底层架构逻辑、设计哲学与关键性能考量。

---

## 一、 架构设计哲学

### 1. 模块边界与职责隔离
在 `termpv` 中，**禁止在同一个文件中堆砌多种关注点**。各模块的依赖方向为单向依赖：

```
[main.rs]
   │
   ▼
 [app.rs] ─────── 控制与调度层
   │
   ├──> [core/]   底层多媒体核心（解封装、解码、时钟同步）
   ├──> [render/] 图像格式化、色空间转换与终端协议编码
   └──> [tui/]    终端状态接管、非阻塞事件驱动与 OSD 视图绘制
```

### 2. 编写无“箭头嵌套”的代码（Flat & Clean Code）
所有模块必须贯彻以下习惯：
- **前置校验与卫语句（Guard Clauses）**：遇到非法参数、边界条件、无事件发生时，立即 `return` 或 `continue`。
- **模式匹配扁平化**：善用 `let Some(x) = opt else { return; }`，避免多重嵌套的 `match` 与 `if let`。
- **拆分小函数**：单一函数尽量控制在 50 行内，超过则提炼为私有辅助函数。

### 3. 热路径“零冗余分配”（Zero-Allocation Hot Path）
由于视频每秒产生 30~60 帧，任何在帧循环内的堆内存分配（如高频 `format!()` 或 `Vec::new()`）都会造成严重的内存抖动：
- `HalfBlockRenderer` 与 `AsciiRenderer` 内部持有 `scratch_buf: String`，通过 `.clear()` 原位复用内存。
- 视频缩放复用 `libswscale` 的 `ScalerContext`，仅在窗口尺寸变动时重新构建。
- 输出流统一包裹 `BufWriter<Stdout>`（容量 8MB），确保单帧 Kitty 图像与 OSD 序列一次性原子刷盘（`flush`）。

---

## 二、 关键技术难题与核心算法

### 1. 音画同步（A/V Sync）与防卡顿丢帧算法
终端模拟器的 ANSI 解析速度受系统负载与 PTY 管道容量制约，**绝对不能依赖简单的定时器睡眠**：
- **主时钟（Master Clock）**：以真实物理时间/音频 PTS 为锚点。
- **时间差计算**：$\Delta t = \text{Frame PTS} - \text{Clock Current Time}$。
  - **严重滞后（$\Delta t < -30\text{ms}$）**：说明终端输出或解码已经跟不上进度，**立即丢弃（Drop）此帧**，跳过颜色转换与 ANSI 生成，让管线快速追赶。
  - **超前（$\Delta t > 2\text{ms}$）**：说明画面快于时钟。为避免阻塞用户输入，将睡眠拆分为细粒度分片（每片不超过 3~5ms），边休眠边排空键盘队列。
  - **准时区间（$-30\text{ms} \le \Delta t \le 2\text{ms}$）**：立即渲染上屏。

### 2. 事件循环防卡死（Event Draining）
在终端原始模式下，高频按键或方向键产生的转义序列会在标准输入积压。  
旧版单次 `poll()` 仅读取单个按键，导致高频操作时输入延迟以秒级递增。  
**解决方案**：`src/tui/event.rs` 中的 `drain_actions()`：
```rust
while event::poll(Duration::ZERO)? {
    // 循环排空 Crossterm 缓冲区中的所有事件
}
```
保证每帧处理前，所有积压的物理按键已全部被即时消费。

### 3. 画面纵横比保持与黑边填充（Letterboxing）
终端字符单元格并非正方形（通常高度约为宽度的两倍）：
- 在 **HalfBlock** 模式下，1个字符格容纳上下2个像素，这使得“1个字符宽”在物理尺寸上恰好与“2个半块高”近似等长（长宽比接近 1:1）。
- `calculate_target_resolution` 利用该物理特性，以原视频比例 $\text{Aspect} = \frac{W_{src}}{H_{src}}$ 拟合字符网格，并在 `HalfBlockRenderer` 中自动计算上下左右留白（黑边），彻底消除了人物被压扁或画面满屏大块的伪影。

### 4. 双向内存缓存、`hr-seek=no` 关键帧瞬跳与 3D LUT 色彩滤镜
- **双向缓存与关键帧秒跳（`src/core/pipeline.rs`、`src/audio/player.rs`）**：
  - 解复用器保持 `demux_epoch` 不断流，视频解码 Worker 维护 150MB `history_packets`，声卡维护 60 秒 PCM `history_buffer`，实现前后双向免网络重连跳转；
  - 缓存命中 Seek 时直接对齐目标附近关键帧（I 帧）并由声卡 `skip_to(snap_pts)` 同步对齐（附带 3ms 线性微淡入），避开整段 GOP 软解预卷耗时。
- **3D LUT 定点三线性插值滤镜（`src/render/filter.rs`）**：
  - 切换风格时在 CPU 上预计算 `33×33×33` 查找表（~108KB），逐帧对 `RgbFrame` 原地执行定点整数三线性插值（消除 64 级直查带来的色阶断层），所有渲染后端（Kitty / HalfBlock / ASCII）统一共享。

---

## 三、 本地调试与构建指令

```bash
# 格式化代码
cargo fmt

# 静态语法与规范检查
cargo clippy

# Debug 调试编译
cargo build

# Release 极致性能优化编译（启用 LTO 与 opt-level 3）
cargo build --release

# 带参数运行测试真实视频
cargo run --release -- /home/arch/Downloads/test.mp4
```

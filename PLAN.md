# termpv (Terminal MPV) 项目开发计划与架构规范

> **目标**：打造一款兼具「现代终端高清图像协议（Kitty/Sixel）」与「经典字符艺术（Half-block/ASCII）」的高性能、零卡顿、MPV 级交互体验的终端视频播放器。

---

## 一、 核心设计准则与代码规范

为了保证代码具备高度可读性、可维护性与极致运行性能，本项目严格遵循以下规范：

1. **注释规范**：
   - 关键结构体、枚举、公共方法、复杂算法（如 A/V 同步、ANSI 压缩）均采用**清晰的中文注释**。
2. **拒绝箭头型嵌套代码（Anti-Arrow Code）**：
   - **Guard Clauses & Early Return**：不符合前置条件时立即 `return` 或 `continue`，严禁多层 `if-else` 嵌套。
   - **充分利用 Rust 现代语法**：优先使用 `?` 错误传递、`let Some(val) = opt else { ... }` 展开，严禁深层嵌套的 `match` 或 `if let`。
   - **单一职责拆分**：渲染循环、按键分发等复杂逻辑必须拆分为具名的私有辅助函数，每个函数控制在 30~50 行以内。
3. **渲染主循环“零堆内存分配”（Zero-Allocation in Render Loop）**：
   - 在 60 FPS 播放时，严禁在渲染循环中每次 `format!()` 动态创建大量 String 或无底线 `vec![]`。
   - 预分配固定容量的 FrameBuffer 与 ANSI 输出缓冲区（Reusable Scratch Buffer），通过原位复写和 `stdout.write_all()` 批量刷盘。

---

## 二、 架构模块划分

```
termpv/
├── Cargo.toml
├── README.md                 # 项目介绍与使用指南
├── CHANGELOG.md              # 版本迭代日志 (更新至 v0.2.4)
├── DEVELOPMENT.md            # 底层架构逻辑与开发指南
├── PLAN.md                   # 阶段里程碑与实施计划
└── src/
    ├── main.rs               # 程序入口、CLI 参数解析与生命周期调度
    ├── app.rs                # 应用核心调度器（Contain 自适应计算、音视频协同解码）
    ├── config.rs             # CLI 参数定义（RenderMode、音量、循环、日志等）
    ├── error.rs              # 强类型错误定义 (TermpvError)
    ├── logger.rs             # TUI 友好的无锁异步文件日志系统
    ├── core/                 # 核心解封装与解码
    │   ├── clock.rs          # 单调高精度 CPU 主时钟，保证 60 FPS 丝滑无卡顿
    │   ├── decoder.rs        # FFmpeg 视频解码器（Area/Bicubic 高保真色彩下采样、逐行裁剪）
    │   └── demuxer.rs        # FFmpeg 容器解复用与音视频流分离
    ├── audio/                # 音频播放管道
    │   ├── decoder.rs        # FFmpeg 音频解码与 swresample 立体声 F32 重采样
    │   └── player.rs         # cpal 音频硬件管道（自动跟随 PipeWire/PulseAudio 默认输出）
    ├── render/               # 渲染后端
    │   ├── mod.rs            # RenderEngine 统一调度与 Layout 结构
    │   ├── kitty.rs          # Kitty 图形协议实现（零堆内存分配 Base64、静默模式）
    │   ├── halfblock.rs      # 24位真彩半块字符渲染器（绝对坐标定位，防滚屏）
    │   └── ascii.rs          # 经典灰度梯度 ASCII 渲染器
    └── tui/                  # 终端生命周期与交互
        ├── event.rs          # 非阻塞按键事件排空与映射
        ├── guard.rs          # 终端模式接管 RAII Guard
        └── osd.rs            # MPV 风格 OSD 状态栏与进度条
```

---

## 三、 里程碑进度与详细规划

### ✅ 第一阶段：基础框架与解复用解码（已完成）
- [x] 工程初始化与 Cargo 依赖配置（FFmpeg 9.1, Crossterm 0.29, cpal 0.18, Clap 4.6）
- [x] FFmpeg 媒体容器解复用与流分离 (`Demuxer`)
- [x] 逐行 Stride 裁剪修复画面斜切错位 Bug
- [x] 首帧 PTS 锚定主时钟，解决瞬间闪退 Bug

### ✅ 第二阶段：终端自适应渲染与 MPV 交互（已完成）
- [x] 物理像素比例推导与 `Contain` 等比缩放算法（自适应保留黑边，100% 杜绝裁切与拉伸变形）
- [x] 关闭自动换行（`\x1b[?7l`）与绝对光标寻址（`\x1b[{row};{col}H`），杜绝终端滚动条错乱
- [x] Kitty / HalfBlock / ASCII 三模热键无缝切换（<kbd>m</kbd>）
- [x] 非阻塞按键事件批量排空，彻底消除按键延迟卡顿

### ✅ 第三阶段：物理音频直连与纯粹高帧率调度（已完成）
- [x] **PipeWire / PulseAudio 默认设备直连**：
  - 自动绑定系统当前选中的默认音频设备（`@DEFAULT_SINK@`），电脑当前选择什么设备就由什么设备发声，绝不硬编码设备名。
- [x] **架构纠偏：彻底剔除人工延迟检测，恢复原生丝滑流畅**：
  - 废弃脆弱的 PulseAudio 延迟估算与阶梯式时钟，杜绝人工干涉；
  - 视频帧完全交由高精度单调 CPU 时钟（`Instant::now()`）连续驱动，彻底消除掉帧与卡顿；
  - 音频与视频独立平稳运行，音量与静音（<kbd>↑</kbd>/<kbd>↓</kbd>/<kbd>x</kbd>）即时生效。

### ✅ 第四阶段：1080P 超高清画质与零闪烁抗频闪（已完成 - v0.2.7）
- [x] **彻底解除 Kitty 模式分辨率限制**：在共享内存加持下，最高支持源视频原生 1080P (1920px) 乃至 4K 原画无损直显。
- [x] **升级下采样滤镜（根除色块断层 Banding）**：引入 `Flags::AREA` 与 `Flags::BICUBIC`，使大幅缩小与缩放时的色彩渐变细腻柔和。
- [x] **零堆内存分配 Base64**：优化 Kitty 协议字符编码吞吐，高分辨率推流零延迟。
- [x] **Kitty 协议双缓冲（Ping-Pong Buffer：a=t + a=p）**：后台静默上传新帧，待完全就绪后原子化瞬时切屏，彻底消除旧帧提前被删导致的 30Hz 黑屏闪烁。
- [x] **POSIX 共享内存（t=s, Zero-Copy Shared Memory）落地**：直接通过 `/dev/shm` 内存映射向 Ghostty 传输原始像素，单帧 PTY 字符吞吐量暴降 50,000 倍（仅数十字节），彻底释放 60 FPS 满帧性能！
- [x] **修复 OSD 测速仪 Bug**：精准捕获并统计 100% 真实上屏帧，实时精准显示 30/60 FPS。

### ✅ 第五阶段：工业级多线程解耦管线与多模式完美重构（已完成 - v0.3.0）
- [x] **MPV 同款多线程解耦管线（`PlaybackPipeline`）**：
  - 彻底解耦 Demuxer、音频解码、视频解码与终端渲染，消除单线程串行卡顿；
  - 启动 6ms 内毫秒级预缓冲音频并瞬时开闸出声，彻底解决数秒启动音频延迟；
  - 12 帧视频有界环形缓冲队列，消除 I 帧解码峰值抖动，主线程零负担稳锁 60 FPS；
  - 音频主时钟（Audio-Master Clock）精准物理对齐。
- [x] **修复半高（Half-Block）与 ASCII 模式播放与切换**：
  - 发送 `\x1b_Ga=d,d=A,q=2\x1b\` 彻底消除 Kitty GPU 图层残留遮挡；
  - 偶数点阵严格对齐，杜绝 YUV420P 色度抽样异常与内存越界；
  - 模式热切换时即刻清屏并通知 Worker 线程重置分辨率。

### ⏳ 第六阶段：硬件加速与扩展特性（下一阶段）
- [ ] **VA-API 硬件解码加速**：在视频 Worker 线程引入 Intel Arc A770 硬件解码。
- [ ] **Sixel 协议支持**：为 `foot` 等支持 Sixel 点阵协议的终端编写专用渲染器。
- [ ] **关键帧精确 Seek**：构建基于视频关键帧的快速拖拽与精确定位。

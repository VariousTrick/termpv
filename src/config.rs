// 播放器配置与命令行参数解析模块
use crate::render::filter::ColorFilterPreset;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// 支持的视频渲染后端模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum RenderMode {
    /// 自动根据终端类型挑选最佳模式（如 Ghostty/Kitty 优先 Kitty 协议，Foot 优先 Sixel，其余半块）
    #[default]
    Auto,
    /// Kitty 终端图形协议（针对 Ghostty/Kitty 等极速 GPU 原生画质）
    Kitty,
    /// Sixel 协议点阵渲染（针对 Foot 等轻量终端）
    Sixel,
    /// 24位真彩色半块字符模式（▀，上下双像素，兼容性最高）
    HalfBlock,
    /// 经典 ASCII 字符灰度梯度模式（黑客帝国复古风）
    Ascii,
}

/// termpv 命令行参数定义
#[derive(Debug, Parser, Clone)]
#[command(name = "termpv", version, about = "高性能多模式终端视频播放器", long_about = None)]
pub struct Config {
    /// 视频/音频媒体文件路径（留空则进入交互式 TUI 启动器菜单）
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// 指定起始播放时间（秒，用于断点续播）
    #[arg(long = "start", value_name = "SECONDS")]
    pub start: Option<f64>,

    /// 初始渲染模式 (auto, kitty, sixel, half-block, ascii)
    #[arg(short = 'm', long = "mode", value_enum, default_value_t = RenderMode::Auto)]
    pub render_mode: RenderMode,

    /// 初始色彩风格滤镜 (original, skin-glow, sin-city-red, cyber-blue, noir-bw, teal-orange, custom-hue)
    #[arg(short = 'f', long = "filter", value_enum, default_value_t = ColorFilterPreset::Original)]
    pub color_filter: ColorFilterPreset,

    /// 初始音量 (0.0 到 2.0，默认为 1.0)
    #[arg(long = "volume", default_value_t = 1.0)]
    pub volume: f32,

    /// 静音启动
    #[arg(long = "mute", default_value_t = false)]
    pub mute: bool,

    /// 循环播放
    #[arg(short = 'l', long = "loop", default_value_t = false)]
    pub loop_playback: bool,

    /// 禁用音频输出
    #[arg(long = "no-audio", default_value_t = false)]
    pub no_audio: bool,

    /// 强制禁用硬件加速（默认尝试启用 VA-API 等）
    #[arg(long = "no-hwaccel", default_value_t = false)]
    pub no_hwaccel: bool,

    /// 启用调试日志（默认输出到 termpv.log）
    #[arg(short = 'd', long = "debug", default_value_t = false)]
    pub debug: bool,

    /// 自定义日志文件输出路径
    #[arg(long = "log", value_name = "LOG_FILE")]
    pub log_file: Option<PathBuf>,
}

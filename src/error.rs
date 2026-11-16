// 播放器错误类型统一定义模块
use std::path::PathBuf;
use thiserror::Error;

/// termpv 统一错误枚举
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum TermpvError {
    #[error("文件未找到或无法打开: {0}")]
    FileNotFound(PathBuf),

    #[error("媒体容器格式解析失败: {0}")]
    DemuxFailed(String),

    #[error("找不到可用的流: {0}")]
    NoStreamFound(String),

    #[error("视频解码错误: {0}")]
    VideoDecodeError(String),

    #[error("音频设备初始化或播放错误: {0}")]
    AudioError(String),

    #[error("渲染失败: {0}")]
    RenderError(String),

    #[error("终端控制或I/O错误: {0}")]
    TerminalError(#[from] std::io::Error),

    #[error("FFmpeg 底层错误: {0}")]
    FfmpegError(String),
}

/// 全局便利别名
pub type Result<T> = std::result::Result<T, TermpvError>;

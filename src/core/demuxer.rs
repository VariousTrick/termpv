// 媒体解封装与流分离模块
use crate::error::{Result, TermpvError};
use ffmpeg_next::format::context::Input;
use ffmpeg_next::media::Type;
use ffmpeg_next::Packet;
use std::path::Path;

/// 流类型标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Other,
}

/// 媒体解封装管理器
pub struct Demuxer {
    input_ctx: Input,
    video_stream_index: Option<usize>,
    audio_stream_index: Option<usize>,
    duration_secs: f64,
}

impl Demuxer {
    /// 打开媒体文件并解析流元信息（支持本地文件与 HTTP/HTTPS 网络流）
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let path_str = path_ref.to_string_lossy();
        let is_network = path_str.starts_with("http://")
            || path_str.starts_with("https://")
            || path_str.starts_with("rtsp://")
            || path_str.starts_with("rtmp://");

        let input_ctx = if is_network {
            let mut opts = ffmpeg_next::Dictionary::new();
            // 注入通用浏览器 User-Agent，避免 PikPak/网盘 CDN 阻断请求
            opts.set(
                "user_agent",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            );
            opts.set("probesize", "1048576"); // 1MB 探测限制，大幅加速网络流嗅探建连
            opts.set("analyzeduration", "1000000"); // 1s 分析窗口
            opts.set("reconnect", "1"); // 网络抖动时自动重连
            opts.set("reconnect_streamed", "1");
            opts.set("reconnect_delay_max", "5");
            ffmpeg_next::format::input_with_dictionary(&path_ref, opts)
        } else {
            ffmpeg_next::format::input(&path_ref)
        }
        .map_err(|e| TermpvError::DemuxFailed(format!("无法打开媒体文件/网络流: {e}")))?;

        let video_stream_index = input_ctx.streams().best(Type::Video).map(|s| s.index());
        let audio_stream_index = input_ctx.streams().best(Type::Audio).map(|s| s.index());

        if video_stream_index.is_none() {
            return Err(TermpvError::NoStreamFound("未检测到有效视频流".into()));
        }

        // 获取视频时长（优先使用全局时长，单位微秒）
        let duration_secs = if input_ctx.duration() > 0 {
            (input_ctx.duration() as f64) / (ffmpeg_next::ffi::AV_TIME_BASE as f64)
        } else {
            0.0
        };

        Ok(Self {
            input_ctx,
            video_stream_index,
            audio_stream_index,
            duration_secs,
        })
    }

    /// 获取视频总时长（秒）
    pub fn duration(&self) -> f64 {
        self.duration_secs
    }

    /// 获取最佳视频流引用
    pub fn video_stream(&self) -> Result<ffmpeg_next::Stream<'_>> {
        let idx = self
            .video_stream_index
            .ok_or_else(|| TermpvError::NoStreamFound("无视频流".into()))?;
        self.input_ctx
            .stream(idx)
            .ok_or_else(|| TermpvError::NoStreamFound("无法获取视频流引用".into()))
    }

    /// 获取最佳音频流引用（若存在）
    pub fn audio_stream(&self) -> Option<ffmpeg_next::Stream<'_>> {
        let idx = self.audio_stream_index?;
        self.input_ctx.stream(idx)
    }

    /// 获取视频流时间基准（用于计算数据包绝对 PTS）
    #[allow(dead_code)]
    pub fn video_time_base(&self) -> f64 {
        self.video_stream().map(|s| f64::from(s.time_base())).unwrap_or(0.0)
    }

    /// 获取音频流时间基准（用于计算数据包绝对 PTS）
    pub fn audio_time_base(&self) -> f64 {
        self.audio_stream().map(|s| f64::from(s.time_base())).unwrap_or(0.0)
    }

    /// 读取下一个数据包并标识其所属流类型
    pub fn read_packet(&mut self) -> Option<(StreamKind, Packet)> {
        let mut packet = Packet::empty();
        if packet.read(&mut self.input_ctx).is_err() {
            return None;
        }

        let stream_idx = packet.stream();
        let kind = if Some(stream_idx) == self.video_stream_index {
            StreamKind::Video
        } else if Some(stream_idx) == self.audio_stream_index {
            StreamKind::Audio
        } else {
            StreamKind::Other
        };

        Some((kind, packet))
    }

    /// 跳转至指定播放时间点（秒）
    pub fn seek(&mut self, target_secs: f64) -> Result<()> {
        let target_ts = (target_secs * (ffmpeg_next::ffi::AV_TIME_BASE as f64)) as i64;
        self.input_ctx
            .seek(target_ts, ..target_ts)
            .map_err(|e| TermpvError::DemuxFailed(format!("跳转进度失败: {e}")))?;
        Ok(())
    }
}



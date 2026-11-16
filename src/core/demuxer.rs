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
    /// 打开媒体文件并解析流元信息
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let input_ctx = ffmpeg_next::format::input(&path)
            .map_err(|e| TermpvError::DemuxFailed(format!("无法打开媒体文件: {e}")))?;

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

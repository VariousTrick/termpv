// FFmpeg 视频流解码与 libswscale 色彩空间缩放模块
use crate::error::{Result, TermpvError};
use crate::render::buffer::RgbFrame;
use ffmpeg_next::codec::context::Context;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling::{context::Context as ScalerContext, flag::Flags};
use ffmpeg_next::util::frame::video::Video as VideoFrame;
use ffmpeg_next::Packet;

/// 视频解码器封装
pub struct VideoDecoder {
    decoder: ffmpeg_next::decoder::Video,
    scaler: Option<ScalerContext>,
    scaler_target_w: u32,
    scaler_target_h: u32,
    time_base: f64,
    source_w: u32,
    source_h: u32,
}

// SAFETY: VideoDecoder 独占底层的解码器句柄与 SwsContext，在跨线程转移后由对应线程独占操作
unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    /// 根据视频流参数初始化解码器
    pub fn new(stream: &ffmpeg_next::Stream) -> Result<Self> {
        let codec_ctx = Context::from_parameters(stream.parameters())
            .map_err(|e| TermpvError::VideoDecodeError(format!("创建解码上下文失败: {e}")))?;

        let decoder = codec_ctx
            .decoder()
            .video()
            .map_err(|e| TermpvError::VideoDecodeError(format!("打开视频解码器失败: {e}")))?;

        let time_base = f64::from(stream.time_base());
        let source_w = decoder.width();
        let source_h = decoder.height();

        Ok(Self {
            decoder,
            scaler: None,
            scaler_target_w: 0,
            scaler_target_h: 0,
            time_base,
            source_w,
            source_h,
        })
    }

    pub fn source_width(&self) -> u32 {
        self.source_w
    }

    pub fn source_height(&self) -> u32 {
        self.source_h
    }

    pub fn time_base(&self) -> f64 {
        self.time_base
    }

    /// 向解码器投递媒体数据包并提取解码后的 RGB 帧
    pub fn decode_packet(
        &mut self,
        packet: &Packet,
        target_w: u32,
        target_h: u32,
    ) -> Result<Vec<RgbFrame>> {
        self.decoder
            .send_packet(packet)
            .map_err(|e| TermpvError::VideoDecodeError(format!("发送数据包失败: {e}")))?;

        let mut frames = Vec::new();
        let mut decoded = VideoFrame::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            if let Some(rgb_frame) = self.convert_to_rgb(&decoded, target_w, target_h)? {
                frames.push(rgb_frame);
            }
        }

        Ok(frames)
    }

    /// 刷新解码器状态（在 Seek 时重置解码器缓冲，杜绝 send_eof 毒化导致解码器无法继续解码）
    pub fn flush(&mut self) {
        self.decoder.flush();
    }

    /// 使用 FFmpeg 内置 libswscale 将原生格式（如 YUV420P）高速转换为 RGB24 并缩放到目标大小
    fn convert_to_rgb(
        &mut self,
        frame: &VideoFrame,
        target_w: u32,
        target_h: u32,
    ) -> Result<Option<RgbFrame>> {
        if target_w == 0 || target_h == 0 {
            return Ok(None);
        }

        // 仅在目标尺寸变更时重新分配 Scaler，避免高频内存申请
        if self.scaler.is_none()
            || self.scaler_target_w != target_w
            || self.scaler_target_h != target_h
        {
            let flags = if target_w < frame.width() / 2 {
                Flags::AREA | Flags::ACCURATE_RND
            } else {
                Flags::BICUBIC | Flags::ACCURATE_RND
            };

            let scaler = ScalerContext::get(
                frame.format(),
                frame.width(),
                frame.height(),
                Pixel::RGB24,
                target_w,
                target_h,
                flags,
            )
            .map_err(|e| TermpvError::VideoDecodeError(format!("初始化色彩转换器失败: {e}")))?;

            self.scaler = Some(scaler);
            self.scaler_target_w = target_w;
            self.scaler_target_h = target_h;
        }

        let scaler = self.scaler.as_mut().unwrap();
        let mut rgb_video_frame = VideoFrame::empty();

        scaler
            .run(frame, &mut rgb_video_frame)
            .map_err(|e| TermpvError::VideoDecodeError(format!("色彩空间转换失败: {e}")))?;

        let pts_secs = frame
            .pts()
            .map(|pts| (pts as f64) * self.time_base)
            .unwrap_or(0.0);

        // 去除 FFmpeg 内存对齐的 Stride 填充字节，将各行连续打包
        let stride = rgb_video_frame.stride(0);
        let row_bytes = (target_w as usize) * 3;
        let plane = rgb_video_frame.data(0);

        let mut data = Vec::with_capacity(row_bytes * (target_h as usize));
        for y in 0..(target_h as usize) {
            let start = y * stride;
            let end = start + row_bytes;
            if end <= plane.len() {
                data.extend_from_slice(&plane[start..end]);
            }
        }

        Ok(Some(RgbFrame {
            width: target_w,
            height: target_h,
            data,
            pts: pts_secs,
        }))
    }
}

// FFmpeg 音频流解码与重采样模块
use crate::error::{Result, TermpvError};
use ffmpeg_next::channel_layout::ChannelLayout;
use ffmpeg_next::codec::context::Context;
use ffmpeg_next::format::Sample;
use ffmpeg_next::software::resampling::Context as Resampler;
use ffmpeg_next::util::frame::Audio as AudioFrame;
use ffmpeg_next::Packet;

/// 音频解码与重采样器
pub struct AudioDecoder {
    decoder: ffmpeg_next::decoder::Audio,
    resampler: Option<Resampler>,
    target_sample_rate: u32,
    target_channels: u16,
}

// SAFETY: AudioDecoder 独占底层的解码器句柄与 SwrContext
unsafe impl Send for AudioDecoder {}

impl AudioDecoder {
    /// 初始化音频解码器并设定目标播放采样率（如 48000Hz 立体声）
    pub fn new(stream: &ffmpeg_next::Stream, target_sample_rate: u32, target_channels: u16) -> Result<Self> {
        let codec_ctx = Context::from_parameters(stream.parameters())
            .map_err(|e| TermpvError::AudioError(format!("创建音频解码上下文失败: {e}")))?;

        let decoder = codec_ctx
            .decoder()
            .audio()
            .map_err(|e| TermpvError::AudioError(format!("打开音频解码器失败: {e}")))?;

        Ok(Self {
            decoder,
            resampler: None,
            target_sample_rate,
            target_channels,
        })
    }

    /// 解码数据包并重采样为标准立体声 F32 PCM 浮点采样数组
    pub fn decode_packet(&mut self, packet: &Packet) -> Result<Vec<f32>> {
        self.decoder
            .send_packet(packet)
            .map_err(|e| TermpvError::AudioError(format!("发送音频数据包失败: {e}")))?;

        let mut samples = Vec::new();
        let mut decoded = AudioFrame::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let resampled = self.resample_frame(&decoded)?;
            samples.extend(resampled);
        }

        Ok(samples)
    }

    /// 刷新残留音频帧
    #[allow(dead_code)]
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        self.decoder
            .send_eof()
            .map_err(|e| TermpvError::AudioError(format!("发送音频 EOF 失败: {e}")))?;

        let mut samples = Vec::new();
        let mut decoded = AudioFrame::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let resampled = self.resample_frame(&decoded)?;
            samples.extend(resampled);
        }

        Ok(samples)
    }

    /// 将任意格式（如 AAC/MP3 各种采样率）重采样为目标格式
    fn resample_frame(&mut self, frame: &AudioFrame) -> Result<Vec<f32>> {
        if self.resampler.is_none() {
            let out_layout = if self.target_channels == 1 {
                ChannelLayout::MONO
            } else {
                ChannelLayout::STEREO
            };

            let resampler = Resampler::get(
                frame.format(),
                frame.channel_layout(),
                frame.rate(),
                Sample::F32(ffmpeg_next::format::sample::Type::Packed),
                out_layout,
                self.target_sample_rate,
            )
            .map_err(|e| TermpvError::AudioError(format!("初始化音频重采样器失败: {e}")))?;

            self.resampler = Some(resampler);
        }

        let resampler = self.resampler.as_mut().unwrap();
        let mut out_frame = AudioFrame::empty();

        resampler
            .run(frame, &mut out_frame)
            .map_err(|e| TermpvError::AudioError(format!("音频重采样执行失败: {e}")))?;

        let raw_bytes = out_frame.data(0);
        let samples = raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        Ok(samples)
    }
}

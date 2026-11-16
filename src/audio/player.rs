// 基于 cpal 的音频设备输出与播放模块（自动绑定系统默认输出设备，支持绝对音频主时钟）
use crate::error::{Result, TermpvError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 音频硬件播放控制器
pub struct AudioPlayer {
    stream: Stream,
    device_name: String,
    sample_rate: u32,
    channels: u16,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    played_samples: Arc<AtomicU64>,
    base_pts_us: Arc<AtomicU64>,
    volume_bits: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
    last_callback: Arc<Mutex<Option<(std::time::Instant, u64)>>>,
}

impl AudioPlayer {
    /// 初始化音频输出设备（优先选用 PulseAudio/PipeWire 确保系统当前默认设备正常发声）
    pub fn new() -> Result<Self> {
        let host = if cpal::available_hosts().contains(&cpal::HostId::PulseAudio) {
            cpal::host_from_id(cpal::HostId::PulseAudio).unwrap_or_else(|_| cpal::default_host())
        } else {
            cpal::default_host()
        };

        log::info!("正在通过音频宿主 [{:?}] 获取系统默认音频输出设备...", host.id());

        let device = host
            .default_output_device()
            .ok_or_else(|| TermpvError::AudioError("未检测到可用的系统默认音频输出设备".into()))?;

        let device_name = device.to_string();
        log::info!("成功绑定音频输出设备: [{}]", device_name);

        let supported_config = device
            .default_output_config()
            .map_err(|e| TermpvError::AudioError(format!("获取音频配置失败: {e}")))?;

        let sample_rate = supported_config.sample_rate();
        let channels = supported_config.channels();

        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(48000 * 4)));
        let played_samples = Arc::new(AtomicU64::new(0));
        let base_pts_us = Arc::new(AtomicU64::new(0));
        let volume_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let muted = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let last_callback = Arc::new(Mutex::new(None));

        let buf_clone = Arc::clone(&buffer);
        let played_clone = Arc::clone(&played_samples);
        let vol_clone = Arc::clone(&volume_bits);
        let mute_clone = Arc::clone(&muted);
        let paused_clone = Arc::clone(&paused);
        let cb_clone = Arc::clone(&last_callback);

        // 创建音频硬件拉流回调（高优先级异步流填充，不阻塞视频时钟）
        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _| {
                    Self::audio_callback(
                        data,
                        &buf_clone,
                        &played_clone,
                        &vol_clone,
                        &mute_clone,
                        &paused_clone,
                        &cb_clone,
                    );
                },
                move |err| {
                    log::warn!("音频输出设备流异常: {err}");
                },
                None,
            )
            .map_err(|e| TermpvError::AudioError(format!("构建音频输出流失败: {e}")))?;

        // 对齐 MPV：初始保持流封堵（corked / paused），等到解码器预卷填充了首批有效数据后再播放，杜绝底层灌入静音导致延时出声
        Ok(Self {
            stream,
            device_name,
            sample_rate,
            channels,
            buffer,
            played_samples,
            base_pts_us,
            volume_bits,
            muted,
            paused,
            started,
            last_callback,
        })
    }

    /// 核心声卡数据填充回调（运行于高优先级音频线程）
    fn audio_callback(
        data: &mut [f32],
        buffer: &Arc<Mutex<VecDeque<f32>>>,
        played_samples: &Arc<AtomicU64>,
        vol_bits: &Arc<AtomicU32>,
        muted: &Arc<AtomicBool>,
        paused: &Arc<AtomicBool>,
        last_callback: &Arc<Mutex<Option<(std::time::Instant, u64)>>>,
    ) {
        if paused.load(Ordering::Relaxed) {
            data.fill(0.0);
            return;
        }

        let is_muted = muted.load(Ordering::Relaxed);
        let vol = f32::from_bits(vol_bits.load(Ordering::Relaxed));
        let gain = if is_muted { 0.0 } else { vol };

        let mut guard = match buffer.lock() {
            Ok(g) => g,
            Err(_) => {
                data.fill(0.0);
                return;
            }
        };

        let mut actual_played = 0u64;
        for sample in data.iter_mut() {
            if let Some(s) = guard.pop_front() {
                *sample = s * gain;
                actual_played += 1;
            } else {
                *sample = 0.0;
            }
        }

        let total_played = played_samples.fetch_add(actual_played, Ordering::Relaxed) + actual_played;
        if actual_played > 0 {
            if let Ok(mut cb) = last_callback.lock() {
                *cb = Some((std::time::Instant::now(), total_played));
            }
        }
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// 启动声卡底层拉流播放（对齐 MPV 的 Uncork）
    pub fn start(&self) -> Result<()> {
        if !self.started.swap(true, Ordering::SeqCst) {
            self.stream
                .play()
                .map_err(|e| TermpvError::AudioError(format!("启动音频播放失败: {e}")))?;
            log::info!("音频硬件流已正式触发播放 (Uncork)");
        }
        Ok(())
    }

    /// 向环形缓冲追加待播放的 PCM 采样（达到预卷水位时自适应激活出声）
    pub fn push_samples(&self, samples: &[f32]) {
        let Ok(mut guard) = self.buffer.lock() else {
            return;
        };
        guard.extend(samples);
        let count = guard.len();
        drop(guard);

        // 当缓冲了至少 40ms 的音频数据（例如 48000 * 2 * 0.040 = 3840 采样）时，立即启动声卡硬件流
        let min_start_samples = (self.sample_rate as usize * self.channels as usize * 40) / 1000;
        if count >= min_start_samples && !self.started.load(Ordering::Relaxed) {
            let _ = self.start();
        }
    }

    /// 检查声卡拉流是否已经真正激活出声
    pub fn is_active(&self) -> bool {
        self.started.load(Ordering::Relaxed)
            && (self.played_samples.load(Ordering::Relaxed) > 0
                || self.base_pts_us.load(Ordering::Relaxed) > 0)
    }

    /// 获取声卡硬件当前物理播放进度（秒，带回调间高精度单调时钟平滑插值，消除阶梯跳变）
    pub fn current_pts(&self) -> f64 {
        let played = self.played_samples.load(Ordering::Relaxed);
        let base_us = self.base_pts_us.load(Ordering::Relaxed);
        let rate_chan = self.sample_rate as f64 * self.channels as f64;
        let secs_played = (played as f64) / rate_chan;

        // 平滑插值：利用单调时钟平滑两次声卡回调之间的时间间隔，消除阶梯抖动
        let interp = if let Ok(guard) = self.last_callback.lock() {
            if let Some((inst, samples_at)) = *guard {
                if played >= samples_at {
                    let elapsed = inst.elapsed().as_secs_f64();
                    // 最多插值到下一次回调周期（约 35ms），防止过度外推
                    elapsed.min(0.035)
                } else {
                    0.0
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        (base_us as f64 / 1_000_000.0) + secs_played + interp
    }

    /// 获取当前环形缓冲已堆积的音频时长（秒，用于流控）
    #[allow(dead_code)]
    pub fn buffered_duration(&self) -> f64 {
        let len = self.buffer.lock().map(|b| b.len()).unwrap_or(0);
        (len as f64) / (self.sample_rate as f64 * self.channels as f64)
    }

    /// 执行 Seek 时重置音频缓冲并更新基准时间戳
    pub fn seek_to(&self, target_secs: f64) {
        if let Ok(mut guard) = self.buffer.lock() {
            guard.clear();
        }
        self.played_samples.store(0, Ordering::Relaxed);
        let us = (target_secs.max(0.0) * 1_000_000.0) as u64;
        self.base_pts_us.store(us, Ordering::Relaxed);
        if let Ok(mut cb) = self.last_callback.lock() {
            *cb = None;
        }
    }

    /// 设置播放音量 (0.0 ~ 2.0)
    pub fn set_volume(&self, volume: f32) {
        self.volume_bits.store(volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    /// 设置静音状态
    pub fn set_mute(&self, is_muted: bool) {
        self.muted.store(is_muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// 设置暂停状态
    pub fn set_paused(&self, is_paused: bool) {
        self.paused.store(is_paused, Ordering::Relaxed);
        if is_paused {
            let _ = self.stream.pause();
        } else if self.started.load(Ordering::Relaxed) {
            let _ = self.stream.play();
        }
    }
}

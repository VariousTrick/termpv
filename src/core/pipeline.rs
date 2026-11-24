// 多线程高并发媒体解复用与音视频解码管线（对齐 MPV 工业级多线程解耦架构）
use crate::audio::{AudioDecoder, AudioPlayer};
use crate::config::Config;
use crate::core::decoder::VideoDecoder;
use crate::core::demuxer::{Demuxer, StreamKind};
use crate::error::{Result, TermpvError};
use crate::render::buffer::RgbFrame;
use ffmpeg_next::Packet;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 管线内部控制指令
#[derive(Debug, Clone, Copy)]
enum PipelineCmd {
    Seek(f64, u64),
    SetResolution(u32, u32),
    Stop,
}

/// 工业级多线程音视频处理管线
pub struct PlaybackPipeline {
    duration_secs: f64,
    source_w: u32,
    source_h: u32,
    audio_player: Option<Arc<AudioPlayer>>,

    // 线程安全队列与状态
    frame_queue: Arc<(Mutex<VecDeque<(u64, RgbFrame)>>, Condvar)>,
    target_res: Arc<Mutex<(u32, u32)>>,
    is_eof: Arc<AtomicBool>,
    running: Arc<AtomicBool>,

    // 环形缓冲统计（用于向 OSD 反馈已缓存时长与字节大小）
    latest_demux_pts: Arc<AtomicU64>,
    buffered_bytes: Arc<AtomicUsize>,

    // 线程控制信道
    demux_cmd_tx: Sender<PipelineCmd>,
    video_cmd_tx: Sender<PipelineCmd>,

    // 线程句柄
    demux_handle: Option<JoinHandle<()>>,
    video_handle: Option<JoinHandle<()>>,
}

impl PlaybackPipeline {
    /// 初始化并启动多线程播放管线（解封装线程 + 视频解码线程 + 独立声卡输出）
    pub fn new<P: AsRef<Path>>(path: P, config: &Config) -> Result<Self> {
        let demuxer = Demuxer::open(&path)?;
        let duration_secs = demuxer.duration();

        // 1. 初始化音频硬件与解码器（如未禁用音频且存在音频流）
        let (audio_player, audio_decoder) = if !config.no_audio {
            if let Some(audio_stream) = demuxer.audio_stream() {
                match AudioPlayer::new() {
                    Ok(player) => {
                        player.set_volume(config.volume);
                        player.set_mute(config.mute);

                        match AudioDecoder::new(&audio_stream, player.sample_rate(), player.channels()) {
                            Ok(decoder) => {
                                log::info!(
                                    "多线程音频管线就绪: 设备=[{}] 采样率={}Hz 声道={}",
                                    player.device_name(),
                                    player.sample_rate(),
                                    player.channels()
                                );
                                (Some(Arc::new(player)), Some(decoder))
                            }
                            Err(e) => {
                                log::warn!("初始化音频解码器失败 ({e})，静音运行");
                                (None, None)
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("初始化音频输出设备失败 ({e})，静音运行");
                        (None, None)
                    }
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // 2. 初始化视频解码器
        let video_stream = demuxer.video_stream()?;
        let video_decoder = VideoDecoder::new(&video_stream)?;
        let source_w = video_decoder.source_width();
        let source_h = video_decoder.source_height();

        // 3. 构建线程间通信通道与共享环形缓冲
        let path_str = path.as_ref().to_string_lossy();
        let is_network = path_str.starts_with("http://")
            || path_str.starts_with("https://")
            || path_str.starts_with("rtsp://")
            || path_str.starts_with("rtmp://");

        // 视频数据包通道（网络流使用 15000 包/约 500MB 预读超大缓冲，本地流保持 30 包低内存占用）
        let packet_cap = if is_network { 15000 } else { 30 };
        let (video_packet_tx, video_packet_rx): (
            SyncSender<(u64, Option<Packet>)>,
            Receiver<(u64, Option<Packet>)>,
        ) = mpsc::sync_channel(packet_cap);

        let (demux_cmd_tx, demux_cmd_rx) = mpsc::channel();
        let (video_cmd_tx, video_cmd_rx) = mpsc::channel();

        // 解码完成的 RGB 帧环形缓冲队列（存储 (generation, RgbFrame)，最大 16~24 帧，防突发卡顿）
        let frame_cap = if is_network { 24 } else { 16 };
        let frame_queue = Arc::new((
            Mutex::new(VecDeque::<(u64, RgbFrame)>::with_capacity(frame_cap)),
            Condvar::new(),
        ));
        let target_res = Arc::new(Mutex::new((source_w, source_h)));
        let is_eof = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));

        let latest_demux_pts = Arc::new(AtomicU64::new(0));
        let buffered_bytes = Arc::new(AtomicUsize::new(0));

        // 4. 派生【线程 1: 后台极速解复用与音频解码 Worker】
        let demux_handle = {
            let running = Arc::clone(&running);
            let is_eof = Arc::clone(&is_eof);
            let latest_demux_pts = Arc::clone(&latest_demux_pts);
            let buffered_bytes = Arc::clone(&buffered_bytes);
            let audio_player = audio_player.clone();
            let loop_playback = config.loop_playback;

            thread::Builder::new()
                .name("termpv-demux".into())
                .spawn(move || {
                    Self::demux_worker_loop(
                        demuxer,
                        audio_decoder,
                        audio_player,
                        video_packet_tx,
                        demux_cmd_rx,
                        running,
                        is_eof,
                        latest_demux_pts,
                        buffered_bytes,
                        is_network,
                        loop_playback,
                    );
                })
                .map_err(|e| TermpvError::DemuxFailed(format!("派生解封装线程失败: {e}")))?
        };

        // 5. 派生【线程 2: 后台视频解码与色彩空间缩放 Worker】
        let video_handle = {
            let running = Arc::clone(&running);
            let buffered_bytes = Arc::clone(&buffered_bytes);
            let frame_queue = Arc::clone(&frame_queue);
            let target_res = Arc::clone(&target_res);

            thread::Builder::new()
                .name("termpv-vdecode".into())
                .spawn(move || {
                    Self::video_worker_loop(
                        video_decoder,
                        video_packet_rx,
                        video_cmd_rx,
                        frame_queue,
                        target_res,
                        buffered_bytes,
                        running,
                    );
                })
                .map_err(|e| TermpvError::VideoDecodeError(format!("派生视频解码线程失败: {e}")))?
        };

        Ok(Self {
            duration_secs,
            source_w,
            source_h,
            audio_player,
            frame_queue,
            target_res,
            is_eof,
            running,
            latest_demux_pts,
            buffered_bytes,
            demux_cmd_tx,
            video_cmd_tx,
            demux_handle: Some(demux_handle),
            video_handle: Some(video_handle),
        })
    }

    /// 解复用与音频解码工作循环（高优先级生产者）
    #[allow(clippy::too_many_arguments)]
    fn demux_worker_loop(
        mut demuxer: Demuxer,
        mut audio_decoder: Option<AudioDecoder>,
        audio_player: Option<Arc<AudioPlayer>>,
        video_packet_tx: SyncSender<(u64, Option<Packet>)>,
        cmd_rx: Receiver<PipelineCmd>,
        running: Arc<AtomicBool>,
        is_eof: Arc<AtomicBool>,
        latest_demux_pts: Arc<AtomicU64>,
        buffered_bytes: Arc<AtomicUsize>,
        is_network: bool,
        loop_playback: bool,
    ) {
        let audio_time_base = demuxer.audio_time_base();
        let video_time_base = demuxer.video_time_base();
        let max_buffer_bytes = if is_network {
            500 * 1024 * 1024 // 500 MB 内存缓冲上限
        } else {
            30 * 1024 * 1024
        };
        let mut current_gen = 0u64;
        let mut pending_first_audio = false;

        while running.load(Ordering::Relaxed) {
            // 1. 处理控制指令（如 Seek、Stop）
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    PipelineCmd::Seek(pts, generation) => {
                        let _ = demuxer.seek(pts);
                        is_eof.store(false, Ordering::SeqCst);
                        if let Some(audio_dec) = &mut audio_decoder {
                            audio_dec.flush();
                        }
                        current_gen = generation;
                        pending_first_audio = true;
                        buffered_bytes.store(0, Ordering::Relaxed);
                        latest_demux_pts.store(pts.to_bits(), Ordering::Relaxed);
                    }
                    PipelineCmd::Stop => {
                        running.store(false, Ordering::Relaxed);
                        return;
                    }
                    PipelineCmd::SetResolution(_, _) => {}
                }
            }

            // 2. 检查 500MB 内存背压（达到上限时休眠让出 CPU，防内存无节制膨胀）
            if buffered_bytes.load(Ordering::Relaxed) >= max_buffer_bytes {
                thread::sleep(Duration::from_millis(15));
                continue;
            }

            // 3. 读取下一媒体包（视频队列满时通过 SyncSender 自然反压，避免单向饥饿）
            match demuxer.read_packet() {
                Some((StreamKind::Audio, packet)) => {
                    // 音频数据包即刻解码为 PCM，推入声卡水库，实现零延迟出声
                    if let (Some(audio_dec), Some(player)) = (&mut audio_decoder, &audio_player) {
                        if let Ok(samples) = audio_dec.decode_packet(&packet) {
                            if pending_first_audio {
                                let pts = packet
                                    .pts()
                                    .map(|p| (p as f64) * audio_time_base)
                                    .unwrap_or(0.0);
                                player.seek_to(pts);
                                pending_first_audio = false;
                            }
                            player.push_samples(&samples);
                        }
                    }
                }
                Some((StreamKind::Video, packet)) => {
                    let size = packet.size();
                    if let Some(p) = packet.pts() {
                        let pts_secs = (p as f64) * video_time_base;
                        latest_demux_pts.store(pts_secs.to_bits(), Ordering::Relaxed);
                    }
                    buffered_bytes.fetch_add(size, Ordering::Relaxed);
                    // 附带当前代数投递给视频解码线程（若队列满自动阻塞，自然反压）
                    if video_packet_tx.send((current_gen, Some(packet))).is_err() {
                        break;
                    }
                }
                Some((StreamKind::Other, _)) => {}
                None => {
                    // EOF 处理
                    if loop_playback {
                        let _ = demuxer.seek(0.0);
                        if let Some(player) = &audio_player {
                            player.seek_to(0.0);
                        }
                        pending_first_audio = true;
                        continue;
                    }

                    is_eof.store(true, Ordering::Relaxed);
                    let _ = video_packet_tx.send((current_gen, None));
                    thread::sleep(Duration::from_millis(15));
                }
            }
        }
    }

    /// 视频解码与色彩转换工作循环（高优先级并行解码器）
    fn video_worker_loop(
        mut decoder: VideoDecoder,
        packet_rx: Receiver<(u64, Option<Packet>)>,
        cmd_rx: Receiver<PipelineCmd>,
        frame_queue: Arc<(Mutex<VecDeque<(u64, RgbFrame)>>, Condvar)>,
        target_res: Arc<Mutex<(u32, u32)>>,
        buffered_bytes: Arc<AtomicUsize>,
        running: Arc<AtomicBool>,
    ) {
        let mut current_gen = 0u64;

        while running.load(Ordering::Relaxed) {
            // 1. 处理控制指令
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    PipelineCmd::Seek(_, generation) => {
                        current_gen = generation;
                        decoder.flush();
                        // 彻底排空 packet_rx 中可能积压的旧代数据包或 EOF 哨兵，防止解码脏包
                        while let Ok(_) = packet_rx.try_recv() {}
                        let (lock, cvar) = &*frame_queue;
                        let mut q = lock.lock().unwrap();
                        q.clear();
                        cvar.notify_all();
                    }
                    PipelineCmd::SetResolution(w, h) => {
                        let (lock, cvar) = &*frame_queue;
                        let mut q = lock.lock().unwrap();
                        // 分辨率变更时清空旧尺寸残留帧
                        q.clear();
                        cvar.notify_all();
                        let _ = (w, h);
                    }
                    PipelineCmd::Stop => {
                        running.store(false, Ordering::Relaxed);
                        return;
                    }
                }
            }

            // 2. 获取目标尺寸
            let (target_w, target_h) = *target_res.lock().unwrap();
            if target_w == 0 || target_h == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }

            // 3. 接收媒体包并解码
            let (pkt_gen, packet_opt) = match packet_rx.recv_timeout(Duration::from_millis(15)) {
                Ok(item) => item,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            if let Some(packet) = &packet_opt {
                let size = packet.size();
                let _ = buffered_bytes.try_update(Ordering::Relaxed, Ordering::Relaxed, |b| {
                    Some(b.saturating_sub(size))
                });
            }

            // 若接收到的数据包早于当前 Seek 代数，立即丢弃，零 CPU 解码浪费！
            if pkt_gen < current_gen {
                continue;
            }

            let frames = match packet_opt {
                Some(packet) => decoder
                    .decode_packet(&packet, target_w, target_h)
                    .unwrap_or_default(),
                None => {
                    decoder.flush();
                    Vec::new()
                }
            };

            // 4. 将成品帧推入有界环形缓冲队列（最多 12 帧，防止显存无度膨胀）
            for frame in frames {
                let (lock, cvar) = &*frame_queue;
                let mut q = lock.lock().unwrap();
                while q.len() >= 12 && running.load(Ordering::Relaxed) {
                    match cvar.wait_timeout(q, Duration::from_millis(15)) {
                        Ok((guard, _)) => {
                            q = guard;
                        }
                        Err(poisoned) => {
                            q = poisoned.into_inner().0;
                            break;
                        }
                    }
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                }
                q.push_back((pkt_gen, frame));
                cvar.notify_all();
            }
        }
    }

    /// 从预解码环形队列弹出一帧（耗时 0ms，主线程零负担）
    #[allow(dead_code)]
    pub fn pop_video_frame(&self) -> Option<(u64, RgbFrame)> {
        let (lock, cvar) = &*self.frame_queue;
        let mut q = lock.lock().unwrap();
        let frame = q.pop_front();
        if frame.is_some() {
            cvar.notify_all();
        }
        frame
    }

    /// 从预解码环形队列弹出一帧，若暂时为空则微等待超时
    pub fn pop_video_frame_timeout(&self, timeout: Duration) -> Option<(u64, RgbFrame)> {
        let (lock, cvar) = &*self.frame_queue;
        let mut q = lock.lock().unwrap();
        if q.is_empty() {
            let (guard, _) = cvar.wait_timeout(q, timeout).ok()?;
            q = guard;
        }
        let frame = q.pop_front();
        if frame.is_some() {
            cvar.notify_all();
        }
        frame
    }

    /// 获取声卡硬件当前绝对播放进度（作为系统主时钟，声卡未激活时回退）
    pub fn current_audio_pts(&self) -> Option<f64> {
        self.audio_player.as_ref().and_then(|p| {
            if p.is_active() {
                Some(p.current_pts())
            } else {
                None
            }
        })
    }

    /// 动态变更目标缩放尺寸（例如切换渲染模式时）
    pub fn set_target_resolution(&self, w: u32, h: u32) {
        {
            let mut res = self.target_res.lock().unwrap();
            if *res == (w, h) {
                return;
            }
            *res = (w, h);
        }

        let _ = self.video_cmd_tx.send(PipelineCmd::SetResolution(w, h));
        let (lock, cvar) = &*self.frame_queue;
        let mut q = lock.lock().unwrap();
        q.clear();
        cvar.notify_all();
    }

    /// 执行 Seek 快进/快退（带 Seek 代数标记）
    pub fn seek(&self, target_secs: f64, generation: u64) {
        let target = target_secs.clamp(0.0, self.duration_secs);
        // 主线程发起 Seek 时第一时间原子重置 EOF 状态，杜绝主渲染循环在换流真空期误判退出
        self.is_eof.store(false, Ordering::SeqCst);

        let _ = self.demux_cmd_tx.send(PipelineCmd::Seek(target, generation));
        let _ = self.video_cmd_tx.send(PipelineCmd::Seek(target, generation));

        let (lock, cvar) = &*self.frame_queue;
        let mut q = lock.lock().unwrap();
        q.clear();
        cvar.notify_all();
    }

    /// 获取当前流缓冲统计（缓冲剩余秒数, 缓冲总字节大小）
    pub fn buffer_stats(&self, current_pts: f64) -> (f64, usize) {
        let latest_bits = self.latest_demux_pts.load(Ordering::Relaxed);
        let latest_pts = f64::from_bits(latest_bits);
        let buffered_secs = (latest_pts - current_pts).max(0.0);
        let bytes = self.buffered_bytes.load(Ordering::Relaxed);
        (buffered_secs, bytes)
    }

    /// 暂停/恢复声卡输出
    pub fn set_paused(&self, paused: bool) {
        if let Some(player) = &self.audio_player {
            player.set_paused(paused);
        }
    }

    /// 调节音量 (0.0 ~ 2.0)
    pub fn set_volume(&self, vol: f32) {
        if let Some(player) = &self.audio_player {
            player.set_volume(vol);
        }
    }

    pub fn volume(&self) -> f32 {
        self.audio_player.as_ref().map(|p| p.volume()).unwrap_or(1.0)
    }

    /// 切换静音
    pub fn toggle_mute(&self) {
        if let Some(player) = &self.audio_player {
            let next = !player.is_muted();
            player.set_mute(next);
        }
    }

    pub fn is_muted(&self) -> bool {
        self.audio_player.as_ref().map(|p| p.is_muted()).unwrap_or(false)
    }

    pub fn duration(&self) -> f64 {
        self.duration_secs
    }

    pub fn source_width(&self) -> u32 {
        self.source_w
    }

    pub fn source_height(&self) -> u32 {
        self.source_h
    }

    pub fn is_eof(&self) -> bool {
        let (lock, _) = &*self.frame_queue;
        let q = lock.lock().unwrap();
        self.is_eof.load(Ordering::Relaxed) && q.is_empty()
    }
}

impl Drop for PlaybackPipeline {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.demux_cmd_tx.send(PipelineCmd::Stop);
        let _ = self.video_cmd_tx.send(PipelineCmd::Stop);
        let (_, cvar) = &*self.frame_queue;
        cvar.notify_all();

        if let Some(h) = self.demux_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.video_handle.take() {
            let _ = h.join();
        }
    }
}

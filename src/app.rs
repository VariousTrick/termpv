// 应用程序核心控制器模块（驱动视频解封装、音频输出、TUI 渲染主循环）
use crate::audio::{AudioDecoder, AudioPlayer};
use crate::config::{Config, RenderMode};
use crate::core::clock::{MasterClock, SyncDecision};
use crate::core::decoder::VideoDecoder;
use crate::core::demuxer::{Demuxer, StreamKind};
use crate::error::{Result, TermpvError};
use crate::render::buffer::RgbFrame;
use crate::render::{Layout, RenderEngine};
use crate::tui::{drain_actions, render_osd_bar, OsdState, PlayerAction, TerminalGuard};
use crossterm::terminal;
use std::collections::VecDeque;
use std::io::{stdout, BufWriter, Write};
use std::thread;
use std::time::{Duration, Instant};

/// termpv 应用主控制器
pub struct App {
    config: Config,
    clock: MasterClock,
    render_engine: RenderEngine,
    audio_player: Option<AudioPlayer>,
    audio_decoder: Option<AudioDecoder>,
    osd_state: OsdState,
    running: bool,
}

impl App {
    pub fn new(config: Config) -> Self {
        let osd = OsdState {
            visible: true,
            paused: false,
            current_secs: 0.0,
            total_secs: 0.0,
            volume: config.volume,
            muted: config.mute,
            mode: config.render_mode,
            fps: 0.0,
            dropped_frames: 0,
        };

        Self {
            render_engine: RenderEngine::new(config.render_mode),
            config,
            clock: MasterClock::new(),
            audio_player: None,
            audio_decoder: None,
            osd_state: osd,
            running: true,
        }
    }

    /// 启动应用主生命周期
    pub fn run(&mut self) -> Result<()> {
        if !self.config.file.exists() {
            return Err(TermpvError::FileNotFound(self.config.file.clone()));
        }

        // 初始化 FFmpeg
        ffmpeg_next::init()
            .map_err(|e| TermpvError::FfmpegError(format!("FFmpeg 初始化失败: {e}")))?;

        // 打开媒体解封装器
        let mut demuxer = Demuxer::open(&self.config.file)?;
        self.osd_state.total_secs = demuxer.duration();

        // 1. 初始化视频解码器
        let video_stream = demuxer.video_stream()?;
        let mut video_decoder = VideoDecoder::new(&video_stream)?;
        log::info!(
            "视频就绪: 时长={:.2}s, 分辨率={}x{}",
            demuxer.duration(),
            video_decoder.source_width(),
            video_decoder.source_height()
        );

        // 2. 初始化音频硬件与解码器（如未禁用音频）
        if !self.config.no_audio {
            self.setup_audio(&demuxer);
        }

        // 激活终端接管保护
        let _guard = TerminalGuard::new()?;
        let mut out_writer = BufWriter::with_capacity(256 * 1024, stdout());

        let mut frame_queue: VecDeque<RgbFrame> = VecDeque::with_capacity(16);
        let mut is_eof = false;

        while self.running {
            // 1. 处理用户键盘事件
            let mode_changed = self.handle_input_events(&mut demuxer, &mut frame_queue)?;
            if !self.running {
                break;
            }

            // 模式切换时全屏清屏复位，清除旧模式残影
            if mode_changed {
                write!(out_writer, "\x1b[2J\x1b[1;1H")?;
                out_writer.flush()?;
            }

            // 2. 暂停状态处理
            if self.osd_state.paused {
                thread::sleep(Duration::from_millis(15));
                continue;
            }

            // 3. 动态获取终端尺寸并以 Contain 算法计算自适应居中分辨率
            let (term_w, term_h) = terminal::size()?;
            let video_term_h = term_h.saturating_sub(1).max(1);
            let (layout, target_w, target_h) = self.calculate_contain_layout(
                video_decoder.source_width(),
                video_decoder.source_height(),
                term_w,
                video_term_h,
            );

            // 4. 缓冲池不足时读取数据包进行解码填充
            self.fill_frame_queue(
                &mut demuxer,
                &mut video_decoder,
                &mut frame_queue,
                target_w,
                target_h,
                &mut is_eof,
            )?;

            // 5. 取出待渲染帧
            let Some(frame) = frame_queue.pop_front() else {
                if is_eof {
                    self.running = false;
                }
                thread::sleep(Duration::from_millis(5));
                continue;
            };

            // 6. 音画同步与帧调度（单调 CPU 时钟，保证视频丝滑流畅）
            if !self.schedule_frame_pts(frame.pts, &mut demuxer, &mut frame_queue)? {
                continue;
            }

            // 7. 渲染视频帧与 OSD 状态栏
            self.render_engine
                .render_frame(&frame, &layout, &mut out_writer)?;

            self.render_osd(term_w, term_h, frame.pts, &mut out_writer)?;

            out_writer.flush()?;
        }

        Ok(())
    }

    /// 初始化音频硬件输出与解码管线
    fn setup_audio(&mut self, demuxer: &Demuxer) {
        let Some(audio_stream) = demuxer.audio_stream() else {
            log::info!("媒体文件无音频流");
            return;
        };

        let player = match AudioPlayer::new() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("未能初始化音频输出设备 ({e})，将以静音模式运行");
                return;
            }
        };

        player.set_volume(self.config.volume);
        player.set_mute(self.config.mute);

        match AudioDecoder::new(&audio_stream, player.sample_rate(), player.channels()) {
            Ok(decoder) => {
                log::info!(
                    "音频输出已启动: 设备=[{}] 采样率={}Hz, 声道={}",
                    player.device_name(),
                    player.sample_rate(),
                    player.channels()
                );
                self.audio_decoder = Some(decoder);
                self.audio_player = Some(player);
            }
            Err(e) => {
                log::warn!("初始化音频解码器失败: {e}");
            }
        }
    }

    /// 严格遵循物理字符几何与 Contain 算法计算等比居中尺寸（自适应保留黑边，100% 杜绝裁切失真）
    fn calculate_contain_layout(
        &self,
        src_w: u32,
        src_h: u32,
        term_w: u16,
        video_term_h: u16,
    ) -> (Layout, u32, u32) {
        if src_w == 0 || src_h == 0 || term_w == 0 || video_term_h == 0 {
            let layout = Layout {
                render_cols: term_w,
                render_rows: video_term_h,
                pad_x: 0,
                pad_y: 0,
                term_w,
                term_h: video_term_h,
            };
            return (layout, 160, 90);
        }

        let vid_aspect = (src_w as f64) / (src_h as f64);
        let max_cols = term_w as f64;
        let max_rows = video_term_h as f64;
        let font_aspect = 2.0;

        let term_aspect = max_cols / (max_rows * font_aspect);

        let (render_cols, render_rows) = if term_aspect > vid_aspect {
            let r = max_rows;
            let c = (max_rows * font_aspect * vid_aspect).round();
            (c.clamp(2.0, max_cols) as u16, r as u16)
        } else {
            let c = max_cols;
            let r = (max_cols / (font_aspect * vid_aspect)).round();
            (c as u16, r.clamp(2.0, max_rows) as u16)
        };

        let pad_x = term_w.saturating_sub(render_cols) / 2;
        let pad_y = video_term_h.saturating_sub(render_rows) / 2;

        let layout = Layout {
            render_cols,
            render_rows,
            pad_x,
            pad_y,
            term_w,
            term_h: video_term_h,
        };

        let (target_px_w, target_px_h) = match self.render_engine.current_mode() {
            RenderMode::Kitty => {
                let px_w = (render_cols as u32 * 8).clamp(320, 640);
                let px_h = (((px_w as f64) / vid_aspect).round() as u32).max(2);
                (px_w, px_h)
            }
            RenderMode::HalfBlock => {
                (render_cols as u32, (render_rows as u32) * 2)
            }
            RenderMode::Ascii => {
                (render_cols as u32, render_rows as u32)
            }
            _ => (render_cols as u32, (render_rows as u32) * 2),
        };

        (layout, target_px_w, target_px_h)
    }

    /// 持续拉取解封装数据包，音视频协同解码分发
    fn fill_frame_queue(
        &mut self,
        demuxer: &mut Demuxer,
        decoder: &mut VideoDecoder,
        queue: &mut VecDeque<RgbFrame>,
        target_w: u32,
        target_h: u32,
        is_eof: &mut bool,
    ) -> Result<()> {
        while queue.len() < 8 && !*is_eof {
            let Some((kind, packet)) = demuxer.read_packet() else {
                if self.config.loop_playback {
                    demuxer.seek(0.0)?;
                    self.clock.seek_to(0.0);
                    if let Some(player) = &self.audio_player {
                        player.seek_to(0.0);
                    }
                    continue;
                }
                *is_eof = true;
                let flushed = decoder.flush(target_w, target_h)?;
                queue.extend(flushed);
                break;
            };

            match kind {
                StreamKind::Video => {
                    let frames = decoder.decode_packet(&packet, target_w, target_h)?;
                    queue.extend(frames);
                }
                StreamKind::Audio => {
                    if let (Some(audio_dec), Some(audio_ply)) =
                        (&mut self.audio_decoder, &self.audio_player)
                    {
                        if let Ok(samples) = audio_dec.decode_packet(&packet) {
                            audio_ply.push_samples(&samples);
                        }
                    }
                }
                StreamKind::Other => {}
            }
        }
        Ok(())
    }

    /// 根据单调主时钟评估视频帧时间戳
    fn schedule_frame_pts(
        &mut self,
        frame_pts: f64,
        demuxer: &mut Demuxer,
        queue: &mut VecDeque<RgbFrame>,
    ) -> Result<bool> {
        let decision = self.clock.evaluate_frame(frame_pts);

        match decision {
            SyncDecision::Wait(dur) => {
                let deadline = Instant::now() + dur;
                while Instant::now() < deadline {
                    self.handle_input_events(demuxer, queue)?;
                    if !self.running || self.osd_state.paused {
                        return Ok(false);
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(Duration::from_millis(3)));
                }
                Ok(true)
            }
            SyncDecision::Drop => Ok(false),
            SyncDecision::Render => Ok(true),
        }
    }

    /// 排空键盘事件并响应播放控制
    fn handle_input_events(
        &mut self,
        demuxer: &mut Demuxer,
        queue: &mut VecDeque<RgbFrame>,
    ) -> Result<bool> {
        let mut mode_changed = false;
        let actions = drain_actions(Duration::from_millis(1))?;

        for action in actions {
            match action {
                PlayerAction::Quit => {
                    self.running = false;
                    return Ok(false);
                }
                PlayerAction::TogglePause => {
                    self.toggle_pause();
                }
                PlayerAction::CycleRenderMode => {
                    let new_mode = self.render_engine.cycle_mode();
                    self.osd_state.mode = new_mode;
                    queue.clear();
                    mode_changed = true;
                    log::info!("切换渲染模式至: {:?}", new_mode);
                }
                PlayerAction::ToggleOsd => {
                    self.osd_state.visible = !self.osd_state.visible;
                }
                PlayerAction::AdjustVolume(delta) => {
                    self.osd_state.volume = (self.osd_state.volume + delta).clamp(0.0, 2.0);
                    if let Some(player) = &self.audio_player {
                        player.set_volume(self.osd_state.volume);
                    }
                }
                PlayerAction::ToggleMute => {
                    self.osd_state.muted = !self.osd_state.muted;
                    if let Some(player) = &self.audio_player {
                        player.set_mute(self.osd_state.muted);
                    }
                }
                PlayerAction::Seek(delta_secs) => {
                    let current = self.clock.current_time();
                    let target = (current + delta_secs as f64).max(0.0);
                    let _ = demuxer.seek(target);
                    self.clock.seek_to(target);
                    if let Some(player) = &self.audio_player {
                        player.seek_to(target);
                    }
                    queue.clear();
                }
                PlayerAction::None => {}
            }
        }
        Ok(mode_changed)
    }

    fn toggle_pause(&mut self) {
        self.osd_state.paused = !self.osd_state.paused;
        if self.osd_state.paused {
            self.clock.pause();
            if let Some(player) = &self.audio_player {
                player.set_paused(true);
            }
        } else {
            self.clock.resume();
            if let Some(player) = &self.audio_player {
                player.set_paused(false);
            }
        }
    }

    fn render_osd<W: Write>(
        &mut self,
        term_w: u16,
        term_h: u16,
        current_pts: f64,
        writer: &mut W,
    ) -> Result<()> {
        if !self.osd_state.visible {
            return Ok(());
        }

        self.osd_state.current_secs = current_pts;
        self.osd_state.fps = self.clock.fps();
        self.osd_state.dropped_frames = self.clock.dropped_frames();
        self.osd_state.mode = self.render_engine.current_mode();

        let bar_text = render_osd_bar(&self.osd_state, term_w);
        write!(writer, "\x1b[{term_h};1H\x1b[7m{bar_text}\x1b[0m")?;
        Ok(())
    }
}

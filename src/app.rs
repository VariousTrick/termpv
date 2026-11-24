// 应用程序核心控制器模块（驱动多线程音视频管线、TUI 渲染主循环）
use crate::config::{Config, RenderMode};
use crate::core::clock::{MasterClock, SyncDecision};
use crate::core::pipeline::PlaybackPipeline;
use crate::error::{Result, TermpvError};
use crate::render::{Layout, RenderEngine};
use crate::tui::{drain_actions, render_osd_bar, OsdState, PlayerAction, TerminalGuard};
use crossterm::terminal;
use std::io::{stdout, BufWriter, Write};
use std::thread;
use std::time::{Duration, Instant};

/// termpv 应用主控制器
pub struct App {
    config: Config,
    clock: MasterClock,
    render_engine: RenderEngine,
    osd_state: OsdState,
    running: bool,
    last_seek_at: Option<Instant>,
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
            osd_state: osd,
            running: true,
            last_seek_at: None,
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

        // 启动工业级多线程解耦管线（后台启动 Demuxer + AudioWorker + VideoWorker）
        let pipeline = PlaybackPipeline::new(&self.config.file, &self.config)?;
        self.osd_state.total_secs = pipeline.duration();

        log::info!(
            "多线程播放管线已就绪: 时长={:.2}s, 视频分辨率={}x{}",
            pipeline.duration(),
            pipeline.source_width(),
            pipeline.source_height()
        );

        // 激活终端接管保护
        let _guard = TerminalGuard::new()?;
        let mut out_writer = BufWriter::with_capacity(512 * 1024, stdout());

        // 计算初始尺寸并通知视频解码 Worker 准备首批缩放帧
        let (term_w, term_h) = terminal::size()?;
        let video_term_h = term_h.saturating_sub(1).max(1);
        let (initial_layout, init_w, init_h) = self.calculate_contain_layout(
            pipeline.source_width(),
            pipeline.source_height(),
            term_w,
            video_term_h,
        );
        pipeline.set_target_resolution(init_w, init_h);

        let mut last_layout = initial_layout;

        while self.running {
            // 1. 处理用户键盘事件
            let mode_changed = self.handle_input_events(&pipeline, &mut out_writer)?;
            if !self.running {
                break;
            }

            // 2. 暂停状态处理
            if self.osd_state.paused {
                thread::sleep(Duration::from_millis(15));
                continue;
            }

            // 3. 终端窗口尺寸变更或模式切换响应
            let (cur_term_w, cur_term_h) = terminal::size()?;
            let cur_video_h = cur_term_h.saturating_sub(1).max(1);
            if mode_changed || cur_term_w != last_layout.term_w || cur_video_h != last_layout.term_h {
                let (new_layout, new_target_w, new_target_h) = self.calculate_contain_layout(
                    pipeline.source_width(),
                    pipeline.source_height(),
                    cur_term_w,
                    cur_video_h,
                );
                last_layout = new_layout;
                pipeline.set_target_resolution(new_target_w, new_target_h);
            }

            // 4. 从后台有界队列提取预解码成品帧（超时 15ms，绝不阻塞 UI）
            let Some(frame) = pipeline.pop_video_frame_timeout(Duration::from_millis(15)) else {
                let is_seeking = self
                    .last_seek_at
                    .map_or(false, |t| t.elapsed() < Duration::from_millis(600));
                if !is_seeking && pipeline.is_eof() {
                    self.running = false;
                }
                continue;
            };

            // 5. 帧调度：优先以硬件音频时钟为基准，无音频时回退至 CPU 绝对单调时钟
            let audio_pts = pipeline.current_audio_pts();
            let decision = self.schedule_frame_pts(frame.pts, audio_pts);

            match decision {
                SyncDecision::Drop => {
                    self.clock.evaluate_frame(frame.pts); // 累计丢帧统计
                    continue;
                }
                SyncDecision::Wait(dur) => {
                    let deadline = Instant::now() + dur;
                    while Instant::now() < deadline {
                        let inner_mode_changed = self.handle_input_events(&pipeline, &mut out_writer)?;
                        if inner_mode_changed {
                            let (cw, ch) = terminal::size().unwrap_or((120, 40));
                            let cvh = ch.saturating_sub(1).max(1);
                            let (nl, ntw, nth) = self.calculate_contain_layout(
                                pipeline.source_width(),
                                pipeline.source_height(),
                                cw,
                                cvh,
                            );
                            last_layout = nl;
                            pipeline.set_target_resolution(ntw, nth);
                        }
                        if !self.running || self.osd_state.paused {
                            break;
                        }
                        let rem = deadline.saturating_duration_since(Instant::now());
                        if rem > Duration::from_millis(4) {
                            thread::sleep(rem - Duration::from_millis(2));
                        } else if rem > Duration::from_micros(500) {
                            thread::yield_now();
                        }
                    }
                }
                SyncDecision::Render => {}
            }

            if !self.running {
                break;
            }

            self.clock.record_rendered_frame();

            // 6. 渲染视频图像与 OSD 状态栏
            self.render_engine
                .render_frame(&frame, &last_layout, &mut out_writer)?;

            let current_pts = audio_pts.unwrap_or(frame.pts);
            self.render_osd(cur_term_w, cur_term_h, current_pts, &mut out_writer)?;
            out_writer.flush()?;
        }

        Ok(())
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
                // 对齐 MPV：在 Kitty 协议模式下，终端具备 GPU 级别的双三次滤波硬件缩放。
                // 现代高清屏（1080p HiDPI / 2K / 4K）单字符物理宽度通常为 18~24 像素。
                // 严禁以 9px 严重低估导致 CPU 强行欠采样失真；确保保留足够的超采样细节，实现如 mpv 般的超清锐利画质
                let est_col_width = if let Ok(ws) = crossterm::terminal::window_size() {
                    if ws.columns > 0 && ws.width > 0 {
                        (ws.width / ws.columns).clamp(12, 48) as u32
                    } else {
                        18
                    }
                } else {
                    18
                };
                let viewport_px = render_cols as u32 * est_col_width;
                let raw_w = viewport_px.clamp(1280.min(src_w), src_w).max(2);
                let px_w = (raw_w & !1).max(2);
                let raw_h = ((px_w as f64) / vid_aspect).round() as u32;
                let px_h = (raw_h & !1).max(2);
                (px_w, px_h)
            }
            RenderMode::HalfBlock => {
                let px_w = (render_cols as u32 & !1).max(2);
                let px_h = ((render_rows as u32 * 2) & !1).max(2);
                (px_w, px_h)
            }
            RenderMode::Ascii => {
                let px_w = (render_cols as u32 & !1).max(2);
                let px_h = (render_rows as u32 & !1).max(2);
                (px_w, px_h)
            }
            _ => {
                let px_w = (render_cols as u32 & !1).max(2);
                let px_h = ((render_rows as u32 * 2) & !1).max(2);
                (px_w, px_h)
            }
        };

        (layout, target_px_w, target_px_h)
    }

    /// 帧调度决策（音频主时钟优先，无音频时使用单调 CPU 壁钟）
    fn schedule_frame_pts(&mut self, frame_pts: f64, audio_pts: Option<f64>) -> SyncDecision {
        if let Some(a_pts) = audio_pts {
            let diff = frame_pts - a_pts;
            // 画面落后音频超过 200ms：才丢帧追赶音频（对齐 MPV，容忍微秒级 I/O 抖动，防止误杀帧）
            if diff < -0.200 {
                return SyncDecision::Drop;
            }
            // 画面超前音频超过 3ms：等待音频对齐
            if diff > 0.003 {
                return SyncDecision::Wait(Duration::from_secs_f64(diff));
            }
            // 准时渲染
            SyncDecision::Render
        } else {
            // 回退到 CPU 单调时钟
            self.clock.evaluate_frame(frame_pts)
        }
    }

    /// 排空键盘事件并响应播放控制
    fn handle_input_events<W: Write>(
        &mut self,
        pipeline: &PlaybackPipeline,
        writer: &mut W,
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
                    self.osd_state.paused = !self.osd_state.paused;
                    pipeline.set_paused(self.osd_state.paused);
                    if self.osd_state.paused {
                        self.clock.pause();
                    } else {
                        self.clock.resume();
                    }
                }
                PlayerAction::CycleRenderMode => {
                    let new_mode = self.render_engine.cycle_mode();
                    self.osd_state.mode = new_mode;
                    mode_changed = true;
                    log::info!("切换渲染模式至: {:?}", new_mode);

                    // 彻底清除终端中可能残留的 Kitty GPU 贴图纹理与全屏内容，防止图层遮挡
                    write!(
                        writer,
                        "\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=d,d=a,q=2\x1b\\\x1b[2J\x1b[1;1H\x1b[3J"
                    )?;
                    writer.flush()?;
                }
                PlayerAction::ToggleOsd => {
                    self.osd_state.visible = !self.osd_state.visible;
                }
                PlayerAction::AdjustVolume(delta) => {
                    let next = (pipeline.volume() + delta).clamp(0.0, 2.0);
                    pipeline.set_volume(next);
                    self.osd_state.volume = next;
                }
                PlayerAction::ToggleMute => {
                    pipeline.toggle_mute();
                    self.osd_state.muted = pipeline.is_muted();
                }
                PlayerAction::Seek(delta_secs) => {
                    let current = pipeline
                        .current_audio_pts()
                        .unwrap_or_else(|| self.clock.current_time());
                    let max_target = (pipeline.duration() - 0.5).max(0.0);
                    let target = (current + delta_secs as f64).clamp(0.0, max_target);
                    self.last_seek_at = Some(Instant::now());
                    pipeline.seek(target);
                    self.clock.seek_to(target);
                }
                PlayerAction::None => {}
            }
        }
        Ok(mode_changed)
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

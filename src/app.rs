// 应用程序核心控制器模块（驱动多线程音视频管线、TUI 渲染主循环）
use crate::config::{Config, RenderMode};
use crate::core::clock::{MasterClock, SyncDecision};
use crate::core::pipeline::PlaybackPipeline;
use crate::error::{Result, TermpvError};
use crate::render::{Layout, RenderEngine};
use crate::tui::{drain_actions, OsdState, PlayerAction, TerminalGuard};
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
    current_seek_gen: u64,
    pending_seek_snap: bool,
}

impl App {
    pub fn new(config: Config) -> Self {
        let osd = OsdState::new(config.render_mode, config.volume, config.mute);

        Self {
            render_engine: RenderEngine::new(config.render_mode),
            config,
            clock: MasterClock::new(),
            osd_state: osd,
            running: true,
            last_seek_at: None,
            current_seek_gen: 0,
            pending_seek_snap: false,
        }
    }

    /// 启动应用主生命周期，返回 (最终播放时间戳, 总时长)
    pub fn run(&mut self) -> Result<(f64, f64)> {
        let file_path = self.config.file.clone().ok_or_else(|| {
            TermpvError::FileNotFound(std::path::PathBuf::from("<未指定媒体目标>"))
        })?;
        let path_str = file_path.to_string_lossy();
        let is_network_url = path_str.starts_with("http://")
            || path_str.starts_with("https://")
            || path_str.starts_with("rtsp://")
            || path_str.starts_with("rtmp://");
        if !is_network_url && !file_path.exists() {
            return Err(TermpvError::FileNotFound(file_path));
        }

        if is_network_url {
            eprintln!("\x1b[36m🌐 正在连接网络媒体流并解析元信息，请稍候...\x1b[0m");
        }

        // 初始化 FFmpeg 并彻底静音底层 stderr 日志（杜绝在 TUI 备用屏幕下输出换行导致终端画面顶滚错位）
        ffmpeg_next::init()
            .map_err(|e| TermpvError::FfmpegError(format!("FFmpeg 初始化失败: {e}")))?;
        unsafe {
            ffmpeg_next::ffi::av_log_set_level(ffmpeg_next::ffi::AV_LOG_QUIET);
        }

        // 启动工业级多线程解耦管线（后台启动 Demuxer + AudioWorker + VideoWorker）
        let pipeline = PlaybackPipeline::new(&file_path, &self.config)?;
        let duration = pipeline.duration();
        self.osd_state.total_secs = duration;

        // 若指定了起始断点（例如断点续播），立即执行初始 Seek
        if let Some(start_secs) = self.config.start {
            if start_secs > 0.0 {
                let max_target = (duration - 0.5).max(0.0);
                let target = start_secs.clamp(0.0, max_target);
                self.current_seek_gen += 1;
                self.pending_seek_snap = true;
                self.clock.seek_to(target);
                pipeline.seek(target, self.current_seek_gen, 0.0);
            }
        }

        log::info!(
            "多线程播放管线已就绪: 时长={:.2}s, 视频分辨率={}x{}",
            pipeline.duration(),
            pipeline.source_width(),
            pipeline.source_height()
        );

        // 激活终端接管保护
        let _guard = TerminalGuard::new()?;
        let mut out_writer = BufWriter::with_capacity(8 * 1024 * 1024, stdout());

        // 计算初始尺寸并通知视频解码 Worker 准备首批缩放帧
        // 在 Kitty 模式下启用 z=-1 背景图层，视频直接 100% 铺满整个终端高度，OSD 浮动在图层上层；
        // 纯字符模式（HalfBlock/Ascii）保留底行用于状态文本显示
        let (term_w, term_h) = terminal::size()?;
        let video_term_h = match self.render_engine.current_mode() {
            RenderMode::Kitty => term_h,
            _ => term_h.saturating_sub(1).max(1),
        };
        let (initial_layout, init_w, init_h) = self.calculate_contain_layout(
            pipeline.source_width(),
            pipeline.source_height(),
            term_w,
            video_term_h,
        );
        pipeline.set_target_resolution(init_w, init_h);

        let mut last_layout = initial_layout;
        let mut final_pts = self.config.start.unwrap_or(0.0);

        let mut diag_frames = 0u64;
        let mut diag_last_wall = Instant::now();
        let mut diag_last_audio: Option<f64> = None;
        let mut diag_last_video: Option<f64> = None;

        while self.running {
            // 1. 处理用户键盘事件
            let mode_changed = self.handle_input_events(&pipeline, &mut out_writer)?;
            if !self.running {
                break;
            }

            // 2. 暂停状态响应（停止帧提取与时钟步进，但持续响应键盘与 OSD 状态更新）
            if self.osd_state.paused {
                if self.pending_seek_snap {
                    if let Some((frame_gen, frame)) = pipeline.pop_video_frame_timeout(Duration::from_millis(50)) {
                        if frame_gen >= self.current_seek_gen {
                            self.pending_seek_snap = false;
                            self.clock.seek_to(frame.pts);
                            self.osd_state.current_secs = frame.pts;
                            self.render_engine
                                .render_frame(&frame, &last_layout, &mut out_writer)?;
                        }
                    }
                }
                let (cur_term_w, cur_term_h) = terminal::size()?;
                let current_pts = pipeline
                    .current_audio_pts()
                    .unwrap_or_else(|| self.clock.current_time());
                self.render_osd(cur_term_w, cur_term_h, current_pts, &pipeline, &mut out_writer)?;
                out_writer.flush()?;
                thread::sleep(Duration::from_millis(16));
                continue;
            }

            // 3. 终端窗口尺寸变更或模式切换响应
            let (cur_term_w, cur_term_h) = terminal::size()?;
            let cur_video_h = match self.render_engine.current_mode() {
                RenderMode::Kitty => cur_term_h,
                _ => cur_term_h.saturating_sub(1).max(1),
            };
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
            let Some((frame_gen, frame)) = pipeline.pop_video_frame_timeout(Duration::from_millis(15)) else {
                let is_seeking = self
                    .last_seek_at
                    .map_or(false, |t| t.elapsed() < Duration::from_millis(600));
                if !is_seeking && pipeline.is_eof() {
                    self.running = false;
                }
                continue;
            };

            // 丢弃陈旧代数的残余帧
            if frame_gen < self.current_seek_gen {
                continue;
            }

            // 关键优化：Seek 后到达的首帧瞬态直刷（零等待、零丢帧、即刻呈现并对齐系统主时钟）
            if self.pending_seek_snap {
                self.pending_seek_snap = false;
                self.clock.seek_to(frame.pts);
                self.osd_state.current_secs = frame.pts;
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "[DIAG-SNAP] frame_pts={:.3}s audio_pts={:?}",
                        frame.pts,
                        pipeline.current_audio_pts()
                    );
                    diag_last_wall = Instant::now();
                    diag_last_audio = pipeline.current_audio_pts();
                    diag_last_video = Some(frame.pts);
                }

                self.clock.record_rendered_frame();
                self.render_engine
                    .render_frame(&frame, &last_layout, &mut out_writer)?;

                self.render_osd(cur_term_w, cur_term_h, frame.pts, &pipeline, &mut out_writer)?;
                out_writer.flush()?;
                continue;
            }

            // 5. 帧调度：优先以硬件音频时钟为基准，无音频时回退至 CPU 绝对单调时钟
            let audio_pts = pipeline.current_audio_pts();
            let decision = self.schedule_frame_pts(frame.pts, audio_pts);

            if log::log_enabled!(log::Level::Debug) {
                diag_frames += 1;
                let diff_ms = audio_pts.map(|a| (frame.pts - a) * 1000.0).unwrap_or(0.0);
                let is_anomaly = matches!(decision, SyncDecision::Drop) || diff_ms.abs() > 150.0;
                if diag_frames % 30 == 0 || (is_anomaly && diag_last_wall.elapsed() >= Duration::from_millis(200)) {
                    let wall_dt = diag_last_wall.elapsed().as_secs_f64().max(1e-6);
                    let a_rate = match (audio_pts, diag_last_audio) {
                        (Some(now), Some(prev)) => (now - prev) / wall_dt,
                        _ => 0.0,
                    };
                    let v_rate = match diag_last_video {
                        Some(prev) => (frame.pts - prev) / wall_dt,
                        None => 0.0,
                    };
                    log::debug!(
                        "[DIAG] frame={:.3}s audio={:?} diff={:+.1}ms dec={:?} fps={:.1} a_rate={:.3}x v_rate={:.3}x",
                        frame.pts,
                        audio_pts.map(|a| format!("{:.3}s", a)),
                        diff_ms,
                        decision,
                        self.clock.fps(),
                        a_rate,
                        v_rate
                    );
                    diag_last_wall = Instant::now();
                    diag_last_audio = audio_pts;
                    diag_last_video = Some(frame.pts);
                }
            }

            match decision {
                SyncDecision::Drop => {
                    self.clock.evaluate_frame(frame.pts); // 累计丢帧统计
                    continue;
                }
                SyncDecision::Wait(dur) => {
                    // 对齐 MPV：按实际音画差值等待（上限 500ms），并以 10ms 为切片持续响应按键，绝不阻塞 Seek/暂停
                    let effective_dur = dur.min(Duration::from_millis(500));

                    let deadline = Instant::now() + effective_dur;
                    while Instant::now() < deadline {
                        let inner_mode_changed = self.handle_input_events(&pipeline, &mut out_writer)?;
                        if inner_mode_changed {
                            let (cw, ch) = terminal::size().unwrap_or((120, 40));
                            let cvh = match self.render_engine.current_mode() {
                                RenderMode::Kitty => ch,
                                _ => ch.saturating_sub(1).max(1),
                            };
                            let (nl, ntw, nth) = self.calculate_contain_layout(
                                pipeline.source_width(),
                                pipeline.source_height(),
                                cw,
                                cvh,
                            );
                            last_layout = nl;
                            pipeline.set_target_resolution(ntw, nth);
                        }
                        if !self.running || self.osd_state.paused || self.pending_seek_snap {
                            break;
                        }
                        let rem = deadline.saturating_duration_since(Instant::now());
                        if rem > Duration::from_millis(4) {
                            thread::sleep((rem - Duration::from_millis(2)).min(Duration::from_millis(10)));
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
            if self.pending_seek_snap {
                continue;
            }

            self.clock.record_rendered_frame();

            // 6. 渲染视频图像与 OSD 状态栏
            self.render_engine
                .render_frame(&frame, &last_layout, &mut out_writer)?;

            let current_pts = audio_pts.unwrap_or(frame.pts);
            final_pts = current_pts;
            self.render_osd(cur_term_w, cur_term_h, current_pts, &pipeline, &mut out_writer)?;
            out_writer.flush()?;
        }

        Ok((final_pts, duration))
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

        // 动态推导终端单元格的真实物理高宽比（cell_h / cell_w）
        // 在 Kitty/Ghostty 等现代终端中，若 window_size 可用则精确推导真实物理点阵比例；
        // 若不可用，默认取标准现代等宽字体的 1.8（绝非死板粗糙的 2.0）
        let font_aspect = if let Ok(ws) = crossterm::terminal::window_size() {
            if ws.columns > 0 && ws.rows > 0 && ws.width > 0 && ws.height > 0 {
                let cell_w = ws.width as f64 / ws.columns as f64;
                let cell_h = ws.height as f64 / ws.rows as f64;
                if cell_w > 0.0 && cell_h > 0.0 {
                    (cell_h / cell_w).clamp(1.4, 2.4)
                } else {
                    1.8
                }
            } else {
                1.8
            }
        } else {
            1.8
        };

        // 视口物理几何纵横比
        let term_aspect = max_cols / (max_rows * font_aspect);

        // 严格遵循 Aspect Ratio Contain（保持 16:9 原画比例，等比最大化铺满）：
        // 当视口偏宽时：高度必然 100% 顶满整个视口，pad_y 严格为 0，消灭上下多余黑边！
        // 当视口偏高时：宽度必然 100% 顶满整个视口，pad_x 严格为 0，消灭左右多余黑边！
        let (render_cols, render_rows, pad_x, pad_y) = if term_aspect >= vid_aspect {
            let r = video_term_h;
            let c_float = (max_rows * font_aspect * vid_aspect).round();
            let c = (c_float.clamp(2.0, max_cols) as u16).min(term_w);
            let px = term_w.saturating_sub(c) / 2;
            (c, r, px, 0)
        } else {
            let c = term_w;
            let r_float = (max_cols / (font_aspect * vid_aspect)).round();
            let r = (r_float.clamp(2.0, max_rows) as u16).min(video_term_h);
            let py = video_term_h.saturating_sub(r) / 2;
            (c, r, 0, py)
        };

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
            // 极端异常保护（如刚 Seek 尚未对齐或流时间戳跳变超过 3 秒），直接渲染让主时钟对齐
            if diff.abs() > 3.0 {
                return SyncDecision::Render;
            }
            // 画面落后音频超过 50ms：丢帧追赶音频
            if diff < -0.050 {
                return SyncDecision::Drop;
            }
            // 画面超前音频超过 3ms：严格按实际差值等待音频追赶（绝不放任倍速脱缰）
            if diff > 0.003 {
                return SyncDecision::Wait(Duration::from_secs_f64(diff).min(Duration::from_millis(500)));
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
                    self.osd_state.trigger_pause(self.osd_state.paused);
                }
                PlayerAction::CycleRenderMode => {
                    let new_mode = self.render_engine.cycle_mode();
                    self.osd_state.mode = new_mode;
                    mode_changed = true;
                    log::info!("切换渲染模式至: {:?}", new_mode);
                    self.osd_state.trigger_mode(new_mode);

                    // 彻底清除终端中可能残留的 Kitty GPU 贴图纹理与全屏内容，防止图层遮挡
                    write!(
                        writer,
                        "\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=d,d=a,q=2\x1b\\\x1b[2J\x1b[1;1H\x1b[3J"
                    )?;
                    writer.flush()?;
                }
                PlayerAction::ToggleOsd => {
                    self.osd_state.toggle_stats();
                }
                PlayerAction::AdjustVolume(delta) => {
                    let next = (pipeline.volume() + delta).clamp(0.0, 2.0);
                    pipeline.set_volume(next);
                    self.osd_state.volume = next;
                    self.osd_state.trigger_volume(delta);
                }
                PlayerAction::ToggleMute => {
                    pipeline.toggle_mute();
                    self.osd_state.muted = pipeline.is_muted();
                    self.osd_state.trigger_mute(self.osd_state.muted);
                }
                PlayerAction::Seek(delta_secs) => {
                    let current = pipeline
                        .current_audio_pts()
                        .unwrap_or_else(|| self.clock.current_time());
                    let max_target = (pipeline.duration() - 0.5).max(0.0);
                    let target = (current + delta_secs as f64).clamp(0.0, max_target);
                    self.last_seek_at = Some(Instant::now());
                    self.osd_state.trigger_seek(delta_secs);
                    self.current_seek_gen += 1;
                    self.pending_seek_snap = true;
                    self.clock.seek_to(target);
                    let hit_cache = pipeline.seek(target, self.current_seek_gen, current);
                    if hit_cache {
                        log::info!("跳转命中双向内存缓存（前向/后向免断流），执行零延迟纯内存跳转至 {:.2}s", target);
                    }
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
        pipeline: &PlaybackPipeline,
        writer: &mut W,
    ) -> Result<()> {
        let (buf_secs, buf_bytes) = pipeline.buffer_stats(current_pts);
        self.osd_state.current_secs = current_pts;
        self.osd_state.buffered_secs = buf_secs;
        self.osd_state.buffered_bytes = buf_bytes;
        self.osd_state.fps = self.clock.fps();
        self.osd_state.dropped_frames = self.clock.dropped_frames();
        self.osd_state.mode = self.render_engine.current_mode();

        if let Some(content) = self.osd_state.render(term_w, term_h) {
            write!(writer, "{content}")?;
        }

        Ok(())
    }
}

// 屏幕状态显示（OSD, On-Screen Display）极简现代单行渲染组件
// 采用原生 MPV 极简克制单行风格（对齐 mpv --vo=tct / 纯终端模式）：
// 平时 100% 纯净无干扰，按键即时单行反馈，关闭/超时彻底擦除，绝不留多行残影
use crate::config::RenderMode;
use crate::render::filter::ColorFilterPreset;
use std::fmt::Write as FmtWrite;
use std::time::{Duration, Instant};

// 极简深色背景与 TrueColor 调色盘 (Catppuccin / Tokyo Night 配色体系)
pub const C_RESET: &str = "\x1b[0m";
pub const C_BAR_BG: &str = "\x1b[48;2;18;20;26m";     // 统一下底栏深色背景 (#12141a)
pub const C_BADGE_BG: &str = "\x1b[48;2;32;36;48m";   // 徽章微高亮背景 (#202430)

// 文字与高亮
pub const C_TEXT_MAIN: &str = "\x1b[1;38;2;230;235;245m"; // 高亮纯白主文字 (#e6ebf5)
pub const C_TEXT_MUTED: &str = "\x1b[38;2;140;145;165m"; // 优雅中灰副文字 (#8c91a5)
pub const C_TEXT_DIM: &str = "\x1b[38;2;85;90;110m";     // 暗灰分割与未激活轨道 (#555a6e)

// 功能主题色
pub const C_CYAN: &str = "\x1b[1;38;2;137;220;235m";     // 冰青 (termpv 标志 / 播放进度)
pub const C_GREEN: &str = "\x1b[1;38;2;166;227;161m";    // 薄荷绿 (高帧率 / 恢复 / 音量正常)
pub const C_YELLOW: &str = "\x1b[1;38;2;249;226;175m";   // 暖金 (暂停指示)
pub const C_PEACH: &str = "\x1b[1;38;2;250;179;135m";    // 蜜桃橙 (音量调节 / 拖拽)
pub const C_RED: &str = "\x1b[1;38;2;243;139;168m";      // 柔粉红 (静音 / 丢帧 / 快退)
pub const C_PURPLE: &str = "\x1b[1;38;2;180;190;254m";   // 薰衣草紫 (渲染引擎)

pub const C_DOT: &str = "\x1b[1;38;2;255;255;255m";     // 进度条白色圆点游标
pub const C_BUFFERED: &str = "\x1b[38;2;90;140;175m";   // 次级中亮蓝灰，已缓冲区间 (#5a8caf)

/// 瞬态浮层动作类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OsdPopupKind {
    Seek { delta_secs: i64 },
    Volume { delta: f32 },
    Mute { muted: bool },
    Pause,
    Resume,
    Mode(RenderMode),
    Filter {
        preset: ColorFilterPreset,
        hue_deg: Option<u16>,
    },
}

/// 瞬态浮层结构体（支持生命周期定时器与自动淡化）
#[derive(Debug, Clone)]
pub struct OsdPopup {
    pub kind: OsdPopupKind,
    pub created_at: Instant,
    pub duration: Duration,
}

impl OsdPopup {
    pub fn new(kind: OsdPopupKind, duration: Duration) -> Self {
        Self {
            kind,
            created_at: Instant::now(),
            duration,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration
    }
}

/// 内部区分 OSD 显示意图
#[derive(Debug, Clone, Copy, PartialEq)]
enum OsdKind {
    Stats,
    Popup(OsdPopupKind),
}

/// OSD 核心状态机与调度中心
#[derive(Debug, Clone)]
pub struct OsdState {
    pub paused: bool,
    pub current_secs: f64,
    pub total_secs: f64,
    pub volume: f32,
    pub muted: bool,
    pub mode: RenderMode,
    pub fps: f64,
    pub dropped_frames: u64,
    pub buffered_secs: f64,
    pub buffered_bytes: usize,

    /// 是否开启常驻详细诊断面板（按 Tab 或 o 键呼出）
    pub show_stats: bool,

    /// 当前激活的瞬态动作浮层
    pub active_popup: Option<OsdPopup>,

    /// 当前 OSD 是否处于可见显示状态（用于在关闭/超时帧精准发出整行擦除指令）
    pub is_visible: bool,

    /// 上次渲染 OSD 所在行号（若终端大小改变，自动擦除旧行）
    pub last_rendered_row: u16,

    /// 兼容字段（用于清理旧版多行残余）
    pub lines_to_clear: u16,
}

impl Default for OsdState {
    fn default() -> Self {
        Self {
            paused: false,
            current_secs: 0.0,
            total_secs: 0.0,
            volume: 1.0,
            muted: false,
            mode: RenderMode::Auto,
            fps: 0.0,
            dropped_frames: 0,
            buffered_secs: 0.0,
            buffered_bytes: 0,
            show_stats: false,
            active_popup: None,
            is_visible: false,
            last_rendered_row: 0,
            lines_to_clear: 0,
        }
    }
}

impl OsdState {
    pub fn new(mode: RenderMode, volume: f32, muted: bool) -> Self {
        Self {
            paused: false,
            current_secs: 0.0,
            total_secs: 0.0,
            volume,
            muted,
            mode,
            fps: 0.0,
            dropped_frames: 0,
            buffered_secs: 0.0,
            buffered_bytes: 0,
            show_stats: false,
            active_popup: None,
            is_visible: false,
            last_rendered_row: 0,
            lines_to_clear: 0,
        }
    }

    /// 触发快进/快退浮层反馈（1.5 秒自动淡出）
    pub fn trigger_seek(&mut self, delta_secs: i64) {
        self.active_popup = Some(OsdPopup::new(
            OsdPopupKind::Seek { delta_secs },
            Duration::from_millis(1500),
        ));
    }

    /// 触发音量调节浮层反馈（1.5 秒自动淡出）
    pub fn trigger_volume(&mut self, delta: f32) {
        self.active_popup = Some(OsdPopup::new(
            OsdPopupKind::Volume { delta },
            Duration::from_millis(1500),
        ));
    }

    /// 触发静音浮层反馈（1.5 秒自动淡出）
    pub fn trigger_mute(&mut self, muted: bool) {
        self.active_popup = Some(OsdPopup::new(
            OsdPopupKind::Mute { muted },
            Duration::from_millis(1500),
        ));
    }

    /// 触发暂停/恢复浮层反馈
    pub fn trigger_pause(&mut self, paused: bool) {
        if paused {
            // 暂停态常驻浮层提醒
            self.active_popup = Some(OsdPopup::new(
                OsdPopupKind::Pause,
                Duration::from_secs(86400 * 365),
            ));
        } else {
            // 恢复播放闪烁 0.8 秒后快速隐退，归还画面
            self.active_popup = Some(OsdPopup::new(
                OsdPopupKind::Resume,
                Duration::from_millis(800),
            ));
        }
    }

    /// 触发渲染模式切换浮层反馈（1.5 秒自动淡出）
    pub fn trigger_mode(&mut self, mode: RenderMode) {
        self.active_popup = Some(OsdPopup::new(
            OsdPopupKind::Mode(mode),
            Duration::from_millis(1500),
        ));
    }

    /// 触发色彩风格滤镜切换浮层反馈（1.8 秒自动淡出）
    pub fn trigger_filter(&mut self, preset: ColorFilterPreset, hue_deg: Option<u16>) {
        self.active_popup = Some(OsdPopup::new(
            OsdPopupKind::Filter { preset, hue_deg },
            Duration::from_millis(1800),
        ));
    }

    /// 切换详细统计面板
    pub fn toggle_stats(&mut self) {
        self.show_stats = !self.show_stats;
    }

    /// 核心渲染生成器：计算当前帧需要上屏的 ANSI 转义序列
    /// 遵循原生 MPV 极简主义：
    /// 1. 严格单行 HUD，定位在终端最后一行（term_h），绝不换行，绝不造成终端滚屏
    /// 2. 关闭或超时隐藏瞬间，主动返回整行彻底清屏指令，100% 消除任何残影
    /// 3. 静默期间返回 None，实现零开销纯净观影
    pub fn render(&mut self, term_w: u16, term_h: u16) -> Option<String> {
        if term_w < 10 || term_h < 1 {
            return None;
        }

        // 1. 检查浮层是否超时过期
        if let Some(popup) = &self.active_popup {
            if popup.is_expired() && !self.paused {
                self.active_popup = None;
            }
        }

        // 2. 检查当前是否有需要显示的 OSD 意图
        let active_kind = if self.show_stats {
            Some(OsdKind::Stats)
        } else if let Some(popup) = &self.active_popup {
            Some(OsdKind::Popup(popup.kind))
        } else if self.paused {
            Some(OsdKind::Popup(OsdPopupKind::Pause))
        } else {
            None
        };

        // 3. 当前无显示意图：如果上一帧处于可见状态，本帧返回彻底擦除序列并转为隐藏
        let Some(kind) = active_kind else {
            if self.is_visible || self.lines_to_clear > 0 {
                self.is_visible = false;
                self.lines_to_clear = 0;
                let mut clear_seq = String::with_capacity(64);
                // 彻底重置属性，擦除底行
                let _ = write!(clear_seq, "{C_RESET}\x1b[{term_h};1H\x1b[2K");
                if self.last_rendered_row > 0 && self.last_rendered_row != term_h {
                    let _ = write!(clear_seq, "\x1b[{};1H\x1b[2K", self.last_rendered_row);
                    self.last_rendered_row = 0;
                }
                return Some(clear_seq);
            }
            return None;
        };

        // 4. 有显示意图：构建严格单行 HUD（绝不换行，永不触发终端卷屏）
        self.is_visible = true;
        self.lines_to_clear = 1;

        let mut out = String::with_capacity(512);

        // 若终端高度改变，先清除旧行
        if self.last_rendered_row > 0 && self.last_rendered_row != term_h {
            let _ = write!(out, "{C_RESET}\x1b[{};1H\x1b[2K", self.last_rendered_row);
        }
        self.last_rendered_row = term_h;

        // 定位至底行并彻底清空当前行
        let _ = write!(out, "{C_RESET}\x1b[{term_h};1H\x1b[2K");

        match kind {
            OsdKind::Stats => {
                self.render_stats_hud(&mut out, term_w);
            }
            OsdKind::Popup(popup_kind) => match popup_kind {
                OsdPopupKind::Seek { delta_secs } => {
                    self.render_seek_hud(&mut out, term_w, delta_secs);
                }
                OsdPopupKind::Volume { delta } => {
                    self.render_volume_hud(&mut out, term_w, delta);
                }
                OsdPopupKind::Mute { muted } => {
                    self.render_mute_hud(&mut out, term_w, muted);
                }
                OsdPopupKind::Pause => {
                    self.render_pause_hud(&mut out, term_w);
                }
                OsdPopupKind::Resume => {
                    self.render_resume_hud(&mut out, term_w);
                }
                OsdPopupKind::Mode(m) => {
                    self.render_mode_hud(&mut out, term_w, m);
                }
                OsdPopupKind::Filter { preset, hue_deg } => {
                    self.render_filter_hud(&mut out, term_w, preset, hue_deg);
                }
            },
        }

        // 确保末尾重置 SGR，防止样式污染后续画面
        out.push_str(C_RESET);

        Some(out)
    }

    /// 渲染 Tab 单行极简状态栏（对齐 mpv 状态行，清晰克制优雅）
    fn render_stats_hud(&self, out: &mut String, term_w: u16) {
        let cur = format_duration(self.current_secs);
        let tot = format_duration(self.total_secs);
        let pct = if self.total_secs > 0.0 {
            ((self.current_secs / self.total_secs) * 100.0).clamp(0.0, 100.0) as u32
        } else {
            0
        };

        let mode_desc = match self.mode {
            RenderMode::Kitty => "Kitty (GPU)",
            RenderMode::HalfBlock => "HalfBlock",
            RenderMode::Ascii => "ASCII",
            _ => "Auto",
        };

        let vol_pct = (self.volume * 100.0).round() as u32;
        let vol_str = if self.muted {
            format!("{C_RED}🔇 Muted{C_RESET}{C_BAR_BG}")
        } else {
            format!("{C_PEACH}🔊 {vol_pct}%{C_RESET}{C_BAR_BG}")
        };

        let drop_str = if self.dropped_frames > 0 {
            format!(" {C_RED}({} drop){C_RESET}{C_BAR_BG}", self.dropped_frames)
        } else {
            String::new()
        };

        let cache_str = if self.buffered_secs > 0.05 {
            let mb = (self.buffered_bytes as f64) / (1024.0 * 1024.0);
            if mb >= 1.0 {
                format!(" {C_TEXT_DIM}│{C_BAR_BG} {C_CYAN}📥 {:.1}s ({:.0}MB){C_RESET}{C_BAR_BG}", self.buffered_secs, mb)
            } else {
                format!(" {C_TEXT_DIM}│{C_BAR_BG} {C_CYAN}📥 {:.1}s{C_RESET}{C_BAR_BG}", self.buffered_secs)
            }
        } else {
            String::new()
        };

        out.push_str(C_BAR_BG);
        let left_text = format!(" {C_BADGE_BG}{C_CYAN} termpv {C_RESET}{C_BAR_BG}  {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot} ");
        let right_text = format!(" {C_TEXT_DIM}({pct}%) {C_TEXT_DIM}│{C_BAR_BG} {C_GREEN}{:.1} FPS{C_RESET}{C_BAR_BG}{drop_str}{cache_str} {C_TEXT_DIM}│{C_BAR_BG} {C_PURPLE}{mode_desc}{C_RESET}{C_BAR_BG} {C_TEXT_DIM}│{C_BAR_BG} {vol_str} ", self.fps);
        let needed_len = visible_len(&left_text) + visible_len(&right_text) + 2;

        if (term_w as usize) >= needed_len + 8 {
            // 宽屏模式：空间充裕时完整呈现三段式迷你进度轨
            let rail_len = (term_w as usize).saturating_sub(needed_len);
            out.push_str(&left_text);
            render_rail(out, rail_len, self.current_secs, self.buffered_secs, self.total_secs, C_CYAN);
            out.push(' ');
            out.push_str(&right_text);
        } else if term_w >= 75 {
            let _ = write!(
                out,
                " {C_BADGE_BG}{C_CYAN} termpv {C_RESET}{C_BAR_BG}  {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot} {C_TEXT_DIM}({pct}%) {C_TEXT_DIM}│{C_BAR_BG} {C_GREEN}{:.1} FPS{C_RESET}{C_BAR_BG}{drop_str}{cache_str} {C_TEXT_DIM}│{C_BAR_BG} {C_PURPLE}{mode_desc}{C_RESET}{C_BAR_BG} {C_TEXT_DIM}│{C_BAR_BG} {vol_str} ",
                self.fps
            );
        } else if term_w >= 50 {
            let _ = write!(
                out,
                " {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot}{cache_str} {C_TEXT_DIM}│{C_BAR_BG} {C_GREEN}{:.1} FPS{C_RESET}{C_BAR_BG} {C_TEXT_DIM}│{C_BAR_BG} {vol_str} ",
                self.fps
            );
        } else {
            let _ = write!(
                out,
                " {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot} {vol_str} "
            );
        }
    }

    /// 渲染快进/快退浮层：时间戳 + 细圆点进度轨 + 动态步长胶囊
    fn render_seek_hud(&self, out: &mut String, term_w: u16, delta: i64) {
        let cur = format_duration(self.current_secs);
        let tot = format_duration(self.total_secs);

        let badge = if delta >= 0 {
            format!("{C_BADGE_BG}{C_GREEN} ▶▶ +{delta}s {C_RESET}{C_BAR_BG}")
        } else {
            format!("{C_BADGE_BG}{C_RED} ◀◀ {delta}s {C_RESET}{C_BAR_BG}")
        };
        let badge_visible_len = 9 + if delta.abs() >= 10 { 1 } else { 0 };

        // 左侧时间戳
        let left = format!(" {C_CYAN}{cur}{C_RESET}{C_BAR_BG} {C_TEXT_DIM}/{C_BAR_BG} {C_TEXT_MUTED}{tot}{C_RESET}{C_BAR_BG}  ");
        let left_visible_len = 1 + cur.len() + 3 + tot.len() + 2;

        let fixed_len = left_visible_len + badge_visible_len + 3;
        let rail_len = (term_w as usize).saturating_sub(fixed_len);

        out.push_str(C_BAR_BG);
        out.push_str(&left);
        if rail_len >= 8 {
            render_rail(out, rail_len, self.current_secs, self.buffered_secs, self.total_secs, C_CYAN);
            out.push_str("  ");
        }
        out.push_str(&badge);
        out.push(' ');
    }

    /// 渲染音量调节浮层：音量数值 + 能量细轨 + 微调指示
    fn render_volume_hud(&self, out: &mut String, term_w: u16, delta: f32) {
        let vol_pct = (self.volume * 100.0).round() as u32;
        let icon = if self.muted || vol_pct == 0 { "🔇" } else { "🔊" };

        let left = format!(" {C_PEACH}{icon} {vol_pct}%{C_RESET}{C_BAR_BG}  ");
        let left_visible_len = 7 + if vol_pct >= 100 { 1 } else { 0 };

        let delta_pct = (delta.abs() * 100.0).round() as u32;
        let badge = if delta >= 0.0 {
            format!("{C_BADGE_BG}{C_PEACH} ▲ +{delta_pct}% {C_RESET}{C_BAR_BG}")
        } else {
            format!("{C_BADGE_BG}{C_PEACH} ▼ -{delta_pct}% {C_RESET}{C_BAR_BG}")
        };
        let badge_visible_len = 10;

        let fixed_len = left_visible_len + badge_visible_len + 3;
        let rail_len = (term_w as usize).saturating_sub(fixed_len);

        out.push_str(C_BAR_BG);
        out.push_str(&left);
        if rail_len >= 8 {
            render_rail(out, rail_len, self.volume as f64, 0.0, 2.0, C_PEACH);
            out.push_str("  ");
        }
        out.push_str(&badge);
        out.push(' ');
    }

    /// 渲染静音切换浮层
    fn render_mute_hud(&self, out: &mut String, _term_w: u16, muted: bool) {
        out.push_str(C_BAR_BG);
        if muted {
            let _ = write!(
                out,
                " {C_BADGE_BG}{C_RED} 🔇 已开启静音 {C_RESET}{C_BAR_BG}  {C_TEXT_MUTED}(按 x 或 ↑ 恢复音量){C_RESET}{C_BAR_BG} "
            );
        } else {
            let pct = (self.volume * 100.0).round() as u32;
            let _ = write!(
                out,
                " {C_BADGE_BG}{C_GREEN} 🔊 声音已恢复: {pct}% {C_RESET}{C_BAR_BG} "
            );
        }
    }

    /// 渲染暂停浮层：优雅停留并指示时间与进度条
    fn render_pause_hud(&self, out: &mut String, term_w: u16) {
        let cur = format_duration(self.current_secs);
        let tot = format_duration(self.total_secs);

        let left = format!(" {C_BADGE_BG}{C_YELLOW} ⏸ PAUSED {C_RESET}{C_BAR_BG}  {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot}{C_RESET}{C_BAR_BG}  ");
        let left_visible_len = 12 + 2 + cur.len() + 1 + tot.len() + 2;

        let fixed_len = left_visible_len + 3;
        let rail_len = (term_w as usize).saturating_sub(fixed_len);

        out.push_str(C_BAR_BG);
        out.push_str(&left);
        if rail_len >= 8 {
            render_rail(out, rail_len, self.current_secs, self.buffered_secs, self.total_secs, C_YELLOW);
            out.push(' ');
        }
    }

    /// 渲染恢复播放浮层：闪现后秒退
    fn render_resume_hud(&self, out: &mut String, _term_w: u16) {
        let cur = format_duration(self.current_secs);
        let tot = format_duration(self.total_secs);
        out.push_str(C_BAR_BG);
        let _ = write!(
            out,
            " {C_BADGE_BG}{C_GREEN} ▶ PLAYING {C_RESET}{C_BAR_BG}  {C_TEXT_MAIN}{cur}{C_TEXT_DIM}/{C_TEXT_MUTED}{tot}{C_RESET}{C_BAR_BG} "
        );
    }

    /// 渲染渲染模式切换提示
    fn render_mode_hud(&self, out: &mut String, _term_w: u16, mode: RenderMode) {
        let desc = match mode {
            RenderMode::Kitty => "Kitty (GPU 原画直刷)",
            RenderMode::HalfBlock => "HalfBlock (24位真彩双像素半块)",
            RenderMode::Ascii => "ASCII (黑客帝国复古字符)",
            _ => "自动侦测",
        };
        out.push_str(C_BAR_BG);
        let _ = write!(
            out,
            " {C_BADGE_BG}{C_PURPLE} 📺 渲染模式: {desc} {C_RESET}{C_BAR_BG} "
        );
    }

    /// 渲染色彩风格滤镜提示
    fn render_filter_hud(
        &self,
        out: &mut String,
        _term_w: u16,
        preset: ColorFilterPreset,
        hue_deg: Option<u16>,
    ) {
        let name = preset.display_name();
        out.push_str(C_BAR_BG);
        if let Some(deg) = hue_deg {
            let _ = write!(
                out,
                " {C_BADGE_BG}{C_PEACH} 🎨 {name} ({deg}°) {C_RESET}{C_BAR_BG} "
            );
        } else {
            let _ = write!(
                out,
                " {C_BADGE_BG}{C_PEACH} 🎨 色彩风格: {name} {C_RESET}{C_BAR_BG} "
            );
        }
    }
}

/// 绘制三段式高精平滑细圆点轨道：已播放(━) + ●游标 + 已缓存(━) + 未加载(─)
fn render_rail(
    out: &mut String,
    rail_len: usize,
    current: f64,
    buffered_secs: f64,
    total: f64,
    active_color: &str,
) {
    if rail_len == 0 {
        return;
    }

    let ratio = if total > 0.0 {
        (current / total).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let dot_idx = ((rail_len.saturating_sub(1)) as f64 * ratio).round() as usize;

    let buf_ratio = if total > 0.0 {
        ((current + buffered_secs) / total).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let buf_idx = (((rail_len.saturating_sub(1)) as f64 * buf_ratio).round() as usize).max(dot_idx);

    // 1. 已播放段（高亮主色）
    out.push_str(active_color);
    for _ in 0..dot_idx {
        out.push('━');
    }

    // 2. 播放游标
    out.push_str(C_DOT);
    out.push('●');

    // 3. 已缓冲段（次级中亮蓝灰，指示已预读入 500MB 环形缓冲的数据）
    if buf_idx > dot_idx {
        out.push_str(C_BUFFERED);
        for _ in (dot_idx + 1)..=buf_idx {
            out.push('━');
        }
    }

    // 4. 未下载段（暗灰细线轨）
    out.push_str(C_TEXT_DIM);
    for _ in (buf_idx + 1)..rail_len {
        out.push('─');
    }
    out.push_str(C_RESET);
    out.push_str(C_BAR_BG);
}

/// 辅助计算含 ANSI 颜色转义序列的文本在终端中的实际可见字符宽度
fn visible_len(s: &str) -> usize {
    let mut in_escape = false;
    let mut count = 0;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            count += 1;
        }
    }
    count
}

/// 格式化时间戳为 MM:SS 或 HH:MM:SS
pub fn format_duration(seconds: f64) -> String {
    let secs = seconds.max(0.0) as u64;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osd_hud_rendering() {
        let mut state = OsdState::new(RenderMode::Kitty, 0.85, false);
        state.total_secs = 7954.0;
        state.current_secs = 754.0;
        state.fps = 60.0;

        // 1. 静默状态应当返回 None
        assert!(state.render(100, 30).is_none());

        // 2. 快进触发
        state.trigger_seek(10);
        let seek_hud = state.render(100, 30).expect("Seek HUD should render");
        assert!(seek_hud.contains("▶▶ +10s"));
        assert!(seek_hud.contains("12:34"));
        assert!(state.is_visible);

        // 3. 音量调节触发
        state.trigger_volume(0.05);
        let vol_hud = state.render(100, 30).expect("Volume HUD should render");
        assert!(vol_hud.contains("85%"));
        assert!(vol_hud.contains("▲ +5%"));

        // 4. 暂停触发
        state.trigger_pause(true);
        let pause_hud = state.render(100, 30).expect("Pause HUD should render");
        assert!(pause_hud.contains("PAUSED"));

        // 5. 详细诊断单行面板触发（Tab）
        state.toggle_stats();
        let stats_hud = state.render(100, 30).expect("Stats HUD should render");
        assert!(stats_hud.contains("termpv"));
        assert!(stats_hud.contains("Kitty"));

        // 6. 恢复播放并测试 Tab 关闭后的彻底擦除
        state.trigger_pause(false);
        state.active_popup = None; // 模拟已无瞬态浮层
        state.toggle_stats(); // 关闭 Tab stats
        assert!(!state.show_stats);

        // 关闭后的首帧必须返回整行彻底擦除指令并转为隐藏状态
        let clear_hud = state.render(100, 30).expect("Clear HUD command should be returned on close");
        assert!(clear_hud.contains("\x1b[2K"));
        assert!(!state.is_visible);

        // 7. 再次渲染应完全静默无输出（零开销纯净观影）
        assert!(state.render(100, 30).is_none());

        // 8. 验证所有 HUD 绝对不含换行符（防止终端意外卷屏）
        state.toggle_stats();
        let s = state.render(100, 30).unwrap();
        assert!(!s.contains('\n') && !s.contains('\r'), "OSD HUD must never contain newlines");

        state.trigger_seek(5);
        let s = state.render(100, 30).unwrap();
        assert!(!s.contains('\n') && !s.contains('\r'), "Seek HUD must never contain newlines");

        state.trigger_mute(true);
        let s = state.render(100, 30).unwrap();
        assert!(!s.contains('\n') && !s.contains('\r'), "Mute HUD must never contain newlines");
    }

    #[test]
    fn test_popup_expiration_erases_line() {
        let mut state = OsdState::new(RenderMode::Kitty, 1.0, false);
        state.trigger_seek(5);
        assert!(state.render(80, 24).is_some());
        assert!(state.is_visible);

        // 模拟时间流逝导致浮层过期
        if let Some(popup) = &mut state.active_popup {
            popup.created_at = Instant::now() - Duration::from_secs(10);
        }

        // 过期后的第一帧必须发出清屏指令
        let clear = state.render(80, 24).expect("Must return clear command on popup expiration");
        assert!(clear.contains("\x1b[2K"));
        assert!(!state.is_visible);

        // 随后帧恢复静默
        assert!(state.render(80, 24).is_none());
    }

    #[test]
    fn test_three_tier_progress_bar_and_cache_indicator() {
        let mut state = OsdState::new(RenderMode::Kitty, 1.0, false);
        state.total_secs = 1000.0;
        state.current_secs = 200.0;
        state.buffered_secs = 150.0;
        state.buffered_bytes = 45 * 1024 * 1024;
        state.toggle_stats();

        let s = state.render(120, 30).unwrap();
        // 应该包含缓存指示器
        assert!(s.contains("📥 150.0s (45MB)"));
        // 应该包含三段式缓冲颜色
        assert!(s.contains(C_BUFFERED));
    }
}

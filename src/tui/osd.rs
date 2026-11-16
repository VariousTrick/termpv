// 屏幕状态显示（OSD, On-Screen Display）渲染组件
use crate::config::RenderMode;
use std::fmt::Write as FmtWrite;

/// OSD 所需的播放状态元数据
#[derive(Debug, Clone)]
pub struct OsdState {
    pub visible: bool,
    pub paused: bool,
    pub current_secs: f64,
    pub total_secs: f64,
    pub volume: f32,
    pub muted: bool,
    pub mode: RenderMode,
    pub fps: f64,
    pub dropped_frames: u64,
}

impl Default for OsdState {
    fn default() -> Self {
        Self {
            visible: true,
            paused: false,
            current_secs: 0.0,
            total_secs: 0.0,
            volume: 1.0,
            muted: false,
            mode: RenderMode::Auto,
            fps: 0.0,
            dropped_frames: 0,
        }
    }
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

/// 构建单行紧凑 OSD 文本行
pub fn render_osd_bar(state: &OsdState, term_width: u16) -> String {
    let mut bar = String::with_capacity(term_width as usize + 32);

    let current_str = format_duration(state.current_secs);
    let total_str = format_duration(state.total_secs);
    let status_str = if state.paused { "⏸ PAUSE" } else { "▶ PLAY " };
    let vol_str = if state.muted {
        "🔇 MUTE".to_string()
    } else {
        format!("🔊 {:.0}%", state.volume * 100.0)
    };
    let mode_str = format!("{:?}", state.mode);

    // 状态前缀
    let _ = write!(
        bar,
        " {status_str} | {current_str} / {total_str} | {vol_str} | {mode_str} | {:.0}FPS ",
        state.fps
    );

    // 计算可用进度条宽度
    let prefix_len = bar.chars().count();
    let reserved = prefix_len + 4;
    if (term_width as usize) <= reserved {
        return bar;
    }

    let progress_width = (term_width as usize) - reserved;
    let ratio = if state.total_secs > 0.0 {
        (state.current_secs / state.total_secs).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let filled = (progress_width as f64 * ratio).round() as usize;
    let empty = progress_width.saturating_sub(filled);

    bar.push('[');
    for _ in 0..filled {
        bar.push('█');
    }
    for _ in 0..empty {
        bar.push('░');
    }
    bar.push(']');

    bar
}

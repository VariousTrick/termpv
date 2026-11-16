// 终端生命周期与 RAII 保护守卫
use crate::error::Result;
use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use std::io::{stdout, Write};
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

// 标记终端是否已被劫持接管
static TERMINAL_CLAIMED: AtomicBool = AtomicBool::new(false);

/// 终端状态守卫（RAII 模式）
///
/// 创建时自动切入备用屏幕、开启原始输入模式（Raw Mode）并隐藏光标。
/// 当离开作用域或遭遇 Panic 时，自动无损恢复用户的终端状态。
pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    /// 激活终端接管并设置崩溃防护钩子
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;

        let mut out = stdout();
        // \x1b[?7l 禁用终端自动折行（防止边缘字符引发额外换行和滚屏）
        execute!(out, EnterAlternateScreen, Hide)?;
        out.write_all(b"\x1b[?7l")?;
        out.flush()?;

        TERMINAL_CLAIMED.store(true, Ordering::SeqCst);
        Self::install_panic_hook();

        Ok(Self { active: true })
    }

    /// 恢复终端默认状态的内部逻辑
    fn restore_terminal() {
        if !TERMINAL_CLAIMED.swap(false, Ordering::SeqCst) {
            return;
        }

        let mut out = stdout();
        // 解除终端同步刷新锁定（防止崩溃或异常退出时终端画面被锁住）
        let _ = out.write_all(b"\x1b[?2026l");
        // 清除终端可能残留的 Kitty 图形协议纹理
        let _ = out.write_all(b"\x1b_Ga=d,d=a,q=2\x1b\\");
        // \x1b[?7h 恢复终端自动换行
        let _ = out.write_all(b"\x1b[?7h");
        let _ = execute!(out, Show, LeaveAlternateScreen);
        let _ = out.flush();
        let _ = disable_raw_mode();
    }

    /// 安装全局 Panic 钩子，防止异常退出导致终端光标丢失或乱码
    fn install_panic_hook() {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            TerminalGuard::restore_terminal();
            default_hook(panic_info);
        }));
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        Self::restore_terminal();
        self.active = false;
    }
}

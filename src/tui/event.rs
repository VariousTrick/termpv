// 终端键盘输入与交互动作映射模块
use crate::error::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

/// 播放器受支持的用户动作指令
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerAction {
    /// 暂停 / 播放切换
    TogglePause,
    /// 跳转进度（单位：秒，正数为快进，负数为快退）
    Seek(i64),
    /// 音量微调（正数为增大，负数为减小）
    AdjustVolume(f32),
    /// 静音切换
    ToggleMute,
    /// 切换渲染模式（Kitty -> HalfBlock -> Ascii 轮转）
    CycleRenderMode,
    /// 切换 OSD 信息显示
    ToggleOsd,
    /// 退出播放
    Quit,
    /// 无需响应的按键
    None,
}

/// 非阻塞轮询并排空当前所有待处理按键事件，防止按键积压卡死
pub fn drain_actions(initial_timeout: Duration) -> Result<Vec<PlayerAction>> {
    let mut actions = Vec::new();

    // 先用初始超时等待首个事件（如无事件立即返回）
    if !event::poll(initial_timeout)? {
        return Ok(actions);
    }

    // 循环排空所有积压事件
    while event::poll(Duration::ZERO)? {
        let Event::Key(key_event) = event::read()? else {
            continue;
        };

        let action = map_key_to_action(key_event);
        if action != PlayerAction::None {
            actions.push(action);
        }
    }

    Ok(actions)
}

/// 将键盘事件扁平映射为业务动作，避免深层分支嵌套
fn map_key_to_action(key: KeyEvent) -> PlayerAction {
    // 捕获 Ctrl+C 中断信号
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return PlayerAction::Quit;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => PlayerAction::Quit,
        KeyCode::Char(' ') => PlayerAction::TogglePause,
        KeyCode::Right => PlayerAction::Seek(5),
        KeyCode::Left => PlayerAction::Seek(-5),
        KeyCode::Up => PlayerAction::Seek(60),
        KeyCode::Down => PlayerAction::Seek(-60),
        KeyCode::Char('l') => PlayerAction::Seek(10),
        KeyCode::Char('h') => PlayerAction::Seek(-10),
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('0') => PlayerAction::AdjustVolume(0.05),
        KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Char('9') => PlayerAction::AdjustVolume(-0.05),
        KeyCode::Char('x') => PlayerAction::ToggleMute,
        KeyCode::Char('m') => PlayerAction::CycleRenderMode,
        KeyCode::Char('o') | KeyCode::Tab => PlayerAction::ToggleOsd,
        _ => PlayerAction::None,
    }
}

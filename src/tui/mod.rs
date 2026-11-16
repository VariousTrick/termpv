// TUI 终端交互与界面子模块
pub mod event;
pub mod guard;
pub mod osd;

pub use event::{drain_actions, PlayerAction};
pub use guard::TerminalGuard;
pub use osd::{render_osd_bar, OsdState};

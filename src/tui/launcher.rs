// 交互式 TUI 启动器菜单与媒体选择器
use crate::error::Result;
use crate::tui::guard::TerminalGuard;
use crate::tui::history::{HistoryItem, HistoryManager};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;
use std::fs;
use std::io::{stdout, BufWriter, Write};
use std::path::{Path, PathBuf};

/// 支持的常见媒体扩展名
const MEDIA_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v", "wmv", "mp3", "flac", "wav", "aac",
    "m4a", "ogg", "opus",
];

/// 启动目标定义
#[derive(Debug, Clone)]
pub struct LaunchTarget {
    pub url_or_path: String,
    pub start_pts: Option<f64>,
}

/// 启动器子界面枚举
enum LauncherScreen {
    MainMenu {
        selected: usize,
    },
    NetworkInput {
        input: String,
        err_msg: Option<String>,
    },
    FileBrowser {
        current_dir: PathBuf,
        entries: Vec<DirEntryItem>,
        selected: usize,
        scroll: usize,
    },
    History {
        items: Vec<HistoryItem>,
        selected: usize,
        scroll: usize,
    },
}

#[derive(Debug, Clone)]
struct DirEntryItem {
    is_dir: bool,
    is_parent: bool,
    name: String,
    path: PathBuf,
    size_str: String,
}

pub struct Launcher {
    screen: LauncherScreen,
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            screen: LauncherScreen::MainMenu { selected: 0 },
        }
    }

    /// 运行 TUI 交互式启动器主事件循环
    pub fn run(&mut self) -> Result<Option<LaunchTarget>> {
        let _guard = TerminalGuard::new()?;
        let mut out = BufWriter::with_capacity(32 * 1024, stdout());

        loop {
            let (term_w, term_h) = terminal::size()?;
            self.render(&mut out, term_w, term_h)?;
            out.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(None);
                }

                if let Some(target) = self.handle_key(key)? {
                    return Ok(Some(target));
                }

                if let LauncherScreen::MainMenu { .. } = self.screen {
                    if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                        return Ok(None);
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<LaunchTarget>> {
        match &mut self.screen {
            LauncherScreen::MainMenu { selected } => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *selected = if *selected == 0 { 3 } else { *selected - 1 };
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (*selected + 1) % 4;
                    }
                    KeyCode::Char('1') => return self.switch_screen(0),
                    KeyCode::Char('2') => return self.switch_screen(1),
                    KeyCode::Char('3') => return self.switch_screen(2),
                    KeyCode::Char('4') => return Ok(None),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let cur = *selected;
                        return self.switch_screen(cur);
                    }
                    _ => {}
                }
            }
            LauncherScreen::NetworkInput { input, err_msg } => {
                match key.code {
                    KeyCode::Esc => {
                        self.screen = LauncherScreen::MainMenu { selected: 0 };
                    }
                    KeyCode::Enter => {
                        let trimmed = input.trim();
                        if trimmed.is_empty() {
                            *err_msg = Some("请输入有效的媒体流 URL 或文件路径".into());
                        } else {
                            return Ok(Some(LaunchTarget {
                                url_or_path: trimmed.to_string(),
                                start_pts: None,
                            }));
                        }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        *err_msg = None;
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        input.clear();
                        *err_msg = None;
                    }
                    KeyCode::Char(c) => {
                        input.push(c);
                        *err_msg = None;
                    }
                    _ => {}
                }
            }
            LauncherScreen::FileBrowser {
                current_dir,
                entries,
                selected,
                scroll,
            } => {
                let count = entries.len();
                match key.code {
                    KeyCode::Esc => {
                        self.screen = LauncherScreen::MainMenu { selected: 1 };
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if count > 0 {
                            *selected = if *selected == 0 { count - 1 } else { *selected - 1 };
                            if *selected < *scroll {
                                *scroll = *selected;
                            } else if *selected >= *scroll + 12 {
                                *scroll = selected.saturating_sub(11);
                            }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if count > 0 {
                            *selected = (*selected + 1) % count;
                            if *selected >= *scroll + 12 {
                                *scroll = selected.saturating_sub(11);
                            } else if *selected < *scroll {
                                *scroll = *selected;
                            }
                        }
                    }
                    KeyCode::Backspace | KeyCode::Left => {
                        if let Some(parent) = current_dir.parent() {
                            let parent_buf = parent.to_path_buf();
                            let new_entries = Self::read_directory(&parent_buf);
                            *current_dir = parent_buf;
                            *entries = new_entries;
                            *selected = 0;
                            *scroll = 0;
                        }
                    }
                    KeyCode::Enter | KeyCode::Right => {
                        if count > 0 && *selected < count {
                            let item = &entries[*selected];
                            if item.is_dir {
                                let new_dir = item.path.clone();
                                let new_entries = Self::read_directory(&new_dir);
                                *current_dir = new_dir;
                                *entries = new_entries;
                                *selected = 0;
                                *scroll = 0;
                            } else {
                                return Ok(Some(LaunchTarget {
                                    url_or_path: item.path.to_string_lossy().to_string(),
                                    start_pts: None,
                                }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            LauncherScreen::History {
                items,
                selected,
                scroll,
            } => {
                let count = items.len();
                match key.code {
                    KeyCode::Esc => {
                        self.screen = LauncherScreen::MainMenu { selected: 2 };
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if count > 0 {
                            *selected = if *selected == 0 { count - 1 } else { *selected - 1 };
                            if *selected < *scroll {
                                *scroll = *selected;
                            } else if *selected >= *scroll + 6 {
                                *scroll = selected.saturating_sub(5);
                            }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if count > 0 {
                            *selected = (*selected + 1) % count;
                            if *selected >= *scroll + 6 {
                                *scroll = selected.saturating_sub(5);
                            } else if *selected < *scroll {
                                *scroll = *selected;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if count > 0 && *selected < count {
                            let item = &items[*selected];
                            return Ok(Some(LaunchTarget {
                                url_or_path: item.url_or_path.clone(),
                                start_pts: Some(item.last_pts),
                            }));
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Delete => {
                        if count > 0 && *selected < count {
                            let _ = HistoryManager::remove(*selected);
                            *items = HistoryManager::load();
                            if *selected >= items.len() && !items.is_empty() {
                                *selected = items.len() - 1;
                            }
                        }
                    }
                    KeyCode::Char('c') => {
                        let _ = HistoryManager::clear();
                        items.clear();
                        *selected = 0;
                        *scroll = 0;
                    }
                    _ => {}
                }
            }
        }
        Ok(None)
    }

    fn switch_screen(&mut self, menu_idx: usize) -> Result<Option<LaunchTarget>> {
        match menu_idx {
            0 => {
                self.screen = LauncherScreen::NetworkInput {
                    input: String::new(),
                    err_msg: None,
                };
            }
            1 => {
                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let entries = Self::read_directory(&current_dir);
                self.screen = LauncherScreen::FileBrowser {
                    current_dir,
                    entries,
                    selected: 0,
                    scroll: 0,
                };
            }
            2 => {
                let items = HistoryManager::load();
                self.screen = LauncherScreen::History {
                    items,
                    selected: 0,
                    scroll: 0,
                };
            }
            3 => return Ok(None),
            _ => {}
        }
        Ok(None)
    }

    fn read_directory(dir: &Path) -> Vec<DirEntryItem> {
        let mut result = Vec::new();

        if let Some(parent) = dir.parent() {
            result.push(DirEntryItem {
                is_dir: true,
                is_parent: true,
                name: ".. (返回上一级目录)".into(),
                path: parent.to_path_buf(),
                size_str: "-".into(),
            });
        }

        let Ok(read_dir) = fs::read_dir(dir) else {
            return result;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // 忽略隐藏文件
            if name.starts_with('.') {
                continue;
            }

            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    dirs.push(DirEntryItem {
                        is_dir: true,
                        is_parent: false,
                        name: format!("{name}/"),
                        path,
                        size_str: "<目录>".into(),
                    });
                } else if ft.is_file() {
                    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                        if MEDIA_EXTS.contains(&ext.to_lowercase().as_str()) {
                            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            let size_str = format_bytes(size);
                            files.push(DirEntryItem {
                                is_dir: false,
                                is_parent: false,
                                name,
                                path,
                                size_str,
                            });
                        }
                    }
                }
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        result.extend(dirs);
        result.extend(files);
        result
    }

    fn render<W: Write>(&self, out: &mut W, term_w: u16, term_h: u16) -> Result<()> {
        write!(out, "\x1b[2J\x1b[H")?;

        match &self.screen {
            LauncherScreen::MainMenu { selected } => {
                self.render_main_menu(out, *selected, term_w, term_h)?;
            }
            LauncherScreen::NetworkInput { input, err_msg } => {
                self.render_network_input(out, input, err_msg.as_deref(), term_w)?;
            }
            LauncherScreen::FileBrowser {
                current_dir,
                entries,
                selected,
                scroll,
            } => {
                self.render_file_browser(out, current_dir, entries, *selected, *scroll, term_w, term_h)?;
            }
            LauncherScreen::History {
                items,
                selected,
                scroll,
            } => {
                self.render_history(out, items, *selected, *scroll, term_w, term_h)?;
            }
        }

        Ok(())
    }

    fn render_main_menu<W: Write>(
        &self,
        out: &mut W,
        selected: usize,
        term_w: u16,
        _term_h: u16,
    ) -> Result<()> {
        let logo = [
            r"  ████████╗███████╗██████╗ ███╗   ███╗██████╗ ██╗   ██╗",
            r"  ╚══██╔══╝██╔════╝██╔══██╗████╗ ████║██╔══██╗██║   ██║",
            r"     ██║   █████╗  ██████╔╝██╔████╔██║██████╔╝██║   ██║",
            r"     ██║   ██╔══╝  ██╔══██╗██║╚██╔╝██║██╔═══╝ ╚██╗ ██╔╝",
            r"     ██║   ███████╗██║  ██║██║ ╚═╝ ██║██║      ╚████╔╝ ",
            r"     ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝╚═╝       ╚═══╝  ",
        ];

        writeln!(out)?;
        for line in &logo {
            writeln!(out, "\x1b[38;2;0;215;215;1m{line}\x1b[0m")?;
        }

        writeln!(
            out,
            "\x1b[38;2;246;193;119m       高性能现代化终端媒体播放器 (termpv v0.3.5)\x1b[0m\n"
        )?;

        let options = [
            (
                "🌐  播放网络流媒体",
                "支持 HTTP / HTTPS / PikPak 网页直链 / RTSP 流媒体等",
            ),
            (
                "📁  浏览本地媒体文件",
                "方向键交互式选择本地目录视频/音频并一键直放",
            ),
            (
                "🕒  最近播放历史记录",
                "查看近期播放进度，支持一键无缝断点续播",
            ),
            (
                "🚪  退出播放器",
                "退出 termpv 应用程序",
            ),
        ];

        writeln!(
            out,
            "\x1b[38;2;140;150;170m  请使用方向键选择操作，按 Enter 确认：\x1b[0m\n"
        )?;

        for (i, (title, desc)) in options.iter().enumerate() {
            if i == selected {
                writeln!(
                    out,
                    "  \x1b[48;2;0;100;140;38;2;255;255;255;1m ▶ [{}] {:<24} \x1b[0m  \x1b[38;2;180;210;235m{}\x1b[0m",
                    i + 1,
                    title,
                    desc
                )?;
            } else {
                writeln!(
                    out,
                    "    \x1b[38;2;200;210;220m[{}] {:<24}\x1b[0m  \x1b[38;2;110;120;140m{}\x1b[0m",
                    i + 1,
                    title,
                    desc
                )?;
            }
            writeln!(out)?;
        }

        let sep_len = (term_w as usize).clamp(40, 72);
        let sep: String = "─".repeat(sep_len);
        writeln!(out, "  \x1b[38;2;80;90;110m{sep}\x1b[0m")?;
        writeln!(
            out,
            "  \x1b[38;2;130;140;160m快捷操作: [↑/↓ 或 j/k] 选择  [Enter] 确认  [1-4] 快捷跳转  [q/Esc] 退出\x1b[0m"
        )?;

        Ok(())
    }

    fn render_network_input<W: Write>(
        &self,
        out: &mut W,
        input: &str,
        err_msg: Option<&str>,
        term_w: u16,
    ) -> Result<()> {
        writeln!(out)?;
        writeln!(
            out,
            "  \x1b[38;2;0;215;215;1m🌐 播放网络流媒体\x1b[0m"
        )?;
        writeln!(
            out,
            "  \x1b[38;2;140;150;170m请输入或直接粘贴媒体流链接（支持 PikPak、HTTP(S)、RTSP 等）：\x1b[0m\n"
        )?;

        let box_w = (term_w as usize).saturating_sub(6).clamp(30, 90);
        let h_line: String = "─".repeat(box_w);

        writeln!(out, "  \x1b[38;2;90;140;175m┌{}┐\x1b[0m", h_line)?;

        let inner_w = box_w.saturating_sub(4);
        let display_input = if input.chars().count() > inner_w {
            let skip_chars = input.chars().count().saturating_sub(inner_w);
            format!("...{}", input.chars().skip(skip_chars + 3).collect::<String>())
        } else {
            input.to_string()
        };

        writeln!(
            out,
            "  \x1b[38;2;90;140;175m│\x1b[0m  \x1b[38;2;255;255;255;1m{:<inner_w$}\x1b[38;2;0;215;215;5m█\x1b[0m \x1b[38;2;90;140;175m│\x1b[0m",
            display_input,
            inner_w = inner_w
        )?;

        writeln!(out, "  \x1b[38;2;90;140;175m└{}┘\x1b[0m\n", h_line)?;

        if let Some(err) = err_msg {
            writeln!(out, "  \x1b[38;2;255;85;85;1m⚠ {err}\x1b[0m\n")?;
        }

        writeln!(
            out,
            "  \x1b[38;2;130;140;160m[Enter] 开始解析播放   [Ctrl+U] 清空输入   [Esc] 返回主菜单\x1b[0m"
        )?;

        Ok(())
    }

    fn render_file_browser<W: Write>(
        &self,
        out: &mut W,
        current_dir: &Path,
        entries: &[DirEntryItem],
        selected: usize,
        scroll: usize,
        term_w: u16,
        term_h: u16,
    ) -> Result<()> {
        writeln!(out)?;
        writeln!(
            out,
            "  \x1b[38;2;0;215;215;1m📁 本地媒体文件浏览器\x1b[0m"
        )?;
        writeln!(
            out,
            "  \x1b[38;2;140;150;170m当前路径: \x1b[38;2;246;193;119;1m{}\x1b[0m\n",
            current_dir.display()
        )?;

        let visible_lines = (term_h as usize).saturating_sub(10).clamp(6, 16);
        let end = (scroll + visible_lines).min(entries.len());

        if entries.is_empty() {
            writeln!(
                out,
                "  \x1b[38;2;130;140;160m(当前目录下未发现支持的视频/音频媒体文件)\x1b[0m\n"
            )?;
        } else {
            for i in scroll..end {
                let item = &entries[i];
                let is_sel = i == selected;

                let icon = if item.is_parent {
                    "📁 .."
                } else if item.is_dir {
                    "📁"
                } else {
                    "🎬"
                };

                let max_name_w = (term_w as usize).saturating_sub(30).clamp(20, 60);
                let name_truncated = if item.name.chars().count() > max_name_w {
                    format!("{}...", item.name.chars().take(max_name_w - 3).collect::<String>())
                } else {
                    item.name.clone()
                };

                if is_sel {
                    writeln!(
                        out,
                        "  \x1b[48;2;0;100;140;38;2;255;255;255;1m ▶ {} {:<max_name_w$} \x1b[0m \x1b[38;2;246;193;119m{:>10}\x1b[0m",
                        icon,
                        name_truncated,
                        item.size_str,
                        max_name_w = max_name_w
                    )?;
                } else {
                    let color = if item.is_dir {
                        "\x1b[38;2;130;185;235m"
                    } else {
                        "\x1b[38;2;220;225;235m"
                    };
                    writeln!(
                        out,
                        "     {} {}{:<max_name_w$}\x1b[0m \x1b[38;2;130;140;155m{:>10}\x1b[0m",
                        icon,
                        color,
                        name_truncated,
                        item.size_str,
                        max_name_w = max_name_w
                    )?;
                }
            }
        }

        writeln!(out)?;
        let sep_len = (term_w as usize).clamp(40, 72);
        let sep: String = "─".repeat(sep_len);
        writeln!(out, "  \x1b[38;2;80;90;110m{sep}\x1b[0m")?;
        writeln!(
            out,
            "  \x1b[38;2;130;140;160m[↑/↓] 选择   [Enter] 打开目录/播放   [Backspace/←] 返回上级   [Esc] 返回菜单\x1b[0m"
        )?;

        Ok(())
    }

    fn render_history<W: Write>(
        &self,
        out: &mut W,
        items: &[HistoryItem],
        selected: usize,
        scroll: usize,
        term_w: u16,
        term_h: u16,
    ) -> Result<()> {
        writeln!(out)?;
        writeln!(
            out,
            "  \x1b[38;2;0;215;215;1m🕒 最近播放历史记录\x1b[0m"
        )?;
        writeln!(
            out,
            "  \x1b[38;2;140;150;170m选择历史条目按 Enter 即可以上次的断点进度继续播放：\x1b[0m\n"
        )?;

        if items.is_empty() {
            writeln!(
                out,
                "  \x1b[38;2;130;140;160m(暂无播放历史记录，播放本地文件或网络视频后将自动保存)\x1b[0m\n"
            )?;
        } else {
            let visible_items = (term_h as usize).saturating_sub(11).clamp(3, 8);
            let end = (scroll + visible_items).min(items.len());

            for i in scroll..end {
                let item = &items[i];
                let is_sel = i == selected;

                let icon = if item.url_or_path.starts_with("http") {
                    "🌐"
                } else {
                    "🎬"
                };

                let max_title_w = (term_w as usize).saturating_sub(20).clamp(25, 65);
                let title_truncated = if item.title.chars().count() > max_title_w {
                    format!("{}...", item.title.chars().take(max_title_w - 3).collect::<String>())
                } else {
                    item.title.clone()
                };

                let pts_str = HistoryItem::format_duration(item.last_pts);
                let dur_str = HistoryItem::format_duration(item.total_secs);
                let pct = item.progress_pct();
                let rel_time = item.format_relative_time();

                if is_sel {
                    writeln!(
                        out,
                        "  \x1b[48;2;0;100;140;38;2;255;255;255;1m ▶ [{}] {} {} \x1b[0m",
                        i + 1,
                        icon,
                        title_truncated
                    )?;
                    writeln!(
                        out,
                        "      \x1b[38;2;246;193;119m进度: [{} / {}] ({:.0}%)  •  {}\x1b[0m",
                        pts_str,
                        dur_str,
                        pct,
                        rel_time
                    )?;
                } else {
                    writeln!(
                        out,
                        "     [{}] {} \x1b[38;2;220;225;235;1m{}\x1b[0m",
                        i + 1,
                        icon,
                        title_truncated
                    )?;
                    writeln!(
                        out,
                        "      \x1b[38;2;120;130;150m进度: [{} / {}] ({:.0}%)  •  {}\x1b[0m",
                        pts_str,
                        dur_str,
                        pct,
                        rel_time
                    )?;
                }
                writeln!(out)?;
            }
        }

        let sep_len = (term_w as usize).clamp(40, 72);
        let sep: String = "─".repeat(sep_len);
        writeln!(out, "  \x1b[38;2;80;90;110m{sep}\x1b[0m")?;
        writeln!(
            out,
            "  \x1b[38;2;130;140;160m[Enter] 断点续播   [d] 删除此项   [c] 清空历史   [Esc] 返回主菜单\x1b[0m"
        )?;

        Ok(())
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

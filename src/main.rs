// termpv 应用程序主入口
mod app;
mod audio;
mod config;
mod core;
mod error;
mod logger;
mod render;
mod tui;

use app::App;
use clap::Parser;
use config::Config;
use std::path::PathBuf;
use std::process;
use tui::{HistoryManager, Launcher};

fn main() {
    let config = Config::parse();

    // 检查是否开启调试日志
    if config.debug || config.log_file.is_some() {
        let log_path = config
            .log_file
            .clone()
            .unwrap_or_else(|| PathBuf::from("termpv.log"));
        let _ = logger::init_logger(log_path);
        log::info!("termpv 启动，文件: {:?}", config.file);
    }

    if let Some(file) = &config.file {
        // 命令行显式指定文件/URL，直接启动播放
        let url_or_path = file.to_string_lossy().to_string();
        play_stream(config, &url_or_path);
    } else {
        // 未指定参数：进入交互式 TUI 启动器菜单
        let mut launcher = Launcher::new();
        loop {
            match launcher.run() {
                Ok(Some(target)) => {
                    let mut run_cfg = config.clone();
                    run_cfg.file = Some(PathBuf::from(&target.url_or_path));
                    if let Some(pts) = target.start_pts {
                        run_cfg.start = Some(pts);
                    }
                    play_stream(run_cfg, &target.url_or_path);
                }
                Ok(None) => {
                    // 用户选择退出
                    break;
                }
                Err(err) => {
                    eprintln!("\x1b[31;1m[termpv 启动器错误]\x1b[0m {err}");
                    process::exit(1);
                }
            }
        }
    }
}

fn play_stream(config: Config, url_or_path: &str) {
    let mut app = App::new(config);
    match app.run() {
        Ok((last_pts, duration)) => {
            HistoryManager::record(url_or_path, last_pts, duration);
        }
        Err(err) => {
            log::error!("播放器运行致命错误: {err}");
            eprintln!("\x1b[31;1m[termpv 运行错误]\x1b[0m {err}");
        }
    }
}

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

    let mut app = App::new(config);
    if let Err(err) = app.run() {
        log::error!("播放器运行致命错误: {err}");
        eprintln!("\x1b[31;1m[termpv 运行错误]\x1b[0m {err}");
        process::exit(1);
    }
}

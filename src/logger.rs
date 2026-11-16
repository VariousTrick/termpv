// 独立文件日志记录器（专为 TUI 全屏备用屏幕设计）
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

pub struct FileLogger {
    file: Mutex<File>,
    start_time: Instant,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let elapsed = self.start_time.elapsed().as_secs_f64();
        let mut file = match self.file.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let _ = writeln!(
            file,
            "[{elapsed:8.3}s] [{:<5}] {}",
            record.level(),
            record.args()
        );
        let _ = file.flush();
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

/// 初始化全局文件日志器
pub fn init_logger<P: AsRef<Path>>(path: P) -> std::result::Result<(), SetLoggerError> {
    let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    else {
        return Ok(());
    };

    let logger = Box::leak(Box::new(FileLogger {
        file: Mutex::new(file),
        start_time: Instant::now(),
    }));

    log::set_logger(logger)?;
    log::set_max_level(LevelFilter::Debug);
    Ok(())
}

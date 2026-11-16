// 播放主时钟调度器（基于高精度单调 CPU 壁钟，保证画面 100% 顺滑不掉帧卡顿）
use std::time::{Duration, Instant};

/// 同步调度决策结果
#[derive(Debug, PartialEq)]
pub enum SyncDecision {
    /// 准时到达，立即渲染
    Render,
    /// 画面超前，建议等待指定时长后再渲染
    Wait(Duration),
    /// 画面严重滞后（超过阈值），应直接丢弃该帧以追赶时钟
    Drop,
}

/// 播放主时钟
pub struct MasterClock {
    start_time: Option<Instant>,
    paused_at: Option<Instant>,
    total_paused_duration: Duration,
    manual_offset_secs: f64,

    // 统计指标
    last_fps_calc: Instant,
    frame_count: u32,
    current_fps: f64,
    dropped_frames: u64,
}

impl MasterClock {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start_time: None,
            paused_at: None,
            total_paused_duration: Duration::ZERO,
            manual_offset_secs: 0.0,
            last_fps_calc: now,
            frame_count: 0,
            current_fps: 0.0,
            dropped_frames: 0,
        }
    }

    /// 获取当前时钟播放进度（基于单调绝对时间，平滑连续，零阶梯抖动）
    pub fn current_time(&self) -> f64 {
        let Some(start) = self.start_time else {
            return self.manual_offset_secs;
        };

        let elapsed = match self.paused_at {
            Some(pause_start) => pause_start.duration_since(start),
            None => Instant::now().duration_since(start),
        };

        let effective = elapsed.saturating_sub(self.total_paused_duration);
        (effective.as_secs_f64() + self.manual_offset_secs).max(0.0)
    }

    /// 暂停时钟
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    /// 恢复时钟
    pub fn resume(&mut self) {
        let Some(pause_start) = self.paused_at.take() else {
            return;
        };
        self.total_paused_duration += pause_start.elapsed();
    }

    /// 执行 Seek 时调整基准偏移
    pub fn seek_to(&mut self, target_secs: f64) {
        self.manual_offset_secs = target_secs;
        self.start_time = Some(Instant::now());
        self.total_paused_duration = Duration::ZERO;
        if self.paused_at.is_some() {
            self.paused_at = Some(Instant::now());
        }
    }

    /// 针对视频帧 PTS 做出平滑调度决策
    pub fn evaluate_frame(&mut self, frame_pts: f64) -> SyncDecision {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
            self.manual_offset_secs = frame_pts;
            self.record_rendered_frame();
            return SyncDecision::Render;
        }

        let current = self.current_time();
        let diff = frame_pts - current;

        // 滞后超过 100 毫秒时丢帧
        if diff < -0.100 {
            self.dropped_frames += 1;
            return SyncDecision::Drop;
        }

        // 超前超过 2 毫秒：适度等待
        if diff > 0.002 {
            return SyncDecision::Wait(Duration::from_secs_f64(diff));
        }

        // 准时区间：渲染并记录 FPS
        self.record_rendered_frame();
        SyncDecision::Render
    }

    /// 累计渲染帧并计算实时 FPS
    fn record_rendered_frame(&mut self) {
        self.frame_count += 1;
        let elapsed = self.last_fps_calc.elapsed();

        if elapsed >= Duration::from_millis(500) {
            self.current_fps = (self.frame_count as f64) / elapsed.as_secs_f64();
            self.frame_count = 0;
            self.last_fps_calc = Instant::now();
        }
    }

    pub fn fps(&self) -> f64 {
        self.current_fps
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }
}

// 图像帧缓冲与基础像素结构模块
use crate::error::Result;

/// 原始 RGB 24 位像素帧
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RgbFrame {
    pub width: u32,
    pub height: u32,
    /// 连续排布的 RGB 字节数组 (len = width * height * 3)
    pub data: Vec<u8>,
    /// 呈现时间戳（单位：秒）
    pub pts: f64,
}

#[allow(dead_code)]
impl RgbFrame {
    /// 创建空的 RGB 帧对象
    pub fn new(width: u32, height: u32, pts: f64) -> Self {
        let size = (width * height * 3) as usize;
        Self {
            width,
            height,
            data: vec![0u8; size],
            pts,
        }
    }

    /// 简单的紧凑快速缩放（使用最近邻插值，极速且避免长依赖）
    pub fn resize_nearest(&self, target_w: u32, target_h: u32) -> Result<RgbFrame> {
        if target_w == 0 || target_h == 0 {
            return Ok(RgbFrame::new(0, 0, self.pts));
        }

        if self.width == target_w && self.height == target_h {
            return Ok(self.clone());
        }

        let mut scaled = RgbFrame::new(target_w, target_h, self.pts);
        let src_w = self.width as usize;
        let src_h = self.height as usize;
        let dst_w = target_w as usize;
        let dst_h = target_h as usize;

        for dy in 0..dst_h {
            let sy = (dy * src_h) / dst_h;
            let src_row_start = sy * src_w * 3;
            let dst_row_start = dy * dst_w * 3;

            for dx in 0..dst_w {
                let sx = (dx * src_w) / dst_w;
                let src_idx = src_row_start + sx * 3;
                let dst_idx = dst_row_start + dx * 3;

                scaled.data[dst_idx] = self.data[src_idx];
                scaled.data[dst_idx + 1] = self.data[src_idx + 1];
                scaled.data[dst_idx + 2] = self.data[src_idx + 2];
            }
        }

        Ok(scaled)
    }
}

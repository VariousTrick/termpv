// 经典 ASCII 字符灰度矩阵渲染器（基于 Layout 绝对坐标输出与严格内存安全防线）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::fmt::Write as FmtWrite;
use std::io::Write;

/// 灰度字符阶梯
const ASCII_CHARS: &[u8] = b" .:-=+*#%@";

/// ASCII 字符渲染器
pub struct AsciiRenderer {
    scratch_buf: String,
}

impl AsciiRenderer {
    pub fn new() -> Self {
        Self {
            scratch_buf: String::with_capacity(256 * 1024),
        }
    }

    /// 将图像转换为带颜色的 ASCII 字符流（绝对行坐标输出）
    pub fn render<W: Write>(
        &mut self,
        frame: &RgbFrame,
        layout: &Layout,
        writer: &mut W,
    ) -> Result<()> {
        if layout.term_w == 0 || layout.term_h == 0 || frame.width == 0 || frame.height == 0 {
            return Ok(());
        }

        self.scratch_buf.clear();

        let start_row = (layout.pad_y as usize) + 1;
        let start_col = (layout.pad_x as usize) + 1;
        // 安全截断避免超越解码帧实际数据维度
        let cell_w = (layout.render_cols as usize).min(frame.width as usize);
        let cell_h = (layout.render_rows as usize).min(frame.height as usize);

        // 1. 顶部空行清除
        for r in 1..start_row {
            let _ = write!(self.scratch_buf, "\x1b[{r};1H\x1b[2K");
        }

        // 2. ASCII 主体渲染
        self.encode_ascii(frame, cell_w, cell_h, start_row, start_col);

        // 3. 底部空行清除
        let end_row = start_row + cell_h;
        for r in end_row..=(layout.term_h as usize) {
            let _ = write!(self.scratch_buf, "\x1b[{r};1H\x1b[2K");
        }

        writer.write_all(self.scratch_buf.as_bytes())?;
        Ok(())
    }

    fn encode_ascii(
        &mut self,
        frame: &RgbFrame,
        width: usize,
        height: usize,
        start_row: usize,
        start_col: usize,
    ) {
        let frame_w = frame.width as usize;
        let frame_len = frame.data.len();

        for y in 0..height {
            let row = start_row + y;
            let _ = write!(self.scratch_buf, "\x1b[{row};{start_col}H");
            let mut last_color: Option<(u8, u8, u8)> = None;

            let row_idx = y * frame_w * 3;

            for x in 0..width {
                let px_idx = row_idx + x * 3;
                if px_idx + 2 >= frame_len {
                    break;
                }

                let r = frame.data[px_idx];
                let g = frame.data[px_idx + 1];
                let b = frame.data[px_idx + 2];

                let luma = ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8;
                let char_idx = (luma as usize * (ASCII_CHARS.len() - 1)) / 255;
                let ch = ASCII_CHARS[char_idx] as char;

                let color = (r, g, b);
                if last_color != Some(color) {
                    let _ = write!(self.scratch_buf, "\x1b[38;2;{};{};{}m", r, g, b);
                    last_color = Some(color);
                }

                self.scratch_buf.push(ch);
            }
        }

        self.scratch_buf.push_str("\x1b[0m");
    }
}

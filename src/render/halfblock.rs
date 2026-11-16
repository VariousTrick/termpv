// 24位真彩色半块双像素（▀）渲染器（基于 Layout 绝对坐标定位，彻底杜绝换行滚屏与画面裁切）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::fmt::Write as FmtWrite;
use std::io::Write;

/// 半块像素渲染器
pub struct HalfBlockRenderer {
    scratch_buf: String,
}

impl HalfBlockRenderer {
    pub fn new() -> Self {
        Self {
            scratch_buf: String::with_capacity(128 * 1024),
        }
    }

    /// 将图像渲染为半块 ANSI 字符串（按 Layout 绝对坐标输出）
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
        let cell_w = layout.render_cols as usize;
        let cell_h = layout.render_rows as usize;

        // 1. 顶部留白空行清除（如果有）
        for r in 1..start_row {
            let _ = write!(self.scratch_buf, "\x1b[{r};1H\x1b[2K");
        }

        // 2. 主体半块渲染（绝对坐标行行跳转，绝不使用 \r\n，杜绝终端自动换行引发滚屏裁切）
        self.encode_halfblocks(frame, cell_w, cell_h, start_row, start_col);

        // 3. 底部留白空行清除（如果有）
        let end_row = start_row + cell_h;
        for r in end_row..=(layout.term_h as usize) {
            let _ = write!(self.scratch_buf, "\x1b[{r};1H\x1b[2K");
        }

        writer.write_all(self.scratch_buf.as_bytes())?;
        Ok(())
    }

    /// 核心 ANSI 半块编码循环（行绝对寻址与颜色去重）
    fn encode_halfblocks(
        &mut self,
        frame: &RgbFrame,
        width: usize,
        height: usize,
        start_row: usize,
        start_col: usize,
    ) {
        for cell_y in 0..height {
            let row = start_row + cell_y;
            // 行首直接跳转到指定居中列
            let _ = write!(self.scratch_buf, "\x1b[{row};{start_col}H");
            let mut last_fg: Option<(u8, u8, u8)> = None;
            let mut last_bg: Option<(u8, u8, u8)> = None;

            let top_y = cell_y * 2;
            let bot_y = top_y + 1;

            let top_row_idx = top_y * (frame.width as usize) * 3;
            let bot_row_idx = bot_y * (frame.width as usize) * 3;

            for cell_x in 0..width {
                let px_offset = cell_x * 3;
                let top_idx = top_row_idx + px_offset;
                let bot_idx = bot_row_idx + px_offset;

                let fg = (
                    frame.data[top_idx],
                    frame.data[top_idx + 1],
                    frame.data[top_idx + 2],
                );

                let bg = if bot_idx + 2 < frame.data.len() {
                    (
                        frame.data[bot_idx],
                        frame.data[bot_idx + 1],
                        frame.data[bot_idx + 2],
                    )
                } else {
                    fg
                };

                // 前景色变更时输出 ANSI
                if last_fg != Some(fg) {
                    let _ = write!(self.scratch_buf, "\x1b[38;2;{};{};{}m", fg.0, fg.1, fg.2);
                    last_fg = Some(fg);
                }

                // 背景色变更时输出 ANSI
                if last_bg != Some(bg) {
                    let _ = write!(self.scratch_buf, "\x1b[48;2;{};{};{}m", bg.0, bg.1, bg.2);
                    last_bg = Some(bg);
                }

                self.scratch_buf.push('▀');
            }
        }

        self.scratch_buf.push_str("\x1b[0m");
    }
}

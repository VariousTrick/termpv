// Kitty 终端图形协议渲染器（基于 Layout 精准居中对齐）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::fmt::Write as FmtWrite;
use std::io::Write;

/// Kitty 协议单次传输分块最大 Base64 字符数
const CHUNK_SIZE: usize = 4096;

/// Kitty 图形渲染器
pub struct KittyRenderer {
    scratch_buf: String,
}

impl KittyRenderer {
    pub fn new() -> Self {
        Self {
            scratch_buf: String::with_capacity(128 * 1024),
        }
    }

    /// 通过 Kitty 图形转义序列渲染原始 RGB 图像（按 Layout 精确居中）
    pub fn render<W: Write>(
        &mut self,
        frame: &RgbFrame,
        layout: &Layout,
        writer: &mut W,
    ) -> Result<()> {
        if frame.width == 0 || frame.height == 0 || layout.render_cols == 0 || layout.render_rows == 0 {
            return Ok(());
        }

        self.scratch_buf.clear();

        // 精确定位光标到居中矩形起始单元格
        let start_row = layout.pad_y + 1;
        let start_col = layout.pad_x + 1;
        let _ = write!(self.scratch_buf, "\x1b[{start_row};{start_col}H");

        let b64 = fast_base64_encode(&frame.data);
        self.chunk_and_transmit(
            &b64,
            frame.width,
            frame.height,
            layout.render_cols,
            layout.render_rows,
        );

        writer.write_all(self.scratch_buf.as_bytes())?;
        Ok(())
    }

    /// 将 Base64 数据分块发送，设置 q=2 静默模式防止终端回复污染输入流
    fn chunk_and_transmit(
        &mut self,
        b64: &str,
        img_w: u32,
        img_h: u32,
        cols: u16,
        rows: u16,
    ) {
        let total_len = b64.len();
        let mut offset = 0;
        let mut is_first = true;

        while offset < total_len {
            let end = (offset + CHUNK_SIZE).min(total_len);
            let chunk = &b64[offset..end];
            let is_last = end == total_len;
            let m_flag = if is_last { 0 } else { 1 };

            if is_first {
                // q=2: 静默模式，禁止终端返回控制响应
                let _ = write!(
                    self.scratch_buf,
                    "\x1b_Ga=T,f=24,s={img_w},v={img_h},c={cols},r={rows},m={m_flag},q=2;{chunk}\x1b\\"
                );
                is_first = false;
            } else {
                let _ = write!(self.scratch_buf, "\x1b_Gm={m_flag};{chunk}\x1b\\");
            }

            offset = end;
        }
    }
}

/// 高性能 Base64 编码器
fn fast_base64_encode(input: &[u8]) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);

    let chunks = input.chunks_exact(3);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let b0 = chunk[0] as usize;
        let b1 = chunk[1] as usize;
        let b2 = chunk[2] as usize;

        out.push(CHARSET[b0 >> 2] as char);
        out.push(CHARSET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(CHARSET[((b1 & 0x0F) << 2) | (b2 >> 6)] as char);
        out.push(CHARSET[b2 & 0x3F] as char);
    }

    match remainder.len() {
        1 => {
            let b0 = remainder[0] as usize;
            out.push(CHARSET[b0 >> 2] as char);
            out.push(CHARSET[(b0 & 0x03) << 4] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b0 = remainder[0] as usize;
            let b1 = remainder[1] as usize;
            out.push(CHARSET[b0 >> 2] as char);
            out.push(CHARSET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            out.push(CHARSET[(b1 & 0x0F) << 2] as char);
            out.push('=');
        }
        _ => {}
    }

    out
}

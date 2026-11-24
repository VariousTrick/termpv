// Kitty 终端图形协议渲染器（对齐 MPV 的 a=T 瞬时流式直刷，零 ID 缓存无显存泄漏，极速平滑呈现）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::io::Write;

/// Kitty 协议直接流式传输分块大小
const CHUNK_SIZE: usize = 4096;

/// Kitty 图形渲染器（对齐 MPV 工业级 a=T 瞬时直刷，零堆内存重分配）
pub struct KittyRenderer {
    b64_buf: Vec<u8>,
}

impl KittyRenderer {
    pub fn new() -> Self {
        Self {
            b64_buf: Vec::with_capacity(4 * 1024 * 1024),
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

        let start_row = layout.pad_y + 1;
        let start_col = layout.pad_x + 1;

        // 1. 原地快速 Base64 编码，复用已分配内存池
        self.b64_buf.clear();
        fast_base64_encode_into(&frame.data, &mut self.b64_buf);

        // 2. 移动光标至左上角起始绘制点
        write!(writer, "\x1b[{start_row};{start_col}H")?;

        // 3. 对齐 MPV：使用 a=T（Transmit and display immediately），瞬时直刷屏幕
        // 不分配图片 ID，不建立终端显存常驻，C=1 抑制光标偏移，q=2 静默静音，解决终端 GPU 堆叠与背压卡顿
        self.stream_chunks(
            writer,
            frame.width,
            frame.height,
            layout.render_cols,
            layout.render_rows,
        )?;

        Ok(())
    }

    /// 将 Base64 数据分块直接流式写入输出流（a=T），零临时分配
    fn stream_chunks<W: Write>(
        &self,
        writer: &mut W,
        img_w: u32,
        img_h: u32,
        render_cols: u16,
        render_rows: u16,
    ) -> Result<()> {
        let total_len = self.b64_buf.len();
        let mut offset = 0;
        let mut is_first = true;

        while offset < total_len {
            let end = (offset + CHUNK_SIZE).min(total_len);
            let is_last = end == total_len;
            let m_flag = if is_last { 0 } else { 1 };

            if is_first {
                write!(
                    writer,
                    "\x1b_Ga=T,f=24,s={img_w},v={img_h},c={render_cols},r={render_rows},z=-1,C=1,q=2,m={m_flag};"
                )?;
                is_first = false;
            } else {
                write!(writer, "\x1b_Gm={m_flag};")?;
            }

            writer.write_all(&self.b64_buf[offset..end])?;
            writer.write_all(b"\x1b\\")?;

            offset = end;
        }

        Ok(())
    }
}

/// 高性能原地 Base64 编码器（复用预分配缓冲，零堆内存重分配）
fn fast_base64_encode_into(input: &[u8], out: &mut Vec<u8>) {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let needed = (input.len() + 2) / 3 * 4;
    out.resize(needed, 0);

    let chunks = input.chunks_exact(3);
    let remainder = chunks.remainder();
    let mut out_idx = 0;

    for chunk in chunks {
        let b0 = chunk[0] as usize;
        let b1 = chunk[1] as usize;
        let b2 = chunk[2] as usize;

        out[out_idx] = CHARSET[b0 >> 2];
        out[out_idx + 1] = CHARSET[((b0 & 0x03) << 4) | (b1 >> 4)];
        out[out_idx + 2] = CHARSET[((b1 & 0x0F) << 2) | (b2 >> 6)];
        out[out_idx + 3] = CHARSET[b2 & 0x3F];
        out_idx += 4;
    }

    match remainder.len() {
        1 => {
            let b0 = remainder[0] as usize;
            out[out_idx] = CHARSET[b0 >> 2];
            out[out_idx + 1] = CHARSET[(b0 & 0x03) << 4];
            out[out_idx + 2] = b'=';
            out[out_idx + 3] = b'=';
        }
        2 => {
            let b0 = remainder[0] as usize;
            let b1 = remainder[1] as usize;
            out[out_idx] = CHARSET[b0 >> 2];
            out[out_idx + 1] = CHARSET[((b0 & 0x03) << 4) | (b1 >> 4)];
            out[out_idx + 2] = CHARSET[(b1 & 0x0F) << 2];
            out[out_idx + 3] = b'=';
        }
        _ => {}
    }
}

// Kitty 终端图形协议渲染器（基于原子双缓冲 a=t/a=p 与原地快速 Base64 流式直刷，杜绝闪烁并完美兼容现代终端）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::io::Write;

/// Kitty 协议直接流式传输分块大小
const CHUNK_SIZE: usize = 4096;

/// Kitty 图形渲染器（双缓冲原子上屏，零堆内存重分配）
pub struct KittyRenderer {
    b64_buf: Vec<u8>,
    active_slot: u8,
}

impl KittyRenderer {
    pub fn new() -> Self {
        Self {
            b64_buf: Vec::with_capacity(4 * 1024 * 1024),
            active_slot: 0,
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

        // 双缓冲轮转：i=1 与 i=2 交替上传
        let upload_id = if self.active_slot == 0 { 1 } else { 2 };
        self.active_slot = 1 - self.active_slot;

        let start_row = layout.pad_y + 1;
        let start_col = layout.pad_x + 1;

        // 1. 原地快速 Base64 编码，复用已分配内存池
        self.b64_buf.clear();
        fast_base64_encode_into(&frame.data, &mut self.b64_buf);

        // 2. 离屏传输整帧像素数据（a=t），静默接收不污染屏幕
        self.stream_chunks(
            writer,
            upload_id,
            frame.width,
            frame.height,
        )?;

        // 3. 原子化上屏（a=p）覆写 placement (p=1)，C=1 保持光标位置不发生跳动
        write!(
            writer,
            "\x1b[{start_row};{start_col}H\x1b_Ga=p,i={upload_id},p=1,c={},r={},q=2,C=1\x1b\\",
            layout.render_cols,
            layout.render_rows
        )?;

        Ok(())
    }

    /// 将 Base64 数据分块直接流式写入输出流（a=t），零临时分配
    fn stream_chunks<W: Write>(
        &self,
        writer: &mut W,
        img_id: u32,
        img_w: u32,
        img_h: u32,
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
                    "\x1b_Ga=t,i={img_id},f=24,s={img_w},v={img_h},m={m_flag},q=2;"
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

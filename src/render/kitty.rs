// Kitty 终端图形协议渲染器（基于 Layout 精准居中对齐，支持零内存分配超清直显）
use crate::error::Result;
use crate::render::buffer::RgbFrame;
use crate::render::Layout;
use std::io::Write;

/// Kitty 协议单次传输分块最大 Base64 字符数
const CHUNK_SIZE: usize = 4096;

/// Kitty 图形渲染器（基于双缓冲 Ping-Pong 机制与零内存拷贝流式直刷，实现 100% 零闪烁丝滑上屏）
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
    ///
    /// 核心抗闪烁与极致性能机制：
    /// 1. 采用 a=t 仅向终端显存静默传输像素数据，新帧传输期间旧帧稳定驻留屏幕，彻底杜绝黑屏间隙；
    /// 2. 分块数据直接流式推入终端输出缓冲，绝不在内存中反复构造拼接大型 String，零堆内存重分配；
    /// 3. 分块传输完成后，通过 a=p 配合固定 placement id (p=1) 原子化瞬时切换，杜绝撕裂与频闪；
    /// 4. 双缓冲轮换复用 image id (1 和 2)，显存自动覆盖回收，零显存泄漏。
    pub fn render<W: Write>(
        &mut self,
        frame: &RgbFrame,
        layout: &Layout,
        writer: &mut W,
    ) -> Result<()> {
        if frame.width == 0 || frame.height == 0 || layout.render_cols == 0 || layout.render_rows == 0 {
            return Ok(());
        }

        self.b64_buf.clear();

        // 轮换目标双缓冲槽位（1 与 2 交替）
        let upload_id = if self.active_slot == 0 { 1 } else { 2 };
        self.active_slot = 1 - self.active_slot;

        // 1. 快速 Base64 编码至预分配缓冲区
        fast_base64_encode_into(&frame.data, &mut self.b64_buf);

        // 2. a=t: 静默分块直接流式写入 writer，零中转堆内存拷贝
        self.stream_chunks(
            writer,
            upload_id,
            frame.width,
            frame.height,
        )?;

        // 3. a=p: 像素数据完全就绪后，以固定 placement id (p=1) 原子切换上屏
        let start_row = layout.pad_y + 1;
        let start_col = layout.pad_x + 1;
        write!(
            writer,
            "\x1b[{start_row};{start_col}H\x1b_Ga=p,i={upload_id},p=1,c={},r={},q=2\x1b\\",
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
                // a=t: 仅传输存储到终端显存，q=2: 静默模式，不返回 ACK 污染终端输入
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

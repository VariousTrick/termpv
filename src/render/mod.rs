// 渲染系统核心门面与抽象 Trait
pub mod ascii;
pub mod buffer;
pub mod filter;
pub mod halfblock;
pub mod kitty;

use crate::config::RenderMode;
use crate::error::Result;
use crate::render::ascii::AsciiRenderer;
use crate::render::buffer::RgbFrame;
use crate::render::filter::{ColorFilter, ColorFilterPreset};
use crate::render::halfblock::HalfBlockRenderer;
use crate::render::kitty::KittyRenderer;
use std::io::Write;

/// 屏幕布局与居中排版参数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub render_cols: u16,
    pub render_rows: u16,
    pub pad_x: u16,
    pub pad_y: u16,
    pub term_w: u16,
    pub term_h: u16,
}

/// 综合渲染调度器
pub struct RenderEngine {
    current_mode: RenderMode,
    halfblock: HalfBlockRenderer,
    ascii: AsciiRenderer,
    kitty: KittyRenderer,
    filter: ColorFilter,
}

impl RenderEngine {
    /// 初始化渲染引擎，支持自动探测终端环境与初始色彩风格
    pub fn new(preferred: RenderMode, filter_preset: ColorFilterPreset) -> Self {
        let active_mode = match preferred {
            RenderMode::Auto => Self::detect_best_mode(),
            specific => specific,
        };

        Self {
            current_mode: active_mode,
            halfblock: HalfBlockRenderer::new(),
            ascii: AsciiRenderer::new(),
            kitty: KittyRenderer::new(),
            filter: ColorFilter::new(filter_preset),
        }
    }

    /// 检查终端是否支持 Kitty 图形协议
    pub fn supports_kitty() -> bool {
        // VSCode 内置终端目前对 Kitty 协议支持不完整
        if std::env::var("TERM_PROGRAM").map_or(false, |p| p == "vscode") {
            return false;
        }

        std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
            || std::env::var("KITTY_WINDOW_ID").is_ok()
            || std::env::var("WEZTERM_PANE").is_ok()
            || std::env::var("TERM_PROGRAM").map_or(false, |p| p.contains("ghostty") || p == "WezTerm")
            || std::env::var("TERM").map_or(false, |t| t.contains("kitty") || t.contains("ghostty") || t.contains("wezterm"))
    }

    /// 探测终端最佳图形能力
    fn detect_best_mode() -> RenderMode {
        if Self::supports_kitty() {
            return RenderMode::Kitty;
        }

        RenderMode::HalfBlock
    }

    /// 智能轮换下一渲染模式 (Kitty -> HalfBlock -> Ascii 循环)
    pub fn cycle_mode(&mut self) -> RenderMode {
        self.current_mode = match self.current_mode {
            RenderMode::Kitty => RenderMode::HalfBlock,
            RenderMode::HalfBlock => RenderMode::Ascii,
            RenderMode::Ascii => RenderMode::Kitty,
            _ => RenderMode::Kitty,
        };

        self.current_mode
    }

    /// 获取当前模式
    pub fn current_mode(&self) -> RenderMode {
        self.current_mode
    }

    /// 循环切换 3D LUT 色彩风格滤镜
    pub fn cycle_filter(&mut self) -> (ColorFilterPreset, u16) {
        let preset = self.filter.cycle_preset();
        (preset, self.filter.custom_hue_deg())
    }

    /// 实时微调自定义保留色相中心（度）
    pub fn adjust_filter_hue(&mut self, delta_deg: f32) -> (ColorFilterPreset, u16) {
        self.filter.adjust_hue(delta_deg)
    }

    /// 渲染当前帧到终端（先执行 3D LUT 色彩风格化，再结合 Layout 居中输出）
    pub fn render_frame<W: Write>(
        &mut self,
        frame: &mut RgbFrame,
        layout: &Layout,
        writer: &mut W,
    ) -> Result<()> {
        self.filter.apply_in_place(&mut frame.data);
        match self.current_mode {
            RenderMode::Kitty => self.kitty.render(frame, layout, writer),
            RenderMode::HalfBlock => self.halfblock.render(frame, layout, writer),
            RenderMode::Ascii => self.ascii.render(frame, layout, writer),
            _ => self.halfblock.render(frame, layout, writer),
        }
    }
}

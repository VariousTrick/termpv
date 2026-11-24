// 实时 3D LUT（三线性插值）色彩风格滤镜引擎
// 在切换风格时预计算 33x33x33（~108KB，常驻 CPU L2 缓存）查找表，逐帧仅执行定点整数三线性插值，零色彩断层、零浮点开销
use clap::ValueEnum;

const LUT_N: usize = 33;
const LUT_MAX_IDX: usize = LUT_N - 1; // 32

/// 支持的实时画面色彩风格预设
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ColorFilterPreset {
    /// 原画直出（零处理开销）
    #[default]
    Original,
    /// 暗调留肤 · 暖肌高光（剥离背景杂色，聚焦温暖肌肤质感）
    SkinGlow,
    /// 罪恶之城 · 烈焰红（保留红唇/红衣，其余压成高反差 Noir 暗调）
    SinCityRed,
    /// 赛博霓虹 · 冷艳蓝（保留蓝光/蓝衣，其余暗调去色）
    CyberBlue,
    /// 黑色电影 · 高反差黑白（经典 Noir S 曲线）
    NoirBw,
    /// 好莱坞 · 青橙大片（暗部冷青 + 肌肤暖橙）
    TealOrange,
    /// 自定义留色 · 色相旋钮（可用 [ / ] 实时旋转保留的色相中心）
    CustomHue,
}

impl ColorFilterPreset {
    /// 循环切换下一预设风格
    pub fn next(self) -> Self {
        match self {
            Self::Original => Self::SkinGlow,
            Self::SkinGlow => Self::SinCityRed,
            Self::SinCityRed => Self::CyberBlue,
            Self::CyberBlue => Self::NoirBw,
            Self::NoirBw => Self::TealOrange,
            Self::TealOrange => Self::CustomHue,
            Self::CustomHue => Self::Original,
        }
    }

    /// 获取用于 OSD 浮层展示的中文名称
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Original => "原画 · Original",
            Self::SkinGlow => "暗调留肤 · 暖肌高光",
            Self::SinCityRed => "罪恶之城 · 烈焰红",
            Self::CyberBlue => "赛博霓虹 · 冷艳蓝",
            Self::NoirBw => "黑色电影 · 高反差黑白",
            Self::TealOrange => "好莱坞 · 青橙大片",
            Self::CustomHue => "自定义留色 · 色相旋钮",
        }
    }
}

/// 实时 3D LUT 色彩滤镜处理器
pub struct ColorFilter {
    preset: ColorFilterPreset,
    custom_hue_deg: f32,
    lut: Vec<[u8; 3]>,
}

impl ColorFilter {
    pub fn new(preset: ColorFilterPreset) -> Self {
        let mut filter = Self {
            preset,
            custom_hue_deg: 355.0,
            lut: Vec::new(),
        };
        filter.rebuild_lut();
        filter
    }

    #[allow(dead_code)]
    pub fn preset(&self) -> ColorFilterPreset {
        self.preset
    }

    pub fn custom_hue_deg(&self) -> u16 {
        self.custom_hue_deg.round() as u16 % 360
    }

    /// 循环切换预设风格并立即重算 3D LUT（耗时 < 1ms）
    pub fn cycle_preset(&mut self) -> ColorFilterPreset {
        self.preset = self.preset.next();
        self.rebuild_lut();
        self.preset
    }

    /// 实时旋转自定义保留色相中心（度），若当前不在 CustomHue 模式则自动切换过去
    pub fn adjust_hue(&mut self, delta_deg: f32) -> (ColorFilterPreset, u16) {
        self.preset = ColorFilterPreset::CustomHue;
        self.custom_hue_deg = (self.custom_hue_deg + delta_deg).rem_euclid(360.0);
        self.rebuild_lut();
        (self.preset, self.custom_hue_deg())
    }

    /// 对 RGB24 帧缓冲执行原地定点整数三线性插值映射（1080p ~1.5ms，零堆内存分配、零色阶断层）
    pub fn apply_in_place(&self, rgb_data: &mut [u8]) {
        if self.preset == ColorFilterPreset::Original || self.lut.len() != LUT_N * LUT_N * LUT_N {
            return;
        }

        let lut = &self.lut;
        const STRIDE_R: usize = LUT_N * LUT_N;
        const STRIDE_G: usize = LUT_N;

        for px in rgb_data.chunks_exact_mut(3) {
            let r = px[0] as u32;
            let g = px[1] as u32;
            let b = px[2] as u32;

            // 将 0..=255 精确映射到 [0..31] 格点与 [0..=255] 定点小数权重
            let sr = r * (LUT_MAX_IDX as u32);
            let sg = g * (LUT_MAX_IDX as u32);
            let sb = b * (LUT_MAX_IDX as u32);

            let r0 = (sr / 255).min((LUT_MAX_IDX - 1) as u32) as usize;
            let g0 = (sg / 255).min((LUT_MAX_IDX - 1) as u32) as usize;
            let b0 = (sb / 255).min((LUT_MAX_IDX - 1) as u32) as usize;

            let fr = (sr - (r0 as u32) * 255) as i32;
            let fg = (sg - (g0 as u32) * 255) as i32;
            let fb = (sb - (b0 as u32) * 255) as i32;

            let base = r0 * STRIDE_R + g0 * STRIDE_G + b0;
            let c000 = lut[base];
            let c001 = lut[base + 1];
            let c010 = lut[base + STRIDE_G];
            let c011 = lut[base + STRIDE_G + 1];
            let c100 = lut[base + STRIDE_R];
            let c101 = lut[base + STRIDE_R + 1];
            let c110 = lut[base + STRIDE_R + STRIDE_G];
            let c111 = lut[base + STRIDE_R + STRIDE_G + 1];

            for ch in 0..3 {
                // B 轴插值
                let c00 = (c000[ch] as i32) * 255 + ((c001[ch] as i32) - (c000[ch] as i32)) * fb;
                let c01 = (c010[ch] as i32) * 255 + ((c011[ch] as i32) - (c010[ch] as i32)) * fb;
                let c10 = (c100[ch] as i32) * 255 + ((c101[ch] as i32) - (c100[ch] as i32)) * fb;
                let c11 = (c110[ch] as i32) * 255 + ((c111[ch] as i32) - (c110[ch] as i32)) * fb;

                // G 轴插值
                let c0 = c00 + (((c01 - c00) * fg) / 255);
                let c1 = c10 + (((c11 - c10) * fg) / 255);

                // R 轴插值并还原到 0..=255
                let val = (c0 + (((c1 - c0) * fr) / 255) + 127) / 255;
                px[ch] = val.clamp(0, 255) as u8;
            }
        }
    }

    /// 预计算 33x33x33 三维色彩查找表
    fn rebuild_lut(&mut self) {
        if self.preset == ColorFilterPreset::Original {
            self.lut.clear();
            return;
        }

        let mut table = Vec::with_capacity(LUT_N * LUT_N * LUT_N);
        let inv = 1.0 / (LUT_MAX_IDX as f32);

        for ri in 0..LUT_N {
            let r = (ri as f32) * inv;
            for gi in 0..LUT_N {
                let g = (gi as f32) * inv;
                for bi in 0..LUT_N {
                    let b = (bi as f32) * inv;
                    let (or, og, ob) = self.eval_pixel(r, g, b);
                    table.push([
                        (or.clamp(0.0, 1.0) * 255.0).round() as u8,
                        (og.clamp(0.0, 1.0) * 255.0).round() as u8,
                        (ob.clamp(0.0, 1.0) * 255.0).round() as u8,
                    ]);
                }
            }
        }
        self.lut = table;
    }

    #[inline]
    fn eval_pixel(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        match self.preset {
            ColorFilterPreset::Original => (r, g, b),
            ColorFilterPreset::NoirBw => {
                let luma = perceptual_luma(r, g, b);
                let noir = noir_s_curve(luma, 1.32) * 0.96;
                (noir, noir, noir)
            }
            ColorFilterPreset::SkinGlow => eval_skin_glow(r, g, b),
            ColorFilterPreset::SinCityRed => eval_selective_hue(r, g, b, 350.0, 17.0, 0.42, 1.32, 0.06),
            ColorFilterPreset::CyberBlue => eval_selective_hue(r, g, b, 215.0, 28.0, 0.25, 1.32, 0.08),
            ColorFilterPreset::CustomHue => {
                eval_selective_hue(r, g, b, self.custom_hue_deg, 24.0, 0.28, 1.30, 0.08)
            }
            ColorFilterPreset::TealOrange => eval_teal_orange(r, g, b),
        }
    }
}

#[inline]
fn perceptual_luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// 电影 Noir 柔和 S 型对比度曲线（压深暗部、保留高光通透度，不死黑不刺眼）
#[inline]
fn noir_s_curve(luma: f32, strength: f32) -> f32 {
    let centered = luma - 0.5;
    let s = (luma * luma * (3.0 - 2.0 * luma)) - luma;
    (0.5 + centered + s * (strength - 1.0)).clamp(0.0, 1.0)
}

#[inline]
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta < 1e-5 {
        0.0
    } else if (max - r).abs() < 1e-5 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-5 {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let s = if max < 1e-5 { 0.0 } else { delta / max };
    (h, s, max)
}

#[inline]
fn hue_distance(h1: f32, h2: f32) -> f32 {
    let d = (h1 - h2).abs();
    d.min(360.0 - d)
}

/// 按色相与饱和度平滑羽化留色（边缘丝滑不割裂，背景呈低饱和暗调）
#[inline]
fn eval_selective_hue(
    r: f32,
    g: f32,
    b: f32,
    target_hue: f32,
    hue_half_width: f32,
    min_sat: f32,
    sat_boost: f32,
    bg_chroma_keep: f32,
) -> (f32, f32, f32) {
    let (h, s, _v) = rgb_to_hsv(r, g, b);
    let luma = perceptual_luma(r, g, b);

    // 宽范围 smoothstep 计算色相权重与饱和度权重，彻底杜绝生硬抠图边缘
    let h_dist = hue_distance(h, target_hue);
    let w_hue = 1.0 - smoothstep(hue_half_width * 0.45, hue_half_width * 1.15, h_dist);
    let w_sat = smoothstep(min_sat * 0.65, min_sat * 1.45, s);
    let w = (w_hue * w_sat).clamp(0.0, 1.0);

    // 低调背景：高反差 S 曲线压暗至 0.66x，保留极微弱空气感底色
    let bg_luma = noir_s_curve(luma, 1.26) * 0.66;
    let bg_r = lerp(bg_luma, r * 0.66, bg_chroma_keep);
    let bg_g = lerp(bg_luma, g * 0.66, bg_chroma_keep);
    let bg_b = lerp(bg_luma, b * 0.66, bg_chroma_keep);

    // 高亮前景：适度提纯饱和度，保持自然立体光影
    let fg_r = lerp(luma, r, sat_boost) * 1.04;
    let fg_g = lerp(luma, g, sat_boost) * 1.04;
    let fg_b = lerp(luma, b, sat_boost) * 1.04;

    (lerp(bg_r, fg_r, w), lerp(bg_g, fg_g, w), lerp(bg_b, fg_b, w))
}

/// 暗调留肤 · 暖肌高光（YCbCr 肤色带 + 暖色相双重平滑融合）
#[inline]
fn eval_skin_glow(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let luma = perceptual_luma(r, g, b);
    // BT.601 YCbCr 色度平面（归一化到 0..255 标度以对齐经典肤色区间）
    let cb = 128.0 - 43.0 * r - 85.0 * g + 128.0 * b;
    let cr = 128.0 + 128.0 * r - 107.0 * g - 21.0 * b;

    // 经典肤色椭圆中心 (Cb=102, Cr=151)，拓宽平顶区避免 4:2:0 压缩色块在大腿/手臂皮肤上发花
    let d_cb = (cb - 102.0) / 30.0;
    let d_cr = (cr - 151.0) / 26.0;
    let dist = (d_cb * d_cb + d_cr * d_cr).sqrt();
    let w_ycbcr = 1.0 - smoothstep(0.85, 1.45, dist);

    // 结合暖色相与最小亮度约束（避开纯黑噪点）
    let (h, s, v) = rgb_to_hsv(r, g, b);
    let w_hue = 1.0 - smoothstep(24.0, 46.0, hue_distance(h, 22.0));
    let w_val = smoothstep(0.04, 0.16, v) * smoothstep(0.04, 0.14, s);
    let w = (w_ycbcr * (0.55 + 0.45 * w_hue) * w_val).clamp(0.0, 1.0);

    // 暗调冷灰背景（保留 12% 底色 + 压低亮度，让暖肌脱颖而出）
    let bg_luma = noir_s_curve(luma, 1.24) * 0.68;
    let bg_r = lerp(bg_luma * 0.96, r * 0.68, 0.12);
    let bg_g = lerp(bg_luma * 0.99, g * 0.68, 0.12);
    let bg_b = lerp(bg_luma * 1.04, b * 0.68, 0.12);

    // 肌肤提亮微暖
    let fg_r = (lerp(luma, r, 1.18) * 1.04).min(1.0);
    let fg_g = (lerp(luma, g, 1.14) * 1.02).min(1.0);
    let fg_b = lerp(luma, b, 1.10);

    (lerp(bg_r, fg_r, w), lerp(bg_g, fg_g, w), lerp(bg_b, fg_b, w))
}

/// 好莱坞青橙大片（阴影推冷青，高光/肤色推暖橙）
#[inline]
fn eval_teal_orange(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let luma = perceptual_luma(r, g, b);
    let shadow_w = 1.0 - smoothstep(0.15, 0.55, luma);
    let warm_w = smoothstep(0.25, 0.75, luma);

    let cr = (r + 0.08 * warm_w - 0.06 * shadow_w).clamp(0.0, 1.0);
    let cg = (g + 0.03 * warm_w + 0.02 * shadow_w).clamp(0.0, 1.0);
    let cb = (b - 0.07 * warm_w + 0.09 * shadow_w).clamp(0.0, 1.0);

    (
        lerp(luma, cr, 1.18),
        lerp(luma, cg, 1.15),
        lerp(luma, cb, 1.18),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_original_is_noop() {
        let filter = ColorFilter::new(ColorFilterPreset::Original);
        let mut data = vec![123, 45, 210, 0, 255, 128];
        let orig = data.clone();
        filter.apply_in_place(&mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn test_sin_city_red_preserves_red_desaturates_green() {
        let filter = ColorFilter::new(ColorFilterPreset::SinCityRed);
        let mut data = vec![220, 20, 35, 20, 200, 30, 210, 160, 130];
        filter.apply_in_place(&mut data);

        // 红色像素应依然保持高红色分量（R 远大于 G）
        assert!(data[0] > data[1] + 100, "red pixel lost red: {:?}", &data[0..3]);
        // 绿色像素应被显著去色，G 与 R 接近
        let green_chroma = (data[4] as i32 - data[3] as i32).abs();
        assert!(green_chroma < 30, "green pixel was not desaturated: {:?}", &data[3..6]);
        // 肤色像素应被干净去色为黑白，绝不残余橙红杂斑
        let skin_chroma = (data[6] as i32 - data[8] as i32).abs();
        assert!(skin_chroma < 15, "skin pixel leaked color in SinCityRed: {:?}", &data[6..9]);
    }

    #[test]
    fn test_skin_glow_preserves_skin_desaturates_blue() {
        let filter = ColorFilter::new(ColorFilterPreset::SkinGlow);
        let mut data = vec![224, 172, 138, 25, 120, 225];
        filter.apply_in_place(&mut data);

        // 肤色保持暖色（R > B）
        assert!(data[0] > data[2] + 40, "skin lost warmth: {:?}", &data[0..3]);
        // 纯蓝被压成低饱和暗调
        let blue_chroma = (data[5] as i32 - data[3] as i32).abs();
        assert!(blue_chroma < 30, "blue was not subdued: {:?}", &data[3..6]);
    }
}

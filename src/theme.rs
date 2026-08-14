use gpui::{Hsla, hsla, rgb, transparent_black};

/// foxmail-rs 配色，对齐 waku theme.rs 深色档（neutral graphite，颜色仅表语义）。
/// 来源：D:\code\waku\waku-main\src\theme.rs dark()
#[derive(Clone, Copy)]
pub struct Theme {
    pub canvas: Hsla,            // 主画布
    pub sidebar: Hsla,           // 侧栏背景
    pub surface: Hsla,           // 列表/阅读区表面
    pub raised: Hsla,            // 抬升表面（选中行）
    pub sidebar_item_background: Hsla, // 侧栏项 hover/active
    pub border: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_ghost: Hsla,
    pub accent: Hsla,            // 品牌/未读强调
    pub selected: Hsla,          // 列表选中行
    pub unread_dot: Hsla,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            canvas: rgb(0x1A1A1A).into(),
            sidebar: rgb(0x181818).into(),
            surface: rgb(0x1A1A1A).into(),
            raised: rgb(0x232323).into(),
            sidebar_item_background: hsla(0.0, 0.0, 0.941, 0.06),
            border: hsla(220.0 / 360.0, 0.10, 0.90, 0.07),
            text: rgb(0xE2E2E2).into(),
            text_secondary: rgb(0xA3A3A3).into(),
            text_tertiary: rgb(0x7D7D7D).into(),
            text_ghost: rgb(0x575757).into(),
            accent: rgb(0xE2795B).into(),
            selected: hsla(0.0, 0.0, 0.941, 0.08),
            unread_dot: rgb(0x3B82F6).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: rgb(0xF6F5F6).into(),
            sidebar: rgb(0xF3F3F3).into(),
            surface: rgb(0xF6F5F6).into(),
            raised: rgb(0xECECEC).into(),
            sidebar_item_background: hsla(0.0, 0.0, 0.078, 0.06),
            border: hsla(220.0 / 360.0, 0.10, 0.12, 0.08),
            text: rgb(0x242424).into(),
            text_secondary: rgb(0x666666).into(),
            text_tertiary: rgb(0x858585).into(),
            text_ghost: rgb(0xA4A4A4).into(),
            accent: rgb(0xC85F44).into(),
            selected: hsla(0.0, 0.0, 0.078, 0.08),
            unread_dot: rgb(0x2563EB).into(),
        }
    }

    #[allow(dead_code)]
    pub fn transparent() -> Hsla {
        transparent_black()
    }
}

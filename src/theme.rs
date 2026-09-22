use crate::prefs::{self, mix_u32, Prefs, TokenSet};
use gpui::{hsla, rgb, Hsla};

#[derive(Clone, Copy)]
pub struct Theme {
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub active: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub focus: Hsla,
    pub overlay: Hsla,
}

impl Theme {
    // 仅供 native 迁移测试通过 lib 依赖调用。
    #[allow(dead_code)]
    pub fn dark() -> Self {
        Self {
            canvas: rgb(0x191919).into(),
            sidebar: rgb(0x161616).into(),
            surface: rgb(0x1C1C1C).into(),
            raised: rgb(0x242424).into(),
            active: hsla(12.0 / 360.0, 0.65, 0.60, 0.14),
            border: hsla(0.0, 0.0, 1.0, 0.07),
            text: rgb(0xE6E6E6).into(),
            text_secondary: rgb(0xA8A8A8).into(),
            text_tertiary: rgb(0x777777).into(),
            accent: rgb(0xE2795B).into(),
            accent_hover: rgb(0xE8896D).into(),
            on_accent: rgb(0x111111).into(),
            danger: rgb(0xE05555).into(),
            success: rgb(0x5BA86A).into(),
            warning: rgb(0xD4A017).into(),
            info: rgb(0x5B8DE2).into(),
            focus: rgb(0xE2795B).into(),
            overlay: hsla(0.0, 0.0, 0.0, 0.72),
        }
    }

    #[allow(dead_code)]
    pub fn light() -> Self {
        Self {
            canvas: rgb(0xF4F4F4).into(),
            sidebar: rgb(0xEFEFEF).into(),
            surface: rgb(0xFFFFFF).into(),
            raised: rgb(0xF7F7F7).into(),
            active: hsla(12.0 / 360.0, 0.65, 0.52, 0.16),
            border: hsla(0.0, 0.0, 0.0, 0.10),
            text: rgb(0x1A1A1A).into(),
            text_secondary: rgb(0x5A5A5A).into(),
            text_tertiary: rgb(0x8A8A8A).into(),
            accent: rgb(0xC45C3E).into(),
            accent_hover: rgb(0xD46A4C).into(),
            on_accent: rgb(0xFFFFFF).into(),
            danger: rgb(0xC23B3B).into(),
            success: rgb(0x2F7D45).into(),
            warning: rgb(0xA87A00).into(),
            info: rgb(0x2F66C0).into(),
            focus: rgb(0xC45C3E).into(),
            overlay: hsla(0.0, 0.0, 0.0, 0.46),
        }
    }

    pub fn from_env() -> Self {
        Self::from_prefs(&prefs::cached())
    }

    pub fn from_prefs(prefs: &Prefs) -> Self {
        Self::from_tokens(prefs.tokens(), prefs.is_dark())
    }

    pub fn from_tokens(tokens: TokenSet, dark: bool) -> Self {
        let canvas: Hsla = rgb(tokens.canvas).into();
        let surface: Hsla = rgb(tokens.surface).into();
        let text: Hsla = rgb(tokens.text).into();
        let accent: Hsla = rgb(tokens.primary).into();
        let border: Hsla = rgb(tokens.border).into();
        let sidebar: Hsla = rgb(mix_u32(
            tokens.canvas,
            if dark { 0x000000 } else { 0xE8E8E8 },
            0.14,
        ))
        .into();
        let raised: Hsla = rgb(mix_u32(tokens.surface, tokens.text, 0.06)).into();
        let accent_hover: Hsla = rgb(mix_u32(
            tokens.primary,
            if dark { 0xFFFFFF } else { 0x000000 },
            0.12,
        ))
        .into();
        let on_accent: Hsla = if luminance(tokens.primary) > 0.45 {
            rgb(0x111111).into()
        } else {
            rgb(0xFFFFFF).into()
        };
        let text_secondary: Hsla = rgb(mix_u32(tokens.text, tokens.canvas, 0.38)).into();
        let text_tertiary: Hsla = rgb(mix_u32(tokens.text, tokens.canvas, 0.58)).into();
        let mut active = accent;
        active.a = 0.14;
        Self {
            canvas,
            sidebar,
            surface,
            raised,
            active,
            border,
            text,
            text_secondary,
            text_tertiary,
            accent,
            accent_hover,
            on_accent,
            danger: if dark {
                rgb(0xE05555).into()
            } else {
                rgb(0xC23B3B).into()
            },
            success: if dark {
                rgb(0x5BA86A).into()
            } else {
                rgb(0x2F7D45).into()
            },
            warning: if dark {
                rgb(0xD4A017).into()
            } else {
                rgb(0xA87A00).into()
            },
            info: if dark {
                rgb(0x5B8DE2).into()
            } else {
                rgb(0x2F66C0).into()
            },
            focus: accent,
            overlay: if dark {
                hsla(0.0, 0.0, 0.0, 0.72)
            } else {
                hsla(0.0, 0.0, 0.0, 0.46)
            },
        }
    }
}

fn luminance(color: u32) -> f32 {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(test)]
mod tests {
    use super::Theme;

    #[test]
    fn semantic_tokens_are_distinct_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            assert_ne!(theme.danger, theme.success);
            assert_ne!(theme.danger, theme.accent);
            assert_ne!(theme.warning, theme.info);
            assert_ne!(theme.on_accent, theme.accent);
            assert_ne!(theme.accent, theme.accent_hover);
            assert_ne!(theme.focus, theme.border);
        }
    }
}

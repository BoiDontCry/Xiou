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
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub focus: Hsla,
    pub overlay: Hsla,
}

impl Theme {
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
            on_accent: rgb(0x111111).into(),
            danger: rgb(0xE05555).into(),
            success: rgb(0x5BA86A).into(),
            warning: rgb(0xD4A017).into(),
            info: rgb(0x5B8DE2).into(),
            focus: rgb(0xE2795B).into(),
            overlay: hsla(0.0, 0.0, 0.0, 0.72),
        }
    }

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
        match std::env::var("FOXMAIL_THEME") {
            Ok(value) if value.eq_ignore_ascii_case("light") => Self::light(),
            _ => Self::dark(),
        }
    }
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
            assert_ne!(theme.focus, theme.border);
        }
    }
}

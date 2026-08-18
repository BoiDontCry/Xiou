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
        }
    }
}

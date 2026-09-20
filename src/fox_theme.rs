//! Foxmail token injection into the gpui-kit Theme global.
//!
//! `theme.rs` stays the source of truth for hand-painted UI; this module maps
//! its 18 tokens onto gpui-component's ThemeColor so library-owned widgets
//! (Button, Input, Dialog, ...) render in the same palette.

use crate::theme::Theme;
use gpui_kit::component::theme::{Theme as KitTheme, ThemeColor, ThemeMode, ThemeTokens};

/// Map foxmail's 18 tokens onto gpui-kit color fields. Pure fn so it is unit
/// testable without a gpui App.
fn fox_color(theme: Theme) -> ThemeColor {
    let mut colors = if theme.canvas.l > 0.5 {
        ThemeColor::light().as_ref().clone()
    } else {
        ThemeColor::dark().as_ref().clone()
    };
    colors.background = theme.canvas;
    colors.foreground = theme.text;
    colors.primary = theme.accent;
    colors.primary_foreground = theme.on_accent;
    colors.primary_hover = theme.accent_hover;
    colors.primary_active = theme.accent;
    colors.secondary = theme.surface;
    colors.secondary_foreground = theme.text;
    colors.secondary_hover = theme.raised;
    colors.secondary_active = theme.raised;
    colors.muted_foreground = theme.text_secondary;
    colors.accent = theme.active;
    colors.accent_foreground = theme.text;
    colors.popover = theme.raised;
    colors.popover_foreground = theme.text;
    colors.danger = theme.danger;
    colors.success = theme.success;
    colors.warning = theme.warning;
    colors.info = theme.info;
    colors.border = theme.border;
    colors.input = theme.border;
    colors.ring = theme.focus;
    colors.overlay = theme.overlay;
    colors.sidebar = theme.sidebar;
    colors.sidebar_foreground = theme.text_secondary;
    colors.sidebar_border = theme.border;
    colors.sidebar_accent = theme.active;
    colors.sidebar_accent_foreground = theme.text;
    colors.title_bar = theme.sidebar;
    colors.title_bar_border = theme.border;
    colors.selection = theme.active;
    colors.list_active = theme.active;
    colors.list_active_border = theme.accent;
    colors.list_hover = theme.raised;
    colors.drag_border = theme.focus;
    colors.caret = theme.accent;
    colors.progress_bar = theme.accent;
    colors.slider_bar = theme.accent;
    // Button tokens (used by library Button variants): mirror the hand-painted
    // control_button palette so T2 swaps render identically.
    colors.button = theme.raised;
    colors.button_foreground = theme.text;
    colors.button_hover = theme.active;
    colors.button_primary = theme.accent;
    colors.button_primary_foreground = theme.on_accent;
    colors.button_primary_hover = theme.accent_hover;
    colors.button_primary_active = theme.accent;
    colors.button_secondary = theme.raised;
    colors.button_secondary_foreground = theme.text;
    colors.button_secondary_hover = theme.active;
    colors.button_secondary_active = theme.raised;
    colors
}

/// Push the current foxmail palette into the gpui-kit Theme global.
/// Called at startup and whenever settings change the active palette.
pub fn apply_foxmail_theme(theme: Theme, cx: &mut gpui_kit::App) {
    let mode = if theme.canvas.l > 0.5 {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    // Theme::change rebuilds from the registry config for `mode`; overriding
    // the colors afterwards keeps every unmapped field at the library default.
    KitTheme::change(mode, None, cx);
    let kit = KitTheme::global_mut(cx);
    kit.colors = fox_color(theme);
    // Component Button/Dialog/List/Progress render resolved background tokens,
    // not only the legacy solid color fields. Base synchronization alone does
    // not rebuild this layer; derive both from the same Foxmail palette first.
    kit.tokens = ThemeTokens::from(kit.colors);
    kit.mode = mode;
    // Base inputs/scrollbars keep a separate snapshot of these public fields.
    KitTheme::sync_base(cx);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::fox_color;
    use crate::theme::Theme;

    #[test]
    fn maps_all_foxmail_tokens_in_both_modes() {
        for theme in [Theme::dark(), Theme::light()] {
            let colors = fox_color(theme);
            assert_eq!(colors.background, theme.canvas);
            assert_eq!(colors.foreground, theme.text);
            assert_eq!(colors.primary, theme.accent);
            assert_eq!(colors.primary_foreground, theme.on_accent);
            assert_eq!(colors.primary_hover, theme.accent_hover);
            assert_eq!(colors.secondary, theme.surface);
            assert_eq!(colors.muted_foreground, theme.text_secondary);
            assert_eq!(colors.popover, theme.raised);
            assert_eq!(colors.danger, theme.danger);
            assert_eq!(colors.success, theme.success);
            assert_eq!(colors.warning, theme.warning);
            assert_eq!(colors.info, theme.info);
            assert_eq!(colors.border, theme.border);
            assert_eq!(colors.ring, theme.focus);
            assert_eq!(colors.overlay, theme.overlay);
            assert_eq!(colors.sidebar, theme.sidebar);
            assert_eq!(colors.selection, theme.active);
        }
    }

    #[test]
    fn default_dark_palette_lands_on_foxmail_values() {
        let colors = fox_color(Theme::dark());
        assert_eq!(colors.primary, gpui_kit::rgb(0xE2795B).into());
        assert_eq!(colors.background, gpui_kit::rgb(0x191919).into());
    }
}

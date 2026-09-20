use crate::theme::Theme;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, Application, ElementId, Entity, IntoElement,
    SharedString, Window,
};

pub use gpui::rems;

// The shared IconName catalog is larger than the default component bundle.
// Embed only the mail-specific additions, then retain all official defaults.
gpui_kit::assets::icon_assets!(
    MailIcons,
    [
        Download,
        FilePen,
        Forward,
        Image,
        Paperclip,
        Reply,
        Send,
        ShieldAlert,
        SquarePen,
        Tags,
        Trash,
        X,
    ]
);

pub struct MailAssets;

impl gpui::AssetSource for MailAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = MailIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(MailIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Use the same asset source in every application entry point and UI test.
pub fn application() -> Application {
    gpui_platform::application().with_assets(MailAssets)
}

/// Every native window participates in the component focus/overlay system.
pub fn root_view(
    view: impl Into<AnyView>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<gpui_kit::component::Root> {
    cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
}

/// Mount all component service layers on every screen, including onboarding.
pub fn overlay_layers(window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
    use gpui_kit::component::Root;
    [
        Root::render_dialog_layer(window, cx).map(|layer| layer.into_any_element()),
        Root::render_sheet_layer(window, cx).map(|layer| layer.into_any_element()),
        Root::render_notification_layer(window, cx).map(|layer| layer.into_any_element()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
}

/// Library Button pre-styled to match the old hand-painted control_button.
/// Callers keep `.child(...)`, `.on_click(...)`, and any `Styled` tweaks.
pub fn control_button(
    id: impl Into<ElementId>,
    _theme: Theme,
    kind: ButtonKind,
    disabled: bool,
) -> gpui_kit::component::button::Button {
    use gpui_kit::base::Disableable as _;
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    let id = id.into();
    let mut b = Button::new(id);
    b = match kind {
        ButtonKind::Primary => b.primary(),
        ButtonKind::Secondary => b.secondary(),
        ButtonKind::Ghost => b.ghost(),
    };
    if disabled {
        b.disabled(true)
    } else {
        b
    }
}

pub fn tooltip(
    text: impl Into<SharedString>,
    _theme: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}

pub fn step_index(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let start = current.unwrap_or(0) as isize;
    Some((start + delta).rem_euclid(count as isize) as usize)
}

#[cfg(test)]
mod tests {
    use super::{step_index, MailAssets};
    use gpui::AssetSource as _;

    #[test]
    fn every_application_mail_icon_has_embedded_svg_bytes() {
        use gpui_kit::assets::IconName;
        // Inventory the actual call sites: adding a new shared icon without its
        // SVG must fail even when Rust accepts the enum and the layout is valid.
        let mut count = 0;
        for tail in include_str!("app.rs").split("MailIcon::").skip(1) {
            let name: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            let icon = IconName::ALL
                .iter()
                .find(|icon| format!("{icon:?}") == name)
                .unwrap_or_else(|| panic!("unknown mail icon {name}"));
            let path = icon.path();
            let bytes = MailAssets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("missing bundled mail icon: {name} ({path})"));
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("<svg"),
                "{path}"
            );
            count += 1;
        }
        assert!(
            count >= 20,
            "source inventory must not silently become empty"
        );
    }

    #[test]
    fn mail_assets_keep_defaults_deduplicate_lists_and_do_not_embed_every_icon() {
        let source = MailAssets;
        for path in [
            "icons/search.svg",
            "icons/check.svg",
            "icons/reply.svg",
            "icons/download.svg",
        ] {
            assert!(source.load(path).unwrap().is_some(), "{path}");
        }
        assert!(source.load("icons/nonexistent.svg").is_err());
        assert!(source.load("icons/bike.svg").is_err());
        assert!(source.load("").unwrap().is_none());
        let paths = source.list("icons/").unwrap();
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(paths.len() < 200, "keep the application bundle selective");
        assert_eq!(source.list("icons/reply").unwrap().len(), 1);
    }

    #[test]
    fn step_index_wraps_and_ignores_empty_lists() {
        assert_eq!(step_index(Some(0), 3, 1), Some(1));
        assert_eq!(step_index(Some(2), 3, 1), Some(0));
        assert_eq!(step_index(Some(0), 3, -1), Some(2));
        assert_eq!(step_index(None, 4, 1), Some(1));
        assert_eq!(step_index(Some(0), 0, 1), None);
    }
}

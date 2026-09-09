use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyView, App, AppContext as _, Context, Div, ElementId, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
};

pub use gpui::rems;

pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
}

pub fn control_button(
    id: impl Into<ElementId>,
    theme: Theme,
    kind: ButtonKind,
    disabled: bool,
) -> Stateful<Div> {
    let (bg, fg, hover) = match kind {
        ButtonKind::Primary => (Some(theme.accent), theme.on_accent, theme.accent_hover),
        ButtonKind::Secondary => (Some(theme.raised), theme.text, theme.active),
        ButtonKind::Ghost => (None, theme.text_secondary, theme.raised),
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(gpui::hsla(0.0, 0.0, 0.0, 0.0))
        .rounded(rems(0.25))
        .text_color(fg)
        .when_some(bg, |el, bg| el.bg(bg))
        .when(disabled, |el| el.opacity(0.5))
        .when(!disabled, |el| {
            el.tab_index(0)
                .hover(move |style| style.bg(hover))
                .active(|style| style.opacity(0.85))
                .focus(move |style| style.border_color(theme.focus))
        })
}

pub fn tooltip(
    text: impl Into<SharedString>,
    theme: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| LabelTooltip {
            text: text.clone(),
            theme,
        })
        .into()
    }
}

pub fn step_index(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let start = current.unwrap_or(0) as isize;
    Some((start + delta).rem_euclid(count as isize) as usize)
}

struct LabelTooltip {
    text: SharedString,
    theme: Theme,
}

impl Render for LabelTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(rems(0.5))
            .py(rems(0.25))
            .rounded(rems(0.25))
            .bg(self.theme.raised)
            .border_1()
            .border_color(self.theme.border)
            .text_color(self.theme.text)
            .text_xs()
            .child(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::step_index;

    #[test]
    fn step_index_wraps_and_ignores_empty_lists() {
        assert_eq!(step_index(Some(0), 3, 1), Some(1));
        assert_eq!(step_index(Some(2), 3, 1), Some(0));
        assert_eq!(step_index(Some(0), 3, -1), Some(2));
        assert_eq!(step_index(None, 4, 1), Some(1));
        assert_eq!(step_index(Some(0), 0, 1), None);
    }
}

use gpui::{
    actions, div, fill, hsla, point, px, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement,
    Pixels, Point, Render, SharedString, Styled, StyledText, TextLayout, TextRun, UTF16Selection,
    Window,
};
use std::ops::Range;
use std::time::Duration;

use crate::theme::Theme;

actions!(
    composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Enter,
        Newline,
        Paste,
        Cut,
        Copy,
    ]
);

#[derive(Clone)]
pub enum ComposerEvent {
    Submit,
    Cancel,
    Edited,
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    is_selecting: bool,
    last_layout: Option<TextLayout>,
    cursor_visible: bool,
    cursor_epoch: usize,
}

impl ComposerInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, initial: String) -> Self {
        let focus_handle = cx.focus_handle();
        let _ = cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.cursor_epoch += 1;
            this.cursor_visible = true;
            this.blink(this.cursor_epoch, cx);
            cx.notify();
        });
        let _ = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.cursor_epoch += 1;
            this.cursor_visible = false;
            cx.notify();
        });
        let offset = initial.len();
        Self {
            focus_handle,
            content: initial,
            selected_range: offset..offset,
            selection_reversed: false,
            is_selecting: false,
            last_layout: None,
            cursor_visible: false,
            cursor_epoch: 0,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    #[allow(dead_code)]
    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content;
        let offset = self.content.len();
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.emit(ComposerEvent::Edited);
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch != self.cursor_epoch {
            return;
        }
        self.cursor_visible = !self.cursor_visible;
        cx.notify();
        let next = epoch + 1;
        self.cursor_epoch = next;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.blink(next, cx));
            }
        })
        .detach();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn previous_line_start(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let prefix = &self.content[..offset];
        let trimmed = prefix.strip_suffix('\n').unwrap_or(prefix);
        match trimmed.rfind('\n') {
            Some(index) => index + 1,
            None => 0,
        }
    }

    fn next_line_start(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            return self.content.len();
        }
        match self.content[offset..].find('\n') {
            Some(relative) => offset + relative + 1,
            None => self.content.len(),
        }
    }

    fn previous_char(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let mut index = offset - 1;
        while index > 0 && !self.content.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn next_char(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            return self.content.len();
        }
        let mut index = offset + 1;
        while index < self.content.len() && !self.content.is_char_boundary(index) {
            index += 1;
        }
        index
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = self.last_layout.as_ref() else {
            return self.cursor_offset();
        };
        layout
            .index_for_position(position)
            .unwrap_or_else(|index| index)
            .min(self.content.len())
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for character in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += character.len_utf8();
            utf16 += character.len_utf16();
        }
        utf16
    }

    fn offset_from_utf16(&self, utf16_target: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for character in self.content.chars() {
            if utf16 >= utf16_target {
                break;
            }
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        utf8
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn splice(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());
        if !self.content.is_char_boundary(start)
            || !self.content.is_char_boundary(end)
            || start > end
        {
            return;
        }
        let mut new = String::with_capacity(start + text.len() + self.content.len() - end);
        new.push_str(&self.content[..start]);
        new.push_str(text);
        new.push_str(&self.content[end..]);
        self.content = new;
        let offset = start + text.len();
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.emit(ComposerEvent::Edited);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.previous_char(self.cursor_offset());
            self.selected_range = offset..self.cursor_offset();
        }
        self.splice(self.selected_range.clone(), "", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.next_char(self.cursor_offset());
            self.selected_range = self.cursor_offset()..offset;
        }
        self.splice(self.selected_range.clone(), "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_char(self.cursor_offset()), cx);
        } else {
            let anchor = if self.selection_reversed {
                self.selected_range.end
            } else {
                self.selected_range.start
            };
            self.move_to(anchor, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_char(self.cursor_offset()), cx);
        } else {
            let anchor = if self.selection_reversed {
                self.selected_range.start
            } else {
                self.selected_range.end
            };
            self.move_to(anchor, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_line_start(self.cursor_offset()), cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_line_start(self.cursor_offset()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_char(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_char(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_line_start(self.cursor_offset()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(
            self.next_line_start(self.cursor_offset())
                .min(self.content.len()),
            cx,
        );
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.splice(self.selected_range.clone(), "\n", cx);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.splice(self.selected_range.clone(), "\n", cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = clipboard.text() else {
            return;
        };
        let text = text.replace('\r', "");
        self.splice(self.selected_range.clone(), &text, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
        self.splice(self.selected_range.clone(), "", cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let offset = self.index_for_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn dispatch_submit(&mut self, send: bool, cancel: bool, cx: &mut Context<Self>) {
        if send {
            cx.emit(ComposerEvent::Submit);
        } else if cancel {
            cx.emit(ComposerEvent::Cancel);
        }
    }
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<ComposerEvent> for ComposerInput {}

impl Render for ComposerInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let on_key = cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            let modifiers = &event.keystroke.modifiers;
            let key = event.keystroke.key.as_str();
            if key.eq_ignore_ascii_case("enter") && (modifiers.control || modifiers.platform) {
                this.dispatch_submit(true, false, cx);
            } else if key.eq_ignore_ascii_case("escape") {
                this.dispatch_submit(false, true, cx);
            }
        });
        let theme_for_cursor = theme;
        let _ = theme_for_cursor;
        div()
            .id("composer-field")
            .key_context("ComposerInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .flex_1()
            .min_h(px(24.0))
            .w_full()
            .text_color(theme.text)
            .text_sm()
            .line_height(px(22.0))
            .on_action(cx.listener(ComposerInput::backspace))
            .on_action(cx.listener(ComposerInput::delete))
            .on_action(cx.listener(ComposerInput::left))
            .on_action(cx.listener(ComposerInput::right))
            .on_action(cx.listener(ComposerInput::up))
            .on_action(cx.listener(ComposerInput::down))
            .on_action(cx.listener(ComposerInput::select_left))
            .on_action(cx.listener(ComposerInput::select_right))
            .on_action(cx.listener(ComposerInput::select_all))
            .on_action(cx.listener(ComposerInput::home))
            .on_action(cx.listener(ComposerInput::end))
            .on_action(cx.listener(ComposerInput::enter))
            .on_action(cx.listener(ComposerInput::newline))
            .on_action(cx.listener(ComposerInput::paste))
            .on_action(cx.listener(ComposerInput::copy))
            .on_action(cx.listener(ComposerInput::cut))
            .on_key_down(on_key)
            .on_mouse_down(MouseButton::Left, cx.listener(ComposerInput::on_mouse_down))
            .on_mouse_move(cx.listener(ComposerInput::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(ComposerInput::on_mouse_up))
            .child(ComposerElement { input: cx.entity() })
    }
}

struct ComposerElement {
    input: Entity<ComposerInput>,
}

impl IntoElement for ComposerElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ComposerElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<PaintQuad>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let content_empty = input.content.is_empty();
        let text: SharedString = if content_empty {
            "在这里输入邮件正文…".into()
        } else {
            input.content.clone().into()
        };
        let style = window.text_style();
        let run = if content_empty {
            TextRun {
                len: text.len(),
                font: style.font(),
                color: Theme::dark().text_tertiary,
                background_color: None,
                underline: None,
                strikethrough: None,
            }
        } else {
            TextRun {
                len: text.len(),
                font: style.font(),
                color: style.color,
                background_color: if !input.selected_range.is_empty() {
                    Some(hsla(0.0, 0.0, 1.0, 0.18))
                } else {
                    None
                },
                underline: None,
                strikethrough: None,
            }
        };
        let mut styled = StyledText::new(text).with_runs(vec![run]);
        let (layout_id, ()) = styled.request_layout(None, None, window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (text, run, cursor_offset, cursor_visible, selected_empty, content_empty) = {
            let input = self.input.read(cx);
            let content_empty = input.content.is_empty();
            let text: SharedString = if content_empty {
                "在这里输入邮件正文…".into()
            } else {
                input.content.clone().into()
            };
            let style = window.text_style();
            let run = if content_empty {
                TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: Theme::dark().text_tertiary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }
            } else {
                TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: style.color,
                    background_color: if !input.selected_range.is_empty() {
                        Some(hsla(0.0, 0.0, 1.0, 0.18))
                    } else {
                        None
                    },
                    underline: None,
                    strikethrough: None,
                }
            };
            (
                text,
                run,
                input.cursor_offset(),
                input.cursor_visible,
                input.selected_range.is_empty(),
                content_empty,
            )
        };
        let mut styled = StyledText::new(text).with_runs(vec![run]);
        styled.prepaint(None, None, bounds, &mut (), window, cx);
        let layout = styled.layout().clone();
        let cursor = if !content_empty && selected_empty && cursor_visible {
            layout.position_for_index(cursor_offset).map(|position| {
                fill(
                    Bounds::new(position, size(px(1.5), layout.line_height())),
                    Theme::dark().accent,
                )
            })
        } else {
            None
        };
        self.input.update(cx, |input, _| {
            input.last_layout = Some(layout);
        });
        cursor
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let (text, run) = {
            let input = self.input.read(cx);
            let content_empty = input.content.is_empty();
            let text: SharedString = if content_empty {
                "在这里输入邮件正文…".into()
            } else {
                input.content.clone().into()
            };
            let style = window.text_style();
            let run = if content_empty {
                TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: Theme::dark().text_tertiary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }
            } else {
                TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: style.color,
                    background_color: if !input.selected_range.is_empty() {
                        Some(hsla(0.0, 0.0, 1.0, 0.18))
                    } else {
                        None
                    },
                    underline: None,
                    strikethrough: None,
                }
            };
            (text, run)
        };
        let mut styled = StyledText::new(text).with_runs(vec![run]);
        styled.prepaint(None, None, bounds, &mut (), window, cx);
        styled.paint(None, None, bounds, &mut (), &mut (), window, cx);
        let layout = styled.layout().clone();
        self.input.update(cx, |input, _| {
            input.last_layout = Some(layout);
        });
        if let Some(cursor) = prepaint.take() {
            window.paint_quad(cursor);
        }
    }
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .unwrap_or_else(|| self.selected_range.clone());
        self.splice(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .unwrap_or_else(|| self.selected_range.clone());
        self.splice(range, new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = layout.position_for_index(range.start)?;
        let end = layout.position_for_index(range.end)?;
        let line_height = layout.line_height();
        Some(Bounds::from_corners(
            start,
            point(end.x, end.y + line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.last_layout.as_ref()?;
        let utf8_index = layout
            .index_for_position(point)
            .unwrap_or_else(|index| index)
            .min(self.content.len());
        Some(self.offset_to_utf16(utf8_index))
    }
}

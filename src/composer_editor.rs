use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, fill, point, px, size, App, Bounds, ClipboardItem, Context, CursorStyle, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    StyledText, TextLayout, TextRun, UTF16Selection, Window,
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
    NavigatePrevious,
    NavigateNext,
}

#[derive(Clone)]
pub struct ComposerInputOptions {
    placeholder: SharedString,
    single_line: bool,
    submit_on_enter: bool,
    list_navigation: bool,
    masked: bool,
}

impl ComposerInputOptions {
    pub fn single_line(placeholder: impl Into<SharedString>) -> Self {
        Self {
            placeholder: placeholder.into(),
            single_line: true,
            submit_on_enter: false,
            list_navigation: false,
            masked: false,
        }
    }

    pub fn multiline(placeholder: impl Into<SharedString>) -> Self {
        Self {
            placeholder: placeholder.into(),
            single_line: false,
            submit_on_enter: false,
            list_navigation: false,
            masked: false,
        }
    }

    pub fn submit_on_enter(mut self) -> Self {
        self.submit_on_enter = true;
        self
    }

    pub fn list_navigation(mut self) -> Self {
        self.list_navigation = true;
        self
    }

    /// 密码字段：渲染为掩码，复制/剪切不外泄明文，仍可正常输入与编辑。
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    marked_range: Option<Range<usize>>,
    selection_reversed: bool,
    is_selecting: bool,
    last_layout: Option<TextLayout>,
    cursor_visible: bool,
    cursor_epoch: usize,
    scroll_handle: ScrollHandle,
    placeholder: SharedString,
    single_line: bool,
    submit_on_enter: bool,
    list_navigation: bool,
    masked: bool,
}

impl ComposerInput {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        initial: String,
        options: ComposerInputOptions,
    ) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let _ = cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.cursor_epoch += 1;
            this.blink(this.cursor_epoch, cx);
            cx.notify();
        });
        let _ = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.cursor_epoch += 1;
            this.cursor_visible = false;
            cx.notify();
        });
        // ponytail: on_focus misses focus set during window construction (listener
        // activates after the focus change). Window-activation catches it: when the
        // window gains focus and we're already focused, start blinking. Mirrors waku.
        let _ = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && this.focus_handle.is_focused(window) {
                this.cursor_epoch += 1;
                this.blink(this.cursor_epoch, cx);
                cx.notify();
            } else if !window.is_window_active() {
                this.cursor_epoch += 1;
                this.cursor_visible = false;
                cx.notify();
            }
        });
        let offset = initial.len();
        Self {
            focus_handle,
            content: initial,
            selected_range: offset..offset,
            marked_range: None,
            selection_reversed: false,
            is_selecting: false,
            last_layout: None,
            cursor_visible: true,
            cursor_epoch: 0,
            scroll_handle: ScrollHandle::new(),
            placeholder: options.placeholder,
            single_line: options.single_line,
            submit_on_enter: options.submit_on_enter,
            list_navigation: options.list_navigation,
            masked: options.masked,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content;
        let offset = self.content.len();
        self.selected_range = offset..offset;
        self.marked_range = None;
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
        self.marked_range = None;
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
        if self.list_navigation && self.marked_range.is_none() {
            cx.emit(ComposerEvent::NavigatePrevious);
        } else {
            self.move_to(self.previous_line_start(self.cursor_offset()), cx);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_navigation && self.marked_range.is_none() {
            cx.emit(ComposerEvent::NavigateNext);
        } else {
            self.move_to(self.next_line_start(self.cursor_offset()), cx);
        }
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
        if !self.single_line {
            self.splice(self.selected_range.clone(), "\n", cx);
        }
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if !self.single_line {
            self.splice(self.selected_range.clone(), "\n", cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = clipboard.text() else {
            return;
        };
        let text = normalize_pasted_text(&text, self.single_line);
        self.splice(self.selected_range.clone(), &text, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        // ponytail: 掩码字段不把明文写剪贴板；仍允许读取（清空）让 Ctrl+C 不报错。
        if self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
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
        if !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
        self.splice(self.selected_range.clone(), "", cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        // ponytail: on_focus misses focus set during window construction, and
        // observe_window_activation only fires on activation change. A click is
        // the one guaranteed signal the user wants this field, so start blinking here.
        self.cursor_epoch += 1;
        self.cursor_visible = true;
        self.blink(self.cursor_epoch, cx);
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
        let theme = Theme::from_env();
        let on_key = cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            let modifiers = &event.keystroke.modifiers;
            let key = event.keystroke.key.as_str();
            if key.eq_ignore_ascii_case("enter")
                && this.marked_range.is_none()
                && (modifiers.control || modifiers.platform || this.submit_on_enter)
            {
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
            .min_h_0()
            .min_h(if self.single_line {
                gpui::rems(1.5)
            } else {
                gpui::rems(6.0)
            })
            .w_full()
            .when(self.single_line, |element| {
                element.flex().items_center().h_full().px_1()
            })
            .when(!self.single_line, |element| {
                element
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p_3()
            })
            .text_color(theme.text)
            .text_sm()
            .line_height(gpui::rems(1.375))
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
            .child(ComposerElement {
                input: cx.entity(),
                styled: None,
            })
    }
}

struct ComposerElement {
    input: Entity<ComposerInput>,
    // ponytail: reuse one StyledText across request_layout/prepaint/paint;
    // rebuilding it per phase gives each phase a fresh unmeasured TextLayout → panic.
    styled: Option<StyledText>,
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
        self.styled = Some(build_styled_text(self.input.read(cx), window));
        let styled = self.styled.as_mut().expect("styled text built above");
        styled.request_layout(None, None, window, cx)
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
        let styled = self.styled.as_mut().expect("request_layout ran first");
        styled.prepaint(None, None, bounds, &mut (), window, cx);
        let layout = styled.layout().clone();
        let input = self.input.read(cx);
        // ponytail: cursor visible only when focused AND blink tick is on,
        // mirroring waku's cursor_should_be_visible. blink is started by on_focus.
        let cursor_visible = input.cursor_visible && input.focus_handle.is_focused(window);
        let cursor = (input.selected_range.is_empty() && cursor_visible)
            .then(|| layout.position_for_index(input.cursor_offset()))
            .flatten()
            .map(|position| {
                fill(
                    Bounds::new(position, size(px(1.5), layout.line_height())),
                    Theme::dark().accent,
                )
            });
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
        let styled = self.styled.as_mut().expect("request_layout ran first");
        styled.prepaint(None, None, bounds, &mut (), window, cx);
        styled.paint(None, None, bounds, &mut (), &mut (), window, cx);
        self.input.update(cx, |input, _| {
            if let Some(styled) = self.styled.as_ref() {
                input.last_layout = Some(styled.layout().clone());
            }
        });
        if let Some(cursor) = prepaint.take() {
            window.paint_quad(cursor);
        }
    }
}

fn build_styled_text(input: &ComposerInput, window: &Window) -> StyledText {
    let content_empty = input.content.is_empty();
    let text: SharedString = if content_empty {
        input.placeholder.clone()
    } else if input.masked {
        // ponytail: 按字符数生成 ● 掩码，保留光标/选区长度语义。
        // 不暴露明文给布局层；IME 组合态下 marked_range 仍按真实长度计数。
        SharedString::from("●".repeat(input.content.chars().count()))
    } else {
        input.content.clone().into()
    };
    let style = window.text_style();
    let run = if content_empty {
        TextRun {
            len: text.len(),
            font: style.font(),
            color: Theme::from_env().text_tertiary,
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
                Some(Theme::from_env().active)
            } else {
                None
            },
            underline: None,
            strikethrough: None,
        }
    };
    StyledText::new(text).with_runs(vec![run])
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
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

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
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.splice(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        if !self.content.is_char_boundary(range.start)
            || !self.content.is_char_boundary(range.end)
            || range.start > range.end
        {
            return;
        }
        self.content.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| {
                let start = utf8_offset_for_utf16(new_text, selection.start);
                let end = utf8_offset_for_utf16(new_text, selection.end);
                range.start + start..range.start + end
            })
            .unwrap_or_else(|| {
                let offset = range.start + new_text.len();
                offset..offset
            });
        self.selection_reversed = false;
        cx.emit(ComposerEvent::Edited);
        cx.notify();
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

fn utf8_offset_for_utf16(value: &str, utf16_target: usize) -> usize {
    let mut utf16 = 0;
    for (utf8, character) in value.char_indices() {
        if utf16 >= utf16_target {
            return utf8;
        }
        utf16 += character.len_utf16();
    }
    value.len()
}

fn normalize_pasted_text(value: &str, single_line: bool) -> String {
    if single_line {
        value.replace("\r\n", " ").replace(['\r', '\n'], " ")
    } else {
        value.replace('\r', "")
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_pasted_text, utf8_offset_for_utf16};

    #[test]
    fn converts_ime_selection_offsets_without_splitting_utf8() {
        let value = "a中😀";
        assert_eq!(utf8_offset_for_utf16(value, 0), 0);
        assert_eq!(utf8_offset_for_utf16(value, 1), 1);
        assert_eq!(utf8_offset_for_utf16(value, 2), 4);
        assert_eq!(utf8_offset_for_utf16(value, 3), value.len());
        assert_eq!(utf8_offset_for_utf16(value, 4), value.len());
    }

    #[test]
    fn normalizes_paste_for_single_and_multiline_fields() {
        assert_eq!(normalize_pasted_text("a\r\nb\nc", true), "a b c");
        assert_eq!(normalize_pasted_text("a\r\nb\nc", false), "a\nb\nc");
    }
}

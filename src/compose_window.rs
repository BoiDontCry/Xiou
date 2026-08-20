use crate::composer_editor::{ComposerEvent, ComposerInput};
use crate::theme::Theme;
use foxmail_imap::credentials;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, px, size, App, AppContext, Bounds, Context, Entity, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Styled, Window, WindowBounds,
    WindowOptions,
};
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

pub const SMTP_HOST: &str = "smtp.yunyou.top";
pub const SMTP_PORT: u16 = 465;
pub const FROM_ADDR: &str = "zhangwei@szshuori.com";
const DEFAULT_TO: &str = "2vain0131@gmail.com";

actions!(compose, [CloseCompose]);

#[derive(Clone, Debug)]
pub enum ComposeKind {
    New,
}

#[derive(Clone, Debug)]
enum SendState {
    Idle,
    Sending,
    Success(String),
    Failed(String),
}

pub struct ComposeWindow {
    theme: Theme,
    kind: ComposeKind,
    to: String,
    cc: String,
    subject: String,
    body: Entity<ComposerInput>,
    send_state: SendState,
    _body_subscription: gpui::Subscription,
}

impl ComposeWindow {
    pub fn open(kind: ComposeKind, app: &mut App) {
        let (to, subject, body_initial) = initial_fields(&kind);

        let bounds = Bounds::centered(None, size(px(720.), px(560.)), app);
        let _ = app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(compose_title(&kind).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |window, app| {
                app.new(|cx| {
                    let body = cx.new(|cx| ComposerInput::new(window, cx, body_initial.clone()));
                    let body_subscription = cx.subscribe(
                        &body,
                        |this: &mut ComposeWindow, _body, event: &ComposerEvent, cx| match event {
                            ComposerEvent::Submit => this.attempt_send(cx),
                            ComposerEvent::Cancel => this.close(cx),
                            ComposerEvent::Edited => cx.notify(),
                        },
                    );
                    Self {
                        theme: Theme::dark(),
                        kind,
                        to,
                        cc: String::new(),
                        subject,
                        body,
                        send_state: SendState::Idle,
                        _body_subscription: body_subscription,
                    }
                })
            },
        );
    }

    fn attempt_send(&mut self, cx: &mut Context<Self>) {
        if matches!(self.send_state, SendState::Sending) {
            return;
        }
        let to = self.to.trim().to_string();
        let subject = self.subject.trim().to_string();
        let body = self.body.read(cx).content().trim().to_string();
        if to.is_empty() {
            self.send_state = SendState::Failed("收件人不能为空".into());
            cx.notify();
            return;
        }
        if body.is_empty() {
            self.send_state = SendState::Failed("正文不能为空".into());
            cx.notify();
            return;
        }
        self.send_state = SendState::Sending;
        cx.notify();

        let cc = self.cc.trim().to_string();
        let task = cx
            .background_executor()
            .spawn(async move { send_email(&to, &cc, &subject, &body) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.send_state = match result {
                    Ok(info) => SendState::Success(info),
                    Err(error) => SendState::Failed(error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.dispatch_action(&CloseCompose);
    }

    fn handle_to_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_text_field(event, TextTarget::To, cx);
    }

    fn handle_cc_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_text_field(event, TextTarget::Cc, cx);
    }

    fn handle_subject_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_text_field(event, TextTarget::Subject, cx);
    }

    fn handle_text_field(
        &mut self,
        event: &KeyDownEvent,
        target: TextTarget,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("escape") {
            self.close(cx);
            return;
        }
        if (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
            && key.eq_ignore_ascii_case("enter")
        {
            self.attempt_send(cx);
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        if key.eq_ignore_ascii_case("backspace") {
            match target {
                TextTarget::To => self.to.pop(),
                TextTarget::Cc => self.cc.pop(),
                TextTarget::Subject => self.subject.pop(),
            };
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("enter") {
            return;
        }
        let character = event
            .keystroke
            .key_char
            .as_deref()
            .or_else(|| (event.keystroke.key.chars().count() == 1).then_some(key));
        let Some(character) = character else { return };
        if character.chars().all(char::is_control) {
            return;
        }
        match target {
            TextTarget::To => self.to.push_str(character),
            TextTarget::Cc => self.cc.push_str(character),
            TextTarget::Subject => self.subject.push_str(character),
        }
        cx.notify();
    }

    fn render_header_field(
        &self,
        theme: Theme,
        label: &str,
        value: &str,
        target: TextTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let placeholder = value.is_empty();
        let placeholder_text: SharedString = if placeholder {
            header_placeholder(label).into()
        } else {
            SharedString::default()
        };
        let value_text: SharedString = value.to_owned().into();
        let label_text: SharedString = label.to_owned().into();
        let display: SharedString = if placeholder {
            placeholder_text
        } else {
            value_text
        };
        #[allow(clippy::type_complexity)]
        let listener: Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App)> = match target {
            TextTarget::To => Box::new(cx.listener(Self::handle_to_key)),
            TextTarget::Cc => Box::new(cx.listener(Self::handle_cc_key)),
            TextTarget::Subject => Box::new(cx.listener(Self::handle_subject_key)),
        };
        div()
            .h(px(36.))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .cursor(gpui::CursorStyle::IBeam)
            .hover(|style| style.bg(theme.raised))
            .on_key_down(listener)
            .child(
                div()
                    .w(px(56.))
                    .flex_shrink_0()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child(label_text),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(if placeholder {
                        theme.text_tertiary
                    } else {
                        theme.text
                    })
                    .text_sm()
                    .child(display),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let sending = matches!(self.send_state, SendState::Sending);
        let status: SharedString = match &self.send_state {
            SendState::Idle => "Ctrl/Cmd+Enter 发送 · Esc 关闭".into(),
            SendState::Sending => "正在发送…".into(),
            SendState::Success(info) => format!("已发送：{}", info).into(),
            SendState::Failed(error) => format!("发送失败：{}", error).into(),
        };
        div()
            .h(px(48.))
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id("send-button")
                            .px_3()
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .bg(theme.accent)
                            .text_color(gpui::hsla(0., 0., 0., 1.))
                            .text_sm()
                            .when(sending, |el| el.opacity(0.5))
                            .child(if sending { "发送中…" } else { "发送" })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.attempt_send(cx)),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child(status),
                    ),
            )
    }
}

impl Render for ComposeWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let header_to = self.render_header_field(theme, "收件人", &self.to, TextTarget::To, cx);
        let header_cc = self.render_header_field(theme, "抄送", &self.cc, TextTarget::Cc, cx);
        let header_subject =
            self.render_header_field(theme, "主题", &self.subject, TextTarget::Subject, cx);
        let body = self.body.clone();

        div()
            .size_full()
            .bg(theme.canvas)
            .text_color(theme.text)
            .flex()
            .flex_col()
            .child(self.render_toolbar(cx))
            .child(header_to)
            .child(header_cc)
            .child(header_subject)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.surface)
                    .child(body.clone()),
            )
            .child(
                div()
                    .h(px(28.))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child(compose_hint(&self.kind)),
            )
    }
}

#[derive(Clone, Copy)]
enum TextTarget {
    To,
    Cc,
    Subject,
}

fn initial_fields(kind: &ComposeKind) -> (String, String, String) {
    match kind {
        ComposeKind::New => (DEFAULT_TO.to_string(), String::new(), String::new()),
    }
}

fn compose_title(kind: &ComposeKind) -> &'static str {
    match kind {
        ComposeKind::New => "写信",
    }
}

fn compose_hint(kind: &ComposeKind) -> SharedString {
    let text = match kind {
        ComposeKind::New => "新邮件",
    };
    format!("{} · Ctrl/Cmd+Enter 发送 · Esc 关闭", text).into()
}

fn header_placeholder(label: &str) -> String {
    match label {
        "收件人" => "输入收件人邮箱地址".to_owned(),
        "抄送" => "可选抄送".to_owned(),
        "主题" => "输入主题".to_owned(),
        _ => String::new(),
    }
}

fn send_email(to: &str, cc: &str, subject: &str, body: &str) -> Result<String, String> {
    let password = credentials::load_or_initialize(false).map_err(|e| e.to_string())?;
    let from: Mailbox = FROM_ADDR
        .parse()
        .map_err(|e| format!("发件人地址无效：{e}"))?;
    let to_mailbox: Mailbox = to.parse().map_err(|e| format!("收件人地址无效：{e}"))?;
    let mut builder = Message::builder()
        .from(from)
        .to(to_mailbox)
        .subject(subject);
    for address in cc.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(mb) = address.parse::<Mailbox>() {
            builder = builder.cc(mb);
        }
    }
    let email = builder
        .body(body.to_string())
        .map_err(|e| format!("构造邮件失败：{e}"))?;
    let transport = SmtpTransport::relay(SMTP_HOST)
        .map_err(|e| format!("SMTP 连接配置失败：{e}"))?
        .port(SMTP_PORT)
        .credentials(Credentials::new(
            FROM_ADDR.to_string(),
            password.as_str().to_string(),
        ))
        .build();
    match transport.send(&email) {
        Ok(response) => Ok(format!("code={}", response.code())),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_config_matches_poc() {
        assert_eq!(SMTP_HOST, "smtp.yunyou.top");
        assert_eq!(SMTP_PORT, 465);
        assert_eq!(FROM_ADDR, "zhangwei@szshuori.com");
    }

    #[test]
    fn initial_fields_default_to_gmail_recipient_for_new() {
        let (to, subject, body) = initial_fields(&ComposeKind::New);
        assert_eq!(to, "2vain0131@gmail.com");
        assert!(subject.is_empty());
        assert!(body.is_empty());
    }
}

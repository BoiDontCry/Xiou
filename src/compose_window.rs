use crate::composer_editor::{ComposerEvent, ComposerInput, ComposerInputOptions};
use crate::draft_store::{self, DraftData};
use crate::theme::Theme;
use crate::ui::{control_button, rems, ButtonKind};
use foxmail_imap::credentials;
use foxmail_imap::mail_store::MessageDetail;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, px, size, App, AppContext, Bounds, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    WindowBounds, WindowOptions,
};
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

pub const SMTP_HOST: &str = "smtp.yunyou.top";
pub const SMTP_PORT: u16 = 465;
pub const FROM_ADDR: &str = "zhangwei@szshuori.com";

actions!(compose, [CloseCompose, SendCompose]);

#[derive(Clone, Debug)]
pub enum ComposeKind {
    New,
    Reply(ReplyDraft),
    Forward(ForwardDraft),
}

#[derive(Clone, Debug)]
pub struct ReplyDraft {
    pub message_id: i64,
    pub recipient: String,
    pub subject: String,
    pub quoted_body: String,
}

#[derive(Clone, Debug)]
pub struct ForwardDraft {
    pub message_id: i64,
    pub subject: String,
    pub quoted_body: String,
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
    to: Entity<ComposerInput>,
    cc: Entity<ComposerInput>,
    subject: Entity<ComposerInput>,
    body: Entity<ComposerInput>,
    send_state: SendState,
    _input_subscriptions: Vec<gpui::Subscription>,
}

impl ComposeWindow {
    pub fn open(kind: ComposeKind, app: &mut App) {
        let (to, cc, subject, body_initial) = draft_fields(&kind);

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
                    let to = cx.new(|cx| {
                        ComposerInput::new(
                            window,
                            cx,
                            to.clone(),
                            ComposerInputOptions::single_line("输入收件人地址"),
                        )
                    });
                    let cc = cx.new(|cx| {
                        ComposerInput::new(
                            window,
                            cx,
                            cc.clone(),
                            ComposerInputOptions::single_line("输入抄送地址，可用逗号分隔"),
                        )
                    });
                    let subject = cx.new(|cx| {
                        ComposerInput::new(
                            window,
                            cx,
                            subject.clone(),
                            ComposerInputOptions::single_line("输入主题"),
                        )
                    });
                    let body = cx.new(|cx| {
                        ComposerInput::new(
                            window,
                            cx,
                            body_initial.clone(),
                            ComposerInputOptions::multiline("在这里输入邮件正文…"),
                        )
                    });
                    let input_subscriptions = vec![
                        subscribe_input(cx, &to),
                        subscribe_input(cx, &cc),
                        subscribe_input(cx, &subject),
                        subscribe_input(cx, &body),
                    ];
                    window.focus(&to.read(cx).focus_handle(), cx);
                    Self {
                        theme: Theme::from_env(),
                        kind,
                        to,
                        cc,
                        subject,
                        body,
                        send_state: SendState::Idle,
                        _input_subscriptions: input_subscriptions,
                    }
                })
            },
        );
    }

    pub fn reply(detail: &MessageDetail, app: &mut App) {
        Self::open(ComposeKind::Reply(reply_draft(detail)), app);
    }

    pub fn forward(detail: &MessageDetail, app: &mut App) {
        Self::open(ComposeKind::Forward(forward_draft(detail)), app);
    }

    fn attempt_send(&mut self, cx: &mut Context<Self>) {
        if matches!(self.send_state, SendState::Sending) {
            return;
        }
        let to = self.to.read(cx).content().trim().to_string();
        let subject = self.subject.read(cx).content().trim().to_string();
        let body = self.body.read(cx).content().to_string();
        if to.is_empty() {
            self.send_state = SendState::Failed("收件人不能为空".into());
            cx.notify();
            return;
        }
        if body.trim().is_empty() {
            self.send_state = SendState::Failed("正文不能为空".into());
            cx.notify();
            return;
        }
        self.send_state = SendState::Sending;
        cx.notify();

        let cc = self.cc.read(cx).content().trim().to_string();
        let task = cx
            .background_executor()
            .spawn(async move { send_email(&to, &cc, &subject, &body) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.send_state = match result {
                    Ok(info) => {
                        let _ = draft_store::remove(&draft_key(&this.kind));
                        SendState::Success(info)
                    }
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

    fn close_window(&mut self, _: &CloseCompose, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn send_from_action(&mut self, _: &SendCompose, _: &mut Window, cx: &mut Context<Self>) {
        self.attempt_send(cx);
    }

    fn render_header_field(
        &self,
        theme: Theme,
        label: &str,
        input: Entity<ComposerInput>,
    ) -> impl IntoElement {
        let label_text: SharedString = label.to_owned().into();
        div()
            .h(rems(2.25))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .hover(|style| style.bg(theme.raised))
            .child(
                div()
                    .w(rems(3.5))
                    .flex_shrink_0()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child(label_text),
            )
            .child(div().flex_1().min_w_0().h_full().child(input))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let sending = matches!(self.send_state, SendState::Sending);
        let status_color = match &self.send_state {
            SendState::Idle => theme.text_tertiary,
            SendState::Sending => theme.info,
            SendState::Success(_) => theme.success,
            SendState::Failed(_) => theme.danger,
        };
        let status: SharedString = match &self.send_state {
            SendState::Idle => "Ctrl/Cmd+Enter 发送 · Esc 关闭".into(),
            SendState::Sending => "正在发送…".into(),
            SendState::Success(info) => format!("已发送：{}", info).into(),
            SendState::Failed(error) => format!("发送失败：{}", error).into(),
        };
        div()
            .h(rems(3.))
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
                        control_button("send-button", theme, ButtonKind::Primary, sending)
                            .h(rems(1.875))
                            .px(rems(0.75))
                            .text_sm()
                            .child(if sending { "发送中…" } else { "发送" })
                            .when(!sending, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.attempt_send(cx)))
                            }),
                    )
                    .child(div().text_color(status_color).text_xs().child(status)),
            )
    }
}

impl Render for ComposeWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let header_to = self.render_header_field(theme, "收件人", self.to.clone());
        let header_cc = self.render_header_field(theme, "抄送", self.cc.clone());
        let header_subject = self.render_header_field(theme, "主题", self.subject.clone());
        let body = self.body.clone();

        div()
            .size_full()
            .key_context("Compose")
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::send_from_action))
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
                    .h(rems(1.75))
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

fn initial_fields(kind: &ComposeKind) -> (String, String, String) {
    match kind {
        ComposeKind::New => (String::new(), String::new(), String::new()),
        ComposeKind::Reply(draft) => (
            draft.recipient.clone(),
            draft.subject.clone(),
            draft.quoted_body.clone(),
        ),
        ComposeKind::Forward(draft) => (
            String::new(),
            draft.subject.clone(),
            draft.quoted_body.clone(),
        ),
    }
}

fn draft_fields(kind: &ComposeKind) -> (String, String, String, String) {
    let (to, subject, body) = initial_fields(kind);
    let fallback = DraftData {
        to,
        subject,
        body,
        ..Default::default()
    };
    let draft = draft_store::load(&draft_key(kind))
        .unwrap_or(None)
        .unwrap_or(fallback);
    (draft.to, draft.cc, draft.subject, draft.body)
}

fn draft_key(kind: &ComposeKind) -> String {
    match kind {
        ComposeKind::New => "new".to_string(),
        ComposeKind::Reply(draft) => format!("reply-{}", draft.message_id),
        ComposeKind::Forward(draft) => format!("forward-{}", draft.message_id),
    }
}

fn persist_draft(window: &ComposeWindow, cx: &App) {
    let draft = DraftData {
        to: window.to.read(cx).content().to_string(),
        cc: window.cc.read(cx).content().to_string(),
        subject: window.subject.read(cx).content().to_string(),
        body: window.body.read(cx).content().to_string(),
    };
    let _ = draft_store::save(&draft_key(&window.kind), &draft);
}

fn compose_title(kind: &ComposeKind) -> &'static str {
    match kind {
        ComposeKind::New => "写信",
        ComposeKind::Reply(_) => "回复邮件",
        ComposeKind::Forward(_) => "转发邮件",
    }
}

fn compose_hint(kind: &ComposeKind) -> SharedString {
    let text = match kind {
        ComposeKind::New => "新邮件",
        ComposeKind::Reply(_) => "回复",
        ComposeKind::Forward(_) => "转发",
    };
    format!("{} · Ctrl/Cmd+Enter 发送 · Esc 关闭", text).into()
}

fn reply_draft(detail: &MessageDetail) -> ReplyDraft {
    ReplyDraft {
        message_id: detail.id,
        recipient: detail.sender.clone(),
        subject: prefixed_subject("Re: ", &detail.subject),
        quoted_body: quoted_body(detail),
    }
}

fn forward_draft(detail: &MessageDetail) -> ForwardDraft {
    ForwardDraft {
        message_id: detail.id,
        subject: prefixed_subject("Fwd: ", &detail.subject),
        quoted_body: quoted_body(detail),
    }
}

fn prefixed_subject(prefix: &str, subject: &str) -> String {
    let subject = subject.trim();
    if subject
        .get(..prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
    {
        subject.to_string()
    } else if subject.is_empty() {
        prefix.trim_end().to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

fn quoted_body(detail: &MessageDetail) -> String {
    let body = if detail.body.trim().is_empty() {
        html_to_plain_text(&detail.html_body)
    } else {
        detail.body.clone()
    };
    let date = detail.date_text.as_deref().unwrap_or("未知时间");
    let header = format!(
        "\n\n--- 原始邮件 ---\n发件人：{}\n时间：{}\n主题：{}\n\n",
        detail.sender, date, detail.subject
    );
    let quoted = body
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}{quoted}")
}

fn html_to_plain_text(value: &str) -> String {
    let mut output = String::new();
    let mut inside_tag = false;
    for character in value.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(character),
            _ => {}
        }
    }
    output.trim().to_string()
}

fn subscribe_input(
    cx: &mut Context<ComposeWindow>,
    input: &Entity<ComposerInput>,
) -> gpui::Subscription {
    cx.subscribe(
        input,
        |this: &mut ComposeWindow, _input, event: &ComposerEvent, cx| match event {
            ComposerEvent::Submit => this.attempt_send(cx),
            ComposerEvent::Cancel => this.close(cx),
            ComposerEvent::Edited => {
                persist_draft(this, cx);
                cx.notify();
            }
            ComposerEvent::NavigatePrevious | ComposerEvent::NavigateNext => {}
        },
    )
}

fn send_email(to: &str, cc: &str, subject: &str, body: &str) -> Result<String, String> {
    let password = credentials::load_or_initialize(false).map_err(|e| e.to_string())?;
    let from: Mailbox = FROM_ADDR
        .parse()
        .map_err(|e| format!("发件人地址无效：{e}"))?;
    let to_mailbox: Mailbox = to.parse().map_err(|e| format!("收件人地址无效：{e}"))?;
    let cc_mailboxes: Result<Vec<Mailbox>, _> = cc
        .split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::parse)
        .collect();
    let cc_mailboxes = cc_mailboxes.map_err(|e| format!("抄送地址无效：{e}"))?;
    let mut builder = Message::builder()
        .from(from)
        .to(to_mailbox)
        .subject(subject);
    for mailbox in cc_mailboxes {
        builder = builder.cc(mailbox);
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
    fn initial_fields_are_blank_for_new_message() {
        let (to, subject, body) = initial_fields(&ComposeKind::New);
        assert!(to.is_empty());
        assert!(subject.is_empty());
        assert!(body.is_empty());
    }

    #[test]
    fn rejects_invalid_cc_addresses_before_smtp_connection() {
        let cc_mailboxes: Result<Vec<Mailbox>, _> = "valid@example.com, invalid"
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::parse)
            .collect();
        assert!(cc_mailboxes.is_err());
    }

    #[test]
    fn reply_and_forward_preserve_message_context_without_duplicate_prefixes() {
        let detail = MessageDetail {
            id: 1,
            mailbox_id: 1,
            uid: 1,
            subject: "Re: 进度确认".into(),
            sender: "Sender <sender@example.com>".into(),
            recipients: "recipient@example.com".into(),
            date_text: Some("2026-08-24 10:00".into()),
            date_unix: None,
            body: "第一行\n第二行".into(),
            html_body: String::new(),
            attachment_count: 0,
            attachments: Vec::new(),
            attachment_load_error: None,
            size_bytes: 0,
            eml_path: String::new(),
        };
        let reply = reply_draft(&detail);
        let forward = forward_draft(&detail);
        assert_eq!(reply.recipient, detail.sender);
        assert_eq!(reply.subject, "Re: 进度确认");
        assert_eq!(forward.subject, "Fwd: Re: 进度确认");
        assert!(reply.quoted_body.contains("> 第一行\n> 第二行"));
    }
}

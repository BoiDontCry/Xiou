use crate::draft_store::{self, DraftData};
use crate::feedback::{self, Feedback, NoticeKind, NoticeLevel};
use crate::theme::Theme;
use crate::ui::{control_button, rems, ButtonKind};
use foxmail_imap::account_config::{self, SmtpConfig};
use foxmail_imap::mail_store::MessageDetail;
use foxmail_imap::smtp::{self, SendRequest};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, px, size, App, AppContext, Bounds, Context, Entity, EntityInputHandler,
    EventEmitter, Focusable as _, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};

actions!(compose, [CloseCompose, SendCompose]);

/// Override the library's secondary-enter only inside composer inputs. On
/// Windows it is the same key as ctrl-enter; the more specific, later binding
/// must run before Textarea inserts a newline and emits PressEnter.
pub fn input_send_binding() -> KeyBinding {
    KeyBinding::new("secondary-enter", SendCompose, Some("Compose > Input"))
}

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
    Sending {
        generation: u64,
        request: Box<SendRequest>,
        draft_snapshot: DraftData,
    },
    Success {
        detail: String,
        warning: Option<String>,
    },
    Failed(String),
}

pub struct ComposeWindow {
    theme: Theme,
    kind: ComposeKind,
    to: Entity<InputState>,
    cc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    /// 发件身份：账号配置里的收信用户名。None = 未指定（取第一个带 SMTP 的账号）。
    sender_user: Option<String>,
    /// 窗口打开时刻的发件配置快照，用于只读展示；发送身份以点击发送时刻重新加载的配置为准。
    outgoing_display: Option<SmtpConfig>,
    send_state: SendState,
    send_generation: u64,
    draft_error: Option<SharedString>,
    feedback: Feedback,
    _input_subscriptions: Vec<gpui::Subscription>,
}

impl ComposeWindow {
    fn new(
        kind: ComposeKind,
        sender_user: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (to_initial, cc_initial, subject_initial, body_initial) = draft_fields(&kind);
        let accounts = account_config::list().unwrap_or_default();
        let outgoing_display = resolve_sender(&accounts, &sender_user).map(|(_, smtp)| smtp);
        let sender_user = Some(
            resolve_sender(&accounts, &sender_user)
                .map(|(user, _)| user)
                .unwrap_or_else(|| sender_user.unwrap_or_default()),
        );

        let to = cx.new(|cx| InputState::new(window, cx).placeholder("输入收件人地址"));
        to.update(cx, |state, cx| state.set_value(to_initial, window, cx));
        let cc = cx.new(|cx| InputState::new(window, cx).placeholder("输入抄送地址，可用逗号分隔"));
        cc.update(cx, |state, cx| state.set_value(cc_initial, window, cx));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("输入主题"));
        subject.update(cx, |state, cx| state.set_value(subject_initial, window, cx));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("在这里输入邮件正文…"));
        body.update(cx, |state, cx| state.set_value(body_initial, window, cx));
        let input_subscriptions = vec![
            subscribe_input(cx, &to),
            subscribe_input(cx, &cc),
            subscribe_input(cx, &subject),
            subscribe_input(cx, &body),
        ];
        window.focus(&to.read(cx).focus_handle(cx), cx);
        Self {
            theme: Theme::from_env(),
            kind,
            to,
            cc,
            subject,
            body,
            sender_user,
            outgoing_display,
            send_state: SendState::Idle,
            send_generation: 0,
            draft_error: None,
            feedback: Feedback::default(),
            _input_subscriptions: input_subscriptions,
        }
    }

    pub fn open(kind: ComposeKind, app: &mut App) {
        Self::open_with_sender(kind, None, app);
    }

    /// 带 `sender_user`（收信用户名，即账号身份）打开写信窗口。
    pub fn open_with_sender(kind: ComposeKind, sender_user: Option<String>, app: &mut App) {
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
                let view = app.new(|cx| Self::new(kind, sender_user, window, cx));
                crate::ui::root_view(view, window, app)
            },
        );
    }

    pub fn reply(detail: &MessageDetail, app: &mut App) {
        Self::open(ComposeKind::Reply(reply_draft(detail)), app);
    }

    pub fn forward(detail: &MessageDetail, app: &mut App) {
        Self::open(ComposeKind::Forward(forward_draft(detail)), app);
    }

    fn attempt_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((generation, request, snapshot)) = self.begin_send(window, cx) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            smtp::send(&request)
                .map(|receipt| format!("code={}", receipt.code))
                .map_err(|error| error.message())
        });
        self.dispatch_send(generation, snapshot, task, cx);
    }

    fn begin_send(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(u64, SendRequest, DraftData)> {
        if matches!(self.send_state, SendState::Sending { .. })
            || input_is_composing(&self.to, window, cx)
            || input_is_composing(&self.cc, window, cx)
            || input_is_composing(&self.subject, window, cx)
            || input_is_composing(&self.body, window, cx)
        {
            return None;
        }
        let snapshot = current_draft(self, cx);
        if snapshot.to.trim().is_empty() {
            self.reject_send("收件人不能为空", cx);
            return None;
        }
        if snapshot.body.trim().is_empty() {
            self.reject_send("正文不能为空", cx);
            return None;
        }
        // 点击发送时刻的账号快照：按发件身份加载、验证并冻结为不可变请求。
        // 此后发生的设置变化不影响本次发送。
        let accounts = match account_config::list() {
            Ok(accounts) => accounts,
            Err(error) => {
                self.reject_send(&format!("读取账号配置失败：{error}"), cx);
                return None;
            }
        };
        let account = match resolve_sender(&accounts, &self.sender_user) {
            Some((_, smtp)) if accounts.iter().any(|a| a.outgoing.as_ref() == Some(&smtp)) => {
                accounts
                    .into_iter()
                    .find(|a| a.outgoing.as_ref() == Some(&smtp))
                    .unwrap()
            }
            _ => {
                self.reject_send("未保存邮箱账号或未配置发件（SMTP），请先在设置中配置", cx);
                return None;
            }
        };
        if let Err(error) = account.validate() {
            self.reject_send(&error, cx);
            return None;
        }
        let request = match SendRequest::build(
            account.outgoing,
            &snapshot.to,
            &snapshot.cc,
            snapshot.subject.trim(),
            &snapshot.body,
        ) {
            Ok(request) => request,
            Err(error) => {
                self.reject_send(&error.message(), cx);
                return None;
            }
        };
        self.save_current_draft(cx);
        self.send_generation = self.send_generation.wrapping_add(1);
        let generation = self.send_generation;
        self.send_state = SendState::Sending {
            generation,
            request: Box::new(request.clone()),
            draft_snapshot: snapshot.clone(),
        };
        cx.notify();
        Some((generation, request, snapshot))
    }

    fn reject_send(&mut self, error: &str, cx: &mut Context<Self>) {
        let repeated = matches!(&self.send_state, SendState::Failed(previous) if previous == error);
        self.send_state = SendState::Failed(error.to_owned());
        if !repeated {
            self.feedback.push(
                NoticeKind::Send,
                NoticeLevel::Error,
                "无法发送",
                error.to_owned(),
            );
        }
        cx.notify();
    }

    // The SMTP adapter and offline tests both deliver results through this
    // same callback. Tests supply a local Task, never a real SMTP transport.
    fn dispatch_send(
        &self,
        generation: u64,
        snapshot: DraftData,
        task: gpui::Task<Result<String, String>>,
        cx: &mut Context<Self>,
    ) {
        let key = draft_key(&self.kind);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.complete_send(generation, result, cx));
            } else if result.is_ok() {
                // Closing the compose window does not mean the SMTP job was
                // cancelled. Only remove an unchanged transmitted snapshot;
                // another window's/newer draft must survive.
                let _ = clear_sent_draft(&key, &snapshot);
            }
        })
        .detach();
    }

    fn complete_send(
        &mut self,
        generation: u64,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        let SendState::Sending {
            generation: active,
            draft_snapshot: snapshot,
            ..
        } = &self.send_state
        else {
            return;
        };
        if generation != *active {
            return;
        }
        let snapshot = snapshot.clone();
        match result {
            Ok(info) => {
                let cleanup = if current_draft(self, cx) == snapshot {
                    clear_sent_draft(&draft_key(&self.kind), &snapshot)
                } else {
                    Ok(false)
                };
                let (detail, warning) = match cleanup {
                    Ok(true) => (info, None),
                    Ok(false) => (format!("{info}；发送期间的新修改未被清空。"), None),
                    Err(error) => (
                        info,
                        Some(format!(
                            "本地草稿清理失败：{error}。邮件已发送，请勿重复发送。"
                        )),
                    ),
                };
                let level = if warning.is_some() {
                    NoticeLevel::Warning
                } else {
                    NoticeLevel::Success
                };
                let message = warning.clone().unwrap_or_else(|| detail.clone());
                self.send_state = SendState::Success { detail, warning };
                self.feedback
                    .push(NoticeKind::Send, level, "邮件已发送", message);
            }
            Err(error) => {
                self.send_state = SendState::Failed(error.clone());
                self.feedback
                    .push(NoticeKind::Send, NoticeLevel::Error, "发送失败", error);
            }
        }
        cx.notify();
    }

    fn save_current_draft(&mut self, cx: &mut Context<Self>) {
        match persist_draft(self, cx) {
            Ok(()) => {
                if self.draft_error.take().is_some() {
                    self.feedback.push(
                        NoticeKind::Draft,
                        NoticeLevel::Info,
                        "草稿已恢复保存",
                        "本地草稿保存已恢复，当前修改已保存。",
                    );
                }
            }
            Err(error) => {
                let message: SharedString =
                    format!("草稿保存失败：{error}。关闭窗口前请保留正文副本。").into();
                if self.draft_error.as_ref() != Some(&message) {
                    self.feedback.push(
                        NoticeKind::Draft,
                        NoticeLevel::Error,
                        "草稿未保存",
                        message.clone(),
                    );
                }
                self.draft_error = Some(message);
            }
        }
        cx.notify();
    }

    fn render_send_status(&self) -> Option<gpui::AnyElement> {
        let (level, message, busy) = match &self.send_state {
            SendState::Idle => return None,
            SendState::Sending { request, .. } => (
                NoticeLevel::Info,
                format!(
                    "正在以 {} 发送，等待服务器确认…（发送身份已在点击时冻结）",
                    request.account.from_address
                ),
                true,
            ),
            SendState::Success { detail, warning } => (
                if warning.is_some() {
                    NoticeLevel::Warning
                } else {
                    NoticeLevel::Success
                },
                warning
                    .clone()
                    .unwrap_or_else(|| format!("已发送：{detail}")),
                false,
            ),
            SendState::Failed(error) => (NoticeLevel::Error, format!("发送失败：{error}"), false),
        };
        Some(
            div()
                .id("send-status-region")
                .test_support()
                .flex_shrink_0()
                .max_h(rems(6.))
                .overflow_y_scroll()
                .child(feedback::alert("send-status", level, message).banner())
                .when(busy, |panel| {
                    panel.child(feedback::progress("send-progress", None, "邮件发送进度"))
                })
                .into_any_element(),
        )
    }

    fn close_window(&mut self, _: &CloseCompose, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn send_from_action(&mut self, _: &SendCompose, window: &mut Window, cx: &mut Context<Self>) {
        self.attempt_send(window, cx);
    }

    fn render_header_field(
        &self,
        theme: Theme,
        label: &str,
        input: Entity<InputState>,
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
            .child(
                div().flex_1().min_w_0().h_full().child(
                    Input::new(&input)
                        .appearance(false)
                        .bordered(false)
                        .h_full(),
                ),
            )
    }

    fn render_sender_row(&self, theme: Theme) -> impl IntoElement {
        let sender: SharedString = match &self.outgoing_display {
            Some(smtp) => smtp.from_address.clone().into(),
            None => "未配置发件账号（仅可收信）".into(),
        };
        div()
            .h(rems(2.25))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.raised)
            .child(
                div()
                    .w(rems(3.5))
                    .flex_shrink_0()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child("发件人"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child(sender),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let sending = matches!(self.send_state, SendState::Sending { .. });
        div()
            .h(rems(3.))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                control_button("send-button", theme, ButtonKind::Primary, sending)
                    .h(rems(1.875))
                    .px(rems(0.75))
                    .flex_shrink_0()
                    .text_sm()
                    .child(if sending { "发送中…" } else { "发送" })
                    .on_click(cx.listener(|this, _, window, cx| this.attempt_send(window, cx))),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child("Ctrl/Cmd+Enter 发送 · Esc 关闭"),
            )
    }
}

impl Render for ComposeWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.feedback.flush(window, cx);
        self.theme = Theme::from_env();
        let theme = self.theme;
        let header_to = self.render_header_field(theme, "收件人", self.to.clone());
        let header_cc = self.render_header_field(theme, "抄送", self.cc.clone());
        let header_subject = self.render_header_field(theme, "主题", self.subject.clone());
        let body = self.body.clone();

        div()
            .relative()
            .size_full()
            .key_context("Compose")
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::send_from_action))
            .bg(theme.canvas)
            .text_color(theme.text)
            .flex()
            .flex_col()
            .child(self.render_toolbar(cx))
            .children(self.render_send_status())
            .child(self.render_sender_row(theme))
            .when_some(self.draft_error.clone(), |view, error| {
                view.child(
                    div()
                        .id("draft-save-status-region")
                        .test_support()
                        .max_h(rems(5.))
                        .overflow_y_scroll()
                        .child(
                            feedback::alert("draft-save-error", NoticeLevel::Error, error).banner(),
                        ),
                )
            })
            .child(header_to)
            .child(header_cc)
            .child(header_subject)
            .child(
                div().flex_1().min_h_0().bg(theme.surface).child(
                    Textarea::new(&body)
                        .appearance(false)
                        .bordered(false)
                        .size_full(),
                ),
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
            .children(crate::ui::overlay_layers(window, cx))
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

/// 从账号列表解析发件身份：优先 `sender_user` 指定且带 SMTP 的账号；
/// 否则第一个带 SMTP 的账号。返回 (收信用户名, SMTP 配置)。
fn resolve_sender(
    accounts: &[foxmail_imap::account_config::AccountConfig],
    sender_user: &Option<String>,
) -> Option<(String, SmtpConfig)> {
    if let Some(user) = sender_user.as_deref().filter(|u| !u.trim().is_empty()) {
        if let Some(account) = accounts
            .iter()
            .find(|a| a.outgoing.is_some() && a.incoming.user.trim() == user.trim())
        {
            let smtp = account.outgoing.clone().unwrap();
            return Some((account.incoming.user.trim().to_owned(), smtp));
        }
    }
    accounts.iter().find(|a| a.outgoing.is_some()).map(|a| {
        (
            a.incoming.user.trim().to_owned(),
            a.outgoing.clone().unwrap(),
        )
    })
}

fn current_draft(window: &ComposeWindow, cx: &App) -> DraftData {
    DraftData {
        to: window.to.read(cx).value().to_string(),
        cc: window.cc.read(cx).value().to_string(),
        subject: window.subject.read(cx).value().to_string(),
        body: window.body.read(cx).value().to_string(),
    }
}

fn persist_draft(window: &ComposeWindow, cx: &App) -> Result<(), String> {
    draft_store::save(&draft_key(&window.kind), &current_draft(window, cx))
        .map_err(|error| error.to_string())
}

fn clear_sent_draft(key: &str, snapshot: &DraftData) -> Result<bool, String> {
    match draft_store::load(key).map_err(|error| error.to_string())? {
        Some(stored) if stored != *snapshot => Ok(false),
        Some(_) => draft_store::remove(key)
            .map(|_| true)
            .map_err(|error| error.to_string()),
        None => Ok(true),
    }
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

pub fn reply_draft(detail: &MessageDetail) -> ReplyDraft {
    ReplyDraft {
        message_id: detail.id,
        recipient: detail.sender.clone(),
        subject: prefixed_subject("Re: ", &detail.subject),
        quoted_body: quoted_body(detail),
    }
}

pub fn forward_draft(detail: &MessageDetail) -> ForwardDraft {
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

fn input_is_composing<T: EntityInputHandler>(
    input: &Entity<T>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    input.update(cx, |state, cx| {
        state.marked_text_range(window, cx).is_some()
    })
}

fn subscribe_input<T: EventEmitter<InputEvent>>(
    cx: &mut Context<ComposeWindow>,
    input: &Entity<T>,
) -> gpui::Subscription {
    cx.subscribe(
        input,
        |this: &mut ComposeWindow, _input, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.save_current_draft(cx);
            }
            // PressEnter is emitted for ordinary Textarea newlines too. It is
            // not the old Submit event: sending goes through SendCompose or
            // the button, both guarded against an active IME composition.
            InputEvent::PressEnter { .. } | InputEvent::Focus | InputEvent::Blur => {}
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer_input_keymap(include_override: bool) -> gpui::Keymap {
        use gpui_kit::component::input::Enter;
        let mut bindings = vec![
            KeyBinding::new(
                "enter",
                Enter {
                    secondary: false,
                    shift: false,
                },
                Some("Input"),
            ),
            KeyBinding::new(
                "shift-enter",
                Enter {
                    secondary: false,
                    shift: true,
                },
                Some("Input"),
            ),
            KeyBinding::new(
                "secondary-enter",
                Enter {
                    secondary: true,
                    shift: false,
                },
                Some("Input"),
            ),
            KeyBinding::new("ctrl-enter", SendCompose, Some("Compose")),
        ];
        if include_override {
            bindings.push(input_send_binding());
        }
        gpui::Keymap::new(bindings)
    }

    fn input_context(parent: &str) -> [gpui::KeyContext; 2] {
        [
            gpui::KeyContext::parse(parent).unwrap(),
            gpui::KeyContext::parse("Input").unwrap(),
        ]
    }

    #[test]
    fn secondary_enter_is_the_platform_modifier_not_a_distinct_key() {
        let modifier = if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        };
        assert_eq!(
            gpui::Keystroke::parse("secondary-enter").unwrap(),
            gpui::Keystroke::parse(modifier).unwrap()
        );
    }

    #[test]
    fn composer_send_precedes_library_enter_without_inserting_a_newline() {
        let input = [gpui::Keystroke::parse("secondary-enter").unwrap()];
        let context = input_context("Compose");
        let (old_bindings, _) = composer_input_keymap(false).bindings_for_input(&input, &context);
        assert!(old_bindings[0]
            .action()
            .as_any()
            .is::<gpui_kit::component::input::Enter>());
        let (bindings, pending) = composer_input_keymap(true).bindings_for_input(&input, &context);
        assert!(!pending);
        assert!(bindings[0].action().as_any().is::<SendCompose>());
    }

    #[test]
    fn plain_and_shift_enter_do_not_send_a_message() {
        let keymap = composer_input_keymap(true);
        for key in ["enter", "shift-enter"] {
            let input = [gpui::Keystroke::parse(key).unwrap()];
            let (bindings, pending) = keymap.bindings_for_input(&input, &input_context("Compose"));
            assert!(!pending);
            assert!(!bindings.is_empty());
            assert!(bindings
                .iter()
                .all(|binding| !binding.action().as_any().is::<SendCompose>()));
        }
    }

    #[test]
    fn compose_override_does_not_capture_other_windows_inputs() {
        let input = [gpui::Keystroke::parse("secondary-enter").unwrap()];
        let (bindings, _) =
            composer_input_keymap(true).bindings_for_input(&input, &input_context("MailApp"));
        assert!(bindings[0]
            .action()
            .as_any()
            .is::<gpui_kit::component::input::Enter>());
        assert!(bindings
            .iter()
            .all(|binding| !binding.action().as_any().is::<SendCompose>()));
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
        let request = SendRequest::build(
            None,
            "valid@example.com",
            "valid@example.com, invalid",
            "主题",
            "正文",
        );
        // SMTP 未配置也先报未配置；地址校验在配置存在时先行于任何网络动作。
        assert!(request.is_err());
        let request = SendRequest::build(
            Some(foxmail_imap::account_config::SmtpConfig {
                host: "smtp.example.com".into(),
                port: 465,
                security: foxmail_imap::account_config::SmtpSecurity::ImplicitTls,
                auth_user: "user@example.com".into(),
                password: "secret".into(),
                from_address: "user@example.com".into(),
            }),
            "valid@example.com",
            "valid@example.com, invalid",
            "主题",
            "正文",
        );
        assert_eq!(request.unwrap_err().category(), "invalid-recipient");
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

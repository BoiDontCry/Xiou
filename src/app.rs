use gpui::{
    App, Context, FontWeight, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, div, px,
};
use gpui::prelude::FluentBuilder as _;

use crate::theme::Theme;

/// 邮件领域模型（mock，M0 后接 IMAP）
#[derive(Clone)]
pub struct Mail {
    pub from: SharedString,
    pub subject: SharedString,
    pub preview: SharedString,
    pub date: SharedString,
    pub unread: bool,
}

/// 邮箱账号（侧栏顶层数据）
#[derive(Clone)]
pub struct Account {
    pub name: SharedString,
    pub address: SharedString,
    pub folders: Vec<Folder>,
}

#[derive(Clone)]
pub struct Folder {
    pub name: SharedString,
    pub unread: usize,
}

impl Account {
    fn mock() -> Vec<Self> {
        vec![Account {
            name: "zhangwei".into(),
            address: "zhangwei@szshuori.com".into(),
            folders: vec![
                Folder { name: "收件箱".into(), unread: 3 },
                Folder { name: "已发送".into(), unread: 0 },
                Folder { name: "草稿箱".into(), unread: 0 },
                Folder { name: "垃圾邮件".into(), unread: 1 },
            ],
        }]
    }
}

fn mock_mails() -> Vec<Mail> {
    let senders = [
        ("tangjl@szshuori.com", "Solar Co.,Ltd 季度复盘"),
        ("ops@szshuori.com", "服务器迁移通知 - 周五 02:00"),
        ("hr@szshuori.com", "8月考勤确认"),
        ("noreply@github.com", "[foxmail-rs] PR #12 merged"),
        ("support@westmail.com", "WESTMail 安全提醒"),
        ("sales@vendor.com", "Re: 报价单确认"),
        ("admin@szshuori.com", "企业邮箱容量 78%"),
        ("newsletter@dev.to", "Weekly Digest"),
        ("jerry@external.com", "周五 lunch?"),
        ("ci@szshuori.com", "Build #451 failed"),
    ];
    senders
        .iter()
        .enumerate()
        .map(|(i, (from, subject))| Mail {
            from: (*from).into(),
            subject: (*subject).into(),
            preview: "这封邮件的预览文本，M2 接 IMAP 后替换为真实正文前两行…".into(),
            date: match i {
                0 => "今天 09:32".into(),
                1 => "今天 08:15".into(),
                2 => "昨天".into(),
                3 => "昨天".into(),
                _ => "8/12".into(),
            },
            unread: i < 4,
        })
        .collect()
}

/// 选中状态
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Sidebar,
    List,
    Reader,
}

pub struct MailApp {
    theme: Theme,
    accounts: Vec<Account>,
    mails: Vec<Mail>,
    selected_mail: usize,
    selected_folder: usize,
    focused: Pane,
    #[allow(dead_code)]
    _load: Task<()>,
    list_scroll: ScrollHandle,
    reader_scroll: ScrollHandle,
    #[allow(dead_code)]
    _subscriptions: Vec<Subscription>,
}

impl MailApp {
    pub fn new(cx: &mut App) -> Self {
        let theme = Theme::dark();
        let accounts = Account::mock();
        let mails = mock_mails();
        let _load = cx.background_executor().spawn(async move {
            // M0 占位：未来在此异步拉 IMAP，写 SQLite，回灌 mails
        });
        Self {
            theme,
            accounts,
            mails,
            selected_mail: 0,
            selected_folder: 0,
            focused: Pane::List,
            _load,
            list_scroll: ScrollHandle::new(),
            reader_scroll: ScrollHandle::new(),
            _subscriptions: Vec::new(),
        }
    }

    fn select_mail(&mut self, ix: usize, _cx: &mut Context<Self>) {
        if ix < self.mails.len() {
            self.mails[ix].unread = false;
            self.selected_mail = ix;
            self.focused = Pane::Reader;
        }
    }
}

impl Render for MailApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;

        div()
            .size_full()
            .bg(t.canvas)
            .text_color(t.text)
            .flex()
            // ===== 侧栏 =====
            .child(self.render_sidebar(t, cx))
            // ===== 列表 =====
            .child(self.render_list(t, cx))
            // ===== 阅读区 =====
            .child(self.render_reader(t))
    }
}

impl MailApp {
    fn render_sidebar(&mut self, t: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let width = px(220.);
        let mut sidebar = div()
            .h_full()
            .w(width)
            .flex_shrink_0()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.border)
            .flex()
            .flex_col();

        // 账号区
        for account in self.accounts.clone() {
            sidebar = sidebar.child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_color(t.text)
                            .text_sm()
                            .child(account.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(t.text_tertiary)
                            .text_xs()
                            .child(account.address.clone()),
                    ),
            );

            for (fix, folder) in account.folders.iter().enumerate() {
                let active = fix == self.selected_folder;
                let unread = folder.unread;
                sidebar = sidebar.child(
                    div()
                        .id(("folder", fix))
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .when(active, |s| s.bg(t.sidebar_item_background))
                        .hover(|s| s.bg(t.sidebar_item_background))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_folder = fix;
                            this.focused = Pane::Sidebar;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_color(if active { t.text } else { t.text_secondary })
                                .child(folder.name.clone()),
                        )
                        .when(unread > 0, |s| {
                            s.child(
                                div()
                                    .text_color(t.accent)
                                    .child(SharedString::from(unread.to_string())),
                            )
                        }),
                );
            }
        }

        // 底部占位
        sidebar = sidebar.child(div().flex_1());

        sidebar
    }

    fn render_list(&mut self, t: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let width = px(360.);
        let mut list = div()
            .h_full()
            .w(width)
            .flex_shrink_0()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.border)
                    .text_color(t.text)
                    .text_sm()
                    .child("收件箱"),
            )
            .child(
                div()
                    .id("mail-list")
                    .flex_1()
                    .track_scroll(&self.list_scroll)
                    .children(
                        self.mails
                            .iter()
                            .enumerate()
                            .map(|(ix, mail)| self.render_mail_row(ix, mail, t, cx)),
                    ),
            );

        // ponytail: 列表底部状态条，M2 后接真实未读统计
        list = list.child(
            div()
                .px_4()
                .py_2()
                .border_t_1()
                .border_color(t.border)
                .text_color(t.text_tertiary)
                .text_xs()
                .child(format!(
                    "{} 封 · {} 未读",
                    self.mails.len(),
                    self.mails.iter().filter(|m| m.unread).count()
                )),
        );

        list
    }

    fn render_mail_row(
        &self,
        ix: usize,
        mail: &Mail,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = ix == self.selected_mail;
        div()
            .id(("mail", ix))
            .w_full()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(t.border)
            .when(selected, |s| s.bg(t.selected))
            .hover(|s| s.bg(t.raised))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_mail(ix, cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(if mail.unread { t.text } else { t.text_secondary })
                            .text_xs()
                            .child(mail.from.clone()),
                    )
                    .child(
                        div()
                            .text_color(t.text_tertiary)
                            .text_xs()
                            .child(mail.date.clone()),
                    ),
            )
            .child(
                div()
                    .mt_1()
                    .text_color(if mail.unread { t.text } else { t.text_secondary })
                    .text_sm()
                    .font_weight(if mail.unread { FontWeight(600.0) } else { FontWeight(400.0) })
                    .child(mail.subject.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_color(t.text_tertiary)
                    .text_xs()
                    .child(mail.preview.clone()),
            )
            .when(mail.unread, |s| {
                s.child(
                    div()
                        .absolute()
                        .left_2()
                        .top_3()
                        .size(px(6.))
                        .rounded_full()
                        .bg(t.accent),
                )
            })
    }

    fn render_reader(&self, t: Theme) -> impl IntoElement {
        let mail = self.mails.get(self.selected_mail);
        let mut reader = div()
            .h_full()
            .flex_1()
            .bg(t.canvas)
            .flex()
            .flex_col();

        match mail {
            Some(mail) => {
                reader = reader
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .border_b_1()
                            .border_color(t.border)
                            .child(
                                div()
                                    .text_color(t.text)
                                    .text_lg()
                                    .child(mail.subject.clone()),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_color(t.text_secondary)
                                            .text_sm()
                                            .child(mail.from.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(t.text_ghost)
                                            .text_xs()
                                            .child(mail.date.clone()),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("mail-body")
                            .flex_1()
                            .px_6()
                            .py_6()
                            .track_scroll(&self.reader_scroll)
                            .text_color(t.text_secondary)
                            .text_sm()
                            .child(
                                "邮件正文区。\n\n\
                                M3 阶段此处渲染真实 MIME 正文（go-message 解析），支持 HTML/markdown。\n\
                                当前为 mock 占位，用于验证三栏布局与滚动。",
                            ),
                    );
            }
            None => {
                reader = reader.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.text_tertiary)
                        .child("未选择邮件"),
                );
            }
        }

        reader
    }
}

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", crate::actions::Quit, None)]);
}

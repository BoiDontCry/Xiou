use foxmail_imap::mail_store::{MailStore, MailboxAccount, MessageSummary};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, uniform_list, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window,
};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::path::PathBuf;

use crate::theme::Theme;

const MESSAGE_PAGE_SIZE: usize = 50;

pub struct MailApp {
    theme: Theme,
    store: Option<MailStore>,
    accounts: Vec<MailboxAccount>,
    selected_mailbox: Option<i64>,
    collapsed_accounts: HashSet<String>,
    load_error: Option<SharedString>,
    message_pages: HashMap<usize, Vec<MessageSummary>>,
    message_load_error: Option<SharedString>,
    message_scroll: UniformListScrollHandle,
}

impl MailApp {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let index_path = index_path();
        let result = if index_path.is_file() {
            MailStore::open_read_only(&index_path).and_then(|store| {
                let accounts = store.mailbox_tree()?;
                Ok((store, accounts))
            })
        } else {
            Err(format!("邮件索引不存在：{}", index_path.display()).into())
        };

        let (store, accounts, load_error) = match result {
            Ok((store, accounts)) => (Some(store), accounts, None),
            Err(error) => (
                None,
                Vec::new(),
                Some(SharedString::from(error.to_string())),
            ),
        };
        let selected_mailbox = accounts
            .iter()
            .flat_map(|account| account.mailboxes.iter())
            .next()
            .map(|mailbox| mailbox.id);

        Self {
            theme: Theme::dark(),
            store,
            accounts,
            selected_mailbox,
            collapsed_accounts: HashSet::new(),
            load_error,
            message_pages: HashMap::new(),
            message_load_error: None,
            message_scroll: UniformListScrollHandle::new(),
        }
    }

    fn toggle_account(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_accounts.remove(&key) {
            self.collapsed_accounts.insert(key);
        }
        cx.notify();
    }

    fn select_mailbox(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.selected_mailbox == Some(id) {
            return;
        }
        self.selected_mailbox = Some(id);
        self.message_pages.clear();
        self.message_load_error = None;
        if self.selected_count() > 0 {
            self.message_scroll
                .scroll_to_item_strict(0, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn selected_name(&self) -> SharedString {
        self.accounts
            .iter()
            .flat_map(|account| account.mailboxes.iter())
            .find(|mailbox| Some(mailbox.id) == self.selected_mailbox)
            .map(|mailbox| {
                SharedString::from(mailbox_label(&mailbox.source_name, &mailbox.display_name))
            })
            .unwrap_or_else(|| "邮件".into())
    }

    fn selected_count(&self) -> usize {
        self.accounts
            .iter()
            .flat_map(|account| account.mailboxes.iter())
            .find(|mailbox| Some(mailbox.id) == self.selected_mailbox)
            .map_or(0, |mailbox| mailbox.message_count)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let total_messages = self
            .accounts
            .iter()
            .flat_map(|account| account.mailboxes.iter())
            .map(|mailbox| mailbox.message_count)
            .sum::<usize>();
        let mut tree = div().id("mailbox-tree").flex_1().overflow_y_scroll().py_2();

        for (account_index, account) in self.accounts.iter().cloned().enumerate() {
            let account_key = format!("{}\0{}", account.account, account.server);
            let collapsed = self.collapsed_accounts.contains(&account_key);
            let toggle_key = account_key.clone();
            tree = tree.child(
                div()
                    .id(("account", account_index))
                    .mx_2()
                    .mt_2()
                    .px_2()
                    .py_2()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.raised))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_account(toggle_key.clone(), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(12.))
                                    .text_color(theme.text_tertiary)
                                    .text_xs()
                                    .child(if collapsed { "›" } else { "⌄" }),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .text_color(theme.text)
                                            .text_sm()
                                            .child(SharedString::from(account.account.clone())),
                                    )
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .text_color(theme.text_tertiary)
                                            .text_xs()
                                            .child(SharedString::from(account.server.clone())),
                                    ),
                            ),
                    ),
            );

            if collapsed {
                continue;
            }

            for mailbox in account.mailboxes {
                let mailbox_id = mailbox.id;
                let active = Some(mailbox_id) == self.selected_mailbox;
                tree = tree.child(
                    div()
                        .id(("mailbox", u64::try_from(mailbox_id).unwrap_or_default()))
                        .relative()
                        .mx_2()
                        .h(px(44.))
                        .pl(px(30.))
                        .pr_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(active, |style| style.bg(theme.active))
                        .hover(|style| style.bg(theme.raised))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_mailbox(mailbox_id, cx);
                        }))
                        .when(active, |element| {
                            element.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(px(10.))
                                    .h(px(24.))
                                    .w(px(2.))
                                    .rounded_r(px(1.))
                                    .bg(theme.accent),
                            )
                        })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_color(if active {
                                    theme.text
                                } else {
                                    theme.text_secondary
                                })
                                .text_sm()
                                .child(SharedString::from(mailbox_label(
                                    &mailbox.source_name,
                                    &mailbox.display_name,
                                ))),
                        )
                        .child(
                            div()
                                .ml_2()
                                .text_color(theme.text_tertiary)
                                .text_xs()
                                .child(SharedString::from(mailbox.message_count.to_string())),
                        ),
                );
            }
        }

        if let Some(error) = &self.load_error {
            tree = tree.child(
                div()
                    .mx_4()
                    .mt_4()
                    .p_3()
                    .rounded(px(4.))
                    .bg(theme.raised)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child(error.clone()),
            );
        } else if self.accounts.is_empty() {
            tree = tree.child(
                div()
                    .px_4()
                    .py_6()
                    .text_color(theme.text_tertiary)
                    .text_sm()
                    .child("暂无邮箱"),
            );
        }

        div()
            .h_full()
            .w(px(248.))
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(54.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(theme.text).text_sm().child("邮箱"))
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child(SharedString::from(total_messages.to_string())),
                    ),
            )
            .child(tree)
    }

    fn load_message_page(&mut self, page_index: usize) {
        if self.message_pages.contains_key(&page_index) || self.message_load_error.is_some() {
            return;
        }
        let Some(mailbox_id) = self.selected_mailbox else {
            return;
        };
        let Some(store) = &self.store else {
            return;
        };

        match store.message_page(
            mailbox_id,
            page_index * MESSAGE_PAGE_SIZE,
            MESSAGE_PAGE_SIZE,
        ) {
            Ok(messages) => {
                self.message_pages.insert(page_index, messages);
            }
            Err(error) => {
                self.message_load_error = Some(SharedString::from(error.to_string()));
            }
        }
    }

    fn message_at(&mut self, index: usize) -> Option<MessageSummary> {
        let page_index = index / MESSAGE_PAGE_SIZE;
        self.load_message_page(page_index);
        self.message_pages
            .get(&page_index)
            .and_then(|page| page.get(index % MESSAGE_PAGE_SIZE))
            .cloned()
    }

    fn render_message_rows(&mut self, range: Range<usize>) -> Vec<AnyElement> {
        let theme = self.theme;
        range
            .filter_map(|index| {
                self.message_at(index).map(|message| {
                    let subject = nonempty_label(&message.subject, "（无主题）");
                    let sender = nonempty_label(&message.sender, "未知发件人");
                    let date = compact_date(message.date_text.as_deref());
                    let metadata = if message.attachment_count == 0 {
                        date
                    } else {
                        format!("{date}  附件 {}", message.attachment_count)
                    };

                    div()
                        .id(("message", u64::try_from(message.id).unwrap_or_default()))
                        .h(px(76.))
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.raised))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .text_color(theme.text_secondary)
                                        .text_xs()
                                        .child(SharedString::from(sender)),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .whitespace_nowrap()
                                        .text_color(theme.text_tertiary)
                                        .text_xs()
                                        .child(SharedString::from(metadata)),
                                ),
                        )
                        .child(
                            div()
                                .mt_2()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text)
                                .text_sm()
                                .child(SharedString::from(subject)),
                        )
                        .into_any_element()
                })
            })
            .collect()
    }

    fn render_message_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let message_count = self.selected_count();
        let list = uniform_list(
            "message-list",
            message_count,
            cx.processor(|this, range, _window, _cx| this.render_message_rows(range)),
        )
        .track_scroll(&self.message_scroll)
        .h_full()
        .flex_1();

        div()
            .h_full()
            .w(px(360.))
            .flex_shrink_0()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(54.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_color(theme.text)
                            .text_sm()
                            .child(self.selected_name()),
                    )
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child(SharedString::from(self.selected_count().to_string())),
                    ),
            )
            .when(message_count > 0, |element| element.child(list))
            .when(message_count == 0 && self.load_error.is_none(), |element| {
                element.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.text_tertiary)
                        .text_sm()
                        .child("此邮箱暂无邮件"),
                )
            })
            .when_some(self.message_load_error.clone(), |element, error| {
                element.child(
                    div()
                        .mx_4()
                        .mt_4()
                        .p_3()
                        .rounded(px(4.))
                        .bg(theme.raised)
                        .text_color(theme.text_secondary)
                        .text_xs()
                        .child(error),
                )
            })
    }

    fn render_reader(&self) -> impl IntoElement {
        let theme = self.theme;
        div()
            .h_full()
            .min_w_0()
            .flex_1()
            .bg(theme.canvas)
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_tertiary)
            .text_sm()
            .child("未选择邮件")
    }
}

impl Render for MailApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_hidden()
            .bg(self.theme.canvas)
            .text_color(self.theme.text)
            .flex()
            .child(self.render_sidebar(cx))
            .child(self.render_message_list(cx))
            .child(self.render_reader())
    }
}

fn index_path() -> PathBuf {
    resolve_index_path(
        std::env::var_os("FOXMAIL_INDEX_PATH"),
        std::env::var_os("FOXMAIL_BACKUP_DIR"),
    )
}

fn resolve_index_path(index: Option<OsString>, backup_root: Option<OsString>) -> PathBuf {
    index.map(PathBuf::from).unwrap_or_else(|| {
        backup_root
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data"))
            .join("mail-index.sqlite")
    })
}

fn mailbox_label(source_name: &str, display_name: &str) -> String {
    match source_name.to_ascii_uppercase().as_str() {
        "INBOX" => "收件箱".to_owned(),
        "SENT" | "SENT ITEMS" | "SENT MESSAGES" => "已发送".to_owned(),
        "DRAFTS" => "草稿箱".to_owned(),
        "TRASH" | "DELETED ITEMS" => "已删除".to_owned(),
        "JUNK" | "SPAM" => "垃圾箱".to_owned(),
        _ => display_name.to_owned(),
    }
}

fn nonempty_label(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn compact_date(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(16).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{compact_date, mailbox_label, nonempty_label, resolve_index_path};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn localizes_standard_mailbox_names_without_overwriting_custom_names() {
        assert_eq!(mailbox_label("INBOX", "INBOX"), "收件箱");
        assert_eq!(mailbox_label("Sent Items", "Sent Items"), "已发送");
        assert_eq!(mailbox_label("&XfJT0ZAB-", "已发送"), "已发送");
    }

    #[test]
    fn index_path_uses_explicit_index_then_configured_backup_root() {
        assert_eq!(
            resolve_index_path(
                Some(OsString::from("custom.sqlite")),
                Some(OsString::from("backup")),
            ),
            PathBuf::from("custom.sqlite")
        );
        assert_eq!(
            resolve_index_path(None, Some(OsString::from("backup"))),
            PathBuf::from("backup").join("mail-index.sqlite")
        );
        assert_eq!(
            resolve_index_path(None, None),
            PathBuf::from("data").join("mail-index.sqlite")
        );
    }

    #[test]
    fn message_labels_handle_empty_and_long_metadata() {
        assert_eq!(nonempty_label("  ", "（无主题）"), "（无主题）");
        assert_eq!(nonempty_label("  主题  ", "（无主题）"), "主题");
        assert_eq!(compact_date(None), "");
        assert_eq!(
            compact_date(Some("2026-08-17 09:00:00 +08:00")),
            "2026-08-17 09:00"
        );
    }
}

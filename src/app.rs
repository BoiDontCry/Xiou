use foxmail_imap::mail_store::{
    AttachmentMeta, MailStore, MailboxAccount, MessageDetail, MessageSummary, SearchHit,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, relative, uniform_list, AnyElement, AppContext, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, ScrollHandle, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use crate::composer_editor::{ComposerEvent, ComposerInput, ComposerInputOptions};
use crate::diagnostics::{
    classify_index_error, index_message, log_index_problem, with_log_failure, with_log_path,
    IndexProblem,
};
use crate::paths::{index_is_missing, AppPaths};
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
    body_scroll: ScrollHandle,
    selected_message: Option<i64>,
    selected_detail: Option<MessageDetail>,
    detail_load_error: Option<SharedString>,
    search_query: String,
    search_results: Vec<SearchHit>,
    search_error: Option<SharedString>,
    search_pending: bool,
    search_selection: usize,
    search_generation: usize,
    search_input: Entity<ComposerInput>,
    _search_subscription: gpui::Subscription,
    paths: AppPaths,
}

impl MailApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                String::new(),
                ComposerInputOptions::single_line("搜索邮件…")
                    .submit_on_enter()
                    .list_navigation(),
            )
        });
        let search_subscription = cx.subscribe(
            &search_input,
            |this: &mut MailApp, _input, event: &ComposerEvent, cx| {
                this.handle_search_event(event, cx)
            },
        );
        let paths = AppPaths::resolve();
        let _ = crate::diagnostics::log_startup(&paths.log, paths.index_source.as_str());
        let (store, accounts, load_error) = if index_is_missing(&paths.index) {
            let problem = IndexProblem::Missing;
            (
                None,
                Vec::new(),
                Some(index_problem_message(
                    &paths,
                    "index_missing",
                    problem,
                    None,
                    "打开邮件索引",
                )),
            )
        } else {
            match MailStore::open_read_only(&paths.index).and_then(|store| {
                let accounts = store.mailbox_tree()?;
                Ok((store, accounts))
            }) {
                Ok((store, accounts)) => (Some(store), accounts, None),
                Err(error) => {
                    let problem = classify_index_error(error.as_ref());
                    let message = index_problem_message(
                        &paths,
                        "index_open_failed",
                        problem,
                        Some(error.as_ref()),
                        "读取邮箱列表",
                    );
                    (None, Vec::new(), Some(message))
                }
            }
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
            body_scroll: ScrollHandle::new(),
            selected_message: None,
            selected_detail: None,
            detail_load_error: None,
            search_query: String::new(),
            search_results: Vec::new(),
            search_error: None,
            search_pending: false,
            search_selection: 0,
            search_generation: 0,
            search_input,
            _search_subscription: search_subscription,
            paths,
        }
    }

    fn toggle_account(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_accounts.remove(&key) {
            self.collapsed_accounts.insert(key);
        }
        cx.notify();
    }

    fn select_mailbox(&mut self, id: i64, clear_search: bool, cx: &mut Context<Self>) {
        let mailbox_changed = self.selected_mailbox != Some(id);
        if !mailbox_changed && !clear_search {
            return;
        }

        if mailbox_changed {
            self.selected_mailbox = Some(id);
            self.message_pages.clear();
            self.message_load_error = None;
            self.selected_message = None;
            self.selected_detail = None;
            self.detail_load_error = None;
            self.body_scroll.set_offset(Default::default());
            if self.selected_count() > 0 {
                self.message_scroll
                    .scroll_to_item_strict(0, ScrollStrategy::Top);
            }
        }
        if clear_search {
            self.clear_search(cx);
        }
        cx.notify();
    }

    fn select_message_from_search(
        &mut self,
        mailbox_id: i64,
        message_id: i64,
        cx: &mut Context<Self>,
    ) {
        let mailbox_changed = self.selected_mailbox != Some(mailbox_id);
        if mailbox_changed {
            self.selected_mailbox = Some(mailbox_id);
            self.message_pages.clear();
            self.message_load_error = None;
            self.selected_message = None;
            self.selected_detail = None;
            self.detail_load_error = None;
            self.body_scroll.set_offset(Default::default());
        }
        self.select_message(message_id, cx);
    }

    fn handle_search_event(&mut self, event: &ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Edited => self.search_edited(cx),
            ComposerEvent::Submit => self.open_selected_search_result(cx),
            ComposerEvent::Cancel => self.clear_search(cx),
            ComposerEvent::NavigatePrevious => self.move_search_selection(false, cx),
            ComposerEvent::NavigateNext => self.move_search_selection(true, cx),
        }
    }

    fn search_edited(&mut self, cx: &mut Context<Self>) {
        self.search_query = self.search_input.read(cx).content().to_owned();
        self.search_selection = 0;
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;

        if self.search_query.trim().is_empty() {
            self.search_results.clear();
            self.search_error = None;
            self.search_pending = false;
            cx.notify();
            return;
        }

        self.search_results.clear();
        self.search_error = None;
        self.search_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.search_generation == generation {
                        this.run_search();
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_results.clear();
        self.search_error = None;
        self.search_pending = false;
        self.search_selection = 0;
        if !self.search_input.read(cx).content().is_empty() {
            self.search_input
                .update(cx, |input, cx| input.set_content(String::new(), cx));
        }
        cx.notify();
    }

    fn move_search_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.search_results.len();
        if count == 0 {
            return;
        }
        self.search_selection = next_search_selection(self.search_selection, count, forward);
        cx.notify();
    }

    fn open_selected_search_result(&mut self, cx: &mut Context<Self>) {
        if let Some(hit) = self.search_results.get(self.search_selection).cloned() {
            self.select_message_from_search(hit.mailbox_id, hit.id, cx);
        }
    }

    fn run_search(&mut self) {
        self.search_pending = false;
        let query = self.search_query.trim();
        if query.is_empty() {
            self.search_results.clear();
            self.search_error = None;
            return;
        }
        let Some(store) = &self.store else {
            self.search_results.clear();
            self.search_error = Some("邮件索引未打开".into());
            return;
        };
        match store.search(query, 100) {
            Ok(results) => {
                self.search_results = results;
                self.search_error = None;
                self.search_selection = self
                    .search_selection
                    .min(self.search_results.len().saturating_sub(1));
            }
            Err(error) => {
                let problem = classify_index_error(error.as_ref());
                self.search_results.clear();
                self.search_selection = 0;
                self.search_error = Some(index_problem_message(
                    &self.paths,
                    "search_failed",
                    problem,
                    Some(error.as_ref()),
                    "搜索邮件",
                ));
            }
        }
    }

    fn select_message(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.selected_message == Some(id) && self.selected_detail.is_some() {
            return;
        }

        self.selected_message = Some(id);
        self.selected_detail = None;
        self.detail_load_error = None;
        self.body_scroll.set_offset(Default::default());
        match self.store.as_ref().map(|store| store.message_detail(id)) {
            Some(Ok(Some(detail))) => {
                self.selected_detail = Some(detail);
            }
            Some(Ok(None)) => {
                self.selected_message = None;
                self.detail_load_error = Some("邮件已不存在，请重新生成索引".into());
            }
            Some(Err(error)) => {
                let problem = classify_index_error(error.as_ref());
                self.detail_load_error = Some(index_problem_message(
                    &self.paths,
                    "message_detail_failed",
                    problem,
                    Some(error.as_ref()),
                    "读取邮件详情",
                ));
            }
            None => {
                self.selected_message = None;
                self.detail_load_error = Some("邮件索引未打开".into());
            }
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
                            this.select_mailbox(mailbox_id, true, cx);
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

        if self.load_error.is_some() {
            tree = tree.child(
                div()
                    .mt_4()
                    .px_4()
                    .py_3()
                    .rounded(px(4.))
                    .bg(theme.raised)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("索引不可用，请查看右侧说明"),
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
            .w(relative(0.12))
            .min_w(px(144.))
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
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("compose-button")
                                    .h(px(26.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded(px(4.))
                                    .bg(theme.accent)
                                    .text_color(gpui::hsla(0., 0., 0., 1.))
                                    .text_xs()
                                    .child("写信")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            crate::compose_window::ComposeWindow::open(
                                                crate::compose_window::ComposeKind::New,
                                                cx,
                                            );
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_color(theme.text_tertiary)
                                    .text_xs()
                                    .child(SharedString::from(total_messages.to_string())),
                            ),
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
                let problem = classify_index_error(error.as_ref());
                self.message_load_error = Some(index_problem_message(
                    &self.paths,
                    "message_page_failed",
                    problem,
                    Some(error.as_ref()),
                    "读取邮件列表",
                ));
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

    fn render_message_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme;
        range
            .filter_map(|index| {
                self.message_at(index).map(|message| {
                    let message_id = message.id;
                    let active = self.selected_message == Some(message_id);
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
                        .relative()
                        .h(px(76.))
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .when(active, |style| style.bg(theme.active))
                        .hover(|style| style.bg(theme.raised))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_message(message_id, cx);
                        }))
                        .when(active, |element| {
                            element.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(px(12.))
                                    .h(px(52.))
                                    .w(px(2.))
                                    .bg(theme.accent),
                            )
                        })
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

    fn render_message_list(&mut self, cx: &mut Context<Self>, window: &Window) -> AnyElement {
        let theme = self.theme;
        if !self.search_query.trim().is_empty() {
            return self.render_search_results(cx, window);
        }
        let message_count = self.selected_count();
        let list = uniform_list(
            "message-list",
            message_count,
            cx.processor(|this, range, _window, cx| this.render_message_rows(range, cx)),
        )
        .track_scroll(&self.message_scroll)
        .h_full()
        .flex_1();

        div()
            .h_full()
            .w(relative(0.18))
            .min_w(px(220.))
            .flex_shrink_0()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(self.render_search_bar(cx, window))
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
            .into_any_element()
    }

    fn render_search_results(&mut self, cx: &mut Context<Self>, window: &Window) -> AnyElement {
        let theme = self.theme;
        let query = self.search_query.trim().to_owned();
        let mut rows = div()
            .id("search-results")
            .h_full()
            .flex_1()
            .overflow_y_scroll();
        for (index, hit) in self.search_results.iter().enumerate() {
            let message_id = hit.id;
            let mailbox_id = hit.mailbox_id;
            let selected = index == self.search_selection;
            let subject = nonempty_label(&hit.subject, "（无主题）");
            let sender = nonempty_label(&hit.sender, "未知发件人");
            let snippet = hit.snippet.as_str();
            rows = rows.child(
                div()
                    .id((
                        "search-result",
                        u64::try_from(message_id).unwrap_or_default(),
                    ))
                    .px_4()
                    .py_3()
                    .border_l_2()
                    .border_b_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .when(selected, |style| style.bg(theme.raised))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.raised))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.search_selection = index;
                            this.select_message_from_search(mailbox_id, message_id, cx);
                        }),
                    )
                    .child(highlighted_text(theme, &sender, &query, true))
                    .child(highlighted_text(theme, &subject, &query, false))
                    .child(highlighted_text(theme, snippet, &query, false)),
            );
        }
        if let Some(error) = &self.search_error {
            rows = rows.child(
                div()
                    .mx_4()
                    .mt_4()
                    .p_3()
                    .w_full()
                    .min_w_0()
                    .rounded(px(4.))
                    .bg(theme.raised)
                    .whitespace_normal()
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .line_height(px(18.))
                    .child(error.clone()),
            );
        } else if self.search_results.is_empty() && !self.search_pending {
            rows = rows.child(
                div()
                    .flex_1()
                    .px_4()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_color(theme.text_tertiary)
                    .child(div().text_lg().child("🔍"))
                    .child(
                        div()
                            .mt_1()
                            .text_color(theme.text_secondary)
                            .text_sm()
                            .child("未找到匹配邮件"),
                    )
                    .child(div().text_xs().child("换个关键词再试试")),
            );
        }
        let count = self.search_results.len();
        let status = if self.search_pending {
            "搜索中…".to_owned()
        } else {
            search_status(count, self.search_error.as_deref())
        };
        div()
            .h_full()
            .w(relative(0.18))
            .min_w(px(220.))
            .flex_shrink_0()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(self.render_search_bar(cx, window))
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
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .text_sm()
                            .child(SharedString::from(format!("搜索：{}", query))),
                    )
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child(SharedString::from(status)),
                    ),
            )
            .child(rows)
            .into_any_element()
    }

    fn render_search_bar(&self, cx: &mut Context<Self>, window: &Window) -> impl IntoElement {
        let theme = self.theme;
        let search_input = self.search_input.clone();
        let focus_handle = search_input.read(cx).focus_handle();
        let focused = focus_handle.is_focused(window);
        div()
            .id("search-input")
            .mx_3()
            .mt_3()
            .h(px(36.))
            .px_3()
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(theme.raised)
            .when(focused, |el| el.border_1().border_color(theme.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, window, cx| {
                    window.focus(&focus_handle, cx);
                }),
            )
            .child(div().flex_1().min_w_0().h_full().child(search_input))
            .when(!self.search_query.is_empty(), |element| {
                element.child(
                    div()
                        .id("clear-search")
                        .size(px(24.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .text_color(theme.text_tertiary)
                        .hover(|style| style.bg(theme.border).text_color(theme.text))
                        .child("×")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.clear_search(cx);
                                let focus = this.search_input.read(cx).focus_handle();
                                window.focus(&focus, cx);
                            }),
                        ),
                )
            })
    }

    fn render_reader(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut reader = div()
            .h_full()
            .min_w(px(300.))
            .flex_1()
            .bg(theme.canvas)
            .flex()
            .flex_col();

        if let Some(error) = &self.load_error {
            reader = reader.child(
                div()
                    .id("startup-error")
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_y_scroll()
                    .px_10()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .whitespace_normal()
                            .text_color(theme.text_secondary)
                            .text_sm()
                            .line_height(px(22.))
                            .child(error.clone()),
                    ),
            );
        } else if let Some(detail) = &self.selected_detail {
            let subject = nonempty_label(&detail.subject, "（无主题）");
            let sender = nonempty_label(&detail.sender, "未知发件人");
            let recipients = nonempty_label(&detail.recipients, "未记录收件人");
            let date = nonempty_label(&compact_date(detail.date_text.as_deref()), "未知时间");
            let attachments = if detail.attachment_count == 0 {
                "无附件".to_owned()
            } else {
                format!("{} 个附件", detail.attachment_count)
            };

            reader = reader
                .child(
                    div()
                        .px_6()
                        .py_5()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_ellipsis()
                                .line_clamp(2)
                                .text_color(theme.text)
                                .text_lg()
                                .child(SharedString::from(subject)),
                        )
                        .child(
                            div()
                                .mt_3()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("reply-button")
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.))
                                        .bg(theme.raised)
                                        .text_color(theme.text_secondary)
                                        .text_xs()
                                        .child("回复")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener({
                                                let detail = detail.clone();
                                                move |_this, _, _, cx| {
                                                    crate::compose_window::ComposeWindow::reply(
                                                        &detail, cx,
                                                    );
                                                }
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("forward-button")
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.))
                                        .bg(theme.raised)
                                        .text_color(theme.text_secondary)
                                        .text_xs()
                                        .child("转发")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener({
                                                let detail = detail.clone();
                                                move |_this, _, _, cx| {
                                                    crate::compose_window::ComposeWindow::forward(
                                                        &detail, cx,
                                                    );
                                                }
                                            }),
                                        ),
                                ),
                        )
                        .child(detail_row(theme, "发件人", sender))
                        .child(detail_row(theme, "收件人", recipients))
                        .child(detail_row(theme, "时间", date))
                        .child(detail_row(theme, "附件", attachments))
                        .child(render_attachment_list(
                            theme,
                            &detail.attachments,
                            detail.attachment_count,
                            detail.attachment_load_error.as_deref(),
                        )),
                )
                .child(render_plain_body(
                    theme,
                    preferred_body(&detail.body, &detail.html_body),
                    &self.body_scroll,
                ));
        } else if let Some(error) = &self.detail_load_error {
            reader = reader.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .text_color(theme.text_secondary)
                    .text_sm()
                    .child(error.clone()),
            );
        } else {
            reader = reader.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_tertiary)
                    .text_sm()
                    .child("未选择邮件"),
            );
        }

        reader
    }
}

impl Render for MailApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_hidden()
            .bg(self.theme.canvas)
            .text_color(self.theme.text)
            .flex()
            .child(self.render_sidebar(cx))
            .child(self.render_message_list(cx, window))
            .child(self.render_reader(cx))
    }
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

fn search_status(count: usize, error: Option<&str>) -> String {
    match (error, count) {
        (Some(_), _) => "搜索失败".to_owned(),
        (None, 0) => "未找到匹配邮件".to_owned(),
        (None, count) => format!("{} 条结果", count),
    }
}

fn next_search_selection(current: usize, count: usize, forward: bool) -> usize {
    if count == 0 {
        return 0;
    }
    if forward {
        (current + 1) % count
    } else if current == 0 {
        count - 1
    } else {
        current - 1
    }
}

fn index_problem_message(
    paths: &AppPaths,
    event: &str,
    problem: IndexProblem,
    error: Option<&(dyn std::error::Error + 'static)>,
    operation: &str,
) -> SharedString {
    let message = index_message(problem, operation, &paths.index);
    let message = match log_index_problem(&paths.log, event, problem, error) {
        Ok(()) => with_log_path(message, paths.log_hint),
        Err(_) => with_log_failure(message),
    };
    SharedString::from(message)
}

fn detail_row(theme: Theme, label: &'static str, value: String) -> impl IntoElement {
    div()
        .mt_3()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(48.))
                .flex_shrink_0()
                .text_color(theme.text_tertiary)
                .text_xs()
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_ellipsis()
                .line_clamp(2)
                .text_color(theme.text_secondary)
                .text_sm()
                .child(SharedString::from(value)),
        )
}

fn highlighted_text(theme: Theme, value: &str, query: &str, secondary: bool) -> impl IntoElement {
    let mut element = div()
        .w_full()
        .min_w_0()
        .truncate()
        .flex()
        .text_color(if secondary {
            theme.text_secondary
        } else {
            theme.text
        })
        .text_xs();
    let mut cursor = 0;
    for (start, end) in case_insensitive_match_ranges(value, query) {
        if start > cursor {
            element = element.child(SharedString::from(value[cursor..start].to_owned()));
        }
        element = element.child(
            div()
                .flex_shrink_0()
                .text_color(theme.accent)
                .child(SharedString::from(value[start..end].to_owned())),
        );
        cursor = end;
    }
    if cursor < value.len() {
        element = element.child(SharedString::from(value[cursor..].to_owned()));
    }
    element
}

fn case_insensitive_match_ranges(value: &str, query: &str) -> Vec<(usize, usize)> {
    let query_chars = query
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<Vec<_>>();
    if query_chars.is_empty() {
        return Vec::new();
    }

    let mut lowered = Vec::new();
    for (start, character) in value.char_indices() {
        let end = start + character.len_utf8();
        lowered.extend(
            character
                .to_lowercase()
                .map(|lowered_character| (lowered_character, start, end)),
        );
    }

    let mut ranges = Vec::new();
    let mut index = 0;
    while index + query_chars.len() <= lowered.len() {
        let matches = lowered[index..index + query_chars.len()]
            .iter()
            .zip(&query_chars)
            .all(|((value_character, _, _), query_character)| value_character == query_character);
        if matches {
            let start = lowered[index].1;
            let end = lowered[index + query_chars.len() - 1].2;
            if ranges
                .last()
                .is_none_or(|(_, previous_end)| start >= *previous_end)
            {
                ranges.push((start, end));
            }
            index += query_chars.len();
        } else {
            index += 1;
        }
    }
    ranges
}

fn render_plain_body(theme: Theme, value: String, scroll: &ScrollHandle) -> impl IntoElement {
    let body = normalize_plain_body(&value);
    let empty = body.is_empty();
    let text = if empty {
        "正文为空".to_owned()
    } else {
        body
    };

    div()
        .id("message-body")
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .px_6()
        .py_5()
        .text_color(if empty {
            theme.text_tertiary
        } else {
            theme.text_secondary
        })
        .text_sm()
        .line_height(px(22.))
        .child(SharedString::from(text))
}

fn preferred_body(plain_body: &str, html_body: &str) -> String {
    if !plain_body.trim().is_empty() {
        return normalize_plain_body(plain_body);
    }

    sanitize_html_to_text(html_body)
}

fn sanitize_html_to_text(value: &str) -> String {
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "dd",
        "div",
        "dl",
        "dt",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hr",
        "li",
        "main",
        "nav",
        "ol",
        "p",
        "pre",
        "section",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "tr",
        "ul",
    ];
    const VOID_TAGS: &[&str] = &["br", "hr"];
    const BLOCKED_TAGS: &[&str] = &[
        "applet", "embed", "iframe", "object", "script", "style", "svg", "template",
    ];

    let mut output = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('<') else {
            append_decoded_text(&mut output, &value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        append_decoded_text(&mut output, &value[cursor..start]);

        if value[start..].starts_with("<!--") {
            let Some(relative_end) = value[start + 4..].find("-->") else {
                break;
            };
            cursor = start + 4 + relative_end + 3;
            continue;
        }

        let Some(end) = find_tag_end(value, start + 1) else {
            append_decoded_text(&mut output, &value[start..]);
            break;
        };
        let raw_tag = value[start + 1..end].trim();
        let closing = raw_tag.starts_with('/');
        let tag = raw_tag
            .trim_start_matches('/')
            .trim_start_matches('!')
            .trim_start_matches('?')
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        let tag = tag
            .trim_matches(|character: char| !character.is_ascii_alphanumeric())
            .to_ascii_lowercase();

        if BLOCKED_TAGS.contains(&tag.as_str()) {
            if !closing {
                let closing_marker = format!("</{tag}");
                if let Some(relative_close) =
                    value[end + 1..].to_ascii_lowercase().find(&closing_marker)
                {
                    cursor = end + 1 + relative_close;
                    continue;
                }
                break;
            }
        } else if VOID_TAGS.contains(&tag.as_str()) || BLOCK_TAGS.contains(&tag.as_str()) {
            push_newline(&mut output);
        }

        cursor = end + 1;
    }

    normalize_plain_body(&output)
}

fn find_tag_end(value: &str, mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < value.len() {
        let character = value[cursor..].chars().next()?;
        cursor += character.len_utf8();
        match (quote, character) {
            (Some(expected), current) if expected == current => quote = None,
            (None, '\'') | (None, '"') => quote = Some(character),
            (None, '>') => return Some(cursor - 1),
            _ => {}
        }
    }
    None
}

fn append_decoded_text(output: &mut String, text: &str) {
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative_start) = text[cursor..].find('&') else {
            output.push_str(&text[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&text[cursor..start]);
        let Some(relative_end) = text[start..].find(';') else {
            output.push('&');
            cursor = start + 1;
            continue;
        };
        let end = start + relative_end;
        let entity = &text[start + 1..end];
        if let Some(decoded) = decode_entity(entity) {
            output.push(decoded);
            cursor = end + 1;
        } else {
            output.push('&');
            cursor = start + 1;
        }
    }
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "apos" => Some('\''),
        "gt" => Some('>'),
        "lt" => Some('<'),
        "nbsp" => Some(' '),
        "quot" => Some('"'),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn push_newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn render_attachment_list(
    theme: Theme,
    attachments: &[AttachmentMeta],
    attachment_count: usize,
    load_error: Option<&str>,
) -> impl IntoElement {
    let mut section = div();
    if attachment_count == 0 {
        return section;
    }

    section = section
        .mt_4()
        .pt_3()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .text_color(theme.text_secondary)
                .text_xs()
                .child(SharedString::from(format!("附件（{}）", attachment_count))),
        );

    if attachments.is_empty() {
        return section.child(
            div()
                .mt_2()
                .text_color(theme.text_tertiary)
                .text_xs()
                .child(if load_error.is_some() {
                    "附件信息暂不可用"
                } else {
                    "未解析到附件信息"
                }),
        );
    }

    for attachment in attachments {
        let filename = nonempty_label(&attachment.filename, "未命名附件");
        let marker = if attachment.inline {
            "内嵌"
        } else {
            "附件"
        };
        let metadata = format!(
            "{} · {} · {}",
            marker,
            attachment.content_type,
            format_bytes(attachment.size_bytes)
        );
        section = section.child(
            div()
                .mt_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .text_sm()
                        .child(SharedString::from(filename)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_tertiary)
                        .text_xs()
                        .child(SharedString::from(metadata)),
                ),
        );
    }

    section
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes < KIB {
        format!("{} B", bytes as usize)
    } else if bytes < MIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{:.1} MiB", bytes / MIB)
    }
}

fn normalize_plain_body(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_owned()
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
    use super::{
        case_insensitive_match_ranges, compact_date, format_bytes, mailbox_label,
        next_search_selection, nonempty_label, normalize_plain_body, preferred_body,
        sanitize_html_to_text, search_status,
    };

    #[test]
    fn localizes_standard_mailbox_names_without_overwriting_custom_names() {
        assert_eq!(mailbox_label("INBOX", "INBOX"), "收件箱");
        assert_eq!(mailbox_label("Sent Items", "Sent Items"), "已发送");
        assert_eq!(mailbox_label("&XfJT0ZAB-", "已发送"), "已发送");
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

    #[test]
    fn normalizes_plain_body_line_endings_without_losing_content() {
        assert_eq!(
            normalize_plain_body("第一行\r\n\r\n第二行\r"),
            "第一行\n\n第二行"
        );
        assert_eq!(normalize_plain_body("  \r\n"), "");
    }

    #[test]
    fn formats_attachment_sizes_for_compact_metadata() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn finds_case_insensitive_highlights_on_utf8_boundaries() {
        assert_eq!(
            case_insensitive_match_ranges("Request REQUEST", "request"),
            vec![(0, 7), (8, 15)]
        );
        assert_eq!(case_insensitive_match_ranges("İstanbul", "i"), vec![(0, 2)]);
        assert_eq!(
            case_insensitive_match_ranges("中文主题", "中文"),
            vec![(0, 6)]
        );
    }

    #[test]
    fn reports_search_empty_error_and_result_states() {
        assert_eq!(search_status(0, None), "未找到匹配邮件");
        assert_eq!(search_status(0, Some("索引损坏")), "搜索失败");
        assert_eq!(search_status(2, None), "2 条结果");
    }

    #[test]
    fn wraps_search_selection_in_both_directions() {
        assert_eq!(next_search_selection(0, 3, true), 1);
        assert_eq!(next_search_selection(2, 3, true), 0);
        assert_eq!(next_search_selection(0, 3, false), 2);
        assert_eq!(next_search_selection(2, 3, false), 1);
        assert_eq!(next_search_selection(0, 0, true), 0);
    }

    #[test]
    fn renders_safe_html_as_text_and_preserves_basic_structure() {
        assert_eq!(
            sanitize_html_to_text(
                "<h1>标题</h1><p>第一行<br>第二行 &amp; &#x4E2D;</p><ul><li>项目</li></ul>"
            ),
            "标题\n第一行\n第二行 & 中\n项目"
        );
    }

    #[test]
    fn drops_active_html_content_attributes_and_resources() {
        let rendered = sanitize_html_to_text(
            "<p onclick=alert(1)>正文</p><script>alert(2)</script><style>body{color:red}</style>\
             <iframe src='https://evil.test'>外部内容</iframe><img src='https://evil.test/x' alt='图片'>\
             <a href='javascript:alert(3)'>安全链接文本</a>",
        );
        assert_eq!(rendered, "正文\n安全链接文本");
        assert!(!rendered.contains("alert"));
        assert!(!rendered.contains("evil.test"));
    }

    #[test]
    fn falls_back_to_plain_body_when_html_has_no_displayable_text() {
        assert_eq!(
            preferred_body("纯文本正文", "<p>HTML 正文</p>"),
            "纯文本正文"
        );
        assert_eq!(preferred_body("", "<p>HTML 正文</p>"), "HTML 正文");
        assert_eq!(preferred_body("", "<script>secret()</script>"), "");
        assert_eq!(preferred_body("纯文本正文", ""), "纯文本正文");
    }
}

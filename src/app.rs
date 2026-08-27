use foxmail_imap::account_config::{self, AccountConfig};
use foxmail_imap::backup::{self, BackupOptions};
use foxmail_imap::mail_store::{
    sanitized_attachment_filename, AttachmentMeta, MailStore, MailboxAccount, MessageDetail,
    MessageSummary, SearchHit,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, relative, uniform_list, AnyElement, AppContext, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, ScrollHandle, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
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
    save_state: Option<SaveState>,
    paths: AppPaths,
    onboarding: Option<Onboarding>,
    sync: Option<SyncState>,
}

struct Onboarding {
    host: Entity<ComposerInput>,
    port: Entity<ComposerInput>,
    user: Entity<ComposerInput>,
    password: Entity<ComposerInput>,
    _subscriptions: Vec<gpui::Subscription>,
    error: Option<SharedString>,
}

#[derive(Clone, Debug)]
enum SyncState {
    Connecting,
    Scanning {
        mailboxes: usize,
        done: usize,
    },
    Mailbox {
        name: String,
        index: usize,
        total: usize,
        backed_up: usize,
        skipped: usize,
    },
    Message {
        mailbox: String,
        mailbox_index: usize,
        mailbox_total: usize,
        uid: u32,
        done: u64,
        total: u64,
    },
    Done {
        backed_up: usize,
        skipped: usize,
    },
    Failed(SharedString),
}

enum SaveState {
    Saving,
    Done(SharedString),
    Failed(SharedString),
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

        let index_missing = index_is_missing(&paths.index);
        let (store, accounts, load_error) = if index_missing {
            (None, Vec::new(), None)
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

        // 索引缺失 → 进入 onboarding；账号配置不存在时预填默认值（imap.yunyou.top / 993 / 当前 user）。
        let onboarding = if index_missing {
            let defaults = account_config::load()
                .ok()
                .flatten()
                .unwrap_or_else(AccountConfig::defaults);
            Some(Self::build_onboarding(window, cx, defaults))
        } else {
            None
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
            save_state: None,
            paths,
            onboarding,
            sync: None,
        }
    }

    fn build_onboarding(
        window: &mut Window,
        cx: &mut Context<Self>,
        defaults: AccountConfig,
    ) -> Onboarding {
        let host = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                defaults.host,
                ComposerInputOptions::single_line("IMAP 服务器"),
            )
        });
        let port = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                defaults.port.to_string(),
                ComposerInputOptions::single_line("端口"),
            )
        });
        let user = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                defaults.user,
                ComposerInputOptions::single_line("邮箱地址"),
            )
        });
        let password = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                String::new(),
                ComposerInputOptions::single_line("密码 / 授权码").masked(),
            )
        });
        let fields = [&host, &port, &user, &password];
        let subscriptions = fields
            .iter()
            .map(|field| {
                cx.subscribe(
                    field,
                    |this: &mut MailApp, _input, _event: &ComposerEvent, cx| {
                        if this.onboarding.as_ref().is_some_and(|o| o.error.is_some()) {
                            this.update_onboarding_error(None, cx);
                        }
                    },
                )
            })
            .collect();
        Onboarding {
            host,
            port,
            user,
            password,
            _subscriptions: subscriptions,
            error: None,
        }
    }

    fn update_onboarding_error(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.error = error;
        }
        cx.notify();
    }

    /// 读取 onboarding 表单，校验并落盘账号配置，返回 ready 的 AccountConfig。
    fn collect_onboarding_account(
        &self,
        cx: &mut Context<Self>,
    ) -> Result<AccountConfig, SharedString> {
        let onboarding = self
            .onboarding
            .as_ref()
            .ok_or_else(|| SharedString::from("账号配置界面不可用"))?;
        let host = onboarding.host.read(cx).content().trim().to_owned();
        let user = onboarding.user.read(cx).content().trim().to_owned();
        let port_text = onboarding.port.read(cx).content().trim().to_owned();
        let password = onboarding.password.read(cx).content().to_owned();

        if host.is_empty() {
            return Err(SharedString::from("请填写 IMAP 服务器地址"));
        }
        if user.is_empty() {
            return Err(SharedString::from("请填写邮箱地址"));
        }
        if password.is_empty() {
            return Err(SharedString::from("请填写密码或授权码"));
        }
        let port: u16 = port_text
            .parse()
            .map_err(|_| SharedString::from("端口必须是数字，IMAP 通常为 993"))?;
        Ok(AccountConfig {
            host,
            port,
            user,
            password,
        })
    }

    fn start_sync(&mut self, cx: &mut Context<Self>) {
        let account = match self.collect_onboarding_account(cx) {
            Ok(account) => account,
            Err(error) => {
                self.update_onboarding_error(Some(error), cx);
                return;
            }
        };
        let backup_root = self.backup_root_for(&account);
        if let Err(error) = account_config::save(&account) {
            self.update_onboarding_error(
                Some(SharedString::from(format!("保存账号配置失败：{error}"))),
                cx,
            );
            return;
        }

        let host = account.host.clone();
        let port = account.port;
        let user = account.user.clone();
        let password = account.password.clone();
        // 记住预期备份根目录，finish_sync 直接复用，避免重新读 DPAPI + safe_component
        // 算的一致性（旧 last_backup_root 路径会报"未找到备份目录"）。
        // 记住预期备份根目录，finish_sync 直接复用，避免 last_backup_root 依赖
        // 重新读取 DPAPI + safe_component 算的一致性。须在线程 move user 之前算。
        let expected_backup_root = backup_root.join(backup::safe_component(&user));
        self.sync = Some(SyncState::Connecting);
        self.onboarding = None;
        cx.notify();

        let (progress_tx, progress_rx) = std::sync::mpsc::channel::<backup::ProgressEvent>();

        // 后台线程跑 IMAP 拉取；错误在线程内转 String 并发一个 Failed 事件后退出。
        // Box<dyn Error> 不跨线程，规避 Send 缺失；sender drop 让 pump 收尾。
        let options = BackupOptions {
            output: backup_root.clone(),
            mailbox: None,
            max_messages: None,
            chunk_size: backup::DEFAULT_CHUNK_SIZE,
            dry_run: false,
        };
        let progress_tx_clone = progress_tx.clone();
        std::thread::spawn(move || {
            let mut progress = backup::ChannelProgress::new(progress_tx_clone);
            if let Err(error) =
                backup::run_backup_with(&options, &host, port, &user, &password, &mut progress)
            {
                progress.send_failed(error.to_string());
            }
        });
        drop(progress_tx);

        cx.spawn(async move |this, cx| {
            // ponytail: 只在收到 Finished/Failed 终端事件时退出，绝不因"连续空轮"提前 break。
            // 之前两轮空就退出 → 大附件慢下载间隙触发误退出，sync 停在中间态 →
            // finish_sync 报"未找到备份目录"。空轮只意味着"无新数据，继续等"。
            // 线程结束前必发 Finished（成功）或 send_failed（失败），故终会收到。
            let mut batch: Vec<backup::ProgressEvent> = Vec::new();
            let mut finished = false;
            while !finished {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                while let Ok(event) = progress_rx.try_recv() {
                    if matches!(
                        event,
                        backup::ProgressEvent::Finished { .. }
                            | backup::ProgressEvent::Failed { .. }
                    ) {
                        finished = true;
                    }
                    batch.push(event);
                }
                if !batch.is_empty() {
                    this.update(cx, |this, cx| {
                        for event in batch.drain(..) {
                            this.apply_progress(event);
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
            this.update(cx, |this, cx| {
                this.finish_sync_with(expected_backup_root, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply_progress(&mut self, event: backup::ProgressEvent) {
        use backup::ProgressEvent::*;
        let next = match event {
            Connecting { .. } => SyncState::Connecting,
            LoggedIn { .. } => self.sync.clone().unwrap_or(SyncState::Connecting),
            MailboxesPlanned { count } => SyncState::Scanning {
                mailboxes: count,
                done: 0,
            },
            MailboxStart {
                name,
                uids: _,
                exists: _,
            } => {
                let (index, total) = match &self.sync {
                    Some(SyncState::Scanning { done, mailboxes }) => (*done + 1, *mailboxes),
                    Some(SyncState::Mailbox { index, total, .. }) => (*index + 1, *total),
                    _ => (1, 0),
                };
                SyncState::Mailbox {
                    name,
                    index,
                    total,
                    backed_up: 0,
                    skipped: 0,
                }
            }
            MessageSkipped { .. } => {
                if let Some(SyncState::Mailbox { skipped, .. }) = self.sync.as_mut() {
                    *skipped += 1;
                }
                return;
            }
            MessageStart { uid, total, .. } => {
                let (mailbox, mailbox_index, mailbox_total) = match &self.sync {
                    Some(SyncState::Mailbox {
                        name, index, total, ..
                    }) => (name.clone(), *index, *total),
                    _ => (String::new(), 0, 0),
                };
                if let Some(SyncState::Mailbox { backed_up, .. }) = self.sync.as_mut() {
                    *backed_up += 1;
                }
                SyncState::Message {
                    mailbox,
                    mailbox_index,
                    mailbox_total,
                    uid,
                    done: 0,
                    total,
                }
            }
            MessageProgress { done, total, .. } => {
                if let Some(SyncState::Message {
                    done: d, total: t, ..
                }) = self.sync.as_mut()
                {
                    *d = done;
                    *t = total;
                }
                return;
            }
            MailboxDone {
                backed_up, skipped, ..
            } => {
                // 回到 Mailbox 视图并落定本文件夹最终计数。
                let (name, index, total) = match self.sync.clone() {
                    Some(SyncState::Mailbox {
                        name, index, total, ..
                    }) => (name, index, total),
                    Some(SyncState::Message {
                        mailbox,
                        mailbox_index,
                        mailbox_total,
                        ..
                    }) => (mailbox, mailbox_index, mailbox_total),
                    other => match other {
                        Some(SyncState::Connecting) | None => (String::new(), 0, 0),
                        Some(_) => (String::new(), 0, 0),
                    },
                };
                SyncState::Mailbox {
                    name,
                    index,
                    total,
                    backed_up,
                    skipped,
                }
            }
            Finished { backed_up, skipped } => SyncState::Done { backed_up, skipped },
            Failed { message } => SyncState::Failed(SharedString::from(message)),
        };
        self.sync = Some(next);
    }

    fn finish_sync_with(&mut self, backup_root: PathBuf, cx: &mut Context<Self>) {
        if matches!(self.sync.as_ref(), Some(SyncState::Failed(_))) {
            cx.notify();
            return;
        }
        let index_path = self.paths.index.clone();
        self.sync = Some(SyncState::Done {
            backed_up: 0,
            skipped: 0,
        });
        cx.notify();

        cx.background_executor()
            .spawn(async move {
                let result = std::thread::spawn(move || -> Result<(), String> {
                    let mut store = MailStore::open(&index_path).map_err(|e| e.to_string())?;
                    store
                        .import_backup(&backup_root)
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .join();
                match result {
                    Ok(Ok(())) => Ok::<(), String>(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("入库线程异常退出".to_string()),
                }
            })
            .detach();

        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| this.reload_index(cx)).ok();
        })
        .detach();
    }

    fn reload_index(&mut self, cx: &mut Context<Self>) {
        match MailStore::open_read_only(&self.paths.index).and_then(|store| {
            let accounts = store.mailbox_tree()?;
            Ok((store, accounts))
        }) {
            Ok((store, accounts)) => {
                self.store = Some(store);
                self.accounts = accounts;
                self.selected_mailbox = self
                    .accounts
                    .iter()
                    .flat_map(|a| a.mailboxes.iter())
                    .next()
                    .map(|m| m.id);
                self.load_error = None;
                self.sync = None;
                self.message_pages.clear();
                self.selected_message = None;
                self.selected_detail = None;
            }
            Err(error) => {
                let problem = classify_index_error(error.as_ref());
                self.load_error = Some(index_problem_message(
                    &self.paths,
                    "index_open_after_sync",
                    problem,
                    Some(error.as_ref()),
                    "拉取后打开索引",
                ));
                self.sync = Some(SyncState::Failed(SharedString::from(
                    "入库后打开索引失败，请查看说明",
                )));
            }
        }
        cx.notify();
    }

    fn backup_root_for(&self, _account: &AccountConfig) -> PathBuf {
        // 便携优先：EXE 同目录 data；否则用户目录 data。
        // 索引路径的父目录即 data 根（见 paths::resolve_paths）；账号无关。
        self.paths
            .index
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("data"))
    }

    fn render_onboarding_or_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        if let Some(sync) = &self.sync {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(self.render_sync_panel(theme, sync.clone(), window, cx));
        }
        let Some(onboarding) = self.onboarding.as_ref() else {
            return div().size_full();
        };
        let host_field = onboarding.host.clone();
        let port_field = onboarding.port.clone();
        let user_field = onboarding.user.clone();
        let password_field = onboarding.password.clone();
        let error = onboarding.error.clone();
        let host_label = onboarding.host.read(cx).content().to_owned();
        let _ = (window, host_label);

        let mut card = div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap_3()
            .px_8()
            .py_8()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .child(div().text_color(theme.text).text_lg().child("设置邮箱账号"))
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("填写 IMAP 服务器与凭据，首次拉取会把邮件备份到本地并建立索引。"),
            )
            .child(onboarding_field(theme, "IMAP 服务器", &host_field, cx))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .child(onboarding_field(theme, "端口", &port_field, cx)),
                    ),
            )
            .child(onboarding_field(theme, "邮箱地址", &user_field, cx))
            .child(onboarding_field(
                theme,
                "密码 / 授权码",
                &password_field,
                cx,
            ));

        if let Some(error) = error {
            card = card.child(div().text_color(theme.accent).text_xs().child(error));
        }

        card = card.child(
            div()
                .id("start-sync-button")
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(theme.accent)
                .text_color(gpui::hsla(0., 0., 0., 1.))
                .text_sm()
                .child("拉取邮件")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _, _, cx| {
                        _this.start_sync(cx);
                    }),
                ),
        );

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(card)
    }

    fn render_sync_panel(
        &mut self,
        theme: Theme,
        sync: SyncState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (title, detail, is_failed) = match sync {
            SyncState::Connecting => ("正在连接 IMAP 服务器…".to_owned(), None, false),
            SyncState::Scanning { mailboxes, done } => (
                format!("发现邮箱文件夹…（{done}/{mailboxes}）"),
                None,
                false,
            ),
            SyncState::Mailbox {
                name,
                index,
                total,
                backed_up,
                skipped,
            } => (
                format!("正在拉取 {name}（{index}/{total}）"),
                Some(format!("已下载 {backed_up} 封，跳过 {skipped} 封")),
                false,
            ),
            SyncState::Message {
                mailbox,
                mailbox_index,
                mailbox_total,
                uid,
                done,
                total,
            } => {
                let pct = if total == 0 {
                    100.0
                } else {
                    done as f64 * 100.0 / total as f64
                };
                (
                    format!("{mailbox}（{mailbox_index}/{mailbox_total}）UID {uid}"),
                    Some(format!(
                        "{:.1}%（{} / {}）",
                        pct,
                        format_bytes(done),
                        format_bytes(total)
                    )),
                    false,
                )
            }
            SyncState::Done { backed_up, skipped } => (
                "拉取完成，正在建立索引…".to_owned(),
                Some(format!("新增 {backed_up} 封，跳过 {skipped} 封")),
                false,
            ),
            SyncState::Failed(message) => ("拉取失败".to_owned(), Some(message.to_string()), true),
        };
        let mut panel = div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap_2()
            .px_8()
            .py_8()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .child(div().text_color(theme.text).text_lg().child(title))
            .when_some(detail, |panel, detail| {
                panel.child(
                    div()
                        .text_color(theme.text_secondary)
                        .text_xs()
                        .child(detail),
                )
            });

        // 失败时给"重新输入账号"按钮：回到 onboarding，保留 host/port/user，清空密码。
        if is_failed {
            panel = panel.child(
                div()
                    .id("retry-onboarding-button")
                    .mt_2()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(theme.raised)
                    .text_color(theme.text)
                    .text_sm()
                    .child("重新输入账号")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, window, cx| {
                            _this.return_to_onboarding(window, cx);
                        }),
                    ),
            );
        }

        panel
    }

    fn return_to_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = account_config::load()
            .ok()
            .flatten()
            .unwrap_or_else(AccountConfig::defaults);
        let onboarding = Self::build_onboarding(window, cx, defaults);
        // build_onboarding 已把 password 初始化为空；host/port/user 从保存的配置预填。
        self.onboarding = Some(onboarding);
        self.sync = None;
        cx.notify();
    }

    fn start_attachment_save(
        &mut self,
        message_id: i64,
        attachment_index: usize,
        filename: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .save_state
            .as_ref()
            .is_some_and(|state| matches!(state, SaveState::Saving))
        {
            return;
        }
        let Some(_store) = self.store.as_ref() else {
            self.save_state = Some(SaveState::Failed("邮件索引未打开".into()));
            cx.notify();
            return;
        };
        let Some(target) = crate::save_dialog::prompt_save_path(window, &filename) else {
            return; // user cancelled
        };
        self.save_state = Some(SaveState::Saving);
        cx.notify();
        let display_name: SharedString = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_string_lossy().into_owned())
            .into();
        let task = cx.background_executor().spawn({
            // ponytail: re-open a read-only handle on a background thread; rusqlite Connection
            // isn't Clone+Send, so we reopen by path instead of cloning the live connection.
            // Errors are stringified so the task stays Send (Box<dyn Error> is not Send).
            let index_path = self.paths.index.clone();
            async move {
                let store =
                    MailStore::open_read_only(&index_path).map_err(|error| error.to_string())?;
                store
                    .save_attachment(message_id, attachment_index, &target)
                    .map_err(|error| error.to_string())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.save_state = Some(match result {
                    Ok(()) => SaveState::Done(display_name),
                    Err(error) => {
                        let message = index_problem_message(
                            &this.paths,
                            "attachment_save_failed",
                            IndexProblem::QueryFailed,
                            None,
                            "保存附件",
                        );
                        let _ = log_index_problem(
                            &this.paths.log,
                            "attachment_save_failed",
                            IndexProblem::QueryFailed,
                            None,
                        );
                        SaveState::Failed(SharedString::from(format!("{message}：{error}")))
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
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
            self.save_state = None;
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
        self.save_state = None;
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

    fn render_reader(&self, cx: &mut Context<Self>, window: &Window) -> impl IntoElement {
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
                        .child(self.render_attachment_list(
                            &detail.attachments,
                            detail.attachment_count,
                            detail.attachment_load_error.as_deref(),
                            window,
                            cx,
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
        if self.onboarding.is_some() || self.sync.is_some() {
            return div()
                .size_full()
                .overflow_hidden()
                .bg(self.theme.canvas)
                .text_color(self.theme.text)
                .child(self.render_onboarding_or_sync(window, cx));
        }
        div()
            .size_full()
            .overflow_hidden()
            .bg(self.theme.canvas)
            .text_color(self.theme.text)
            .flex()
            .child(self.render_sidebar(cx))
            .child(self.render_message_list(cx, window))
            .child(self.render_reader(cx, window))
    }
}

fn field_label(theme: Theme, text: &'static str) -> impl IntoElement {
    div().text_color(theme.text_tertiary).text_xs().child(text)
}

fn onboarding_field(
    theme: Theme,
    label: &'static str,
    field: &Entity<ComposerInput>,
    _cx: &mut Context<MailApp>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(field_label(theme, label))
        .child(
            div()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.))
                .px_2()
                .py_1()
                .child(field.clone()),
        )
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

impl MailApp {
    fn render_attachment_list(
        &self,
        attachments: &[AttachmentMeta],
        attachment_count: usize,
        load_error: Option<&str>,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
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

        if let Some(state) = &self.save_state {
            let text: SharedString = match state {
                SaveState::Saving => "正在保存…".into(),
                SaveState::Done(name) => format!("已保存：{name}").into(),
                SaveState::Failed(error) => format!("保存失败：{error}").into(),
            };
            section = section.child(
                div()
                    .mt_2()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(match state {
                        SaveState::Failed(_) => theme.accent,
                        _ => theme.text_tertiary,
                    })
                    .child(text),
            );
        }

        for (index, attachment) in attachments.iter().enumerate() {
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
                format_attachment_bytes(attachment.size_bytes)
            );
            let message_id = self.selected_message.unwrap_or_default();
            let save_target = sanitized_attachment_filename(&attachment.filename);
            section = section.child(
                div()
                    .mt_2()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
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
                    )
                    .child(
                        div()
                            .id(("attachment-save", index))
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(theme.raised)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.active))
                            .text_color(theme.text_secondary)
                            .text_xs()
                            .child("保存")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_attachment_save(
                                    message_id,
                                    index,
                                    save_target.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        }

        section
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{} B", bytes)
    } else if value < MIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{:.1} MiB", value / MIB)
    }
}

fn format_attachment_bytes(bytes: usize) -> String {
    format_bytes(bytes as u64)
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

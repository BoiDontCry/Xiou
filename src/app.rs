use foxmail_imap::account_config::{self, AccountConfig};
use foxmail_imap::backup::{self, BackupOptions};
use foxmail_imap::mail_store::{
    attachment_preview_kind, resolve_image_format, sanitized_attachment_filename, AttachmentMeta,
    AttachmentPayload, AttachmentPreviewKind, MailStore, MailboxAccount, MailboxSummary,
    MessageDetail, MessageSummary, SearchHit, SniffedImageFormat, StoreResult,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, img, list, px, relative, rgb, uniform_list, AnyElement, AppContext, Context,
    CursorStyle, DragMoveEvent, Empty, Entity, FocusHandle, Image, ImageFormat,
    InteractiveElement, IntoElement, ListAlignment, ListState, MouseButton, ObjectFit,
    ParentElement, Render, ScrollHandle, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, UniformListScrollHandle, Window,
};

use crate::ui::{control_button, rems, step_index, tooltip, ButtonKind};

actions!(
    mail,
    [
        Compose,
        Reply,
        Forward,
        OpenSearch,
        ClosePreview,
        DismissOverlay,
        NextMessage,
        PrevMessage,
        NextMailbox,
        PrevMailbox,
        ConfirmSelection,
        ToggleSidebar,
        SyncNow,
        OpenSettings,
    ]
);
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::composer_editor::{ComposerEvent, ComposerInput, ComposerInputOptions};
use crate::diagnostics::{
    classify_index_error, index_message, log_index_problem, with_log_failure, with_log_path,
    IndexProblem,
};
use crate::paths::{index_is_missing, AppPaths};
use crate::prefs::{
    self, format_hex, hsv_to_rgb, parse_hex, rgb_to_hsv, ColorScheme, Palette, Prefs, TokenId,
    TokenSet, WorkspaceMode,
};
use crate::theme::Theme;
use foxmail_imap::html::{self, BodyBlock, InlineSpan};

const MESSAGE_PAGE_SIZE: usize = 50;
const ATTACHMENT_PANE_DEFAULT_REM: f32 = 6.0;
const ATTACHMENT_PANE_MIN_REM: f32 = 4.0;
const ATTACHMENT_PANE_MAX_REM: f32 = 28.0;
const SEARCH_DROPDOWN_MAX_REM: f32 = 12.0;
const COMPACT_WIDTH_REM: f32 = 48.0;

pub struct MailApp {
    theme: Theme,
    prefs: Prefs,
    settings: Option<SettingsSession>,
    workspace_pane: WorkspacePane,
    store: Option<MailStore>,
    accounts: Vec<MailboxAccount>,
    selected_mailbox: Option<i64>,
    collapsed_accounts: HashSet<String>,
    sidebar_collapsed: bool,
    load_error: Option<SharedString>,
    message_pages: HashMap<usize, Vec<MessageSummary>>,
    message_load_error: Option<SharedString>,
    message_scroll: UniformListScrollHandle,
    body_scroll: ScrollHandle,
    attachment_scroll: ScrollHandle,
    attachment_pane_rem: f32,
    split_drag_y: Option<f32>,
    selected_message: Option<i64>,
    selected_detail: Option<MessageDetail>,
    selected_body_cids: Arc<HashMap<String, Arc<Image>>>,
    selected_body_blocks: Arc<Vec<BodyBlock>>,
    body_list: ListState,
    detail_generation: usize,
    detail_load_error: Option<SharedString>,
    search_query: String,
    search_results: Vec<SearchHit>,
    search_error: Option<SharedString>,
    search_pending: bool,
    search_selection: usize,
    search_generation: usize,
    search_input: Entity<ComposerInput>,
    _search_subscription: gpui::Subscription,
    search_open: bool,
    save_state: Option<SaveState>,
    paths: AppPaths,
    onboarding: Option<Onboarding>,
    sync: Option<SyncState>,
    preview: Option<PreviewState>,
    preview_generation: usize,
    preview_focus: FocusHandle,
    preview_return_focus: Option<FocusHandle>,
    sidebar_focus: FocusHandle,
    list_focus: FocusHandle,
    selected_index: Option<usize>,
}

struct SettingsSession {
    draft: Prefs,
    hex_input: Entity<ComposerInput>,
    _hex_subscription: gpui::Subscription,
    token: TokenId,
    editing_dark: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspacePane {
    Sidebar,
    List,
    Reader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chrome {
    Default,
    Minimal,
    Compact,
}

fn chrome_for(width_px: f32, rem: f32, workspace: WorkspaceMode) -> Chrome {
    if width_px < rem.max(1.0) * COMPACT_WIDTH_REM {
        Chrome::Compact
    } else if workspace == WorkspaceMode::Minimal {
        Chrome::Minimal
    } else {
        Chrome::Default
    }
}

fn chrome(window: &Window, workspace: WorkspaceMode) -> Chrome {
    chrome_for(
        f32::from(window.viewport_size().width),
        f32::from(window.rem_size()),
        workspace,
    )
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

enum PreviewState {
    Loading {
        filename: SharedString,
    },
    Image {
        filename: SharedString,
        image: Arc<Image>,
    },
    Pdf {
        filename: SharedString,
    },
    Failed {
        filename: SharedString,
        error: SharedString,
    },
}

const MAX_IMAGE_PREVIEW_BYTES: usize = 20 * 1024 * 1024;

// ponytail: 固定 10 分钟轮询，不做可配置项；嫌慢或嫌费电时再加设置。
const BACKGROUND_SYNC_INTERVAL_SECS: u64 = 600;

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

        let selected_mailbox = first_sidebar_mailbox_id(&accounts);

        let mut app = Self {
            theme: Theme::from_env(),
            prefs: prefs::cached(),
            settings: None,
            workspace_pane: WorkspacePane::List,
            store,
            accounts,
            selected_mailbox,
            collapsed_accounts: HashSet::new(),
            sidebar_collapsed: false,
            load_error,
            message_pages: HashMap::new(),
            message_load_error: None,
            message_scroll: UniformListScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            attachment_scroll: ScrollHandle::new(),
            attachment_pane_rem: ATTACHMENT_PANE_DEFAULT_REM,
            split_drag_y: None,
            selected_message: None,
            selected_detail: None,
            selected_body_cids: Arc::new(HashMap::new()),
            selected_body_blocks: Arc::new(Vec::new()),
            body_list: ListState::new(0, ListAlignment::Top, px(1000.)),
            detail_generation: 0,
            detail_load_error: None,
            search_query: String::new(),
            search_results: Vec::new(),
            search_error: None,
            search_pending: false,
            search_selection: 0,
            search_generation: 0,
            search_input,
            _search_subscription: search_subscription,
            search_open: false,
            save_state: None,
            paths,
            onboarding,
            sync: None,
            preview: None,
            preview_generation: 0,
            preview_focus: cx.focus_handle().tab_stop(true),
            preview_return_focus: None,
            sidebar_focus: cx.focus_handle().tab_stop(true),
            list_focus: cx.focus_handle().tab_stop(true),
            selected_index: None,
        };
        if app.onboarding.is_none() {
            // 启动即拉一次新邮件，之后交给固定间隔轮询。
            app.maybe_start_background_sync(cx);
            app.schedule_background_sync(cx);
        }

        // 关窗进托盘：开启时吞掉 WM_CLOSE，把窗口藏起来；退出走托盘菜单 / 任务管理器。
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, app| {
            let Some(entity) = this.upgrade() else {
                return true;
            };
            let mut hide = false;
            let _ = entity.update(app, |app, _| hide = app.prefs.close_to_tray);
            if hide {
                crate::tray::hide_window(window);
                false
            } else {
                true
            }
        });
        app
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
        if let Err(error) = account_config::save(&account) {
            self.update_onboarding_error(
                Some(SharedString::from(format!("保存账号配置失败：{error}"))),
                cx,
            );
            return;
        }
        self.onboarding = None;
        self.run_sync_pipeline(account, cx);
    }

    /// 后台轮询入口：已有配置且当前空闲时自动拉新邮件。
    fn maybe_start_background_sync(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.is_some() {
            return;
        }
        // 同步进行中（含入库/重载段）不重叠开新一轮；Done/Failed 视为空闲。
        if self
            .sync
            .as_ref()
            .is_some_and(|s| !matches!(s, SyncState::Done { .. } | SyncState::Failed(_)))
        {
            return;
        }
        let Ok(account) = account_config::load() else {
            return; // 配置缺失或读取失败，静默跳过，下一轮再试
        };
        let Some(account) = account else {
            return;
        };
        self.run_sync_pipeline(account, cx);
    }

    /// 启动后立即同步一次，之后按固定间隔轮询。
    fn schedule_background_sync(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(BACKGROUND_SYNC_INTERVAL_SECS))
                .await;
            this.update(cx, |this, cx| this.maybe_start_background_sync(cx))
                .ok();
        })
        .detach();
    }

    /// 手动（onboarding）与后台轮询共用的同步执行段：IMAP 拉取（后台线程）→ 事件泵 → 入库。
    fn run_sync_pipeline(&mut self, account: AccountConfig, cx: &mut Context<Self>) {
        let backup_root = self.backup_root_for(&account);
        let host = account.host.clone();
        let port = account.port;
        let user = account.user.clone();
        let password = account.password.clone();
        // 入库根目录必须是 data 根，不能是 <data>\<账号> 子目录：`import_backup` 按这个根算
        // 相对 eml_path，而读取端 `MailStore::source_root` 用的是索引文件所在目录（= data 根）。
        // 传子目录会让存库路径少一层账号目录，导致附件读取按错误路径解析。
        // `import_backup` 会递归找 mailbox.meta.json，传 data 根同样能发现全部邮箱。
        let import_root = backup_root.clone();
        self.sync = Some(SyncState::Connecting);
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
                this.finish_sync_with(import_root, cx);
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
        // 保留 Finished 事件带来的真实计数，别用 0/0 覆盖掉。
        if !matches!(self.sync, Some(SyncState::Done { .. })) {
            self.sync = Some(SyncState::Done {
                backed_up: 0,
                skipped: 0,
            });
        }
        // 后台轮询没拉到新邮件时索引未变，跳过 reload——reload 会清空选中邮件/分页缓存，
        // 会把用户正在读的邮件踢回列表首封。store 尚未加载（首次 onboarding 同步）除外。
        let backed_up = match &self.sync {
            Some(SyncState::Done { backed_up, .. }) => *backed_up,
            _ => 0,
        };
        if backed_up == 0 && self.store.is_some() {
            self.sync = None;
            cx.notify();
            return;
        }
        cx.notify();

        let task = cx.background_executor().spawn(async move {
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
        });

        // ponytail: 必须等入库结束再 reload。之前 detach 入库后立刻 reload，
        // 索引文件尚未建出 → 只读打开报 SQLITE_CANTOPEN(14)，明明成功却显示失败。
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.reload_index(cx),
                Err(error) => {
                    this.sync = Some(SyncState::Failed(SharedString::from(format!(
                        "入库失败：{error}"
                    ))));
                    cx.notify();
                }
            })
            .ok();
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
                self.selected_mailbox = first_sidebar_mailbox_id(&self.accounts);
                self.load_error = None;
                self.sync = None;
                self.message_pages.clear();
                self.selected_message = None;
                self.selected_detail = None;
                self.preview = None;
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
            .w(rems(26.25))
            .flex()
            .flex_col()
            .gap_3()
            .px_8()
            .py_8()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(rems(0.5))
            .child(div().text_color(theme.text).text_lg().child("设置邮箱账号"))
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("填写 IMAP 服务器与凭据，首次拉取会把邮件备份到本地并建立索引。"),
            )
            .child(onboarding_field(theme, "IMAP 服务器", &host_field, cx))
            .child(onboarding_field(theme, "端口", &port_field, cx))
            .child(onboarding_field(theme, "邮箱地址", &user_field, cx))
            .child(onboarding_field(
                theme,
                "密码 / 授权码",
                &password_field,
                cx,
            ));

        if let Some(error) = error {
            card = card.child(div().text_color(theme.danger).text_xs().child(error));
        }

        card = card.child(
            control_button("start-sync-button", theme, ButtonKind::Primary, false)
                .h(rems(2.25))
                .text_sm()
                .child("拉取邮件")
                .on_click(cx.listener(|this, _, _, cx| this.start_sync(cx))),
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
            SyncState::Failed(message) => ("同步失败".to_owned(), Some(message.to_string()), true),
        };
        let mut panel = div()
            .w(rems(26.25))
            .flex()
            .flex_col()
            .gap_2()
            .px_8()
            .py_8()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(rems(0.5))
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
                control_button(
                    "retry-onboarding-button",
                    theme,
                    ButtonKind::Secondary,
                    false,
                )
                .mt_2()
                .h(rems(2.))
                .text_sm()
                .child("重新输入账号")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.return_to_onboarding(window, cx);
                })),
            );
        }

        panel
    }

    fn render_sync_banner(&self, theme: Theme, sync: SyncState) -> impl IntoElement {
        let (title, is_failed) = match sync {
            SyncState::Connecting => ("正在检查新邮件…".to_owned(), false),
            SyncState::Scanning { mailboxes, done } => {
                (format!("正在检查文件夹（{done}/{mailboxes}）"), false)
            }
            SyncState::Mailbox {
                name,
                index,
                total,
                backed_up,
                skipped,
            } => (
                format!("检查 {name}（{index}/{total}）新增 {backed_up}，本地已有 {skipped}"),
                false,
            ),
            SyncState::Message { mailbox, .. } => (format!("正在下载 {mailbox} 的新邮件…"), false),
            SyncState::Done { backed_up, skipped } => (
                format!("同步完成：新增 {backed_up} 封，本地已有 {skipped} 封"),
                false,
            ),
            SyncState::Failed(message) => (format!("同步失败：{message}"), true),
        };
        div()
            .w_full()
            .flex_shrink_0()
            .px_4()
            .py_2()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(if is_failed { theme.danger } else { theme.info })
                    .child(title),
            )
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
                let bytes = load_attachment_bytes(&index_path, message_id, attachment_index)?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&target, bytes).map_err(|error| error.to_string())
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

    fn start_attachment_preview(
        &mut self,
        message_id: i64,
        attachment_index: usize,
        filename: String,
        content_type: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.preview, Some(PreviewState::Loading { .. })) {
            return;
        }
        self.preview_return_focus = window.focused(cx);
        window.focus(&self.preview_focus, cx);
        let display_name = nonempty_label(&filename, "未命名附件");
        match attachment_preview_kind(&content_type, &filename) {
            AttachmentPreviewKind::Pdf => {
                self.preview = Some(PreviewState::Pdf {
                    filename: SharedString::from(display_name),
                });
                cx.notify();
                return;
            }
            AttachmentPreviewKind::Unsupported => {
                self.preview = Some(PreviewState::Failed {
                    filename: SharedString::from(display_name),
                    error: "该类型暂不支持预览，请先保存到本地打开".into(),
                });
                cx.notify();
                return;
            }
            AttachmentPreviewKind::Image => {}
        }
        if self.store.is_none() {
            self.preview = Some(PreviewState::Failed {
                filename: SharedString::from(display_name),
                error: "邮件索引未打开".into(),
            });
            cx.notify();
            return;
        }

        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        self.preview = Some(PreviewState::Loading {
            filename: SharedString::from(display_name.clone()),
        });
        cx.notify();

        let index_path = self.paths.index.clone();
        let task = cx.background_executor().spawn(async move {
            let bytes = load_attachment_bytes(&index_path, message_id, attachment_index)?;
            if bytes.len() > MAX_IMAGE_PREVIEW_BYTES {
                return Err(format!(
                    "附件过大（{}），超过 20 MiB 预览上限",
                    format_bytes(bytes.len() as u64)
                ));
            }
            let format = resolve_image_format(&content_type, &filename, Some(&bytes))
                .ok_or_else(|| "无法识别图片格式".to_owned())?;
            Ok(Arc::new(Image::from_bytes(
                gpui_image_format(format),
                bytes,
            )))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.preview_generation != generation {
                    return;
                }
                this.preview = Some(match result {
                    Ok(image) => PreviewState::Image {
                        filename: SharedString::from(display_name),
                        image,
                    },
                    Err(error) => PreviewState::Failed {
                        filename: SharedString::from(display_name),
                        error: SharedString::from(error),
                    },
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = None;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        if let Some(handle) = self.preview_return_focus.take() {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn compose(&mut self, _: &Compose, _: &mut Window, cx: &mut Context<Self>) {
        crate::compose_window::ComposeWindow::open(crate::compose_window::ComposeKind::New, cx);
    }

    fn reply(&mut self, _: &Reply, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(detail) = self.selected_detail.clone() {
            crate::compose_window::ComposeWindow::reply(&detail, cx);
        }
    }

    fn forward(&mut self, _: &Forward, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(detail) = self.selected_detail.clone() {
            crate::compose_window::ComposeWindow::forward(&detail, cx);
        }
    }

    fn open_search_action(&mut self, _: &OpenSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(window, cx);
    }

    fn open_settings_action(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
    }

    fn close_preview_action(
        &mut self,
        _: &ClosePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_preview(window, cx);
    }

    fn dismiss_overlay_action(
        &mut self,
        _: &DismissOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_some() {
            self.close_settings(cx);
        } else if self.preview.is_some() {
            self.close_preview(window, cx);
        } else {
            self.dismiss_search(cx);
        }
    }

    fn next_message(&mut self, _: &NextMessage, window: &mut Window, cx: &mut Context<Self>) {
        self.move_message(1, window, cx);
    }

    fn prev_message(&mut self, _: &PrevMessage, window: &mut Window, cx: &mut Context<Self>) {
        self.move_message(-1, window, cx);
    }

    fn next_mailbox(&mut self, _: &NextMailbox, window: &mut Window, cx: &mut Context<Self>) {
        self.move_mailbox(1, window, cx);
    }

    fn prev_mailbox(&mut self, _: &PrevMailbox, window: &mut Window, cx: &mut Context<Self>) {
        self.move_mailbox(-1, window, cx);
    }

    fn confirm_selection(
        &mut self,
        _: &ConfirmSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.list_focus.is_focused(window) {
            if !self.search_query.trim().is_empty() {
                self.open_selected_search_result(cx);
            } else if let Some(index) = self.selected_index {
                self.select_message_at(index, window, cx);
            }
        }
    }

    fn toggle_sidebar_action(
        &mut self,
        _: &ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(window, cx);
    }

    fn sync_now(&mut self, _: &SyncNow, _: &mut Window, cx: &mut Context<Self>) {
        self.maybe_start_background_sync(cx);
    }

    fn move_message(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_query.trim().is_empty() {
            self.move_search_selection(delta > 0, cx);
            self.open_selected_search_result(cx);
            return;
        }
        let Some(index) = step_index(self.selected_index, self.selected_count(), delta) else {
            return;
        };
        self.select_message_at(index, window, cx);
    }

    fn move_mailbox(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.visible_mailbox_ids();
        let current = ids.iter().position(|&id| Some(id) == self.selected_mailbox);
        let Some(index) = step_index(current, ids.len(), delta) else {
            return;
        };
        window.focus(&self.sidebar_focus, cx);
        self.select_mailbox(ids[index], true, cx);
    }

    fn visible_mailbox_ids(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        for account in &self.accounts {
            let account_key = format!("{}\0{}", account.account, account.server);
            if self.accounts.len() > 1 && self.collapsed_accounts.contains(&account_key) {
                continue;
            }
            ids.extend(
                sidebar_mailboxes(&account.mailboxes)
                    .into_iter()
                    .map(|mailbox| mailbox.id),
            );
        }
        ids
    }

    fn select_message_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self.message_at(index) else {
            return;
        };
        self.selected_index = Some(index);
        window.focus(&self.list_focus, cx);
        self.message_scroll
            .scroll_to_item_strict(index, ScrollStrategy::Center);
        self.select_message(message.id, cx);
    }

    fn render_preview_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.preview.as_ref()?;
        let theme = self.theme;
        let (title, body): (SharedString, AnyElement) = match preview {
            PreviewState::Loading { filename } => (
                filename.clone(),
                div()
                    .text_color(theme.text_secondary)
                    .text_sm()
                    .child("正在解码图片…")
                    .into_any_element(),
            ),
            PreviewState::Image { filename, image } => (
                filename.clone(),
                img(image.clone())
                    .object_fit(ObjectFit::Contain)
                    .max_w(relative(1.))
                    .max_h(relative(1.))
                    .with_fallback(move || {
                        div()
                            .text_color(theme.danger)
                            .text_sm()
                            .child("图片解码失败")
                            .into_any_element()
                    })
                    .into_any_element(),
            ),
            PreviewState::Pdf { filename } => (
                filename.clone(),
                div()
                    .max_w(rems(26.25))
                    .text_color(theme.text_secondary)
                    .text_sm()
                    .child("PDF 暂不内联预览，请保存后用系统阅读器打开。")
                    .into_any_element(),
            ),
            PreviewState::Failed { filename, error } => (
                filename.clone(),
                div()
                    .max_w(rems(26.25))
                    .text_color(theme.danger)
                    .text_sm()
                    .child(error.clone())
                    .into_any_element(),
            ),
        };

        Some(
            div()
                .id("attachment-preview-overlay")
                .key_context("Preview")
                .track_focus(&self.preview_focus)
                .tab_index(0)
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .bg(theme.overlay)
                .on_action(cx.listener(Self::close_preview_action))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_preview(window, cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .py_3()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text)
                                .text_sm()
                                .child(title),
                        )
                        .child(
                            control_button(
                                "attachment-preview-close",
                                theme,
                                ButtonKind::Secondary,
                                false,
                            )
                            .px(rems(0.5))
                            .py(rems(0.25))
                            .text_xs()
                            .child("关闭")
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.close_preview(window, cx);
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_6()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn toggle_account(&mut self, key: String, cx: &mut Context<Self>) {
        self.dismiss_search(cx);
        if !self.collapsed_accounts.remove(&key) {
            self.collapsed_accounts.insert(key);
        }
        cx.notify();
    }

    fn toggle_sidebar(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.dismiss_search(cx);
        if chrome(window, self.prefs.workspace) == Chrome::Compact {
            self.workspace_pane = match self.workspace_pane {
                WorkspacePane::Sidebar => WorkspacePane::List,
                _ => WorkspacePane::Sidebar,
            };
        } else {
            self.sidebar_collapsed = !self.sidebar_collapsed;
        }
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = true;
        let focus = self.search_input.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn dismiss_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_open && self.search_query.is_empty() {
            return;
        }
        self.search_open = false;
        self.clear_search(cx);
    }

    fn select_mailbox(&mut self, id: i64, clear_search: bool, cx: &mut Context<Self>) {
        self.search_open = false;
        let mailbox_changed = self.selected_mailbox != Some(id);
        if !mailbox_changed && !clear_search {
            self.workspace_pane = WorkspacePane::List;
            cx.notify();
            return;
        }

        if mailbox_changed {
            self.selected_mailbox = Some(id);
            self.message_pages.clear();
            self.message_load_error = None;
            self.selected_message = None;
            self.selected_index = None;
            self.selected_detail = None;
            self.detail_load_error = None;
            self.save_state = None;
            self.preview = None;
            self.workspace_pane = WorkspacePane::List;
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
            self.preview = None;
            self.body_scroll.set_offset(Default::default());
        }
        self.select_message(message_id, cx);
    }

    fn handle_search_event(&mut self, event: &ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Edited => {
                self.search_open = true;
                self.search_edited(cx);
            }
            ComposerEvent::Submit => {
                self.search_open = false;
                self.open_selected_search_result(cx);
            }
            ComposerEvent::Cancel => self.dismiss_search(cx),
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

    fn search_dropdown_items(&self) -> Vec<(i64, i64, String)> {
        const LIMIT: usize = 5;
        if self.search_query.trim().is_empty() {
            let Some(mailbox_id) = self.selected_mailbox else {
                return Vec::new();
            };
            let recents = self.message_pages.get(&0).cloned().unwrap_or_else(|| {
                self.store
                    .as_ref()
                    .and_then(|store| store.message_page(mailbox_id, 0, LIMIT).ok())
                    .unwrap_or_default()
            });
            return recents
                .into_iter()
                .take(LIMIT)
                .map(|message| {
                    (
                        message.id,
                        mailbox_id,
                        nonempty_label(&message.subject, "（无主题）"),
                    )
                })
                .collect();
        }
        self.search_results
            .iter()
            .take(LIMIT)
            .map(|hit| {
                (
                    hit.id,
                    hit.mailbox_id,
                    nonempty_label(&hit.subject, "（无主题）"),
                )
            })
            .collect()
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
        self.search_open = false;
        if self.selected_message == Some(id) && self.selected_detail.is_some() {
            self.workspace_pane = WorkspacePane::Reader;
            cx.notify();
            return;
        }

        self.selected_message = Some(id);
        self.workspace_pane = WorkspacePane::Reader;
        self.selected_detail = None;
        self.selected_body_cids = Arc::default();
        self.selected_body_blocks = Arc::default();
        self.detail_load_error = None;
        self.save_state = None;
        self.preview = None;
        self.body_scroll.set_offset(Default::default());
        if self.store.is_none() {
            self.selected_message = None;
            self.detail_load_error = Some("邮件索引未打开".into());
            cx.notify();
            return;
        }

        self.detail_generation = self.detail_generation.wrapping_add(1);
        let generation = self.detail_generation;
        let index_path = self.paths.index.clone();
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    load_detail_with_cids(&index_path, id).map_err(|error| {
                        let problem = classify_index_error(error.as_ref());
                        index_problem_message(
                            &paths,
                            "message_detail_failed",
                            problem,
                            Some(error.as_ref()),
                            "读取邮件详情",
                        )
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if this.detail_generation != generation {
                    return;
                }
                match result {
                    Ok(Some((detail, cids, blocks))) => {
                        this.selected_body_cids = Arc::new(cids);
                        this.selected_body_blocks = Arc::new(blocks);
                        this.body_list = ListState::new(
                            this.selected_body_blocks.len(),
                            ListAlignment::Top,
                            px(1000.),
                        );
                        this.selected_detail = Some(detail);
                    }
                    Ok(None) => {
                        this.selected_message = None;
                        this.detail_load_error = Some("邮件已不存在，请重新生成索引".into());
                    }
                    Err(error) => {
                        this.selected_message = None;
                        this.detail_load_error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    fn render_sidebar(&self, cx: &mut Context<Self>, chrome: Chrome) -> impl IntoElement {
        let theme = self.theme;
        let collapsed = self.sidebar_collapsed && chrome != Chrome::Compact;
        let width = if collapsed { 3.25 } else { 14. };
        let fill_width = chrome == Chrome::Compact;
        let account_name = self
            .accounts
            .first()
            .map(|account| account.account.clone())
            .unwrap_or_else(|| "Foxmail".to_owned());
        let monogram = account_monogram(&account_name);
        let multi_account = self.accounts.len() > 1;

        let mut tree = div()
            .id("mailbox-tree")
            .key_context("MailboxList")
            .track_focus(&self.sidebar_focus)
            .tab_index(0)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .on_action(cx.listener(Self::next_mailbox))
            .on_action(cx.listener(Self::prev_mailbox))
            .on_action(cx.listener(Self::confirm_selection));

        for (account_index, account) in self.accounts.iter().cloned().enumerate() {
            let account_key = format!("{}\0{}", account.account, account.server);
            let account_folded = self.collapsed_accounts.contains(&account_key);
            let toggle_key = account_key.clone();
            if multi_account && !collapsed {
                tree = tree.child(
                    div()
                        .id(("account", account_index))
                        .mx_2()
                        .mt_1()
                        .h(rems(2.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(rems(0.5))
                        .hover(|style| style.bg(theme.raised))
                        .tooltip(tooltip(account.account.clone(), theme))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.sidebar_focus, cx);
                            this.toggle_account(toggle_key.clone(), cx);
                        }))
                        .child(
                            div()
                                .size(rems(1.25))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(theme.text_tertiary)
                                .text_xs()
                                .child(if account_folded { "›" } else { "⌄" }),
                        )
                        .child(
                            div()
                                .ml_1()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .truncate()
                                .text_color(theme.text_secondary)
                                .text_sm()
                                .child(SharedString::from(account.account.clone())),
                        ),
                );
            }

            if multi_account && account_folded {
                continue;
            }

            for mailbox in sidebar_mailboxes(&account.mailboxes) {
                let mailbox_id = mailbox.id;
                let active = Some(mailbox_id) == self.selected_mailbox;
                let label = mailbox_label(&mailbox.source_name, &mailbox.display_name);
                let glyph = mailbox_glyph(&mailbox.source_name, &mailbox.display_name);
                let glyph_color = mailbox_glyph_color(
                    theme,
                    &mailbox.source_name,
                    &mailbox.display_name,
                    self.prefs.sidebar_mono_icons,
                    active,
                );
                let tip = format!("{label} · {}", mailbox.message_count);
                tree = tree.child(
                    div()
                        .id(("mailbox", u64::try_from(mailbox_id).unwrap_or_default()))
                        .when(collapsed, |row| row.mx_1().justify_center())
                        .when(!collapsed, |row| row.mx_2().px_2())
                        .h(rems(2.))
                        .flex()
                        .items_center()
                        .rounded(rems(0.5))
                        .when(active, |style| style.bg(theme.active))
                        .hover(|style| style.bg(theme.raised))
                        .tooltip(tooltip(tip, theme))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.sidebar_focus, cx);
                            this.select_mailbox(mailbox_id, true, cx);
                        }))
                        .child(
                            div()
                                .size(rems(1.25))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(glyph_color)
                                .text_xs()
                                .child(SharedString::from(glyph)),
                        )
                        .when(!collapsed, |row| {
                            row.child(
                                div()
                                    .ml_1()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .truncate()
                                    .text_color(if active {
                                        theme.text
                                    } else {
                                        theme.text_secondary
                                    })
                                    .text_sm()
                                    .child(SharedString::from(label)),
                            )
                            .child(
                                div()
                                    .ml_1()
                                    .flex_shrink_0()
                                    .text_color(theme.text_tertiary)
                                    .text_xs()
                                    .child(SharedString::from(mailbox.message_count.to_string())),
                            )
                        }),
                );
            }
        }

        if self.load_error.is_some() {
            tree = tree.child(
                div()
                    .mx_2()
                    .mt_2()
                    .px_2()
                    .py_2()
                    .rounded(rems(0.5))
                    .bg(theme.raised)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("索引不可用"),
            );
        } else if self.accounts.is_empty() {
            tree = tree.child(
                div()
                    .mx_2()
                    .px_2()
                    .py_4()
                    .text_color(theme.text_tertiary)
                    .text_sm()
                    .child("暂无邮箱"),
            );
        }

        div()
            .h_full()
            .when(fill_width, |el| el.flex_1().w_full().min_w_0())
            .when(!fill_width, |el| el.w(rems(width)).flex_shrink_0())
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .w_full()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .mb_1()
                            .h(rems(2.5))
                            .flex_shrink_0()
                            .when(!collapsed, |header| {
                                header.child(
                                    div()
                                        .absolute()
                                        .left(rems(0.5))
                                        .top(rems(0.25))
                                        .h(rems(2.))
                                        .w(rems(10.25))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(rems(0.5))
                                        .child(
                                            div()
                                                .size(rems(1.25))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(rems(0.375))
                                                .bg(theme.text)
                                                .text_color(theme.sidebar)
                                                .text_xs()
                                                .child(SharedString::from(monogram)),
                                        )
                                        .child(
                                            div()
                                                .ml_1()
                                                .min_w_0()
                                                .flex_1()
                                                .overflow_hidden()
                                                .truncate()
                                                .text_color(theme.text_secondary)
                                                .text_sm()
                                                .child(SharedString::from(account_name)),
                                        ),
                                )
                            })
                            .child(
                                control_button("sidebar-collapse", theme, ButtonKind::Ghost, false)
                                    .absolute()
                                    .top(rems(0.25))
                                    .when(collapsed, |el| el.left(rems(0.5)))
                                    .when(!collapsed, |el| el.right(rems(0.5)))
                                    .size(rems(2.))
                                    .text_color(theme.text_tertiary)
                                    .tooltip(tooltip(
                                        if chrome == Chrome::Compact {
                                            "返回列表"
                                        } else if collapsed {
                                            "展开侧栏"
                                        } else {
                                            "折叠侧栏"
                                        },
                                        theme,
                                    ))
                                    .child(if collapsed { "›" } else { "‹" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_sidebar(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        control_button("compose-button", theme, ButtonKind::Ghost, false)
                            .when(collapsed, |el| el.mx_1().justify_center())
                            .when(!collapsed, |el| el.mx_2().px_2().justify_start())
                            .h(rems(2.))
                            .tooltip(tooltip("写信", theme))
                            .on_click(cx.listener(|_this, _, _, cx| {
                                crate::compose_window::ComposeWindow::open(
                                    crate::compose_window::ComposeKind::New,
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .size(rems(1.25))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(theme.text_secondary)
                                    .text_sm()
                                    .child("✎"),
                            )
                            .when(!collapsed, |el| {
                                el.child(
                                    div()
                                        .ml_1()
                                        .min_w_0()
                                        .flex_1()
                                        .overflow_hidden()
                                        .truncate()
                                        .text_color(theme.text_secondary)
                                        .text_sm()
                                        .child("写信"),
                                )
                            }),
                    )
                    .when(!collapsed, |col| {
                        col.child(
                            div()
                                .mx_2()
                                .mt_2()
                                .mb_1()
                                .h(rems(2.))
                                .px_2()
                                .flex()
                                .items_center()
                                .text_color(theme.text_tertiary)
                                .text_xs()
                                .child("文件夹"),
                        )
                    })
                    .child(tree)
                    .child(
                        control_button("sidebar-settings", theme, ButtonKind::Ghost, false)
                            .when(collapsed, |el| el.mx_1().justify_center())
                            .when(!collapsed, |el| el.mx_2().px_2().justify_start())
                            .mb_1()
                            .h(rems(2.))
                            .tooltip(tooltip("设置", theme))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(window, cx);
                            }))
                            .child(
                                div()
                                    .size(rems(1.25))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(theme.text_secondary)
                                    .text_sm()
                                    .child("⚙"),
                            )
                            .when(!collapsed, |el| {
                                el.child(
                                    div()
                                        .ml_1()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .text_color(theme.text_secondary)
                                        .text_sm()
                                        .child("设置"),
                                )
                            }),
                    )
                    .when(!collapsed, |col| {
                        col.child(
                            control_button("sidebar-sync", theme, ButtonKind::Secondary, false)
                                .mx_2()
                                .mb_3()
                                .mt_2()
                                .h(rems(2.))
                                .text_xs()
                                .child("检查新邮件")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.maybe_start_background_sync(cx);
                                })),
                        )
                    }),
            )
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
                    let monogram = account_monogram(&sender);
                    let date = compact_date(message.date_text.as_deref());
                    let metadata = if message.attachment_count == 0 {
                        date
                    } else {
                        format!("{date}  附件 {}", message.attachment_count)
                    };

                    div()
                        .id(("message", u64::try_from(message.id).unwrap_or_default()))
                        .relative()
                        .h(rems(4.75))
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme.border)
                        .when(active, |style| style.bg(theme.active))
                        .hover(|style| style.bg(theme.raised))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_message_at(index, window, cx);
                        }))
                        .when(active, |element| {
                            element.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(rems(0.75))
                                    .h(rems(3.25))
                                    .w(rems(0.125))
                                    .bg(theme.accent),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .when(!self.prefs.hide_sender_avatar, |row| {
                                    row.child(
                                        div()
                                            .size(rems(1.75))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .bg(theme.raised)
                                            .text_color(theme.text_secondary)
                                            .text_xs()
                                            .child(SharedString::from(monogram)),
                                    )
                                })
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

    fn render_message_list(
        &mut self,
        cx: &mut Context<Self>,
        window: &Window,
        chrome: Chrome,
    ) -> AnyElement {
        let theme = self.theme;
        if !self.search_query.trim().is_empty() {
            return self.render_search_results(cx, window, chrome);
        }
        let message_count = self.selected_count();
        let fill = chrome != Chrome::Default;
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
            .when(fill, |el| el.flex_1().w_full().min_w_0())
            .when(!fill, |el| {
                el.w(relative(0.18)).min_w(rems(13.75)).flex_shrink_0()
            })
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .key_context("MessageList")
            .track_focus(&self.list_focus)
            .tab_index(0)
            .on_action(cx.listener(Self::next_message))
            .on_action(cx.listener(Self::prev_message))
            .on_action(cx.listener(Self::confirm_selection))
            .child(self.render_search_bar(cx, window))
            .child(
                div()
                    .h(rems(3.375))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(chrome == Chrome::Compact, |el| {
                                el.child(
                                    control_button(
                                        "workspace-back-list",
                                        theme,
                                        ButtonKind::Ghost,
                                        false,
                                    )
                                    .size(rems(1.75))
                                    .tooltip(tooltip("返回文件夹", theme))
                                    .child("‹")
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.workspace_pane = WorkspacePane::Sidebar;
                                            cx.notify();
                                        },
                                    )),
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.text)
                                    .text_sm()
                                    .child(self.selected_name()),
                            ),
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
                        .rounded(rems(0.25))
                        .bg(theme.raised)
                        .text_color(theme.text_secondary)
                        .text_xs()
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn render_search_results(
        &mut self,
        cx: &mut Context<Self>,
        window: &Window,
        chrome: Chrome,
    ) -> AnyElement {
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
                    .hover(|style| style.bg(theme.raised))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_selection = index;
                        this.search_open = false;
                        window.focus(&this.list_focus, cx);
                        this.select_message_from_search(mailbox_id, message_id, cx);
                    }))
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
                    .rounded(rems(0.25))
                    .bg(theme.raised)
                    .whitespace_normal()
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .line_height(rems(1.125))
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
                    .child(
                        div()
                            .mb_1()
                            .size(rems(2.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(rems(0.5))
                            .bg(theme.raised)
                            .text_color(theme.text_tertiary)
                            .text_sm()
                            .child("⌕"),
                    )
                    .child(
                        div()
                            .text_color(theme.text)
                            .text_sm()
                            .child("未找到匹配邮件"),
                    )
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child("换个关键词再试试"),
                    ),
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
            .when(chrome != Chrome::Default, |el| {
                el.flex_1().w_full().min_w_0()
            })
            .when(chrome == Chrome::Default, |el| {
                el.w(relative(0.18)).min_w(rems(13.75)).flex_shrink_0()
            })
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .key_context("MessageList")
            .track_focus(&self.list_focus)
            .tab_index(0)
            .on_action(cx.listener(Self::next_message))
            .on_action(cx.listener(Self::prev_message))
            .on_action(cx.listener(Self::confirm_selection))
            .child(self.render_search_bar(cx, window))
            .child(
                div()
                    .h(rems(3.375))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(chrome == Chrome::Compact, |el| {
                                el.child(
                                    control_button(
                                        "workspace-back-search",
                                        theme,
                                        ButtonKind::Ghost,
                                        false,
                                    )
                                    .size(rems(1.75))
                                    .tooltip(tooltip("返回文件夹", theme))
                                    .child("‹")
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.workspace_pane = WorkspacePane::Sidebar;
                                            cx.notify();
                                        },
                                    )),
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.text)
                                    .text_sm()
                                    .child(SharedString::from(format!("搜索：{}", query))),
                            ),
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

    fn drag_split(
        &mut self,
        event: &DragMoveEvent<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y = f32::from(event.event.position.y);
        if let Some(previous_y) = self.split_drag_y {
            let rem = f32::from(window.rem_size()).max(1.0);
            self.attachment_pane_rem =
                clamp_attachment_pane(self.attachment_pane_rem + (previous_y - y) / rem);
            cx.notify();
        }
        self.split_drag_y = Some(y);
    }

    fn render_search_bar(&self, cx: &mut Context<Self>, _window: &Window) -> impl IntoElement {
        let theme = self.theme;
        let search_input = self.search_input.clone();
        let focused = self.search_open;
        let items = self.search_dropdown_items();
        let query = self.search_query.trim();
        let selected = self.selected_message;
        let search_selection = self.search_selection;
        let show_dropdown = self.search_open && query.is_empty() && !items.is_empty();

        let mut results = div()
            .id("search-dropdown")
            .p_1()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .max_h(rems(SEARCH_DROPDOWN_MAX_REM))
            .overflow_y_scroll()
            .child(
                div()
                    .h(rems(1.5))
                    .px_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child("最近邮件"),
            );
        for (index, (message_id, mailbox_id, label)) in items.into_iter().enumerate() {
            let active = selected == Some(message_id) || search_selection == index;
            results = results.child(
                div()
                    .id(("search-hit", u64::try_from(message_id).unwrap_or_default()))
                    .h(rems(2.))
                    .px_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .overflow_hidden()
                    .rounded(rems(0.375))
                    .when(active, |style| style.bg(theme.active))
                    .hover(|style| style.bg(theme.active))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_selection = index;
                        this.search_open = false;
                        window.focus(&this.list_focus, cx);
                        this.select_message_from_search(mailbox_id, message_id, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .truncate()
                            .text_ellipsis()
                            .text_color(theme.text)
                            .text_sm()
                            .child(SharedString::from(label)),
                    ),
            );
        }

        div()
            .id("search-card")
            .mx_3()
            .mt_3()
            .mb_1()
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(rems(0.75))
            .bg(theme.raised)
            .border_1()
            .border_color(if focused { theme.focus } else { theme.border })
            .child(
                div()
                    .id("search-input")
                    .h(rems(2.5))
                    .px_3()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .when(show_dropdown, |el| {
                        el.border_b_1().border_color(theme.border)
                    })
                    .hover(|style| style.bg(theme.active))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.open_search(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .size(rems(1.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.text_tertiary)
                            .text_sm()
                            .line_height(rems(1.))
                            .child("⌕"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .child(search_input),
                    )
                    .when(!self.search_query.is_empty(), |element| {
                        element.child(
                            control_button("clear-search", theme, ButtonKind::Ghost, false)
                                .size(rems(1.5))
                                .rounded_full()
                                .text_color(theme.text_tertiary)
                                .tooltip(tooltip("清除搜索", theme))
                                .child("×")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_search(cx);
                                    this.search_open = true;
                                    let focus = this.search_input.read(cx).focus_handle();
                                    window.focus(&focus, cx);
                                    cx.stop_propagation();
                                })),
                        )
                    }),
            )
            .when(show_dropdown, |card| card.child(results))
    }

    fn render_reader(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
        chrome: Chrome,
    ) -> impl IntoElement {
        let theme = self.theme;
        let mut reader = div()
            .h_full()
            .when(chrome == Chrome::Default, |el| el.min_w(rems(18.75)))
            .flex_1()
            .min_w_0()
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
                            .line_height(rems(1.375))
                            .child(error.clone()),
                    ),
            );
        } else if let Some(detail) = &self.selected_detail {
            let subject = nonempty_label(&detail.subject, "（无主题）");
            let sender = nonempty_label(&detail.sender, "未知发件人");
            let sender_monogram = account_monogram(&sender);
            let recipients = nonempty_label(&detail.recipients, "未记录收件人");
            let date = nonempty_label(&compact_date(detail.date_text.as_deref()), "未知时间");
            let attachments = if detail.attachment_count == 0 {
                "无附件".to_owned()
            } else {
                format!("{} 个附件", detail.attachment_count)
            };

            reader = reader
                .when(chrome != Chrome::Default, |el| {
                    el.child(
                        div()
                            .h(rems(2.5))
                            .px_4()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                control_button(
                                    "workspace-back-reader",
                                    theme,
                                    ButtonKind::Ghost,
                                    false,
                                )
                                .size(rems(1.75))
                                .tooltip(tooltip("返回列表", theme))
                                .child("‹")
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.workspace_pane = WorkspacePane::List;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .ml_1()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.text_secondary)
                                    .text_sm()
                                    .child("邮件"),
                            ),
                    )
                })
                .child(
                    div()
                        .flex_shrink_0()
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
                                    control_button(
                                        "reply-button",
                                        theme,
                                        ButtonKind::Secondary,
                                        false,
                                    )
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .child("回复")
                                    .on_click(cx.listener({
                                        let detail = detail.clone();
                                        move |_this, _, _, cx| {
                                            crate::compose_window::ComposeWindow::reply(
                                                &detail, cx,
                                            );
                                        }
                                    })),
                                )
                                .child(
                                    control_button(
                                        "forward-button",
                                        theme,
                                        ButtonKind::Secondary,
                                        false,
                                    )
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .child("转发")
                                    .on_click(cx.listener({
                                        let detail = detail.clone();
                                        move |_this, _, _, cx| {
                                            crate::compose_window::ComposeWindow::forward(
                                                &detail, cx,
                                            );
                                        }
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .mt_3()
                                .flex()
                                .items_start()
                                .gap_3()
                                .when(!self.prefs.hide_sender_avatar, |row| {
                                    row.child(
                                        div()
                                            .size(rems(2.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .bg(theme.raised)
                                            .text_color(theme.text_secondary)
                                            .text_xs()
                                            .child(SharedString::from(sender_monogram)),
                                    )
                                })
                                .child(detail_row(theme, "发件人", sender)),
                        )
                        .child(detail_row(theme, "收件人", recipients))
                        .child(detail_row(theme, "时间", date))
                        .child(detail_row(theme, "附件", attachments)),
                )
                .child(render_message_body(
                    theme,
                    detail,
                    &self.selected_body_cids,
                    &self.selected_body_blocks,
                    &self.body_list,
                    &self.body_scroll,
                ))
                .when(detail.attachment_count > 0, |reader| {
                    reader
                        .child(
                            div()
                                .id("reader-split")
                                .relative()
                                .h(rems(0.75))
                                .w_full()
                                .flex_shrink_0()
                                .occlude()
                                .cursor(CursorStyle::ResizeRow)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, _| {
                                        this.split_drag_y = None;
                                    }),
                                )
                                .on_drag((), |_, _, _, cx| cx.new(|_| Empty))
                                .on_drag_move(cx.listener(Self::drag_split))
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(rems(0.3125))
                                        .h(rems(0.125))
                                        .bg(theme.border),
                                ),
                        )
                        .child(
                            div()
                                .h(rems(self.attachment_pane_rem))
                                .flex_shrink_0()
                                .min_h_0()
                                .overflow_hidden()
                                .px_6()
                                .py_2()
                                .child(
                                    div()
                                        .id("attachment-pane")
                                        .h_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.attachment_scroll)
                                        .child(self.render_attachment_list(
                                            &detail.attachments,
                                            detail.attachment_count,
                                            detail.attachment_load_error.as_deref(),
                                            window,
                                            cx,
                                        )),
                                ),
                        )
                });
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
        } else if self.selected_message.is_some() {
            reader = reader.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_tertiary)
                    .text_sm()
                    .child("正在加载邮件…"),
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

    fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let chrome = chrome(window, self.prefs.workspace);
        let shell = div().flex_1().min_h_0().min_w_0().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.dismiss_search(cx)),
        );
        match chrome {
            Chrome::Compact => {
                let child = match self.workspace_pane {
                    WorkspacePane::Sidebar => self.render_sidebar(cx, chrome).into_any_element(),
                    WorkspacePane::List => self.render_message_list(cx, window, chrome),
                    WorkspacePane::Reader => {
                        self.render_reader(cx, window, chrome).into_any_element()
                    }
                };
                shell.flex().child(child)
            }
            Chrome::Minimal => {
                let sidebar = self.render_sidebar(cx, chrome);
                let content = if self.workspace_pane == WorkspacePane::Reader {
                    self.render_reader(cx, window, chrome).into_any_element()
                } else {
                    self.render_message_list(cx, window, chrome)
                };
                shell.flex().child(sidebar).child(content)
            }
            Chrome::Default => shell
                .flex()
                .child(self.render_sidebar(cx, chrome))
                .child(self.render_message_list(cx, window, chrome))
                .child(self.render_reader(cx, window, chrome)),
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let draft = self.prefs.clone();
        let editing_dark = draft.is_dark();
        let color = settings_token_color(&draft, editing_dark, TokenId::Primary);
        let hex_input = cx.new(|cx| {
            ComposerInput::new(
                window,
                cx,
                format_hex(color),
                ComposerInputOptions::single_line("#RRGGBB"),
            )
        });
        let _hex_subscription = cx.subscribe(
            &hex_input,
            |this: &mut MailApp, _input, event: &ComposerEvent, cx| {
                if matches!(event, ComposerEvent::Edited) {
                    this.on_settings_hex_edited(cx);
                } else if matches!(event, ComposerEvent::Cancel) {
                    this.close_settings(cx);
                }
            },
        );
        self.settings = Some(SettingsSession {
            draft,
            hex_input,
            _hex_subscription,
            token: TokenId::Primary,
            editing_dark,
        });
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings = None;
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_ref() else {
            return;
        };
        if !session.draft.can_save_custom() {
            return;
        }
        let previous_workspace = self.prefs.workspace;
        let draft = session.draft.clone();
        if prefs::apply(draft.clone()).is_err() {
            return;
        }
        self.prefs = draft;
        self.theme = Theme::from_prefs(&self.prefs);
        if self.prefs.workspace == WorkspaceMode::Minimal
            && previous_workspace != WorkspaceMode::Minimal
        {
            self.workspace_pane = WorkspacePane::List;
        }
        self.settings = None;
        cx.notify();
    }

    fn settings_draft(&mut self) -> Option<&mut Prefs> {
        self.settings.as_mut().map(|session| &mut session.draft)
    }

    fn on_settings_hex_edited(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        let Some(color) = parse_hex(session.hex_input.read(cx).content()) else {
            return;
        };
        let token = session.token;
        let dark = session.editing_dark;
        settings_set_token(&mut session.draft, dark, token, color);
        cx.notify();
    }

    fn set_settings_token(&mut self, token: TokenId, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        session.token = token;
        let color = settings_token_color(&session.draft, session.editing_dark, token);
        let hex = format_hex(color);
        session.hex_input.update(cx, |input, cx| {
            if input.content() != hex {
                input.set_content(hex, cx);
            }
        });
        cx.notify();
    }

    fn set_settings_hsv(&mut self, channel: usize, t: f32, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        let color = settings_token_color(&session.draft, session.editing_dark, session.token);
        let (mut h, mut s, mut v) = rgb_to_hsv(color);
        match channel {
            0 => h = t * 360.0,
            1 => s = t,
            _ => v = t,
        }
        let next = hsv_to_rgb(h, s, v);
        let token = session.token;
        let dark = session.editing_dark;
        settings_set_token(&mut session.draft, dark, token, next);
        let hex = format_hex(next);
        session.hex_input.update(cx, |input, cx| {
            if input.content() != hex {
                input.set_content(hex, cx);
            }
        });
        cx.notify();
    }

    fn render_settings_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.settings.as_ref()?;
        let theme = self.theme;
        let draft = &session.draft;
        let can_save = draft.can_save_custom();
        let failures = if draft.palette == Palette::Custom {
            let mut items = draft.custom_dark.contrast_failures();
            items.extend(draft.custom_light.contrast_failures());
            items
        } else {
            Vec::new()
        };
        let hex_input = session.hex_input.clone();
        let token = session.token;
        let editing_dark = session.editing_dark;
        let tokens = if editing_dark {
            draft.custom_dark
        } else {
            draft.custom_light
        };
        let current = tokens.get(token);
        let (h, s, v) = rgb_to_hsv(current);

        Some(
            div()
                .id("settings-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_settings(cx)),
                )
                .child(
                    div()
                        .id("settings-card")
                        .w(rems(28.))
                        .max_h(relative(0.9))
                        .flex()
                        .flex_col()
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(rems(0.5))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px_4()
                                .h(rems(3.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(div().text_color(theme.text).text_sm().child("设置"))
                                .child(
                                    control_button(
                                        "settings-close",
                                        theme,
                                        ButtonKind::Ghost,
                                        false,
                                    )
                                    .px(rems(0.5))
                                    .child("关闭")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_settings(cx)),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .id("settings-body")
                                .flex_1()
                                .min_h_0()
                                .px_4()
                                .py_3()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(settings_label(theme, "配色方案"))
                                .child(self.render_scheme_row(cx, draft.scheme, theme))
                                .child(settings_label(theme, "调色板"))
                                .child(self.render_palette_row(cx, draft.palette, theme))
                                .when(draft.palette == Palette::Custom, |col| {
                                    col.child(self.render_custom_editor(
                                        cx,
                                        theme,
                                        tokens,
                                        token,
                                        editing_dark,
                                        hex_input,
                                        h,
                                        s,
                                        v,
                                    ))
                                })
                                .when(!failures.is_empty(), |col| {
                                    let mut block = div().flex().flex_col().gap_1();
                                    for (label, ratio) in failures {
                                        block = block.child(
                                            div().text_color(theme.danger).text_xs().child(
                                                format!("{label} 对比度 {ratio:.2}，需要 ≥ 4.5"),
                                            ),
                                        );
                                    }
                                    col.child(block)
                                })
                                .child(self.render_toggle_row(
                                    cx,
                                    "sidebar-mono",
                                    "侧栏单色图标",
                                    draft.sidebar_mono_icons,
                                    theme,
                                ))
                                .child(self.render_toggle_row(
                                    cx,
                                    "hide-avatar",
                                    "隐藏发件人头像",
                                    draft.hide_sender_avatar,
                                    theme,
                                ))
                                .child(self.render_toggle_row(
                                    cx,
                                    "close-tray",
                                    "关窗口进托盘",
                                    draft.close_to_tray,
                                    theme,
                                ))
                                .child(settings_label(theme, "工作区"))
                                .child(self.render_workspace_row(cx, draft.workspace, theme)),
                        )
                        .child(
                            div()
                                .px_4()
                                .h(rems(3.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_end()
                                .border_t_1()
                                .border_color(theme.border)
                                .child(
                                    control_button(
                                        "settings-save",
                                        theme,
                                        ButtonKind::Primary,
                                        !can_save,
                                    )
                                    .h(rems(2.))
                                    .px_3()
                                    .text_sm()
                                    .child("保存")
                                    .when(can_save, |el| {
                                        el.on_click(cx.listener(|this, _, _, cx| {
                                            this.save_settings(cx);
                                        }))
                                    }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_scheme_row(
        &self,
        cx: &mut Context<Self>,
        current: ColorScheme,
        theme: Theme,
    ) -> impl IntoElement {
        settings_segments(
            theme,
            &[
                ("scheme-system", "系统", current == ColorScheme::System),
                ("scheme-dark", "深色", current == ColorScheme::Dark),
                ("scheme-light", "浅色", current == ColorScheme::Light),
            ],
            cx,
            |this, index, cx| {
                if let Some(draft) = this.settings_draft() {
                    draft.scheme =
                        [ColorScheme::System, ColorScheme::Dark, ColorScheme::Light][index];
                }
                cx.notify();
            },
        )
    }

    fn render_palette_row(
        &self,
        cx: &mut Context<Self>,
        current: Palette,
        theme: Theme,
    ) -> impl IntoElement {
        settings_segments(
            theme,
            &[
                ("pal-default", "默认", current == Palette::Default),
                ("pal-teal", "青绿", current == Palette::Teal),
                ("pal-green", "绿色", current == Palette::Green),
                ("pal-purple", "紫色", current == Palette::Purple),
                ("pal-custom", "自定义", current == Palette::Custom),
            ],
            cx,
            |this, index, cx| {
                if let Some(draft) = this.settings_draft() {
                    draft.palette = [
                        Palette::Default,
                        Palette::Teal,
                        Palette::Green,
                        Palette::Purple,
                        Palette::Custom,
                    ][index];
                }
                cx.notify();
            },
        )
    }

    fn render_workspace_row(
        &self,
        cx: &mut Context<Self>,
        current: WorkspaceMode,
        theme: Theme,
    ) -> impl IntoElement {
        settings_segments(
            theme,
            &[
                ("ws-default", "默认", current == WorkspaceMode::Default),
                ("ws-minimal", "极简", current == WorkspaceMode::Minimal),
            ],
            cx,
            |this, index, cx| {
                if let Some(draft) = this.settings_draft() {
                    draft.workspace = [WorkspaceMode::Default, WorkspaceMode::Minimal][index];
                }
                cx.notify();
            },
        )
    }

    fn render_toggle_row(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        label: &'static str,
        on: bool,
        theme: Theme,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(rems(2.))
            .child(div().text_color(theme.text).text_sm().child(label))
            .child(
                div()
                    .id(id)
                    .w(rems(2.))
                    .h(rems(1.125))
                    .px(rems(0.125))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(if on { theme.accent } else { theme.raised })
                    .when(on, |el| el.justify_end())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = this.settings_draft() {
                            if id == "sidebar-mono" {
                                draft.sidebar_mono_icons = !draft.sidebar_mono_icons;
                            } else if id == "close-tray" {
                                draft.close_to_tray = !draft.close_to_tray;
                            } else {
                                draft.hide_sender_avatar = !draft.hide_sender_avatar;
                            }
                        }
                        cx.notify();
                    }))
                    .child(div().size(rems(0.875)).rounded_full().bg(if on {
                        theme.on_accent
                    } else {
                        theme.text_tertiary
                    })),
            )
    }

    fn render_custom_editor(
        &self,
        cx: &mut Context<Self>,
        theme: Theme,
        tokens: TokenSet,
        token: TokenId,
        editing_dark: bool,
        hex_input: Entity<ComposerInput>,
        h: f32,
        s: f32,
        v: f32,
    ) -> impl IntoElement {
        let mut tokens_row = div().flex().flex_col().gap_1();
        for (index, id) in TokenId::ALL.into_iter().enumerate() {
            let color = tokens.get(id);
            let active = id == token;
            tokens_row = tokens_row.child(
                div()
                    .id(("token", index as u64))
                    .h(rems(2.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(rems(0.25))
                    .when(active, |el| el.bg(theme.active))
                    .hover(|el| el.bg(theme.raised))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_settings_token(id, cx)))
                    .child(
                        div()
                            .size(rems(1.))
                            .rounded(rems(0.25))
                            .bg(rgb(color))
                            .border_1()
                            .border_color(theme.border),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text)
                            .text_xs()
                            .child(id.label()),
                    )
                    .child(
                        div()
                            .text_color(theme.text_tertiary)
                            .text_xs()
                            .child(format_hex(color)),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.render_custom_mode_row(cx, theme, editing_dark))
            .child(tokens_row)
            .child(
                div()
                    .h(rems(2.25))
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(theme.raised)
                    .rounded(rems(0.25))
                    .child(hex_input),
            )
            .child(self.render_hsv_slider(cx, theme, 0, h / 360.0, h, s, v))
            .child(self.render_hsv_slider(cx, theme, 1, s, h, s, v))
            .child(self.render_hsv_slider(cx, theme, 2, v, h, s, v))
    }

    fn render_custom_mode_row(
        &self,
        cx: &mut Context<Self>,
        theme: Theme,
        editing_dark: bool,
    ) -> impl IntoElement {
        settings_segments(
            theme,
            &[
                ("custom-light", "浅色", !editing_dark),
                ("custom-dark", "深色", editing_dark),
            ],
            cx,
            |this, index, cx| {
                if let Some(session) = this.settings.as_mut() {
                    session.editing_dark = index == 1;
                    let color =
                        settings_token_color(&session.draft, session.editing_dark, session.token);
                    let hex = format_hex(color);
                    session.hex_input.update(cx, |input, cx| {
                        if input.content() != hex {
                            input.set_content(hex, cx);
                        }
                    });
                }
                cx.notify();
            },
        )
    }

    fn render_hsv_slider(
        &self,
        cx: &mut Context<Self>,
        theme: Theme,
        channel: usize,
        value: f32,
        h: f32,
        s: f32,
        v: f32,
    ) -> impl IntoElement {
        let label = ["H", "S", "V"][channel];
        let mut bar = div()
            .id(("hsv", channel as u64))
            .h(rems(0.75))
            .w_full()
            .flex()
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .on_drag((), |_, _, _, cx| cx.new(|_| Empty))
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<()>, _, cx| {
                let x = f32::from(event.event.position.x) - f32::from(event.bounds.origin.x);
                let w = f32::from(event.bounds.size.width).max(1.0);
                this.set_settings_hsv(channel, (x / w).clamp(0.0, 1.0), cx);
            }));
        for i in 0..24 {
            let t = i as f32 / 23.0;
            let color = match channel {
                0 => hsv_to_rgb(t * 360.0, 1.0, 1.0),
                1 => hsv_to_rgb(h, t, v.max(0.15)),
                _ => hsv_to_rgb(h, s, t),
            };
            bar = bar.child(div().flex_1().h_full().bg(rgb(color)));
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(rems(1.))
                    .flex_shrink_0()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child(label),
            )
            .child(
                div().flex_1().relative().child(bar).child(
                    div()
                        .absolute()
                        .top(rems(-0.125))
                        .left(relative(value.clamp(0.0, 1.0)))
                        .ml(rems(-0.125))
                        .size(rems(1.))
                        .rounded_full()
                        .bg(theme.text)
                        .border_1()
                        .border_color(theme.surface),
                ),
            )
    }
}

impl Render for MailApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let block_for_sync = self.sync.is_some() && self.store.is_none();
        if self.onboarding.is_some() || block_for_sync {
            return div()
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(self.theme.canvas)
                .text_color(self.theme.text)
                .child(self.render_onboarding_or_sync(window, cx));
        }
        let overlay = self.render_preview_overlay(cx);
        let settings = self.render_settings_overlay(cx);
        let banner = self.sync.clone();
        let theme = self.theme;
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.theme.canvas)
            .text_color(self.theme.text)
            .flex()
            .flex_col()
            .key_context("MailApp")
            .on_action(cx.listener(Self::compose))
            .on_action(cx.listener(Self::reply))
            .on_action(cx.listener(Self::forward))
            .on_action(cx.listener(Self::open_search_action))
            .on_action(cx.listener(Self::dismiss_overlay_action))
            .on_action(cx.listener(Self::toggle_sidebar_action))
            .on_action(cx.listener(Self::sync_now))
            .on_action(cx.listener(Self::open_settings_action))
            .when_some(banner, |this, sync| {
                this.child(self.render_sync_banner(theme, sync))
            })
            .child(self.render_workspace(window, cx))
            .when_some(overlay, |this, overlay| this.child(overlay))
            .when_some(settings, |this, settings| this.child(settings))
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
                .rounded(rems(0.25))
                .px_2()
                .py_1()
                .child(field.clone()),
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MailboxKind {
    Inbox,
    Drafts,
    Sent,
    Trash,
    Junk,
    Tagged,
    Other,
}

fn name_eq(value: &str, candidates: &[&str]) -> bool {
    let trimmed = value.trim();
    let ascii = trimmed.to_ascii_uppercase();
    candidates
        .iter()
        .any(|candidate| trimmed == *candidate || ascii == candidate.to_ascii_uppercase())
}

fn mailbox_kind(source_name: &str, display_name: &str) -> MailboxKind {
    let names = [source_name, display_name];
    if names.iter().any(|name| name_eq(name, &["INBOX", "收件箱"])) {
        MailboxKind::Inbox
    } else if names
        .iter()
        .any(|name| name_eq(name, &["DRAFTS", "DRAFT", "草稿箱", "草稿"]))
    {
        MailboxKind::Drafts
    } else if names.iter().any(|name| {
        name_eq(
            name,
            &["SENT", "SENT ITEMS", "SENT MESSAGES", "已发送", "发件箱"],
        )
    }) {
        MailboxKind::Sent
    } else if names.iter().any(|name| {
        name_eq(
            name,
            &["TRASH", "DELETED ITEMS", "已删除", "已刪除", "废件箱"],
        )
    }) {
        MailboxKind::Trash
    } else if names
        .iter()
        .any(|name| name_eq(name, &["JUNK", "SPAM", "垃圾箱", "垃圾邮件", "垃圾郵件"]))
    {
        MailboxKind::Junk
    } else if names
        .iter()
        .any(|name| name_eq(name, &["TAGGED", "LABELS", "标签邮件", "標籤郵件", "标签"]))
    {
        MailboxKind::Tagged
    } else {
        MailboxKind::Other
    }
}

fn mailbox_label(source_name: &str, display_name: &str) -> String {
    match mailbox_kind(source_name, display_name) {
        MailboxKind::Inbox => "收件箱".to_owned(),
        MailboxKind::Drafts => "草稿箱".to_owned(),
        MailboxKind::Sent => "已发送".to_owned(),
        MailboxKind::Trash => "已删除".to_owned(),
        MailboxKind::Junk => "垃圾邮件".to_owned(),
        MailboxKind::Tagged => "标签邮件".to_owned(),
        MailboxKind::Other => display_name.to_owned(),
    }
}

fn sidebar_mailboxes(mailboxes: &[MailboxSummary]) -> Vec<&MailboxSummary> {
    let mut items: Vec<&MailboxSummary> = mailboxes
        .iter()
        .filter(|mailbox| {
            mailbox_kind(&mailbox.source_name, &mailbox.display_name) != MailboxKind::Other
        })
        .collect();
    items.sort_by_key(|mailbox| mailbox_kind(&mailbox.source_name, &mailbox.display_name));
    items
}

fn first_sidebar_mailbox_id(accounts: &[MailboxAccount]) -> Option<i64> {
    accounts
        .iter()
        .flat_map(|account| sidebar_mailboxes(&account.mailboxes))
        .next()
        .map(|mailbox| mailbox.id)
}

fn account_monogram(account: &str) -> String {
    let local = account.split('@').next().unwrap_or(account).trim();
    local
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "M".to_owned())
}

fn mailbox_glyph(source_name: &str, display_name: &str) -> &'static str {
    match mailbox_kind(source_name, display_name) {
        MailboxKind::Inbox => "✉",
        MailboxKind::Drafts => "▭",
        MailboxKind::Sent => "↗",
        MailboxKind::Trash => "⌫",
        MailboxKind::Junk => "⚠",
        MailboxKind::Tagged => "★",
        MailboxKind::Other => "▤",
    }
}

fn mailbox_glyph_color(
    theme: Theme,
    source_name: &str,
    display_name: &str,
    mono: bool,
    active: bool,
) -> gpui::Hsla {
    if mono {
        return if active {
            theme.text
        } else {
            theme.text_secondary
        };
    }
    match mailbox_kind(source_name, display_name) {
        MailboxKind::Inbox => theme.info,
        MailboxKind::Drafts => theme.warning,
        MailboxKind::Sent => theme.success,
        MailboxKind::Trash | MailboxKind::Junk => theme.danger,
        MailboxKind::Tagged => theme.accent,
        MailboxKind::Other if active => theme.text,
        MailboxKind::Other => theme.text_secondary,
    }
}

fn settings_label(theme: Theme, text: &'static str) -> impl IntoElement {
    div()
        .mt_1()
        .text_color(theme.text_tertiary)
        .text_xs()
        .child(text)
}

fn settings_segments(
    theme: Theme,
    items: &[(&'static str, &'static str, bool)],
    cx: &mut Context<MailApp>,
    on_pick: impl Fn(&mut MailApp, usize, &mut Context<MailApp>) + 'static,
) -> impl IntoElement {
    let on_pick = std::rc::Rc::new(on_pick);
    let mut row = div()
        .flex()
        .flex_wrap()
        .gap_1()
        .p_1()
        .rounded(rems(0.375))
        .bg(theme.raised);
    for (index, (id, label, active)) in items.iter().copied().enumerate() {
        let on_pick = on_pick.clone();
        row = row.child(
            control_button(id, theme, ButtonKind::Ghost, false)
                .h(rems(1.75))
                .px_2()
                .text_xs()
                .when(active, |el| el.bg(theme.active).text_color(theme.text))
                .when(!active, |el| el.text_color(theme.text_secondary))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| on_pick(this, index, cx))),
        );
    }
    row
}

fn settings_token_color(prefs: &Prefs, dark: bool, token: TokenId) -> u32 {
    if dark {
        prefs.custom_dark.get(token)
    } else {
        prefs.custom_light.get(token)
    }
}

fn settings_set_token(prefs: &mut Prefs, dark: bool, token: TokenId, color: u32) {
    if dark {
        prefs.custom_dark.set(token, color);
    } else {
        prefs.custom_light.set(token, color);
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
                .w(rems(3.5))
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

fn render_message_body(
    theme: Theme,
    detail: &MessageDetail,
    cids: &Arc<HashMap<String, Arc<Image>>>,
    blocks: &Arc<Vec<BodyBlock>>,
    body_list: &ListState,
    scroll: &ScrollHandle,
) -> impl IntoElement {
    let html = detail.html_body.trim();
    let body = div().flex_1().min_h_0().min_w_0().px_6().py_5();
    if !html.is_empty() {
        let cids = Arc::clone(cids);
        let blocks = Arc::clone(blocks);
        return body
            .flex()
            .flex_col()
            .child(
                list(body_list.clone(), move |ix, _, _| {
                    div()
                        .pb_2()
                        .child(render_block(theme, blocks[ix].clone(), &cids))
                        .into_any_element()
                })
                .flex_1()
                .min_h_0(),
            )
            .into_any_element();
    }
    let text = normalize_plain_body(&detail.body);
    let column = if text.is_empty() {
        div()
            .text_color(theme.text_tertiary)
            .text_sm()
            .child("正文为空")
    } else {
        div()
            .text_color(theme.text_secondary)
            .text_sm()
            .line_height(rems(1.375))
            .child(SharedString::from(text))
    };
    body.id("message-body")
        .overflow_y_scroll()
        .track_scroll(scroll)
        .child(div().flex().flex_col().gap_2().child(column))
        .into_any_element()
}

fn load_detail_with_cids(
    index_path: &std::path::Path,
    message_id: i64,
) -> StoreResult<Option<(MessageDetail, HashMap<String, Arc<Image>>, Vec<BodyBlock>)>> {
    let store = MailStore::open_read_only(index_path)?;
    let Some((detail, cid_payloads)) = store.message_detail_with_assets(message_id)? else {
        return Ok(None);
    };
    let mut cids = HashMap::new();
    for meta in &detail.attachments {
        let Some(cid) = meta.content_id.clone() else {
            continue;
        };
        let Some(bytes) = cid_payloads.get(&cid) else {
            continue;
        };
        if bytes.len() > MAX_IMAGE_PREVIEW_BYTES {
            continue;
        }
        let Some(format) = resolve_image_format(&meta.content_type, &meta.filename, Some(bytes))
        else {
            continue;
        };
        cids.insert(
            cid,
            Arc::new(Image::from_bytes(gpui_image_format(format), bytes.clone())),
        );
    }
    let blocks = if detail.html_body.trim().is_empty() {
        Vec::new()
    } else {
        html::layout_html(&detail.html_body)
    };
    Ok(Some((detail, cids, blocks)))
}

fn render_block(theme: Theme, block: BodyBlock, cids: &HashMap<String, Arc<Image>>) -> AnyElement {
    match block {
        BodyBlock::Heading { level, spans } => div()
            .text_color(theme.text)
            .when(level <= 1, |el| el.text_lg())
            .when(level == 2, |el| el.text_sm())
            .child(render_spans(theme, spans, cids))
            .into_any_element(),
        BodyBlock::Quote { spans } => div()
            .pl_3()
            .border_l_2()
            .border_color(theme.border)
            .text_color(theme.text_secondary)
            .text_sm()
            .child(render_spans(theme, spans, cids))
            .into_any_element(),
        BodyBlock::Pre { text } => div()
            .p_2()
            .rounded(rems(0.25))
            .bg(theme.raised)
            .text_color(theme.text_secondary)
            .text_xs()
            .child(SharedString::from(text))
            .into_any_element(),
        BodyBlock::ListItem { ordered, spans } => div()
            .flex()
            .gap_2()
            .text_color(theme.text_secondary)
            .text_sm()
            .child(if ordered { "•" } else { "•" })
            .child(render_spans(theme, spans, cids))
            .into_any_element(),
        BodyBlock::Paragraph { spans } => div()
            .text_color(theme.text_secondary)
            .text_sm()
            .line_height(rems(1.375))
            .child(render_spans(theme, spans, cids))
            .into_any_element(),
        BodyBlock::Table { rows } => render_table(theme, rows, cids),
    }
}

fn render_table(
    theme: Theme,
    rows: Vec<Vec<foxmail_imap::html::TableCell>>,
    cids: &HashMap<String, Arc<Image>>,
) -> AnyElement {
    let mut table = div()
        .w_full()
        .min_w_0()
        .border_1()
        .border_color(theme.border)
        .rounded(rems(0.25))
        .flex()
        .flex_col();
    for (row_index, row) in rows.into_iter().enumerate() {
        let mut line = div().w_full().flex().when(row_index > 0, |el| {
            el.border_t_1().border_color(theme.border)
        });
        for cell in row {
            line = line.child(
                div()
                    .flex_1()
                    .min_w(rems(4.))
                    .px_2()
                    .py_1()
                    .border_r_1()
                    .border_color(theme.border)
                    .when(cell.occupied, |el| el.py_0())
                    .when(cell.header && !cell.occupied, |el| {
                        el.bg(theme.raised).text_color(theme.text)
                    })
                    .when(!cell.header && !cell.occupied, |el| {
                        el.text_color(theme.text_secondary)
                    })
                    .text_xs()
                    .child(if cell.occupied {
                        div().into_any_element()
                    } else {
                        render_spans(theme, cell.spans, cids).into_any_element()
                    }),
            );
        }
        table = table.child(line);
    }
    div()
        .id("html-table")
        .w_full()
        .min_w_0()
        .overflow_x_scroll()
        .child(table)
        .into_any_element()
}

fn render_spans(
    theme: Theme,
    spans: Vec<InlineSpan>,
    cids: &HashMap<String, Arc<Image>>,
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap_1();
    for span in spans {
        row = match span {
            InlineSpan::Text(text) => row.child(SharedString::from(text)),
            InlineSpan::Break => row.child(div().w_full().h(rems(0.25))),
            InlineSpan::Image { cid, alt } => {
                if let Some(image) = cids.get(&cid) {
                    row.child(
                        img(image.clone())
                            .object_fit(ObjectFit::Contain)
                            .max_w(rems(20.))
                            .max_h(rems(16.)),
                    )
                } else {
                    row.child(div().text_color(theme.text_tertiary).text_xs().child(
                        if alt.is_empty() {
                            SharedString::from("[图片]")
                        } else {
                            SharedString::from(alt)
                        },
                    ))
                }
            }
        };
    }
    row
}

fn clamp_attachment_pane(height: f32) -> f32 {
    height.clamp(ATTACHMENT_PANE_MIN_REM, ATTACHMENT_PANE_MAX_REM)
}

fn preferred_body(plain_body: &str, html_body: &str) -> String {
    if !plain_body.trim().is_empty() {
        return normalize_plain_body(plain_body);
    }

    foxmail_imap::html::html_to_plain(html_body)
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

        if attachments.is_empty() {
            return section.child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(if load_error.is_some() {
                        theme.warning
                    } else {
                        theme.text_tertiary
                    })
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
                        SaveState::Failed(_) => theme.danger,
                        SaveState::Done(_) => theme.success,
                        SaveState::Saving => theme.info,
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
                            .flex_shrink_0()
                            .flex()
                            .gap_1()
                            .child(
                                control_button(
                                    ("attachment-preview", index),
                                    theme,
                                    ButtonKind::Secondary,
                                    false,
                                )
                                .px(rems(0.5))
                                .py(rems(0.25))
                                .text_xs()
                                .child("预览")
                                .on_click(cx.listener({
                                    let preview_name = attachment.filename.clone();
                                    let preview_type = attachment.content_type.clone();
                                    move |this, _, window, cx| {
                                        this.start_attachment_preview(
                                            message_id,
                                            index,
                                            preview_name.clone(),
                                            preview_type.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                            )
                            .child(
                                control_button(
                                    ("attachment-save", index),
                                    theme,
                                    ButtonKind::Secondary,
                                    false,
                                )
                                .px(rems(0.5))
                                .py(rems(0.25))
                                .text_xs()
                                .child("保存")
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start_attachment_save(
                                            message_id,
                                            index,
                                            save_target.clone(),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            ),
                    ),
            );
        }

        section
    }
}

fn load_attachment_bytes(
    index_path: &std::path::Path,
    message_id: i64,
    attachment_index: usize,
) -> Result<Vec<u8>, String> {
    let store = MailStore::open_read_only(index_path).map_err(|error| error.to_string())?;
    match store
        .attachment_payload(message_id, attachment_index)
        .map_err(|error| error.to_string())?
    {
        AttachmentPayload::Bytes(bytes) => Ok(bytes),
        AttachmentPayload::Deferred(deferred) => {
            let account = account_config::load()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "未找到邮箱账号配置".to_owned())?;
            backup::fetch_deferred_attachment(
                &deferred,
                &account.host,
                account.port,
                &account.user,
                &account.password,
            )
            .map_err(|error| error.to_string())
        }
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

fn gpui_image_format(format: SniffedImageFormat) -> ImageFormat {
    match format {
        SniffedImageFormat::Png => ImageFormat::Png,
        SniffedImageFormat::Jpeg => ImageFormat::Jpeg,
        SniffedImageFormat::Gif => ImageFormat::Gif,
        SniffedImageFormat::Webp => ImageFormat::Webp,
        SniffedImageFormat::Bmp => ImageFormat::Bmp,
        SniffedImageFormat::Tiff => ImageFormat::Tiff,
        SniffedImageFormat::Ico => ImageFormat::Ico,
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
        case_insensitive_match_ranges, chrome_for, clamp_attachment_pane, compact_date,
        format_bytes, mailbox_glyph, mailbox_kind, mailbox_label, next_search_selection,
        nonempty_label, normalize_plain_body, preferred_body, search_status, sidebar_mailboxes,
        Chrome, MailboxKind, MailboxSummary, WorkspaceMode, ATTACHMENT_PANE_DEFAULT_REM,
        ATTACHMENT_PANE_MAX_REM, ATTACHMENT_PANE_MIN_REM, COMPACT_WIDTH_REM,
    };

    #[test]
    fn compact_chrome_wins_over_workspace_mode() {
        let rem = 16.0;
        let compact_px = rem * COMPACT_WIDTH_REM - 1.0;
        let wide_px = rem * COMPACT_WIDTH_REM + 200.0;
        assert_eq!(
            chrome_for(compact_px, rem, WorkspaceMode::Default),
            Chrome::Compact
        );
        assert_eq!(
            chrome_for(compact_px, rem, WorkspaceMode::Minimal),
            Chrome::Compact
        );
        assert_eq!(
            chrome_for(wide_px, rem, WorkspaceMode::Default),
            Chrome::Default
        );
        assert_eq!(
            chrome_for(wide_px, rem, WorkspaceMode::Minimal),
            Chrome::Minimal
        );
    }

    #[test]
    fn localizes_standard_mailbox_names_without_overwriting_custom_names() {
        assert_eq!(mailbox_label("INBOX", "INBOX"), "收件箱");
        assert_eq!(mailbox_label("Sent Items", "Sent Items"), "已发送");
        assert_eq!(mailbox_label("&XfJT0ZAB-", "已发送"), "已发送");
        assert_eq!(mailbox_label("&V4NXPnux-", "垃圾箱"), "垃圾邮件");
        assert_eq!(mailbox_label("标签邮件", "标签邮件"), "标签邮件");
        assert_eq!(mailbox_label("工作", "工作"), "工作");
    }

    #[test]
    fn sidebar_keeps_only_standard_roles_in_fixed_order() {
        let mailboxes = [
            ("INBOX", "INBOX"),
            ("&V4NXPnux-", "垃圾箱"),
            ("&XfJSIJZk-", "已删除"),
            ("&XfJT0ZAB-", "已发送"),
            ("&bYhgb3bSW1A-", "消息盒子"),
            ("&f6SQrk72-", "群邮件"),
            ("&g0l6P3ux-", "草稿箱"),
            ("标签邮件", "标签邮件"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (source_name, display_name))| MailboxSummary {
            id: index as i64 + 1,
            source_name: source_name.to_owned(),
            display_name: display_name.to_owned(),
            message_count: 0,
        })
        .collect::<Vec<_>>();
        let labels: Vec<String> = sidebar_mailboxes(&mailboxes)
            .into_iter()
            .map(|mailbox| mailbox_label(&mailbox.source_name, &mailbox.display_name))
            .collect();
        assert_eq!(
            labels,
            [
                "收件箱",
                "草稿箱",
                "已发送",
                "已删除",
                "垃圾邮件",
                "标签邮件"
            ]
        );
        assert_eq!(mailbox_kind("消息盒子", "消息盒子"), MailboxKind::Other);
        assert_eq!(mailbox_kind("群邮件", "群邮件"), MailboxKind::Other);
    }

    #[test]
    fn collapsed_glyphs_identify_role_not_first_character() {
        let sent = mailbox_glyph("&XfJT0ZAB-", "已发送");
        let trash = mailbox_glyph("已删除", "已删除");
        let junk = mailbox_glyph("垃圾箱", "垃圾箱");
        let inbox = mailbox_glyph("收件箱", "收件箱");
        let other = mailbox_glyph("消息盒子", "消息盒子");
        assert_ne!(sent, trash);
        assert_ne!(sent, junk);
        assert_ne!(trash, junk);
        assert_ne!(inbox, other);
        assert_eq!(sent, mailbox_glyph("SENT", "Sent"));
        assert_eq!(other, mailbox_glyph("工作", "工作"));
        assert_ne!(other.chars().next(), Some('工'));
        assert_ne!(sent.chars().next(), Some('已'));
        assert_ne!(trash.chars().next(), Some('已'));
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
    fn falls_back_to_plain_body_when_html_has_no_displayable_text() {
        assert_eq!(
            preferred_body("纯文本正文", "<p>HTML 正文</p>"),
            "纯文本正文"
        );
        assert_eq!(preferred_body("", "<p>HTML 正文</p>"), "HTML 正文");
        assert_eq!(preferred_body("", "<script>secret()</script>"), "");
        assert_eq!(preferred_body("纯文本正文", ""), "纯文本正文");
    }

    #[test]
    fn clamps_attachment_pane_height() {
        assert_eq!(
            clamp_attachment_pane(ATTACHMENT_PANE_DEFAULT_REM),
            ATTACHMENT_PANE_DEFAULT_REM
        );
        assert_eq!(
            clamp_attachment_pane(ATTACHMENT_PANE_MIN_REM - 8.0),
            ATTACHMENT_PANE_MIN_REM
        );
        assert_eq!(
            clamp_attachment_pane(ATTACHMENT_PANE_MAX_REM + 40.0),
            ATTACHMENT_PANE_MAX_REM
        );
    }
}

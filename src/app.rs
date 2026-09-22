use foxmail_imap::account_config::{self, AccountConfig};
use foxmail_imap::backup::{self, BackupOptions};
use foxmail_imap::mail_store::{
    attachment_preview_kind, resolve_image_format, sanitized_attachment_filename, AttachmentMeta,
    AttachmentPayload, AttachmentPreviewKind, MailStore, MailboxAccount, MailboxSummary,
    MessageDetail, MessageSummary, SearchHit, SniffedImageFormat, StoreResult,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    actions, div, img, list, px, relative, rgb, AnyElement, AppContext, ClickEvent, Context, Empty,
    FocusHandle, Image, ImageFormat, InteractiveElement, IntoElement, ListAlignment, ListState,
    MouseButton, ObjectFit, ParentElement, Render, ScrollHandle, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Window,
};

use crate::ui::{control_button, rems, tooltip, ButtonKind};
use gpui_kit::assets::IconName as MailIcon;

type SidebarSignature = Vec<(String, String, Vec<(i64, String, String, usize)>)>;
type DetailAssets = (
    MessageDetail,
    HashMap<String, Arc<Image>>,
    Vec<MailBodyBlock>,
);

actions!(
    mail,
    [
        Compose,
        Reply,
        Forward,
        OpenSearch,
        SearchNext,
        SearchPrevious,
        SearchSubmit,
        SearchDismiss,
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
use crate::navigation::{navigation_step, touch_page, VirtualRows};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::diagnostics::{
    classify_index_error, index_message, log_index_problem, log_operation_error, with_log_failure,
    with_log_path, IndexProblem,
};
use crate::feedback::{self, Feedback, NoticeKind, NoticeLevel};
use crate::fox_theme::apply_foxmail_theme;
use crate::mail_body::{self, normalize_plain_body, MailBodyBlock};
use crate::paths::{index_is_missing, AppPaths};
use crate::prefs::{
    self, format_hex, hsv_to_rgb, parse_hex, rgb_to_hsv, ColorScheme, Palette, Prefs, TokenId,
    WorkspaceMode,
};
use crate::theme::Theme;
use gpui::Focusable as _;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{
    attachment::{
        Attachment, AttachmentActions, AttachmentContent, AttachmentDescription, AttachmentMedia,
        AttachmentTitle,
    },
    avatar::Avatar,
    badge::Badge,
    form::{Field, Form},
    input::Input,
    list::ListItem,
    popover::Popover,
    resizable_panel,
    scroll::ScrollableElement as _,
    select::{Select, SelectEvent, SelectItem, SelectState},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    text::{SelectionFormat, TextView, TextViewStyle},
    tree::{tree, TreeEvent, TreeItem, TreeState},
    v_resizable, v_virtual_list, Icon, IconName, ResizableState, Selectable, Sizable as _,
    VirtualListScrollHandle, WindowExt as _,
};

const MESSAGE_PAGE_SIZE: usize = 50;
const MESSAGE_CACHE_PAGES: usize = 8;
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
    sidebar_tree: gpui::Entity<TreeState>,
    sidebar_signature: SidebarSignature,
    sidebar_selected_mailbox: Option<i64>,
    sidebar_last_selection: Option<SharedString>,
    _sidebar_observer: gpui::Subscription,
    _sidebar_events: gpui::Subscription,
    load_error: Option<SharedString>,
    message_pages: HashMap<usize, Vec<MessageSummary>>,
    message_load_error: Option<SharedString>,
    message_scroll: VirtualListScrollHandle,
    message_row_metrics: VirtualRows,
    search_result_metrics: VirtualRows,
    search_result_scroll: VirtualListScrollHandle,
    message_page_order: VecDeque<usize>,
    attachment_scroll: ScrollHandle,
    attachment_pane_rem: f32,
    reader_split: gpui::Entity<ResizableState>,
    _reader_split_events: gpui::Subscription,
    selected_message: Option<i64>,
    selected_detail: Option<MessageDetail>,
    selected_body_cids: Arc<HashMap<String, Arc<Image>>>,
    selected_body_blocks: Arc<Vec<MailBodyBlock>>,
    body_list: ListState,
    detail_generation: usize,
    detail_load_error: Option<SharedString>,
    search_query: String,
    search_results: Vec<SearchHit>,
    search_error: Option<SharedString>,
    search_pending: bool,
    search_selection: usize,
    search_generation: usize,
    search_input: InputEntity,
    search_dropdown_scroll: ScrollHandle,
    _search_subscription: gpui::Subscription,
    search_open: bool,
    save_state: Option<SaveState>,
    save_message: Option<i64>,
    save_generation: u64,
    feedback: Feedback,
    paths: AppPaths,
    onboarding: Option<Onboarding>,
    sync: Option<SyncState>,
    sync_generation: u64,
    sync_import_started: bool,
    sync_user_requested: bool,
    sync_status_hidden: bool,
    preview: Option<PreviewState>,
    preview_generation: usize,
    sidebar_focus: FocusHandle,
    list_focus: FocusHandle,
    selected_index: Option<usize>,
}

/// Library single-line input entity (`gpui-component` InputState).
type InputEntity = gpui::Entity<gpui_kit::component::input::InputState>;

struct SettingsSession {
    draft: Prefs,
    hex_input: InputEntity,
    _hex_subscription: gpui::Subscription,
    token_select: gpui::Entity<SelectState<Vec<ColorTokenItem>>>,
    _token_subscription: gpui::Subscription,
    sliders: [gpui::Entity<SliderState>; 3],
    _slider_subscriptions: Vec<gpui::Subscription>,
    slider_focus: [FocusHandle; 3],
    // RGB cannot retain hue at zero saturation, or saturation at zero
    // brightness. Keep HSV independently so dragging through grey/black
    // does not jump to a different hue when saturation/brightness returns.
    hsv: [f32; 3],
    hex_invalid: bool,
    save_error: Option<SharedString>,
    token: TokenId,
    editing_dark: bool,
}

impl SettingsSession {
    fn can_save(&self) -> bool {
        self.draft.can_save_custom() && (self.draft.palette != Palette::Custom || !self.hex_invalid)
    }
}

#[derive(Clone)]
struct ColorTokenItem(TokenId);

impl SelectItem for ColorTokenItem {
    type Value = TokenId;

    fn title(&self) -> SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &TokenId {
        &self.0
    }
}

#[derive(Clone, Copy)]
enum BusinessOverlay {
    Settings,
    Preview,
}

// Root builds its layers during MailApp::render. The builder must not borrow
// MailApp again. An observed child view renders later, outside that borrow,
// while still rebuilding on async business updates and theme changes.
struct BusinessOverlayView {
    app: gpui::WeakEntity<MailApp>,
    kind: BusinessOverlay,
    _subscription: gpui::Subscription,
}

impl BusinessOverlayView {
    fn new(app: &gpui::Entity<MailApp>, kind: BusinessOverlay, cx: &mut Context<Self>) -> Self {
        Self {
            app: app.downgrade(),
            kind,
            _subscription: cx.observe(app, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for BusinessOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self
            .app
            .update(cx, |app, cx| match self.kind {
                BusinessOverlay::Settings => app.render_settings_content(window, cx),
                BusinessOverlay::Preview => app.render_preview_content(window, cx),
            })
            .ok()
            .flatten();
        if let Some(body) = body {
            body
        } else {
            // A background index reload can invalidate the preview. Closing
            // after render lets Root restore focus without re-entering it.
            cx.defer_in(window, |_, window, cx| window.close_dialog(cx));
            Empty.into_any_element()
        }
    }
}

// Kit's Popover trigger is Selectable. Keep the composite search field
// (input + clear button) as its anchor, without nesting it in a button.
#[derive(IntoElement)]
struct SearchTrigger(AnyElement);

impl Selectable for SearchTrigger {
    fn selected(self, _: bool) -> Self {
        self
    }
    fn is_selected(&self) -> bool {
        false
    }
}

impl gpui::RenderOnce for SearchTrigger {
    fn render(self, _: &mut Window, _: &mut gpui::App) -> impl IntoElement {
        self.0
    }
}

pub(crate) fn search_key_bindings() -> [gpui::KeyBinding; 5] {
    [
        gpui::KeyBinding::new("down", SearchNext, Some("MailSearch > Input")),
        gpui::KeyBinding::new("up", SearchPrevious, Some("MailSearch > Input")),
        gpui::KeyBinding::new("escape", SearchDismiss, Some("MailSearch > Input")),
        gpui::KeyBinding::new("enter", SearchSubmit, Some("MailSearch > Input")),
        // Popover treats Space as Confirm; inside an Input it must remain text.
        gpui::KeyBinding::new("space", gpui::NoAction, Some("MailSearch > Input")),
    ]
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
    host: InputEntity,
    port: InputEntity,
    user: InputEntity,
    password: InputEntity,
    smtp_host: InputEntity,
    smtp_port: InputEntity,
    smtp_user: InputEntity,
    smtp_password: InputEntity,
    smtp_from: InputEntity,
    security: foxmail_imap::account_config::SmtpSecurity,
    _subscriptions: Vec<gpui::Subscription>,
    error: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    Indexing {
        backed_up: usize,
        skipped: usize,
    },
    Done {
        backed_up: usize,
        skipped: usize,
    },
    Failed(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SaveState {
    Saving,
    Done(SharedString),
    Failed(SharedString),
}

// A bad destination is not a damaged index. Keep the operation stage until the
// UI/logging boundary so the recovery hint and diagnostic category stay honest.
enum AttachmentSaveError {
    Read(String),
    Write(std::io::Error),
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
const MAX_INLINE_CID_BYTES: usize = 4 * 1024 * 1024;

// ponytail: 固定 10 分钟轮询，不做可配置项；嫌慢或嫌费电时再加设置。
const BACKGROUND_SYNC_INTERVAL_SECS: u64 = 600;

impl SyncState {
    fn is_busy(&self) -> bool {
        !matches!(self, Self::Done { .. } | Self::Failed(_))
    }

    fn presentation(&self) -> (NoticeLevel, String, Option<f32>) {
        let level = match self {
            Self::Done { .. } => NoticeLevel::Success,
            Self::Failed(_) => NoticeLevel::Error,
            _ => NoticeLevel::Info,
        };
        let (message, value) = match self {
            Self::Connecting => ("正在连接服务器并检查邮件…".to_owned(), None),
            Self::Scanning { mailboxes, done } => {
                (format!("正在发现文件夹（{done}/{mailboxes}）"), None)
            }
            Self::Mailbox {
                name,
                index,
                total,
                backed_up,
                skipped,
            } => (
                format!(
                    "正在拉取 {name}（{index}/{total}），已下载 {backed_up} 封，跳过 {skipped} 封"
                ),
                None,
            ),
            Self::Message {
                mailbox,
                uid,
                done,
                total,
                ..
            } => {
                let value = feedback::bounded_percent(*done, *total);
                let detail = value
                    .map(|percent| {
                        format!(
                            "{percent:.1}%（{} / {}）",
                            format_bytes(*done),
                            format_bytes(*total)
                        )
                    })
                    .unwrap_or_else(|| format!("大小未知，已接收 {}", format_bytes(*done)));
                (format!("正在下载 {mailbox} / UID {uid}：{detail}"), value)
            }
            Self::Indexing { backed_up, skipped } => (
                format!("下载完成：{backed_up} 封，跳过 {skipped} 封；正在建立索引…"),
                None,
            ),
            Self::Done { backed_up, skipped } => (
                format!("同步完成：下载 {backed_up} 封，跳过 {skipped} 封，索引已就绪。"),
                None,
            ),
            Self::Failed(error) => (format!("同步失败：{error}"), None),
        };
        (level, message, value)
    }
}

fn next_sync_batch(
    receiver: &std::sync::mpsc::Receiver<backup::ProgressEvent>,
) -> (Vec<backup::ProgressEvent>, bool) {
    use std::sync::mpsc::TryRecvError;
    let mut events = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                let terminal = matches!(
                    event,
                    backup::ProgressEvent::Finished { .. } | backup::ProgressEvent::Failed { .. }
                );
                events.push(event);
                if terminal {
                    return (events, true);
                }
            }
            Err(TryRecvError::Empty) => return (events, false),
            Err(TryRecvError::Disconnected) => {
                // Empty polls during a slow download are fine; a disconnected
                // worker without a terminal event is not a successful sync.
                events.push(backup::ProgressEvent::Failed {
                    message: "同步工作线程已结束，但未返回完成状态。请重试。".into(),
                });
                return (events, true);
            }
        }
    }
}

pub fn navigation_key_bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    vec![
        KeyBinding::new("down", NextMessage, Some("MessageList")),
        KeyBinding::new("up", PrevMessage, Some("MessageList")),
        KeyBinding::new("j", NextMessage, Some("MessageList")),
        KeyBinding::new("k", PrevMessage, Some("MessageList")),
        KeyBinding::new("enter", ConfirmSelection, Some("MessageList")),
        KeyBinding::new("down", NextMailbox, Some("MailboxList")),
        KeyBinding::new("up", PrevMailbox, Some("MailboxList")),
        KeyBinding::new("j", NextMailbox, Some("MailboxList")),
        KeyBinding::new("k", PrevMailbox, Some("MailboxList")),
        KeyBinding::new("enter", ConfirmSelection, Some("MailboxList")),
        KeyBinding::new(
            "enter",
            gpui_kit::base::actions::Confirm { secondary: false },
            Some("Tree"),
        ),
    ]
}

impl MailApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            // MailSearch owns navigation without letting candidate keys select mail.
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("搜索邮件…")
                .submit_on_enter(true)
        });
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |this: &mut MailApp,
             _input,
             event: &gpui_kit::component::input::InputEvent,
             window,
             cx| { this.handle_search_event(event, window, cx) },
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

        // 索引缺失 → 进入 onboarding；账号配置不存在时预填空白配置（仅标准端口 993）。
        let onboarding = if index_missing {
            let defaults = account_config::load()
                .ok()
                .flatten()
                .unwrap_or_else(AccountConfig::blank);
            Some(Self::build_onboarding(window, cx, defaults))
        } else {
            None
        };

        let selected_mailbox = first_sidebar_mailbox_id(&accounts);

        let sidebar_tree = cx.new(|cx| TreeState::new(cx));
        let sidebar_observer = cx.observe_in(
            &sidebar_tree,
            window,
            |this: &mut Self, tree, window, cx| {
                this.sidebar_selection_changed(tree, window, cx);
            },
        );
        let sidebar_events = cx.subscribe(
            &sidebar_tree,
            |this: &mut Self, _, event: &TreeEvent, cx| {
                let (id, expanded) = match event {
                    TreeEvent::Expanded(id) => (id, true),
                    TreeEvent::Collapsed(id) => (id, false),
                };
                if let Some(key) = id.strip_prefix("account:") {
                    if expanded {
                        this.collapsed_accounts.remove(key);
                    } else {
                        this.collapsed_accounts.insert(key.to_owned());
                    }
                    cx.notify();
                }
            },
        );
        let reader_split = cx.new(|_| ResizableState::default());
        // The group's on_resize callback is mouse-only in Base 0.6.1. The
        // state event covers both real drags and programmatic resize_panel.
        let reader_split_events = cx.subscribe_in(
            &reader_split,
            window,
            |this: &mut Self, state, _: &gpui_kit::component::ResizablePanelEvent, window, cx| {
                this.reconcile_reader_split(state, window, cx);
            },
        );
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
            sidebar_tree,
            sidebar_signature: Vec::new(),
            sidebar_selected_mailbox: None,
            sidebar_last_selection: None,
            _sidebar_observer: sidebar_observer,
            _sidebar_events: sidebar_events,
            load_error,
            message_pages: HashMap::new(),
            message_load_error: None,
            message_scroll: VirtualListScrollHandle::new(),
            message_row_metrics: VirtualRows::default(),
            search_result_metrics: VirtualRows::default(),
            search_result_scroll: VirtualListScrollHandle::new(),
            message_page_order: VecDeque::new(),
            attachment_scroll: ScrollHandle::new(),
            attachment_pane_rem: ATTACHMENT_PANE_DEFAULT_REM,
            reader_split,
            _reader_split_events: reader_split_events,
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
            search_dropdown_scroll: ScrollHandle::new(),
            search_open: false,
            save_state: None,
            save_message: None,
            save_generation: 0,
            feedback: Feedback::default(),
            paths,
            onboarding,
            sync: None,
            sync_generation: 0,
            sync_import_started: false,
            sync_user_requested: false,
            sync_status_hidden: false,
            preview: None,
            preview_generation: 0,
            sidebar_focus: cx.focus_handle().tab_stop(true),
            list_focus: cx.focus_handle().tab_stop(true),
            selected_index: None,
        };
        if app.onboarding.is_none() {
            // 启动即拉一次新邮件，之后交给固定间隔轮询。
            app.maybe_start_background_sync(cx);
            app.schedule_background_sync(cx);
        }
        apply_foxmail_theme(app.theme, cx);

        // 关窗进托盘：开启时吞掉 WM_CLOSE，把窗口藏起来；退出走托盘菜单 / 任务管理器。
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, app| {
            let Some(entity) = this.upgrade() else {
                return true;
            };
            let mut hide = false;
            entity.update(app, |app, _| hide = app.prefs.close_to_tray);
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
            let mut s =
                gpui_kit::component::input::InputState::new(window, cx).placeholder("IMAP 服务器");
            s.set_value(defaults.incoming.host, window, cx);
            s
        });
        let port = cx.new(|cx| {
            let mut s = gpui_kit::component::input::InputState::new(window, cx).placeholder("端口");
            s.set_value(defaults.incoming.port.to_string(), window, cx);
            s
        });
        let user = cx.new(|cx| {
            let mut s =
                gpui_kit::component::input::InputState::new(window, cx).placeholder("邮箱地址");
            s.set_value(defaults.incoming.user, window, cx);
            s
        });
        let password = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("密码 / 授权码")
                .masked(true)
        });
        let (smtp_defaults, security) = match &defaults.outgoing {
            Some(smtp) => (Some(smtp), smtp.security),
            None => (
                None,
                foxmail_imap::account_config::SmtpSecurity::ImplicitTls,
            ),
        };
        let smtp_host = cx.new(|cx| {
            let mut s = gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("SMTP 服务器（留空则仅可收信）");
            if let Some(smtp) = smtp_defaults {
                s.set_value(smtp.host.clone(), window, cx);
            }
            s
        });
        let smtp_port = cx.new(|cx| {
            let mut s = gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("端口（默认 465）");
            if let Some(smtp) = smtp_defaults {
                s.set_value(smtp.port.to_string(), window, cx);
            }
            s
        });
        let smtp_user = cx.new(|cx| {
            let mut s =
                gpui_kit::component::input::InputState::new(window, cx).placeholder("SMTP 用户名");
            if let Some(smtp) = smtp_defaults {
                s.set_value(smtp.auth_user.clone(), window, cx);
            }
            s
        });
        let smtp_password = cx.new(|cx| {
            let mut s = gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("SMTP 密码")
                .masked(true);
            if let Some(smtp) = smtp_defaults {
                s.set_value(smtp.password.clone(), window, cx);
            }
            s
        });
        let smtp_from = cx.new(|cx| {
            let mut s =
                gpui_kit::component::input::InputState::new(window, cx).placeholder("发件地址");
            if let Some(smtp) = smtp_defaults {
                s.set_value(smtp.from_address.clone(), window, cx);
            }
            s
        });
        let fields = [
            &host,
            &port,
            &user,
            &password,
            &smtp_host,
            &smtp_port,
            &smtp_user,
            &smtp_password,
            &smtp_from,
        ];
        let subscriptions = fields
            .iter()
            .map(|field| {
                cx.subscribe(
                    field,
                    |this: &mut MailApp,
                     _input,
                     _event: &gpui_kit::component::input::InputEvent,
                     cx| {
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
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_password,
            smtp_from,
            security,
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

    fn toggle_smtp_security(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.security = match onboarding.security {
                foxmail_imap::account_config::SmtpSecurity::ImplicitTls => {
                    foxmail_imap::account_config::SmtpSecurity::StartTls
                }
                foxmail_imap::account_config::SmtpSecurity::StartTls => {
                    foxmail_imap::account_config::SmtpSecurity::ImplicitTls
                }
            };
        }
        cx.notify();
    }

    /// 显式便利操作：把收件用户名写入 SMTP 用户名与发件地址字段。
    fn copy_smtp_user_from_account(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(onboarding) = self.onboarding.as_ref() {
            let user = onboarding.user.read(cx).value().to_owned();
            onboarding
                .smtp_user
                .update(cx, |state, cx| state.set_value(user.clone(), window, cx));
            onboarding
                .smtp_from
                .update(cx, |state, cx| state.set_value(user, window, cx));
        }
        cx.notify();
    }

    /// 显式便利操作：把收件密码写入 SMTP 密码字段。
    fn copy_smtp_password_from_account(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(onboarding) = self.onboarding.as_ref() {
            let password = onboarding.password.read(cx).value().to_owned();
            onboarding
                .smtp_password
                .update(cx, |state, cx| state.set_value(password, window, cx));
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
        let host = onboarding.host.read(cx).value().trim().to_owned();
        let user = onboarding.user.read(cx).value().trim().to_owned();
        let port_text = onboarding.port.read(cx).value().trim().to_owned();
        let password = onboarding.password.read(cx).value().to_string();

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

        // SMTP 可选：留空保存 IMAP-only（仅可收信）；填写则要求字段完整。
        let smtp_host = onboarding.smtp_host.read(cx).value().trim().to_owned();
        let outgoing = if smtp_host.is_empty() {
            None
        } else {
            let smtp_port_text = onboarding.smtp_port.read(cx).value().trim().to_owned();
            let smtp_port: u16 = if smtp_port_text.is_empty() {
                465
            } else {
                smtp_port_text
                    .parse()
                    .map_err(|_| SharedString::from("SMTP 端口必须是数字，隐式 TLS 通常为 465"))?
            };
            let smtp_user = onboarding.smtp_user.read(cx).value().trim().to_owned();
            let smtp_password = onboarding.smtp_password.read(cx).value().to_string();
            let smtp_from = onboarding.smtp_from.read(cx).value().trim().to_owned();
            if smtp_user.is_empty() {
                return Err(SharedString::from("请填写 SMTP 用户名"));
            }
            if smtp_password.is_empty() {
                return Err(SharedString::from("请填写 SMTP 密码"));
            }
            if smtp_from.is_empty() {
                return Err(SharedString::from("请填写发件地址"));
            }
            Some(foxmail_imap::account_config::SmtpConfig {
                host: smtp_host,
                port: smtp_port,
                security: onboarding.security,
                auth_user: smtp_user,
                password: smtp_password,
                from_address: smtp_from,
            })
        };
        let account = AccountConfig {
            schema_version: foxmail_imap::account_config::SCHEMA_VERSION,
            incoming: foxmail_imap::account_config::ImapConfig {
                host,
                port,
                user,
                password,
            },
            outgoing,
        };
        account.validate().map_err(SharedString::from)?;
        Ok(account)
    }

    fn start_sync(&mut self, cx: &mut Context<Self>) {
        if self.sync.as_ref().is_some_and(SyncState::is_busy) {
            return;
        }
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
        if account.outgoing.is_none() {
            self.feedback.push(
                NoticeKind::Sync,
                NoticeLevel::Warning,
                "已保存，仅可收信",
                "未填写 SMTP 发件设置；写信发送前请先补全发件账号。",
            );
            cx.notify();
        }
        self.onboarding = None;
        self.run_sync_pipeline(account, true, cx);
    }

    /// 后台轮询入口：已有配置且当前空闲时自动拉新邮件。
    fn maybe_start_background_sync(&mut self, cx: &mut Context<Self>) {
        self.request_sync(false, cx);
    }

    fn request_sync(&mut self, user_requested: bool, cx: &mut Context<Self>) {
        if user_requested {
            self.sync_status_hidden = false;
            cx.notify();
        }
        if self.onboarding.is_some() || self.sync.as_ref().is_some_and(SyncState::is_busy) {
            return;
        }
        match account_config::load() {
            Ok(Some(account)) => self.run_sync_pipeline(account, user_requested, cx),
            Ok(None) if user_requested => {
                self.feedback.push(
                    NoticeKind::Sync,
                    NoticeLevel::Warning,
                    "无法开始同步",
                    "未保存邮箱账号，请先配置账号。",
                );
                cx.notify();
            }
            Err(error) if user_requested => {
                self.feedback.push(
                    NoticeKind::Sync,
                    NoticeLevel::Error,
                    "无法开始同步",
                    format!("读取账号配置失败：{error}"),
                );
                cx.notify();
            }
            _ => {} // An unconfigured background poll is not an operation failure.
        }
    }

    fn begin_sync(&mut self, user_requested: bool, cx: &mut Context<Self>) -> Option<u64> {
        if self.sync.as_ref().is_some_and(SyncState::is_busy) {
            return None;
        }
        self.sync_generation = self.sync_generation.wrapping_add(1);
        self.sync_import_started = false;
        self.sync_user_requested = user_requested;
        self.sync_status_hidden = false;
        self.sync = Some(SyncState::Connecting);
        cx.notify();
        Some(self.sync_generation)
    }

    fn schedule_background_sync(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(BACKGROUND_SYNC_INTERVAL_SECS))
                .await;
            if this
                .update(cx, |this, cx| this.maybe_start_background_sync(cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    /// 手动（onboarding）与后台轮询共用的同步执行段：IMAP 拉取（后台线程）→ 事件泵 → 入库。
    fn run_sync_pipeline(
        &mut self,
        account: AccountConfig,
        user_requested: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(generation) = self.begin_sync(user_requested, cx) else {
            return;
        };
        // Import relative to the index parent, not an account subdirectory:
        // MailStore resolves attachment/EML paths against this same data root.
        let backup_root = self.backup_root_for(&account);
        let import_root = backup_root.clone();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel::<backup::ProgressEvent>();
        let options = BackupOptions {
            output: backup_root,
            mailbox: None,
            max_messages: None,
            chunk_size: backup::DEFAULT_CHUNK_SIZE,
            dry_run: false,
        };
        std::thread::spawn(move || {
            let mut progress = backup::ChannelProgress::new(progress_tx);
            if let Err(error) = backup::run_backup_with(
                &options,
                &account.incoming.host,
                account.incoming.port,
                &account.incoming.user,
                &account.incoming.password,
                &mut progress,
            ) {
                progress.send_failed(error.to_string());
            }
        });
        self.pump_sync_events(generation, progress_rx, import_root, cx);
    }

    fn pump_sync_events(
        &self,
        generation: u64,
        receiver: std::sync::mpsc::Receiver<backup::ProgressEvent>,
        import_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let (events, terminal) = next_sync_batch(&receiver);
                if !events.is_empty()
                    && this
                        .update(cx, |this, cx| {
                            for event in events {
                                this.apply_progress(generation, event, cx);
                            }
                        })
                        .is_err()
                {
                    return;
                }
                if terminal {
                    break;
                }
                if this.upgrade().is_none() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.finish_sync_with(generation, import_root, cx)
            });
        })
        .detach();
    }

    fn apply_progress(
        &mut self,
        generation: u64,
        event: backup::ProgressEvent,
        cx: &mut Context<Self>,
    ) {
        if generation != self.sync_generation || !self.sync.as_ref().is_some_and(SyncState::is_busy)
        {
            return;
        }
        if let backup::ProgressEvent::Failed { message } = event {
            self.fail_sync(generation, message, cx);
            return;
        }
        if matches!(self.sync, Some(SyncState::Indexing { .. })) {
            return;
        }
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
                cx.notify();
                return;
            }
            MessageStart { uid, total, resume } => {
                let (mailbox, mailbox_index, mailbox_total) = match &self.sync {
                    Some(SyncState::Mailbox {
                        name, index, total, ..
                    }) => (name.clone(), *index, *total),
                    Some(SyncState::Message {
                        mailbox,
                        mailbox_index,
                        mailbox_total,
                        ..
                    }) => (mailbox.clone(), *mailbox_index, *mailbox_total),
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
                    done: resume,
                    total,
                }
            }
            MessageProgress { uid, done, total } => {
                if let Some(SyncState::Message {
                    uid: active_uid,
                    done: d,
                    total: t,
                    ..
                }) = self.sync.as_mut()
                {
                    if uid != *active_uid {
                        return;
                    }
                    *d = done;
                    *t = total;
                }
                cx.notify();
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
            Finished { backed_up, skipped } => SyncState::Indexing { backed_up, skipped },
            Failed { message } => SyncState::Failed(SharedString::from(message)),
        };
        self.sync = Some(next);
        cx.notify();
    }

    fn finish_sync_with(&mut self, generation: u64, backup_root: PathBuf, cx: &mut Context<Self>) {
        if generation != self.sync_generation || self.sync_import_started {
            return;
        }
        let Some(SyncState::Indexing { backed_up, .. }) = self.sync.as_ref() else {
            return;
        };
        self.sync_import_started = true;
        // No new data: preserve the reader, pagination and selection. A quiet
        // automatic poll does not produce a repetitive "success" toast.
        if *backed_up == 0 && self.store.is_some() {
            self.complete_sync(generation, cx);
            return;
        }
        let index_path = self.paths.index.clone();
        let task = cx.background_executor().spawn(async move {
            std::thread::spawn(move || -> Result<(), String> {
                let mut store = MailStore::open(&index_path).map_err(|error| error.to_string())?;
                store
                    .import_backup(&backup_root)
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .join()
            .unwrap_or_else(|_| Err("入库线程异常退出".into()))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_sync_import(generation, result, cx)
            });
        })
        .detach();
    }

    fn finish_sync_import(
        &mut self,
        generation: u64,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.sync_generation
            || !self.sync_import_started
            || !matches!(self.sync, Some(SyncState::Indexing { .. }))
        {
            return;
        }
        if let Err(error) = result {
            self.fail_sync(generation, format!("索引入库失败：{error}"), cx);
            return;
        }
        match self.reload_index(cx) {
            Ok(()) => self.complete_sync(generation, cx),
            Err(error) => self.fail_sync(generation, error, cx),
        }
    }

    fn complete_sync(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.sync_generation || !self.sync_import_started {
            return;
        }
        let Some(SyncState::Indexing { backed_up, skipped }) = self.sync.clone() else {
            return;
        };
        self.sync = Some(SyncState::Done { backed_up, skipped });
        let quiet = !self.sync_user_requested && backed_up == 0;
        self.sync_status_hidden = quiet;
        if !quiet {
            self.feedback.push(
                NoticeKind::Sync,
                NoticeLevel::Success,
                "同步完成",
                format!("下载 {backed_up} 封，跳过 {skipped} 封，索引已就绪。"),
            );
        }
        cx.notify();
    }

    fn fail_sync(
        &mut self,
        generation: u64,
        error: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.sync_generation || !self.sync.as_ref().is_some_and(SyncState::is_busy)
        {
            return;
        }
        let error = error.into();
        self.sync = Some(SyncState::Failed(error.clone()));
        self.sync_status_hidden = false;
        self.feedback
            .push(NoticeKind::Sync, NoticeLevel::Error, "同步失败", error);
        cx.notify();
    }

    fn reload_index(&mut self, cx: &mut Context<Self>) -> Result<(), SharedString> {
        match MailStore::open_read_only(&self.paths.index).and_then(|store| {
            let accounts = store.mailbox_tree()?;
            Ok((store, accounts))
        }) {
            Ok((store, accounts)) => {
                self.store = Some(store);
                self.accounts = accounts;
                self.selected_mailbox = first_sidebar_mailbox_id(&self.accounts);
                self.load_error = None;
                self.message_pages.clear();
                self.message_page_order.clear();
                self.clear_reader_selection();
                cx.notify();
                Ok(())
            }
            Err(error) => {
                let problem = classify_index_error(error.as_ref());
                let message = index_problem_message(
                    &self.paths,
                    "index_open_after_sync",
                    problem,
                    Some(error.as_ref()),
                    "拉取后打开索引",
                );
                self.load_error = Some(message.clone());
                Err(message)
            }
        }
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
        let smtp_host_field = onboarding.smtp_host.clone();
        let smtp_port_field = onboarding.smtp_port.clone();
        let smtp_user_field = onboarding.smtp_user.clone();
        let smtp_password_field = onboarding.smtp_password.clone();
        let smtp_from_field = onboarding.smtp_from.clone();
        let smtp_security = onboarding.security;
        let error = onboarding.error.clone();
        let host_label = onboarding.host.read(cx).value().to_string();
        let card_width = (window.rem_size() * 26.25).min(window.viewport_size().width - px(32.));
        let card_max_height = window.viewport_size().height - px(48.);
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
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_color(theme.text_secondary)
                            .text_xs()
                            .child("发件（SMTP，可选）"),
                    )
                    .child(
                        control_button("smtp-security-toggle", theme, ButtonKind::Secondary, false)
                            .text_xs()
                            .child(match smtp_security {
                                foxmail_imap::account_config::SmtpSecurity::ImplicitTls => {
                                    "隐式 TLS"
                                }
                                foxmail_imap::account_config::SmtpSecurity::StartTls => "STARTTLS",
                            })
                            .on_click(cx.listener(Self::toggle_smtp_security)),
                    ),
            )
            .child(onboarding_field(theme, "SMTP 服务器", &smtp_host_field, cx))
            .child(onboarding_field(theme, "SMTP 端口", &smtp_port_field, cx))
            .child(onboarding_field(theme, "SMTP 用户名", &smtp_user_field, cx))
            .child(onboarding_field(
                theme,
                "SMTP 密码",
                &smtp_password_field,
                cx,
            ))
            .child(onboarding_field(theme, "发件地址", &smtp_from_field, cx))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        control_button(
                            "copy-smtp-user-button",
                            theme,
                            ButtonKind::Secondary,
                            false,
                        )
                        .text_xs()
                        .child("SMTP 用户名同邮箱账号")
                        .on_click(cx.listener(Self::copy_smtp_user_from_account)),
                    )
                    .child(
                        control_button(
                            "copy-smtp-password-button",
                            theme,
                            ButtonKind::Secondary,
                            false,
                        )
                        .text_xs()
                        .child("SMTP 密码同收件密码")
                        .on_click(cx.listener(Self::copy_smtp_password_from_account)),
                    ),
            );

        if let Some(error) = error {
            card = card.child(feedback::alert(
                "onboarding-error",
                NoticeLevel::Error,
                error,
            ));
        }

        card = card.child(
            control_button("start-sync-button", theme, ButtonKind::Primary, false)
                .h(rems(2.25))
                .text_sm()
                .child("拉取邮件")
                .on_click(cx.listener(|this, _, _, cx| this.start_sync(cx))),
        );

        // SMTP 字段加入后卡片可能超出视口：与 sync-panel 相同的最大高度 +
        // 纵向滚动，避免矮窗口下只能看到上方字段。
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                card.id("onboarding-card")
                    .test_support()
                    .w(card_width)
                    .max_h(card_max_height)
                    .overflow_y_scroll(),
            )
    }

    fn render_sync_panel(
        &mut self,
        theme: Theme,
        sync: SyncState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (level, message, value) = sync.presentation();
        div()
            .id("sync-panel")
            .test_support()
            .w((window.rem_size() * 26.25).min(window.viewport_size().width - px(32.)))
            .max_h(window.viewport_size().height - px(48.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(feedback::alert("sync-panel-alert", level, message))
            .when(sync.is_busy(), |panel| {
                panel.child(feedback::progress(
                    "sync-panel-progress",
                    value,
                    "当前同步阶段进度",
                ))
            })
            .when(matches!(sync, SyncState::Failed(_)), |panel| {
                panel.child(
                    control_button(
                        "retry-onboarding-button",
                        theme,
                        ButtonKind::Secondary,
                        false,
                    )
                    .child("重新输入账号")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.return_to_onboarding(window, cx)),
                    ),
                )
            })
    }

    fn render_sync_banner(&self, sync: SyncState, cx: &mut Context<Self>) -> impl IntoElement {
        let (level, message, value) = sync.presentation();
        div()
            .id("sync-status-region")
            .test_support()
            .w_full()
            .flex_shrink_0()
            .max_h(rems(8.))
            .overflow_y_scroll()
            .child(
                feedback::alert("sync-status", level, message)
                    .banner()
                    .on_close(cx.listener(|this, _, _, cx| {
                        // Hiding progress must neither cancel nor unlock the job.
                        // Ctrl+Shift+S restores it while the same job is running.
                        this.sync_status_hidden = true;
                        cx.notify();
                    })),
            )
            .when(sync.is_busy(), |panel| {
                panel.child(div().px_4().pb_1().child(feedback::progress(
                    "sync-progress",
                    value,
                    "当前同步阶段进度",
                )))
            })
    }

    /// 设置页“重新配置邮箱账号”入口：关掉设置弹层并回到预填的引导页
    /// （SMTP 字段在此页配置）。
    fn reopen_account_setup(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_settings(window, cx);
        self.return_to_onboarding(window, cx);
    }

    fn return_to_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = account_config::load()
            .ok()
            .flatten()
            .unwrap_or_else(AccountConfig::blank);
        let onboarding = Self::build_onboarding(window, cx, defaults);
        // build_onboarding 已把 password 初始化为空；host/port/user 从保存的配置预填。
        self.onboarding = Some(onboarding);
        self.sync = None;
        self.sync_generation = self.sync_generation.wrapping_add(1);
        self.sync_import_started = false;
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
        if matches!(self.save_state, Some(SaveState::Saving)) {
            return;
        }
        if self.store.is_none() {
            self.reject_attachment_save(message_id, "邮件索引未打开", cx);
            return;
        }
        let target = match crate::save_dialog::prompt_save_path(window, &filename) {
            Ok(target) => target,
            Err(error) => {
                self.reject_attachment_save(message_id, error, cx);
                return;
            }
        };
        self.save_attachment_to(message_id, attachment_index, target, cx);
    }

    fn reject_attachment_save(
        &mut self,
        message_id: i64,
        error: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let error = error.into();
        self.save_generation = self.save_generation.wrapping_add(1);
        self.save_message = Some(message_id);
        self.save_state = Some(SaveState::Failed(error.clone()));
        self.feedback.push(
            NoticeKind::Attachment,
            NoticeLevel::Error,
            "附件保存失败",
            error,
        );
        cx.notify();
    }

    // This is the real post-file-dialog path. None is user cancellation, not
    // an error or a completed save, and must not disturb the previous result.
    fn save_attachment_to(
        &mut self,
        message_id: i64,
        attachment_index: usize,
        target: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.save_state, Some(SaveState::Saving)) {
            return;
        }
        let Some(target) = target else {
            return;
        };
        if self.store.is_none() {
            self.reject_attachment_save(message_id, "邮件索引未打开", cx);
            return;
        }
        self.save_generation = self.save_generation.wrapping_add(1);
        let generation = self.save_generation;
        self.save_message = Some(message_id);
        self.save_state = Some(SaveState::Saving);
        cx.notify();
        let display_name: SharedString = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_string_lossy().into_owned())
            .into();
        let index_path = self.paths.index.clone();
        let task = cx.background_executor().spawn(async move {
            let bytes = load_attachment_bytes(&index_path, message_id, attachment_index)
                .map_err(AttachmentSaveError::Read)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(AttachmentSaveError::Write)?;
            }
            std::fs::write(&target, bytes).map_err(AttachmentSaveError::Write)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_attachment_save(generation, display_name, result, cx)
            });
        })
        .detach();
    }

    fn complete_attachment_save(
        &mut self,
        generation: u64,
        display_name: SharedString,
        result: Result<(), AttachmentSaveError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.save_generation || !matches!(self.save_state, Some(SaveState::Saving))
        {
            return;
        }
        match result {
            Ok(()) => {
                self.save_state = Some(SaveState::Done(display_name.clone()));
                self.feedback.push(
                    NoticeKind::Attachment,
                    NoticeLevel::Success,
                    "附件已保存",
                    display_name,
                );
            }
            Err(error) => {
                let (message, logged) = match error {
                    AttachmentSaveError::Read(error) => (
                        format!("无法读取附件 {display_name}：{error}"),
                        log_index_problem(&self.paths.log, "attachment_read_failed", IndexProblem::QueryFailed, None),
                    ),
                    AttachmentSaveError::Write(error) => (
                        format!("无法写入 {display_name}：{error}。请检查保存位置的访问权限和可用空间。"),
                        log_operation_error(&self.paths.log, "attachment_write_failed", &error),
                    ),
                };
                let message: SharedString = match logged {
                    Ok(()) => with_log_path(message, self.paths.log_hint),
                    Err(_) => with_log_failure(message),
                }
                .into();
                self.save_state = Some(SaveState::Failed(message.clone()));
                self.feedback.push(
                    NoticeKind::Attachment,
                    NoticeLevel::Error,
                    "附件保存失败",
                    message,
                );
            }
        }
        cx.notify();
    }

    fn render_attachment_save_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.save_state.as_ref()?;
        let same_message = self.save_message == self.selected_message;
        let busy = matches!(state, SaveState::Saving);
        if !busy && !same_message {
            return None;
        }
        let (level, message) = match state {
            SaveState::Saving => (
                NoticeLevel::Info,
                if same_message {
                    "正在保存附件…".to_owned()
                } else {
                    "正在保存其他邮件的附件…".to_owned()
                },
            ),
            SaveState::Done(name) => (NoticeLevel::Success, format!("已保存：{name}")),
            SaveState::Failed(error) => (NoticeLevel::Error, format!("保存失败：{error}")),
        };
        Some(
            div()
                .id("attachment-save-status-region")
                .test_support()
                .w_full()
                .mt_2()
                .max_h(rems(8.))
                .overflow_y_scroll()
                .child(
                    feedback::alert("attachment-save-status", level, message).when(
                        !busy,
                        |alert| {
                            alert.on_close(cx.listener(|this, _, _, cx| {
                                this.save_state = None;
                                cx.notify();
                            }))
                        },
                    ),
                )
                .when(busy, |panel| {
                    panel.child(feedback::progress(
                        "attachment-save-progress",
                        None,
                        "附件保存进度",
                    ))
                })
                .into_any_element(),
        )
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
        if window.has_active_dialog(cx) {
            return;
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        let display_name = nonempty_label(&filename, "未命名附件");
        let kind = attachment_preview_kind(&content_type, &filename);
        let known_size = self
            .selected_detail
            .as_ref()
            .filter(|detail| detail.id == message_id)
            .and_then(|detail| detail.attachments.get(attachment_index))
            .map(|meta| meta.size_bytes);
        self.preview = Some(match kind {
            AttachmentPreviewKind::Image
                if known_size.is_some_and(|size| size > MAX_IMAGE_PREVIEW_BYTES) =>
            {
                PreviewState::Failed {
                    filename: display_name.clone().into(),
                    error: "附件超过 20 MiB 预览上限，请保存后打开".into(),
                }
            }
            AttachmentPreviewKind::Pdf => PreviewState::Pdf {
                filename: display_name.clone().into(),
            },
            AttachmentPreviewKind::Unsupported => PreviewState::Failed {
                filename: display_name.clone().into(),
                error: "该类型暂不支持预览，请先保存到本地打开".into(),
            },
            AttachmentPreviewKind::Image if self.store.is_none() => PreviewState::Failed {
                filename: display_name.clone().into(),
                error: "邮件索引未打开".into(),
            },
            AttachmentPreviewKind::Image => PreviewState::Loading {
                filename: display_name.clone().into(),
            },
        });
        self.open_business_dialog(BusinessOverlay::Preview, window, cx);
        cx.notify();
        if !matches!(self.preview, Some(PreviewState::Loading { .. })) {
            return;
        }

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
                        filename: display_name.into(),
                        image,
                    },
                    Err(error) => PreviewState::Failed {
                        filename: display_name.into(),
                        error: error.into(),
                    },
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn invalidate_preview(&mut self) {
        self.preview = None;
        self.preview_generation = self.preview_generation.wrapping_add(1);
    }

    fn close_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview.is_some() {
            self.invalidate_preview();
            window.close_dialog(cx);
            cx.notify();
        }
    }

    fn open_business_dialog(
        &self,
        kind: BusinessOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let panel = cx.new(|cx| BusinessOverlayView::new(&app, kind, cx));
        let app = app.downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let (title, width) = match kind {
                BusinessOverlay::Settings => ("设置", 28.),
                BusinessOverlay::Preview => ("附件预览", 56.),
            };
            let width = (window.rem_size() * width)
                .min(window.viewport_size().width - px(32.))
                .max(px(120.));
            let app = app.clone();
            let panel = panel.clone();
            dialog
                .title(title)
                .w(width)
                .margin_top(px(24.))
                // Enter in a form is not implicit save/dismiss. Only the
                // explicit Save button commits, after business validation.
                .on_ok(|_, _, _| false)
                .on_close(move |_, _, cx| {
                    let _ = app.update(cx, |this, cx| {
                        match kind {
                            BusinessOverlay::Settings => this.settings = None,
                            BusinessOverlay::Preview => this.invalidate_preview(),
                        }
                        cx.notify();
                    });
                })
                .content(move |content, _, _| content.child(panel.clone()))
        });
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

    fn dismiss_overlay_action(
        &mut self,
        _: &DismissOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_some() {
            self.close_settings(window, cx);
        } else if self.preview.is_some() {
            self.close_preview(window, cx);
        } else {
            self.dismiss_search(window, cx);
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
        self.request_sync(true, cx);
    }

    fn move_message(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_query.trim().is_empty() {
            self.move_search_selection(delta > 0, cx);
            self.open_selected_search_result(cx);
            return;
        }
        let Some(index) = navigation_step(self.selected_index, self.selected_count(), delta) else {
            return;
        };
        self.select_message_at(index, window, cx);
    }

    fn move_mailbox(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.visible_mailbox_ids();
        let current = ids.iter().position(|&id| Some(id) == self.selected_mailbox);
        let Some(index) = navigation_step(current, ids.len(), delta) else {
            return;
        };
        window.focus(&self.sidebar_focus, cx);
        self.select_mailbox(ids[index], true, window, cx);
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
            .scroll_to_item(index, ScrollStrategy::Center);
        self.select_message(message.id, cx);
    }

    fn render_preview_content(
        &self,
        window: &Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let preview = self.preview.as_ref()?;
        let theme = self.theme;
        let (title, body): (SharedString, AnyElement) = match preview {
            PreviewState::Loading { filename } => (
                filename.clone(),
                div()
                    .text_color(theme.text_secondary)
                    .child("正在解码图片…")
                    .into_any_element(),
            ),
            PreviewState::Image { filename, image } => (
                filename.clone(),
                img(image.clone())
                    .object_fit(ObjectFit::Contain)
                    .max_w(relative(1.))
                    .max_h((window.viewport_size().height - px(200.)).max(px(80.)))
                    .with_fallback(move || {
                        div()
                            .text_color(theme.danger)
                            .child("图片解码失败")
                            .into_any_element()
                    })
                    .into_any_element(),
            ),
            PreviewState::Pdf { filename } => (
                filename.clone(),
                div()
                    .text_color(theme.text_secondary)
                    .child("PDF 暂不内联预览，请保存后用系统阅读器打开。")
                    .into_any_element(),
            ),
            PreviewState::Failed { filename, error } => (
                filename.clone(),
                div()
                    .text_color(theme.danger)
                    .child(error.clone())
                    .into_any_element(),
            ),
        };
        Some(
            div()
                .id("attachment-preview-content")
                .test_support()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_3()
                .max_h((window.viewport_size().height - px(144.)).max(px(80.)))
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child(title),
                )
                .child(
                    div()
                        .id("attachment-preview-body")
                        .test_support()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .justify_center()
                        .text_sm()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    // 程序化展开/折叠入口：仅被 native 迁移测试调用。
    #[allow(dead_code)]
    fn toggle_account(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_search(window, cx);
        if !self.collapsed_accounts.remove(&key) {
            self.collapsed_accounts.insert(key);
        }
        // Programmatic callers use the same controlled model as Tree events.
        // Rebuilding installs the requested expansion in the official Tree.
        self.sidebar_signature.clear();
        cx.notify();
    }

    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_search(window, cx);
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
        let focus = self.search_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn dismiss_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open && self.search_query.is_empty() {
            return;
        }
        self.search_open = false;
        self.clear_search(window, cx);
    }

    fn clear_reader_selection(&mut self) {
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.selected_message = None;
        self.selected_index = None;
        self.selected_detail = None;
        self.detail_load_error = None;
        self.selected_body_cids = Arc::default();
        self.selected_body_blocks = Arc::default();
        self.body_list.reset(0);
        self.invalidate_preview();
    }

    fn select_mailbox(
        &mut self,
        id: i64,
        clear_search: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_open = false;
        self.workspace_pane = WorkspacePane::List;
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
            self.message_page_order.clear();
            self.clear_reader_selection();
            self.workspace_pane = WorkspacePane::List;
            if self.selected_count() > 0 {
                self.message_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
        }
        if clear_search {
            self.clear_search(window, cx);
        }
        cx.notify();
    }

    fn select_message_from_search(
        &mut self,
        mailbox_id: i64,
        message_id: i64,
        cx: &mut Context<Self>,
    ) {
        if self.selected_mailbox != Some(mailbox_id) {
            self.selected_mailbox = Some(mailbox_id);
            self.message_pages.clear();
            self.message_page_order.clear();
            self.message_load_error = None;
            self.clear_reader_selection();
        }
        match self
            .store
            .as_ref()
            .map(|store| store.message_position(mailbox_id, message_id))
        {
            Some(Ok(position)) => self.selected_index = position,
            Some(Err(error)) => {
                self.selected_index = None;
                self.message_load_error = Some(index_problem_message(
                    &self.paths,
                    "message_position_failed",
                    classify_index_error(error.as_ref()),
                    Some(error.as_ref()),
                    "定位搜索结果",
                ));
            }
            None => self.selected_index = None,
        }
        if let Some(index) = self.selected_index {
            self.message_scroll
                .scroll_to_item(index, ScrollStrategy::Center);
        }
        self.select_message(message_id, cx);
    }

    fn search_is_composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.search_input.update(cx, |input, cx| {
            gpui::EntityInputHandler::marked_text_range(input, window, cx).is_some()
        })
    }

    fn search_next(&mut self, _: &SearchNext, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_is_composing(window, cx) {
            self.search_open = true;
            self.move_search_selection(true, cx);
        }
    }

    fn search_previous(&mut self, _: &SearchPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_is_composing(window, cx) {
            self.search_open = true;
            self.move_search_selection(false, cx);
        }
    }

    fn search_dismiss(&mut self, _: &SearchDismiss, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_is_composing(window, cx) {
            self.dismiss_search(window, cx);
        }
    }

    fn search_submit(&mut self, _: &SearchSubmit, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_search(window, cx);
    }

    fn confirm_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_is_composing(window, cx) {
            return;
        }
        // Commit the highlighted result before Blur or Popover dismissal can
        // clear query/selection. Do not bubble the generic Confirm action.
        self.open_selected_search_result(cx);
        self.search_open = false;
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    fn handle_search_event(
        &mut self,
        event: &gpui_kit::component::input::InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::input::InputEvent;
        match event {
            InputEvent::Change => {
                self.search_open = true;
                self.search_edited(cx);
            }
            InputEvent::PressEnter { .. } => self.confirm_search(window, cx),
            InputEvent::Blur => self.dismiss_search(window, cx),
            InputEvent::Focus => {
                self.search_open = true;
                cx.notify();
            }
        }
    }

    fn search_edited(&mut self, cx: &mut Context<Self>) {
        self.search_dropdown_scroll.set_offset(Default::default());
        self.search_query = self.search_input.read(cx).value().to_string();
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

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_dropdown_scroll.set_offset(Default::default());
        self.search_results.clear();
        self.search_error = None;
        self.search_pending = false;
        self.search_selection = 0;
        if !self.search_input.read(cx).value().is_empty() {
            // Reuse the callback's window instead of re-entering its handle.
            // set_value is silent, so clearing cannot reopen a dismissed search.
            self.search_input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn move_search_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = if self.search_query.trim().is_empty() {
            self.search_dropdown_items().len()
        } else {
            self.search_results.len()
        };
        if count == 0 {
            return;
        }
        self.search_selection = next_search_selection(self.search_selection, count, forward);
        // Child 0 is the section label; keep keyboard selection visible.
        self.search_dropdown_scroll
            .scroll_to_item(self.search_selection + 1);
        self.search_result_scroll
            .scroll_to_item(self.search_selection, ScrollStrategy::Center);
        cx.notify();
    }

    fn open_selected_search_result(&mut self, cx: &mut Context<Self>) {
        if let Some((message_id, mailbox_id, _)) = self
            .search_dropdown_items()
            .get(self.search_selection)
            .cloned()
        {
            self.select_message_from_search(mailbox_id, message_id, cx);
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

        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.selected_message = Some(id);
        self.workspace_pane = WorkspacePane::Reader;
        self.selected_detail = None;
        self.selected_body_cids = Arc::default();
        self.selected_body_blocks = Arc::default();
        self.detail_load_error = None;
        self.invalidate_preview();
        if self.store.is_none() {
            self.selected_message = None;
            self.detail_load_error = Some("邮件索引未打开".into());
            cx.notify();
            return;
        }

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
            let _ = this.update(cx, |this, cx| {
                this.finish_detail_load(generation, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_detail_load(
        &mut self,
        generation: usize,
        result: Result<Option<DetailAssets>, SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.detail_generation != generation {
            return;
        }
        match result {
            Ok(Some((detail, cids, blocks))) if self.selected_message == Some(detail.id) => {
                self.selected_body_cids = Arc::new(cids);
                self.selected_body_blocks = Arc::new(blocks);
                self.body_list = ListState::new(
                    self.selected_body_blocks.len(),
                    ListAlignment::Top,
                    px(1000.),
                );
                self.selected_detail = Some(detail);
            }
            Ok(Some(_)) => return,
            Ok(None) => {
                self.selected_message = None;
                self.detail_load_error = Some("邮件已不存在，请重新生成索引".into());
            }
            Err(error) => {
                self.selected_message = None;
                self.detail_load_error = Some(error);
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

    fn sync_sidebar_tree(&mut self, cx: &mut Context<Self>) {
        let signature = self
            .accounts
            .iter()
            .map(|account| {
                (
                    format!("{}\0{}", account.account, account.server),
                    account.account.clone(),
                    sidebar_mailboxes(&account.mailboxes)
                        .iter()
                        .map(|mailbox| {
                            (
                                mailbox.id,
                                mailbox.source_name.clone(),
                                mailbox.display_name.clone(),
                                mailbox.message_count,
                            )
                        })
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        if signature != self.sidebar_signature {
            let previous = self
                .sidebar_tree
                .read(cx)
                .selected_item()
                .map(|item| item.id.clone());
            let mut items = Vec::new();
            for (key, label, mailboxes) in &signature {
                let children = mailboxes
                    .iter()
                    .map(|(id, source, display, _)| {
                        TreeItem::new(format!("mailbox:{id}"), mailbox_label(source, display))
                    })
                    .collect::<Vec<_>>();
                if signature.len() > 1 {
                    items.push(
                        TreeItem::new(format!("account:{key}"), label.clone())
                            .children(children)
                            .expanded(!self.collapsed_accounts.contains(key)),
                    );
                } else {
                    items.extend(children);
                }
            }
            let wanted = previous.or_else(|| {
                self.selected_mailbox
                    .map(|id| format!("mailbox:{id}").into())
            });
            self.sidebar_last_selection = wanted.clone();
            self.sidebar_tree.update(cx, |state, cx| {
                state.set_items(items, cx);
                let selected = wanted.as_ref().and_then(|id| state.index_of(id));
                state.set_selected_index(selected, cx);
            });
            self.sidebar_signature = signature;
            self.sidebar_selected_mailbox = self.selected_mailbox;
        }
        if self.sidebar_selected_mailbox != self.selected_mailbox {
            self.sidebar_selected_mailbox = self.selected_mailbox;
            let wanted = self
                .selected_mailbox
                .map(|id| SharedString::from(format!("mailbox:{id}")));
            self.sidebar_last_selection = wanted.clone();
            self.sidebar_tree.update(cx, |state, cx| {
                if let Some(id) = wanted {
                    state.reveal_item(&id, ScrollStrategy::Center, cx);
                    state.set_selected_index(state.index_of(&id), cx);
                } else {
                    state.set_selected_index(None, cx);
                }
            });
        }
    }

    fn sidebar_selection_changed(
        &mut self,
        tree: gpui::Entity<TreeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = tree.read(cx).selected_item().map(|item| item.id.clone());
        if selected == self.sidebar_last_selection {
            return;
        }
        self.sidebar_last_selection = selected.clone();
        if let Some(id) = selected
            .as_ref()
            .and_then(|id| id.strip_prefix("mailbox:"))
            .and_then(|id| id.parse::<i64>().ok())
        {
            if self.selected_mailbox != Some(id) {
                self.select_mailbox(id, true, window, cx);
            }
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>, chrome: Chrome) -> impl IntoElement {
        let theme = self.theme;
        let collapsed = self.sidebar_collapsed && chrome != Chrome::Compact;
        let account_name = self
            .accounts
            .first()
            .map(|account| account.account.clone())
            .unwrap_or_else(|| "Foxmail".into());
        let mono = self.prefs.sidebar_mono_icons;
        let metadata = self
            .accounts
            .iter()
            .flat_map(|account| account.mailboxes.iter())
            .map(|mailbox| {
                (
                    format!("mailbox:{}", mailbox.id),
                    (
                        mailbox.id,
                        mailbox.message_count,
                        mailbox_icon(&mailbox.source_name, &mailbox.display_name),
                        mailbox_glyph_color(
                            theme,
                            &mailbox.source_name,
                            &mailbox.display_name,
                            mono,
                            Some(mailbox.id) == self.selected_mailbox,
                        ),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let owner = cx.entity().downgrade();
        let folders = tree(&self.sidebar_tree, move |_, entry, _, _, _| {
            let id = entry.item().id.clone();
            let label = entry.item().label.clone();
            let mailbox = metadata.get(id.as_ref()).cloned();
            let icon = mailbox
                .as_ref()
                .map(|value| Icon::new(value.2).text_color(value.3))
                .unwrap_or_else(|| {
                    Icon::new(if entry.is_expanded() {
                        MailIcon::ChevronDown
                    } else {
                        MailIcon::ChevronRight
                    })
                });
            // ListItem wraps its children in a block container. A single flex
            // child keeps icon, label and count on one line inside a fixed row.
            let mut content = div()
                .id(SharedString::from(format!("sidebar-content:{id}")))
                .test_support()
                .flex()
                .items_center()
                .w_full()
                .min_w_0()
                .gap_2()
                .overflow_hidden()
                .when(collapsed, |content| content.justify_center())
                .child(
                    div()
                        .id(SharedString::from(format!("sidebar-icon:{id}")))
                        .test_support()
                        .flex_shrink_0()
                        .size(rems(1.125))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon.size(rems(1.125))),
                )
                .when(!collapsed, |content| {
                    content.child(
                        div()
                            .id(SharedString::from(format!("sidebar-label:{id}")))
                            .test_support()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(label.clone()),
                    )
                });
            if let Some((_, count, _, _)) = &mailbox {
                content = content.when(!collapsed, |content| {
                    content.child(
                        div()
                            .id(SharedString::from(format!("sidebar-count:{id}")))
                            .test_support()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.text_tertiary)
                            .child(count.to_string()),
                    )
                });
            }
            let mut row = ListItem::new(id.clone())
                .h(rems(2.))
                .rounded(rems(0.375))
                .px_2()
                .when(!collapsed, |row| {
                    row.pl(rems(0.5 + entry.depth() as f32 * 0.75))
                })
                .tooltip(tooltip(
                    mailbox
                        .as_ref()
                        .map(|value| format!("{label} · {}", value.1))
                        .unwrap_or_else(|| label.to_string()),
                    theme,
                ))
                .child(content);
            if let Some((mailbox_id, _, _, _)) = mailbox {
                let owner = owner.clone();
                row = row.on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.select_mailbox(mailbox_id, true, window, cx)
                    });
                });
            }
            row
        });
        div()
            .id("mail-sidebar")
            .test_support()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .when(chrome == Chrome::Compact, |sidebar| {
                sidebar.flex_1().min_w_0()
            })
            .when(chrome != Chrome::Compact, |sidebar| {
                sidebar
                    .w(rems(if collapsed { 3.25 } else { 14. }))
                    .flex_shrink_0()
            })
            .child(
                div()
                    .h(rems(3.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .when(!collapsed, |header| {
                        header
                            .child(Avatar::new().name(account_name.clone()).small())
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_sm()
                                    .child(SharedString::from(account_name)),
                            )
                    })
                    .child(
                        control_button("sidebar-collapse", theme, ButtonKind::Ghost, false)
                            .size(rems(2.))
                            .tooltip(if chrome == Chrome::Compact {
                                "返回列表"
                            } else if collapsed {
                                "展开侧栏"
                            } else {
                                "折叠侧栏"
                            })
                            .icon(if collapsed {
                                MailIcon::PanelLeftOpen
                            } else {
                                MailIcon::PanelLeftClose
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)),
                            ),
                    ),
            )
            .child(
                control_button("compose-button", theme, ButtonKind::Ghost, false)
                    .mx_2()
                    .h(rems(2.25))
                    .icon(MailIcon::SquarePen)
                    .tooltip("写信")
                    .when(!collapsed, |button| button.child("写信"))
                    .on_click(cx.listener(|_, _, _, cx| {
                        crate::compose_window::ComposeWindow::open(
                            crate::compose_window::ComposeKind::New,
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .id("mailbox-tree")
                    .test_support()
                    .key_context("MailboxList")
                    .track_focus(&self.sidebar_focus)
                    .tab_index(0)
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .py_2()
                    .on_action(cx.listener(Self::next_mailbox))
                    .on_action(cx.listener(Self::prev_mailbox))
                    .on_action(cx.listener(Self::confirm_selection))
                    .child(folders)
                    .when(self.accounts.is_empty(), |tree| {
                        tree.child(div().p_2().text_sm().child(if self.load_error.is_some() {
                            "索引不可用"
                        } else {
                            "暂无邮箱"
                        }))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        control_button("sidebar-settings", theme, ButtonKind::Ghost, false)
                            .w_full()
                            .h(rems(2.))
                            .icon(MailIcon::Settings)
                            .tooltip("设置")
                            .when(!collapsed, |button| button.child("设置"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    fn load_message_page(&mut self, page_index: usize) {
        if self.message_pages.contains_key(&page_index) {
            touch_page(
                &mut self.message_pages,
                &mut self.message_page_order,
                page_index,
                MESSAGE_CACHE_PAGES,
            );
            return;
        }
        if self.message_load_error.is_some() {
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
            page_index.saturating_mul(MESSAGE_PAGE_SIZE),
            MESSAGE_PAGE_SIZE,
        ) {
            Ok(messages) => {
                self.message_pages.insert(page_index, messages);
                touch_page(
                    &mut self.message_pages,
                    &mut self.message_page_order,
                    page_index,
                    MESSAGE_CACHE_PAGES,
                );
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
            .map(|index| {
                let Some(message) = self.message_at(index) else {
                    return ListItem::new(("message-unavailable", index))
                        .disabled(true)
                        .h(rems(4.75))
                        .child("邮件暂不可用")
                        .into_any_element();
                };
                let active = self.selected_message == Some(message.id);
                let subject = nonempty_label(&message.subject, "（无主题）");
                let sender = nonempty_label(&message.sender, "未知发件人");
                let date = compact_date(message.date_text.as_deref());
                ListItem::new(("message", u64::try_from(message.id).unwrap_or_default()))
                    .selected(active)
                    .h(rems(4.75))
                    .w_full()
                    .flex_col()
                    .items_stretch()
                    .justify_center()
                    .rounded_none()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_message_at(index, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(!self.prefs.hide_sender_avatar, |row| {
                                row.child(Avatar::new().name(sender.clone()).small())
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
                            .when(message.attachment_count > 0, |row| {
                                row.child(
                                    div().mr_2().child(
                                        Badge::new()
                                            .count(message.attachment_count)
                                            .max(999)
                                            .child(
                                                Icon::new(MailIcon::Paperclip).size(rems(0.875)),
                                            ),
                                    ),
                                )
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(theme.text_tertiary)
                                    .text_xs()
                                    .child(SharedString::from(date)),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .text_sm()
                            .child(SharedString::from(subject)),
                    )
                    .into_any_element()
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
        let sizes = self
            .message_row_metrics
            .sizes(message_count, window.rem_size() * 4.75);
        let list = div()
            .id("message-list-region")
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                v_virtual_list(cx.entity(), "message-list", sizes, |this, range, _, cx| {
                    this.render_message_rows(range, cx)
                })
                .track_scroll(&self.message_scroll)
                .size_full(),
            )
            .vertical_scrollbar(&self.message_scroll);

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
                                    .tooltip("返回文件夹")
                                    .icon(MailIcon::ChevronLeft)
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
        let sizes = self
            .search_result_metrics
            .sizes(self.search_results.len(), window.rem_size() * 5.375);
        let mut rows = div()
            .id("search-results")
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(!self.search_results.is_empty(), |rows| {
                rows.child(
                    v_virtual_list(
                        cx.entity(),
                        "search-result-list",
                        sizes,
                        |this, range, _, cx| this.render_search_result_rows(range, cx),
                    )
                    .track_scroll(&self.search_result_scroll)
                    .size_full(),
                )
            })
            .vertical_scrollbar(&self.search_result_scroll);
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
                            .child(Icon::new(MailIcon::Search).size(rems(1.25))),
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
                                    .tooltip("返回文件夹")
                                    .icon(MailIcon::ChevronLeft)
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

    fn render_search_result_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme;
        let query = self.search_query.trim().to_owned();
        range
            .filter_map(|index| {
                self.search_results.get(index).map(|hit| {
                    let message_id = hit.id;
                    let mailbox_id = hit.mailbox_id;
                    ListItem::new((
                        "search-result",
                        u64::try_from(message_id).unwrap_or_default(),
                    ))
                    .selected(index == self.search_selection)
                    .h(rems(5.375))
                    .w_full()
                    .flex_col()
                    .items_stretch()
                    .justify_center()
                    .rounded_none()
                    .px_4()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_selection = index;
                        this.search_open = false;
                        window.focus(&this.list_focus, cx);
                        this.select_message_from_search(mailbox_id, message_id, cx);
                    }))
                    .child(highlighted_text(
                        theme,
                        &nonempty_label(&hit.sender, "未知发件人"),
                        &query,
                        true,
                    ))
                    .child(highlighted_text(
                        theme,
                        &nonempty_label(&hit.subject, "（无主题）"),
                        &query,
                        false,
                    ))
                    .child(highlighted_text(theme, &hit.snippet, &query, false))
                    .into_any_element()
                })
            })
            .collect()
    }

    fn render_search_bar(&self, cx: &mut Context<Self>, window: &Window) -> impl IntoElement {
        let theme = self.theme;
        let items = self.search_dropdown_items();
        let focus = self.search_input.read(cx).focus_handle(cx);
        let query_empty = self.search_query.trim().is_empty();
        let status = if self.search_pending {
            Some("正在搜索…".to_owned())
        } else if let Some(error) = &self.search_error {
            Some(error.to_string())
        } else if items.is_empty() {
            Some(
                if query_empty {
                    "暂无最近邮件"
                } else {
                    "没有匹配的邮件"
                }
                .to_owned(),
            )
        } else {
            None
        };
        let mut results = div()
            .id("search-dropdown")
            .test_support()
            .flex()
            .flex_col()
            .gap_1()
            .w((window.rem_size() * 20.).min(window.viewport_size().width - px(48.)))
            .max_h(rems(SEARCH_DROPDOWN_MAX_REM))
            .overflow_y_scroll()
            .track_scroll(&self.search_dropdown_scroll)
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.text_tertiary)
                    .child(if query_empty {
                        "最近邮件"
                    } else {
                        "搜索结果"
                    }),
            );
        if let Some(status) = status {
            results = results.child(
                div()
                    .id("search-status")
                    .test_support()
                    .p_2()
                    .text_sm()
                    .text_color(if self.search_error.is_some() {
                        theme.danger
                    } else {
                        theme.text_secondary
                    })
                    .child(status),
            );
        }
        for (index, (message_id, mailbox_id, label)) in items.into_iter().enumerate() {
            results = results.child(
                ListItem::new(("search-hit", index))
                    .selected(self.search_selection == index)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        // Selecting a result must run before Input::Blur clears it.
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_selection = index;
                        this.search_open = false;
                        this.select_message_from_search(mailbox_id, message_id, cx);
                        window.focus(&this.list_focus, cx);
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child(div().min_w_0().truncate().text_sm().child(label)),
            );
        }
        let trigger = div()
            .id("search-input")
            .test_support()
            .h(rems(2.5))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme.raised)
            .rounded(rems(0.5))
            .border_1()
            .border_color(if self.search_open {
                theme.focus
            } else {
                theme.border
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.open_search(window, cx);
                    // Clicking a focused input edits/repositions; it must not toggle the popup shut.
                    cx.stop_propagation();
                }),
            )
            .child(Icon::new(IconName::Search).text_color(theme.text_tertiary))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.search_input)
                        .aria_label("搜索邮件")
                        .appearance(false)
                        .bordered(false),
                ),
            )
            .when(!self.search_query.is_empty(), |el| {
                el.child(
                    control_button("clear-search", theme, ButtonKind::Ghost, false)
                        .size(rems(1.5))
                        .tooltip("清除搜索")
                        .icon(MailIcon::X)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.clear_search(window, cx);
                            this.open_search(window, cx);
                            cx.stop_propagation();
                        })),
                )
            });
        div()
            .id("search-card")
            .key_context("MailSearch")
            .mx_3()
            .mt_3()
            .mb_1()
            .flex_shrink_0()
            .on_action(cx.listener(Self::search_next))
            .on_action(cx.listener(Self::search_previous))
            .on_action(cx.listener(Self::search_submit))
            .on_action(cx.listener(Self::search_dismiss))
            .child(
                Popover::new("mail-search-popover")
                    .anchor(gpui::Anchor::TopLeft)
                    .open(self.search_open)
                    .track_focus(&focus)
                    .on_open_change(cx.listener(|this, open, window, cx| {
                        if *open {
                            this.open_search(window, cx);
                        } else {
                            this.dismiss_search(window, cx);
                        }
                    }))
                    .trigger(SearchTrigger(trigger.into_any_element()))
                    .child(results),
            )
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
                                .tooltip("返回列表")
                                .icon(MailIcon::ChevronLeft)
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
                                    .icon(MailIcon::Reply)
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
                                    .icon(MailIcon::Forward)
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
                                        div().id("reader-avatar").test_support().child(
                                            Avatar::new().name(sender.clone()).size(rems(2.)),
                                        ),
                                    )
                                })
                                .child(detail_row(theme, "发件人", sender)),
                        )
                        .child(detail_row(theme, "收件人", recipients))
                        .child(detail_row(theme, "时间", date))
                        .child(detail_row(theme, "附件", attachments)),
                )
                .child(self.render_reader_content(detail, window, cx));
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

    fn reconcile_reader_split(
        &mut self,
        state: &gpui::Entity<ResizableState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(height) = state.read(cx).sizes().get(1).copied() else {
            return;
        };
        let rem = window.rem_size();
        let container = state.read(cx).container_size();
        let mut target = rem * clamp_attachment_pane(f32::from(height) / f32::from(rem));
        // Base 0.6.1 clamps the dragged panel, not the maximum of its growing
        // sibling. CSS can then display a different height from sizes(). Keep
        // the official shared state and the remembered domain size consistent.
        // Do not attempt an impossible fit below the two panels' minimums.
        if container >= px(64.) + rem * ATTACHMENT_PANE_MIN_REM {
            target = target.min(container - px(64.));
            if f32::from(height - target).abs() > 0.01 {
                state.update(cx, |layout, cx| {
                    layout.resize_panel(0, container - target, window, cx);
                });
            }
        }
        let pane_rem = clamp_attachment_pane(f32::from(target) / f32::from(rem));
        if (self.attachment_pane_rem - pane_rem).abs() > 0.0001 {
            self.attachment_pane_rem = pane_rem;
            cx.notify();
        }
    }

    fn render_reader_content(
        &self,
        detail: &MessageDetail,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = render_message_body(
            self.theme,
            detail,
            &self.selected_body_cids,
            &self.selected_body_blocks,
            &self.body_list,
        )
        .into_any_element();
        if detail.attachment_count == 0 {
            return body;
        }
        let attachments = div()
            .id("attachment-panel-region")
            .test_support()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .px_6()
            .py_2()
            .child(
                div()
                    .id("attachment-pane")
                    .test_support()
                    .relative()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.attachment_scroll)
                    .child(self.render_attachment_list(
                        &detail.attachments,
                        detail.attachment_count,
                        detail.attachment_load_error.as_deref(),
                        window,
                        cx,
                    ))
                    .vertical_scrollbar(&self.attachment_scroll),
            );
        div()
            .id("reader-content")
            .test_support()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_resizable("reader-split")
                    .with_state(&self.reader_split)
                    // Clip content inside each panel, never the panel itself: its
                    // resize hit area extends outside its bounds.
                    .child(
                        resizable_panel()
                            .size_range(px(64.)..gpui::Pixels::MAX)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .size_full()
                                    .min_h_0()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(body),
                            ),
                    )
                    .child(
                        resizable_panel()
                            .size(window.rem_size() * self.attachment_pane_rem)
                            .size_range(
                                window.rem_size() * ATTACHMENT_PANE_MIN_REM
                                    ..window.rem_size() * ATTACHMENT_PANE_MAX_REM,
                            )
                            .flex_none()
                            .child(attachments),
                    ),
            )
            .into_any_element()
    }

    fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let chrome = chrome(window, self.prefs.workspace);
        let shell = div().flex_1().min_h_0().min_w_0().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.dismiss_search(window, cx)),
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
        if self.settings.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let draft = self.prefs.clone();
        let editing_dark = draft.is_dark();
        let color = settings_token_color(&draft, editing_dark, TokenId::Primary);
        let (h, s, v) = rgb_to_hsv(color);
        let hsv = [h / 360., s, v];
        let hex_input = cx.new(|cx| {
            let mut input =
                gpui_kit::component::input::InputState::new(window, cx).placeholder("#RRGGBB");
            input.set_value(format_hex(color), window, cx);
            input
        });
        let hex_subscription = cx.subscribe_in(
            &hex_input,
            window,
            |this: &mut MailApp, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                    this.on_settings_hex_edited(window, cx);
                }
                // Blur is not cancellation. Root owns the modal's lifetime.
            },
        );
        let token_select = cx.new(|cx| {
            SelectState::new(
                TokenId::ALL
                    .into_iter()
                    .map(ColorTokenItem)
                    .collect::<Vec<_>>(),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let token_subscription = cx.subscribe_in(
            &token_select,
            window,
            |this: &mut MailApp, _, event: &SelectEvent<Vec<ColorTokenItem>>, window, cx| {
                if let SelectEvent::Confirm(Some(token)) = event {
                    this.set_settings_token(*token, window, cx);
                }
            },
        );
        let sliders = std::array::from_fn(|channel| {
            cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(1.)
                    .step(0.001)
                    .default_value(hsv[channel])
            })
        });
        let slider_subscriptions = sliders
            .iter()
            .enumerate()
            .map(|(channel, slider)| {
                cx.subscribe_in(
                    slider,
                    window,
                    move |this: &mut MailApp, _, event: &SliderEvent, window, cx| {
                        if let SliderEvent::Change(value) = event {
                            this.set_settings_hsv(channel, value.start(), window, cx);
                        }
                    },
                )
            })
            .collect();
        self.settings = Some(SettingsSession {
            draft,
            hex_input,
            _hex_subscription: hex_subscription,
            token_select,
            _token_subscription: token_subscription,
            sliders,
            _slider_subscriptions: slider_subscriptions,
            slider_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            hsv,
            hex_invalid: false,
            save_error: None,
            token: TokenId::Primary,
            editing_dark,
        });
        self.open_business_dialog(BusinessOverlay::Settings, window, cx);
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            window.close_dialog(cx);
            cx.notify();
        }
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        if !session.can_save() {
            return;
        }
        let draft = session.draft.clone();
        if let Err(error) = prefs::apply(draft.clone()) {
            let message: SharedString = format!("保存设置失败：{error}").into();
            if session.save_error.as_ref() != Some(&message) {
                self.feedback.push(
                    NoticeKind::Settings,
                    NoticeLevel::Error,
                    "设置保存失败",
                    message.clone(),
                );
            }
            session.save_error = Some(message);
            cx.notify();
            return;
        }
        self.feedback.push(
            NoticeKind::Settings,
            NoticeLevel::Success,
            "设置已保存",
            "设置已应用到当前界面及已打开的写信窗口。",
        );
        let previous_workspace = self.prefs.workspace;
        self.prefs = draft;
        self.theme = Theme::from_prefs(&self.prefs);
        apply_foxmail_theme(self.theme, cx);
        if self.prefs.workspace == WorkspaceMode::Minimal
            && previous_workspace != WorkspaceMode::Minimal
        {
            self.workspace_pane = WorkspacePane::List;
        }
        self.close_settings(window, cx);
    }

    fn settings_draft(&mut self) -> Option<&mut Prefs> {
        self.settings.as_mut().map(|session| &mut session.draft)
    }

    fn on_settings_hex_edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        let color = parse_hex(&session.hex_input.read(cx).value());
        session.hex_invalid = color.is_none();
        session.save_error = None;
        if let Some(color) = color {
            settings_set_token(
                &mut session.draft,
                session.editing_dark,
                session.token,
                color,
            );
            let (h, s, v) = rgb_to_hsv(color);
            if s > 0. {
                session.hsv[0] = h / 360.;
            }
            if v > 0. {
                session.hsv[1] = s;
            }
            session.hsv[2] = v;
            for (slider, value) in session.sliders.iter().zip(session.hsv) {
                slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
        cx.notify();
    }

    fn sync_settings_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        let color = settings_token_color(&session.draft, session.editing_dark, session.token);
        let (h, s, v) = rgb_to_hsv(color);
        session.hsv = [h / 360., s, v];
        session.hex_invalid = false;
        session.hex_input.update(cx, |input, cx| {
            input.set_value(format_hex(color), window, cx)
        });
        session.token_select.update(cx, |select, cx| {
            select.set_selected_value(&session.token, window, cx)
        });
        for (slider, value) in session.sliders.iter().zip(session.hsv) {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn set_settings_token(&mut self, token: TokenId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.settings.as_mut() {
            session.token = token;
        }
        self.sync_settings_editor(window, cx);
    }

    fn set_settings_editing_dark(
        &mut self,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.settings.as_mut() {
            session.editing_dark = dark;
        }
        self.sync_settings_editor(window, cx);
    }

    fn set_settings_hsv(
        &mut self,
        channel: usize,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if channel >= 3 || !value.is_finite() {
            return;
        }
        let Some(session) = self.settings.as_mut() else {
            return;
        };
        session.hsv[channel] = value.clamp(0., 1.);
        let [h, s, v] = session.hsv;
        let next = hsv_to_rgb(h * 360., s, v);
        settings_set_token(
            &mut session.draft,
            session.editing_dark,
            session.token,
            next,
        );
        session.hex_invalid = false;
        session.save_error = None;
        session.hex_input.update(cx, |input, cx| {
            input.set_value(format_hex(next), window, cx)
        });
        for (slider, value) in session.sliders.iter().zip(session.hsv) {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn settings_slider_key(
        &mut self,
        channel: usize,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.settings.as_ref() else {
            return;
        };
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        let step = if channel == 0 { 1. / 360. } else { 0.01 };
        let step = if modifiers.shift { step * 10. } else { step };
        let current = session.hsv[channel];
        let next = match event.keystroke.key.as_str() {
            "left" | "down" => current - step,
            "right" | "up" => current + step,
            "home" => 0.,
            "end" => 1.,
            _ => return,
        };
        self.set_settings_hsv(channel, next, window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }

    fn render_settings_content(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.settings.as_ref()?;
        let draft = &session.draft;
        let theme = self.theme;
        let mut failures = Vec::new();
        if draft.palette == Palette::Custom {
            for (mode, tokens) in [("深色", draft.custom_dark), ("浅色", draft.custom_light)] {
                for (label, ratio) in tokens.contrast_failures() {
                    failures.push(format!("{mode} {label} 对比度 {ratio:.2}，需要 ≥ 4.5"));
                }
            }
        }
        Some(
            div()
                .id("settings-content")
                .test_support()
                .flex()
                .flex_col()
                .gap_3()
                .min_h_0()
                .max_h((window.viewport_size().height - px(144.)).max(px(80.)))
                .child(
                    div()
                        .id("settings-body")
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(settings_label(theme, "配色方案"))
                        .child(self.render_scheme_row(cx, draft.scheme, theme))
                        .child(settings_label(theme, "调色板"))
                        .child(self.render_palette_row(cx, draft.palette, theme))
                        .when(draft.palette == Palette::Custom, |col| {
                            col.child(self.render_custom_editor(cx, session))
                        })
                        .children(
                            failures.into_iter().map(|message| {
                                div().text_xs().text_color(theme.danger).child(message)
                            }),
                        )
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
                        .child(self.render_workspace_row(cx, draft.workspace, theme))
                        .child(settings_label(theme, "邮箱账号"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child("仅收信账号无法发送；重新配置可填写 SMTP 发信。"),
                                )
                                .child(
                                    control_button(
                                        "reopen-account-setup",
                                        theme,
                                        ButtonKind::Secondary,
                                        false,
                                    )
                                    .text_xs()
                                    .child("重新配置邮箱账号…")
                                    .on_click(cx.listener(Self::reopen_account_setup)),
                                ),
                        ),
                )
                .when_some(session.save_error.clone(), |col, error| {
                    col.child(feedback::alert(
                        "settings-save-error",
                        NoticeLevel::Error,
                        error,
                    ))
                })
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .justify_end()
                        .gap_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .pt_3()
                        .child(
                            control_button("settings-cancel", theme, ButtonKind::Secondary, false)
                                .child("取消")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_settings(window, cx)
                                })),
                        )
                        .child(
                            control_button(
                                "settings-save",
                                theme,
                                ButtonKind::Primary,
                                !session.can_save(),
                            )
                            .child("保存")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_settings(window, cx)),
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
            |this, index, _window, cx| {
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
            |this, index, _window, cx| {
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
            |this, index, _window, cx| {
                if let Some(draft) = this.settings_draft() {
                    draft.workspace = [WorkspaceMode::Default, WorkspaceMode::Minimal][index];
                }
                cx.notify();
            },
        )
    }

    fn set_settings_flag(&mut self, id: &str, value: bool, cx: &mut Context<Self>) {
        let Some(draft) = self.settings_draft() else {
            return;
        };
        match id {
            "sidebar-mono" => draft.sidebar_mono_icons = value,
            "hide-avatar" => draft.hide_sender_avatar = value,
            "close-tray" => draft.close_to_tray = value,
            _ => return,
        }
        cx.notify();
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
                Switch::new(id)
                    .checked(on)
                    .accessibility_label(label)
                    .on_change(cx.listener(move |this, value, _, cx| {
                        this.set_settings_flag(id, *value, cx);
                    })),
            )
    }

    fn render_custom_editor(
        &self,
        cx: &mut Context<Self>,
        session: &SettingsSession,
    ) -> impl IntoElement {
        let theme = self.theme;
        let color = settings_token_color(&session.draft, session.editing_dark, session.token);
        Form::new()
            .gap_3()
            .child(
                Field::new()
                    .label("编辑模式")
                    .child(self.render_custom_mode_row(cx, theme, session.editing_dark)),
            )
            .child(
                Field::new()
                    .label("主题 token")
                    .child(Select::new(&session.token_select).accessibility_label("主题 token")),
            )
            .child(
                Field::new()
                    .label("HEX")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(rems(1.5))
                                    .flex_shrink_0()
                                    .rounded(rems(0.25))
                                    .bg(rgb(color))
                                    .border_1()
                                    .border_color(theme.border),
                            )
                            .child(
                                Input::new(&session.hex_input)
                                    .aria_label("十六进制颜色")
                                    .when(session.hex_invalid, |input| {
                                        input.border_color(theme.danger)
                                    }),
                            ),
                    )
                    .when(session.hex_invalid, |field| {
                        field.description("请输入 #RRGGBB 或 #RGB；修正前不能保存自定义配色。")
                    }),
            )
            .children((0..3).map(|channel| {
                Field::new()
                    .label(["色相 H", "饱和度 S", "亮度 V"][channel])
                    .child(self.render_hsv_slider(cx, session, channel))
            }))
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
            |this, index, window, cx| this.set_settings_editing_dark(index == 1, window, cx),
        )
    }

    fn render_hsv_slider(
        &self,
        cx: &mut Context<Self>,
        session: &SettingsSession,
        channel: usize,
    ) -> impl IntoElement {
        let theme = self.theme;
        let value = session.hsv[channel];
        let label = if channel == 0 {
            format!("{:.0}°", value * 360.)
        } else {
            format!("{:.0}%", value * 100.)
        };
        // Kit 0.6.1 supplies pointer/accessibility Slider semantics; its
        // SliderState has no Focusable implementation. This thin keyboard
        // adapter adds Tab and arrow/Home/End control, not a second drag path.
        div()
            .id(("settings-hsv", channel))
            .test_support()
            .track_focus(&session.slider_focus[channel])
            .tab_index(0)
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .py_1()
            .border_1()
            .border_color(theme.border)
            .rounded(rems(0.25))
            .focus(|style| style.border_color(theme.focus))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                this.settings_slider_key(channel, event, window, cx)
            }))
            .child(Slider::new(&session.sliders[channel]).flex_1())
            .child(
                div()
                    .w(rems(3.))
                    .text_right()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(label),
            )
    }
}

impl Render for MailApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_sidebar_tree(cx);
        self.feedback.flush(window, cx);
        let block_for_sync = self.sync.is_some() && self.store.is_none();
        if self.onboarding.is_some() || block_for_sync {
            return div()
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(self.theme.canvas)
                .text_color(self.theme.text)
                .child(self.render_onboarding_or_sync(window, cx))
                .children(crate::ui::overlay_layers(window, cx));
        }
        let banner = self.sync.clone().filter(|_| !self.sync_status_hidden);
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
                this.child(self.render_sync_banner(sync, cx))
            })
            .child(self.render_workspace(window, cx))
            .children(crate::ui::overlay_layers(window, cx))
    }
}

fn field_label(theme: Theme, text: &'static str) -> impl IntoElement {
    div().text_color(theme.text_tertiary).text_xs().child(text)
}

fn onboarding_field(
    theme: Theme,
    label: &'static str,
    field: &InputEntity,
    _cx: &mut Context<MailApp>,
) -> impl IntoElement {
    use gpui_kit::component::input::Input;
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
                .child(Input::new(field)),
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

fn mailbox_icon(source_name: &str, display_name: &str) -> MailIcon {
    match mailbox_kind(source_name, display_name) {
        MailboxKind::Inbox => MailIcon::Inbox,
        MailboxKind::Drafts => MailIcon::FilePen,
        MailboxKind::Sent => MailIcon::Send,
        MailboxKind::Trash => MailIcon::Trash,
        MailboxKind::Junk => MailIcon::ShieldAlert,
        MailboxKind::Tagged => MailIcon::Tags,
        MailboxKind::Other => MailIcon::Folder,
    }
}

// 本文件测试专用（native 迁移测试也经 include! 复用）。
#[allow(dead_code)]
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
    _theme: Theme,
    items: &[(&'static str, &'static str, bool)],
    cx: &mut Context<MailApp>,
    on_pick: impl Fn(&mut MailApp, usize, &mut Window, &mut Context<MailApp>) + 'static,
) -> impl IntoElement {
    use gpui_kit::component::{
        tab::{Tab, TabBar},
        Sizable as _,
    };
    let id = items
        .first()
        .map(|item| item.0)
        .unwrap_or("empty-settings-tabs");
    let selected = items.iter().position(|item| item.2).unwrap_or_default();
    TabBar::new(id)
        .segmented()
        .small()
        .selected_index(selected)
        .children(items.iter().map(|(_, label, _)| Tab::new().label(*label)))
        .on_click(cx.listener(move |this, index, window, cx| on_pick(this, *index, window, cx)))
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
                .w(rems(4.5))
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
                .line_clamp(3)
                .line_height(relative(1.35))
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
    blocks: &Arc<Vec<MailBodyBlock>>,
    body_list: &ListState,
) -> AnyElement {
    let cids = Arc::clone(cids);
    let blocks = Arc::clone(blocks);
    div()
        .id(("message-body", u64::try_from(detail.id).unwrap_or_default()))
        .test_support()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .relative()
        .px_6()
        .py_5()
        .flex()
        .flex_col()
        .when(blocks.is_empty(), |body| {
            body.child(
                div()
                    .text_color(theme.text_tertiary)
                    .text_sm()
                    .child("正文为空"),
            )
        })
        .when(!blocks.is_empty(), |body| {
            body.child(
                list(body_list.clone(), move |index, _, _| {
                    div()
                        .id(("mail-body-block", index))
                        .test_support()
                        .w_full()
                        .min_w_0()
                        .pb_3()
                        .child(render_body_block(theme, &blocks[index], &cids))
                        .into_any_element()
                })
                .flex_1()
                .min_h_0()
                .min_w_0(),
            )
        })
        .vertical_scrollbar(body_list)
        .into_any_element()
}

fn load_detail_with_cids(
    index_path: &std::path::Path,
    message_id: i64,
) -> StoreResult<Option<DetailAssets>> {
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
    let blocks = mail_body::prepare_blocks(&detail.html_body, &detail.body);
    Ok(Some((detail, cids, blocks)))
}

fn render_body_block(
    theme: Theme,
    block: &MailBodyBlock,
    cids: &HashMap<String, Arc<Image>>,
) -> AnyElement {
    match block {
        MailBodyBlock::Text { html, monospace } => {
            // Keep the original block hierarchy intact for div-heavy
            // Foxmail/Outlook messages. Only CID image sources are rewritten
            // here; remote URLs were already removed by sanitize_html().
            //
            // L1 readability: GPUI 0.3.4 has no white-space: pre-wrap, so
            // paragraph_gap and line-height are the only knobs that visibly
            // affect mail-body layout today. 1.2rem / 1.65 are the smallest
            // values that make Outlook-style paragraph-less emails readable
            // without making single-paragraph replies feel stretched.
            let rendered_html = rewrite_cid_sources(html, cids);
            div()
                .id("mail-text-region")
                .test_support()
                .w_full()
                .min_w_0()
                .px_4()
                .py_2()
                .when(*monospace, |block| {
                    block.p_2().rounded(rems(0.25)).bg(theme.raised)
                })
                .child(
                    TextView::html("mail-text", SharedString::from(rendered_html))
                        .selectable(true)
                        .selection_format(SelectionFormat::Plain)
                        .scrollable(false)
                        .style(TextViewStyle::default().paragraph_gap(rems(1.2)))
                        .w_full()
                        .min_w_0()
                        .text_base()
                        .line_height(relative(1.65))
                        .text_color(theme.text_secondary)
                        .when(*monospace, |text| text.font_family("Consolas").text_xs()),
                )
                .into_any_element()
        }
        MailBodyBlock::Image { cid, alt } => {
            // The marker id must live outside any Scrollable wrapper:
            // gpui-component's Scrollable rewrites the wrapped element's id
            // to (caller-location, "content"), which hides it from tests.
            if let Some(image) = cids.get(cid) {
                // gpui-pre does not re-layout for ObjectFit; max_w/max_h clamp the
                // two axes independently and distort the image. Size it explicitly
                // from the source aspect ratio (header-only decode) instead;
                // over-wide images scroll horizontally like table rows.
                let dims = image::ImageReader::new(std::io::Cursor::new(&image.bytes))
                    .with_guessed_format()
                    .ok()
                    .and_then(|reader| reader.into_dimensions().ok());
                let content = if let Some((nw, nh)) = dims.filter(|(_, nh)| *nh > 0) {
                    let scale = (384.0 / nh as f32).min(1.0);
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_x_scrollbar()
                        .child(
                            img(image.clone())
                                .w(px((nw as f32 * scale).ceil()))
                                .h(px((nh as f32 * scale).ceil())),
                        )
                        .into_any_element()
                } else {
                    div()
                        .child(
                            img(image.clone())
                                .object_fit(ObjectFit::Contain)
                                .max_w_full()
                                .max_h(rems(16.)),
                        )
                        .into_any_element()
                };
                div()
                    .id("mail-cid-image")
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .child(content)
                    .into_any_element()
            } else {
                div()
                    .id("mail-cid-missing")
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .text_color(theme.text_tertiary)
                    .text_xs()
                    .child(SharedString::from(if alt.is_empty() {
                        "[图片不可用]"
                    } else {
                        alt
                    }))
                    .into_any_element()
            }
        }
        MailBodyBlock::TableRow { cells } => {
            let mut row = div()
                .flex()
                .w_full()
                .min_w(rems(cells.len() as f32 * 6.))
                .border_l_1()
                .border_b_1()
                .border_color(theme.border);
            for (index, cell) in cells.iter().enumerate() {
                row = row.child(
                    div()
                        .id(("mail-table-cell", index))
                        .test_support()
                        .flex_1()
                        .min_w(rems(6.))
                        .px_3()
                        .py_2()
                        .border_r_1()
                        .border_color(theme.border)
                        .when(cell.header, |cell| cell.bg(theme.raised))
                        .when_some(cell.content.as_ref(), |cell, block| {
                            cell.child(render_body_block(theme, block, cids))
                        }),
                );
            }
            div()
                .id("mail-table-row")
                .test_support()
                .w_full()
                .min_w_0()
                .child(row)
                .overflow_x_scrollbar()
                .into_any_element()
        }
    }
}

fn rewrite_cid_sources(html: &str, cids: &HashMap<String, Arc<Image>>) -> String {
    if !html.to_ascii_lowercase().contains("<img") {
        return html.to_owned();
    }
    let lower = html.to_ascii_lowercase();
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(relative_start) = lower[cursor..].find("<img") {
        let start = cursor + relative_start;
        if start > cursor {
            output.push_str(&html[cursor..start]);
        }
        let Some(end) = find_html_tag_end(html, start) else {
            output.push_str(&html[start..]);
            return output;
        };
        let tag = &html[start..=end];
        let src = html_attr(tag, "src");
        let Some(cid) = foxmail_imap::html::cid_from_src(src.as_deref()) else {
            output.push_str(tag);
            cursor = end + 1;
            continue;
        };
        if let Some(image) = cids.get(&cid) {
            if let Some(uri) = image_data_uri(image) {
                output.push_str(&replace_html_attr(tag, "src", &uri));
            } else {
                output.push_str(tag);
            }
        } else {
            let alt = html_attr(tag, "alt").filter(|value| !value.trim().is_empty());
            output.push_str("<span>");
            output.push_str(&escape_html_text(alt.as_deref().unwrap_or("[图片不可用]")));
            output.push_str("</span>");
        }
        cursor = end + 1;
    }
    output.push_str(&html[cursor..]);
    output
}

fn find_html_tag_end(value: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in value[start..].char_indices() {
        match (quote, character) {
            (Some(expected), character) if character == expected => quote = None,
            (None, '"' | '\'') => quote = Some(character),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let marker = format!("{name}=");
    let index = lower.find(&marker)?;
    let rest = tag[index + marker.len()..].trim_start();
    let first = rest.chars().next()?;
    if matches!(first, '"' | '\'') {
        let inner = rest.get(first.len_utf8()..)?;
        let end = inner.find(first).unwrap_or(inner.len());
        Some(inner[..end].to_owned())
    } else {
        Some(
            rest.split(|character: char| character.is_whitespace() || character == '>')
                .next()?
                .to_owned(),
        )
    }
}

fn replace_html_attr(tag: &str, name: &str, value: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let marker = format!("{name}=");
    let Some(index) = lower.find(&marker) else {
        return tag.to_owned();
    };
    let value_start = index + marker.len();
    let rest = &tag[value_start..];
    let rest_trimmed = rest.trim_start();
    let leading = &rest[..rest.len() - rest_trimmed.len()];
    let Some(first) = rest_trimmed.chars().next() else {
        return tag.to_owned();
    };
    if matches!(first, '"' | '\'') {
        let inner = &rest_trimmed[first.len_utf8()..];
        let end = inner.find(first).unwrap_or(inner.len());
        let suffix = if end < inner.len() {
            &inner[end + first.len_utf8()..]
        } else {
            ""
        };
        format!(
            "{}{leading}{first}{value}{first}{suffix}",
            &tag[..value_start]
        )
    } else {
        let end = rest_trimmed
            .find(|character: char| character.is_whitespace() || character == '>')
            .unwrap_or(rest_trimmed.len());
        format!(
            "{}{leading}{value}{}",
            &tag[..value_start],
            &rest_trimmed[end..]
        )
    }
}

fn image_data_uri(image: &Image) -> Option<String> {
    let bytes: &[u8] = image.bytes.as_ref();
    if bytes.len() > MAX_INLINE_CID_BYTES {
        return None;
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", base64_encode(bytes)))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        output.push(TABLE[((a >> 2) & 0x3f) as usize] as char);
        output.push(TABLE[(((a << 4) | (b >> 4)) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b << 2) | (c >> 6)) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn escape_html_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

fn clamp_attachment_pane(height: f32) -> f32 {
    height.clamp(ATTACHMENT_PANE_MIN_REM, ATTACHMENT_PANE_MAX_REM)
}

// 本文件测试专用。
#[allow(dead_code)]
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
        let mut section = div().flex().flex_col().gap_2().py_2();
        if attachment_count == 0 {
            return section;
        }
        if attachments.is_empty() {
            return section.child(
                div()
                    .id("attachment-metadata-unavailable")
                    .test_support()
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
        if let Some(status) = self.render_attachment_save_status(cx) {
            section = section.child(status);
        }
        for (index, attachment) in attachments.iter().enumerate() {
            let filename = nonempty_label(&attachment.filename, "未命名附件");
            let content_type = nonempty_label(&attachment.content_type, "未知类型");
            let metadata = format!(
                "{} · {} · {}",
                if attachment.inline {
                    "内嵌"
                } else {
                    "附件"
                },
                content_type,
                format_attachment_bytes(attachment.size_bytes)
            );
            let message_id = self.selected_message.unwrap_or_default();
            let save_target = sanitized_attachment_filename(&attachment.filename);
            let preview_name = attachment.filename.clone();
            let preview_type = attachment.content_type.clone();
            let card_name = preview_name.clone();
            let card_type = preview_type.clone();
            let icon = match attachment_preview_kind(&attachment.content_type, &attachment.filename)
            {
                AttachmentPreviewKind::Image => MailIcon::Image,
                AttachmentPreviewKind::Pdf => MailIcon::FileText,
                AttachmentPreviewKind::Unsupported => MailIcon::Paperclip,
            };
            section = section.child(
                div()
                    .id(("attachment-card-region", index))
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .child(
                        Attachment::new()
                            .id(("attachment-card", index))
                            .w_full()
                            .small()
                            // Metadata-only render: no file reads, network URI, decoding,
                            // or eager image thumbnails for potentially huge attachments.
                            .media(AttachmentMedia::new().child(Icon::new(icon).size(rems(1.25))))
                            .content(
                                AttachmentContent::new()
                                    .title(AttachmentTitle::new(filename))
                                    .description(AttachmentDescription::new(metadata)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_attachment_preview(
                                    message_id,
                                    index,
                                    card_name.clone(),
                                    card_type.clone(),
                                    window,
                                    cx,
                                );
                            }))
                            .actions(
                                AttachmentActions::new()
                                    .child(
                                        control_button(
                                            ("attachment-preview", index),
                                            theme,
                                            ButtonKind::Ghost,
                                            false,
                                        )
                                        .size(rems(1.75))
                                        .icon(MailIcon::Eye)
                                        .tooltip("预览附件")
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.start_attachment_preview(
                                                    message_id,
                                                    index,
                                                    preview_name.clone(),
                                                    preview_type.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    )
                                    .child(
                                        control_button(
                                            ("attachment-save", index),
                                            theme,
                                            ButtonKind::Ghost,
                                            matches!(self.save_state, Some(SaveState::Saving)),
                                        )
                                        .size(rems(1.75))
                                        .icon(MailIcon::Download)
                                        .tooltip("保存附件")
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.start_attachment_save(
                                                    message_id,
                                                    index,
                                                    save_target.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ),
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
                &account.incoming.host,
                account.incoming.port,
                &account.incoming.user,
                &account.incoming.password,
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
    fn sync_empty_poll_is_not_worker_completion() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (events, terminal) = super::next_sync_batch(&receiver);
        assert!(events.is_empty());
        assert!(!terminal);
        drop(sender);
        let (events, terminal) = super::next_sync_batch(&receiver);
        assert!(terminal);
        assert!(
            matches!(&events[..], [super::backup::ProgressEvent::Failed { message }] if message.contains("未返回完成状态"))
        );
    }

    #[test]
    fn sync_worker_terminal_events_do_not_turn_into_disconnect_failures() {
        use super::backup::ProgressEvent;
        for event in [
            ProgressEvent::Finished {
                backed_up: 1,
                skipped: 2,
            },
            ProgressEvent::Failed {
                message: "offline error".into(),
            },
        ] {
            let (sender, receiver) = std::sync::mpsc::channel();
            sender
                .send(ProgressEvent::Connecting {
                    host: "example.invalid".into(),
                    port: 993,
                })
                .unwrap();
            sender.send(event).unwrap();
            drop(sender);
            let (events, terminal) = super::next_sync_batch(&receiver);
            assert!(terminal);
            assert_eq!(
                events.len(),
                2,
                "do not append a second synthetic failure after a terminal event"
            );
        }
    }

    #[test]
    fn sync_indexing_is_busy_and_never_presented_as_success() {
        let indexing = super::SyncState::Indexing {
            backed_up: 2,
            skipped: 3,
        };
        assert!(indexing.is_busy());
        let (level, message, progress) = indexing.presentation();
        assert_eq!(level, super::NoticeLevel::Info);
        assert!(message.contains("正在建立索引"));
        assert_eq!(progress, None);
        assert!(!super::SyncState::Done {
            backed_up: 2,
            skipped: 3
        }
        .is_busy());
        assert!(!super::SyncState::Failed("offline".into()).is_busy());
    }

    #[test]
    fn sync_presentation_distinguishes_unknown_and_bounded_progress() {
        for (done, total, expected) in [(12, 0, None), (25, 100, Some(25.)), (120, 100, Some(100.))]
        {
            let state = super::SyncState::Message {
                mailbox: "INBOX".into(),
                mailbox_index: 1,
                mailbox_total: 2,
                uid: 7,
                done,
                total,
            };
            let (level, message, value) = state.presentation();
            assert_eq!(level, super::NoticeLevel::Info);
            assert_eq!(value, expected);
            if total == 0 {
                assert!(message.contains("大小未知"));
                assert!(!message.contains("100%"));
            }
        }
    }

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

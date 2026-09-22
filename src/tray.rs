//! 系统托盘：图标 + （打开 / 退出）菜单，事件轮询回灌 gpui 主循环。

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AsyncApp, Global};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_HIDE, SW_RESTORE};

const OPEN_ID: &str = "tray-open";
const QUIT_ID: &str = "tray-quit";

enum Command {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
}

/// Keeps the native tray icon alive for the whole GPUI application lifetime.
///
/// `tray-icon` owns the Windows notification-area handle through `TrayIcon`;
/// dropping it immediately after startup makes the icon disappear and leaves
/// no native event source for the pump below.
pub struct TrayState {
    _tray: Tray,
}

impl Global for TrayState {}

pub fn install(app: &mut App) {
    app.set_global(TrayState { _tray: create() });
}

pub fn create() -> Tray {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(OPEN_ID, "打开 Foxmail Reader", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(QUIT_ID, "退出", true, None),
    ])
    .expect("failed to build tray menu");
    let mut builder = TrayIconBuilder::new()
        .with_tooltip("Foxmail Reader")
        .with_menu(Box::new(menu));
    if let Ok(icon) = tray_icon::Icon::from_path(ico_path(), Some((32, 32))) {
        builder = builder.with_icon(icon);
    }
    Tray {
        _icon: builder.build().expect("failed to create tray icon"),
    }
}

/// 关窗进托盘：隐藏窗口并吞掉本次 WM_CLOSE。
pub fn hide_window(window: &gpui::Window) {
    let Some(hwnd) = crate::save_dialog::native_hwnd(window) else {
        return;
    };
    unsafe {
        let _ = ShowWindowAsync(hwnd, SW_HIDE);
    }
}

/// Restore a window hidden by `hide_window` and bring it to the foreground.
pub fn show_window(window: &gpui::Window) {
    let Some(hwnd) = crate::save_dialog::native_hwnd(window) else {
        window.activate_window();
        return;
    };
    unsafe {
        let _ = ShowWindowAsync(hwnd, SW_RESTORE);
    }
    window.activate_window();
}

/// 托盘左键 / 菜单事件 → 主线程（150ms 轮询，事件频率是人手速，够用）。
pub fn spawn_event_pump(window: AnyWindowHandle, app: &mut App) {
    let menu_rx = MenuEvent::receiver().clone();
    let tray_rx = TrayIconEvent::receiver().clone();
    app.spawn(async move |cx: &mut AsyncApp| loop {
        let menu_rx = menu_rx.clone();
        let tray_rx = tray_rx.clone();
        let command = cx
            .background_executor()
            .spawn(async move {
                let mut command = None;
                while let Ok(event) = menu_rx.try_recv() {
                    if event.id.0 == QUIT_ID {
                        command = Some(Command::Quit);
                    } else if event.id.0 == OPEN_ID {
                        command = Some(Command::Show);
                    }
                }
                while command.is_none() {
                    match tray_rx.try_recv() {
                        Ok(TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }) => command = Some(Command::Show),
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                command
            })
            .await;
        match command {
            Some(Command::Show) => {
                let _ = window.update(cx, |_, window, _| show_window(window));
            }
            Some(Command::Quit) => {
                cx.update(|app| app.quit());
            }
            None => {}
        }
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
    })
    .detach();
}

/// 发布包里没有 .ico 时退回无图标托盘（系统默认方块）。
/// 从 EXE 目录向上有限层查找（覆盖 cargo run 的 target/<profile>/ 布局），
/// 不在二进制中嵌入构建机路径。
fn ico_path() -> PathBuf {
    ico_path_from(&std::env::current_exe().unwrap_or_else(|_| PathBuf::from("xiou.exe")))
}

fn ico_path_from(exe: &Path) -> PathBuf {
    if let Some(dir) = exe.parent() {
        let mut dir = dir.to_path_buf();
        for _ in 0..5 {
            let candidate = dir.join("xiou.ico");
            if candidate.is_file() {
                return candidate;
            }
            if !dir.pop() {
                break;
            }
        }
    }
    exe.with_file_name("xiou.ico")
}

#[test]
fn ico_path_is_runtime_derived() {
    let exe = std::env::current_exe().unwrap();
    let path = ico_path();
    // 派生自运行时 EXE 位置（其祖先目录之一），而非编译期清单路径。
    assert!(path.starts_with(exe.ancestors().nth(5).unwrap()));
    assert!(path.ends_with("xiou.ico"));
}

#[test]
fn ico_path_prefers_nearest_directory_and_falls_back_to_exe_adjacent() {
    let root = std::env::temp_dir().join(format!("xiou-ico-test-{}", std::process::id()));
    let deep = root.join("a").join("b").join("c").join("d").join("e");
    std::fs::create_dir_all(&deep).unwrap();
    let exe = deep.join("app.exe");

    // 无任何 .ico：退回 EXE 同目录路径（由调用方降级为无图标托盘）。
    assert_eq!(ico_path_from(&exe), deep.join("xiou.ico"));

    // 祖先目录有 .ico：取最近的祖先（e–b 层无，a 层有）。
    let ancestor = root.join("a").join("xiou.ico");
    std::fs::write(&ancestor, b"ico").unwrap();
    assert_eq!(ico_path_from(&exe), ancestor);

    // EXE 同目录有 .ico：优先于所有祖先。
    let adjacent = deep.join("xiou.ico");
    std::fs::write(&adjacent, b"ico").unwrap();
    assert_eq!(ico_path_from(&exe), adjacent);

    std::fs::remove_dir_all(&root).ok();
}

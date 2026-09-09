//! 系统托盘：图标 + （打开 / 退出）菜单，事件轮询回灌 gpui 主循环。

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AsyncApp, WindowHandle};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_HIDE};

const OPEN_ID: &str = "tray-open";
const QUIT_ID: &str = "tray-quit";

enum Command {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
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

/// 托盘左键 / 菜单事件 → 主线程（150ms 轮询，事件频率是人手速，够用）。
pub fn spawn_event_pump(window: WindowHandle<crate::app::MailApp>, app: &mut App) {
    let menu_rx = MenuEvent::receiver().clone();
    let tray_rx = TrayIconEvent::receiver().clone();
    app.spawn(async move |cx: &mut AsyncApp| {
        loop {
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
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                }
                Some(Command::Quit) => {
                    let _ = cx.update(|app| app.quit());
                }
                None => {}
            }
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
        }
    })
    .detach();
}

/// 发布包里没有 .ico 时退回无图标托盘（系统默认方块）。
fn ico_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("foxmail-rs.ico");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("foxmail-rs.ico")
}

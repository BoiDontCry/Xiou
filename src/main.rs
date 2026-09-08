#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{px, size, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions};
use gpui_platform::application;
use std::time::Duration;

mod app;
mod compose_window;
mod composer_editor;
mod diagnostics;
mod draft_store;
mod paths;
mod save_dialog;
mod theme;
mod ui;

fn main() {
    application().run(|app: &mut App| {
        app.bind_keys([
            KeyBinding::new("backspace", composer_editor::Backspace, None),
            KeyBinding::new("delete", composer_editor::Delete, None),
            KeyBinding::new("left", composer_editor::Left, None),
            KeyBinding::new("right", composer_editor::Right, None),
            KeyBinding::new("up", composer_editor::Up, None),
            KeyBinding::new("down", composer_editor::Down, None),
            KeyBinding::new("shift-left", composer_editor::SelectLeft, None),
            KeyBinding::new("shift-right", composer_editor::SelectRight, None),
            KeyBinding::new("ctrl-a", composer_editor::SelectAll, None),
            KeyBinding::new("ctrl-v", composer_editor::Paste, None),
            KeyBinding::new("ctrl-c", composer_editor::Copy, None),
            KeyBinding::new("ctrl-x", composer_editor::Cut, None),
            KeyBinding::new("home", composer_editor::Home, None),
            KeyBinding::new("end", composer_editor::End, None),
            KeyBinding::new("enter", composer_editor::Newline, None),
            KeyBinding::new("escape", compose_window::CloseCompose, Some("Compose")),
            KeyBinding::new("ctrl-enter", compose_window::SendCompose, Some("Compose")),
            KeyBinding::new("escape", app::ClosePreview, Some("Preview")),
            KeyBinding::new("escape", app::DismissOverlay, Some("MailApp")),
            KeyBinding::new("ctrl-n", app::Compose, Some("MailApp")),
            KeyBinding::new("ctrl-r", app::Reply, Some("MailApp")),
            KeyBinding::new("ctrl-shift-f", app::Forward, Some("MailApp")),
            KeyBinding::new("ctrl-f", app::OpenSearch, Some("MailApp")),
            KeyBinding::new("ctrl-shift-s", app::SyncNow, Some("MailApp")),
            KeyBinding::new("ctrl-b", app::ToggleSidebar, Some("MailApp")),
            KeyBinding::new("down", app::NextMessage, Some("MessageList")),
            KeyBinding::new("up", app::PrevMessage, Some("MessageList")),
            KeyBinding::new("j", app::NextMessage, Some("MessageList")),
            KeyBinding::new("k", app::PrevMessage, Some("MessageList")),
            KeyBinding::new("enter", app::ConfirmSelection, Some("MessageList")),
            KeyBinding::new("down", app::NextMailbox, Some("MailboxList")),
            KeyBinding::new("up", app::PrevMailbox, Some("MailboxList")),
            KeyBinding::new("j", app::NextMailbox, Some("MailboxList")),
            KeyBinding::new("k", app::PrevMailbox, Some("MailboxList")),
            KeyBinding::new("enter", app::ConfirmSelection, Some("MailboxList")),
        ]);

        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), app);
        app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Foxmail Reader".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |window, app| app.new(|cx| app::MailApp::new(window, cx)),
        )
        .expect("failed to open Foxmail Reader window");

        if let Some(delay) = auto_exit_delay(std::env::var("FOXMAIL_AUTO_EXIT_MS").ok().as_deref())
        {
            app.spawn(async move |app| {
                app.background_executor().timer(delay).await;
                app.update(|app| app.quit());
            })
            .detach();
        }
    });
}

fn auto_exit_delay(value: Option<&str>) -> Option<Duration> {
    const MIN_DELAY_MS: u64 = 1_000;
    const MAX_DELAY_MS: u64 = 300_000;

    let milliseconds = value?.trim().parse::<u64>().ok()?;
    (MIN_DELAY_MS..=MAX_DELAY_MS)
        .contains(&milliseconds)
        .then(|| Duration::from_millis(milliseconds))
}

#[cfg(test)]
mod tests {
    use super::auto_exit_delay;
    use std::time::Duration;

    #[test]
    fn accepts_only_bounded_auto_exit_delays() {
        assert_eq!(auto_exit_delay(Some("1000")), Some(Duration::from_secs(1)));
        assert_eq!(
            auto_exit_delay(Some(" 300000 ")),
            Some(Duration::from_secs(300))
        );
        assert_eq!(auto_exit_delay(None), None);
        assert_eq!(auto_exit_delay(Some("invalid")), None);
        assert_eq!(auto_exit_delay(Some("999")), None);
        assert_eq!(auto_exit_delay(Some("300001")), None);
    }
}

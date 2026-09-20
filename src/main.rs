#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{px, size, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions};
use std::time::Duration;

mod app;
mod compose_window;
mod diagnostics;
mod draft_store;
mod feedback;
mod fox_theme;
mod mail_body;
mod navigation;
mod paths;
mod prefs;
mod save_dialog;
mod single_instance;
mod theme;
mod tray;
mod ui;

fn main() {
    let _single_instance = match single_instance::acquire() {
        single_instance::AcquireResult::Acquired(guard) => guard,
        single_instance::AcquireResult::AlreadyRunning => return,
        single_instance::AcquireResult::Unavailable => {
            eprintln!("single-instance guard unavailable; continuing");
            single_instance::SingleInstance::noop()
        }
    };

    ui::application().run(|app: &mut App| {
        gpui_kit::init(app);
        app.bind_keys(app::search_key_bindings());
        app.bind_keys(app::navigation_key_bindings());
        app.bind_keys([
            KeyBinding::new("escape", compose_window::CloseCompose, Some("Compose")),
            KeyBinding::new("ctrl-enter", compose_window::SendCompose, Some("Compose")),
            compose_window::input_send_binding(),
            KeyBinding::new("escape", app::DismissOverlay, Some("MailApp")),
            KeyBinding::new("ctrl-n", app::Compose, Some("MailApp")),
            KeyBinding::new("ctrl-r", app::Reply, Some("MailApp")),
            KeyBinding::new("ctrl-shift-f", app::Forward, Some("MailApp")),
            KeyBinding::new("ctrl-f", app::OpenSearch, Some("MailApp")),
            KeyBinding::new("ctrl-shift-s", app::SyncNow, Some("MailApp")),
            KeyBinding::new("ctrl-,", app::OpenSettings, Some("MailApp")),
        ]);

        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), app);
        let window = app
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Xiou".into()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                |window, app| {
                    let view = app.new(|cx| app::MailApp::new(window, cx));
                    ui::root_view(view, window, app)
                },
            )
            .expect("failed to open Xiou window");

        tray::install(app);
        tray::spawn_event_pump(window.into(), app);

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

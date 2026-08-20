#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_platform::application;
use std::time::Duration;

mod app;
mod compose_window;
mod composer_editor;
mod diagnostics;
mod paths;
mod theme;

fn main() {
    application().run(|app: &mut App| {
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
            |_window, app| app.new(app::MailApp::new),
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

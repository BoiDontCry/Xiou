#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_platform::application;

mod app;
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
    });
}

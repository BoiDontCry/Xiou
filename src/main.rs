use gpui::{App, AppContext, WindowOptions};
use gpui_platform::application;

mod actions {
    use gpui::actions;
    actions!(foxmail_rs, [Quit]);
}

mod theme;
mod app;

fn main() {
    application().run(|app: &mut App| {
        app::init(app);
        app.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("foxmail-rs".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, app| app.new(|cx| app::MailApp::new(cx)),
        )
        .unwrap();
    });
}

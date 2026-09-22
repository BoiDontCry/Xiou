//! Native state/event regressions against the production modules, not mock inputs.
//! See gpui_migration.md for isolation and the limits of these checks.
// Included production modules also contain unit-test-only imports.
#![allow(dead_code, unused_imports)]

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_kit::component::theme::Theme as KitTheme;
use serde_json::{json, Value};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod compose_window {
    include!("../src/compose_window.rs");
    include!("support/gpui_send_feedback.rs");

    use gpui::{AnyWindowHandle, AsyncApp, Keystroke};
    use gpui_kit::component::{notification::Notification, Root, WindowExt};
    use serde_json::{json, Value};
    use std::time::Duration;

    fn open_hidden(cx: &mut App) -> (AnyWindowHandle, Entity<ComposeWindow>) {
        let mut view = None;
        let bounds = Bounds::centered(None, size(px(720.), px(560.)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    // Reuse exactly the constructor called by ComposeWindow::open.
                    let entity = cx.new(|cx| ComposeWindow::new(ComposeKind::New, window, cx));
                    view = Some(entity.clone());
                    crate::ui::root_view(entity, window, cx)
                },
            )
            .unwrap();
        (handle.into(), view.unwrap())
    }

    pub async fn check_window(cx: &mut AsyncApp) -> Vec<Value> {
        let mut checks = Vec::new();
        let (handle, view) = cx.update(open_hidden);
        handle
            .update(cx, |_, window, _| window.resize(size(px(720.), px(560.))))
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
        handle
            .update(cx, |_, window, cx| {
                assert!(window.root::<Root>().flatten().is_some());
                window.draw(cx).clear(cx);
                assert!(view
                    .read(cx)
                    .to
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window));
            })
            .unwrap();
        checks.push(json!({"check": "composer uses official Root and retains initial To focus"}));

        for field in [0, 1, 2] {
            handle
                .update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    assert!(window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx));
                })
                .unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
            handle
                .update(cx, |_, window, cx| {
                    let view = view.read(cx);
                    let focus = match field {
                        0 => view.cc.read(cx).focus_handle(cx),
                        1 => view.subject.read(cx).focus_handle(cx),
                        _ => view.body.read(cx).focus_handle(cx),
                    };
                    assert!(
                        focus.is_focused(window),
                        "composer Tab order changed at {field}"
                    );
                })
                .unwrap();
        }
        checks.push(json!({"check": "composer simulated Tab follows To, Cc, Subject, Body"}));

        handle
            .update(cx, |_, window, cx| {
                let body = view.read(cx).body.clone();
                body.update(cx, |input, cx| input.set_value("fixture line", window, cx));
                window.draw(cx).clear(cx);
                assert!(window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx));
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        let body_value = handle
            .update(cx, |_, _, cx| {
                let view = view.read(cx);
                assert!(
                    matches!(view.send_state, SendState::Idle),
                    "plain Enter attempted to send"
                );
                // Recipients stay empty throughout the fixture: even a regression
                // must never reach credential loading or the network send path.
                assert!(view.to.read(cx).value().is_empty());
                let body = view.body.read(cx).value().to_string();
                assert!(body.contains("fixture line"));
                assert_eq!(body.matches('\n').count(), 1);
                let draft = draft_store::load(&draft_key(&view.kind)).unwrap().unwrap();
                assert_eq!(
                    draft.body, body,
                    "the real Change subscription must persist the draft"
                );
                body
            })
            .unwrap();
        checks.push(json!({"check": "composer Enter inserts a newline, keeps send idle, and saves the draft"}));

        handle
            .update(cx, |_, window, cx| {
                window.open_dialog(cx, |dialog, _, _| {
                    dialog.title("Compose fixture").child("Dialog content")
                });
                assert!(window.has_active_dialog(cx));
                window.draw(cx).clear(cx);
                window.close_dialog(cx);
                window.push_notification(
                    Notification::info("Compose notification").autohide(false),
                    cx,
                );
                assert_eq!(window.notifications(cx).len(), 1);
                window.draw(cx).clear(cx);
                window.clear_notifications(cx);
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        handle
            .update(cx, |_, window, cx| {
                assert!(!window.has_active_dialog(cx));
                assert!(window.notifications(cx).is_empty());
                view.update(cx, |view, cx| view.close_window(&CloseCompose, window, cx));
            })
            .unwrap();
        checks.push(
            json!({"check": "composer Dialog/Notification layers render and the window closes"}),
        );

        let (restored_handle, restored) = cx.update(open_hidden);
        restored_handle
            .update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                assert_eq!(
                    restored.read(cx).body.read(cx).value().as_ref(),
                    body_value.as_str()
                );
                assert!(matches!(restored.read(cx).send_state, SendState::Idle));
                window.remove_window();
            })
            .unwrap();
        checks.push(json!({"check": "reopening the composer restores the isolated saved draft"}));
        checks
    }

    pub struct ThemeProbe {
        handle: AnyWindowHandle,
        view: Entity<ComposeWindow>,
    }

    impl ThemeProbe {
        pub fn open(cx: &mut App) -> Self {
            let (handle, view) = open_hidden(cx);
            handle
                .update(cx, |_, window, _| window.resize(size(px(720.), px(560.))))
                .unwrap();
            Self { handle, view }
        }

        pub fn capture_refreshed(&self, name: &str, expected: Theme, cx: &mut AsyncApp) -> Value {
            self.handle.update(cx, |_, window, cx| {
                // Check before forcing a draw: the already-open composer must
                // refresh via the real settings/theme notification path.
                let theme = self.view.read(cx).theme;
                assert_eq!(theme.canvas, expected.canvas, "{name}: composer canvas refresh");
                assert_eq!(theme.surface, expected.surface, "{name}: composer surface refresh");
                assert_eq!(theme.text, expected.text, "{name}: composer text refresh");
                assert_eq!(theme.accent, expected.accent, "{name}: composer accent refresh");
                window.draw(cx).clear(cx);
                let root = std::path::PathBuf::from(std::env::var_os("FOXMAIL_INDEX_PATH").unwrap());
                let directory = root.parent().unwrap().join("screenshots");
                std::fs::create_dir_all(&directory).unwrap();
                let path = directory.join(format!("compose-{name}.png"));
                let image = window.render_to_image().unwrap();
                assert!(image.width() >= 640 && image.height() >= 400);
                // Inspect the actual send-button pixels as well as stored theme
                // fields. A stale resolved-token layer used to pass the latter.
                let button = gpui_kit::base::test_support::find(window, &[], &"send-button".into())
                    .expect("real composer send button").bounds();
                let scale = window.scale_factor();
                let left = (f32::from(button.origin.x) * scale).ceil() as u32;
                let top = (f32::from(button.origin.y) * scale).ceil() as u32;
                let right = ((f32::from(button.right()) * scale).floor() as u32).min(image.width());
                let bottom = ((f32::from(button.bottom()) * scale).floor() as u32).min(image.height());
                let color: gpui::Rgba = expected.accent.into();
                let expected_rgb = [color.r, color.g, color.b].map(|channel| (channel * 255.).round() as u8);
                let mut primary_pixels = 0;
                for y in top..bottom {
                    for x in left..right {
                        let pixel = image.get_pixel(x, y).0;
                        if (0..3).all(|i| pixel[i].abs_diff(expected_rgb[i]) <= 3) { primary_pixels += 1; }
                    }
                }
                assert!(right > left && bottom > top, "visible send-button region");
                assert!(primary_pixels > (right - left) * (bottom - top) / 3,
                    "{name}: composer send button did not render the saved primary color ({primary_pixels} pixels)");
                image.save(&path).unwrap();
                json!({"check": "already-open composer follows actual settings save", "theme": name,
                    "path": path, "evidence": "cross-window-state-and-native-gpui-offscreen-screenshot", "visually_reviewed": false})
            }).unwrap()
        }

        pub fn close(self, cx: &mut AsyncApp) {
            self.handle
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
#[path = "../src/diagnostics.rs"]
mod diagnostics;
#[path = "../src/draft_store.rs"]
mod draft_store;
#[path = "../src/feedback.rs"]
mod feedback;
#[path = "../src/fox_theme.rs"]
mod fox_theme;
#[path = "../src/mail_body.rs"]
mod mail_body;
#[path = "../src/navigation.rs"]
mod navigation;
#[path = "../src/paths.rs"]
mod paths;
#[path = "../src/prefs.rs"]
mod prefs;
#[path = "../src/save_dialog.rs"]
mod save_dialog;
#[path = "../src/theme.rs"]
mod theme;
#[path = "../src/tray.rs"]
mod tray;
#[path = "../src/ui.rs"]
mod ui;

// Keep access to private application actions without adding test hooks to the
// production API. The included code is the exact source compiled by foxmail-rs.
mod app {
    include!("../src/app.rs");

    use gpui::{AnyWindowHandle, AsyncApp, Entity, Keystroke};
    use gpui_kit::component::input::InputEvent;
    use gpui_kit::component::{notification::Notification, Root, WindowExt};
    use serde_json::{json, Value};
    use std::{cell::Cell, rc::Rc};

    pub struct TestWindow {
        pub handle: AnyWindowHandle,
        pub view: Entity<MailApp>,
    }

    fn update_window<R>(
        fixture: &TestWindow,
        cx: &mut AsyncApp,
        f: impl FnOnce(&mut Window, &mut gpui::App) -> R,
    ) -> R {
        // Do not mutably borrow Root while exercising its WindowExt services.
        cx.update(|cx| {
            fixture
                .handle
                .update(cx, |_, window, cx| f(window, cx))
                .expect("regression window is alive")
        })
    }

    fn update<R>(
        fixture: &TestWindow,
        cx: &mut AsyncApp,
        f: impl FnOnce(&mut MailApp, &mut Window, &mut Context<MailApp>) -> R,
    ) -> R {
        update_window(fixture, cx, |window, cx| {
            fixture.view.update(cx, |view, cx| f(view, window, cx))
        })
    }

    async fn settle(cx: &mut AsyncApp) {
        // Let GPUI flush subscriptions and notifications before checking for
        // delayed feedback. This is not a native focus/IME acceptance test.
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }

    pub async fn check_window_services(window: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
        // Windows defers applying initial placement when show=false; otherwise
        // the platform can retain CW_USEDEFAULT dimensions and a 1x1 texture.
        // Resize the hidden window explicitly, without showing/focusing it.
        update_window(window, cx, |window, _| {
            window.resize(gpui::size(px(1180.), px(760.)))
        });
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
        update_window(window, cx, |window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.viewport_size().width > px(1000.));
            assert!(window.viewport_size().height > px(600.));
        });
        let mut checks = Vec::new();
        update_window(window, cx, |window, cx| {
            assert!(window.root::<Root>().flatten().is_some());
            assert!(cx.asset_source().load("icons/check.svg").unwrap().is_some());
            window.draw(cx).clear(cx);
        });
        checks.push(json!({"check": "official Root and component SVG assets are installed"}));

        update(window, cx, |view, window, cx| {
            let host = view
                .onboarding
                .as_ref()
                .unwrap()
                .host
                .read(cx)
                .focus_handle(cx);
            window.focus(&host, cx);
        });
        update_window(window, cx, |window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx));
        });
        settle(cx).await;
        update(window, cx, |view, window, cx| {
            assert!(view
                .onboarding
                .as_ref()
                .unwrap()
                .port
                .read(cx)
                .focus_handle(cx)
                .is_focused(window));
        });
        update_window(window, cx, |window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.dispatch_keystroke(Keystroke::parse("shift-tab").unwrap(), cx));
        });
        settle(cx).await;
        update(window, cx, |view, window, cx| {
            assert!(view
                .onboarding
                .as_ref()
                .unwrap()
                .host
                .read(cx)
                .focus_handle(cx)
                .is_focused(window));
        });
        checks.push(
            json!({"check": "Root routes simulated Tab and Shift-Tab between onboarding inputs"}),
        );

        update_window(window, cx, |window, cx| {
            window.open_dialog(cx, |dialog, _, _| {
                dialog.title("Fixture dialog").child("Fixture content")
            });
            assert!(window.has_active_dialog(cx));
            window.draw(cx).clear(cx);
            window.close_dialog(cx);
            assert!(!window.has_active_dialog(cx));
        });
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        checks.push(json!({"check": "official Dialog opens, renders and closes on onboarding"}));

        update_window(window, cx, |window, cx| {
            window.open_sheet(cx, |sheet, _, _| {
                sheet.title("Fixture sheet").child("Fixture content")
            });
            assert!(window.has_active_sheet(cx));
            window.draw(cx).clear(cx);
            window.close_sheet(cx);
            assert!(!window.has_active_sheet(cx));
        });
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        checks.push(json!({"check": "official Sheet opens, renders and closes on onboarding"}));

        update_window(window, cx, |window, cx| {
            window.push_notification(
                Notification::info("Fixture notification").autohide(false),
                cx,
            );
            assert_eq!(window.notifications(cx).len(), 1);
            window.draw(cx).clear(cx);
            window.clear_notifications(cx);
        });
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        update_window(window, cx, |window, cx| {
            assert!(window.notifications(cx).is_empty())
        });
        checks.push(json!({"check": "official in-app Notification renders and clears"}));
        checks
    }

    fn watch_changes(
        input: &InputEntity,
        cx: &mut Context<MailApp>,
    ) -> (Rc<Cell<usize>>, gpui::Subscription) {
        let count = Rc::new(Cell::new(0));
        let observer = count.clone();
        let subscription = cx.subscribe(input, move |_, _, event: &InputEvent, _| {
            if matches!(event, InputEvent::Change) {
                observer.set(observer.get() + 1);
            }
        });
        (count, subscription)
    }

    fn seed_search(view: &mut MailApp, window: &mut Window, cx: &mut Context<MailApp>) {
        view.search_input
            .update(cx, |input, cx| input.set_value("fixture-query", window, cx));
        view.search_query = "fixture-query".into();
        view.search_open = true;
        view.search_pending = true;
        view.search_error = Some("stale fixture error".into());
        view.search_selection = 3;
        view.search_results = vec![SearchHit {
            id: 1,
            mailbox_id: 42,
            mailbox: "INBOX".into(),
            uid: 1,
            subject: "fixture".into(),
            sender: "fixture@example.invalid".into(),
            recipients: String::new(),
            date_text: None,
            snippet: String::new(),
            eml_path: String::new(),
        }];
    }

    fn assert_search_cleared(view: &MailApp, open: bool, cx: &Context<MailApp>) {
        assert!(
            view.search_input.read(cx).value().is_empty(),
            "visible search value"
        );
        assert!(view.search_query.is_empty(), "search model");
        assert!(view.search_results.is_empty());
        assert!(view.search_error.is_none());
        assert!(!view.search_pending);
        assert_eq!(view.search_selection, 0);
        assert_eq!(
            view.search_open, open,
            "programmatic sync changed search intent"
        );
    }

    fn assert_hex_synced(view: &MailApp, expected: &Prefs, cx: &Context<MailApp>) -> String {
        let session = view.settings.as_ref().expect("settings remain open");
        assert_eq!(
            &session.draft, expected,
            "sync must not write back into the draft"
        );
        let hex = format_hex(settings_token_color(
            expected,
            session.editing_dark,
            session.token,
        ));
        assert_eq!(session.hex_input.read(cx).value().as_ref(), hex.as_str());
        hex
    }

    pub async fn check_inputs(window: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
        let mut checks = Vec::new();
        let (search_changes, _search_events) = update(window, cx, |view, _, cx| {
            view.onboarding = None;
            assert!(
                view.store.is_none(),
                "the fixture must not load a real index"
            );
            assert!(view.accounts.is_empty());
            view.prefs = Prefs {
                scheme: ColorScheme::Dark,
                palette: Palette::Custom,
                ..Prefs::default()
            };
            watch_changes(&view.search_input, cx)
        });

        type SearchAction = fn(&mut MailApp, &mut Window, &mut Context<MailApp>);
        let actions: [(&str, bool, SearchAction); 5] = [
            (
                "clear search retains open intent",
                true,
                MailApp::clear_search,
            ),
            (
                "dismiss search stays closed",
                false,
                MailApp::dismiss_search,
            ),
            (
                "mailbox selection clears search",
                false,
                |view, window, cx| view.select_mailbox(42, true, window, cx),
            ),
            (
                "account toggle dismisses search",
                false,
                |view, window, cx| view.toggle_account("fixture-account".into(), window, cx),
            ),
            (
                "sidebar toggle dismisses search",
                false,
                MailApp::toggle_sidebar,
            ),
        ];
        for (name, open, action) in actions {
            let generation = update(window, cx, |view, window, cx| {
                seed_search(view, window, cx);
                let generation = view.search_generation;
                action(view, window, cx);
                assert_search_cleared(view, open, cx);
                generation
            });
            settle(cx).await;
            update(window, cx, |view, _, cx| {
                assert_search_cleared(view, open, cx);
                assert_eq!(view.search_generation, generation.wrapping_add(1));
            });
            assert_eq!(search_changes.get(), 0, "set_value must remain silent");
            checks.push(json!({"check": name, "input": "", "model": "", "open": open}));
        }

        update(window, cx, |view, window, cx| {
            view.search_input
                .update(cx, |input, cx| input.replace_all("event-query", window, cx));
        });
        settle(cx).await;
        update(window, cx, |view, _, cx| {
            assert_eq!(view.search_query, "event-query");
            assert!(view.search_open);
            // Exercise the actual subscribe_in route, without claiming this
            // synthetic Blur establishes native Windows focus behavior.
            view.search_input
                .update(cx, |_, cx| cx.emit(InputEvent::Blur));
        });
        assert!(
            search_changes.get() > 0,
            "editable replacement must emit Change"
        );
        let changes_before_blur = search_changes.get();
        settle(cx).await;
        update(window, cx, |view, _, cx| {
            assert_search_cleared(view, false, cx)
        });
        assert_eq!(search_changes.get(), changes_before_blur);
        checks.push(json!({"check": "Change and synthetic Blur subscription routing"}));

        update(window, cx, |view, window, cx| {
            view.search_input
                .update(cx, |input, cx| input.replace_all("new-query", window, cx));
        });
        settle(cx).await;
        update(window, cx, |view, window, cx| {
            assert_eq!(view.search_query, "new-query");
            assert!(view.search_open, "a subsequent edit must still open search");
            view.dismiss_search(window, cx);
        });
        settle(cx).await;
        update(window, cx, |view, _, cx| {
            assert_search_cleared(view, false, cx)
        });
        checks.push(json!({"check": "editing after dismissal still works"}));

        let (expected, hex_changes, _hex_events) = update(window, cx, |view, window, cx| {
            view.open_settings(window, cx);
            let session = view.settings.as_ref().unwrap();
            let expected = session.draft.clone();
            let (count, subscription) = watch_changes(&session.hex_input, cx);
            (expected, count, subscription)
        });
        update_window(window, cx, |window, cx| window.draw(cx).clear(cx));
        checks.push(json!({"check": "settings Input, Switch and segmented TabBar render through the real app"}));
        update(window, cx, |view, window, cx| {
            let focus = view
                .settings
                .as_ref()
                .unwrap()
                .hex_input
                .read(cx)
                .focus_handle(cx);
            window.focus(&focus, cx);
        });
        update_window(window, cx, |window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx));
        });
        settle(cx).await;
        update(window, cx, |view, window, cx| {
            assert_hex_synced(view, &expected, cx);
            assert!(!view
                .settings
                .as_ref()
                .unwrap()
                .hex_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window));
        });
        checks.push(
            json!({"check": "simulated Tab leaves HEX without closing or discarding settings"}),
        );

        for dark in [true, false, true] {
            update(window, cx, |view, window, cx| {
                view.set_settings_editing_dark(dark, window, cx)
            });
            for token in TokenId::ALL {
                update(window, cx, |view, window, cx| {
                    view.set_settings_token(token, window, cx);
                    assert_hex_synced(view, &expected, cx);
                });
                settle(cx).await;
                let hex = update(window, cx, |view, _, cx| {
                    let session = view.settings.as_ref().unwrap();
                    assert_eq!(session.editing_dark, dark);
                    assert_eq!(session.token, token);
                    assert_hex_synced(view, &expected, cx)
                });
                assert_eq!(hex_changes.get(), 0, "token/mode sync must remain silent");
                checks.push(json!({"check": "HEX token/mode sync", "dark": dark, "token": token.label(), "hex": hex}));
            }
        }

        update(window, cx, |view, window, cx| {
            view.set_settings_token(TokenId::Primary, window, cx)
        });
        for (channel, t) in [(0, 0.6), (1, 0.42), (2, 0.81)] {
            let expected = update(window, cx, |view, window, cx| {
                let session = view.settings.as_ref().unwrap();
                let mut expected = session.draft.clone();
                let (mut h, mut s, mut v) = rgb_to_hsv(expected.custom_dark.primary);
                match channel {
                    0 => h = t * 360.0,
                    1 => s = t,
                    _ => v = t,
                }
                expected.custom_dark.primary = hsv_to_rgb(h, s, v);
                view.set_settings_hsv(channel, t, window, cx);
                assert_hex_synced(view, &expected, cx);
                expected
            });
            settle(cx).await;
            let hex = update(window, cx, |view, _, cx| {
                assert_hex_synced(view, &expected, cx)
            });
            assert_eq!(hex_changes.get(), 0, "HSV sync must remain silent");
            checks.push(
                json!({"check": "HSV updates draft and HEX", "channel": channel, "hex": hex}),
            );
        }

        let expected = update(window, cx, |view, window, cx| {
            let session = view.settings.as_ref().unwrap();
            let mut expected = session.draft.clone();
            expected.custom_dark.primary = 0x123ABC;
            session
                .hex_input
                .update(cx, |input, cx| input.replace_all("#123ABC", window, cx));
            expected
        });
        settle(cx).await;
        update(window, cx, |view, _, cx| {
            assert_hex_synced(view, &expected, cx);
        });
        assert!(hex_changes.get() > 0);
        checks.push(json!({"check": "HEX edits still update the selected draft token"}));

        update(window, cx, |view, window, cx| {
            let session = view.settings.as_ref().unwrap();
            session
                .hex_input
                .update(cx, |input, cx| input.replace_all("not-a-color", window, cx));
        });
        settle(cx).await;
        update(window, cx, |view, _, cx| {
            let session = view.settings.as_ref().unwrap();
            assert_eq!(
                session.draft, expected,
                "invalid HEX must not damage the draft"
            );
            assert_eq!(session.hex_input.read(cx).value().as_ref(), "not-a-color");
        });
        checks.push(json!({"check": "invalid HEX preserves the draft"}));

        update(window, cx, |view, _, cx| {
            view.settings
                .as_ref()
                .unwrap()
                .hex_input
                .update(cx, |_, cx| cx.emit(InputEvent::Blur));
        });
        settle(cx).await;
        update(window, cx, |view, _, _| {
            assert_eq!(
                view.settings
                    .as_ref()
                    .expect("Blur is not cancellation")
                    .draft,
                expected
            );
        });
        checks.push(json!({"check": "HEX Blur preserves the open settings draft"}));

        for id in ["sidebar-mono", "hide-avatar", "close-tray"] {
            update(window, cx, |view, _, cx| {
                let original = view.settings.as_ref().unwrap().draft.clone();
                view.set_settings_flag(id, true, cx);
                view.set_settings_flag(id, true, cx);
                let current = &view.settings.as_ref().unwrap().draft;
                let mut expected = original.clone();
                match id {
                    "sidebar-mono" => expected.sidebar_mono_icons = true,
                    "hide-avatar" => expected.hide_sender_avatar = true,
                    _ => expected.close_to_tray = true,
                }
                assert_eq!(
                    current, &expected,
                    "Switch uses requested values, not blind toggles"
                );
                view.set_settings_flag(id, false, cx);
                assert_eq!(&view.settings.as_ref().unwrap().draft, &original);
            });
            checks.push(json!({"check": "controlled Switch updates only its requested preference", "id": id}));
        }
        checks
    }

    include!("support/gpui_overlays.rs");
    include!("support/gpui_visuals.rs");
    include!("support/gpui_feedback.rs");
    include!("support/gpui_navigation.rs");
    include!("support/gpui_mail_content.rs");
    include!("support/gpui_acceptance.rs");
}

fn check_themes(cx: &mut App) -> Vec<Value> {
    let mut custom = prefs::Prefs {
        palette: prefs::Palette::Custom,
        ..prefs::Prefs::default()
    };
    custom.custom_dark.primary = 0x4ABCA0;
    custom.custom_light.primary = 0x804AD0;
    custom.custom_dark.canvas = 0x101820;
    custom.custom_light.canvas = 0xFFF5E1;
    custom.scheme = prefs::ColorScheme::Dark;
    let custom_dark = theme::Theme::from_prefs(&custom);
    custom.scheme = prefs::ColorScheme::Light;
    let custom_light = theme::Theme::from_prefs(&custom);
    let cases = [
        ("dark", theme::Theme::dark()),
        ("light", theme::Theme::light()),
        ("custom-dark", custom_dark),
        ("custom-light", custom_light),
    ];
    cases
        .into_iter()
        .map(|(name, theme)| {
            // No test-side sync_base: only the production injection may synchronize.
            fox_theme::apply_foxmail_theme(theme, cx);
            let kit = KitTheme::global(cx);
            let base = gpui_kit::base::Theme::global(cx);
            assert_eq!(kit.primary, theme.accent, "{name}: Kit primary");
            assert_eq!(kit.button_primary, theme.accent, "{name}: Kit button");
            assert_eq!(
                kit.tokens.button_primary.color, theme.accent,
                "{name}: resolved Button color"
            );
            assert_eq!(
                kit.tokens.button_primary.background,
                theme.accent.into(),
                "{name}: rendered Button fill"
            );
            assert_eq!(
                kit.tokens.popover.background,
                theme.raised.into(),
                "{name}: rendered Dialog fill"
            );
            assert_eq!(
                kit.tokens.list_active.background,
                theme.active.into(),
                "{name}: rendered List selection"
            );
            assert_eq!(
                kit.tokens.progress_bar.background,
                theme.accent.into(),
                "{name}: rendered Progress fill"
            );
            assert_eq!(
                kit.tokens.slider_bar.background,
                theme.accent.into(),
                "{name}: rendered Slider fill"
            );
            assert_eq!(
                base.tokens.colors.primary, theme.accent,
                "{name}: Base primary"
            );
            assert_eq!(
                base.tokens.colors.background, theme.canvas,
                "{name}: Base background"
            );
            assert_eq!(
                base.tokens.colors,
                kit.semantic_tokens().colors,
                "{name}: all mirrored colors"
            );
            assert_eq!(kit.is_dark(), theme.canvas.l <= 0.5);
            assert_eq!(
                base.appearance == gpui_kit::base::ThemeAppearance::Dark,
                kit.is_dark()
            );
            json!({"check": "runtime Kit/Base color sync", "mode": name})
        })
        .collect()
}

fn isolated_profile() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "foxmail-gpui-migration-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir(&root).expect("create a fresh isolated profile");
    for dir in ["local", "roaming", "temp", "backup"] {
        std::fs::create_dir(root.join(dir)).unwrap();
    }
    // This harness has its own main and sets environment before starting any
    // GPUI threads. It never shares a process/profile with other Rust tests.
    for (key, dir) in [
        ("LOCALAPPDATA", "local"),
        ("APPDATA", "roaming"),
        ("TEMP", "temp"),
        ("TMP", "temp"),
        ("FOXMAIL_BACKUP_DIR", "backup"),
    ] {
        std::env::set_var(key, root.join(dir));
    }
    std::env::set_var("FOXMAIL_INDEX_PATH", root.join("mail-index.sqlite"));
    std::env::remove_var("FOXMAIL_IMAP_PASSWORD");
    std::env::set_current_dir(&root).unwrap();
    root
}

fn main() {
    // Cargo probes harnesses with --list; do not launch a native app for discovery.
    if std::env::args().any(|arg| arg == "--list") {
        println!("gpui_migration: native state/event checks");
        return;
    }
    let args: Vec<_> = std::env::args().collect();
    assert!(
        args.last().map(String::as_str) != Some("--suite"),
        "--suite needs all, E6, E7 or E8"
    );
    let suite = args
        .windows(2)
        .find(|pair| pair[0] == "--suite")
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| "all".into());
    assert!(
        matches!(suite.as_str(), "all" | "E6" | "E7" | "E8"),
        "unknown native suite: {suite}"
    );
    let profile = isolated_profile();
    let complete = Rc::new(Cell::new(false));
    let completed = complete.clone();
    ui::application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            cx.bind_keys(app::search_key_bindings());
            cx.bind_keys(app::navigation_key_bindings());
            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let mut view = None;
            let window = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| {
                        let entity = cx.new(|cx| app::MailApp::new(window, cx));
                        view = Some(entity.clone());
                        ui::root_view(entity, window, cx)
                    },
                )
                .unwrap();
            let fixture = app::TestWindow {
                handle: window.into(),
                view: view.unwrap(),
            };
            cx.spawn(async move |cx| {
                let mut checks = if suite == "E6" {
                    // A focused run owns its MIME/SQLite fixtures; it must not
                    // depend on state left by overlays, feedback or composer tests.
                    app::seed_overlay_mail(&fixture, cx);
                    app::check_navigation(&fixture, cx).await
                } else if suite == "E8" {
                    app::seed_overlay_mail(&fixture, cx);
                    app::check_visual_acceptance(&fixture, cx).await
                } else if suite == "E7" {
                    app::check_mail_content(&fixture, cx).await
                } else {
                    let mut checks = app::check_window_services(&fixture, cx).await;
                    checks.extend(app::check_inputs(&fixture, cx).await);
                    checks.extend(app::check_business_overlays(&fixture, cx).await);
                    checks.extend(app::check_search_and_preview(&fixture, cx).await);
                    checks.extend(app::capture_visual_baseline(&fixture, cx).await);
                    checks.extend(compose_window::check_window(cx).await);
                    checks.extend(app::check_business_feedback(&fixture, cx).await);
                    checks.extend(compose_window::check_send_feedback(cx).await);
                    checks.extend(app::check_navigation(&fixture, cx).await);
                    checks.extend(app::check_mail_content(&fixture, cx).await);
                    checks.extend(app::check_visual_acceptance(&fixture, cx).await);
                    checks
                };
                checks.extend(cx.update(check_themes));
                app::close_fixture(&fixture, cx).await;
                drop(fixture);
                cx.update(|_| {}); // Flush dropped view handles before quitting the App.
                let report_path = profile.join("result.json");
                let screenshot_cases = checks
                    .iter()
                    .filter(|record| {
                        record["evidence"]
                            .as_str()
                            .unwrap_or("")
                            .contains("offscreen-screenshot")
                    })
                    .count();
                let report = json!({
                    "suite": suite,
                    "passed": checks.len(),
                    "state_event_layout_checks": checks.len() - screenshot_cases,
                    "offscreen_screenshot_cases": screenshot_cases,
                    "visual_review_complete": false,
                    "real_account_profile_used": false,
                    "network_transports_used": false,
                    "e5_checks": checks.iter().filter(|record| record["batch"].as_str() == Some("E5")).count(),
                    "e6_checks": checks.iter().filter(|record| record["batch"].as_str() == Some("E6")).count(),
                    "e7_checks": checks.iter().filter(|record| record["batch"].as_str() == Some("E7")).count(),
                    "e8_checks": checks.iter().filter(|record| record["batch"].as_str() == Some("E8")).count(),
                    "checks": checks,
                    "isolated_profile": profile,
                    "native_ime_or_focus_acceptance": false,
                });
                std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
                println!(
                    "PASS {} GPUI migration checks; report: {}",
                    report["passed"],
                    report_path.display()
                );
                completed.set(true);
                cx.update(|cx| cx.quit());
            })
            .detach();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(300))
                    .await;
                eprintln!("GPUI migration regression timed out");
                std::process::exit(1);
            })
            .detach();
        });
    assert!(
        complete.get(),
        "GPUI exited before regression checks completed"
    );
}

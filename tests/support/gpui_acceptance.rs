// E8: rendered icon resources, row geometry and cross-window theme evidence.
// These are native offscreen renders and synthetic layout checks, NOT OS IME.

fn e8_record(check: &str, evidence: &str) -> Value {
    let mut record = overlay_evidence(check, evidence);
    record["batch"] = json!("E8");
    record
}

fn assert_inside(
    inner: gpui::Bounds<gpui::Pixels>,
    outer: gpui::Bounds<gpui::Pixels>,
    label: &str,
) {
    let tolerance = px(1.5);
    assert!(
        inner.origin.x >= outer.origin.x - tolerance,
        "{label}: left overflow"
    );
    assert!(
        inner.origin.y >= outer.origin.y - tolerance,
        "{label}: top overflow"
    );
    assert!(
        inner.right() <= outer.right() + tolerance,
        "{label}: right overflow {inner:?} / {outer:?}"
    );
    assert!(
        inner.bottom() <= outer.bottom() + tolerance,
        "{label}: bottom overflow {inner:?} / {outer:?}"
    );
}

fn check_sidebar_row_geometry(window: &Window, collapsed: bool) -> usize {
    use gpui_kit::base::test_support::{find, snapshots};
    let mut checked = 0;
    for snapshot in snapshots(window) {
        let Some(gpui::ElementId::Name(name)) = snapshot.path().last() else {
            continue;
        };
        let Some(id) = name.strip_prefix("sidebar-content:") else {
            continue;
        };
        if !snapshot.visible() {
            continue;
        }
        let row = element_snapshot(window, SharedString::from(id)).bounds();
        let content = snapshot.bounds();
        let icon =
            element_snapshot(window, SharedString::from(format!("sidebar-icon:{id}"))).bounds();
        assert_inside(content, row, "sidebar flex content");
        assert_inside(icon, row, "sidebar icon");
        let label_id: gpui::ElementId = SharedString::from(format!("sidebar-label:{id}")).into();
        if collapsed {
            assert!(
                find(window, &[], &label_id).is_none(),
                "collapsed rows must not retain hidden text"
            );
        } else {
            let label = element_snapshot(window, label_id).bounds();
            assert_inside(label, row, "sidebar label");
            assert!(label.origin.x >= icon.right(), "label overlaps icon");
            if id.starts_with("mailbox:") {
                let count =
                    element_snapshot(window, SharedString::from(format!("sidebar-count:{id}")))
                        .bounds();
                assert_inside(count, row, "sidebar count");
                assert!(
                    count.origin.x >= label.right() - px(1.),
                    "count overlaps label"
                );
            }
        }
        checked += 1;
    }
    checked
}

fn used_mail_icons() -> Vec<MailIcon> {
    let mut icons = Vec::new();
    for tail in include_str!("../../src/app.rs").split("MailIcon::").skip(1) {
        let name: String = tail
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        let icon = *MailIcon::ALL
            .iter()
            .find(|icon| format!("{icon:?}") == name)
            .expect("known mail icon");
        if !icons.contains(&icon) {
            icons.push(icon);
        }
    }
    icons
}

struct IconCatalog;

impl gpui::Render for IconCatalog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_env();
        div()
            .size_full()
            .bg(theme.canvas)
            .text_color(theme.text)
            .p_3()
            .flex()
            .flex_wrap()
            .content_start()
            .gap_2()
            .children(used_mail_icons().into_iter().map(|icon| {
                div()
                    .w(px(108.))
                    .h(px(70.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(Icon::new(icon).size(px(24.)))
                    .child(div().text_xs().child(format!("{icon:?}")))
            }))
    }
}

pub async fn check_visual_acceptance(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    let index = update(fixture, cx, |view, _, _| view.paths.index.clone());
    let connection = rusqlite::Connection::open(index).unwrap();
    for source in ["INBOX", "Sent"] {
        connection.execute(
            "INSERT INTO mailbox(account,server,source_name,display_name,uid_validity,scanned_at_unix,server_message_count,discovered_uid_count)
             VALUES('very.long.account.label.for.truncation@example.invalid','imap.e8.invalid',?1,?1,1,1,0,0)", [source]
        ).unwrap();
    }
    drop(connection);
    update(fixture, cx, |view, window, cx| {
        view.close_settings(window, cx);
        view.close_preview(window, cx);
        view.dismiss_search(window, cx);
        view.sync = None;
        view.onboarding = None;
        view.reload_index(cx).unwrap();
        let mailbox = view
            .accounts
            .iter()
            .flat_map(|account| &account.mailboxes)
            .find(|mailbox| mailbox.message_count > 0 && mailbox.message_count < 100)
            .unwrap()
            .id;
        view.select_mailbox(mailbox, true, window, cx);
        view.select_message_at(0, window, cx);
        view.sync_sidebar_tree(cx);
    });
    navigation_settle(fixture, cx).await;
    let probe = cx.update(crate::compose_window::ThemeProbe::open);
    let icons = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                show: false,
                focus: false,
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                    None,
                    gpui::size(px(720.), px(360.)),
                    cx,
                ))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|_| IconCatalog);
                crate::ui::root_view(view, window, cx)
            },
        )
        .unwrap()
    });
    let icons: gpui::AnyWindowHandle = icons.into();
    icons
        .update(cx, |_, window, _| {
            window.resize(gpui::size(px(720.), px(360.)))
        })
        .unwrap();
    for (name, dark, custom) in [
        ("default-dark", true, false),
        ("default-light", false, false),
        ("custom-dark", true, true),
        ("custom-light", false, true),
    ] {
        let mut prefs = Prefs {
            scheme: if dark {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
            palette: if custom {
                Palette::Custom
            } else {
                Palette::Default
            },
            ..Prefs::default()
        };
        if custom {
            prefs.custom_dark.primary = 0x4ABCA0;
            prefs.custom_dark.canvas = 0x101820;
            prefs.custom_light.primary = 0x804AD0;
            prefs.custom_light.canvas = 0xFFF5E1;
        }
        let expected = save_visual_prefs(fixture, cx, prefs.clone());
        clear_business_feedback(fixture, cx).await;
        let mut composer = probe.capture_refreshed(&format!("e8-{name}"), expected, cx);
        composer["batch"] = json!("E8");
        checks.push(composer);
        let path = icons
            .update(cx, |_, window, cx| {
                capture_frame(window, cx, &format!("e8-icons-{name}"))
            })
            .unwrap();
        checks.push(json!({"batch":"E8", "check":"all application icons rendered through the production selective AssetSource", "theme":name,
            "path":path, "evidence":"native-gpui-offscreen-screenshot", "visually_reviewed":false, "icon_count":used_mail_icons().len()}));
        for (surface, width, workspace, pane, collapsed, sidebar) in [
            (
                "standard-expanded",
                1180.,
                WorkspaceMode::Default,
                WorkspacePane::List,
                false,
                true,
            ),
            (
                "standard-collapsed",
                1180.,
                WorkspaceMode::Default,
                WorkspacePane::List,
                true,
                true,
            ),
            (
                "compact-list",
                640.,
                WorkspaceMode::Default,
                WorkspacePane::List,
                false,
                false,
            ),
            (
                "compact-sidebar",
                640.,
                WorkspaceMode::Default,
                WorkspacePane::Sidebar,
                false,
                true,
            ),
            (
                "compact-reader",
                640.,
                WorkspaceMode::Default,
                WorkspacePane::Reader,
                false,
                false,
            ),
            (
                "minimal-list",
                1180.,
                WorkspaceMode::Minimal,
                WorkspacePane::List,
                false,
                true,
            ),
        ] {
            let mut next = prefs.clone();
            next.workspace = workspace;
            save_visual_prefs(fixture, cx, next);
            update(fixture, cx, |view, window, cx| {
                view.sidebar_collapsed = collapsed;
                view.workspace_pane = pane;
                window.resize(gpui::size(px(width), px(760.)));
                cx.notify();
            });
            clear_business_feedback(fixture, cx).await;
            navigation_settle(fixture, cx).await;
            let checked = update_window(fixture, cx, |window, _| {
                check_sidebar_row_geometry(window, collapsed)
            });
            if sidebar {
                assert!(
                    checked >= 2,
                    "{name}/{surface}: expected real multi-account rows"
                );
            }
            if !sidebar {
                assert_eq!(
                    checked, 0,
                    "{name}/{surface}: single-pane workspace must hide the sidebar"
                );
            }
            let mut record = e8_record(
                if sidebar {
                    "sidebar icons, account labels and counts stay within one row without overlap"
                } else {
                    "single-pane workspace does not retain rendered sidebar rows"
                },
                "native-gpui-element-geometry",
            );
            record["theme"] = json!(name);
            record["surface"] = json!(surface);
            record["rows_checked"] = json!(checked);
            checks.push(record);
            let path = update_window(fixture, cx, |window, cx| {
                capture_frame(window, cx, &format!("e8-{name}-{surface}"))
            });
            checks.push(json!({"batch":"E8", "check":"E8 theme/workspace visual acceptance capture", "theme":name, "surface":surface,
                "path":path, "evidence":"native-gpui-offscreen-screenshot", "visually_reviewed":false}));
        }
    }
    probe.close(cx);
    icons
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    checks
}

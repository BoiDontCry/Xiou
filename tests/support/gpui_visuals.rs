// Native DirectX offscreen captures. File creation/pixel checks are automated;
// human/assistant visual review and real OS input acceptance remain separate.

fn save_visual_prefs(fixture: &TestWindow, cx: &mut AsyncApp, prefs: Prefs) -> Theme {
    update(fixture, cx, |view, window, cx| {
        view.dismiss_search(window, cx);
        view.open_settings(window, cx);
        view.settings
            .as_mut()
            .expect("visual settings session")
            .draft = prefs;
        view.save_settings(window, cx);
        assert!(
            view.settings.is_none(),
            "visual theme must pass actual settings validation/save"
        );
        view.workspace_pane = WorkspacePane::List;
        cx.notify();
        view.theme
    })
}

pub async fn capture_visual_baseline(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    let probe = cx.update(crate::compose_window::ThemeProbe::open);
    settle_overlay(cx).await;
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
        settle_overlay(cx).await;
        checks.push(probe.capture_refreshed(name, expected, cx));
        for (workspace, width, mode) in [
            ("standard", 1180., WorkspaceMode::Default),
            ("compact", 640., WorkspaceMode::Default),
            ("minimal", 1180., WorkspaceMode::Minimal),
        ] {
            let mut workspace_prefs = prefs.clone();
            workspace_prefs.workspace = mode;
            save_visual_prefs(fixture, cx, workspace_prefs);
            update_window(fixture, cx, |window, _| {
                window.resize(gpui::size(px(width), px(760.)))
            });
            settle_overlay(cx).await;
            let path = update_window(fixture, cx, |window, cx| {
                capture_frame(window, cx, &format!("workspace-{name}-{workspace}"))
            });
            checks.push(json!({"check": "workspace offscreen screenshot", "theme": name, "workspace": workspace,
                "path": path, "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false}));
        }
        save_visual_prefs(fixture, cx, prefs);
        settle_overlay(cx).await;
        update(fixture, cx, |view, window, cx| {
            view.open_settings(window, cx)
        });
        settle_overlay(cx).await;
        let path = update_window(fixture, cx, |window, cx| {
            capture_frame(window, cx, &format!("visual-settings-{name}"))
        });
        checks.push(
            json!({"check": "settings business screenshot", "theme": name, "path": path,
            "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false}),
        );
        update(fixture, cx, |view, window, cx| {
            view.close_settings(window, cx);
            view.open_search(window, cx);
            view.search_input
                .update(cx, |input, cx| input.replace_all("fixture", window, cx));
        });
        settle_overlay(cx).await;
        let path = update_window(fixture, cx, |window, cx| {
            capture_frame(window, cx, &format!("visual-search-{name}"))
        });
        checks.push(
            json!({"check": "search business screenshot", "theme": name, "path": path,
            "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false}),
        );
        update(fixture, cx, |view, window, cx| {
            view.dismiss_search(window, cx);
            let message_id = view.message_pages[&0][0].id;
            view.start_attachment_preview(
                message_id,
                0,
                "fixture.gif".into(),
                "image/gif".into(),
                window,
                cx,
            );
        });
        settle_overlay(cx).await;
        let path = update_window(fixture, cx, |window, cx| {
            capture_frame(window, cx, &format!("visual-preview-{name}"))
        });
        checks.push(
            json!({"check": "preview business screenshot", "theme": name, "path": path,
            "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false}),
        );
        update(fixture, cx, |view, window, cx| {
            view.close_preview(window, cx)
        });
    }
    probe.close(cx);
    checks
}

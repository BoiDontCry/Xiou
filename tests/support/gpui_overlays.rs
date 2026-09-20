// Included inside the production app module by gpui_migration.rs. All input
// below is synthetic GPUI input; this is not evidence of Windows IME behavior.

pub(super) fn overlay_evidence(check: &str, evidence: &str) -> Value {
    eprintln!("PASS [{evidence}] {check}");
    json!({"check": check, "evidence": evidence})
}

pub(super) fn capture_frame(window: &mut Window, cx: &mut gpui::App, name: &str) -> PathBuf {
    window.draw(cx).clear(cx);
    let root = PathBuf::from(std::env::var_os("FOXMAIL_INDEX_PATH").unwrap());
    let directory = root.parent().unwrap().join("screenshots");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}.png"));
    let image = window
        .render_to_image()
        .expect("Windows GPUI offscreen capture");
    assert!(
        image.width() > 64 && image.height() > 64,
        "degenerate capture is not visual evidence"
    );
    image.save(&path).unwrap();
    eprintln!("SCREENSHOT {}", path.display());
    path
}

pub(super) fn element_snapshot(
    window: &Window,
    id: impl Into<gpui::ElementId>,
) -> gpui_kit::base::test_support::ElementSnapshot {
    let id = id.into();
    gpui_kit::base::test_support::find(window, &[], &id).unwrap_or_else(|| {
        panic!(
            "missing element {id:?}: {}",
            gpui_kit::base::test_support::registered_paths(window)
        )
    })
}

fn click_position(window: &mut Window, position: gpui::Point<gpui::Pixels>, cx: &mut gpui::App) {
    window.dispatch_event(
        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position,
            ..Default::default()
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    );
}

fn click_element(fixture: &TestWindow, cx: &mut AsyncApp, id: impl Into<gpui::ElementId>) {
    let id = id.into();
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        let snapshot = element_snapshot(window, id);
        assert!(
            snapshot.visible(),
            "clicked element must be visible: {snapshot:?}"
        );
        let bounds = snapshot.bounds();
        click_position(
            window,
            gpui::point(
                bounds.origin.x + bounds.size.width / 2.,
                bounds.origin.y + bounds.size.height / 2.,
            ),
            cx,
        );
    });
}

fn press_key(fixture: &TestWindow, cx: &mut AsyncApp, key: &str) {
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx),
            "unhandled key: {key}"
        );
    });
}

async fn settle_overlay(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(250))
        .await;
}

pub async fn check_business_overlays(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    let original = update(fixture, cx, |view, window, cx| {
        view.close_settings(window, cx);
        view.dismiss_search(window, cx);
        window.focus(&view.list_focus, cx);
        view.prefs = Prefs {
            scheme: ColorScheme::Dark,
            palette: Palette::Custom,
            ..Prefs::default()
        };
        view.theme = Theme::from_prefs(&view.prefs);
        apply_foxmail_theme(view.theme, cx);
        view.open_settings(window, cx);
        assert!(window.has_active_dialog(cx));
        view.prefs.clone()
    });
    settle(cx).await;
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        capture_frame(window, cx, "settings-custom-dark");
        let snapshot = element_snapshot(window, "settings-content");
        assert!(snapshot.visible(), "{snapshot:?}");
        let sliders = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|element| element.role() == Some(gpui::Role::Slider))
            .count();
        assert_eq!(sliders, 3, "all HSV controls are official Sliders");
        assert!(gpui_kit::base::active_focus_trap(window, cx).is_some());
    });
    checks.push(overlay_evidence(
        "settings business Dialog renders Form, Select and all three Sliders",
        "business-state-and-render",
    ));

    for _ in 0..18 {
        press_key(fixture, cx, "tab");
        settle(cx).await;
        update_window(fixture, cx, |window, cx| {
            let trap = gpui_kit::base::active_focus_trap(window, cx).expect("modal focus trap");
            assert!(
                trap.contains_focused(window, cx),
                "Tab escaped the settings dialog"
            );
        });
    }
    checks.push(overlay_evidence(
        "settings Tab cycles inside the official modal focus trap",
        "synthetic-gpui-keys",
    ));

    // The actual Select keyboard path must reach its production subscription.
    update(fixture, cx, |view, window, cx| {
        let focus = view
            .settings
            .as_ref()
            .unwrap()
            .token_select
            .read(cx)
            .focus_handle(cx);
        window.focus(&focus, cx);
    });
    press_key(fixture, cx, "enter");
    settle(cx).await;
    press_key(fixture, cx, "down");
    press_key(fixture, cx, "enter");
    settle(cx).await;
    update(fixture, cx, |view, _, cx| {
        let session = view.settings.as_ref().unwrap();
        assert_eq!(session.token, TokenId::Canvas);
        assert_eq!(
            session.token_select.read(cx).selected_value(),
            Some(&TokenId::Canvas)
        );
        assert_eq!(
            session.draft, original,
            "choosing a token is not editing its color"
        );
    });
    checks.push(overlay_evidence(
        "Select Enter/Down/Enter selects a token without changing the draft",
        "synthetic-gpui-keys",
    ));

    update(fixture, cx, |view, window, cx| {
        view.set_settings_token(TokenId::Primary, window, cx);
        let session = view.settings.as_ref().unwrap();
        window.focus(&session.slider_focus[0], cx);
    });
    let hue_before = update(fixture, cx, |view, _, _| {
        view.settings.as_ref().unwrap().hsv[0]
    });
    press_key(fixture, cx, "right");
    settle(cx).await;
    update(fixture, cx, |view, _, cx| {
        let session = view.settings.as_ref().unwrap();
        assert!(session.hsv[0] > hue_before);
        assert_eq!(session.sliders[0].read(cx).value().start(), session.hsv[0]);
        assert_eq!(
            session.hex_input.read(cx).value().as_ref(),
            format_hex(session.draft.custom_dark.primary)
        );
    });
    checks.push(overlay_evidence(
        "HSV keyboard adapter updates the official Slider, HEX and draft",
        "synthetic-gpui-keys",
    ));

    let slider_id = update(fixture, cx, |view, _, _| {
        view.settings.as_ref().unwrap().sliders[0].entity_id()
    });
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        let bounds = element_snapshot(window, ("slider", slider_id)).bounds();
        click_position(
            window,
            gpui::point(
                bounds.origin.x + bounds.size.width * 0.25,
                bounds.origin.y + bounds.size.height / 2.,
            ),
            cx,
        );
    });
    settle(cx).await;
    update(fixture, cx, |view, _, cx| {
        let session = view.settings.as_ref().unwrap();
        assert!(
            (session.hsv[0] - 0.25).abs() < 0.06,
            "pointer event must change the hue: {:?}",
            session.hsv
        );
        assert_eq!(
            session.hex_input.read(cx).value().as_ref(),
            format_hex(session.draft.custom_dark.primary)
        );
    });
    checks.push(overlay_evidence(
        "official Slider pointer path reaches the production color subscription",
        "synthetic-gpui-pointer",
    ));

    update(fixture, cx, |view, window, cx| {
        view.set_settings_hsv(0, 0.7, window, cx);
        view.set_settings_hsv(1, 0., window, cx);
        view.set_settings_hsv(2, 0., window, cx);
        view.set_settings_hsv(2, 0.8, window, cx);
        view.set_settings_hsv(1, 0.6, window, cx);
        let session = view.settings.as_ref().unwrap();
        assert_eq!(session.hsv, [0.7, 0.6, 0.8]);
        assert_eq!(
            session.draft.custom_dark.primary,
            hsv_to_rgb(252., 0.6, 0.8)
        );
        let before = session.draft.clone();
        view.set_settings_hsv(3, 1., window, cx);
        view.set_settings_hsv(0, f32::NAN, window, cx);
        assert_eq!(view.settings.as_ref().unwrap().draft, before);
    });
    checks.push(overlay_evidence(
        "HSV preserves hue through grey/black and rejects invalid channels/NaN",
        "business-state",
    ));

    update(fixture, cx, |view, window, cx| {
        view.settings
            .as_ref()
            .unwrap()
            .hex_input
            .update(cx, |input, cx| input.replace_all("#GG00ZZ", window, cx));
    });
    settle(cx).await;
    click_element(fixture, cx, "settings-save");
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.settings.as_ref().unwrap().hex_invalid);
        assert!(!view.settings.as_ref().unwrap().can_save());
        assert!(window.has_active_dialog(cx));
        assert_eq!(view.prefs, original);
    });
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        // The pinned Base Button explicitly documents that GPUI cannot expose
        // aria-disabled yet. Verify the real pointer behavior above and the
        // enabled save path below, rather than inventing an accessibility flag.
        assert!(element_snapshot(window, "settings-save").visible());
        capture_frame(window, cx, "settings-invalid-hex");
    });
    checks.push(overlay_evidence(
        "invalid HEX disables Save and cannot commit the last valid color",
        "synthetic-gpui-pointer-and-state",
    ));

    click_element(fixture, cx, "settings-cancel");
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.settings.is_none());
        assert!(!window.has_active_dialog(cx));
        assert!(view.list_focus.is_focused(window));
        assert_eq!(view.prefs, original);
        view.open_settings(window, cx);
        view.settings
            .as_ref()
            .unwrap()
            .hex_input
            .update(cx, |input, cx| input.replace_all("#3FA280", window, cx));
    });
    settle(cx).await;
    click_element(fixture, cx, "settings-save");
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.settings.is_none());
        assert!(!window.has_active_dialog(cx));
        assert_eq!(view.prefs.custom_dark.primary, 0x3FA280);
        assert_eq!(prefs::load().unwrap(), view.prefs);
        assert!(view.list_focus.is_focused(window));
    });
    checks.push(overlay_evidence(
        "settings Cancel discards, Save persists, and both restore the invoking focus",
        "synthetic-gpui-pointer-and-isolated-persistence",
    ));

    for outside in [false, true] {
        update(fixture, cx, |view, window, cx| {
            view.open_settings(window, cx);
            view.set_settings_flag("sidebar-mono", true, cx);
        });
        settle(cx).await;
        if outside {
            update_window(fixture, cx, |window, cx| {
                window.draw(cx).clear(cx);
                click_position(window, gpui::point(px(10.), px(100.)), cx);
            });
        } else {
            press_key(fixture, cx, "escape");
        }
        settle_overlay(cx).await;
        update(fixture, cx, |view, window, cx| {
            assert!(
                view.settings.is_none(),
                "settings close path: outside={outside}"
            );
            assert!(!window.has_active_dialog(cx));
            assert!(!view.prefs.sidebar_mono_icons, "dismissal must not save");
            assert!(view.list_focus.is_focused(window));
        });
    }
    checks.push(overlay_evidence(
        "settings Esc and backdrop cancel the business session and restore focus",
        "synthetic-gpui-keys-and-pointer",
    ));

    // Fail a save using only a path inside the harness profile. No environment
    // mutation after GPUI threads start, and no real preferences are touched.
    let path =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("foxmail-rs/prefs.json");
    let backup = path.with_extension("fixture-backup");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    update(fixture, cx, |view, window, cx| {
        view.open_settings(window, cx);
        view.set_settings_flag("sidebar-mono", true, cx);
        view.save_settings(window, cx);
        let session = view.settings.as_ref().expect("failed save retains draft");
        assert!(session.save_error.is_some());
        assert!(session.draft.sidebar_mono_icons);
        assert!(!view.prefs.sidebar_mono_icons);
        assert!(window.has_active_dialog(cx));
        view.close_settings(window, cx);
    });
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&backup, &path).unwrap();
    checks.push(overlay_evidence(
        "settings disk failure is visible and retains the unsaved draft",
        "business-state-and-isolated-filesystem",
    ));
    checks
}

pub(super) fn seed_overlay_mail(fixture: &TestWindow, cx: &mut AsyncApp) -> (i64, i64) {
    update(fixture, cx, |view, _, cx| {
        let root = view
            .paths
            .index
            .parent()
            .unwrap()
            .join("overlay-fixture-eml");
        let mailbox = root.join("fixture/INBOX/uidvalidity-1");
        std::fs::create_dir_all(&mailbox).unwrap();
        std::fs::write(
            mailbox.join("mailbox.meta.json"),
            serde_json::to_vec(&json!({
                "account": "fixture@example.invalid", "server": "imap.example.invalid",
                "mailbox": "INBOX", "uid_validity": 1, "scanned_at_unix": 1,
                "server_message_count": 3, "discovered_uid_count": 3
            }))
            .unwrap(),
        )
        .unwrap();
        for uid in 7..10 {
            let eml = format!("From: Fixture <fixture@example.invalid>\r\nTo: test@example.invalid\r\nSubject: fixture result {uid}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=fixture-boundary\r\n\r\n\
--fixture-boundary\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nOffline fixture 中文正文。\r\n\
--fixture-boundary\r\nContent-Type: image/gif; name=fixture.gif\r\nContent-Disposition: attachment; filename=fixture.gif\r\nContent-Transfer-Encoding: base64\r\n\r\nR0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7\r\n\
--fixture-boundary\r\nContent-Type: text/plain; name=report.txt\r\nContent-Disposition: attachment; filename=report.txt\r\n\r\nOffline attachment.\r\n--fixture-boundary--\r\n");
            std::fs::write(mailbox.join(format!("{uid:010}.eml")), eml.as_bytes()).unwrap();
        }
        let mut store = MailStore::open(&view.paths.index).unwrap();
        // Production resolves EML paths relative to the index parent.
        let imported = store
            .import_backup(view.paths.index.parent().unwrap())
            .unwrap();
        assert_eq!(imported.inserted, 3);
        let accounts = store.mailbox_tree().unwrap();
        let mailbox_id = accounts[0].mailboxes[0].id;
        let page = store
            .message_page(mailbox_id, 0, MESSAGE_PAGE_SIZE)
            .unwrap();
        let message_id = page[0].id;
        view.store = Some(store);
        view.accounts = accounts;
        view.selected_mailbox = Some(mailbox_id);
        view.message_pages.clear();
        view.message_pages.insert(0, page);
        view.load_error = None;
        view.onboarding = None;
        view.sync = None;
        cx.notify();
        (mailbox_id, message_id)
    })
}

pub async fn check_search_and_preview(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    let (_, message_id) = seed_overlay_mail(fixture, cx);
    update(fixture, cx, |view, window, cx| {
        view.dismiss_search(window, cx);
        view.open_search(window, cx);
    });
    settle(cx).await;
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(element_snapshot(window, "search-dropdown").visible());
        assert!(element_snapshot(window, ("search-hit", 0_usize)).visible());
    });
    click_element(fixture, cx, "search-input");
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(
            view.search_open,
            "clicking the active input must not toggle the popup"
        );
        assert!(view
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window));
    });
    update_window(fixture, cx, |window, cx| {
        click_position(
            window,
            gpui::point(window.viewport_size().width - px(20.), px(200.)),
            cx,
        );
    });
    settle(cx).await;
    update(fixture, cx, |view, _, cx| {
        assert_search_cleared(view, false, cx)
    });
    checks.push(overlay_evidence(
        "search Popover renders recents, keeps input focus, and dismisses on outside click",
        "synthetic-gpui-pointer",
    ));

    update(fixture, cx, |view, window, cx| {
        view.open_search(window, cx);
        view.search_input
            .update(cx, |input, cx| input.replace_all("fixture", window, cx));
    });
    settle(cx).await;
    press_key(fixture, cx, "space");
    settle(cx).await;
    update(fixture, cx, |view, _, cx| {
        assert!(view.search_pending);
        assert!(view.search_open);
        assert_eq!(view.search_input.read(cx).value().as_ref(), "fixture ");
    });
    checks.push(overlay_evidence(
        "search Space remains Input text instead of toggling the Popover",
        "synthetic-gpui-keys",
    ));
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(element_snapshot(window, "search-status").visible());
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, _| {
        assert!(!view.search_pending);
        assert_eq!(view.search_results.len(), 3);
        assert_eq!(view.search_selection, 0);
    });
    press_key(fixture, cx, "down");
    settle(cx).await;
    let selected = update(fixture, cx, |view, _, _| {
        assert_eq!(
            view.search_selection, 1,
            "search arrows must not use the message-list binding"
        );
        view.search_results[1].id
    });
    press_key(fixture, cx, "enter");
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert_eq!(view.selected_message, Some(selected));
        assert!(view.selected_detail.is_some());
        assert!(!view.search_open);
        assert!(view.list_focus.is_focused(window));
        assert!(view.search_input.read(cx).value().is_empty());
    });
    checks.push(overlay_evidence("debounced search shows pending/results and Down/Enter opens the selected real fixture mail", "synthetic-gpui-keys-and-offline-store"));

    update(fixture, cx, |view, window, cx| {
        view.open_search(window, cx);
        view.search_input.update(cx, |input, cx| {
            input.replace_all("no-such-fixture-term-xyz", window, cx)
        });
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, _| {
        assert!(view.search_results.is_empty());
        assert!(view.search_error.is_none());
    });
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(element_snapshot(window, "search-status").visible());
    });
    let store = update(fixture, cx, |view, window, cx| {
        let store = view.store.take();
        view.search_input.update(cx, |input, cx| {
            input.replace_all("missing-index", window, cx)
        });
        store
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, cx| {
        assert!(view.search_error.is_some());
        view.store = store;
        cx.notify();
    });
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(element_snapshot(window, "search-status").visible());
    });
    checks.push(overlay_evidence(
        "search no-result and missing-index failures render in the business Popover",
        "business-state-and-offline-store-render",
    ));

    update(fixture, cx, |view, window, cx| {
        view.search_input
            .update(cx, |input, cx| input.replace_all("fixture", window, cx));
    });
    settle_overlay(cx).await;
    click_element(fixture, cx, "clear-search");
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert_search_cleared(view, true, cx);
        assert!(view
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window));
        view.search_input
            .update(cx, |input, cx| input.replace_all("fixture", window, cx));
    });
    checks.push(overlay_evidence(
        "Clear button resets search while retaining input focus and open intent",
        "synthetic-gpui-pointer",
    ));
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, cx| {
        let hits = view.search_results.clone();
        assert!(!hits.is_empty());
        view.search_results = (0..20)
            .map(|index| {
                let mut hit = hits[index % hits.len()].clone();
                hit.subject = format!("Synthetic scroll result {index}");
                hit
            })
            .collect();
        view.search_selection = 0;
        cx.notify();
    });
    for _ in 0..12 {
        press_key(fixture, cx, "down");
        settle(cx).await;
    }
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(element_snapshot(window, ("search-hit", 12_usize)).visible());
    });
    update(fixture, cx, |view, _, _| {
        assert_eq!(view.search_selection, 12);
        assert!(view.search_dropdown_scroll.offset().y < px(0.));
    });
    checks.push(overlay_evidence(
        "search keyboard selection scrolls the highlighted ListItem into view",
        "synthetic-gpui-keys-and-layout",
    ));

    update(fixture, cx, |view, window, cx| {
        view.search_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                input,
                None,
                "pin",
                Some(3..3),
                window,
                cx,
            );
        });
    });
    settle(cx).await;
    let selected_before = update(fixture, cx, |view, _, _| {
        view.search_generation = view.search_generation.wrapping_add(1);
        view.search_results = view.store.as_ref().unwrap().search("fixture", 10).unwrap();
        view.search_pending = false;
        view.search_selection = 1;
        view.selected_message
    });
    for key in ["down", "up", "enter", "escape"] {
        press_key(fixture, cx, key);
        settle(cx).await;
    }
    update(fixture, cx, |view, window, cx| {
        assert!(view.search_is_composing(window, cx));
        assert_eq!(view.search_selection, 1);
        assert_eq!(view.selected_message, selected_before);
        assert!(view.search_open);
        view.search_input.update(cx, |input, cx| {
            gpui::EntityInputHandler::unmark_text(input, window, cx)
        });
        view.dismiss_search(window, cx);
        view.open_search(window, cx);
    });
    checks.push(overlay_evidence(
        "marked-text arrows/Enter/Esc do not select mail or dismiss composing search",
        "synthetic-gpui-marked-text-NOT-native-IME",
    ));

    press_key(fixture, cx, "up");
    settle(cx).await;
    update(fixture, cx, |view, _, _| {
        assert_eq!(view.search_selection, 2)
    });
    press_key(fixture, cx, "escape");
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert_search_cleared(view, false, cx);
        view.open_search(window, cx);
        view.search_input
            .update(cx, |input, cx| input.replace_all("fixture", window, cx));
    });
    settle_overlay(cx).await;
    let first = update(fixture, cx, |view, _, _| view.search_results[0].id);
    click_element(fixture, cx, ("search-hit", 0_usize));
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, _| {
        assert_eq!(view.selected_message, Some(first));
        assert!(!view.search_open);
    });
    checks.push(overlay_evidence(
        "recent Up wraps, Esc clears, and result clicks select before input Blur clears hits",
        "synthetic-gpui-keys-and-pointer",
    ));

    for (filename, content_type) in [
        ("fixture.pdf", "application/pdf"),
        ("fixture.bin", "application/octet-stream"),
        ("fixture.gif", "image/gif"),
    ] {
        update(fixture, cx, |view, window, cx| {
            window.focus(&view.list_focus, cx);
            view.start_attachment_preview(
                message_id,
                0,
                filename.into(),
                content_type.into(),
                window,
                cx,
            );
            assert!(window.has_active_dialog(cx));
        });
        settle_overlay(cx).await;
        update_window(fixture, cx, |window, cx| {
            window.draw(cx).clear(cx);
            assert!(element_snapshot(window, "attachment-preview-content").visible());
        });
        update(fixture, cx, |view, _, _| match content_type {
            "application/pdf" => assert!(matches!(view.preview, Some(PreviewState::Pdf { .. }))),
            "image/gif" => assert!(matches!(view.preview, Some(PreviewState::Image { .. }))),
            _ => assert!(matches!(view.preview, Some(PreviewState::Failed { .. }))),
        });
        press_key(fixture, cx, "escape");
        settle_overlay(cx).await;
        update(fixture, cx, |view, window, cx| {
            assert!(view.preview.is_none());
            assert!(!window.has_active_dialog(cx));
            assert!(view.list_focus.is_focused(window));
        });
    }
    checks.push(overlay_evidence(
        "attachment business Dialog renders real image/PDF fallback/error and Esc restores focus",
        "synthetic-gpui-keys-and-offline-attachment-render",
    ));

    update(fixture, cx, |view, window, cx| {
        view.start_attachment_preview(
            message_id,
            0,
            "fixture.gif".into(),
            "image/gif".into(),
            window,
            cx,
        );
        view.close_preview(window, cx);
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(
            view.preview.is_none(),
            "late image load must not reopen a closed preview"
        );
        assert!(!window.has_active_dialog(cx));
        view.start_attachment_preview(
            message_id,
            0,
            "fixture.pdf".into(),
            "application/pdf".into(),
            window,
            cx,
        );
        view.invalidate_preview();
        cx.notify();
    });
    update_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.preview.is_none());
        assert!(
            !window.has_active_dialog(cx),
            "invalidated preview must remove its Root dialog too"
        );
    });
    checks.push(overlay_evidence(
        "preview generation rejects late loads and background invalidation closes its Root dialog",
        "business-state-and-async-render",
    ));

    update_window(fixture, cx, |window, _| {
        window.resize(gpui::size(px(400.), px(380.)))
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        view.open_settings(window, cx)
    });
    settle(cx).await;
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        let save = element_snapshot(window, "settings-save");
        let bounds = save.bounds();
        assert!(save.visible());
        assert!(
            bounds.origin.y + bounds.size.height <= window.viewport_size().height,
            "Save is below the small window"
        );
        let content = element_snapshot(window, "settings-content").bounds();
        assert!(content.origin.x >= px(0.));
        assert!(content.origin.x + content.size.width <= window.viewport_size().width);
    });
    update(fixture, cx, |view, window, cx| {
        view.close_settings(window, cx)
    });
    update_window(fixture, cx, |window, _| {
        window.resize(gpui::size(px(1180.), px(760.)))
    });
    settle_overlay(cx).await;
    checks.push(overlay_evidence(
        "small-window settings content stays bounded with Save reachable",
        "native-window-resize-and-layout-NOT-screenshot",
    ));
    checks
}

pub async fn close_fixture(fixture: &TestWindow, cx: &mut AsyncApp) {
    update(fixture, cx, |view, window, cx| {
        view.close_settings(window, cx);
        view.close_preview(window, cx);
        view.dismiss_search(window, cx);
        window.focus(&view.list_focus, cx);
        window.clear_notifications(cx);
    });
    // Input/Popover dismissal retains handles until deferred events/animations
    // finish. Drain those before destroying the native window and the App.
    settle_overlay(cx).await;
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        window.remove_window();
    });
    settle(cx).await;
}

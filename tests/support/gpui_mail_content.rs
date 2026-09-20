// E7 runs the production MIME -> MailStore -> safe blocks -> TextView/Attachment
// path. Every message is local and synthetic; no account or network is used.

struct ContentFixture {
    mailbox: i64,
    html: i64,
    literal: i64,
    large: i64,
}

fn e7_record(check: &str, evidence: &str) -> Value {
    let mut record = overlay_evidence(check, evidence);
    record["batch"] = json!("E7");
    record
}

fn content_markup(blocks: &[MailBodyBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            MailBodyBlock::Text { html, .. } => html.clone(),
            MailBodyBlock::Image { .. } => String::new(),
            MailBodyBlock::TableRow { cells } => cells
                .iter()
                .filter_map(|cell| cell.content.as_ref())
                .map(|block| content_markup(std::slice::from_ref(block)))
                .collect::<String>(),
        })
        .collect()
}

fn seed_content_mail(fixture: &TestWindow, cx: &mut AsyncApp) -> ContentFixture {
    update(fixture, cx, |view, _, cx| {
        let root = view.paths.index.parent().unwrap();
        let mailbox = root.join("e7-fixture-eml/fixture/INBOX/uidvalidity-1");
        std::fs::create_dir_all(&mailbox).unwrap();
        std::fs::write(
            mailbox.join("mailbox.meta.json"),
            serde_json::to_vec(&json!({
                "account": "e7@example.invalid", "server": "imap.e7.invalid",
                "mailbox": "INBOX", "uid_validity": 1, "scanned_at_unix": 1,
                "server_message_count": 3, "discovered_uid_count": 3
            }))
            .unwrap(),
        )
        .unwrap();
        let html = "<h2>E7 标题</h2><p>正文 &amp; Unicode 🙂</p>\
            <table><tr><th>First</th><th>Second</th></tr>\
            <tr><td rowspan='2'>E7-ROWSPAN</td><td>E7-CELL</td></tr>\
            <tr><td>E7-TAIL-CELL</td></tr></table>\
            <p><a href='https://tracker.example.invalid/link'>safe link label</a>\
            <img src='https://tracker.example.invalid/pixel'>\
            <img src='file:///not-accessed'>\
            <img src='cid:e7-logo' alt='local logo'>\
            <img src='cid:e7-missing' alt='missing local image'></p>\
            <script>e7_script_must_not_render()</script>";
        let mixed = format!("--e7-boundary\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
--e7-boundary\r\nContent-Type: image/gif; name=logo.gif\r\nContent-ID: <e7-logo>\r\nContent-Disposition: inline; filename=logo.gif\r\nContent-Transfer-Encoding: base64\r\n\r\nR0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7\r\n\
--e7-boundary\r\nContent-Type: application/pdf; name=report.pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\n\r\n%PDF-1.4\r\n%%EOF\r\n\
--e7-boundary\r\nContent-Type: application/octet-stream; name=unsafe.bin\r\nContent-Disposition: attachment; filename=\"../../unsafe?.bin\"\r\n\r\nE7 binary fixture\r\n--e7-boundary--\r\n");
        let literal = "# literal [link](https://tracker.example.invalid)\r\n<img src='file:///not-accessed'> & <script>not executable</script>\r\n中文🙂 literal body".to_owned();
        let large = format!(
            "E7-LARGE-FIRST\r\n{}\r\nE7-LARGE-LAST",
            "bounded Unicode 中文🙂 <not-markup> & text\r\n".repeat(12_000)
        );
        for (uid, content_type, body) in [
            (101, "multipart/mixed; boundary=e7-boundary", mixed),
            (102, "text/plain; charset=utf-8", literal),
            (103, "text/plain; charset=utf-8", large),
        ] {
            let eml = format!("From: E7 Fixture <e7@example.invalid>\r\nTo: test@example.invalid\r\nSubject: E7 content {uid}\r\nMessage-ID: <e7-{uid}@example.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: {content_type}\r\n\r\n{body}");
            std::fs::write(mailbox.join(format!("{uid:010}.eml")), eml.as_bytes()).unwrap();
        }
        let mut store = MailStore::open(&view.paths.index).unwrap();
        assert_eq!(store.import_backup(root).unwrap().inserted, 3);
        drop(store);
        let connection = rusqlite::Connection::open(&view.paths.index).unwrap();
        let mailbox: i64 = connection
            .query_row(
                "SELECT id FROM mailbox WHERE account='e7@example.invalid' AND source_name='INBOX'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let message = |uid: i64| -> i64 {
            connection
                .query_row(
                    "SELECT id FROM message WHERE mailbox_id=?1 AND uid=?2",
                    rusqlite::params![mailbox, uid],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let result = ContentFixture {
            mailbox,
            html: message(101),
            literal: message(102),
            large: message(103),
        };
        view.reload_index(cx).unwrap();
        view.onboarding = None;
        view.sync = None;
        result
    })
}

async fn select_content_mail(fixture: &TestWindow, cx: &mut AsyncApp, mailbox: i64, message: i64) {
    update(fixture, cx, |view, window, cx| {
        view.close_preview(window, cx);
        view.select_mailbox(mailbox, true, window, cx);
        view.select_message(message, cx);
    });
    let mut loaded = false;
    for _ in 0..100 {
        loaded = update(fixture, cx, |view, _, _| {
            assert!(
                view.detail_load_error.is_none(),
                "offline content load failed: {:?}",
                view.detail_load_error
            );
            view.selected_detail.as_ref().map(|detail| detail.id) == Some(message)
        });
        if loaded {
            break;
        }
        settle(cx).await;
    }
    assert!(loaded, "real asynchronous content loader did not finish");
    navigation_settle(fixture, cx).await;
}

fn visible_content_part(window: &Window, name: &'static str) -> bool {
    gpui_kit::base::test_support::snapshots(window)
        .iter()
        .any(|snapshot| {
            snapshot.visible()
                && match snapshot.path().last() {
                    Some(gpui::ElementId::Name(id) | gpui::ElementId::NamedInteger(id, _)) => {
                        id.as_ref() == name
                    }
                    _ => false,
                }
        })
}

fn visible_body_blocks(window: &Window) -> Vec<usize> {
    gpui_kit::base::test_support::snapshots(window)
        .iter()
        .filter_map(|snapshot| {
            if !snapshot.visible() {
                return None;
            }
            match snapshot.path().last() {
                Some(gpui::ElementId::NamedInteger(name, index))
                    if name.as_ref() == "mail-body-block" =>
                {
                    Some(*index as usize)
                }
                _ => None,
            }
        })
        .collect()
}

pub async fn check_mail_content(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    update_window(fixture, cx, |window, _| {
        window.resize(gpui::size(px(1440.), px(900.)))
    });
    settle_overlay(cx).await;
    save_visual_prefs(
        fixture,
        cx,
        Prefs {
            scheme: ColorScheme::Dark,
            ..Prefs::default()
        },
    );
    clear_business_feedback(fixture, cx).await;
    let content = seed_content_mail(fixture, cx);
    select_content_mail(fixture, cx, content.mailbox, content.html).await;
    update(fixture, cx, |view, window, cx| {
        let detail = view.selected_detail.as_ref().unwrap();
        assert_eq!(detail.attachments.len(), 3);
        assert_eq!(view.selected_body_cids.len(), 1);
        assert!(view.selected_body_cids.contains_key("e7-logo"));
        let markup = content_markup(&view.selected_body_blocks);
        assert!(markup.contains("E7 标题") && markup.contains("safe link label"));
        for forbidden in [
            "<img",
            "<script",
            "<a ",
            "src=",
            "tracker.example.invalid",
            "e7_script_must_not_render",
        ] {
            assert!(
                !markup.contains(forbidden),
                "unsafe resource or markup reached TextView: {forbidden}"
            );
        }
        assert!(view
            .selected_body_blocks
            .iter()
            .any(|block| matches!(block, MailBodyBlock::TableRow { cells } if cells.len() == 2)));
        let sanitized = sanitized_attachment_filename(&detail.attachments[2].filename);
        assert!(
            !sanitized.contains(['/', '\\', ':', '?', '*']),
            "unsafe output filename: {sanitized}"
        );
        let output = view.paths.index.parent().unwrap().join("e7-safe-filename");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join(&sanitized), b"isolated filename probe").unwrap();
        view.reader_split
            .update(cx, |state, cx| state.resize_panel(1, px(320.), window, cx));
    });
    navigation_settle(fixture, cx).await;
    update(fixture, cx, |view, _, cx| {
        let table = view
            .selected_body_blocks
            .iter()
            .position(|block| matches!(block, MailBodyBlock::TableRow { .. }))
            .unwrap();
        view.body_list.scroll_to_reveal_item(table);
        cx.notify();
    });
    navigation_settle(fixture, cx).await;
    update_window(fixture, cx, |window, _| {
        assert!(visible_content_part(window, "mail-text-region"));
        // Scrollable renames its content div; assert real grid cells, not the
        // overwritten pre-wrapper ID or an assumption about the initial viewport.
        assert!(visible_content_part(window, "mail-table-cell"));
        assert!(element_snapshot(window, ("attachment-card-region", 0_usize)).visible());
    });
    checks.push(e7_record("real imported MIME produces safe TextView markup, expanded tables, local CID bytes and sanitized attachment metadata", "isolated-mime-sqlite-and-native-render"));

    for (cid, marker) in [
        ("e7-logo", "mail-cid-image"),
        ("e7-missing", "mail-cid-missing"),
    ] {
        update(fixture, cx, |view, _, cx| {
            let index = view.selected_body_blocks.iter().position(|block| matches!(block, MailBodyBlock::Image { cid: current, .. } if current == cid)).unwrap();
            view.body_list.scroll_to_reveal_item(index);
            cx.notify();
        });
        navigation_settle(fixture, cx).await;
        update_window(fixture, cx, |window, _| {
            assert!(
                visible_content_part(window, marker),
                "missing CID render marker: {marker}"
            )
        });
    }
    checks.push(e7_record(
        "resolved CID image and missing-CID fallback both render through the actual reader",
        "isolated-mime-and-native-layout",
    ));

    let generation = update(fixture, cx, |view, window, cx| {
        window.focus(&view.list_focus, cx);
        view.preview_generation
    });
    click_element(fixture, cx, ("attachment-card-region", 0_usize));
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert_eq!(view.preview_generation, generation.wrapping_add(1));
        assert!(
            matches!(&view.preview, Some(PreviewState::Image { filename, .. }) if filename.as_ref() == "logo.gif")
        );
        assert!(window.has_active_dialog(cx));
    });
    press_key(fixture, cx, "escape");
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.preview.is_none() && !window.has_active_dialog(cx));
        assert!(view.list_focus.is_focused(window));
    });
    checks.push(e7_record("actual Attachment card click opens the correct local image once; Escape closes and restores focus", "synthetic-gpui-pointer-and-key"));

    for (index, pdf) in [(0_usize, false), (1_usize, true)] {
        let generation = update(fixture, cx, |view, _, _| view.preview_generation);
        click_element(fixture, cx, ("attachment-preview", index));
        settle_overlay(cx).await;
        update(fixture, cx, |view, _, _| {
            assert_eq!(
                view.preview_generation,
                generation.wrapping_add(1),
                "action click must not also fire the card action"
            );
            if pdf {
                assert!(
                    matches!(&view.preview, Some(PreviewState::Pdf { filename }) if filename.as_ref() == "report.pdf")
                );
            } else {
                assert!(matches!(view.preview, Some(PreviewState::Image { .. })));
            }
        });
        press_key(fixture, cx, "escape");
        settle_overlay(cx).await;
    }
    checks.push(e7_record(
        "Attachment preview action avoids card double-dispatch and retains PDF fallback semantics",
        "synthetic-gpui-pointer-and-local-attachment",
    ));

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
        save_visual_prefs(fixture, cx, prefs);
        clear_business_feedback(fixture, cx).await;
        update(fixture, cx, |view, _, cx| {
            view.body_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        navigation_settle(fixture, cx).await;
        let path = update_window(fixture, cx, |window, cx| {
            capture_frame(window, cx, &format!("e7-content-{name}"))
        });
        checks.push(json!({"batch":"E7", "check":"E7 TextView/CID/table/Attachment theme screenshot", "theme":name,
            "evidence":"native-gpui-offscreen-screenshot", "path":path, "visually_reviewed":false}));
    }

    select_content_mail(fixture, cx, content.mailbox, content.literal).await;
    update(fixture, cx, |view, _, _| {
        let markup = content_markup(&view.selected_body_blocks);
        // MailStore retains the MIME parser's generated HTML alternative for
        // text/plain, so its wrapper may be a paragraph rather than <pre>.
        // The security/content contract is literal text, not one wrapper tag.
        assert!(markup.contains("# literal [link](https://tracker.example.invalid)"));
        assert!(markup.contains("&lt;script&gt;not executable&lt;/script&gt;"));
        assert!(markup.contains("&lt;img") && markup.contains("中文🙂 literal body"));
        assert!(!markup.contains("<img") && !markup.contains("<script") && !markup.contains("<a "));
        assert!(view.selected_body_cids.is_empty());
        assert!(view
            .selected_detail
            .as_ref()
            .unwrap()
            .attachments
            .is_empty());
    });
    update_window(fixture, cx, |window, _| {
        assert!(visible_content_part(window, "mail-text-region"))
    });
    checks.push(e7_record("real text/plain MIME stays literal, not Markdown/HTML/resources, and cannot reuse the previous message CID assets", "isolated-mime-and-native-textview"));

    select_content_mail(fixture, cx, content.mailbox, content.large).await;
    let count = update(fixture, cx, |view, _, _| {
        let count = view.selected_body_blocks.len();
        assert!(count > 100);
        assert_eq!(view.body_list.item_count(), count);
        let markup = content_markup(&view.selected_body_blocks);
        assert!(markup.contains("E7-LARGE-FIRST") && markup.contains("E7-LARGE-LAST"));
        count
    });
    update_window(fixture, cx, |window, _| {
        let visible = visible_body_blocks(window);
        assert!(
            !visible.is_empty() && visible.len() < 96,
            "large content must stay virtualized: {} / {count}",
            visible.len()
        );
    });
    checks.push(e7_record("12k-line Unicode MIME is complete yet only a bounded set of production reader blocks is rendered", "isolated-large-mime-and-native-virtual-list"));

    let before = update(fixture, cx, |view, _, _| {
        view.body_list.logical_scroll_top()
    });
    update_window(fixture, cx, |window, cx| {
        let bounds = element_snapshot(window, ("message-body", content.large as u64)).bounds();
        let position = gpui::point(
            bounds.origin.x + bounds.size.width / 2.,
            bounds.origin.y + bounds.size.height / 2.,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-480.))),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            }),
            cx,
        );
    });
    navigation_settle(fixture, cx).await;
    update(fixture, cx, |view, _, _| {
        let after = view.body_list.logical_scroll_top();
        assert_ne!(
            (after.item_ix, after.offset_in_item),
            (before.item_ix, before.offset_in_item),
            "native wheel must reach the reader, not a nested TextView scroller"
        );
        assert_eq!(view.selected_message, Some(content.large));
    });
    checks.push(e7_record(
        "native wheel input scrolls the outer body list without changing mail selection",
        "synthetic-gpui-wheel-and-production-list-state",
    ));

    // reveal_item uses cached height sums; unseen virtual rows do not have
    // final heights yet. The end anchor explicitly lays out backwards from
    // the tail, matching the official list's end-of-scroll contract.
    update(fixture, cx, |view, _, cx| {
        view.body_list.scroll_to_end();
        cx.notify();
    });
    navigation_settle(fixture, cx).await;
    update_window(fixture, cx, |window, _| {
        let visible = visible_body_blocks(window);
        assert!(
            visible.contains(&(count - 1)),
            "large message tail must be reachable: visible={visible:?}, total={count}"
        );
        assert!(visible.len() < 96);
    });
    let path = update_window(fixture, cx, |window, cx| {
        capture_frame(window, cx, "e7-large-body-tail")
    });
    checks.push(e7_record("virtualized long-body scrolling reaches the preserved final block without retaining every row", "native-gpui-list-scroll-and-layout"));
    checks.push(json!({"batch":"E7", "check":"E7 large Unicode message tail screenshot", "evidence":"native-gpui-offscreen-screenshot", "path":path, "visually_reviewed":false}));
    checks
}

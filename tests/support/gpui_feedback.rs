// E5 exercises production transitions with local channels and isolated files.
// No account/transport is created here; screenshots and GPUI events are not OS input.

pub(super) fn e5_record(check: &str, evidence: &str) -> Value {
    let mut record = overlay_evidence(check, evidence);
    record["batch"] = json!("E5");
    record
}

pub(super) async fn settle_feedback(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(350))
        .await;
}

pub(super) fn assert_business_toasts(window: &mut Window, cx: &mut gpui::App, expected: usize) {
    window.draw(cx).clear(cx);
    assert_eq!(
        window.notifications(cx).len(),
        expected,
        "Root notification count"
    );
    let rendered = gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|snapshot| {
            snapshot.path().last() == Some(&gpui::ElementId::from("notification"))
                && snapshot.visible()
                && snapshot.role() == Some(gpui::Role::Alert)
        })
        .count();
    assert_eq!(
        rendered, expected,
        "the official business Toast must actually be painted"
    );
}

async fn flush_business_feedback(fixture: &TestWindow, cx: &mut AsyncApp) {
    update_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
    settle(cx).await;
    update_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
    settle(cx).await;
}

async fn clear_business_feedback(fixture: &TestWindow, cx: &mut AsyncApp) {
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| window.clear_notifications(cx));
    settle_feedback(cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 0)
    });
    update(fixture, cx, |view, _, _| {
        assert!(view.feedback.pending().is_empty())
    });
}

async fn wait_sync_terminal(fixture: &TestWindow, cx: &mut AsyncApp) {
    for _ in 0..100 {
        if update(fixture, cx, |view, _, _| {
            view.sync.as_ref().is_some_and(|sync| !sync.is_busy())
        }) {
            flush_business_feedback(fixture, cx).await;
            return;
        }
        settle(cx).await;
    }
    panic!(
        "local sync task did not finish: {:?}",
        update(fixture, cx, |view, _, _| view.sync.clone())
    );
}

fn feedback_mail_ids(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<i64> {
    update(fixture, cx, |view, _, _| {
        let mailbox = first_sidebar_mailbox_id(&view.accounts).unwrap();
        view.store
            .as_ref()
            .unwrap()
            .message_page(mailbox, 0, MESSAGE_PAGE_SIZE)
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect()
    })
}

fn feedback_frame(fixture: &TestWindow, cx: &mut AsyncApp, name: &str, check: &str) -> Value {
    let path = update_window(fixture, cx, |window, cx| capture_frame(window, cx, name));
    json!({"batch": "E5", "check": check, "path": path,
        "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false})
}

pub async fn check_business_feedback(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    save_visual_prefs(
        fixture,
        cx,
        Prefs {
            scheme: ColorScheme::Dark,
            ..Prefs::default()
        },
    );
    clear_business_feedback(fixture, cx).await;
    let mut checks = check_sync_feedback(fixture, cx).await;
    checks.extend(check_attachment_feedback(fixture, cx).await);
    checks.extend(check_settings_feedback(fixture, cx).await);
    checks.extend(check_feedback_lifecycle(fixture, cx).await);
    checks
}

async fn check_sync_feedback(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    use backup::ProgressEvent;
    let mut checks = Vec::new();
    let ids = feedback_mail_ids(fixture, cx);
    let index_path = update(fixture, cx, |view, _, cx| {
        assert!(view.onboarding.is_none());
        view.select_message(ids[0], cx);
        view.paths.index.clone()
    });
    let root = index_path.parent().unwrap().to_path_buf();
    settle_overlay(cx).await;
    let generation = update(fixture, cx, |view, _, cx| {
        let generation = view.begin_sync(true, cx).unwrap();
        assert_eq!(view.begin_sync(false, cx), None);
        view.apply_progress(generation, ProgressEvent::MailboxesPlanned { count: 2 }, cx);
        view.apply_progress(
            generation,
            ProgressEvent::MailboxStart {
                name: "INBOX".into(),
                exists: 4,
                uids: 4,
            },
            cx,
        );
        view.apply_progress(
            generation,
            ProgressEvent::MessageStart {
                uid: 7,
                total: 0,
                resume: 12,
            },
            cx,
        );
        let (_, text, value) = view.sync.as_ref().unwrap().presentation();
        assert!(text.contains("大小未知"));
        assert_eq!(value, None);
        view.apply_progress(
            generation,
            ProgressEvent::MessageProgress {
                uid: 7,
                done: 125,
                total: 100,
            },
            cx,
        );
        assert_eq!(view.sync.as_ref().unwrap().presentation().2, Some(100.));
        view.apply_progress(
            generation,
            ProgressEvent::MessageStart {
                uid: 8,
                total: 200,
                resume: 80,
            },
            cx,
        );
        assert!(
            matches!(&view.sync, Some(SyncState::Message { mailbox, mailbox_index: 1, mailbox_total: 2, done: 80, .. }) if mailbox == "INBOX")
        );
        view.apply_progress(
            generation,
            ProgressEvent::MessageProgress {
                uid: 7,
                done: 1,
                total: 1,
            },
            cx,
        );
        assert_eq!(view.sync.as_ref().unwrap().presentation().2, Some(40.));
        assert!(view.feedback.pending().is_empty());
        generation
    });
    checks.push(e5_record(
        "sync unknown-size/clamped progress uses real events without a terminal notice",
        "business-state",
    ));
    checks.push(e5_record("sync preserves mailbox and resume offset across consecutive messages and ignores a stale UID", "business-state"));
    checks.push(e5_record(
        "busy sync cannot start another generation",
        "business-state",
    ));

    for (name, dark, custom) in [
        ("dark", true, false),
        ("light", false, false),
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
            prefs.custom_light.primary = 0x804AD0;
        }
        save_visual_prefs(fixture, cx, prefs);
        clear_business_feedback(fixture, cx).await;
        update_window(fixture, cx, |window, cx| {
            window.draw(cx).clear(cx);
            let region = element_snapshot(window, "sync-status-region");
            assert!(region.visible());
            assert!(region.bounds().size.height < px(150.));
        });
        checks.push(feedback_frame(
            fixture,
            cx,
            &format!("e5-sync-progress-{name}"),
            "E5 sync Alert/Progress theme capture",
        ));
    }

    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        let bounds = element_snapshot(window, "sync-status-region").bounds();
        // Official Alert does not expose its close div through test-support.
        // Hit its real close icon using the observed region and library padding.
        click_position(
            window,
            gpui::point(
                bounds.origin.x + bounds.size.width - px(24.),
                bounds.origin.y + px(21.),
            ),
            cx,
        );
    });
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert!(view.sync_status_hidden);
        assert_eq!(
            view.begin_sync(true, cx),
            None,
            "hiding does not cancel/unlock sync"
        );
        window.focus(&view.list_focus, cx);
    });
    cx.update(|cx| {
        cx.bind_keys([gpui::KeyBinding::new(
            "ctrl-shift-s",
            SyncNow,
            Some("MailApp"),
        )])
    });
    press_key(fixture, cx, "ctrl-shift-s");
    update(fixture, cx, |view, _, _| {
        assert!(!view.sync_status_hidden);
        assert_eq!(view.sync_generation, generation);
    });
    checks.push(e5_record("official sync Alert close hides only the status; synthetic Ctrl+Shift+S restores the same job", "synthetic-gpui-pointer-and-keys"));

    let new_eml = root.join("overlay-fixture-eml/fixture/INBOX/uidvalidity-1/0000000010.eml");
    std::fs::write(&new_eml, b"From: offline@example.invalid\r\nTo: test@example.invalid\r\nSubject: E5 offline imported\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nLocal import fixture.\r\n").unwrap();
    update(fixture, cx, |view, _, cx| {
        view.apply_progress(
            generation,
            ProgressEvent::Finished {
                backed_up: 1,
                skipped: 3,
            },
            cx,
        );
        assert!(matches!(
            view.sync,
            Some(SyncState::Indexing {
                backed_up: 1,
                skipped: 3
            })
        ));
        assert!(view.sync.as_ref().unwrap().is_busy());
        assert!(
            view.feedback.pending().is_empty(),
            "download completion is not sync success"
        );
        view.finish_sync_import(generation, Ok(()), cx);
        assert!(
            matches!(view.sync, Some(SyncState::Indexing { .. })),
            "unsolicited import completion is ignored"
        );
    });
    checks.push(e5_record(
        "download Finished stays busy/Indexing and emits no premature success",
        "business-state",
    ));
    checks.push(feedback_frame(
        fixture,
        cx,
        "e5-sync-indexing",
        "E5 indexing remains an informational loading state",
    ));
    update(fixture, cx, |view, _, cx| {
        view.finish_sync_with(generation, root.clone(), cx);
        view.finish_sync_with(generation, root.clone(), cx);
    });
    wait_sync_terminal(fixture, cx).await;
    update(fixture, cx, |view, _, _| {
        assert_eq!(
            view.sync,
            Some(SyncState::Done {
                backed_up: 1,
                skipped: 3
            })
        );
        let mailbox = first_sidebar_mailbox_id(&view.accounts).unwrap();
        let messages = view
            .store
            .as_ref()
            .unwrap()
            .message_page(mailbox, 0, MESSAGE_PAGE_SIZE)
            .unwrap();
        assert_eq!(messages.len(), 4);
        assert!(messages
            .iter()
            .any(|message| message.subject == "E5 offline imported"));
    });
    let notice_ids = update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1);
        window
            .notifications(cx)
            .iter()
            .map(|notice| notice.entity_id())
            .collect::<Vec<_>>()
    });
    update(fixture, cx, |view, _, cx| {
        view.finish_sync_import(generation, Err("duplicate completion".into()), cx);
        view.apply_progress(
            generation,
            ProgressEvent::Failed {
                message: "late worker failure".into(),
            },
            cx,
        );
        assert!(matches!(view.sync, Some(SyncState::Done { .. })));
        assert!(view.feedback.pending().is_empty());
    });
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_eq!(
            window
                .notifications(cx)
                .iter()
                .map(|notice| notice.entity_id())
                .collect::<Vec<_>>(),
            notice_ids
        );
    });
    checks.push(e5_record("real offline import/reload publishes one success; repeated/stale completions do not replace it", "business-callback-and-isolated-sqlite-eml"));
    clear_business_feedback(fixture, cx).await;

    update(fixture, cx, |view, _, cx| view.select_message(ids[0], cx));
    settle_overlay(cx).await;
    update(fixture, cx, |view, _, cx| {
        let before = (
            view.selected_message,
            view.detail_generation,
            view.message_pages.keys().copied().collect::<HashSet<_>>(),
        );
        let generation = view.begin_sync(false, cx).unwrap();
        view.apply_progress(
            generation,
            ProgressEvent::Finished {
                backed_up: 0,
                skipped: 4,
            },
            cx,
        );
        view.finish_sync_with(generation, root.clone(), cx);
        assert_eq!(
            (
                view.selected_message,
                view.detail_generation,
                view.message_pages.keys().copied().collect::<HashSet<_>>()
            ),
            before
        );
        assert!(view.selected_detail.is_some());
        assert!(view.sync_status_hidden);
        assert!(view.feedback.pending().is_empty());
    });
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 0)
    });
    checks.push(e5_record("automatic no-change sync preserves selection/details/pages and produces no repetitive success toast", "business-state-and-render"));

    let (sender, receiver) = std::sync::mpsc::channel();
    let disconnected_generation = update(fixture, cx, |view, _, cx| {
        let generation = view.begin_sync(true, cx).unwrap();
        view.pump_sync_events(generation, receiver, root.clone(), cx);
        generation
    });
    settle_feedback(cx).await;
    update(fixture, cx, |view, _, _| {
        assert_eq!(
            view.sync,
            Some(SyncState::Connecting),
            "an empty live channel is not a failure"
        );
        assert!(view.feedback.pending().is_empty());
    });
    drop(sender);
    wait_sync_terminal(fixture, cx).await;
    update(fixture, cx, |view, _, _| {
        assert!(
            matches!(&view.sync, Some(SyncState::Failed(error)) if error.contains("未返回完成状态"))
        )
    });
    let error_id = update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1);
        window.notifications(cx)[0].entity_id()
    });
    checks.push(e5_record("production event pump tolerates slow empty polls but fails a disconnected worker without Finished", "local-channel-and-business-callback"));

    let (sender, receiver) = std::sync::mpsc::channel();
    update(fixture, cx, |view, _, cx| {
        let next = view.begin_sync(true, cx).unwrap();
        assert_ne!(next, disconnected_generation);
        view.apply_progress(
            disconnected_generation,
            ProgressEvent::Failed {
                message: "old generation".into(),
            },
            cx,
        );
        assert_eq!(view.sync, Some(SyncState::Connecting));
        sender
            .send(ProgressEvent::Finished {
                backed_up: 0,
                skipped: 4,
            })
            .unwrap();
        view.pump_sync_events(next, receiver, root.clone(), cx);
    });
    drop(sender);
    wait_sync_terminal(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1);
        assert_ne!(
            window.notifications(cx)[0].entity_id(),
            error_id,
            "retry replaces the same business channel"
        );
    });
    checks.push(e5_record("retry ignores old-generation events and replaces the prior sync error instead of stacking notifications", "local-channel-and-business-callback"));
    clear_business_feedback(fixture, cx).await;

    let invalid_index = root.join("e5-index-is-a-directory");
    std::fs::create_dir(&invalid_index).unwrap();
    update(fixture, cx, |view, _, cx| {
        view.paths.index = invalid_index;
        let generation = view.begin_sync(true, cx).unwrap();
        view.apply_progress(
            generation,
            ProgressEvent::Finished {
                backed_up: 1,
                skipped: 0,
            },
            cx,
        );
        view.finish_sync_with(generation, root.clone(), cx);
    });
    wait_sync_terminal(fixture, cx).await;
    update(fixture, cx, |view, _, cx| {
        assert!(
            matches!(&view.sync, Some(SyncState::Failed(error)) if error.contains("索引入库失败"))
        );
        view.paths.index = index_path.clone();
        cx.notify();
    });
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record(
        "actual import/open failure after download completion reports failure, never success",
        "business-callback-and-isolated-filesystem",
    ));
    clear_business_feedback(fixture, cx).await;

    update(fixture, cx, |view, _, cx| {
        let generation = view.begin_sync(true, cx).unwrap();
        view.apply_progress(
            generation,
            ProgressEvent::Finished {
                backed_up: 1,
                skipped: 0,
            },
            cx,
        );
        view.sync_import_started = true; // Inject only the worker completion; exercise the real reload.
        view.paths.index = root.join("e5-missing-reload.sqlite");
        view.finish_sync_import(generation, Ok(()), cx);
        assert!(matches!(view.sync, Some(SyncState::Failed(_))));
        assert_eq!(view.feedback.pending()[0].level, NoticeLevel::Error);
        view.paths.index = index_path.clone();
        view.load_error = None;
        cx.notify();
    });
    checks.push(e5_record(
        "successful worker completion with a failed real index reload is not a sync success",
        "injected-completion-and-isolated-filesystem",
    ));
    clear_business_feedback(fixture, cx).await;

    update(fixture, cx, |view, window, cx| {
        assert!(
            account_config::load().unwrap().is_none(),
            "fixture must never have a usable account"
        );
        view.request_sync(false, cx);
        assert!(view.feedback.pending().is_empty());
        view.request_sync(true, cx);
        assert_eq!(view.feedback.pending()[0].level, NoticeLevel::Warning);
        let generation = view.begin_sync(true, cx).unwrap();
        view.fail_sync(generation, "offline connect failure", cx);
        view.return_to_onboarding(window, cx);
        view.apply_progress(
            generation,
            ProgressEvent::Finished {
                backed_up: 9,
                skipped: 0,
            },
            cx,
        );
        assert!(view.sync.is_none());
        assert!(view.onboarding.is_some());
        view.onboarding = None;
        cx.notify();
    });
    checks.push(e5_record("unconfigured automatic poll stays quiet; manual sync explains why; onboarding retry invalidates old callbacks", "business-state-no-account"));
    clear_business_feedback(fixture, cx).await;
    checks
}

async fn wait_attachment_terminal(fixture: &TestWindow, cx: &mut AsyncApp) {
    for _ in 0..100 {
        if update(fixture, cx, |view, _, _| {
            !matches!(view.save_state, Some(SaveState::Saving))
        }) {
            flush_business_feedback(fixture, cx).await;
            return;
        }
        settle(cx).await;
    }
    panic!("offline attachment save did not finish");
}

async fn check_attachment_feedback(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    save_visual_prefs(
        fixture,
        cx,
        Prefs {
            scheme: ColorScheme::Dark,
            ..Prefs::default()
        },
    );
    clear_business_feedback(fixture, cx).await;
    let ids = feedback_mail_ids(fixture, cx);
    // The newly imported plain-text mail has no attachment; use the original MIME fixture.
    let message_id = update(fixture, cx, |view, _, _| {
        ids.iter()
            .copied()
            .find(|id| {
                view.store
                    .as_ref()
                    .unwrap()
                    .message_detail(*id)
                    .unwrap()
                    .unwrap()
                    .attachments
                    .len()
                    == 2
            })
            .unwrap()
    });
    let other_id = *ids.iter().find(|id| **id != message_id).unwrap();
    let root = update(fixture, cx, |view, _, cx| {
        view.attachment_pane_rem = 18.;
        view.select_message(message_id, cx);
        let before = (view.save_state.clone(), view.save_generation);
        view.save_attachment_to(message_id, 1, None, cx);
        assert_eq!((view.save_state.clone(), view.save_generation), before);
        assert!(view.feedback.pending().is_empty());
        view.paths
            .index
            .parent()
            .unwrap()
            .join("e5-saved-attachments")
    });
    settle_overlay(cx).await;
    update(fixture, cx, |view, window, cx| {
        view.reader_split.update(cx, |state, cx| {
            assert_eq!(state.sizes().len(), 2);
            state.resize_panel(1, window.rem_size() * 18., window, cx);
        });
    });
    settle_overlay(cx).await;
    checks.push(e5_record(
        "cancelled attachment file dialog does not change state or notify",
        "business-state",
    ));
    let target = root.join("report.txt");
    let blocked_reentry = root.join("must-not-save.txt");
    let saved_generation = update(fixture, cx, |view, _, cx| {
        view.save_attachment_to(message_id, 1, Some(target.clone()), cx);
        let generation = view.save_generation;
        assert_eq!(view.save_state, Some(SaveState::Saving));
        view.save_attachment_to(message_id, 1, Some(blocked_reentry.clone()), cx);
        assert_eq!(view.save_generation, generation);
        view.select_message(other_id, cx);
        assert_eq!(view.save_message, Some(message_id));
        assert!(
            view.render_attachment_save_status(cx).is_some(),
            "busy status explains another message owns the save"
        );
        generation
    });
    wait_attachment_terminal(fixture, cx).await;
    assert!(String::from_utf8(std::fs::read(&target).unwrap())
        .unwrap()
        .contains("Offline attachment."));
    assert!(!blocked_reentry.exists());
    update(fixture, cx, |view, _, cx| {
        assert_eq!(view.save_state, Some(SaveState::Done("report.txt".into())));
        assert!(
            view.render_attachment_save_status(cx).is_none(),
            "previous message result must not appear in the new message"
        );
        view.complete_attachment_save(
            saved_generation,
            "late.txt".into(),
            Err(AttachmentSaveError::Read("duplicate result".into())),
            cx,
        );
        assert_eq!(view.save_state, Some(SaveState::Done("report.txt".into())));
        assert!(view.feedback.pending().is_empty());
        view.select_message(message_id, cx);
    });
    settle_feedback(cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1);
        assert!(element_snapshot(window, "attachment-save-status-region").visible());
    });
    checks.push(e5_record("actual MIME attachment saves once; selection changes and duplicate callbacks cannot misattribute its result", "business-callback-and-isolated-filesystem"));
    checks.push(feedback_frame(
        fixture,
        cx,
        "e5-attachment-saved",
        "E5 attachment saved status and real business notification",
    ));
    update_window(fixture, cx, |window, cx| {
        let bounds = element_snapshot(window, "attachment-save-status-region").bounds();
        click_position(
            window,
            gpui::point(
                bounds.origin.x + bounds.size.width - px(24.),
                bounds.origin.y + px(21.),
            ),
            cx,
        );
    });
    settle(cx).await;
    update(fixture, cx, |view, _, _| assert!(view.save_state.is_none()));
    checks.push(e5_record(
        "official attachment Alert can be dismissed without losing the saved file",
        "synthetic-gpui-pointer",
    ));
    clear_business_feedback(fixture, cx).await;

    let unwritable = root.join("target-is-a-directory");
    std::fs::create_dir(&unwritable).unwrap();
    update(fixture, cx, |view, _, cx| {
        view.save_attachment_to(message_id, 1, Some(unwritable.clone()), cx)
    });
    wait_attachment_terminal(fixture, cx).await;
    let failed_generation = update(fixture, cx, |view, _, _| {
        assert!(
            matches!(&view.save_state, Some(SaveState::Failed(error)) if error.contains("target-is-a-directory")
            && error.contains("无法写入") && !error.contains("重新生成索引"))
        );
        let log = std::fs::read_to_string(&view.paths.log).unwrap();
        assert!(log.contains("event=attachment_write_failed io_kind="));
        view.save_generation
    });
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record(
        "actual attachment write failure retains an inline error and a single failure notification",
        "business-callback-and-isolated-filesystem",
    ));
    checks.push(feedback_frame(
        fixture,
        cx,
        "e5-attachment-failed",
        "E5 attachment failure status",
    ));
    let retried = root.join("retry.txt");
    update(fixture, cx, |view, _, cx| {
        view.save_attachment_to(message_id, 1, Some(retried.clone()), cx);
        view.complete_attachment_save(failed_generation, "old.txt".into(), Ok(()), cx);
        assert_eq!(view.save_state, Some(SaveState::Saving));
    });
    wait_attachment_terminal(fixture, cx).await;
    assert!(retried.is_file());
    update(fixture, cx, |view, _, cx| {
        assert_eq!(view.save_state, Some(SaveState::Done("retry.txt".into())));
        let before = view.save_state.clone();
        view.save_attachment_to(message_id, 1, None, cx);
        assert_eq!(view.save_state, before);
    });
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record("attachment retry rejects old generations, replaces the failure toast and preserves state on later cancel", "business-callback-and-isolated-filesystem"));
    clear_business_feedback(fixture, cx).await;
    checks
}

async fn check_settings_feedback(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    update(fixture, cx, |view, window, cx| {
        view.open_settings(window, cx);
    });
    settle(cx).await;
    update_window(fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        // The migrated segmented control is an official TabBar: individual
        // tabs use their index under the group's id, not legacy button ids.
        let tab = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|snapshot| {
                snapshot.role() == Some(gpui::Role::Tab)
                    && snapshot
                        .path()
                        .contains(&gpui::ElementId::from("pal-default"))
                    && snapshot.path().last() == Some(&gpui::ElementId::from(4usize))
            })
            .expect("Custom palette tab");
        assert!(tab.visible());
        let bounds = tab.bounds();
        click_position(
            window,
            gpui::point(
                bounds.origin.x + bounds.size.width / 2.,
                bounds.origin.y + bounds.size.height / 2.,
            ),
            cx,
        );
    });
    settle(cx).await;
    update(fixture, cx, |view, window, cx| {
        assert_eq!(
            view.settings.as_ref().unwrap().draft.palette,
            Palette::Custom
        );
        let input = view.settings.as_ref().unwrap().hex_input.clone();
        input.update(cx, |input, cx| input.replace_all("invalid hex", window, cx));
    });
    settle(cx).await;
    click_element(fixture, cx, "settings-save");
    update(fixture, cx, |view, window, cx| {
        assert!(view.settings.as_ref().unwrap().hex_invalid);
        assert!(view.feedback.pending().is_empty());
        view.close_settings(window, cx);
        assert!(view.feedback.pending().is_empty());
    });
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 0)
    });
    checks.push(e5_record(
        "invalid HEX and settings cancel cannot publish a saved notification",
        "synthetic-gpui-pointer-and-business-state",
    ));
    let path =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("foxmail-rs/prefs.json");
    let backup = path.with_extension("e5-fixture-backup");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    update(fixture, cx, |view, window, cx| {
        view.open_settings(window, cx);
        let desired = !view.prefs.sidebar_mono_icons;
        view.set_settings_flag("sidebar-mono", desired, cx);
        view.save_settings(window, cx);
        view.save_settings(window, cx);
        assert_ne!(view.settings.as_ref().unwrap().draft, view.prefs);
        assert_eq!(view.feedback.pending().len(), 1);
        assert_eq!(view.feedback.pending()[0].level, NoticeLevel::Error);
    });
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1)
    });
    clear_business_feedback(fixture, cx).await;
    update(fixture, cx, |view, window, cx| {
        view.save_settings(window, cx);
        assert!(view.settings.as_ref().unwrap().save_error.is_some());
        assert!(
            view.feedback.pending().is_empty(),
            "same error in this editing session is not repeated after dismissal"
        );
    });
    checks.push(e5_record("real settings persistence error keeps the draft and deduplicates repeated attempts after dismissal", "business-callback-and-isolated-filesystem"));
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&backup, &path).unwrap();
    update(fixture, cx, |view, window, cx| {
        let expected = view.settings.as_ref().unwrap().draft.clone();
        view.save_settings(window, cx);
        assert!(view.settings.is_none());
        assert_eq!(prefs::load().unwrap(), expected);
        assert_eq!(view.feedback.pending().len(), 1);
        assert_eq!(view.feedback.pending()[0].level, NoticeLevel::Success);
    });
    flush_business_feedback(fixture, cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record("settings retry reports success only after disk persistence and applying the retained draft", "business-callback-and-isolated-filesystem"));
    clear_business_feedback(fixture, cx).await;
    checks
}

async fn check_feedback_lifecycle(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    update(fixture, cx, |view, window, cx| {
        let generation = view.begin_sync(true, cx).unwrap();
        view.fail_sync(generation, "离线同步结果：连接不可用", cx);
        let message = view.selected_message.unwrap();
        view.reject_attachment_save(message, "离线附件结果：文件不可写", cx);
        view.open_settings(window, cx);
        view.save_settings(window, cx);
        assert_eq!(view.feedback.pending().len(), 3);
    });
    flush_business_feedback(fixture, cx).await;
    settle_feedback(cx).await;
    update_window(fixture, cx, |window, cx| {
        assert_business_toasts(window, cx, 3)
    });
    let screenshot = feedback_frame(
        fixture,
        cx,
        "e5-independent-notifications",
        "E5 independent sync, attachment and settings notifications",
    );
    update_window(fixture, cx, |window, cx| {
        for notice in window.notifications(cx).iter() {
            notice.update(cx, |notice, cx| notice.dismiss(window, cx));
        }
    });
    settle_feedback(cx).await;
    for _ in 0..3 {
        update(fixture, cx, |_, _, cx| cx.notify());
        flush_business_feedback(fixture, cx).await;
        update_window(fixture, cx, |window, cx| {
            assert_business_toasts(window, cx, 0)
        });
    }
    vec![
        e5_record(
            "independent business channels coexist; official dismiss survives subsequent redraws",
            "business-state-render-and-official-dismiss",
        ),
        screenshot,
    ]
}

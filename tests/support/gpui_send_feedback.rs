// Included inside the real compose module. The only send adapter used by these
// checks is a local Task fed by an mpsc channel; never call attempt_send with
// a usable/nonempty recipient. All draft I/O stays in the isolated profile.

struct SendFixture {
    handle: AnyWindowHandle,
    view: Entity<ComposeWindow>,
}

fn open_send_fixture(cx: &mut App, id: i64) -> SendFixture {
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
                let entity = cx.new(|cx| {
                    ComposeWindow::new(
                        ComposeKind::Reply(ReplyDraft {
                            message_id: id,
                            recipient: "receiver@example.invalid".into(),
                            subject: "E5 isolated send fixture".into(),
                            quoted_body: "离线邮件正文。".into(),
                        }),
                        window,
                        cx,
                    )
                });
                view = Some(entity.clone());
                crate::ui::root_view(entity, window, cx)
            },
        )
        .unwrap();
    let fixture = SendFixture {
        handle: handle.into(),
        view: view.unwrap(),
    };
    fixture
        .handle
        .update(cx, |_, window, _| window.resize(size(px(720.), px(560.))))
        .unwrap();
    fixture
}

fn send_window<R>(
    fixture: &SendFixture,
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    fixture
        .handle
        .update(cx, |_, window, cx| f(window, cx))
        .unwrap()
}

fn send_update<R>(
    fixture: &SendFixture,
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut ComposeWindow, &mut Window, &mut Context<ComposeWindow>) -> R,
) -> R {
    send_window(fixture, cx, |window, cx| {
        fixture.view.update(cx, |view, cx| f(view, window, cx))
    })
}

async fn flush_send_feedback(fixture: &SendFixture, cx: &mut AsyncApp) {
    send_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
    cx.background_executor()
        .timer(Duration::from_millis(30))
        .await;
    send_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
    cx.background_executor()
        .timer(Duration::from_millis(30))
        .await;
}

async fn clear_send_feedback(fixture: &SendFixture, cx: &mut AsyncApp) {
    flush_send_feedback(fixture, cx).await;
    send_window(fixture, cx, |window, cx| window.clear_notifications(cx));
    crate::app::settle_feedback(cx).await;
    send_window(fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 0)
    });
    send_update(fixture, cx, |view, _, _| {
        assert!(view.feedback.pending().is_empty())
    });
}

fn send_draft_path(fixture: &SendFixture, cx: &mut AsyncApp) -> std::path::PathBuf {
    let key = send_update(fixture, cx, |view, _, _| draft_key(&view.kind));
    std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
        .join("foxmail-rs/drafts")
        .join(format!("{key}.json"))
}

fn edit_send_body(fixture: &SendFixture, cx: &mut AsyncApp, value: &str) {
    send_window(fixture, cx, |window, cx| {
        let body = fixture.view.read(cx).body.clone();
        body.update(cx, |input, cx| {
            input.set_value("", window, cx);
            // This emits the real InputEvent::Change consumed by autosave.
            input.replace_text_in_range(None, value, window, cx);
        });
    });
}

fn begin_local_send(
    fixture: &SendFixture,
    cx: &mut AsyncApp,
) -> (
    u64,
    DraftData,
    std::sync::mpsc::Sender<Result<String, String>>,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let (generation, snapshot) = send_update(fixture, cx, |view, window, cx| {
        let (generation, snapshot) = view.begin_send(window, cx).expect("valid offline snapshot");
        let task = cx.background_executor().spawn(async move {
            receiver
                .recv()
                .unwrap_or_else(|_| Err("local test result channel closed".into()))
        });
        view.dispatch_send(generation, snapshot.clone(), task, cx);
        (generation, snapshot)
    });
    (generation, snapshot, sender)
}

async fn wait_send_terminal(fixture: &SendFixture, cx: &mut AsyncApp) {
    for _ in 0..100 {
        if send_update(fixture, cx, |view, _, _| {
            !matches!(view.send_state, SendState::Sending { .. })
        }) {
            flush_send_feedback(fixture, cx).await;
            return;
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
    panic!("local send result did not reach the production callback");
}

async fn close_send_fixture(fixture: SendFixture, cx: &mut AsyncApp) {
    clear_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, window, cx| {
        view.close_window(&CloseCompose, window, cx)
    });
    drop(fixture);
    crate::app::settle_feedback(cx).await;
    cx.update(|_| {});
}

fn send_feedback_frame(fixture: &SendFixture, cx: &mut AsyncApp, name: &str, check: &str) -> Value {
    let path = send_window(fixture, cx, |window, cx| {
        crate::app::capture_frame(window, cx, name)
    });
    json!({"batch": "E5", "check": check, "path": path,
        "evidence": "native-gpui-offscreen-screenshot", "visually_reviewed": false})
}

fn mark_send_input<T: EntityInputHandler>(
    input: &Entity<T>,
    mark: bool,
    window: &mut Window,
    cx: &mut App,
) {
    input.update(cx, |input, cx| {
        if mark {
            input.replace_and_mark_text_in_range(None, "zhong", Some(0..5), window, cx);
        } else {
            input.unmark_text(window, cx);
        }
    });
}

pub async fn check_send_feedback(cx: &mut AsyncApp) -> Vec<Value> {
    use crate::app::e5_record;
    let saved_prefs = crate::prefs::cached();
    let mut checks = Vec::new();
    let fixture = cx.update(|cx| open_send_fixture(cx, 95001));
    crate::app::settle_feedback(cx).await;
    send_update(&fixture, cx, |view, window, cx| {
        view.to
            .update(cx, |input, cx| input.set_value("", window, cx));
        assert!(view.begin_send(window, cx).is_none());
        assert!(matches!(&view.send_state, SendState::Failed(error) if error == "收件人不能为空"));
        assert_eq!(view.feedback.pending().len(), 1);
    });
    flush_send_feedback(&fixture, cx).await;
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1)
    });
    clear_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, window, cx| {
        assert!(view.begin_send(window, cx).is_none());
        assert!(
            view.feedback.pending().is_empty(),
            "unchanged validation failure must not resurrect a dismissed toast"
        );
        view.send_state = SendState::Idle;
        let body = view.body.clone();
        window.focus(&body.read(cx).focus_handle(cx), cx);
        mark_send_input(&body, true, window, cx);
    });
    cx.update(|cx| {
        cx.bind_keys([
            gpui::KeyBinding::new("ctrl-enter", SendCompose, Some("Compose")),
            input_send_binding(),
        ])
    });
    send_window(&fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        assert!(window.dispatch_keystroke(Keystroke::parse("ctrl-enter").unwrap(), cx));
    });
    send_update(&fixture, cx, |view, window, cx| {
        assert!(
            view.to.read(cx).value().is_empty(),
            "keyboard test cannot reach SMTP even on a guard regression"
        );
        assert!(matches!(view.send_state, SendState::Idle));
        mark_send_input(&view.body, false, window, cx);
        view.to.update(cx, |input, cx| {
            input.set_value("receiver@example.invalid", window, cx)
        });
    });
    checks.push(e5_record(
        "send validation is deduplicated; composing-body synthetic Ctrl+Enter does not leave idle",
        "business-state-and-synthetic-gpui-key-NOT-native-IME",
    ));

    for field in 0..4 {
        send_update(&fixture, cx, |view, window, cx| {
            match field {
                0 => mark_send_input(&view.to, true, window, cx),
                1 => mark_send_input(&view.cc, true, window, cx),
                2 => mark_send_input(&view.subject, true, window, cx),
                _ => mark_send_input(&view.body, true, window, cx),
            }
            // begin_send only validates/snapshots. It never creates SMTP, even
            // if this assertion finds a regressed composition guard.
            assert!(view.begin_send(window, cx).is_none());
            assert!(matches!(view.send_state, SendState::Idle));
            match field {
                0 => mark_send_input(&view.to, false, window, cx),
                1 => mark_send_input(&view.cc, false, window, cx),
                2 => mark_send_input(&view.subject, false, window, cx),
                _ => mark_send_input(&view.body, false, window, cx),
            }
        });
    }
    send_update(&fixture, cx, |view, window, cx| {
        view.to.update(cx, |input, cx| {
            input.set_value("receiver@example.invalid", window, cx)
        });
        view.cc
            .update(cx, |input, cx| input.set_value("", window, cx));
        view.body
            .update(cx, |input, cx| input.set_value("", window, cx));
        assert!(view.begin_send(window, cx).is_none());
        assert!(matches!(&view.send_state, SendState::Failed(error) if error == "正文不能为空"));
    });
    checks.push(e5_record(
        "all four composer fields guard marked text; empty body is rejected without a transport",
        "injected-marked-text-and-business-state-NOT-native-IME",
    ));
    clear_send_feedback(&fixture, cx).await;
    edit_send_body(&fixture, cx, "离线待发送正文。\n第二行。");
    crate::app::settle_feedback(cx).await;
    let (failed_generation, sent_snapshot, sender) = begin_local_send(&fixture, cx);
    send_update(&fixture, cx, |view, window, cx| {
        assert!(
            view.begin_send(window, cx).is_none(),
            "a send already in flight cannot be reentered"
        );
        assert_eq!(view.send_generation, failed_generation);
        assert_eq!(
            draft_store::load(&draft_key(&view.kind)).unwrap(),
            Some(sent_snapshot.clone())
        );
    });
    checks.push(e5_record(
        "begin_send persists the transmitted snapshot and prevents duplicate in-flight requests",
        "business-state-and-isolated-draft",
    ));

    for (name, dark, custom) in [
        ("dark", true, false),
        ("light", false, false),
        ("custom-dark", true, true),
        ("custom-light", false, true),
    ] {
        send_update(&fixture, cx, |_, _, cx| {
            let mut prefs = crate::prefs::Prefs {
                scheme: if dark {
                    crate::prefs::ColorScheme::Dark
                } else {
                    crate::prefs::ColorScheme::Light
                },
                palette: if custom {
                    crate::prefs::Palette::Custom
                } else {
                    crate::prefs::Palette::Default
                },
                ..crate::prefs::Prefs::default()
            };
            if custom {
                prefs.custom_dark.primary = 0x4ABCA0;
                prefs.custom_light.primary = 0x804AD0;
            }
            crate::prefs::apply(prefs.clone()).unwrap();
            crate::fox_theme::apply_foxmail_theme(Theme::from_prefs(&prefs), cx);
            cx.notify();
        });
        crate::app::settle_feedback(cx).await;
        send_window(&fixture, cx, |window, cx| {
            window.draw(cx).clear(cx);
            let region = crate::app::element_snapshot(window, "send-status-region");
            assert!(region.visible());
            assert!(region.bounds().size.height <= px(96.));
            assert!(crate::app::element_snapshot(window, "send-button").visible());
        });
        checks.push(send_feedback_frame(
            &fixture,
            cx,
            &format!("e5-send-progress-{name}"),
            "E5 composer sending Alert/Progress theme capture",
        ));
    }
    edit_send_body(
        &fixture,
        cx,
        "发送期间的新修改：不要丢失。\nemoji 😀 与中文。",
    );
    crate::app::settle_feedback(cx).await;
    let edited = send_update(&fixture, cx, |view, _, cx| current_draft(view, cx));
    assert_ne!(edited, sent_snapshot);
    sender
        .send(Err("离线测试：SMTP 拒绝（本地任务，没有连接服务器）".into()))
        .unwrap();
    drop(sender);
    wait_send_terminal(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(matches!(view.send_state, SendState::Failed(_)));
        assert_eq!(
            draft_store::load(&draft_key(&view.kind)).unwrap(),
            Some(edited)
        );
    });
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record("local send failure reaches the production callback once and keeps edits made while sending", "local-task-business-callback-and-isolated-draft"));
    checks.push(send_feedback_frame(
        &fixture,
        cx,
        "e5-send-failed",
        "E5 send failure status and notification",
    ));

    let (success_generation, _, sender) = begin_local_send(&fixture, cx);
    send_update(&fixture, cx, |view, _, cx| {
        view.complete_send(failed_generation, Ok("stale success".into()), cx);
        assert!(
            matches!(view.send_state, SendState::Sending { generation, .. } if generation == success_generation)
        );
    });
    sender.send(Ok("离线服务器确认".into())).unwrap();
    drop(sender);
    wait_send_terminal(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(matches!(
            view.send_state,
            SendState::Success { warning: None, .. }
        ));
        assert!(draft_store::load(&draft_key(&view.kind)).unwrap().is_none());
    });
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record("successful retry replaces the failure notification and clears only the unchanged sent draft", "local-task-business-callback-and-isolated-draft"));
    clear_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, cx| {
        view.complete_send(success_generation, Err("duplicate failure".into()), cx);
        assert!(matches!(view.send_state, SendState::Success { .. }));
        assert!(view.feedback.pending().is_empty());
    });
    flush_send_feedback(&fixture, cx).await;
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 0)
    });
    checks.push(e5_record(
        "duplicate send completion and redraw cannot resurrect a dismissed result",
        "business-state-render-and-official-dismiss",
    ));

    let (_, sent, sender) = begin_local_send(&fixture, cx);
    edit_send_body(&fixture, cx, "发送成功后仍需保留的下一稿。");
    crate::app::settle_feedback(cx).await;
    let next_draft = send_update(&fixture, cx, |view, _, cx| current_draft(view, cx));
    assert_ne!(next_draft, sent);
    sender.send(Ok("离线发送成功".into())).unwrap();
    drop(sender);
    wait_send_terminal(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(
            matches!(&view.send_state, SendState::Success { detail, warning: None } if detail.contains("新修改未被清空"))
        );
        assert_eq!(
            draft_store::load(&draft_key(&view.kind)).unwrap(),
            Some(next_draft)
        );
    });
    checks.push(e5_record(
        "successful send preserves newer edits in the still-open composer",
        "local-task-business-callback-and-isolated-draft",
    ));
    clear_send_feedback(&fixture, cx).await;

    let (_, _, sender) = begin_local_send(&fixture, cx);
    let other = cx.update(|cx| open_send_fixture(cx, 95001));
    crate::app::settle_feedback(cx).await;
    edit_send_body(&other, cx, "另一个写信窗口保存的新草稿。");
    crate::app::settle_feedback(cx).await;
    let other_draft = send_update(&other, cx, |view, _, cx| current_draft(view, cx));
    sender.send(Ok("离线发送成功".into())).unwrap();
    drop(sender);
    wait_send_terminal(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(matches!(
            view.send_state,
            SendState::Success { warning: None, .. }
        ));
        assert_eq!(
            draft_store::load(&draft_key(&view.kind)).unwrap(),
            Some(other_draft)
        );
    });
    checks.push(e5_record("send cleanup cannot delete a newer draft saved by another live composer of the same message", "cross-window-business-callback-and-isolated-draft"));
    close_send_fixture(other, cx).await;
    clear_send_feedback(&fixture, cx).await;

    let (_, _, sender) = begin_local_send(&fixture, cx);
    let draft_path = send_draft_path(&fixture, cx);
    #[cfg(target_os = "windows")]
    let cleanup_lock = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // Permit reads/writes but deny deletion: clear_sent_draft can compare
        // the snapshot, then encounters a real Windows removal error.
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&draft_path)
            .unwrap()
    };
    #[cfg(not(target_os = "windows"))]
    std::fs::write(&draft_path, b"intentionally invalid isolated draft JSON").unwrap();
    sender.send(Ok("服务器已确认发送".into())).unwrap();
    drop(sender);
    wait_send_terminal(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(
            matches!(&view.send_state, SendState::Success { warning: Some(warning), .. }
            if warning.contains("草稿清理失败") && warning.contains("请勿重复发送"))
        );
        assert!(draft_path.is_file());
    });
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1)
    });
    checks.push(e5_record("draft-cleanup I/O/parse failure after server success stays Sent with a warning, never SendFailed", "local-task-business-callback-and-isolated-filesystem"));
    send_window(&fixture, cx, |window, _| {
        window.resize(size(px(420.), px(440.)))
    });
    crate::app::settle_feedback(cx).await;
    send_window(&fixture, cx, |window, cx| {
        window.draw(cx).clear(cx);
        let bounds = crate::app::element_snapshot(window, "send-status-region").bounds();
        assert!(bounds.size.height <= px(96.));
        assert!(bounds.origin.x >= px(0.));
        assert!(bounds.origin.x + bounds.size.width <= window.viewport_size().width);
    });
    checks.push(send_feedback_frame(
        &fixture,
        cx,
        "e5-send-cleanup-warning-small",
        "E5 sent-with-warning status remains bounded in a small composer",
    ));
    clear_send_feedback(&fixture, cx).await;

    #[cfg(target_os = "windows")]
    drop(cleanup_lock);
    std::fs::remove_file(&draft_path).unwrap();
    std::fs::create_dir(&draft_path).unwrap();
    edit_send_body(&fixture, cx, "隔离草稿写入失败：窗口里仍保留正文。");
    crate::app::settle_feedback(cx).await;
    flush_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, _| {
        assert!(view.draft_error.is_some())
    });
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1);
        assert!(crate::app::element_snapshot(window, "draft-save-status-region").visible());
    });
    checks.push(send_feedback_frame(
        &fixture,
        cx,
        "e5-draft-save-error-small",
        "E5 autosave failure is visible instead of silently discarded",
    ));
    clear_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, cx| {
        view.save_current_draft(cx);
        assert!(view.draft_error.is_some());
        assert!(view.feedback.pending().is_empty());
    });
    checks.push(e5_record("actual Input Change autosave failure keeps visible text and does not repeat the same error after dismissal", "synthetic-input-business-subscription-and-isolated-filesystem"));
    std::fs::remove_dir(&draft_path).unwrap();
    edit_send_body(&fixture, cx, "草稿保存已恢复。");
    crate::app::settle_feedback(cx).await;
    flush_send_feedback(&fixture, cx).await;
    send_update(&fixture, cx, |view, _, cx| {
        assert!(view.draft_error.is_none());
        assert_eq!(
            draft_store::load(&draft_key(&view.kind)).unwrap(),
            Some(current_draft(view, cx))
        );
    });
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 1)
    });
    clear_send_feedback(&fixture, cx).await;
    edit_send_body(&fixture, cx, "普通继续输入，不应每个按键都弹成功通知。");
    crate::app::settle_feedback(cx).await;
    flush_send_feedback(&fixture, cx).await;
    send_window(&fixture, cx, |window, cx| {
        crate::app::assert_business_toasts(window, cx, 0)
    });
    checks.push(e5_record(
        "autosave recovery emits one informational notice; subsequent normal edits remain quiet",
        "synthetic-input-business-subscription-and-isolated-filesystem",
    ));
    close_send_fixture(fixture, cx).await;
    checks.extend(check_send_after_close(cx).await);
    cx.update(|cx| {
        crate::prefs::apply(saved_prefs.clone()).unwrap();
        crate::fox_theme::apply_foxmail_theme(Theme::from_prefs(&saved_prefs), cx);
    });
    checks
}

async fn check_send_after_close(cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks = Vec::new();
    for (id, newer, succeeds) in [
        (95002, false, true),
        (95003, true, true),
        (95004, false, false),
    ] {
        let fixture = cx.update(|cx| open_send_fixture(cx, id));
        crate::app::settle_feedback(cx).await;
        let (_, snapshot, sender) = begin_local_send(&fixture, cx);
        let path = send_draft_path(&fixture, cx);
        let key = send_update(&fixture, cx, |view, _, _| draft_key(&view.kind));
        let weak = fixture.view.downgrade();
        close_send_fixture(fixture, cx).await;
        assert!(
            weak.upgrade().is_none(),
            "closed-window path must not secretly retain the composer"
        );
        let retained = if newer {
            let new_draft = DraftData {
                body: "窗口关闭后保存的更新草稿。".into(),
                ..snapshot.clone()
            };
            draft_store::save(&key, &new_draft).unwrap();
            new_draft
        } else {
            snapshot
        };
        sender
            .send(if succeeds {
                Ok("offline completed after close".into())
            } else {
                Err("offline failed after close".into())
            })
            .unwrap();
        drop(sender);
        crate::app::settle_feedback(cx).await;
        if succeeds && !newer {
            for _ in 0..100 {
                if !path.exists() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(30))
                    .await;
            }
            assert!(
                !path.exists(),
                "unchanged successfully sent draft should be removed even after close"
            );
        } else {
            assert_eq!(draft_store::load(&key).unwrap(), Some(retained));
        }
        checks.push(crate::app::e5_record(
            match (newer, succeeds) {
                (false, true) => {
                    "closed composer success clears only its unchanged transmitted snapshot"
                }
                (true, _) => "closed composer success cannot remove a newer saved draft",
                _ => "closed composer failure retains the unsent draft",
            },
            "closed-window-local-task-and-isolated-draft",
        ));
    }
    checks
}

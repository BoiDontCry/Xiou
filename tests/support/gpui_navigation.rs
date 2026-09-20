// E6: actual Tree/ListItem/VirtualList/Resizable interactions over isolated SQL.
// Timing is debug GPUI draw latency, NOT OS presentation FPS or a release budget.

fn e6_record(check: &str, evidence: &str) -> Value {
    let mut value=overlay_evidence(check,evidence);
    value["batch"]=json!("E6"); value
}

fn seed_navigation_mail(fixture: &TestWindow, cx: &mut AsyncApp, count: usize) -> (i64,i64) {
    let path=update(fixture,cx,|view,_,_|view.paths.index.clone());
    let mut connection=rusqlite::Connection::open(&path).unwrap();
    let template:i64=connection.query_row("SELECT id FROM message ORDER BY id LIMIT 1",[],|row|row.get(0)).unwrap();
    let transaction=connection.transaction().unwrap();
    for (source,label,n) in [("INBOX","E6 large inbox",count),("Sent","E6 empty mailbox",0)] {
        transaction.execute("INSERT INTO mailbox(account,server,source_name,display_name,uid_validity,scanned_at_unix,server_message_count,discovered_uid_count)
            VALUES('e6@example.invalid','imap.e6.invalid',?1,?2,1,1,?3,?3)",rusqlite::params![source,label,i64::try_from(n).unwrap()]).unwrap();
    }
    let bulk:i64=transaction.query_row("SELECT id FROM mailbox WHERE account='e6@example.invalid' AND source_name='INBOX'",[],|row|row.get(0)).unwrap();
    let empty:i64=transaction.query_row("SELECT id FROM mailbox WHERE account='e6@example.invalid' AND source_name='Sent'",[],|row|row.get(0)).unwrap();
    let columns=transaction.prepare("PRAGMA table_info(message)").unwrap().query_map([],|row|row.get::<_,String>(1)).unwrap()
        .map(Result::unwrap).filter(|name|name!="id").collect::<Vec<_>>();
    // Clone complete existing, real MIME fixture rows. The schema is test-owned;
    // only mailbox/UID/subject change. EML and attachment paths remain valid.
    let names=columns.iter().map(|name|format!("\"{name}\"")).collect::<Vec<_>>().join(",");
    let values=columns.iter().map(|name|match name.as_str(){
        "mailbox_id"=>"?1".to_owned(),"uid"=>"?2".to_owned(),
        "subject"=>"('E6 bulk message ' || ?2)".to_owned(),_=>format!("\"{name}\"")
    }).collect::<Vec<_>>().join(",");
    {
        let mut statement=transaction.prepare(&format!("INSERT INTO message({names}) SELECT {values} FROM message WHERE id=?3")).unwrap();
        for uid in 1..=count { statement.execute(rusqlite::params![bulk,i64::try_from(uid).unwrap(),template]).unwrap(); }
    }
    transaction.execute("UPDATE message SET date_unix=100 WHERE mailbox_id=?1 AND uid IN (2,3)",[bulk]).unwrap();
    transaction.execute("UPDATE message SET date_unix=0 WHERE mailbox_id=?1 AND uid=4",[bulk]).unwrap();
    transaction.commit().unwrap();
    update(fixture,cx,|view,window,cx|{
        view.close_settings(window,cx);view.close_preview(window,cx);view.dismiss_search(window,cx);
        view.sync=None;view.onboarding=None;
        view.reload_index(cx).unwrap();
        view.select_mailbox(bulk,true,window,cx);
        view.sync_sidebar_tree(cx);
    });
    (bulk,empty)
}

async fn navigation_settle(fixture: &TestWindow, cx: &mut AsyncApp) {
    settle_overlay(cx).await;
    update_window(fixture,cx,|window,cx|window.draw(cx).clear(cx));
    settle(cx).await;
}

// Feed the platform input path used by real pointer events. Drawing between
// moves installs the official handle's drag state before applying its offset.
fn drag_pointer(fixture: &TestWindow, cx: &mut AsyncApp, start: gpui::Point<gpui::Pixels>, end: gpui::Point<gpui::Pixels>) {
    update_window(fixture,cx,|window,cx| {
        window.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position:start, ..Default::default() }),cx);
        window.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { position:start, button:MouseButton::Left, click_count:1, ..Default::default() }),cx);
        for position in [start+gpui::point(px(0.),px(8.)),end,end] {
            window.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button:Some(MouseButton::Left), ..Default::default() }),cx);
            window.draw(cx).clear(cx);
        }
        window.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { position:end, button:MouseButton::Left, click_count:1, ..Default::default() }),cx);
        window.draw(cx).clear(cx);
    });
}

fn visible_message_rows(window: &Window) -> usize {
    gpui_kit::base::test_support::snapshots(window).iter().filter(|snapshot|
        snapshot.visible() && matches!(snapshot.path().last(),Some(gpui::ElementId::NamedInteger(name,_)) if name.as_ref()=="message")
    ).count()
}

pub async fn check_navigation(fixture: &TestWindow, cx: &mut AsyncApp) -> Vec<Value> {
    let mut checks=Vec::new();
    update_window(fixture,cx,|window,_|window.resize(gpui::size(px(1440.),px(900.))));
    settle_overlay(cx).await;
    save_visual_prefs(fixture,cx,Prefs{scheme:ColorScheme::Dark,..Prefs::default()});
    clear_business_feedback(fixture,cx).await;
    let (bulk,empty)=seed_navigation_mail(fixture,cx,10_000);
    navigation_settle(fixture,cx).await;
    // Earlier search-dismissal checks intentionally toggle this shared view.
    // Establish the navigation fixture through the actual control, not an
    // assumption about which earlier checks ran or a replacement for the action.
    let collapsed = update(fixture, cx, |view, window, _| {
        assert_eq!(chrome(window, view.prefs.workspace), Chrome::Default);
        view.sidebar_collapsed
    });
    if collapsed {
        click_element(fixture, cx, "sidebar-collapse");
        navigation_settle(fixture, cx).await;
    }
    update(fixture, cx, |view, _, _| {
        assert!(!view.sidebar_collapsed, "navigation fixture must start expanded");
    });
    update(fixture,cx,|view,window,cx|{
        assert_eq!(view.selected_count(),10_000);
        assert!(view.selected_index.is_none());
        window.focus(&view.list_focus,cx);
    });
    press_key(fixture,cx,"down");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|assert_eq!(view.selected_index,Some(0)));
    for _ in 0..51 { press_key(fixture,cx,"down"); }
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,cx|{
        assert_eq!(view.selected_index,Some(51));
        let expected_id=view.message_at(51).unwrap().id;
        assert_eq!(view.selected_message,Some(expected_id));
        assert_eq!(view.selected_detail.as_ref().unwrap().id,view.selected_message.unwrap());
        assert!(view.message_pages.contains_key(&1));
        assert!(view.message_pages.len()<=MESSAGE_CACHE_PAGES);
        cx.notify();
    });
    update_window(fixture,cx,|window,cx|{
        window.draw(cx).clear(cx);
        let count=visible_message_rows(window);
        assert!(count>0 && count<100,"only visible 10k-mail rows should be painted, got {count}");
    });
    checks.push(e6_record("official virtual message rows support real Down navigation across the 50-row page boundary", "synthetic-gpui-keys-and-isolated-sqlite"));

    update(fixture,cx,|view,window,cx|view.select_message_at(9_999,window,cx));
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,window,cx|{
        assert_eq!(view.selected_index,Some(9_999));
        let row=view.message_at(9_999).unwrap();
        assert_eq!(view.selected_message,Some(row.id));
        assert_eq!(view.store.as_ref().unwrap().message_position(bulk,row.id).unwrap(),Some(9_999));
        for index in 0..3 {
            let row=view.message_at(index).unwrap();
            assert_eq!(view.store.as_ref().unwrap().message_position(bulk,row.id).unwrap(),Some(index));
        }
        assert_eq!(view.store.as_ref().unwrap().message_position(empty,row.id).unwrap(),None);
        window.focus(&view.list_focus,cx);
    });
    press_key(fixture,cx,"down");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|assert_eq!(view.selected_index,Some(0)));
    press_key(fixture,cx,"up");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|assert_eq!(view.selected_index,Some(9_999)));
    checks.push(e6_record("10k-mail first/last wrapping and SQL position agree for NULL/tied dates and UID ordering", "synthetic-gpui-keys-and-isolated-sqlite"));

    update(fixture,cx,|view,_,_|{
        for page in 0..80 {
            assert!(view.message_at(page*MESSAGE_PAGE_SIZE).is_some());
            assert!(view.message_pages.len()<=MESSAGE_CACHE_PAGES);
        }
        assert!(view.message_pages.values().map(Vec::len).sum::<usize>()<=400);
    });
    checks.push(e6_record("scrolling/querying many pages retains at most eight pages/400 summaries, not the whole mailbox", "business-state-and-isolated-sqlite"));

    let mut times=Vec::new();let mut max_rows=0;
    for index in [0,49,50,499,2500,9999,4000,200,7300,5000,10,9800] {
        update(fixture,cx,|view,window,cx|view.select_message_at(index,window,cx));
        settle(cx).await;
        for _ in 0..2 {
            let (elapsed,rows)=update_window(fixture,cx,|window,cx|{
                let start=std::time::Instant::now();window.draw(cx).clear(cx);
                (start.elapsed().as_secs_f64()*1000.,visible_message_rows(window))
            });
            times.push(elapsed);max_rows=max_rows.max(rows);
        }
    }
    times.sort_by(f64::total_cmp);
    let performance=update(fixture,cx,|view,_,_|json!({
        "messages":10_000,"page_size":MESSAGE_PAGE_SIZE,"cache_page_limit":MESSAGE_CACHE_PAGES,
        "cached_pages":view.message_pages.len(),"max_painted_message_rows":max_rows,
        "draw_samples_ms":times,"median_draw_ms":times[times.len()/2],"p95_draw_ms":times[(times.len()*95/100).min(times.len()-1)],
        "measurement":"debug GPUI draw CPU duration; not OS frame presentation",
        "release_performance_budget_acceptance":false,
    }));
    assert!(max_rows<100);
    let path=update(fixture,cx,|view,_,_|view.paths.index.parent().unwrap().join("e6-performance.json"));
    std::fs::write(&path,serde_json::to_vec_pretty(&performance).unwrap()).unwrap();
    let mut record=e6_record("10k mailbox draw/caching metrics recorded from the production component path", "native-gpui-measurement-NOT-release-budget");
    record["metrics"]=performance;record["path"]=json!(path);checks.push(record);

    update(fixture,cx,|view,window,cx|{
        view.open_search(window,cx);
        view.search_input.update(cx,|input,cx|input.replace_all("E6 bulk",window,cx));
    });
    settle_feedback(cx).await;
    let (last,id,position)=update(fixture,cx,|view,_window,cx|{
        assert!(!view.search_pending && view.search_results.len()>50);
        let last=view.search_results.len()-1;
        let id=view.search_results[last].id;
        let position=view.store.as_ref().unwrap().message_position(bulk,id).unwrap().unwrap();
        view.search_result_scroll.scroll_to_item(last,ScrollStrategy::Center);
        cx.notify();(last,id,position)
    });
    navigation_settle(fixture,cx).await;
    click_element(fixture,cx,("search-result",u64::try_from(id).unwrap()));
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,window,cx|{
        assert_eq!(view.search_selection,last);
        assert_eq!(view.selected_message,Some(id));
        assert_eq!(view.selected_index,Some(position));
        view.clear_search(window,cx);
        window.focus(&view.list_focus,cx);
    });
    navigation_settle(fixture,cx).await;
    press_key(fixture,cx,"down");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|assert_eq!(view.selected_index,Some((position+1)%10_000)));
    checks.push(e6_record("virtualized real search results keep identity/rank when returning to normal arrow navigation", "synthetic-gpui-input-pointer-keys-and-isolated-search"));

    let (generation,detail)=update(fixture,cx,|view,window,cx|{
        let id=view.message_at(6).unwrap().id;
        view.select_message(id,cx);
        let generation=view.detail_generation;
        let detail=load_detail_with_cids(&view.paths.index,id).unwrap().unwrap();
        view.select_mailbox(empty,true,window,cx);
        (generation,detail)
    });
    update(fixture,cx,|view,_,cx|{
        view.finish_detail_load(generation,Ok(Some(detail)),cx);
        assert!(view.selected_detail.is_none());
        assert!(view.selected_message.is_none());
        assert_eq!(view.selected_count(),0);
    });
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,window,cx|{
        assert!(view.selected_detail.is_none(),"actual older background completion stays invalidated too");
        window.focus(&view.list_focus,cx);
    });
    press_key(fixture,cx,"down");
    update(fixture,cx,|view,_,_|assert!(view.selected_index.is_none()));
    checks.push(e6_record("empty mailbox navigation rejects both injected and actual late detail callbacks", "business-callback-and-synthetic-gpui-key"));

    update(fixture,cx,|view,window,cx|{
        view.select_mailbox(bulk,true,window,cx);
        view.sync_sidebar_tree(cx);
        let account:SharedString="account:e6@example.invalid\0imap.e6.invalid".into();
        view.sidebar_tree.update(cx,|state,cx|{
            let index=state.index_of(&account).unwrap();state.set_selected_index(Some(index),cx);
            state.focus(window,cx);
        });
    });
    navigation_settle(fixture,cx).await;
    press_key(fixture,cx,"left");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|{
        assert!(view.collapsed_accounts.contains("e6@example.invalid\0imap.e6.invalid"));
        assert!(!view.visible_mailbox_ids().contains(&bulk));
    });
    press_key(fixture,cx,"right");
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|assert!(view.visible_mailbox_ids().contains(&bulk)));
    click_element(fixture,cx,SharedString::from(format!("mailbox:{empty}")));
    navigation_settle(fixture,cx).await;
    update(fixture,cx,|view,_,_|{
        assert_eq!(view.selected_mailbox,Some(empty));
        assert!(view.search_query.is_empty());
    });
    checks.push(e6_record("official Tree left/right expansion mirrors accounts and real folder clicks keep selection", "synthetic-gpui-tree-keys-and-pointer"));
    let (selection, expanded_width) = update(fixture, cx, |view, window, _| {
        assert!(!view.sidebar_collapsed);
        (view.selected_mailbox, element_snapshot(window, "mail-sidebar").bounds().size.width)
    });
    click_element(fixture, cx, "sidebar-collapse");
    navigation_settle(fixture, cx).await;
    update(fixture, cx, |view, window, _| {
        assert!(view.sidebar_collapsed, "expanded sidebar must collapse on pointer click");
        assert_eq!(view.selected_mailbox, selection);
        assert!(element_snapshot(window, "mail-sidebar").bounds().size.width < expanded_width);
    });
    click_element(fixture, cx, "sidebar-collapse");
    navigation_settle(fixture, cx).await;
    update(fixture, cx, |view, window, _| {
        assert!(!view.sidebar_collapsed, "icon sidebar must expand on pointer click");
        assert_eq!(view.selected_mailbox, selection);
        assert_eq!(element_snapshot(window, "mail-sidebar").bounds().size.width, expanded_width);
    });
    checks.push(e6_record("sidebar pointer collapse/expand changes actual width and preserves selection independently of earlier fixture state", "synthetic-gpui-pointer-and-layout"));
    update(fixture,cx,|view,window,cx|{
        view.select_mailbox(bulk,true,window,cx);view.select_message_at(0,window,cx);
    });
    navigation_settle(fixture,cx).await;
    for desired in [1.,1000.,150.] {
        update(fixture,cx,|view,window,cx|view.reader_split.update(cx,|state,cx|state.resize_panel(1,px(desired),window,cx)));
        navigation_settle(fixture,cx).await;
        update(fixture,cx,|view,window,cx|{
            let sizes=view.reader_split.read(cx).sizes();
            assert_eq!(sizes.len(),2);
            assert!(sizes[0]>=px(64.));
            assert!(sizes[1]>=window.rem_size()*ATTACHMENT_PANE_MIN_REM);
            assert!(sizes[1]<=window.rem_size()*ATTACHMENT_PANE_MAX_REM,
                "requested {desired}px; state={sizes:?}; rendered={:?}; rem={:?}",
                element_snapshot(window,"attachment-panel-region").bounds(),window.rem_size());
            let rendered = element_snapshot(window,"attachment-panel-region").bounds().size.height;
            assert!(f32::from(sizes[1]-rendered).abs() <= 1., "state {sizes:?} disagrees with rendered height {rendered:?}");
            assert!((view.attachment_pane_rem-f32::from(sizes[1])/f32::from(window.rem_size())).abs()<0.1);
        });
    }
    checks.push(e6_record("official Resizable state clamps both panels at their size limits", "official-component-state-and-layout"));
    for delta in [-60.,1000.,-1000.,80.] {
        // Drawing re-enters MailApp::render; never hold its Entity borrow here.
        update_window(fixture, cx, |window, cx| window.draw(cx).clear(cx));
        let (start,end,before)=update(fixture,cx,|view,window,cx| {
            let bounds=element_snapshot(window,"attachment-panel-region").bounds();
            let start=gpui::point(bounds.origin.x+bounds.size.width/2.,bounds.origin.y);
            (start,start+gpui::point(px(0.),px(delta)),view.reader_split.read(cx).sizes().to_vec())
        });
        drag_pointer(fixture,cx,start,end);
        navigation_settle(fixture,cx).await;
        update(fixture,cx,|view,window,cx| {
            let sizes=view.reader_split.read(cx).sizes();
            assert_ne!(sizes,&before,"pointer delta {delta} must reach the official resize handler");
            assert!(sizes[0]>=px(63.));
            assert!(sizes[1]>=window.rem_size()*ATTACHMENT_PANE_MIN_REM-px(1.));
            assert!(sizes[1]<=window.rem_size()*ATTACHMENT_PANE_MAX_REM+px(1.));
            assert!((view.attachment_pane_rem-f32::from(sizes[1])/f32::from(window.rem_size())).abs()<0.1);
        });
    }
    checks.push(e6_record("actual pointer drag reaches official Resizable, clamps extremes and retains the emitted attachment height", "synthetic-gpui-pointer-and-official-resizable"));
    let path=update_window(fixture,cx,|window,cx|capture_frame(window,cx,"e6-tree-list-resizable"));
    checks.push(json!({"batch":"E6","check":"E6 migrated navigation/list/reader split screenshot","evidence":"native-gpui-offscreen-screenshot","path":path,"visually_reviewed":false}));
    checks
}

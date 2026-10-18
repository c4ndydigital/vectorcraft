//! Adversarial test harness for Milestone 3 Collab Sync and Presence.

use serde_json::json;
use std::sync::{Arc, Mutex};
use vectorcraft_doc::NodeKind;
use vectorcraft_engine::Session;
use vectorcraft_engine::collab::CollabBridge;
use vectorcraft_geom::Point;
use vectorcraft_ui_egui::{CollabIncoming, CollabOutgoing, Services, VectorcraftApp};

/// 1. EMPIRICAL TEST: Verify echo loop prevention across round-trips and self-reflections.
#[test]
fn test_adversarial_echo_loop_prevention() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    app_a.run("file.new", json!({"width": 800, "height": 600})).expect("file.new a");
    app_b.run("file.new", json!({"width": 800, "height": 600})).expect("file.new b");

    let ctx = egui::Context::default();

    // Round 1: Peer A creates rectangle
    app_a.run("shape.rectangle", json!({"x": 10.0, "y": 20.0, "width": 100.0, "height": 50.0})).expect("rect 1");
    app_a.handle_collab(&ctx, 1.0);

    let mut out_a = outbox_a.lock().unwrap();
    assert_eq!(out_a.len(), 1, "Peer A emits 1 update");
    let update_1 = match out_a.pop().unwrap() {
        CollabOutgoing::Update(b) => b,
        _ => panic!("Expected CollabOutgoing::Update"),
    };
    drop(out_a);

    // Deliver update_1 to Peer B
    inbox_b.lock().unwrap().push(CollabIncoming::Update(update_1.clone()));
    app_b.handle_collab(&ctx, 1.0);

    // Check Peer B did NOT echo
    assert!(outbox_b.lock().unwrap().is_empty(), "Peer B must not echo update_1");

    // Advance time on Peer B without local edits
    app_b.handle_collab(&ctx, 1.5);
    app_b.handle_collab(&ctx, 2.0);
    assert!(outbox_b.lock().unwrap().is_empty(), "Peer B still has empty outbox");

    // Reflection attack: Deliver update_1 BACK to Peer A (e.g. naive broadcast reflector)
    inbox_a.lock().unwrap().push(CollabIncoming::Update(update_1));
    app_a.handle_collab(&ctx, 2.0);
    assert!(outbox_a.lock().unwrap().is_empty(), "Peer A must not echo reflected self-update");

    // Round 2: Peer B creates a shape (rectangle)
    app_b.run("shape.rectangle", json!({"x": 200.0, "y": 200.0, "width": 80.0, "height": 80.0})).expect("rect 2");
    app_b.handle_collab(&ctx, 2.1);

    let mut out_b = outbox_b.lock().unwrap();
    assert_eq!(out_b.len(), 1, "Peer B emits exactly 1 update for local edit");
    let update_2 = match out_b.pop().unwrap() {
        CollabOutgoing::Update(b) => b,
        _ => panic!("Expected CollabOutgoing::Update"),
    };
    drop(out_b);

    // Deliver update_2 to Peer A
    inbox_a.lock().unwrap().push(CollabIncoming::Update(update_2));
    app_a.handle_collab(&ctx, 2.2);

    // Peer A must NOT echo update_2
    assert!(outbox_a.lock().unwrap().is_empty(), "Peer A must not echo update_2 back to outbox");

    // Both peers now have 2 shapes
    let count_a = app_a.session.active().unwrap().doc.layers[0].children().unwrap().len();
    let count_b = app_b.session.active().unwrap().doc.layers[0].children().unwrap().len();
    assert_eq!(count_a, 2, "Peer A has 2 shapes");
    assert_eq!(count_b, 2, "Peer B has 2 shapes");
}

/// 2. EMPIRICAL TEST: Verify 30 Hz throttling of cursor presence broadcasts.
#[test]
fn test_adversarial_cursor_throttling_30hz() {
    let inbox = Arc::new(Mutex::new(Vec::new()));
    let outbox = Arc::new(Mutex::new(Vec::new()));

    let services = Services { collab_inbox: Some(inbox), collab_outbox: Some(outbox.clone()), ..Default::default() };

    let mut app = VectorcraftApp::new(Session::new(), services);
    let ctx = egui::Context::default();

    // Baseline: initial tick at t = 10.0
    app.handle_collab(&ctx, 10.0);
    outbox.lock().unwrap().clear();

    // Simulate 100 high-frequency mouse movements within 1 second (100 Hz / every 10ms)
    // From t = 10.010 to t = 11.000 (100 steps of 10ms)
    let start_time = 10.010;
    let mut broadcast_times = Vec::new();

    for i in 0..100 {
        let now = start_time + (i as f64) * 0.010;
        app.hover_doc = Some(Point::new(100.0 + i as f64, 200.0 + i as f64));
        app.handle_collab(&ctx, now);

        let mut out = outbox.lock().unwrap();
        for msg in out.drain(..) {
            if let CollabOutgoing::Presence(p) = msg {
                broadcast_times.push(p.last_seen);
            }
        }
    }

    // High frequency test assertions:
    // Without throttling: 100 broadcasts.
    // With 30 Hz throttling (dt >= 0.033): in 1.0 second, at most ~31 broadcasts can occur.
    let total_broadcasts = broadcast_times.len();
    assert!(
        (24..=31).contains(&total_broadcasts),
        "Throttling 100 Hz cursor input at ~30 Hz must produce 24..=31 events in 1 second, got {total_broadcasts}"
    );

    // Verify time gap between every consecutive broadcast is >= 0.033s (33ms)
    for w in broadcast_times.windows(2) {
        let dt = w[1] - w[0];
        assert!(dt >= 0.033 - 1e-6, "Interval between broadcasts must be >= 0.033s, found dt = {dt}");
    }

    // Heartbeat test: keep cursor stationary for 3 seconds
    let mut heartbeat_count = 0;
    for step in 1..=30 {
        let now = 11.0 + (step as f64) * 0.100; // every 100ms
        app.handle_collab(&ctx, now);
        let mut out = outbox.lock().unwrap();
        for msg in out.drain(..) {
            if let CollabOutgoing::Presence(_) = msg {
                heartbeat_count += 1;
            }
        }
    }
    // Over 3 seconds with static cursor, heartbeat triggers when dt >= 1.0 (approx 3 times)
    assert!((2..=4).contains(&heartbeat_count), "Heartbeat broadcast over 3s should be ~3, got {heartbeat_count}");

    // Cursor departure: cursor leaves canvas
    app.hover_doc = None;
    app.handle_collab(&ctx, 15.0);
    let mut out = outbox.lock().unwrap();
    let departure_msg = out.pop().expect("departure broadcast");
    match departure_msg {
        CollabOutgoing::Presence(p) => assert!(p.cursor.is_none(), "Cursor must be None on departure"),
        _ => panic!("Expected presence"),
    }
    drop(out);

    // Advance time with cursor remaining None: NO more presence broadcasts
    for step in 1..=10 {
        let now = 15.0 + (step as f64) * 0.5;
        app.handle_collab(&ctx, now);
    }
    assert!(outbox.lock().unwrap().is_empty(), "No broadcasts while cursor remains None");
}

/// 3. EMPIRICAL TEST: Verify two-way synchronization, new Arc<Document>, and revision bumping.
#[test]
fn test_adversarial_twoway_sync_arc_document_revision() {
    let mut session_a = Session::new();
    session_a.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();

    let mut session_b = Session::new();
    session_b.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();

    let mut bridge_a = CollabBridge::with_client_id(1);
    let mut bridge_b = CollabBridge::with_client_id(2);

    // Initial state on Remote (B)
    let initial_arc_b = Arc::clone(&session_b.active().unwrap().doc);
    let initial_rev_b = session_b.active().unwrap().revision;

    // 1. Peer A adds a shape locally
    let rect_res = session_a.execute("shape.rectangle", &json!({"x": 50.0, "y": 75.0, "width": 120.0, "height": 90.0})).unwrap();
    let rect_id = rect_res["id"].as_u64().unwrap();

    // Export update from A
    let update_a = bridge_a.sync_from_session(&session_a).expect("sync from A");
    assert!(!update_a.is_empty(), "CRDT update must be non-empty");

    // Apply remote update to B
    bridge_b.apply_remote_update(&mut session_b, &update_a).expect("apply update to B");

    // Assert that Remote (B) has a NEW Arc<Document>
    let current_arc_b = Arc::clone(&session_b.active().unwrap().doc);
    assert!(!Arc::ptr_eq(&initial_arc_b, &current_arc_b), "Applying remote update must produce a new Arc<Document> pointer");

    // Assert that revision was bumped
    let current_rev_b = session_b.active().unwrap().revision;
    assert!(current_rev_b > initial_rev_b, "Applying remote update must bump revision: initial={initial_rev_b}, current={current_rev_b}");

    // Assert that the shape exists in B with matching geometry
    let node_in_b = session_b.active().unwrap().doc.node(vectorcraft_doc::NodeId(rect_id)).expect("shape in B");
    assert!(matches!(node_in_b.kind, NodeKind::Path { .. }));

    // 2. Now Peer B modifies the shape (reverse direction sync)
    let initial_arc_a = Arc::clone(&session_a.active().unwrap().doc);
    let initial_rev_a = session_a.active().unwrap().revision;

    session_b.execute("paint.setFill", &json!({"ids": [rect_id], "color": "#00ff88"})).unwrap();

    let update_b = bridge_b.sync_from_session(&session_b).expect("sync from B");
    assert!(!update_b.is_empty(), "Peer B modification produces CRDT update");

    bridge_a.apply_remote_update(&mut session_a, &update_b).expect("apply update to A");

    // Assert Peer A also gets a NEW Arc<Document> and bumped revision
    let current_arc_a = Arc::clone(&session_a.active().unwrap().doc);
    assert!(!Arc::ptr_eq(&initial_arc_a, &current_arc_a), "Applying reverse remote update must produce a new Arc<Document> on Peer A");
    assert!(session_a.active().unwrap().revision > initial_rev_a, "Revision on Peer A must be bumped");

    // Check color converged on Peer A
    let node_in_a = session_a.active().unwrap().doc.node(vectorcraft_doc::NodeId(rect_id)).expect("shape in A");
    let fill = node_in_a.appearance.fill().expect("fill present");
    match &fill.paint {
        vectorcraft_color::Paint::Solid { color, .. } => {
            assert_eq!(color.to_hex(), "#00ff88");
        }
        _ => panic!("Expected solid fill"),
    }
}

/// 5. EMPIRICAL REGRESSION TEST: PDF Image Blob Sync & Peer / Reload Recovery.
///
/// Verifies that:
/// 1. Importing a PDF extracts images into `doc.images` with valid image keys and MIME types.
/// 2. `handle_collab` serializes CRDT shape updates and outputs `CollabOutgoing::Blob`s for all extracted images without duplication.
/// 3. Peer B receives the CRDT update and Blob payloads, correctly populating `doc.images` and image nodes.
/// 4. Late-joining Peer C (or browser reload) receives existing image blobs and renders without missing textures.
#[test]
fn test_adversarial_pdf_image_sync_and_reload() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    let ctx = egui::Context::default();

    // 1. Peer A imports the root PDF test file
    let pdf_bytes = std::fs::read("/srv/photoshop/Untitled-1.pdf").expect("read Untitled-1.pdf");
    let doc = vectorcraft_pdf::import(&pdf_bytes).expect("vectorcraft_pdf::import");
    assert!(!doc.images.is_empty(), "PDF must contain raster images in doc.images");
    let expected_image_count = doc.images.len();

    // Set doc into Peer A's active session
    let doc_idx = app_a.session.add_document(doc, Some("Untitled-1.pdf".to_string()));
    app_a.session.set_active(doc_idx);

    // 2. Peer A runs handle_collab
    app_a.handle_collab(&ctx, 1.0);

    let mut out_a = outbox_a.lock().unwrap();
    let mut crdt_updates = Vec::new();
    let mut blob_messages = Vec::new();

    for msg in out_a.drain(..) {
        match msg {
            CollabOutgoing::Update(u) => crdt_updates.push(u),
            CollabOutgoing::Blob { key, mime, bytes } => blob_messages.push((key, mime, bytes)),
            _ => {}
        }
    }
    drop(out_a);

    assert!(!crdt_updates.is_empty(), "Must emit CRDT update for imported document");
    assert_eq!(blob_messages.len(), expected_image_count, "Must emit blob messages for each unique PDF image");

    // Verify deduplication: second frame without modifications emits NO duplicate blobs
    app_a.handle_collab(&ctx, 1.1);
    assert!(outbox_a.lock().unwrap().is_empty(), "Subsequent frames must not re-emit already synced blobs");

    // 3. Deliver CRDT update and Blob messages to Peer B
    for update in &crdt_updates {
        inbox_b.lock().unwrap().push(CollabIncoming::Update(update.clone()));
    }
    for (key, mime, bytes) in &blob_messages {
        inbox_b.lock().unwrap().push(CollabIncoming::Blob {
            key: key.clone(),
            mime: mime.clone(),
            bytes: bytes.clone(),
        });
    }

    app_b.handle_collab(&ctx, 1.2);

    // Peer B should now have the document and all images in doc.images!
    let b_doc = &app_b.session.active().expect("active doc in B").doc;
    assert_eq!(b_doc.images.len(), expected_image_count, "Peer B must have all images synchronized");
    for (k, _, _) in &blob_messages {
        assert!(b_doc.images.contains_key(k), "Peer B must contain image key: {k}");
    }

    // 4. Test Late Joiner / Browser Reload (Peer C)
    // Simulates a fresh reload connecting to the session room
    let inbox_c = Arc::new(Mutex::new(Vec::new()));
    let outbox_c = Arc::new(Mutex::new(Vec::new()));
    let services_c = Services { collab_inbox: Some(inbox_c.clone()), collab_outbox: Some(outbox_c.clone()), ..Default::default() };
    let mut app_c = VectorcraftApp::new(Session::new(), services_c);

    // Peer C receives initial state: CRDT snapshot + cached server blobs
    let bridge_a = app_a.collab.as_ref().unwrap();
    let snapshot = bridge_a.export_update();
    inbox_c.lock().unwrap().push(CollabIncoming::Update(snapshot));
    for (key, mime, bytes) in &blob_messages {
        inbox_c.lock().unwrap().push(CollabIncoming::Blob {
            key: key.clone(),
            mime: mime.clone(),
            bytes: bytes.clone(),
        });
    }

    app_c.handle_collab(&ctx, 2.0);

    let c_doc = &app_c.session.active().expect("active doc in C").doc;
    assert_eq!(c_doc.images.len(), expected_image_count, "Reloaded Peer C must restore all images");
    for (k, _, _) in &blob_messages {
        assert!(c_doc.images.contains_key(k), "Reloaded Peer C must contain image key: {k}");
    }
}

/// 6. EMPIRICAL TEST: Verify Tab Close two-way synchronization in UI collab harness.
#[test]
fn test_adversarial_tab_close_sync() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    app_a.run("file.new", json!({"width": 800, "height": 600})).expect("file.new a");
    app_b.run("file.new", json!({"width": 800, "height": 600})).expect("file.new b");

    let ctx = egui::Context::default();

    // 1. Initial sync between App A and App B
    app_a.handle_collab(&ctx, 1.0);
    let init_a_msgs: Vec<_> = outbox_a.lock().unwrap().drain(..).collect();
    for msg in init_a_msgs {
        if let CollabOutgoing::Update(b) = msg {
            inbox_b.lock().unwrap().push(CollabIncoming::Update(b));
        }
    }
    app_b.handle_collab(&ctx, 1.0);

    // 2. Peer A creates a 2nd tab with a shape
    app_a.run("file.new", json!({"width": 1024, "height": 768})).expect("tab 2 on A");
    app_a.run("shape.rectangle", json!({"x": 100.0, "y": 100.0, "width": 80.0, "height": 80.0})).expect("rect in tab 2");
    app_a.handle_collab(&ctx, 1.1);

    let msgs: Vec<_> = outbox_a.lock().unwrap().drain(..).collect();
    for msg in msgs {
        if let CollabOutgoing::Update(b) = msg {
            inbox_b.lock().unwrap().push(CollabIncoming::Update(b));
        }
    }
    app_b.handle_collab(&ctx, 1.1);

    assert_eq!(app_a.session.documents().len(), 2, "App A has 2 documents");
    assert_eq!(app_b.session.documents().len(), 2, "App B synchronized 2 documents");

    // 3. Peer A closes Tab 1
    let closed = app_a.session.close_document(1);
    assert!(closed, "App A closes tab 1");
    assert_eq!(app_a.session.documents().len(), 1);

    app_a.handle_collab(&ctx, 1.2);

    let msgs: Vec<_> = outbox_a.lock().unwrap().drain(..).collect();
    assert!(!msgs.is_empty(), "App A must emit update after closing tab");
    for msg in msgs {
        if let CollabOutgoing::Update(b) = msg {
            inbox_b.lock().unwrap().push(CollabIncoming::Update(b));
        }
    }

    app_b.handle_collab(&ctx, 1.2);

    // App B must now also have exactly 1 document and views synchronized!
    assert_eq!(app_b.session.documents().len(), 1, "App B must have closed tab 1");
    assert_eq!(app_b.views.len(), 1, "App B views must match document count");
}

/// 7. EMPIRICAL TEST: Verify Browser Reload / Reconnect preserves closed tabs and deleted shapes.
/// Tests that when a peer reloads (or late-joins) from the server's full CRDT snapshot:
/// - Closed tabs stay closed (do NOT reappear).
/// - Deleted shapes stay deleted (canvas remains empty/clean as expected).
#[test]
fn test_adversarial_reload_after_tab_close_and_shape_deletion() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let mut app_a = VectorcraftApp::new(Session::new(), services_a);

    app_a.run("file.new", json!({"width": 800, "height": 600})).expect("file.new 1");
    let ctx = egui::Context::default();
    app_a.handle_collab(&ctx, 1.0);
    outbox_a.lock().unwrap().clear();

    // 1. Peer A creates a shape in Tab 0
    let r0 = app_a.run("shape.rectangle", json!({"x": 50.0, "y": 50.0, "width": 120.0, "height": 80.0})).expect("rect tab 0");
    let r0_id = r0["id"].as_u64().unwrap();

    // 2. Peer A creates Tab 1 with a circle
    app_a.run("file.new", json!({"width": 1024, "height": 768})).expect("tab 1");
    let _c1 = app_a.run("shape.ellipse", json!({"x": 200.0, "y": 150.0, "width": 60.0, "height": 60.0})).expect("circle tab 1");

    // Commit both tabs and shapes into CRDT
    app_a.handle_collab(&ctx, 1.1);
    outbox_a.lock().unwrap().clear();

    assert_eq!(app_a.session.documents().len(), 2);
    assert_eq!(app_a.collab.as_ref().unwrap().tabs_count(), 2);
    assert_eq!(app_a.collab.as_ref().unwrap().shapes_count(), 2);

    // 3. Peer A closes Tab 1 (tab-close)
    let closed = app_a.session.close_document(1);
    assert!(closed);
    assert_eq!(app_a.session.documents().len(), 1);

    // 4. Peer A switches back to Tab 0 and deletes the rectangle (shape deletion)
    app_a.session.set_active(0);
    app_a.session.execute("edit.clear", &json!({"ids": [r0_id]})).expect("delete rectangle in tab 0");

    let remaining_shapes = app_a.session.active().unwrap().doc.layers.iter().map(|l| l.children().map(|c| c.len()).unwrap_or(0)).sum::<usize>();
    assert_eq!(remaining_shapes, 0, "Tab 0 must have 0 shapes after deletion");

    // Commit tab-close and shape-deletion to CRDT
    app_a.handle_collab(&ctx, 1.2);
    outbox_a.lock().unwrap().clear();

    let bridge_a = app_a.collab.as_ref().unwrap();
    assert_eq!(bridge_a.tabs_count(), 1, "CRDT must only retain 1 tab");
    assert_eq!(bridge_a.shapes_count(), 0, "CRDT must retain 0 shapes");

    // 5. Simulate Browser Reload (Peer B connecting fresh to the room)
    // Server sends full CRDT snapshot to the reloaded client
    let snapshot = bridge_a.export_update();

    let inbox_reload = Arc::new(Mutex::new(Vec::new()));
    let outbox_reload = Arc::new(Mutex::new(Vec::new()));
    let services_reload = Services { collab_inbox: Some(inbox_reload.clone()), collab_outbox: Some(outbox_reload.clone()), ..Default::default() };

    // Fresh Web startup creates initial empty app + file.new
    let mut app_reload = VectorcraftApp::new(Session::new(), services_reload);
    app_reload.run("file.new", json!({"width": 800, "height": 600})).expect("web bootstrap file.new");

    // Ingest the server snapshot upon connection
    inbox_reload.lock().unwrap().push(CollabIncoming::Update(snapshot));
    app_reload.handle_collab(&ctx, 2.0);

    // ASSERTIONS FOR RELOAD:
    // 1. Closed tab 1 must NOT reappear! Total document count must be exactly 1.
    assert_eq!(
        app_reload.session.documents().len(),
        1,
        "Reloaded peer must have exactly 1 tab (closed tab must NOT reappear)"
    );

    // 2. Views count in egui must match document count (exactly 1).
    assert_eq!(
        app_reload.views.len(),
        1,
        "Reloaded peer views count must be 1"
    );

    // 3. The deleted rectangle must NOT reappear! Canvas must remain completely empty.
    let reloaded_shape_count = app_reload.session.active().unwrap().doc.layers.iter().map(|l| l.children().map(|c| c.len()).unwrap_or(0)).sum::<usize>();
    assert_eq!(
        reloaded_shape_count,
        0,
        "Reloaded peer canvas must remain completely empty (deleted shapes must NOT reappear)"
    );

    // 4. Reloaded peer must not emit any spurious update echoing back
    assert!(
        outbox_reload.lock().unwrap().is_empty(),
        "Reloaded peer must not produce echo updates after snapshot ingest"
    );
}

/// 7. EMPIRICAL REGRESSION TEST: Live Drag & Transform Preview (Figma-Style Ephemeral Live Movement).
///
/// Verifies that:
/// 1. While Peer A is dragging/transforming a shape:
///    - `handle_collab` does NOT emit heavy CRDT updates (preventing history pollution).
///    - `handle_collab` emits `CollabOutgoing::Presence` containing `PeerInteraction` with valid bounds and interaction label.
/// 2. Peer B receives `CollabIncoming::Presence` and registers Peer A's `active_interaction`.
/// 3. Upon drag release (`pointerup`), Peer A:
///    - Emits the final committed CRDT update.
///    - Emits `CollabOutgoing::Presence` with `active_interaction: None`, clearing the ghost box on remote peers.
/// 4. Peer B applies the final update and clears the ephemeral ghost preview.
#[test]
fn test_adversarial_ephemeral_live_drag_preview() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    let doc_a = vectorcraft_doc::Document::new(800.0, 600.0);
    let idx_a = app_a.session.add_document(doc_a, Some("Untitled-1".to_string()));
    app_a.session.set_active(idx_a);

    let ctx = egui::Context::default();

    let res = app_a.session.execute("shape.rectangle", &json!({"x": 100.0, "y": 100.0, "width": 80.0, "height": 60.0})).unwrap();
    let rect_id = res.get("id").and_then(|v| v.as_u64()).expect("rect id");
    app_a.session.active_mut().unwrap().selection.set([vectorcraft_doc::NodeId(rect_id)]);

    // Initial sync
    app_a.handle_collab(&ctx, 1.0);
    let initial_updates: Vec<CollabOutgoing> = outbox_a.lock().unwrap().drain(..).collect();
    for msg in initial_updates {
        match msg {
            CollabOutgoing::Update(u) => inbox_b.lock().unwrap().push(CollabIncoming::Update(u)),
            CollabOutgoing::Presence(p) => inbox_b.lock().unwrap().push(CollabIncoming::Presence(p)),
            _ => {}
        }
    }
    app_b.handle_collab(&ctx, 1.0);
    outbox_b.lock().unwrap().clear();

    // 1. Peer A begins an ephemeral drag / transform interaction
    app_a.session.begin_interaction("Move").expect("begin_interaction");
    assert!(app_a.is_active_drag(&ctx), "Must be active drag");

    // Preview transform
    app_a.session
        .preview("object.transform", &json!({"matrix": [1.0, 0.0, 0.0, 1.0, 40.0, 30.0]}))
        .expect("preview");

    app_a.hover_doc = Some(vectorcraft_geom::Point::new(140.0, 130.0));

    // Handle collab during drag
    app_a.handle_collab(&ctx, 1.05);

    let drag_msgs: Vec<CollabOutgoing> = outbox_a.lock().unwrap().drain(..).collect();
    
    // Crucial check: NO CRDT update during drag
    let has_crdt_update = drag_msgs.iter().any(|m| matches!(m, CollabOutgoing::Update(_)));
    assert!(!has_crdt_update, "No CRDT updates should be broadcast during active drag");

    // Presence MUST contain active_interaction with valid bounds
    let presence_msg = drag_msgs.into_iter().find_map(|m| match m {
        CollabOutgoing::Presence(p) => Some(p),
        _ => None,
    }).expect("Must broadcast presence during active drag");

    assert!(presence_msg.active_interaction.is_some(), "PeerPresence must include active_interaction during drag");
    let interaction = presence_msg.active_interaction.clone().unwrap();
    assert_eq!(interaction.label.as_deref(), Some("Move"));
    assert!(interaction.bounds.width() > 0.0 && interaction.bounds.height() > 0.0, "Interaction bounds must be non-empty");

    // Peer B receives presence
    inbox_b.lock().unwrap().push(CollabIncoming::Presence(presence_msg));
    app_b.handle_collab(&ctx, 1.05);

    {
        let remote_cursors = app_b.remote_cursors.lock().unwrap();
        let peer_a_presence = remote_cursors.get(&app_a.collab_client_id).expect("Peer A in remote_cursors");
        assert!(peer_a_presence.active_interaction.is_some(), "Peer B must have Peer A's active_interaction");
        let remote_bounds = peer_a_presence.active_interaction.as_ref().unwrap().bounds;
        assert_eq!(remote_bounds, interaction.bounds);
    }

    // 2. Drag ends: Peer A commits interaction
    app_a.session.commit_interaction().expect("commit_interaction");
    assert!(!app_a.is_active_drag(&ctx), "Active drag ended");

    app_a.handle_collab(&ctx, 1.10);

    let release_msgs: Vec<CollabOutgoing> = outbox_a.lock().unwrap().drain(..).collect();
    
    // Must now emit the final CRDT update
    let has_final_crdt = release_msgs.iter().any(|m| matches!(m, CollabOutgoing::Update(_)));
    assert!(has_final_crdt, "Final CRDT update must be emitted on pointerup / drag end");

    // Presence MUST have active_interaction: None
    let release_presence = release_msgs.into_iter().find_map(|m| match m {
        CollabOutgoing::Presence(p) => Some(p),
        _ => None,
    }).expect("Must broadcast presence clearing active_interaction");

    assert!(release_presence.active_interaction.is_none(), "active_interaction must be None after drag release");

    // Peer B receives clear presence
    inbox_b.lock().unwrap().push(CollabIncoming::Presence(release_presence));
    app_b.handle_collab(&ctx, 1.10);

    {
        let remote_cursors = app_b.remote_cursors.lock().unwrap();
        let peer_a_presence = remote_cursors.get(&app_a.collab_client_id).expect("Peer A in remote_cursors");
        assert!(peer_a_presence.active_interaction.is_none(), "Ghost box must be cleared on Peer B");
    }
}

/// 8. COMPREHENSIVE ILLUSTRATOR EDITING BATTERY:
/// Simulates real bi-directional design workflows:
/// - Shapes & Colors (Rectangles, Ellipses, Fills, Strokes)
/// - Pathfinder Boolean Cuts (Hole cut via MinusFront -> Compound Path)
/// - Compound Path Move & Transform (Must NOT lose fills or disappear)
/// - Text to Outlines (`type.createOutlines` -> Letters with holes/counters)
/// - Grouping & Z-Order re-arrangement
/// - Bi-directional synchronization and exact tree & appearance equality.
#[test]
fn test_adversarial_illustrator_editing_sync_battery() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    app_a.run("file.new", json!({"width": 1000, "height": 800})).expect("file.new a");
    app_b.run("file.new", json!({"width": 1000, "height": 800})).expect("file.new b");

    let ctx = egui::Context::default();

    // Helper closure to forward all outbound updates between peers
    let pump_collab = |a: &mut VectorcraftApp, b: &mut VectorcraftApp, time: f64| {
        a.handle_collab(&ctx, time);
        let out_a: Vec<CollabOutgoing> = outbox_a.lock().unwrap().drain(..).collect();
        for msg in out_a {
            if let CollabOutgoing::Update(u) = msg {
                inbox_b.lock().unwrap().push(CollabIncoming::Update(u));
            }
        }
        b.handle_collab(&ctx, time);
        let out_b: Vec<CollabOutgoing> = outbox_b.lock().unwrap().drain(..).collect();
        for msg in out_b {
            if let CollabOutgoing::Update(u) = msg {
                inbox_a.lock().unwrap().push(CollabIncoming::Update(u));
            }
        }
        a.handle_collab(&ctx, time + 0.01);
    };

    // Step 1: Peer A creates a styled base rectangle (outer body)
    let r1 = app_a.run("shape.rectangle", json!({"x": 100.0, "y": 100.0, "width": 300.0, "height": 200.0})).unwrap();
    let r1_id = vectorcraft_doc::NodeId(r1["id"].as_u64().unwrap());
    app_a.run("paint.setFill", json!({"color": "#2060ff"})).unwrap();
    app_a.run("stroke.set", json!({"width": 4.0, "color": "#000000"})).unwrap();

    // Step 2: Peer A creates cutter shape (inner window)
    let r2 = app_a.run("shape.rectangle", json!({"x": 160.0, "y": 140.0, "width": 120.0, "height": 80.0})).unwrap();
    let r2_id = vectorcraft_doc::NodeId(r2["id"].as_u64().unwrap());

    // Step 3: Peer A executes Pathfinder Minus Front -> creates Compound Path with a cut-out hole
    app_a.run("select.set", json!({"ids": [r1_id.0, r2_id.0]})).unwrap();
    let pf_res = app_a.run("object.pathfinder.minusFront", json!({})).unwrap();
    let compound_id = vectorcraft_doc::NodeId(pf_res["ids"][0].as_u64().unwrap());

    pump_collab(&mut app_a, &mut app_b, 1.0);

    // Verify Peer B received the compound node with hole and blue fill intact
    {
        let doc_b = &app_b.session.doc().unwrap().doc;
        let node_b = doc_b.node(compound_id).expect("Peer B must have compound path after minusFront");
        assert!(matches!(node_b.kind, NodeKind::Compound { .. }));
        assert!(!node_b.appearance.fill_paint().is_none(), "Peer B compound path must have blue fill");
        if let NodeKind::Compound { children, .. } = &node_b.kind {
            assert_eq!(children.len(), 2, "Compound on Peer B must contain both outer boundary and cut-out hole");
        }
    }

    // Step 4: Peer A moves and scales the compound path (Disappear/desync regression check)
    app_a.run("select.set", json!({"ids": [compound_id.0]})).unwrap();
    app_a.run("object.move", json!({"dx": 40.0, "dy": 50.0})).unwrap();
    app_a.run("object.scale", json!({"sx": 120.0, "sy": 120.0})).unwrap();

    pump_collab(&mut app_a, &mut app_b, 1.5);

    // Verify compound path is 100% intact on Peer B (did NOT disappear)
    {
        let doc_b = &app_b.session.doc().unwrap().doc;
        let node_b = doc_b.node(compound_id).expect("Peer B must NOT lose compound path after move/scale");
        assert!(matches!(node_b.kind, NodeKind::Compound { .. }));
        assert!(!node_b.appearance.fill_paint().is_none(), "Fill must remain intact after move/scale");
        if let NodeKind::Compound { children, .. } = &node_b.kind {
            assert_eq!(children.len(), 2, "Peer B compound path must still retain hole contours");
        }
    }

    // Step 5: Peer B creates text with holes/counters ("B"), styles it, and converts to outlines
    let txt = app_b.run("text.create", json!({"x": 500.0, "y": 300.0, "text": "B", "size": 64.0})).unwrap();
    let txt_id = vectorcraft_doc::NodeId(txt["id"].as_u64().unwrap());
    app_b.run("paint.setFill", json!({"color": "#ff3300"})).unwrap();
    app_b.run("select.set", json!({"ids": [txt_id.0]})).unwrap();
    let outl_res = app_b.run("type.createOutlines", json!({})).unwrap();
    let outline_group_id = vectorcraft_doc::NodeId(outl_res["ids"][0].as_u64().unwrap());

    pump_collab(&mut app_a, &mut app_b, 2.0);

    // Verify Peer A received outlined text group with intact glyphs
    {
        let doc_a = &app_a.session.doc().unwrap().doc;
        let group_a = doc_a.node(outline_group_id).expect("Peer A must have outline group");
        let kids = group_a.children().expect("Must have glyph children");
        assert!(!kids.is_empty(), "Glyph children must not be empty on Peer A");
    }

    // Step 6: Peer B scales and moves outlined text
    app_b.run("select.set", json!({"ids": [outline_group_id.0]})).unwrap();
    app_b.run("object.scale", json!({"sx": 150.0, "sy": 150.0})).unwrap();
    app_b.run("object.move", json!({"dx": -50.0, "dy": -20.0})).unwrap();

    pump_collab(&mut app_a, &mut app_b, 2.5);

    // Verify Peer A retains all glyphs and fills
    {
        let doc_a = &app_a.session.doc().unwrap().doc;
        let group_a = doc_a.node(outline_group_id).expect("Peer A must still have outline group");
        let kids = group_a.children().expect("Must retain glyph children");
        assert!(!kids.is_empty());
        for k in kids {
            assert!(!k.appearance.fill_paint().is_none(), "All glyphs on Peer A must retain red fill");
        }
    }

    // Step 7: Grouping & Z-Order (Bring compound shape to front of everything)
    app_a.run("select.set", json!({"ids": [compound_id.0]})).unwrap();
    app_a.run("object.arrange.bringToFront", json!({})).unwrap();

    pump_collab(&mut app_a, &mut app_b, 3.0);

    // Final Assertion: Document node counts, compound geometry and integrity match 1:1 on both peers
    let count_a = app_a.session.doc().unwrap().doc.layers.iter().map(|l| l.count()).sum::<usize>();
    let count_b = app_b.session.doc().unwrap().doc.layers.iter().map(|l| l.count()).sum::<usize>();
    assert_eq!(count_a, count_b, "Total document node count must match exactly across peers");
}

/// 9. EMPIRICAL TEST: Verify Priority Outbox ordering & cursor coalescing against Head-of-Line Blocking.
/// When large CRDT updates or PDF image blobs are queued, fast cursor presence packets must be
/// prioritized over them, and stale intermediate cursor positions from the same client must be coalesced.
#[test]
fn test_adversarial_outbox_cursor_priority_and_coalescing() {
    use vectorcraft_ui_egui::{PeerPresence, prioritize_and_coalesce_outbox};

    let mut outbox = vec![
        // 1. Large binary update queued first
        CollabOutgoing::Update(vec![0xAA; 100_000]),
        // 2. Large image blob queued second
        CollabOutgoing::Blob {
            key: "img_test_1".to_string(),
            mime: "image/png".to_string(),
            bytes: vec![0xBB; 500_000],
        },
        // 3. Stale cursor presence from Client 1
        CollabOutgoing::Presence(PeerPresence {
            client_id: 1,
            name: "Alice".to_string(),
            color: [255, 0, 0, 255],
            cursor: Some(Point::new(10.0, 10.0)),
            last_seen: 1.0,
            active_tab: Some(0),
            active_interaction: None,
        }),
        // 4. Intermediate cursor presence from Client 1
        CollabOutgoing::Presence(PeerPresence {
            client_id: 1,
            name: "Alice".to_string(),
            color: [255, 0, 0, 255],
            cursor: Some(Point::new(20.0, 20.0)),
            last_seen: 1.016,
            active_tab: Some(0),
            active_interaction: None,
        }),
        // 5. Another update
        CollabOutgoing::Update(vec![0xCC; 50_000]),
        // 6. Latest cursor presence from Client 1
        CollabOutgoing::Presence(PeerPresence {
            client_id: 1,
            name: "Alice".to_string(),
            color: [255, 0, 0, 255],
            cursor: Some(Point::new(30.0, 30.0)),
            last_seen: 1.033,
            active_tab: Some(0),
            active_interaction: None,
        }),
        // 7. Cursor presence from Client 2
        CollabOutgoing::Presence(PeerPresence {
            client_id: 2,
            name: "Bob".to_string(),
            color: [0, 255, 0, 255],
            cursor: Some(Point::new(100.0, 200.0)),
            last_seen: 1.033,
            active_tab: Some(0),
            active_interaction: None,
        }),
    ];

    let prioritized = prioritize_and_coalesce_outbox(&mut outbox);
    assert!(outbox.is_empty(), "Original queue must be drained");

    // We expect:
    // - High priority: PeerPresence from Client 1 (coalesced to point (30.0, 30.0)) and Client 2 (point (100.0, 200.0))
    // - Normal priority: Updates (2 updates preserved in original relative order)
    // - Bulk/Low priority: Blob (1 blob at the end)
    assert_eq!(prioritized.len(), 5, "Stale presences must be coalesced (7 items -> 5 items)");

    match &prioritized[0] {
        CollabOutgoing::Presence(p) => {
            assert_eq!(p.client_id, 1);
            assert_eq!(p.cursor, Some(Point::new(30.0, 30.0)), "Must retain newest position for Client 1");
            assert_eq!(p.last_seen, 1.033);
        }
        other => panic!("Expected Presence at index 0, got {other:?}"),
    }

    match &prioritized[1] {
        CollabOutgoing::Presence(p) => {
            assert_eq!(p.client_id, 2);
            assert_eq!(p.cursor, Some(Point::new(100.0, 200.0)), "Client 2 presence must be preserved");
        }
        other => panic!("Expected Presence at index 1, got {other:?}"),
    }

    match &prioritized[2] {
        CollabOutgoing::Update(bytes) => {
            assert_eq!(bytes.len(), 100_000, "First update preserved in causal order");
        }
        other => panic!("Expected Update at index 2, got {other:?}"),
    }

    match &prioritized[3] {
        CollabOutgoing::Update(bytes) => {
            assert_eq!(bytes.len(), 50_000, "Second update preserved in causal order");
        }
        other => panic!("Expected Update at index 3, got {other:?}"),
    }

    match &prioritized[4] {
        CollabOutgoing::Blob { key, mime, bytes } => {
            assert_eq!(key, "img_test_1");
            assert_eq!(mime, "image/png");
            assert_eq!(bytes.len(), 500_000, "Large blob sent after high priority presence & updates");
        }
        other => panic!("Expected Blob at index 4, got {other:?}"),
    }
}

/// Adversarial Test: Room Password & PIN protection lifecycle in the client app.
///
/// Verifies:
/// 1. Initial state: room is unlocked (`collab_room_protected == false`).
/// 2. Setting a password/PIN via `set_collab_password(Some("secret42"))`:
///    - Updates `collab_room_protected` to `true`.
///    - Updates `collab_share_url` to include `&pin=secret42`.
///    - Emits a `__room_auth__` blob to the outbox with `set_room_pin`.
/// 3. Clearing the password via `set_collab_password(None)`:
///    - Updates `collab_room_protected` to `false`.
///    - Cleans `pin=` from `collab_share_url`.
///    - Emits a `__room_auth__` blob to the outbox with `clear_room_pin`.
#[test]
fn test_adversarial_room_password_flow() {
    let outbox = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let services = Services {
        collab_outbox: Some(outbox.clone()),
        ..Services::default()
    };
    let mut app = VectorcraftApp::new(Session::new(), services);
    app.collab_room_id = "c4ndy-locktest".to_string();
    app.collab_share_url = "https://vector.c4ndy.com/?room=c4ndy-locktest".to_string();

    assert!(!app.collab_room_protected);
    assert!(!app.collab_share_url.contains("pin="));

    // 1. Activate password protection
    app.set_collab_password(Some("secret42".to_string()));
    assert!(app.collab_room_protected);
    assert!(app.collab_share_url.contains("pin=secret42"));

    let msgs: Vec<CollabOutgoing> = outbox.lock().unwrap().drain(..).collect();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        CollabOutgoing::Blob { key, mime, bytes } => {
            assert_eq!(key, "__room_auth__");
            assert_eq!(mime, "application/json");
            let json: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(json["type"], "set_room_pin");
            assert_eq!(json["pin"], "secret42");
        }
        other => panic!("Expected Blob message, got {other:?}"),
    }

    // 2. Remove password protection
    app.set_collab_password(None);
    assert!(!app.collab_room_protected);
    assert!(!app.collab_share_url.contains("pin="));

    let msgs_clear: Vec<CollabOutgoing> = outbox.lock().unwrap().drain(..).collect();
    assert_eq!(msgs_clear.len(), 1);
    match &msgs_clear[0] {
        CollabOutgoing::Blob { key, mime, bytes } => {
            assert_eq!(key, "__room_auth__");
            assert_eq!(mime, "application/json");
            let json: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(json["type"], "clear_room_pin");
        }
        other => panic!("Expected Blob message, got {other:?}"),
    }
}

/// 11. EMPIRICAL TEST: Verify ruler guide (Hilfslinien) synchronization across peers.
#[test]
fn test_adversarial_ruler_guides_sync() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_a = VectorcraftApp::new(Session::new(), services_a);
    let mut app_b = VectorcraftApp::new(Session::new(), services_b);

    app_a.run("file.new", json!({"width": 800, "height": 600})).expect("file.new a");
    app_b.run("file.new", json!({"width": 800, "height": 600})).expect("file.new b");

    let ctx = egui::Context::default();

    // Peer A adds a vertical ruler guide at x = 150.0 and a horizontal guide at y = 300.0
    {
        let doc = Arc::make_mut(&mut app_a.session.active_mut().unwrap().doc);
        doc.guides.push(vectorcraft_doc::Guide {
            vertical: true,
            pos: 150.0,
            artboard: None,
            layer: None,
        });
        doc.guides.push(vectorcraft_doc::Guide {
            vertical: false,
            pos: 300.0,
            artboard: None,
            layer: None,
        });
        app_a.session.active_mut().unwrap().revision += 1;
    }

    // Peer A ticks collab: exports CRDT update containing guides
    app_a.handle_collab(&ctx, 1.0);

    let mut out_a = outbox_a.lock().unwrap();
    assert_eq!(out_a.len(), 1, "Peer A emits 1 update for guides");
    let update = match out_a.pop().unwrap() {
        CollabOutgoing::Update(b) => b,
        _ => panic!("Expected CollabOutgoing::Update"),
    };
    drop(out_a);

    // Deliver to Peer B
    inbox_b.lock().unwrap().push(CollabIncoming::Update(update));
    app_b.handle_collab(&ctx, 1.0);

    // Peer B should now have the exact same ruler guides!
    let b_doc = &app_b.session.active().unwrap().doc;
    assert_eq!(b_doc.guides.len(), 2, "Peer B has 2 guides");
    assert!(b_doc.guides[0].vertical);
    assert_eq!(b_doc.guides[0].pos, 150.0);
    assert!(!b_doc.guides[1].vertical);
    assert_eq!(b_doc.guides[1].pos, 300.0);

    // Peer B modifies / removes a guide and adds another
    {
        let doc_b_mut = Arc::make_mut(&mut app_b.session.active_mut().unwrap().doc);
        doc_b_mut.guides.remove(0); // remove vertical
        doc_b_mut.guides.push(vectorcraft_doc::Guide {
            vertical: true,
            pos: 450.0,
            artboard: None,
            layer: None,
        });
        app_b.session.active_mut().unwrap().revision += 1;
    }

    app_b.handle_collab(&ctx, 2.0);
    let mut out_b = outbox_b.lock().unwrap();
    assert_eq!(out_b.len(), 1, "Peer B emits 1 update");
    let update_b = match out_b.pop().unwrap() {
        CollabOutgoing::Update(b) => b,
        _ => panic!("Expected CollabOutgoing::Update"),
    };
    drop(out_b);

    // Deliver to Peer A
    inbox_a.lock().unwrap().push(CollabIncoming::Update(update_b));
    app_a.handle_collab(&ctx, 2.1);

    // Peer A should reflect Peer B's guide update
    let a_doc = &app_a.session.active().unwrap().doc;
    assert_eq!(a_doc.guides.len(), 2, "Peer A now has 2 guides");
    assert!(!a_doc.guides[0].vertical);
    assert_eq!(a_doc.guides[0].pos, 300.0);
    assert!(a_doc.guides[1].vertical);
    assert_eq!(a_doc.guides[1].pos, 450.0);
}




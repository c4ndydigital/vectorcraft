//! Adversarial test harness for Milestone 3 Collab Sync and Presence.

use serde_json::json;
use std::sync::{Arc, Mutex};
use vectorcraft_doc::NodeKind;
use vectorcraft_engine::collab::CollabBridge;
use vectorcraft_engine::Session;
use vectorcraft_geom::Point;
use vectorcraft_ui_egui::{CollabIncoming, CollabOutgoing, Services, VectorcraftApp};

/// 1. EMPIRICAL TEST: Verify echo loop prevention across round-trips and self-reflections.
#[test]
fn test_adversarial_echo_loop_prevention() {
    let inbox_a = Arc::new(Mutex::new(Vec::new()));
    let outbox_a = Arc::new(Mutex::new(Vec::new()));
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_a = Services {
        collab_inbox: Some(inbox_a.clone()),
        collab_outbox: Some(outbox_a.clone()),
        ..Default::default()
    };
    let services_b = Services {
        collab_inbox: Some(inbox_b.clone()),
        collab_outbox: Some(outbox_b.clone()),
        ..Default::default()
    };

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

    let services = Services {
        collab_inbox: Some(inbox),
        collab_outbox: Some(outbox.clone()),
        ..Default::default()
    };

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
        assert!(
            dt >= 0.033 - 1e-6,
            "Interval between broadcasts must be >= 0.033s, found dt = {dt}"
        );
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
    assert!(
        (2..=4).contains(&heartbeat_count),
        "Heartbeat broadcast over 3s should be ~3, got {heartbeat_count}"
    );

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
    assert!(
        outbox.lock().unwrap().is_empty(),
        "No broadcasts while cursor remains None"
    );
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
    let rect_res = session_a
        .execute("shape.rectangle", &json!({"x": 50.0, "y": 75.0, "width": 120.0, "height": 90.0}))
        .unwrap();
    let rect_id = rect_res["id"].as_u64().unwrap();

    // Export update from A
    let update_a = bridge_a.sync_from_session(&session_a).expect("sync from A");
    assert!(!update_a.is_empty(), "CRDT update must be non-empty");

    // Apply remote update to B
    bridge_b.apply_remote_update(&mut session_b, &update_a).expect("apply update to B");

    // Assert that Remote (B) has a NEW Arc<Document>
    let current_arc_b = Arc::clone(&session_b.active().unwrap().doc);
    assert!(
        !Arc::ptr_eq(&initial_arc_b, &current_arc_b),
        "Applying remote update must produce a new Arc<Document> pointer"
    );

    // Assert that revision was bumped
    let current_rev_b = session_b.active().unwrap().revision;
    assert!(
        current_rev_b > initial_rev_b,
        "Applying remote update must bump revision: initial={initial_rev_b}, current={current_rev_b}"
    );

    // Assert that the shape exists in B with matching geometry
    let node_in_b = session_b.active().unwrap().doc.node(vectorcraft_doc::NodeId(rect_id)).expect("shape in B");
    assert!(matches!(node_in_b.kind, NodeKind::Path { .. }));

    // 2. Now Peer B modifies the shape (reverse direction sync)
    let initial_arc_a = Arc::clone(&session_a.active().unwrap().doc);
    let initial_rev_a = session_a.active().unwrap().revision;

    session_b
        .execute("paint.setFill", &json!({"ids": [rect_id], "color": "#00ff88"}))
        .unwrap();

    let update_b = bridge_b.sync_from_session(&session_b).expect("sync from B");
    assert!(!update_b.is_empty(), "Peer B modification produces CRDT update");

    bridge_a.apply_remote_update(&mut session_a, &update_b).expect("apply update to A");

    // Assert Peer A also gets a NEW Arc<Document> and bumped revision
    let current_arc_a = Arc::clone(&session_a.active().unwrap().doc);
    assert!(
        !Arc::ptr_eq(&initial_arc_a, &current_arc_a),
        "Applying reverse remote update must produce a new Arc<Document> on Peer A"
    );
    assert!(
        session_a.active().unwrap().revision > initial_rev_a,
        "Revision on Peer A must be bumped"
    );

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

//! Integration test for CRDT State Bridge and two-way shape synchronization.

use serde_json::json;
use vectorcraft_doc::NodeId;
use vectorcraft_engine::collab::CollabBridge;
use vectorcraft_testkit::fixtures::{select, session_with};
use vectorcraft_testkit::invariants::check_session;

#[test]
fn test_crdt_rectangle_bidirectional_sync() {
    // 1. Initialize two independent engine/document instances
    let mut session_a = session_with(800.0, 600.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(101);
    let mut bridge_b = CollabBridge::with_client_id(202);

    // Initial session invariants hold
    check_session(&session_a).expect("session_a initially sane");
    check_session(&session_b).expect("session_b initially sane");

    // 2. Create a basic shape (rectangle) in session_a
    let rect_val = session_a
        .execute(
            "shape.rectangle",
            &json!({
                "x": 100.0,
                "y": 150.0,
                "width": 200.0,
                "height": 120.0,
            }),
        )
        .expect("create rectangle in session_a");

    let rect_id = NodeId(rect_val["id"].as_u64().expect("valid rect id"));
    check_session(&session_a).expect("session_a sane after rect creation");

    let node_a_init = session_a.doc().expect("doc_a").doc.node(rect_id).expect("node in session_a").clone();

    // 3. Sync CRDT update from session_a to session_b
    let update_a_to_b = bridge_a.sync_from_session(&session_a).expect("sync_from_session on bridge_a");
    assert!(!update_a_to_b.is_empty(), "update should contain bytes");

    bridge_b.apply_remote_update(&mut session_b, &update_a_to_b).expect("apply_remote_update on bridge_b");

    // 4. Assert session_b reflects the exact same rectangle:
    //    ID, node kind, geometric bounds, appearance, doc invariants.
    check_session(&session_b).expect("session_b invariants hold after applying remote update");

    let node_b = session_b.doc().expect("doc_b").doc.node(rect_id).expect("node must exist in session_b");

    assert_eq!(node_b.id, node_a_init.id, "Node IDs must match");
    assert_eq!(node_b.kind, node_a_init.kind, "NodeKind must match");
    assert_eq!(node_b.geometric_bounds(), node_a_init.geometric_bounds(), "Geometric bounds must match");
    assert_eq!(node_b.appearance, node_a_init.appearance, "Appearance must match");

    // 5. Modify the shape in session_b:
    //    Select it, move it with object.move, and change its fill with paint.setFill
    select(&mut session_b, &[rect_id]);

    session_b.execute("object.move", &json!({ "dx": 50.0, "dy": 30.0 })).expect("object.move in session_b");

    session_b.execute("paint.setFill", &json!({ "color": "#e03030" })).expect("paint.setFill in session_b");

    check_session(&session_b).expect("session_b sane after edits");

    let node_b_mod = session_b.doc().expect("doc_b").doc.node(rect_id).expect("node in session_b after mod").clone();

    // 6. Sync CRDT update from session_b back to session_a
    let update_b_to_a = bridge_b.sync_from_session(&session_b).expect("sync_from_session on bridge_b");
    assert!(!update_b_to_a.is_empty(), "update must not be empty");

    bridge_a.apply_remote_update(&mut session_a, &update_b_to_a).expect("apply_remote_update on bridge_a");

    // 7. Assert bidirectional convergence
    check_session(&session_a).expect("session_a sane after remote update");

    let node_a_final = session_a.doc().expect("doc_a").doc.node(rect_id).expect("node in session_a after convergence");

    assert_eq!(node_a_final.kind, node_b_mod.kind, "Node kinds converged");
    assert_eq!(node_a_final.appearance, node_b_mod.appearance, "Appearance converged");
    assert_eq!(node_a_final.geometric_bounds(), node_b_mod.geometric_bounds(), "Geometric bounds converged");
    assert_eq!(
        node_a_final.geometric_bounds(),
        Some(vectorcraft_geom::Rect::new(150.0, 180.0, 350.0, 300.0)),
        "Exact moved bounds match (100+50, 150+30, 300+50, 270+30)"
    );

    // 8. Test shape deletion propagation
    session_a.execute("edit.clear", &json!({ "ids": [rect_id.0] })).expect("delete object in session_a");
    assert!(session_a.doc().expect("doc_a").doc.node(rect_id).is_none(), "node removed in session_a");

    let del_update = bridge_a.sync_from_session(&session_a).expect("sync deletion from session_a");

    bridge_b.apply_remote_update(&mut session_b, &del_update).expect("apply deletion to session_b");

    assert!(session_b.doc().expect("doc_b").doc.node(rect_id).is_none(), "node removed in session_b after sync");
    check_session(&session_a).expect("session_a invariants hold after delete");
    check_session(&session_b).expect("session_b invariants hold after delete");
}

#[test]
fn test_crdt_export_update_snapshot() {
    let mut session_a = session_with(600.0, 400.0);
    let mut session_b = session_with(600.0, 400.0);

    let mut bridge_a = CollabBridge::new();
    let mut bridge_b = CollabBridge::new();

    let r = session_a.execute("shape.rectangle", &json!({ "x": 10.0, "y": 20.0, "width": 80.0, "height": 50.0 })).expect("create rect");
    let rect_id = NodeId(r["id"].as_u64().expect("id"));

    // Sync bridge_a from session_a
    bridge_a.sync_from_session(&session_a).expect("sync_from_session");

    // Export full update snapshot
    let snapshot = bridge_a.export_update();
    assert!(!snapshot.is_empty(), "snapshot must contain data");

    // Apply snapshot to fresh bridge_b / session_b
    bridge_b.apply_remote_update(&mut session_b, &snapshot).expect("apply snapshot");

    let node_b = session_b.doc().expect("doc_b").doc.node(rect_id).expect("node in b");
    assert_eq!(node_b.geometric_bounds(), Some(vectorcraft_geom::Rect::new(10.0, 20.0, 90.0, 70.0)));
    check_session(&session_b).expect("invariants on session_b");
}

#[test]
fn test_crdt_artboard_dimension_sync() {
    let session_a = session_with(1200.0, 900.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(301);
    let mut bridge_b = CollabBridge::with_client_id(302);

    // Initial artboard dimensions differ
    assert_eq!(session_a.doc().unwrap().doc.artboards[0].rect.width(), 1200.0);
    assert_eq!(session_b.doc().unwrap().doc.artboards[0].rect.width(), 800.0);

    // Sync session_a (1200x900) to session_b
    let update_a = bridge_a.sync_from_session(&session_a).expect("sync a");
    bridge_b.apply_remote_update(&mut session_b, &update_a).expect("apply to b");

    // Session B artboard dimensions must now match Session A
    let artboard_b = &session_b.doc().unwrap().doc.artboards[0];
    assert_eq!(artboard_b.rect.width(), 1200.0);
    assert_eq!(artboard_b.rect.height(), 900.0);
}

#[test]
fn test_crdt_z_order_synchronization() {
    let mut session_a = session_with(800.0, 600.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(401);
    let mut bridge_b = CollabBridge::with_client_id(402);

    // Create bottom rect 1 and top rect 2
    let r1 = session_a.execute("shape.rectangle", &json!({ "x": 10.0, "y": 10.0, "width": 50.0, "height": 50.0 })).unwrap();
    let r2 = session_a.execute("shape.rectangle", &json!({ "x": 20.0, "y": 20.0, "width": 50.0, "height": 50.0 })).unwrap();
    let id1 = NodeId(r1["id"].as_u64().unwrap());
    let id2 = NodeId(r2["id"].as_u64().unwrap());

    // Initially id1 is at index 0, id2 is at index 1
    let pos1_init = session_a.doc().unwrap().doc.position(id1).unwrap().1;
    let pos2_init = session_a.doc().unwrap().doc.position(id2).unwrap().1;
    assert!(pos1_init < pos2_init);

    // Send r2 to back (now r2 is at index 0, r1 is at index 1)
    select(&mut session_a, &[id2]);
    session_a.execute("object.arrange.sendToBack", &json!({})).unwrap();

    let pos2_after = session_a.doc().unwrap().doc.position(id2).unwrap().1;
    let pos1_after = session_a.doc().unwrap().doc.position(id1).unwrap().1;
    assert!(pos2_after < pos1_after, "r2 should now be behind r1");

    // Sync session_a to session_b
    let update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &update).unwrap();

    // Verify session_b reflects the exact same Z-order
    let pos2_b = session_b.doc().unwrap().doc.position(id2).unwrap().1;
    let pos1_b = session_b.doc().unwrap().doc.position(id1).unwrap().1;
    assert_eq!(pos2_b, pos2_after);
    assert_eq!(pos1_b, pos1_after);
    assert!(pos2_b < pos1_b, "r2 must be behind r1 in session_b as well");
}

#[test]
fn test_crdt_group_hierarchy_and_name_sync() {
    let mut session_a = session_with(800.0, 600.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(501);
    let mut bridge_b = CollabBridge::with_client_id(502);

    // Create 2 shapes in session_a
    let r1 = session_a.execute("shape.rectangle", &json!({ "x": 10.0, "y": 10.0, "width": 50.0, "height": 50.0 })).unwrap();
    let r2 = session_a.execute("shape.rectangle", &json!({ "x": 70.0, "y": 70.0, "width": 50.0, "height": 50.0 })).unwrap();
    let id1 = NodeId(r1["id"].as_u64().unwrap());
    let id2 = NodeId(r2["id"].as_u64().unwrap());

    // Rename shape 1
    std::sync::Arc::make_mut(&mut session_a.doc_mut().unwrap().doc).node_mut(id1).unwrap().name = Some("CustomLogoShape".to_string());

    // Select both and group them
    select(&mut session_a, &[id1, id2]);
    let grp_res = session_a.execute("object.group", &json!({})).unwrap();
    let group_id = NodeId(grp_res["id"].as_u64().unwrap());

    // Rename the group
    std::sync::Arc::make_mut(&mut session_a.doc_mut().unwrap().doc).node_mut(group_id).unwrap().name = Some("BrandingGroup".to_string());

    // Verify local structure
    assert_eq!(session_a.doc().unwrap().doc.parent_of(id1), Some(group_id));
    assert_eq!(session_a.doc().unwrap().doc.parent_of(id2), Some(group_id));

    // Sync from session_a to session_b
    let update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &update).unwrap();

    // Verify session_b received the group, child relationships, names, and Z-orders
    let doc_b = &session_b.doc().unwrap().doc;
    assert!(doc_b.node(group_id).is_some(), "Group node must exist on peer B");
    assert_eq!(doc_b.node(group_id).unwrap().name, Some("BrandingGroup".to_string()));
    assert_eq!(doc_b.parent_of(id1), Some(group_id), "Shape 1 must be child of BrandingGroup on peer B");
    assert_eq!(doc_b.parent_of(id2), Some(group_id), "Shape 2 must be child of BrandingGroup on peer B");
    assert_eq!(doc_b.node(id1).unwrap().name, Some("CustomLogoShape".to_string()));

    // Nested hierarchy test: create a 3rd shape and group it with the existing group
    let r3 = session_a.execute("shape.rectangle", &json!({ "x": 150.0, "y": 150.0, "width": 80.0, "height": 80.0 })).unwrap();
    let id3 = NodeId(r3["id"].as_u64().unwrap());
    select(&mut session_a, &[group_id, id3]);
    let outer_grp_res = session_a.execute("object.group", &json!({})).unwrap();
    let outer_group_id = NodeId(outer_grp_res["id"].as_u64().unwrap());
    std::sync::Arc::make_mut(&mut session_a.doc_mut().unwrap().doc).node_mut(outer_group_id).unwrap().name = Some("HeaderContainer".to_string());

    // Sync nested tree to session_b
    let nested_update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &nested_update).unwrap();

    let doc_b_nested = &session_b.doc().unwrap().doc;
    assert!(doc_b_nested.node(outer_group_id).is_some());
    assert_eq!(doc_b_nested.node(outer_group_id).unwrap().name, Some("HeaderContainer".to_string()));
    assert_eq!(doc_b_nested.parent_of(group_id), Some(outer_group_id), "Inner group must be nested inside HeaderContainer");
    assert_eq!(doc_b_nested.parent_of(id3), Some(outer_group_id), "Shape 3 must be inside HeaderContainer");
    assert_eq!(doc_b_nested.parent_of(id1), Some(group_id), "Shape 1 must remain inside inner BrandingGroup");
}

#[test]
fn test_crdt_multi_tab_creation_and_shape_isolation() {
    let mut session_a = session_with(800.0, 600.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(701);
    let mut bridge_b = CollabBridge::with_client_id(702);

    // Initial sync between A and B so B learns tab 0 ID
    let initial_update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &initial_update).unwrap();

    // Create a shape in tab 0 on session A
    let r_tab0 = session_a.execute("shape.rectangle", &json!({ "x": 10.0, "y": 10.0, "width": 50.0, "height": 50.0 })).unwrap();
    let id_tab0 = NodeId(r_tab0["id"].as_u64().unwrap());

    // Sync tab 0 shape to B
    let update_tab0 = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &update_tab0).unwrap();
    assert!(session_b.doc().unwrap().doc.node(id_tab0).is_some(), "Shape from tab 0 must be in tab 0 on peer B");

    // Peer A creates a new document / tab (Tab 1)
    session_a.execute("file.new", &json!({ "width": 1024.0, "height": 768.0 })).unwrap();
    assert_eq!(session_a.documents().len(), 2);
    assert_eq!(session_a.active_index(), Some(1));

    // Peer A creates a shape in Tab 1
    let r_tab1 = session_a.execute("shape.ellipse", &json!({ "x": 200.0, "y": 150.0, "width": 80.0, "height": 60.0 })).unwrap();
    let id_tab1 = NodeId(r_tab1["id"].as_u64().unwrap());

    // Give shapes distinctive names to test isolation regardless of node ID numbering
    std::sync::Arc::make_mut(&mut session_a.doc_mut().unwrap().doc).node_mut(id_tab1).unwrap().name = Some("Tab1Ellipse".to_string());
    session_a.set_active(0);
    std::sync::Arc::make_mut(&mut session_a.doc_mut().unwrap().doc).node_mut(id_tab0).unwrap().name = Some("Tab0Rectangle".to_string());
    session_a.set_active(1);

    // Sync from Peer A to Peer B
    let multi_tab_update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &multi_tab_update).unwrap();

    // Verify Peer B now has 2 documents/tabs
    assert_eq!(session_b.documents().len(), 2, "Peer B must have 2 documents/tabs after sync");

    // Verify Peer B's active focus was NOT stolen (Peer B was on tab 0 and must stay on tab 0)
    assert_eq!(session_b.active_index(), Some(0), "Peer B active tab must remain 0 (focus not stolen)");

    // Verify Tab 0 on Peer B still has the tab 0 shape and does NOT have the tab 1 shape
    assert_eq!(
        session_b.doc().unwrap().doc.node(id_tab0).and_then(|n| n.name.as_deref()),
        Some("Tab0Rectangle"),
        "Tab 0 on peer B must retain its Tab0Rectangle shape"
    );

    // Switch Peer B to Tab 1 and verify Tab 1 has the Tab1Ellipse shape
    session_b.set_active(1);
    assert_eq!(
        session_b.doc().unwrap().doc.node(id_tab1).and_then(|n| n.name.as_deref()),
        Some("Tab1Ellipse"),
        "Tab 1 on peer B must contain the Tab1Ellipse shape"
    );
}

#[test]
fn test_crdt_multi_tab_close_sync() {
    let mut session_a = session_with(800.0, 600.0);
    let mut session_b = session_with(800.0, 600.0);

    let mut bridge_a = CollabBridge::with_client_id(801);
    let mut bridge_b = CollabBridge::with_client_id(802);

    // Initial sync
    let init_update = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &init_update).unwrap();

    // Create a shape in tab 0
    let r0 = session_a.execute("shape.rectangle", &json!({ "x": 10.0, "y": 10.0, "width": 50.0, "height": 50.0 })).unwrap();
    let id_tab0 = NodeId(r0["id"].as_u64().unwrap());

    // Peer A adds a second tab (Tab 1) with an ellipse
    session_a.execute("file.new", &json!({ "width": 1024.0, "height": 768.0 })).unwrap();
    let _r1 = session_a.execute("shape.ellipse", &json!({ "x": 200.0, "y": 150.0, "width": 80.0, "height": 60.0 })).unwrap();

    // Sync from Peer A to Peer B
    let update_open = bridge_a.sync_from_session(&session_a).unwrap();
    bridge_b.apply_remote_update(&mut session_b, &update_open).unwrap();

    assert_eq!(session_a.documents().len(), 2);
    assert_eq!(session_b.documents().len(), 2);
    assert_eq!(bridge_a.tabs_count(), 2);
    assert_eq!(bridge_b.tabs_count(), 2);

    // Now Peer A closes Tab 1
    let closed = session_a.close_document(1);
    assert!(closed, "session_a should close tab 1");
    assert_eq!(session_a.documents().len(), 1);

    // Sync close from Peer A to Peer B
    let update_close = bridge_a.sync_from_session(&session_a).unwrap();
    assert!(!update_close.is_empty(), "Close update should produce CRDT delta bytes");
    assert_eq!(bridge_a.tabs_count(), 1, "Bridge A should have only 1 tab tracked in CRDT");

    bridge_b.apply_remote_update(&mut session_b, &update_close).unwrap();

    // Verify Peer B now has only 1 document/tab as well!
    assert_eq!(session_b.documents().len(), 1, "Peer B must close tab 1 in response to remote close");
    assert_eq!(bridge_b.tabs_count(), 1, "Bridge B should have only 1 tab tracked in CRDT");

    // Verify Tab 0 on both peers still has the tab 0 shape intact
    assert!(session_a.doc().unwrap().doc.node(id_tab0).is_some());
    assert!(session_b.doc().unwrap().doc.node(id_tab0).is_some());

    // Verify Tab 1's shape is no longer tracked anywhere in CRDT
    assert_eq!(bridge_a.shapes_count(), 1, "Only Tab 0 shape remains in Bridge A");
    assert_eq!(bridge_b.shapes_count(), 1, "Only Tab 0 shape remains in Bridge B");

    // Two-way verification: Peer B opens a new tab and then closes it
    session_b.execute("file.new", &json!({ "width": 500.0, "height": 500.0 })).unwrap();
    let _r_b2 = session_b.execute("shape.rectangle", &json!({ "x": 30.0, "y": 30.0, "width": 40.0, "height": 40.0 })).unwrap();

    let b_open_update = bridge_b.sync_from_session(&session_b).unwrap();
    bridge_a.apply_remote_update(&mut session_a, &b_open_update).unwrap();

    assert_eq!(session_a.documents().len(), 2);
    assert_eq!(session_b.documents().len(), 2);

    // Peer B closes this newly created tab (index 1)
    let closed_b = session_b.close_document(1);
    assert!(closed_b);
    let b_close_update = bridge_b.sync_from_session(&session_b).unwrap();
    bridge_a.apply_remote_update(&mut session_a, &b_close_update).unwrap();

    assert_eq!(session_a.documents().len(), 1, "Peer A must have 1 document after Peer B closes tab");
    assert_eq!(session_b.documents().len(), 1, "Peer B must have 1 document");
    assert_eq!(bridge_a.tabs_count(), 1);
    assert_eq!(bridge_b.tabs_count(), 1);
    assert_eq!(bridge_a.shapes_count(), 1);
    assert_eq!(bridge_b.shapes_count(), 1);
}

#[test]
fn test_crdt_room_red_square_injector() {
    let snapshot_path = std::path::Path::new("/tmp/live_room_c4ndy1404.bin");
    let mut session = vectorcraft_engine::Session::new();
    let mut bridge = CollabBridge::with_client_id(777_777);

    let has_snapshot = snapshot_path.exists()
        && std::fs::metadata(snapshot_path).map(|m| m.len() > 2).unwrap_or(false);

    if has_snapshot {
        let snapshot_bytes = std::fs::read(snapshot_path).unwrap();
        let _ = bridge.apply_remote_update(&mut session, &snapshot_bytes);
        println!("Loaded snapshot ({} bytes), active docs: {}", snapshot_bytes.len(), session.documents().len());
    }

    if session.documents().is_empty() {
        let _ = session.execute("file.new", &json!({"width": 800.0, "height": 600.0}));
        let _ = bridge.sync_from_session(&session);
    }

    let rect_val = session
        .execute(
            "shape.rectangle",
            &json!({
                "x": 200.0,
                "y": 150.0,
                "width": 250.0,
                "height": 250.0,
            }),
        )
        .expect("create rectangle");

    let rect_id = NodeId(rect_val["id"].as_u64().expect("valid rect id"));
    vectorcraft_testkit::fixtures::select(&mut session, &[rect_id]);
    let _ = session.execute("paint.setFill", &json!({ "color": "#ff0000" }));

    let update = bridge.sync_from_session(&session).expect("sync from session");
    let full_state = bridge.export_update();

    std::fs::write("/tmp/red_square_diff.bin", &update).unwrap();
    std::fs::write("/tmp/red_square_full.bin", &full_state).unwrap();
    println!("Exported diff update: {} bytes, full state: {} bytes", update.len(), full_state.len());
}

#[test]
fn test_render_red_square_to_png() {
    let snapshot_bytes = std::fs::read("/tmp/red_square_full.bin").expect("read full state");
    let mut session = vectorcraft_engine::Session::new();
    let mut bridge = CollabBridge::with_client_id(999);
    
    // In vectorcraft-web, session starts with file.new:
    let _ = session.execute("file.new", &json!({"width": 800.0, "height": 600.0}));
    println!("Session before apply: docs count = {}", session.documents().len());
    
    bridge.apply_remote_update(&mut session, &snapshot_bytes).expect("apply full state");
    
    println!("Session after apply: docs count = {}", session.documents().len());
    for (i, doc_st) in session.documents().iter().enumerate() {
        println!("Doc {}: uid={}, rev={}, artboards={:?}", i, doc_st.uid, doc_st.revision, doc_st.doc.artboards);
        doc_st.doc.walk(|node| {
            println!("  Node: id={}, name={:?}, kind={:?}, bounds={:?}", node.id, node.name, node.kind, node.geometric_bounds());
        });
        
        let mut renderer = vectorcraft_render::Renderer::new();
        let opts = vectorcraft_render::RenderOptions::default();
        let img = renderer.render(&doc_st.doc, 800, 600, vectorcraft_geom::Affine::IDENTITY, &opts);
        let png_bytes = img.to_png().expect("to png");
        std::fs::write("/tmp/rendered_doc.png", png_bytes).expect("write png");
        println!("Saved /tmp/rendered_doc.png!");
    }
}


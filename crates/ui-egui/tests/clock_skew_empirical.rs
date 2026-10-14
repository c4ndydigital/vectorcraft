//! Empirical Challenger verification test for clock skew and cursor pruning.

use std::sync::{Arc, Mutex};
use vectorcraft_engine::Session;
use vectorcraft_geom::Point;
use vectorcraft_ui_egui::{CollabIncoming, PeerPresence, Services, VectorcraftApp};

#[test]
fn test_clock_skew_resistance_and_pruning() {
    let inbox_b = Arc::new(Mutex::new(Vec::new()));
    let outbox_b = Arc::new(Mutex::new(Vec::new()));

    let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

    let mut app_b = VectorcraftApp::new(Session::new(), services_b);
    let ctx = egui::Context::default();

    // Client B's local monotonic clock is at 100.0 seconds
    let local_uptime_b = 100.0;

    // Simulate incoming PeerPresence from Client A, whose local clock is at 5.0 seconds (massive 95s clock skew)
    let foreign_uptime_a = 5.0;
    let peer_a_presence = PeerPresence {
        client_id: 42,
        name: "ForeignClockPeer".to_string(),
        color: [255, 0, 128, 255],
        cursor: Some(Point::new(150.0, 250.0)),
        last_seen: foreign_uptime_a,
        active_tab: None,
        active_interaction: None,
    };

    inbox_b.lock().unwrap().push(CollabIncoming::Presence(peer_a_presence));

    // Handle collab at t = 100.0 on Client B
    app_b.handle_collab(&ctx, local_uptime_b);

    // Verify Peer A's cursor was NOT pruned and is currently active
    {
        let cursors = app_b.remote_cursors.lock().unwrap();
        assert!(cursors.contains_key(&42), "Clock skew failure: Peer A was incorrectly pruned immediately upon arrival!");
        let stored_p = cursors.get(&42).unwrap();
        assert_eq!(
            stored_p.last_seen, local_uptime_b,
            "Receiver must stamp arrival time with its own local clock (100.0) instead of foreign clock (5.0)"
        );
        assert_eq!(stored_p.name, "ForeignClockPeer");
        assert_eq!(stored_p.cursor, Some(Point::new(150.0, 250.0)));
    }

    // Advance Client B's clock by 3 seconds (t = 103.0) without new messages:
    // With 10s timeout, Peer A MUST still be present.
    app_b.handle_collab(&ctx, local_uptime_b + 3.0);
    {
        let cursors = app_b.remote_cursors.lock().unwrap();
        assert!(cursors.contains_key(&42), "Peer A must remain active within the 10-second pruning window");
    }

    // Now test Nickname update propagation while active:
    let updated_presence = PeerPresence {
        client_id: 42,
        name: "RenamedForeignPeer".to_string(),
        color: [255, 0, 128, 255],
        cursor: Some(Point::new(180.0, 280.0)),
        last_seen: foreign_uptime_a + 5.0, // foreign clock now 10.0
        active_tab: None,
        active_interaction: None,
    };
    inbox_b.lock().unwrap().push(CollabIncoming::Presence(updated_presence));
    app_b.handle_collab(&ctx, local_uptime_b + 5.0); // local clock 105.0
    {
        let cursors = app_b.remote_cursors.lock().unwrap();
        let stored_p = cursors.get(&42).unwrap();
        assert_eq!(stored_p.name, "RenamedForeignPeer", "Nickname update must be reflected in peer presence");
        assert_eq!(stored_p.cursor, Some(Point::new(180.0, 280.0)));
        assert_eq!(stored_p.last_seen, 105.0, "Last seen must be updated to 105.0");
    }

    // Now advance Client B's clock past the 10-second timeout without any new updates:
    // At t = 105.0 + 10.1 = 115.1, Peer A MUST be pruned.
    app_b.handle_collab(&ctx, 115.1);
    {
        let cursors = app_b.remote_cursors.lock().unwrap();
        assert!(!cursors.contains_key(&42), "Stale cursor must be pruned after 10 seconds of silence");
    }
}

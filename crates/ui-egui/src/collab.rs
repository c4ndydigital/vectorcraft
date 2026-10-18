//! Real-time multiplayer collaboration and awareness for the egui frontend.
//!
//! Preserves multi-tab document synchronization and colored tab-presence markers while providing:
//! - Tab-isolated dirty-node tracking and O(1) skip for unchanged inactive tabs (via [`CollabBridge`]).
//! - 100% stable Z-order per tab with in-place geometry/style updates.
//! - Ephemeral drag state: while an object is actively being dragged or transformed (`pointermove`
//!   during an interaction), expensive document CRDT syncs are suppressed and committed cleanly
//!   upon `pointerup`, while cursor/tab awareness (`PeerPresence`) continues syncing smoothly.

pub use vectorcraft_engine::collab::*;

use crate::{VectorcraftApp, View};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerInteraction {
    pub bounds: vectorcraft_geom::Rect,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerPresence {
    pub client_id: u64,
    pub name: String,
    pub color: [u8; 4],
    pub cursor: Option<vectorcraft_geom::Point>,
    pub last_seen: f64,
    #[serde(default)]
    pub active_tab: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_interaction: Option<PeerInteraction>,
}

#[derive(Clone, Debug)]
pub enum CollabIncoming {
    Update(Vec<u8>),
    Presence(PeerPresence),
    Blob { key: String, mime: String, bytes: Vec<u8> },
    AuthRequired,
    Connected,
}

#[derive(Clone, Debug)]
pub enum CollabOutgoing {
    Update(Vec<u8>),
    Presence(PeerPresence),
    Blob { key: String, mime: String, bytes: Vec<u8> },
    ReconnectWithPin(String),
}

/// Prioritizes outgoing collaboration packets to eliminate Head-of-Line blocking:
/// 1. High priority: `PeerPresence` (cursor / interaction stream). Multiple queued presence packets
///    for the same client are coalesced down to the latest one, discarding intermediate stale frames.
/// 2. Normal priority: `Update` (CRDT operations). Preserves strict causal order.
/// 3. Bulk/Low priority: `Blob` (multi-megabyte binary assets / PDF images).
pub fn prioritize_and_coalesce_outbox(queue: &mut Vec<CollabOutgoing>) -> Vec<CollabOutgoing> {
    if queue.is_empty() {
        return Vec::new();
    }
    let items = std::mem::take(queue);
    let mut latest_presences: std::collections::HashMap<u64, PeerPresence> = std::collections::HashMap::new();
    let mut presence_order: Vec<u64> = Vec::new();
    let mut updates = Vec::new();
    let mut blobs = Vec::new();
    let mut reconnects = Vec::new();

    for item in items {
        match item {
            CollabOutgoing::Presence(p) => {
                let id = p.client_id;
                if !latest_presences.contains_key(&id) {
                    presence_order.push(id);
                }
                latest_presences.insert(id, p);
            }
            CollabOutgoing::Update(u) => updates.push(CollabOutgoing::Update(u)),
            CollabOutgoing::Blob { key, mime, bytes } => blobs.push(CollabOutgoing::Blob { key, mime, bytes }),
            CollabOutgoing::ReconnectWithPin(pin) => reconnects.push(CollabOutgoing::ReconnectWithPin(pin)),
        }
    }

    let mut prioritized = Vec::with_capacity(reconnects.len() + presence_order.len() + updates.len() + blobs.len());
    prioritized.extend(reconnects);
    for id in presence_order {
        if let Some(p) = latest_presences.remove(&id) {
            prioritized.push(CollabOutgoing::Presence(p));
        }
    }
    prioritized.extend(updates);
    prioritized.extend(blobs);
    prioritized
}

impl VectorcraftApp {
    pub fn collab_peer_name(&self) -> String {
        let trimmed = self.collab_custom_name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        format!("User {}", self.collab_client_id % 1000)
    }

    pub fn copy_share_link(&mut self, ctx: &egui::Context, now: f64) {
        self.collab_share_copied_until = now + 2.5;
        ctx.copy_text(self.collab_share_url.clone());
        ctx.request_repaint();
    }

    pub fn set_collab_password(&mut self, pin: Option<String>) {
        if let Some(pin_val) = pin {
            self.collab_room_protected = true;
            self.collab_auth_required = false;
            // Update collab_share_url with &pin=...
            let base = self.collab_share_url.split('&').filter(|p| !p.starts_with("pin=")).collect::<Vec<_>>().join("&");
            self.collab_share_url = format!("{base}&pin={pin_val}");
            // Send auth update (and reconnect if socket was closed due to missing PIN) through collab outbox
            if let Some(outbox) = &self.services.collab_outbox {
                let mut g = outbox.lock().unwrap_or_else(|e| e.into_inner());
                g.push(CollabOutgoing::ReconnectWithPin(pin_val.clone()));
                g.push(CollabOutgoing::Blob {
                    key: "__room_auth__".to_string(),
                    mime: "application/json".to_string(),
                    bytes: serde_json::to_vec(&serde_json::json!({
                        "type": "set_room_pin",
                        "pin": pin_val,
                    })).unwrap_or_default(),
                });
            }
        } else {
            self.collab_room_protected = false;
            self.collab_auth_required = false;
            // Remove pin=... from share URL
            self.collab_share_url = self.collab_share_url.split('&').filter(|p| !p.starts_with("pin=")).collect::<Vec<_>>().join("&");
            if let Some(outbox) = &self.services.collab_outbox {
                let msg = CollabOutgoing::Blob {
                    key: "__room_auth__".to_string(),
                    mime: "application/json".to_string(),
                    bytes: serde_json::to_vec(&serde_json::json!({
                        "type": "clear_room_pin",
                    })).unwrap_or_default(),
                };
                outbox.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
            }
        }
    }

    pub fn collab_peer_color(&self) -> [u8; 4] {
        const PEER_COLORS: [[u8; 4]; 8] = [
            [242, 78, 30, 255],  // Figma orange/red
            [255, 114, 98, 255], // Coral
            [162, 89, 255, 255], // Purple
            [24, 160, 251, 255], // Blue
            [10, 207, 131, 255], // Green
            [255, 192, 67, 255], // Yellow/Amber
            [235, 47, 150, 255], // Magenta
            [19, 194, 194, 255], // Cyan
        ];
        PEER_COLORS[(self.collab_client_id as usize) % PEER_COLORS.len()]
    }

    /// Returns `true` while the user is actively dragging/transforming an object on the canvas
    /// or scrubbing a numeric property (`pointermove` while held).
    ///
    /// During an active drag, `sync_from_session` is suppressed on intermediate pointer frames
    /// and committed once upon release (`pointerup`), while cursor/tab presence continues
    /// streaming at 60 FPS.
    pub fn is_active_drag(&self, ctx: &egui::Context) -> bool {
        self.session.in_interaction()
            || self.scrub_group
            || self.session.documents().iter().any(|d| d.interaction.is_some() || d.undo_group.is_some())
            || crate::canvas::is_tool_drag_active(ctx)
    }

    pub fn handle_collab(&mut self, ctx: &egui::Context, now: f64) {
        if self.collab.is_none() && (self.services.collab_inbox.is_some() || self.services.collab_outbox.is_some()) {
            self.collab = Some(CollabBridge::with_client_id(self.collab_client_id));
        }

        let active_drag = self.is_active_drag(ctx);

        // Drain incoming messages
        let incoming = self
            .services
            .collab_inbox
            .as_ref()
            .map(|q| {
                let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *g)
            })
            .unwrap_or_default();

        let mut deferred_incoming = Vec::new();
        for msg in incoming {
            match msg {
                CollabIncoming::Update(bytes) => {
                    if active_drag {
                        // Hold document updates while local interaction preview is active so
                        // `Session::preview` snapshot rollback does not overwrite remote changes;
                        // apply immediately on `pointerup`.
                        deferred_incoming.push(CollabIncoming::Update(bytes));
                    } else if let Some(collab) = &mut self.collab {
                        if let Err(e) = collab.apply_remote_update(&mut self.session, &bytes) {
                            log::warn!("collab: apply remote update error: {e}");
                        } else {
                            let current_uids: std::collections::HashSet<u64> = self.session.documents().iter().map(|d| d.uid).collect();
                            self.collab_doc_revisions.retain(|uid, _| current_uids.contains(uid));
                            for doc_st in self.session.documents() {
                                self.collab_doc_revisions.insert(doc_st.uid, doc_st.revision);
                            }
                            self.collab_last_doc_count = self.session.documents().len();
                            if let Some(st) = self.session.active() {
                                self.collab_last_synced_rev = st.revision;
                            }
                            if self.views.len() != self.session.documents().len() {
                                self.views = self.session.documents().iter().map(View::of).collect();
                            }
                            ctx.request_repaint();
                        }
                    }
                }
                CollabIncoming::Presence(mut p) => {
                    if p.client_id != self.collab_client_id {
                        // Stamp arrival time using receiver's local monotonic clock to eliminate cross-host clock skew.
                        p.last_seen = now;
                        let mut cursors = self.remote_cursors.lock().unwrap_or_else(|e| e.into_inner());
                        cursors.insert(p.client_id, p);
                        ctx.request_repaint();
                    }
                }
                CollabIncoming::AuthRequired => {
                    self.collab_room_protected = true;
                    self.collab_auth_required = true;
                    self.ui.room_password = true;
                    ctx.request_repaint();
                }
                CollabIncoming::Connected => {
                    self.collab_auth_required = false;
                    // Push current local document state so any edits made before connecting/reconnecting are synced
                    self.collab_drag_dirty = true;
                    ctx.request_repaint();
                }
                CollabIncoming::Blob { key, mime, bytes } => {
                    if key == "__room_auth__" {
                        if let Ok(auth_msg) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            match auth_msg.get("type").and_then(|t| t.as_str()) {
                                Some("room_auth_status") => {
                                    if let Some(prot) = auth_msg.get("protected").and_then(|p| p.as_bool()) {
                                        self.collab_room_protected = prot;
                                        ctx.request_repaint();
                                    }
                                }
                                Some("set_room_pin") => {
                                    self.collab_room_protected = true;
                                    ctx.request_repaint();
                                }
                                Some("clear_room_pin") => {
                                    self.collab_room_protected = false;
                                    ctx.request_repaint();
                                }
                                _ => {}
                            }
                        }
                    } else if active_drag {
                        deferred_incoming.push(CollabIncoming::Blob { key, mime, bytes });
                    } else {
                        // Store the blob into all local documents that reference it or the active document
                        self.collab_synced_image_keys.insert(key.clone());
                        let arc_bytes = std::sync::Arc::new(bytes);
                        let mut inserted_any = false;
                        for st in self.session.documents_mut() {
                            let doc = std::sync::Arc::make_mut(&mut st.doc);
                            let needs_image = !doc.images.contains_key(&key);
                            if needs_image {
                                doc.images.insert(key.clone(), vectorcraft_doc::ImageBlob { mime: mime.clone(), bytes: arc_bytes.clone(), proxy: None });
                                st.revision += 1;
                                inserted_any = true;
                            }
                        }
                        if inserted_any {
                            self.canvas.key = None;
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }

        if !deferred_incoming.is_empty()
            && let Some(inbox) = &self.services.collab_inbox
        {
            let mut g = inbox.lock().unwrap_or_else(|e| e.into_inner());
            deferred_incoming.append(&mut *g);
            *g = deferred_incoming;
        }

        // Prune stale cursors (older than 10 seconds)
        let has_remote_cursors = {
            let mut cursors = self.remote_cursors.lock().unwrap_or_else(|e| e.into_inner());
            cursors.retain(|_, p| now - p.last_seen < 10.0);
            !cursors.is_empty()
        };
        if has_remote_cursors {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }

        // Check outgoing local document changes across all open documents and tabs
        let current_doc_count = self.session.documents().len();
        let mut any_doc_changed = current_doc_count != self.collab_last_doc_count || self.collab_drag_dirty;
        if !any_doc_changed {
            for doc_st in self.session.documents() {
                let last_rev = self.collab_doc_revisions.get(&doc_st.uid).copied().unwrap_or(0);
                if doc_st.revision > last_rev {
                    any_doc_changed = true;
                    break;
                }
            }
        }

        // Ephemeral Drag State:
        // While the mouse button is pressed and an object is being dragged/transformed,
        // do NOT run `sync_from_session` on every `pointermove` frame. Commit the changed
        // dirty_nodes immediately upon `pointerup` (`!active_drag`).
        if any_doc_changed {
            if active_drag {
                self.collab_drag_dirty = true;
            } else {
                self.collab_drag_dirty = false;
                if let Some(collab) = &mut self.collab {
                    match collab.sync_from_session(&self.session) {
                        Ok(bytes) => {
                            let current_uids: std::collections::HashSet<u64> = self.session.documents().iter().map(|d| d.uid).collect();
                            self.collab_doc_revisions.retain(|uid, _| current_uids.contains(uid));
                            for doc_st in self.session.documents() {
                                self.collab_doc_revisions.insert(doc_st.uid, doc_st.revision);
                            }
                            self.collab_last_doc_count = current_doc_count;
                            if self.views.len() != current_doc_count {
                                self.views = self.session.documents().iter().map(View::of).collect();
                            }
                            if let Some(st) = self.session.active() {
                                self.collab_last_synced_rev = st.revision;
                            }
                            if let Some(outbox) = &self.services.collab_outbox {
                                let mut g = outbox.lock().unwrap_or_else(|e| e.into_inner());
                                if !bytes.is_empty() {
                                    g.push(CollabOutgoing::Update(bytes));
                                }
                                for doc_st in self.session.documents() {
                                    for (key, blob) in &doc_st.doc.images {
                                        if !blob.bytes.is_empty() && !self.collab_synced_image_keys.contains(key) {
                                            self.collab_synced_image_keys.insert(key.clone());
                                            g.push(CollabOutgoing::Blob {
                                                key: key.clone(),
                                                mime: blob.mime.clone(),
                                                bytes: (*blob.bytes).clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("collab: sync from session error: {e}");
                        }
                    }
                }
            }
        }

        // Outgoing cursor, interaction, and tab-awareness presence updates (60 FPS during active drag, 30 FPS idle)
        let current_interaction = if active_drag {
            let bounds = self.selection_bounds();
            let label = self.session.active().and_then(|d| d.interaction.as_ref()).map(|it| it.label.clone());
            bounds.map(|b| PeerInteraction { bounds: b, label })
        } else {
            None
        };
        let interaction_changed = current_interaction != self.last_broadcast_interaction;

        let current_name = self.collab_peer_name();
        let name_changed = current_name != self.last_broadcast_name;
        let cursor_changed = self.hover_doc != self.last_broadcast_cursor;
        let current_tab = self.session.active_index();
        let tab_changed = current_tab != self.last_broadcast_tab;
        let dt = now - self.last_cursor_broadcast_at;
        let min_cursor_interval = if active_drag { 0.016 } else { 0.033 };
        let should_broadcast = name_changed
            || tab_changed
            || interaction_changed
            || (cursor_changed && dt >= min_cursor_interval)
            || (self.hover_doc.is_some() && dt >= 1.0);

        if should_broadcast {
            if name_changed || tab_changed || interaction_changed || cursor_changed || self.hover_doc.is_some() {
                if let Some(outbox) = &self.services.collab_outbox {
                    let presence = PeerPresence {
                        client_id: self.collab_client_id,
                        name: current_name.clone(),
                        color: self.collab_peer_color(),
                        cursor: self
                            .hover_doc
                            .map(|pt| vectorcraft_geom::Point::new((pt.x * 10.0).round() / 10.0, (pt.y * 10.0).round() / 10.0)),
                        last_seen: now,
                        active_tab: current_tab,
                        active_interaction: current_interaction.clone(),
                    };
                    let mut g = outbox.lock().unwrap_or_else(|e| e.into_inner());
                    g.push(CollabOutgoing::Presence(presence));
                    self.last_broadcast_cursor = self.hover_doc;
                    self.last_broadcast_name = current_name;
                    self.last_broadcast_tab = current_tab;
                    self.last_broadcast_interaction = current_interaction;
                    self.last_cursor_broadcast_at = now;
                }
            }
        } else if cursor_changed {
            let wait_ms = if active_drag { 16 } else { 33 };
            ctx.request_repaint_after(std::time::Duration::from_millis(wait_ms));
        }
    }
}

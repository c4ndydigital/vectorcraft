//! CRDT state bridge for real-time collaboration.
//!
//! Synchronizes VectorCraft's immutable [`Arc<Document>`] state with a [`yrs::Doc`]
//! CRDT document, enabling peer-to-peer or server-mediated multi-user editing.

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vectorcraft_doc::{DocError, Node, NodeId, NodeKind};
use yrs::updates::decoder::Decode;
use yrs::{Doc, Map, MapRef, ReadTxn, StateVector, Transact, Update};

use crate::Session;

/// Errors that can occur during collaboration synchronization.
#[derive(Debug, thiserror::Error)]
pub enum CollabError {
    #[error("no active document in session")]
    NoDocument,
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("deserialization error: {0}")]
    Deserialization(String),
    #[error("crdt error: {0}")]
    Crdt(String),
    #[error("doc error: {0}")]
    Doc(#[from] DocError),
}

/// Serialized tab representation stored in the CRDT `"tabs"` map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrdtTab {
    pub id: String,
    pub title: String,
    pub artboards: Vec<vectorcraft_doc::Artboard>,
    #[serde(default)]
    pub order: usize,
}

/// Serialized shape representation stored in the CRDT `"shapes"` map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrdtShape {
    pub node: Node,
    pub parent_id: Option<u64>,
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub tab_id: String,
}

/// CRDT bridge synchronizing a [`Session`]'s documents and tabs with a [`yrs::Doc`].
#[derive(Debug)]
pub struct CollabBridge {
    doc: Doc,
    shapes: MapRef,
    meta: MapRef,
    tabs: MapRef,
    local_uid_to_tab_id: std::collections::HashMap<u64, String>,
    known_shapes: HashSet<u64>,
    known_shape_keys: HashSet<String>,
}

impl Default for CollabBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl CollabBridge {
    /// Create a new collaboration bridge with a randomized client identifier.
    pub fn new() -> Self {
        let doc = Doc::new();
        let shapes = doc.get_or_insert_map("shapes");
        let meta = doc.get_or_insert_map("meta");
        let tabs = doc.get_or_insert_map("tabs");
        Self {
            doc,
            shapes,
            meta,
            tabs,
            local_uid_to_tab_id: std::collections::HashMap::new(),
            known_shapes: HashSet::new(),
            known_shape_keys: HashSet::new(),
        }
    }

    /// Create a new collaboration bridge with an explicit client identifier.
    pub fn with_client_id(client_id: u64) -> Self {
        let doc = Doc::with_client_id(client_id);
        let shapes = doc.get_or_insert_map("shapes");
        let meta = doc.get_or_insert_map("meta");
        let tabs = doc.get_or_insert_map("tabs");
        Self {
            doc,
            shapes,
            meta,
            tabs,
            local_uid_to_tab_id: std::collections::HashMap::new(),
            known_shapes: HashSet::new(),
            known_shape_keys: HashSet::new(),
        }
    }

    /// Reference to the underlying CRDT document.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Number of shapes currently tracked in the CRDT map across all tabs.
    pub fn shapes_count(&self) -> usize {
        let txn = self.doc.transact();
        self.shapes.len(&txn) as usize
    }

    /// Number of tabs currently tracked in the CRDT map.
    pub fn tabs_count(&self) -> usize {
        let txn = self.doc.transact();
        self.tabs.len(&txn) as usize
    }

    /// Tab ID corresponding to a local document UID, if mapped.
    pub fn tab_id_for_uid(&self, uid: u64) -> Option<String> {
        self.local_uid_to_tab_id.get(&uid).cloned()
    }

    /// Set of shape IDs currently tracked by this bridge.
    pub fn known_shapes(&self) -> &HashSet<u64> {
        &self.known_shapes
    }

    /// Export the entire document state as a binary update v1.
    pub fn export_update(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    /// Synchronize all documents and tabs in `session` into the CRDT document.
    ///
    /// Synchronizes tabs metadata into the `"tabs"` map, and partitions shapes
    /// by `{tab_id}:{node_id}` to prevent cross-tab ID collision.
    pub fn sync_from_session(&mut self, session: &Session) -> Result<Vec<u8>, CollabError> {
        let docs = session.documents();
        if docs.is_empty() {
            return Err(CollabError::NoDocument);
        }

        let mut txn = self.doc.transact_mut();

        // Collect existing remote tabs from CRDT map
        let mut crdt_tabs_map: std::collections::HashMap<String, CrdtTab> = std::collections::HashMap::new();
        for key in self.tabs.keys(&txn) {
            if let Some(out) = self.tabs.get(&txn, key) {
                if let Ok(json_str) = String::try_from(out) {
                    if let Ok(tab) = serde_json::from_str::<CrdtTab>(&json_str) {
                        crdt_tabs_map.insert(key.to_string(), tab);
                    }
                }
            }
        }

        let mut current_local_tab_ids = HashSet::new();
        let mut active_shape_keys = HashSet::new();
        let mut shapes_to_sync = Vec::new();

        // 1. Sync tabs and collect shapes from each document
        for (idx, st) in docs.iter().enumerate() {
            let tab_id = if let Some(existing_id) = self.local_uid_to_tab_id.get(&st.uid) {
                existing_id.clone()
            } else {
                let assigned_id = if idx == 0 {
                    if let Some(first_crdt_tab) = crdt_tabs_map.values().min_by_key(|t| t.order) {
                        first_crdt_tab.id.clone()
                    } else {
                        "tab-0".to_string()
                    }
                } else {
                    format!("tab-{}-{}", self.doc.client_id().get(), st.uid)
                };
                self.local_uid_to_tab_id.insert(st.uid, assigned_id.clone());
                assigned_id
            };

            current_local_tab_ids.insert(tab_id.clone());

            let title = st
                .doc
                .artboards
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| format!("Tab {}", idx + 1));

            let crdt_tab = CrdtTab {
                id: tab_id.clone(),
                title,
                artboards: st.doc.artboards.clone(),
                order: idx,
            };

            if let Ok(tab_json) = serde_json::to_string(&crdt_tab) {
                self.tabs.try_update(&mut txn, tab_id.as_str(), tab_json);
            }

            // Sync active document artboards to legacy meta map for backward compatibility (only in single-document mode)
            if session.documents().len() <= 1 && session.active_index() == Some(idx) {
                if let Ok(artboards_json) = serde_json::to_string(&st.doc.artboards) {
                    self.meta.try_update(&mut txn, "artboards", artboards_json);
                }
            }

            // Sync shapes in this document
            st.doc.walk(|node| {
                if !node.is_layer() {
                    let shape_key = format!("{}:{}", tab_id, node.id.0);
                    active_shape_keys.insert(shape_key.clone());
                    let (parent_id, index) = match st.doc.position(node.id) {
                        Some((p, i, _)) => (p.map(|pid| pid.0), i),
                        None => (st.doc.parent_of(node.id).map(|p| p.0), usize::MAX),
                    };
                    shapes_to_sync.push((
                        shape_key,
                        CrdtShape {
                            node: node.clone(),
                            parent_id,
                            index,
                            tab_id: tab_id.clone(),
                        },
                    ));
                }
            });
        }

        // 2. Handle shape deletions:
        // Only remove keys belonging to tabs currently present in `current_local_tab_ids`
        let existing_keys: Vec<String> = self.shapes.keys(&txn).map(|k| k.to_string()).collect();
        for key in existing_keys {
            let key_tab_id = if let Some(colon_pos) = key.find(':') {
                &key[..colon_pos]
            } else {
                "tab-0"
            };

            if current_local_tab_ids.contains(key_tab_id) && !active_shape_keys.contains(&key) {
                self.shapes.remove(&mut txn, &key);
            }
        }

        // 3. Insert or update shapes in the CRDT map
        for (key, shape) in shapes_to_sync {
            let serialized = serde_json::to_string(&shape).map_err(|e| CollabError::Serialization(e.to_string()))?;
            self.shapes.try_update(&mut txn, key, serialized);
        }

        // 4. Update local known tracking
        self.known_shape_keys.clear();
        self.known_shapes.clear();
        for key in &active_shape_keys {
            self.known_shape_keys.insert(key.clone());
            let node_id_str = if let Some(colon_pos) = key.find(':') {
                &key[colon_pos + 1..]
            } else {
                key.as_str()
            };
            if let Ok(id) = node_id_str.parse::<u64>() {
                self.known_shapes.insert(id);
            }
        }

        let update = txn.encode_update_v1();
        Ok(update)
    }

    /// Apply a remote binary update to the CRDT document and ingest changes into `session`.
    ///
    /// Reconciles tabs with local session documents (adding missing tabs without stealing
    /// focus), maps shapes to their corresponding tab document, preserves hierarchy and Z-order,
    /// and increments document revisions.
    pub fn apply_remote_update(&mut self, session: &mut Session, update: &[u8]) -> Result<(), CollabError> {
        if update.is_empty() {
            return Ok(());
        }

        let decoded = Update::decode_v1(update).map_err(|e| CollabError::Crdt(e.to_string()))?;

        let mut txn = self.doc.transact_mut();
        txn.apply_update(decoded).map_err(|e| CollabError::Crdt(e.to_string()))?;

        // 1. Read remote tabs from CRDT tabs map
        let mut remote_tabs = Vec::new();
        for key in self.tabs.keys(&txn) {
            if let Some(out) = self.tabs.get(&txn, key) {
                if let Ok(json_str) = String::try_from(out) {
                    if let Ok(tab) = serde_json::from_str::<CrdtTab>(&json_str) {
                        remote_tabs.push(tab);
                    }
                }
            }
        }
        remote_tabs.sort_by_key(|t| t.order);

        // Read all shapes currently present in CRDT shapes map
        let mut crdt_shapes: Vec<CrdtShape> = Vec::new();
        let mut crdt_shape_keys = HashSet::new();

        for key in self.shapes.keys(&txn) {
            if let Some(out) = self.shapes.get(&txn, key) {
                let json_str =
                    String::try_from(out).map_err(|_| CollabError::Deserialization(format!("failed to convert value for key {key} to string")))?;
                let mut shape: CrdtShape = serde_json::from_str(&json_str).map_err(|e| CollabError::Deserialization(e.to_string()))?;
                if shape.tab_id.is_empty() {
                    if let Some(colon_pos) = key.find(':') {
                        shape.tab_id = key[..colon_pos].to_string();
                    } else if let Some(first_tab) = remote_tabs.first() {
                        shape.tab_id = first_tab.id.clone();
                    } else {
                        shape.tab_id = "tab-0".to_string();
                    }
                }
                crdt_shape_keys.insert(key.to_string());
                crdt_shapes.push(shape);
            }
        }

        // If session has no documents and there are shapes or tabs, create the initial document
        if session.documents().is_empty() && (!remote_tabs.is_empty() || !crdt_shapes.is_empty()) {
            let _ = session.execute("file.new", &serde_json::json!({"width": 800.0, "height": 600.0}));
        }

        if session.documents().is_empty() {
            return Ok(());
        }

        // 2. Reconcile tabs with session documents
        let initial_active = session.active_index();

        if !remote_tabs.is_empty() {
            for remote_tab in &remote_tabs {
                let already_mapped = self.local_uid_to_tab_id.iter().any(|(_, tid)| tid == &remote_tab.id);
                if !already_mapped {
                    // Check if an existing unmapped document can be bound
                    let unmapped_uid = session.documents().iter().find(|d| !self.local_uid_to_tab_id.contains_key(&d.uid)).map(|d| d.uid);
                    if let Some(uid) = unmapped_uid {
                        self.local_uid_to_tab_id.insert(uid, remote_tab.id.clone());
                    } else {
                        // Create a new document in the session for this remote tab
                        let (w, h) = remote_tab
                            .artboards
                            .first()
                            .map(|a| (a.rect.width(), a.rect.height()))
                            .unwrap_or((800.0, 600.0));
                        let mut new_doc = vectorcraft_doc::Document::new(w, h);
                        if !remote_tab.artboards.is_empty() {
                            new_doc.artboards = remote_tab.artboards.clone();
                        }
                        if let Some(first_ab) = new_doc.artboards.first_mut() {
                            first_ab.name = remote_tab.title.clone();
                        }
                        let new_idx = session.add_document(new_doc, None);
                        let new_uid = session.documents()[new_idx].uid;
                        self.local_uid_to_tab_id.insert(new_uid, remote_tab.id.clone());
                    }
                }
            }

            // Restore previous active tab to prevent remote tabs from stealing user focus
            if let Some(prev) = initial_active {
                if prev < session.documents().len() {
                    session.set_active(prev);
                }
            }
        } else {
            // Legacy single-doc fallback: ensure first doc is mapped to "tab-0"
            if let Some(first) = session.documents().first() {
                self.local_uid_to_tab_id.entry(first.uid).or_insert_with(|| "tab-0".to_string());
            }
        }

        // 3. Ingest shapes into each document corresponding to its tab_id
        let mut shapes_by_tab: std::collections::HashMap<String, Vec<CrdtShape>> = std::collections::HashMap::new();
        for shape in crdt_shapes {
            shapes_by_tab.entry(shape.tab_id.clone()).or_default().push(shape);
        }

        for st in session.documents_mut() {
            let tab_id = match self.local_uid_to_tab_id.get(&st.uid) {
                Some(id) => id.clone(),
                None => continue,
            };

            let tab_shapes = shapes_by_tab.remove(&tab_id).unwrap_or_default();
            let crdt_tab_ids: HashSet<NodeId> = tab_shapes.iter().map(|s| s.node.id).collect();
            let doc = Arc::make_mut(&mut st.doc);

            // Remove shapes deleted in this tab
            for known_key in &self.known_shape_keys {
                let (key_tab, key_node_id) = if let Some(colon_pos) = known_key.find(':') {
                    (&known_key[..colon_pos], &known_key[colon_pos + 1..])
                } else {
                    ("tab-0", known_key.as_str())
                };

                if key_tab == tab_id {
                    if let Ok(id_u64) = key_node_id.parse::<u64>() {
                        let node_id = NodeId(id_u64);
                        if !crdt_tab_ids.contains(&node_id) && doc.node(node_id).is_some() {
                            let _ = doc.remove(node_id);
                        }
                    }
                }
            }

            // Sort shapes by index
            let mut sorted_shapes = tab_shapes;
            sorted_shapes.sort_by_key(|s| s.index);

            // First pass: ensure all nodes exist
            for shape in &sorted_shapes {
                let node_id = shape.node.id;
                if doc.node(node_id).is_none() {
                    let parent_layer = shape
                        .parent_id
                        .map(NodeId)
                        .filter(|&pid| doc.node(pid).is_some_and(|n| n.is_layer() || matches!(n.kind, NodeKind::Group { .. })))
                        .or_else(|| doc.default_layer())
                        .or_else(|| Some(doc.add_layer(None)));

                    let _ = doc.insert(parent_layer, shape.index, shape.node.clone());
                }
            }

            // Second pass: update properties and hierarchy/Z-index
            for shape in sorted_shapes {
                let node_id = shape.node.id;
                let target_parent = shape
                    .parent_id
                    .map(NodeId)
                    .filter(|&pid| doc.node(pid).is_some_and(|n| n.is_layer() || matches!(n.kind, NodeKind::Group { .. })))
                    .or_else(|| doc.default_layer());

                if let Some(existing) = doc.node_mut(node_id) {
                    *existing = shape.node;
                }
                let (current_parent, current_idx) = match doc.position(node_id) {
                    Some((p, idx, _)) => (p, idx),
                    None => (doc.parent_of(node_id), usize::MAX),
                };
                if current_parent != target_parent || current_idx != shape.index {
                    let _ = doc.move_node(node_id, target_parent, shape.index);
                }
            }

            // Artboards update from tab if present
            if let Some(rt) = remote_tabs.iter().find(|t| t.id == tab_id) {
                if !rt.artboards.is_empty() && doc.artboards != rt.artboards {
                    doc.artboards = rt.artboards.clone();
                }
            }

            doc.fix_next_id();
            st.selection.objects.retain(|id| doc.node(*id).is_some());
            if let Some(k) = st.selection.key {
                if doc.node(k).is_none() {
                    st.selection.key = None;
                }
            }
            st.selection.anchors.retain(|id, _| doc.node(*id).is_some());
            st.revision += 1;
        }

        // Synchronize legacy document artboards from meta (only for legacy single-document clients)
        if remote_tabs.is_empty() {
            if let Some(out) = self.meta.get(&txn, "artboards") {
                if let Ok(json_str) = String::try_from(out) {
                    if let Ok(artboards) = serde_json::from_str::<Vec<vectorcraft_doc::Artboard>>(&json_str) {
                        if let Some(active_st) = session.active_mut() {
                            let active_doc = Arc::make_mut(&mut active_st.doc);
                            if !artboards.is_empty() && active_doc.artboards != artboards {
                                active_doc.artboards = artboards;
                                active_st.revision += 1;
                            }
                        }
                    }
                }
            }
        }

        // Update known shapes tracking
        self.known_shape_keys = crdt_shape_keys;
        self.known_shapes.clear();
        for key in &self.known_shape_keys {
            let node_id_str = if let Some(colon_pos) = key.find(':') {
                &key[colon_pos + 1..]
            } else {
                key.as_str()
            };
            if let Ok(id) = node_id_str.parse::<u64>() {
                self.known_shapes.insert(id);
            }
        }

        Ok(())
    }
}

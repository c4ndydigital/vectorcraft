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

/// Serialized shape representation stored in the CRDT `"shapes"` map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrdtShape {
    pub node: Node,
    pub parent_id: Option<u64>,
}

/// CRDT bridge synchronizing a [`Session`]'s active [`Document`] with a [`yrs::Doc`].
#[derive(Debug)]
pub struct CollabBridge {
    doc: Doc,
    shapes: MapRef,
    known_shapes: HashSet<u64>,
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
        Self { doc, shapes, known_shapes: HashSet::new() }
    }

    /// Create a new collaboration bridge with an explicit client identifier.
    pub fn with_client_id(client_id: u64) -> Self {
        let doc = Doc::with_client_id(client_id);
        let shapes = doc.get_or_insert_map("shapes");
        Self { doc, shapes, known_shapes: HashSet::new() }
    }

    /// Reference to the underlying CRDT document.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Number of shapes currently tracked in the CRDT map.
    pub fn shapes_count(&self) -> usize {
        let txn = self.doc.transact();
        self.shapes.len(&txn) as usize
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

    /// Synchronize the active document state from `session` into the CRDT document.
    ///
    /// Scans shapes in the active document (`doc.layers`), serializes rectangles/paths
    /// into the CRDT shapes map, handles shape deletions, and returns the encoded
    /// update bytes for broadcast to peers.
    pub fn sync_from_session(&mut self, session: &Session) -> Result<Vec<u8>, CollabError> {
        let st = session.active().ok_or(CollabError::NoDocument)?;
        let mut txn = self.doc.transact_mut();

        let mut active_shape_ids = HashSet::new();
        let mut shapes_to_sync = Vec::new();

        st.doc.walk(|node| {
            if matches!(node.kind, NodeKind::Path { .. } | NodeKind::Compound { .. }) {
                let key = node.id.0.to_string();
                active_shape_ids.insert(key.clone());
                let parent_id = st.doc.parent_of(node.id).map(|p| p.0);
                shapes_to_sync.push((key, CrdtShape { node: node.clone(), parent_id }));
            }
        });

        // Handle shape deletions: remove keys from CRDT map that no longer exist in the document
        let existing_keys: Vec<String> = self.shapes.keys(&txn).map(|k| k.to_string()).collect();
        for key in existing_keys {
            if !active_shape_ids.contains(&key) {
                self.shapes.remove(&mut txn, &key);
            }
        }

        // Insert or update shapes in the CRDT map
        for (key, shape) in shapes_to_sync {
            let serialized = serde_json::to_string(&shape).map_err(|e| CollabError::Serialization(e.to_string()))?;
            self.shapes.try_update(&mut txn, key, serialized);
        }

        // Update local known shapes tracking
        self.known_shapes.clear();
        for key in &active_shape_ids {
            if let Ok(id) = key.parse::<u64>() {
                self.known_shapes.insert(id);
            }
        }

        let update = txn.encode_update_v1();
        Ok(update)
    }

    /// Apply a remote binary update to the CRDT document and ingest changes into `session`.
    ///
    /// Ingests the update into `yrs::Doc`, synchronizes shapes into `st.doc: Arc<Document>`
    /// using `Arc::make_mut(&mut st.doc)`, inserts new nodes or updates existing nodes in-place,
    /// calls `doc.fix_next_id()`, and increments `st.revision += 1`.
    pub fn apply_remote_update(&mut self, session: &mut Session, update: &[u8]) -> Result<(), CollabError> {
        if update.is_empty() {
            return Ok(());
        }

        let decoded = Update::decode_v1(update).map_err(|e| CollabError::Crdt(e.to_string()))?;

        let mut txn = self.doc.transact_mut();
        txn.apply_update(decoded).map_err(|e| CollabError::Crdt(e.to_string()))?;

        // Read all shapes currently present in the CRDT shapes map
        let mut crdt_shapes = Vec::new();
        let mut crdt_ids = HashSet::new();

        for key in self.shapes.keys(&txn) {
            if let Some(out) = self.shapes.get(&txn, key) {
                let json_str =
                    String::try_from(out).map_err(|_| CollabError::Deserialization(format!("failed to convert value for key {key} to string")))?;
                let shape: CrdtShape = serde_json::from_str(&json_str).map_err(|e| CollabError::Deserialization(e.to_string()))?;
                crdt_ids.insert(shape.node.id);
                crdt_shapes.push(shape);
            }
        }

        let st = match session.active_mut() {
            Some(st) => st,
            None => {
                if !crdt_shapes.is_empty() {
                    let _ = session.execute("file.new", &serde_json::json!({"width": 800.0, "height": 600.0}));
                    session.active_mut().ok_or(CollabError::NoDocument)?
                } else {
                    return Ok(());
                }
            }
        };
        let doc = Arc::make_mut(&mut st.doc);

        // Remove shapes previously known that were deleted in the CRDT map
        for &known_id in &self.known_shapes {
            let node_id = NodeId(known_id);
            if !crdt_ids.contains(&node_id) && doc.node(node_id).is_some() {
                let _ = doc.remove(node_id);
            }
        }

        // Insert new nodes or update existing nodes in-place
        for shape in crdt_shapes {
            let node_id = shape.node.id;
            if let Some(existing) = doc.node_mut(node_id) {
                *existing = shape.node;
                let current_parent = doc.parent_of(node_id).map(|p| p.0);
                if current_parent != shape.parent_id {
                    let target_parent = shape
                        .parent_id
                        .map(NodeId)
                        .filter(|&pid| doc.node(pid).is_some_and(|n| n.is_layer() || matches!(n.kind, NodeKind::Group { .. })))
                        .or_else(|| doc.default_layer());
                    let _ = doc.move_node(node_id, target_parent, usize::MAX);
                }
            } else {
                let parent = shape
                    .parent_id
                    .map(NodeId)
                    .filter(|&pid| doc.node(pid).is_some_and(|n| n.is_layer() || matches!(n.kind, NodeKind::Group { .. })))
                    .or_else(|| doc.default_layer())
                    .or_else(|| doc.layers.first().map(|l| l.id));

                let parent_layer = match parent {
                    Some(p) => Some(p),
                    None => Some(doc.add_layer(None)),
                };

                doc.insert(parent_layer, usize::MAX, shape.node)?;
            }
        }

        // Update known shapes tracking
        self.known_shapes.clear();
        for id in &crdt_ids {
            self.known_shapes.insert(id.0);
        }

        // Maintain document integrity
        doc.fix_next_id();

        // Prune any selection references to deleted nodes
        st.selection.objects.retain(|id| doc.node(*id).is_some());
        if let Some(k) = st.selection.key {
            if doc.node(k).is_none() {
                st.selection.key = None;
            }
        }
        st.selection.anchors.retain(|id, _| doc.node(*id).is_some());

        // Increment revision so UI rerenders and caches invalidate
        st.revision += 1;

        Ok(())
    }
}

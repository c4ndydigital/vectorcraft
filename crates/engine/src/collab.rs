//! CRDT state bridge for real-time collaboration.
//!
//! Synchronizes VectorCraft's immutable [`Arc<Document>`] state with a [`yrs::Doc`]
//! CRDT document, enabling peer-to-peer or server-mediated multi-user editing.
//!
//! Includes tab-isolated dirty-node tracking (`dirty_nodes` / `removed_nodes`),
//! O(1) skipping for unchanged/inactive tabs, strict separation of geometry/style
//! edits from structural Z-order changes (`structure_dirty`), and in-place node updates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vectorcraft_doc::{DocError, Document, Node, NodeId, NodeKind};
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
    /// Explicit Z-order skeleton per container in this tab:
    /// maps container `NodeId.0` (`0` for the top-level layer list) to the ordered list of child `NodeId.0`s.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub z_order: BTreeMap<u64, Vec<u64>>,
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

/// Per-tab synchronization state for O(1) inactive-tab skipping, incremental
/// dirty-node tracking, and stable Z-order layer/group skeletons.
#[derive(Clone, Debug, Default)]
pub struct TabSyncState {
    /// Last synced `DocState::revision` for this tab.
    pub last_revision: Option<u64>,
    /// Last synced tab order index.
    pub last_order: Option<usize>,
    /// Last synced tab title.
    pub last_title: String,
    /// Last synced artboards for this tab.
    pub last_artboards: Vec<vectorcraft_doc::Artboard>,
    /// Snapshot of top-level layer `Arc<Node>` pointers from the last sync for O(1) pointer checks.
    pub synced_layers: Vec<Arc<Node>>,
    /// Synced non-layer nodes in this tab: maps `NodeId` -> `(Arc<Node>, parent_id, sibling_index)`.
    pub synced_nodes: HashMap<NodeId, (Arc<Node>, Option<u64>, usize)>,
    /// Explicit Z-order skeleton per container (`0` for root layers, or container `NodeId.0`) -> ordered child `NodeId.0`s.
    pub layer_skeleton: BTreeMap<u64, Vec<u64>>,
    /// Nodes added or modified in the most recent diff for this tab.
    pub dirty_nodes: HashSet<NodeId>,
    /// Nodes removed in the most recent diff for this tab.
    pub removed_nodes: HashSet<NodeId>,
    /// Whether the layer/group hierarchy or Z-order changed in the most recent diff.
    pub structure_dirty: bool,
}

impl TabSyncState {
    /// Fast O(1) check whether this tab's revision, order, and top-level layer pointers are unchanged.
    pub fn is_unchanged(&self, doc: &Document, revision: u64, order: usize) -> bool {
        self.last_revision == Some(revision)
            && self.last_order == Some(order)
            && self.synced_layers.len() == doc.layers.len()
            && self.synced_layers.iter().zip(doc.layers.iter()).all(|(a, b)| Arc::ptr_eq(a, b))
    }

    /// Compute the minimal set of `dirty_nodes`, `removed_nodes`, and `structure_dirty` for `doc`
    /// by comparing `Arc<Node>` pointers against the previous synced snapshot.
    pub fn collect_changes(&mut self, doc: &Document) {
        self.dirty_nodes.clear();
        self.removed_nodes.clear();
        self.structure_dirty = false;

        let layers_ptr_eq = self.synced_layers.len() == doc.layers.len()
            && self.synced_layers.iter().zip(doc.layers.iter()).all(|(a, b)| Arc::ptr_eq(a, b));
        if layers_ptr_eq {
            return;
        }

        let new_root_ids: Vec<u64> = doc.layers.iter().map(|l| l.id.0).collect();
        if self.layer_skeleton.get(&0) != Some(&new_root_ids) {
            self.structure_dirty = true;
            self.layer_skeleton.insert(0, new_root_ids);
        }

        let prev_layers = std::mem::take(&mut self.synced_layers);
        let old_slice = if prev_layers.is_empty() { None } else { Some(prev_layers.as_slice()) };
        self.diff_container(doc, None, old_slice, &doc.layers);
        self.synced_layers = doc.layers.clone();
    }

    fn diff_container(&mut self, doc: &Document, container_id: Option<u64>, old_children: Option<&[Arc<Node>]>, new_children: &[Arc<Node>]) {
        let same_structure = old_children.is_some_and(|old_ch| {
            old_ch.len() == new_children.len()
                && old_ch
                    .iter()
                    .zip(new_children.iter())
                    .all(|(o, n)| o.id == n.id && o.is_layer() == n.is_layer())
        });

        if same_structure {
            if let Some(old_ch) = old_children {
                for (idx, (old_child, new_child)) in old_ch.iter().zip(new_children.iter()).enumerate() {
                    if Arc::ptr_eq(old_child, new_child) {
                        continue;
                    }
                    if !new_child.is_layer() {
                        let own_changed = !node_header_and_leaf_eq(old_child, new_child);
                        if own_changed {
                            self.dirty_nodes.insert(new_child.id);
                        }
                        self.synced_nodes
                            .insert(new_child.id, (Arc::clone(new_child), container_id, idx));
                    }
                    if let Some(new_grand) = new_child.children() {
                        let old_grand = old_child.children().map(Vec::as_slice);
                        self.diff_container(doc, Some(new_child.id.0), old_grand, new_grand);
                    }
                }
            }
            return;
        }

        // Structural change in this container (addition, deletion, or Z-order change)
        self.structure_dirty = true;
        let container_key = container_id.unwrap_or(0);
        let ordered_ids: Vec<u64> = new_children.iter().map(|c| c.id.0).collect();
        self.layer_skeleton.insert(container_key, ordered_ids);

        // Identify removed subtrees from old_children
        if let Some(old_ch) = old_children {
            let new_id_set: HashSet<NodeId> = new_children.iter().map(|c| c.id).collect();
            for old_child in old_ch {
                if !new_id_set.contains(&old_child.id) {
                    old_child.walk(&mut |n| {
                        if doc.node(n.id).is_none() {
                            if !n.is_layer() {
                                self.removed_nodes.insert(n.id);
                                self.dirty_nodes.remove(&n.id);
                                self.synced_nodes.remove(&n.id);
                            }
                            self.layer_skeleton.remove(&n.id.0);
                        }
                    });
                }
            }
        }

        for (idx, new_child) in new_children.iter().enumerate() {
            if new_child.is_layer() {
                let old_layer = old_children.and_then(|old_ch| old_ch.iter().find(|o| o.id == new_child.id));
                if let Some(old_l) = old_layer {
                    if Arc::ptr_eq(old_l, new_child) {
                        continue;
                    }
                    if let Some(new_grand) = new_child.children() {
                        self.diff_container(doc, Some(new_child.id.0), old_l.children().map(Vec::as_slice), new_grand);
                    }
                } else if let Some(new_grand) = new_child.children() {
                    self.diff_container(doc, Some(new_child.id.0), None, new_grand);
                }
            } else {
                let prev_entry = self.synced_nodes.get(&new_child.id).cloned();
                match prev_entry {
                    Some((old_arc, old_parent, old_idx)) => {
                        let ptr_same = Arc::ptr_eq(&old_arc, new_child);
                        let pos_same = old_parent == container_id && old_idx == idx;
                        if ptr_same && pos_same {
                            continue;
                        }
                        if !pos_same || !node_header_and_leaf_eq(&old_arc, new_child) {
                            self.dirty_nodes.insert(new_child.id);
                        }
                        self.synced_nodes
                            .insert(new_child.id, (Arc::clone(new_child), container_id, idx));
                        if !ptr_same {
                            if let Some(new_grand) = new_child.children() {
                                self.diff_container(doc, Some(new_child.id.0), old_arc.children().map(Vec::as_slice), new_grand);
                            }
                        }
                    }
                    None => {
                        self.dirty_nodes.insert(new_child.id);
                        self.synced_nodes
                            .insert(new_child.id, (Arc::clone(new_child), container_id, idx));
                        if let Some(new_grand) = new_child.children() {
                            self.diff_container(doc, Some(new_child.id.0), None, new_grand);
                        }
                    }
                }
            }
        }
    }

    /// Synchronize the internal snapshot with `doc` after applying a remote update,
    /// preventing redundant re-serialization on the next local frame.
    pub fn mark_synced(&mut self, doc: &Document, revision: u64, order: usize, title: String) {
        self.synced_nodes.clear();
        self.layer_skeleton.clear();
        self.dirty_nodes.clear();
        self.removed_nodes.clear();
        self.structure_dirty = false;

        let root_ids: Vec<u64> = doc.layers.iter().map(|l| l.id.0).collect();
        self.layer_skeleton.insert(0, root_ids);
        for layer in &doc.layers {
            self.populate_snapshot_container(layer);
        }

        self.synced_layers = doc.layers.clone();
        self.last_artboards = doc.artboards.clone();
        self.last_revision = Some(revision);
        self.last_order = Some(order);
        self.last_title = title;
    }

    fn populate_snapshot_container(&mut self, container: &Arc<Node>) {
        if let Some(children) = container.children() {
            let child_ids: Vec<u64> = children.iter().map(|c| c.id.0).collect();
            self.layer_skeleton.insert(container.id.0, child_ids);
            for (idx, child) in children.iter().enumerate() {
                if !child.is_layer() {
                    self.synced_nodes
                        .insert(child.id, (Arc::clone(child), Some(container.id.0), idx));
                }
                if child.children().is_some() {
                    self.populate_snapshot_container(child);
                }
            }
        }
    }
}

/// Compare two nodes excluding container children so a `Group` is only marked dirty
/// when its own properties (name, visibility, lock, appearance, extras, clip) change.
fn node_header_and_leaf_eq(a: &Arc<Node>, b: &Arc<Node>) -> bool {
    if Arc::ptr_eq(a, b) {
        return true;
    }
    match (&a.kind, &b.kind) {
        (NodeKind::Group { clip: clip_a, .. }, NodeKind::Group { clip: clip_b, .. }) => {
            a.id == b.id
                && clip_a == clip_b
                && a.name == b.name
                && a.visible == b.visible
                && a.locked == b.locked
                && a.appearance == b.appearance
                && a.extras == b.extras
        }
        _ => a == b,
    }
}

/// Clone a `Node` for CRDT storage, stripping recursive `Group` children because
/// child shapes are stored as individual entries keyed by `{tab_id}:{node_id}`.
fn snapshot_node_for_crdt(node: &Node) -> Node {
    let mut cloned = node.clone();
    if let NodeKind::Group { children, .. } = &mut cloned.kind {
        children.clear();
    }
    cloned
}

/// Metadata cached per CRDT shape key (`"{tab_id}:{node_id}"`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedShapeMeta {
    tab_id: String,
    node_id: NodeId,
    parent_id: Option<u64>,
    index: usize,
}

/// CRDT bridge synchronizing a [`Session`]'s documents and tabs with a [`yrs::Doc`].
#[derive(Debug)]
pub struct CollabBridge {
    doc: Doc,
    shapes: MapRef,
    meta: MapRef,
    tabs: MapRef,
    local_uid_to_tab_id: HashMap<u64, String>,
    tab_states: HashMap<String, TabSyncState>,
    known_shapes: HashSet<u64>,
    known_shape_keys: HashSet<String>,
    known_shape_json: HashMap<String, String>,
    known_shape_meta: HashMap<String, CachedShapeMeta>,
    known_tab_json: HashMap<String, String>,
    known_tabs: BTreeMap<String, CrdtTab>,
    last_serialized_count: usize,
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
            local_uid_to_tab_id: HashMap::new(),
            tab_states: HashMap::new(),
            known_shapes: HashSet::new(),
            known_shape_keys: HashSet::new(),
            known_shape_json: HashMap::new(),
            known_shape_meta: HashMap::new(),
            known_tab_json: HashMap::new(),
            known_tabs: BTreeMap::new(),
            last_serialized_count: 0,
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
            local_uid_to_tab_id: HashMap::new(),
            tab_states: HashMap::new(),
            known_shapes: HashSet::new(),
            known_shape_keys: HashSet::new(),
            known_shape_json: HashMap::new(),
            known_shape_meta: HashMap::new(),
            known_tab_json: HashMap::new(),
            known_tabs: BTreeMap::new(),
            last_serialized_count: 0,
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

    /// Per-tab synchronization state for inspection and testing.
    pub fn tab_sync_state(&self, tab_id: &str) -> Option<&TabSyncState> {
        self.tab_states.get(tab_id)
    }

    /// Number of shapes serialized with `serde_json` during the most recent `sync_from_session` call.
    pub fn last_serialized_count(&self) -> usize {
        self.last_serialized_count
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
    /// - Skips inactive/unchanged tabs in O(1).
    /// - Tracks only `dirty_nodes` and `removed_nodes` per tab instead of walking and serializing all shapes.
    /// - Separates geometry/style updates from `structure_dirty` Z-order updates.
    pub fn sync_from_session(&mut self, session: &Session) -> Result<Vec<u8>, CollabError> {
        let docs = session.documents();
        if docs.is_empty() {
            return Err(CollabError::NoDocument);
        }

        self.last_serialized_count = 0;

        // Fast check if any tab needs initial mapping
        let needs_tab_lookup = docs.iter().any(|st| !self.local_uid_to_tab_id.contains_key(&st.uid));
        let mut txn = self.doc.transact_mut();

        if needs_tab_lookup {
            for key in self.tabs.keys(&txn) {
                if !self.known_tabs.contains_key(key) {
                    if let Some(out) = self.tabs.get(&txn, key) {
                        if let Ok(json_str) = String::try_from(out) {
                            if let Ok(tab) = serde_json::from_str::<CrdtTab>(&json_str) {
                                self.known_tab_json.insert(key.to_string(), json_str);
                                self.known_tabs.insert(key.to_string(), tab);
                            }
                        }
                    }
                }
            }
        }

        let single_doc_mode = docs.len() <= 1;
        let active_idx = session.active_index();

        // Detect locally closed documents/tabs and remove them from CRDT
        let current_uids: HashSet<u64> = docs.iter().map(|d| d.uid).collect();
        let closed_uids: Vec<(u64, String)> = self
            .local_uid_to_tab_id
            .iter()
            .filter(|(uid, _)| !current_uids.contains(uid))
            .map(|(uid, tid)| (*uid, tid.clone()))
            .collect();

        for (closed_uid, closed_tab_id) in closed_uids {
            self.local_uid_to_tab_id.remove(&closed_uid);
            self.tabs.remove(&mut txn, closed_tab_id.as_str());
            self.known_tabs.remove(&closed_tab_id);
            self.known_tab_json.remove(&closed_tab_id);
            self.tab_states.remove(&closed_tab_id);

            // Remove all CRDT shapes and cached metadata belonging to this tab
            let tab_prefix = format!("{}:", closed_tab_id);
            let shapes_to_remove: Vec<String> = self
                .known_shape_meta
                .iter()
                .filter(|(k, meta)| meta.tab_id == closed_tab_id || k.starts_with(&tab_prefix))
                .map(|(k, _)| k.clone())
                .collect();

            for shape_key in shapes_to_remove {
                self.shapes.remove(&mut txn, shape_key.as_str());
                self.known_shape_keys.remove(&shape_key);
                self.known_shape_json.remove(&shape_key);
                self.known_shape_meta.remove(&shape_key);
            }

            // Also check all keys currently in CRDT shapes map starting with tab_prefix
            let crdt_shapes_to_remove: Vec<String> = self
                .shapes
                .keys(&txn)
                .filter(|k| k.starts_with(&tab_prefix))
                .map(|k| k.to_string())
                .collect();
            for shape_key in crdt_shapes_to_remove {
                self.shapes.remove(&mut txn, shape_key.as_str());
                self.known_shape_keys.remove(&shape_key);
                self.known_shape_json.remove(&shape_key);
                self.known_shape_meta.remove(&shape_key);
            }
        }
        rebuild_known_shapes_set(&mut self.known_shapes, &self.known_shape_meta);

        for (idx, st) in docs.iter().enumerate() {
            let tab_id = if let Some(existing_id) = self.local_uid_to_tab_id.get(&st.uid) {
                existing_id.clone()
            } else {
                let assigned_id = if idx == 0 {
                    if let Some(first_crdt_tab) = self.known_tabs.values().min_by_key(|t| t.order) {
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

            let tab_state = self.tab_states.entry(tab_id.clone()).or_default();

            // Optimization 2: O(1) skip for inactive or unchanged tabs!
            if tab_state.is_unchanged(&st.doc, st.revision, idx) {
                tab_state.dirty_nodes.clear();
                tab_state.removed_nodes.clear();
                tab_state.structure_dirty = false;
                continue;
            }

            // Compute dirty_nodes, removed_nodes, and structure_dirty for this tab
            tab_state.collect_changes(&st.doc);

            let title = st
                .doc
                .artboards
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| format!("Tab {}", idx + 1));

            let artboards_changed = tab_state.last_artboards != st.doc.artboards;
            let meta_changed = tab_state.last_revision.is_none()
                || tab_state.last_order != Some(idx)
                || tab_state.last_title != title
                || artboards_changed
                || tab_state.structure_dirty;

            if meta_changed {
                let crdt_tab = CrdtTab {
                    id: tab_id.clone(),
                    title: title.clone(),
                    artboards: st.doc.artboards.clone(),
                    order: idx,
                    z_order: tab_state.layer_skeleton.clone(),
                };

                if let Ok(tab_json) = serde_json::to_string(&crdt_tab) {
                    if self.known_tab_json.get(&tab_id) != Some(&tab_json) {
                        self.tabs.try_update(&mut txn, tab_id.as_str(), tab_json.as_str());
                        self.known_tab_json.insert(tab_id.clone(), tab_json);
                    }
                    self.known_tabs.insert(tab_id.clone(), crdt_tab);
                }

                if single_doc_mode && active_idx == Some(idx) && (artboards_changed || tab_state.last_revision.is_none()) {
                    if let Ok(artboards_json) = serde_json::to_string(&st.doc.artboards) {
                        self.meta.try_update(&mut txn, "artboards", artboards_json);
                    }
                }
            }

            // Remove only the nodes in `removed_nodes` for this tab
            for &removed_id in &tab_state.removed_nodes {
                let shape_key = format!("{}:{}", tab_id, removed_id.0);
                self.shapes.remove(&mut txn, shape_key.as_str());
                self.known_shape_keys.remove(&shape_key);
                self.known_shape_json.remove(&shape_key);
                self.known_shape_meta.remove(&shape_key);

                // Also clean up unprefixed legacy key if present in tab-0
                if tab_id == "tab-0" {
                    let legacy_key = removed_id.0.to_string();
                    if self.known_shape_keys.remove(&legacy_key) {
                        self.shapes.remove(&mut txn, legacy_key.as_str());
                        self.known_shape_json.remove(&legacy_key);
                        self.known_shape_meta.remove(&legacy_key);
                    }
                }
            }

            // Serialize and write ONLY the nodes in `dirty_nodes` for this tab
            let mut dirty_sorted: Vec<NodeId> = tab_state.dirty_nodes.iter().copied().collect();
            dirty_sorted.sort_by_key(|id| id.0);

            for node_id in dirty_sorted {
                if let Some((node_arc, parent_id, index)) = tab_state.synced_nodes.get(&node_id) {
                    let shape_key = format!("{}:{}", tab_id, node_id.0);
                    let shape = CrdtShape {
                        node: snapshot_node_for_crdt(node_arc),
                        parent_id: *parent_id,
                        index: *index,
                        tab_id: tab_id.clone(),
                    };
                    let serialized = serde_json::to_string(&shape).map_err(|e| CollabError::Serialization(e.to_string()))?;
                    self.last_serialized_count += 1;

                    if self.known_shape_json.get(&shape_key) != Some(&serialized) {
                        self.shapes.try_update(&mut txn, shape_key.as_str(), serialized.as_str());
                        self.known_shape_json.insert(shape_key.clone(), serialized);
                    }
                    self.known_shape_keys.insert(shape_key.clone());
                    self.known_shape_meta.insert(
                        shape_key,
                        CachedShapeMeta {
                            tab_id: tab_id.clone(),
                            node_id,
                            parent_id: *parent_id,
                            index: *index,
                        },
                    );
                    self.known_shapes.insert(node_id.0);
                }
            }

            if !tab_state.removed_nodes.is_empty() {
                rebuild_known_shapes_set(&mut self.known_shapes, &self.known_shape_meta);
            }

            tab_state.last_revision = Some(st.revision);
            tab_state.last_order = Some(idx);
            tab_state.last_title = title;
            if artboards_changed {
                tab_state.last_artboards = st.doc.artboards.clone();
            }
        }

        let update = txn.encode_update_v1();
        Ok(update)
    }

    /// Alias for [`CollabBridge::apply_remote_update`]: ingests a remote CRDT update into `session`,
    /// applying updates to existing `NodeId`s strictly in-place without disturbing layer Z-order.
    pub fn sync_to_session(&mut self, session: &mut Session, update: &[u8]) -> Result<(), CollabError> {
        self.apply_remote_update(session, update)
    }

    /// Apply a remote binary update to the CRDT document and ingest changes into `session`.
    ///
    /// Reconciles tabs with local session documents (adding missing tabs without stealing
    /// focus), updates existing shapes strictly in-place, and maintains 100% deterministic Z-order
    /// per tab via the explicit `z_order` layer/group skeleton.
    pub fn apply_remote_update(&mut self, session: &mut Session, update: &[u8]) -> Result<(), CollabError> {
        if update.is_empty() {
            return Ok(());
        }

        let decoded = Update::decode_v1(update).map_err(|e| CollabError::Crdt(e.to_string()))?;

        let mut txn = self.doc.transact_mut();
        txn.apply_update(decoded).map_err(|e| CollabError::Crdt(e.to_string()))?;

        // 1. Read remote tabs from CRDT tabs map, tracking which tabs changed
        let mut remote_tabs = Vec::new();
        let mut tabs_with_z_order_change = HashSet::new();
        let mut tabs_with_artboard_change = HashSet::new();

        for key in self.tabs.keys(&txn) {
            if let Some(out) = self.tabs.get(&txn, key) {
                if let Ok(json_str) = String::try_from(out) {
                    if self.known_tab_json.get(key) != Some(&json_str) {
                        if let Ok(tab) = serde_json::from_str::<CrdtTab>(&json_str) {
                            let prev_tab = self.known_tabs.get(key);
                            if prev_tab.is_none_or(|p| p.z_order != tab.z_order) {
                                tabs_with_z_order_change.insert(tab.id.clone());
                            }
                            if prev_tab.is_none_or(|p| p.artboards != tab.artboards) {
                                tabs_with_artboard_change.insert(tab.id.clone());
                            }
                            self.known_tab_json.insert(key.to_string(), json_str);
                            self.known_tabs.insert(key.to_string(), tab.clone());
                            remote_tabs.push(tab);
                        }
                    } else if let Some(tab) = self.known_tabs.get(key).cloned() {
                        remote_tabs.push(tab);
                    }
                }
            }
        }
        // Detect tabs deleted remotely from CRDT
        let crdt_tab_keys: HashSet<String> = self.tabs.keys(&txn).map(|s| s.to_string()).collect();
        let removed_tab_ids: Vec<String> = self
            .known_tabs
            .keys()
            .filter(|tid| !crdt_tab_keys.contains(*tid))
            .cloned()
            .collect();

        for removed_tid in &removed_tab_ids {
            self.known_tabs.remove(removed_tid);
            self.known_tab_json.remove(removed_tid);
            self.tab_states.remove(removed_tid);
        }

        remote_tabs.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));

        // 2. Read shapes from CRDT shapes map, deserializing ONLY shapes whose JSON changed
        let mut dirty_shapes_by_tab: HashMap<String, Vec<(CrdtShape, Option<CachedShapeMeta>)>> = HashMap::new();
        let mut current_crdt_keys = HashSet::new();

        for key in self.shapes.keys(&txn) {
            current_crdt_keys.insert(key.to_string());
            if let Some(out) = self.shapes.get(&txn, key) {
                let json_str =
                    String::try_from(out).map_err(|_| CollabError::Deserialization(format!("failed to convert value for key {key} to string")))?;
                if self.known_shape_json.get(key) == Some(&json_str) {
                    continue;
                }
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
                let prev_meta = self.known_shape_meta.insert(
                    key.to_string(),
                    CachedShapeMeta {
                        tab_id: shape.tab_id.clone(),
                        node_id: shape.node.id,
                        parent_id: shape.parent_id,
                        index: shape.index,
                    },
                );
                self.known_shape_json.insert(key.to_string(), json_str);
                dirty_shapes_by_tab
                    .entry(shape.tab_id.clone())
                    .or_default()
                    .push((shape, prev_meta));
            }
        }

        // Detect deleted shape keys per tab
        let mut removed_by_tab: HashMap<String, Vec<NodeId>> = HashMap::new();
        let prev_keys: Vec<String> = self.known_shape_keys.difference(&current_crdt_keys).cloned().collect();
        for removed_key in prev_keys {
            self.known_shape_json.remove(&removed_key);
            if let Some(meta) = self.known_shape_meta.remove(&removed_key) {
                removed_by_tab.entry(meta.tab_id).or_default().push(meta.node_id);
            } else {
                let (key_tab, key_node_id) = if let Some(colon_pos) = removed_key.find(':') {
                    (&removed_key[..colon_pos], &removed_key[colon_pos + 1..])
                } else {
                    ("tab-0", removed_key.as_str())
                };
                if let Ok(id_u64) = key_node_id.parse::<u64>() {
                    removed_by_tab.entry(key_tab.to_string()).or_default().push(NodeId(id_u64));
                }
            }
        }
        self.known_shape_keys = current_crdt_keys;
        rebuild_known_shapes_set(&mut self.known_shapes, &self.known_shape_meta);

        // If session has no documents and there are shapes or tabs, create the initial document
        if session.documents().is_empty() && (!remote_tabs.is_empty() || !self.known_shape_keys.is_empty()) {
            let _ = session.execute("file.new", &serde_json::json!({"width": 800.0, "height": 600.0}));
        }

        if session.documents().is_empty() {
            return Ok(());
        }

        // 3. Reconcile tabs with session documents without stealing active tab focus
        let initial_active = session.active_index();
        let mut newly_bound_tabs = HashSet::new();

        // 3a. Remove local documents corresponding to tabs removed remotely in CRDT
        if !remote_tabs.is_empty() {
            let remote_tab_id_set: HashSet<&str> = remote_tabs.iter().map(|t| t.id.as_str()).collect();
            let mut uids_to_close = Vec::new();

            for (&uid, tid) in &self.local_uid_to_tab_id {
                if !remote_tab_id_set.contains(tid.as_str()) {
                    uids_to_close.push((uid, tid.clone()));
                }
            }

            for (uid, tid) in uids_to_close {
                self.local_uid_to_tab_id.remove(&uid);
                self.tab_states.remove(&tid);
                if let Some(doc_idx) = session.documents().iter().position(|d| d.uid == uid) {
                    if session.documents().len() > 1 {
                        session.close_document(doc_idx);
                    }
                }
            }
        }

        if !remote_tabs.is_empty() {
            for remote_tab in &remote_tabs {
                let already_mapped = self.local_uid_to_tab_id.values().any(|tid| tid == &remote_tab.id);
                if !already_mapped {
                    let unmapped_uid = session
                        .documents()
                        .iter()
                        .find(|d| !self.local_uid_to_tab_id.contains_key(&d.uid))
                        .map(|d| d.uid);
                    if let Some(uid) = unmapped_uid {
                        self.local_uid_to_tab_id.insert(uid, remote_tab.id.clone());
                        newly_bound_tabs.insert(remote_tab.id.clone());
                        if let Some(doc_st) = session.documents_mut().iter_mut().find(|d| d.uid == uid) {
                            let doc = Arc::make_mut(&mut doc_st.doc);
                            if !remote_tab.artboards.is_empty() {
                                doc.artboards = remote_tab.artboards.clone();
                            }
                            if let Some(first_ab) = doc.artboards.first_mut() {
                                first_ab.name = remote_tab.title.clone();
                            }
                        }
                    } else {
                        let (w, h) = remote_tab
                            .artboards
                            .first()
                            .map(|a| (a.rect.width(), a.rect.height()))
                            .unwrap_or((800.0, 600.0));
                        let mut new_doc = Document::new(w, h);
                        if !remote_tab.artboards.is_empty() {
                            new_doc.artboards = remote_tab.artboards.clone();
                        }
                        if let Some(first_ab) = new_doc.artboards.first_mut() {
                            first_ab.name = remote_tab.title.clone();
                        }
                        let new_idx = session.add_document(new_doc, None);
                        let new_uid = session.documents()[new_idx].uid;
                        self.local_uid_to_tab_id.insert(new_uid, remote_tab.id.clone());
                        newly_bound_tabs.insert(remote_tab.id.clone());
                    }
                }
            }

            if let Some(prev) = initial_active {
                if prev < session.documents().len() {
                    session.set_active(prev);
                }
            }
        } else if let Some(first) = session.documents().first() {
            let first_uid = first.uid;
            if let std::collections::hash_map::Entry::Vacant(e) = self.local_uid_to_tab_id.entry(first_uid) {
                e.insert("tab-0".to_string());
                newly_bound_tabs.insert("tab-0".to_string());
            }
        }

        // If a tab was newly bound to a local document, ensure any already-known shapes for that tab are applied
        for newly_bound_tab in &newly_bound_tabs {
            let already_queued: HashSet<NodeId> = dirty_shapes_by_tab
                .get(newly_bound_tab)
                .map(|v| v.iter().map(|(s, _)| s.node.id).collect())
                .unwrap_or_default();
            for (key, meta) in &self.known_shape_meta {
                if &meta.tab_id == newly_bound_tab && !already_queued.contains(&meta.node_id) {
                    if let Some(json_str) = self.known_shape_json.get(key) {
                        if let Ok(mut shape) = serde_json::from_str::<CrdtShape>(json_str) {
                            if shape.tab_id.is_empty() {
                                shape.tab_id = newly_bound_tab.clone();
                            }
                            dirty_shapes_by_tab
                                .entry(newly_bound_tab.clone())
                                .or_default()
                                .push((shape, None));
                        }
                    }
                }
            }
        }

        // 4. Apply changes per tab: in-place updates for existing nodes, stable Z-order skeleton for structural edits
        for (doc_idx, st) in session.documents_mut().iter_mut().enumerate() {
            let tab_id = match self.local_uid_to_tab_id.get(&st.uid) {
                Some(id) => id.clone(),
                None => continue,
            };

            let dirty_shapes = dirty_shapes_by_tab.remove(&tab_id).unwrap_or_default();
            let removed_nodes = removed_by_tab.remove(&tab_id).unwrap_or_default();
            let z_order_changed = tabs_with_z_order_change.contains(&tab_id) || newly_bound_tabs.contains(&tab_id);
            let artboards_changed = tabs_with_artboard_change.contains(&tab_id) || newly_bound_tabs.contains(&tab_id);

            if dirty_shapes.is_empty() && removed_nodes.is_empty() && !z_order_changed && !artboards_changed {
                continue;
            }

            let doc = Arc::make_mut(&mut st.doc);
            let mut containers_to_sort: HashSet<Option<NodeId>> = HashSet::new();

            // 4a. Remove shapes deleted in this tab
            for node_id in removed_nodes {
                if doc.node(node_id).is_some() {
                    let parent = doc.parent_of(node_id);
                    let _ = doc.remove(node_id);
                    containers_to_sort.insert(parent);
                }
            }

            // 4b. Separate dirty shapes into existing nodes (update IN-PLACE) and new nodes (insert)
            let mut existing_updates = Vec::new();
            let mut new_shapes = Vec::new();

            for (shape, prev_meta) in dirty_shapes {
                if doc.node(shape.node.id).is_some() {
                    existing_updates.push((shape, prev_meta));
                } else {
                    new_shapes.push(shape);
                }
            }

            // Sort new shapes deterministically by (index, id) — never rely on YMap/HashMap order
            new_shapes.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.node.id.0.cmp(&b.node.id.0)));

            // Insert new shapes topologically (parents/groups before their children)
            let mut pending_new = new_shapes;
            while !pending_new.is_empty() {
                let pending_ids: HashSet<u64> = pending_new.iter().map(|s| s.node.id.0).collect();
                let mut next_round = Vec::new();
                let mut progressed = false;

                for shape in pending_new {
                    let parent_waiting = shape.parent_id.is_some_and(|pid| pending_ids.contains(&pid));
                    if parent_waiting {
                        next_round.push(shape);
                        continue;
                    }

                    let parent_container = resolve_parent_container(doc, shape.parent_id)
                        .or_else(|| Some(doc.add_layer(None)));
                    let node_to_insert = snapshot_node_for_crdt(&shape.node);
                    let _ = doc.insert(parent_container, shape.index, node_to_insert);
                    containers_to_sort.insert(parent_container);
                    progressed = true;
                }

                if !progressed {
                    // Break any cycle or missing parent by inserting remaining into resolved/default layer
                    for shape in next_round {
                        let parent_container = resolve_parent_container(doc, shape.parent_id)
                            .or_else(|| Some(doc.add_layer(None)));
                        let node_to_insert = snapshot_node_for_crdt(&shape.node);
                        let _ = doc.insert(parent_container, shape.index, node_to_insert);
                        containers_to_sort.insert(parent_container);
                    }
                    break;
                }
                pending_new = next_round;
            }

            // 4c. Update existing nodes strictly IN-PLACE without deleting or re-appending
            existing_updates.sort_by(|(a, _), (b, _)| a.index.cmp(&b.index).then_with(|| a.node.id.0.cmp(&b.node.id.0)));
            for (shape, prev_meta) in existing_updates {
                let node_id = shape.node.id;
                let current_parent = doc.parent_of(node_id);
                let target_parent = resolve_parent_container(doc, shape.parent_id);

                // In-place update of geometry/style/attributes, preserving container children
                if let Some(existing) = doc.node_mut(node_id) {
                    let preserved_children = existing.children().cloned();
                    let mut updated_node = shape.node;
                    if let (Some(children_vec), Some(slot)) = (preserved_children, updated_node.children_mut()) {
                        *slot = children_vec;
                    }
                    *existing = updated_node;
                }

                // Only move/re-sort if parent container changed or index explicitly changed
                let index_changed = prev_meta.is_some_and(|m| m.index != shape.index || m.parent_id != shape.parent_id);
                if current_parent != target_parent {
                    let _ = doc.move_node(node_id, target_parent, shape.index);
                    containers_to_sort.insert(current_parent);
                    containers_to_sort.insert(target_parent);
                } else if index_changed {
                    containers_to_sort.insert(target_parent);
                }
            }

            let remote_tab = remote_tabs.iter().find(|t| t.id == tab_id);
            if z_order_changed {
                if let Some(rt) = remote_tab {
                    for &container_raw_id in rt.z_order.keys() {
                        if container_raw_id == 0 {
                            containers_to_sort.insert(None);
                        } else {
                            let cid = NodeId(container_raw_id);
                            if doc.node(cid).is_some() {
                                containers_to_sort.insert(Some(cid));
                            } else if let Some(def_layer) = doc.default_layer() {
                                containers_to_sort.insert(Some(def_layer));
                            }
                        }
                    }
                }
            }

            // 4d. Reconcile Z-order in-place ONLY for containers whose structure changed
            if !containers_to_sort.is_empty() {
                let mut index_lookup: HashMap<NodeId, usize> = HashMap::new();
                for meta in self.known_shape_meta.values() {
                    if meta.tab_id == tab_id {
                        index_lookup.insert(meta.node_id, meta.index);
                    }
                }

                let default_layer_id = doc.default_layer();
                for container in containers_to_sort {
                    let explicit_order: Option<&Vec<u64>> = remote_tab.and_then(|rt| {
                        match container {
                            None => rt.z_order.get(&0),
                            Some(cid) => rt.z_order.get(&cid.0).or_else(|| {
                                if Some(cid) == default_layer_id {
                                    rt.z_order.iter().find(|&(k, _)| *k != 0).map(|(_, v)| v)
                                } else {
                                    None
                                }
                            }),
                        }
                    });

                    let order_positions: HashMap<u64, usize> = explicit_order
                        .map(|list| list.iter().enumerate().map(|(pos, &id)| (id, pos)).collect())
                        .unwrap_or_default();

                    if let Ok(children) = doc.children_mut(container) {
                        children.sort_by_key(|child| {
                            if let Some(&pos) = order_positions.get(&child.id.0) {
                                (0usize, pos, child.id.0)
                            } else if let Some(&idx) = index_lookup.get(&child.id) {
                                (1usize, idx, child.id.0)
                            } else {
                                (2usize, usize::MAX, child.id.0)
                            }
                        });
                    }
                }
            }

            // 4e. Artboards update from tab if present
            if let Some(rt) = remote_tab {
                if !rt.artboards.is_empty() && doc.artboards != rt.artboards {
                    doc.artboards = rt.artboards.clone();
                }
            }

            doc.fix_next_id();
            if st.active_layer.is_none() || !st.active_layer.is_some_and(|l| doc.node(l).is_some_and(|n| n.is_layer())) {
                st.active_layer = doc.default_layer();
            }
            st.selection.objects.retain(|id| doc.node(*id).is_some());
            if let Some(k) = st.selection.key {
                if doc.node(k).is_none() {
                    st.selection.key = None;
                }
            }
            st.selection.anchors.retain(|id, _| doc.node(*id).is_some());
            st.revision += 1;

            let title = st
                .doc
                .artboards
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| format!("Tab {}", doc_idx + 1));
            let tab_state = self.tab_states.entry(tab_id).or_default();
            tab_state.mark_synced(&st.doc, st.revision, doc_idx, title);
        }

        // Synchronize legacy document artboards from meta (only for legacy single-document clients)
        if remote_tabs.is_empty() {
            if let Some(out) = self.meta.get(&txn, "artboards") {
                if let Ok(json_str) = String::try_from(out) {
                    if let Ok(artboards) = serde_json::from_str::<Vec<vectorcraft_doc::Artboard>>(&json_str) {
                        let active_idx = session.active_index().unwrap_or(0);
                        if let Some(active_st) = session.active_mut() {
                            let active_doc = Arc::make_mut(&mut active_st.doc);
                            if !artboards.is_empty() && active_doc.artboards != artboards {
                                active_doc.artboards = artboards;
                                active_st.revision += 1;
                                if let Some(tab_id) = self.local_uid_to_tab_id.get(&active_st.uid).cloned() {
                                    let title = active_st
                                        .doc
                                        .artboards
                                        .first()
                                        .map(|a| a.name.clone())
                                        .unwrap_or_else(|| "Tab 1".to_string());
                                    self.tab_states
                                        .entry(tab_id)
                                        .or_default()
                                        .mark_synced(&active_st.doc, active_st.revision, active_idx, title);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn rebuild_known_shapes_set(known_shapes: &mut HashSet<u64>, known_shape_meta: &HashMap<String, CachedShapeMeta>) {
    known_shapes.clear();
    for meta in known_shape_meta.values() {
        known_shapes.insert(meta.node_id.0);
    }
}

fn resolve_parent_container(doc: &Document, parent_id: Option<u64>) -> Option<NodeId> {
    parent_id
        .map(NodeId)
        .filter(|&pid| doc.node(pid).is_some_and(|n| n.is_layer() || matches!(n.kind, NodeKind::Group { .. })))
        .or_else(|| doc.default_layer())
}

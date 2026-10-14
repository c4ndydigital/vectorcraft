# VectorCraft Collab & PDF Image Sync Plan

## 1. Abgeschlossener Stand
- [x] **Wasm SIMD**: In `.cargo/config.toml` für `wasm32-unknown-unknown` aktiviert (`+simd128`).
- [x] **Tab-isoliertes Dirty-Node Tracking**: `collab.rs` überspringt inaktive Tabs in $\mathcal{O}(1)$, kein `st.doc.walk()` über 20.000 Shapes mehr bei Teiledits.
- [x] **100% stabile Z-Order**: Geometrie-/Style-Updates verändern nicht die Ebenen-Hierarchie; remote Updates werden strikt in-place angewendet (`doc.node_mut`).
- [x] **Ephemeral Drag State**: Dragging/Transformieren unterdrückt Zwischen-Syncs auf Frame-Ebene; Commit erfolgt sauber auf `pointerup`; Awareness/Cursor streamt mit 60 FPS.
- [x] **PDF-Import & Image-Blob Sync**: Raster-Bilder aus PDF (`Untitled-1.pdf` und `t5-DM-Look`) werden sauber in `doc.images` extrahiert, serverseitig in `room.blobs` gecacht, an Reconnect/Reload-Peers übertragen und per automatisiertem Adversarial-Test abgesichert.
- [x] **Kryptografisch sichere Raum-IDs (Datenschutz & Anti-Enumeration)**: Ersetzung der 4-stelligen Zufallszahlen (`c4ndy-1000..9999`) durch kryptografische 12-Zeichen Web Crypto Tokens (`window.crypto.getRandomValues`). Bietet ~ $4.7 \times 10^{18}$ Kombinationen bei kurzen, sauberen URLs (`c4ndy-a9x8k1m7pq2v`).
- [x] Alle Tests in `vectorcraft-engine`, `vectorcraft-ui-egui`, `vectorcraft-server` und WASM-Build (`vectorcraft-web`) kompilieren und bestehen fehlerfrei.

---

## 2. Nächste Schritte (Top-Prioritäten für den neuen Chat)

- [x] **Tab-Schließen & Reload-Stabilität (`tab_close` & Reload Sync)**:
  - Beim Schließen von Tabs/Dokumenten (`session.close_document`): Automatische Erkennung entfernter UIDs in `sync_from_session`, Löschen des Eintrags aus dem CRDT `tabs`-Map sowie aller zugehörigen Shapes (`{tab_id}:*`) aus `shapes`.
  - Bei Remote-Peers (`apply_remote_update`): Sauberes Schließen der entsprechenden lokalen Dokumente mit `session.close_document(doc_idx)` ohne Fokus-Verlust, Bereinigung von `known_tabs`, `known_tab_json`, `tab_states` und `collab_doc_revisions`.
  - Beim Browser-Reload / Reconnect: Initialer CRDT-Snapshot überschreibt oder initialisiert das Standarddokument sauber; geschlossene Tabs und gelöschte Shapes bleiben dauerhaft gelöscht und tauchen beim Reload nicht wieder auf.
  - Vollständige View-Synchronisation in `crates/ui-egui/src/collab.rs` (`views.len() == session.documents().len()`).
  - **Automatisierte Regressionstests**:
    - `crates/engine/tests/crdt_sync.rs`: `test_crdt_multi_tab_close_sync` (bidirektionaler Sync beim Schließen von Tabs und Shape-Pruning).
    - `crates/ui-egui/tests/collab_adversarial.rs`: `test_adversarial_tab_close_sync` (UI Frame-Loop, In-/Outbox und View-Sync).
    - `crates/ui-egui/tests/collab_adversarial.rs`: `test_adversarial_reload_after_tab_close_and_shape_deletion` (Reload-Stabilität aus Server-CRDT-Snapshot).

---

## 2. Nächste Schritte (Roadmap)

1. **Live Drag & Transform Preview (Figma-Style Ephemeral Live Movement)**:
   - Aktuell werden Dragging/Skalieren/Rotieren lokal in `preview` gehalten und erst bei `pointerup` als CRDT-Commit gesendet (verhindert CRDT-History-Bloat).
   - Um das Live-Verschieben/Skalieren wie in Figma bei Remote-Peers flüssig sichtbar zu machen: Übertragung einer leichtgewichtigen ephemeren `PeerInteraction` / Ghost-Bounding-Box im Presence-/Cursor-Stream (oder separatem Ephemeral-Kanal), sodass Kollegen das Live-Dragging in Echtzeit sehen, ohne dass tausende Zwischenzustände im CRDT gespeichert werden müssen.

2. **Cursor-Latenz & WebSocket Head-of-Line Blocking bei großen Dokumenten / PDFs**:
   - Wenn große PDFs oder viele Shapes importiert werden, belegen Updates oder Blob-Übertragungen die WebSocket-Verbindung.
   - Entkopplung / Priorisierung: Cursor- und Presence-Nachrichten dürfen nicht hinter großen CRDT-Binärblobs in der Queue feststecken (Priority Outbox / separates Droppen veralteter Cursornachrichten).

3. **Passwortgeschützte Räume / Private Room Auth (Optional/Roadmap)**:
   - Raum-PIN oder Passwortschutz im Server-Handshake (`ws_handler` & MCP endpoints), um geschützte Kollaborationsräume zu ermöglichen.

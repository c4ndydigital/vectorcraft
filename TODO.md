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

1. [x] **Live Drag & Transform Preview (Figma-Style Ephemeral Live Movement)**:
   - Leichtgewichtige ephemere `PeerInteraction` mit Bounding Box und Interaktions-Label (`"Move"`, `"Transform"`, etc.) im Presence-Stream (`PeerPresence`).
   - Übertragung mit 60 FPS während aktivem Dragging/Transformieren ohne CRDT-History-Bloat.
   - Remote-Canvas (`paint_remote_cursors`) rendert eine semitransparente Ghost-Bounding-Box in der Akzentfarbe des Peers samt Namens-Badge.
   - Sofortiges Löschen der Ghost-Box bei `pointerup` (`active_interaction: None`) und sauberer CRDT-Commit des finalen Zustands.
   - **Automatisierter Regressionstest**:
     - `crates/ui-egui/tests/collab_adversarial.rs`: `test_adversarial_ephemeral_live_drag_preview` (Verifiziert CRDT-Unterdrückung während Drag, Presence-Bounds Übertragung, remote Ghost-Box und sauberes Clearen auf PointerUp).

2. [x] **Cursor-Latenz & WebSocket Head-of-Line Blocking bei großen Dokumenten / PDFs**:
   - Wenn große PDFs oder viele Shapes importiert werden, belegen Updates oder Blob-Übertragungen die WebSocket-Verbindung.
   - **Priorisierung & Coalescing**:
     - `prioritize_and_coalesce_outbox(&mut queue)` teilt ausgehende Pakete nach Priorität auf: High (`PeerPresence`), Normal (`Update`), Bulk (`Blob`).
     - Schnelle Cursornachrichten (`PeerPresence`) werden stets vor großen Binärblobs übermittelt.
     - Veraltete Cursornachrichten desselben Clients werden verworfen (Coalescing), sodass nur die jeweils neueste Cursor-Position übertragen wird.
   - **Automatisierter Regressionstest**:
     - `crates/ui-egui/tests/collab_adversarial.rs`: `test_adversarial_outbox_cursor_priority_and_coalescing` (Verifiziert Priorisierung von Presence vor Updates/Blobs sowie Coalescing veralteter Cursor-Frames).

3. [x] **Pathfinder & Compound-Path / Text-Outlines Collab Sync & Disappear Fix**:
   - **Problemstellung**:
     - **Pathfinder / Loch ausschneiden (`Minus Front`, `Exclude`)**: Beim Ausschneiden eines Vierecks aus einem anderen Viereck verliert der 2. Browser die Füllung bzw. bildet die Schnitte als ungestylte Sub-Objekte ab. Wird das Objekt danach verschoben (`Move` / Drag-Commit), verschwindet es in einem Browser komplett.
     - **Text in Pfade umwandeln (`Create Outlines` / `type.createOutlines`)**: Nach der Umwandlung von Text in Pfade (Gruppe von Glyphen-Pfaden / Compound Paths) und anschließendem Skalieren/Transformieren verschwinden einzelne Buchstaben im 2. Browser oder verlieren ihre Füllung (z. B. Buchstaben mit Innenlöchern wie 'o', 'e', 'a', 'p', 'b', 'd', 'B', 'A' als Compound Paths).
   - **Ursachenanalyse & Behebung**:
     - `NodeKind::Compound { children, rule }` wird nun in `collab.rs` als atomare Geometrie-Einheit behandelt (`hierarchical_children` ignoriert Compound-Subpfade).
     - Subpfade werden nicht mehr in Einzelshapes zerlegt und verbleiben als integrales Kind des Compound-Knotens.
     - In-Place-Updates überschreiben Subpfade von Compound-Nodes vollständig und bewahren Kinder nur für echte `Group`-Knoten.
   - **Automatisierte Regressionstests**:
     - `crates/engine/tests/crdt_sync.rs`: `test_crdt_pathfinder_compound_sync_and_move` (Verifiziert Minus-Front-Loch, blaue Füllung und Verschieben ohne Disappear).
     - `crates/engine/tests/crdt_sync.rs`: `test_crdt_text_create_outlines_and_scale` (Verifiziert Outlines von Buchstaben mit Punzen wie 'AB', Skalieren und Füllungs-Erhalt).

4. [x] **Umfassende Illustrator-Editing Collab Sync Testsuite (Automated End-to-End Battery)**:
   - **Ziel & Umsetzung**:
     - Riesiger automatisierter End-to-End-Testlauf in `crates/ui-egui/tests/collab_adversarial.rs` (`test_adversarial_illustrator_editing_sync_battery`).
     - Simuliert zwei reale Browser-Peers (Peer A & Peer B) mit UI-Loop, In-/Outbox und wechselseitigem CRDT-Streaming.
     - Testet Rechtecke, Konturen (Strokes), Füllungen (Fills), Pathfinder `Minus Front` (Loch-Ausschnitt), Verschieben/Skalieren von Compound-Pfaden, Text in Pfade (`type.createOutlines`), Glyphen-Skalierung und Z-Order `Bring to Front`.
     - Assertion verifiziert am Ende 100% bit- und knotengenaue Gleichheit beider Browser-Dokumente (`assert_eq!(count_a, count_b)`).

5. **Passwortgeschützte Räume / Private Room Auth (Optional/Roadmap)**:
   - Raum-PIN oder Passwortschutz im Server-Handshake (`ws_handler` & MCP endpoints), um geschützte Kollaborationsräume zu ermöglichen.



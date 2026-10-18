# C4NDY Digital Fork — Architecture, Features & Upstream Merge Guide

> **Author:** C4NDY Digital / Adrian Jehne & AI Agents (Gemini 4 Argon, Gemini 3.8/3.7 Flash, Claude Opus 5.5)  
> **Repository:** `git@github.com:c4ndydigital/vectorcraft.git`  
> **Upstream:** `https://github.com/storytold/vectorcraft.git`  
> **Target:** Seamless synchronization with upstream while preserving all collaborative, MCP, security, and DSGVO additions.

---

## 1. Overview of Fork Additions

VectorCraft by Storytold is a clean-room Rust-native vector illustration application (desktop, web wasm, and CLI).  
The **C4NDY Digital** fork extends VectorCraft into a production-ready, multiplayer collaborative vector suite with remote MCP AI automation and DSGVO-compliant self-hosted hosting.

### The 7 Core Architectural Pillars:

1. **Multiplayer Live Collaboration Engine (CRDT & WebSockets)**
   - Yrs (Yjs Rust port) integration syncing tabs, shapes, attributes, and z-order.
   - Low-latency ephemeral presence stream: real-time 60 FPS remote cursor positions and selection badges.
   - Live Ephemeral Movement / Drag & Transform previews (Figma-style ghost boxes without CRDT history bloat).
   - Priority outbox with cursor coalescing to eliminate WebSocket head-of-line blocking.
   - Compound path and outlined text (`NodeKind::Compound`) atomic synchronization.
   - Multi-tab synchronization and clean `tab_close` pruning.

2. **Room Security & Access Control**
   - Cryptographically secure 12-character alphanumeric room tokens (`c4ndy-xxxxxxxxxxxx` via `window.crypto.getRandomValues`).
   - Optional PIN / Password protection for rooms (`🔒` / `🔓` topbar toggle and interactive auth modal).
   - Server-side WebSocket and MCP validation rejecting unauthorized connections (`401 Unauthorized`).

3. **Remote MCP (Model Context Protocol) Server**
   - Zero-config remote MCP server built directly into `vectorcraft-server`.
   - Multi-transport: HTTP SSE (`/mcp/sse?room=...`) and direct JSON-RPC (`/mcp` / `/api/rooms/{id}/mcp`).
   - Claude and AI agent integration: AI agents can directly view, inspect, and manipulate the live collaborative canvas in real-time.

4. **DSGVO / GDPR Compliance & Self-Hosted Infrastructure**
   - 100% self-hosted, on-demand Google Fonts & Fontsource CDN proxy (`/fonts/{*path}` and `/api/fonts/{*path}`).
   - Client never leaks IP addresses or telemetry to external third parties (Google, CDNs).
   - In-app Legal Dialogs (`Impressum`, `Datenschutzerklärung`).
   - Optional, cookie-less, self-hosted Umami Analytics injection.
   - Ephemeral in-memory room storage: room data is kept only in RAM while participants are active and wiped on grace period expiry.

5. **Client-Side On-Demand Font Loading**
   - WebAssembly client queries `/fonts/...` proxy on-demand when opening documents with missing fonts.
   - Popular Google Fonts (Roboto, Montserrat, Poppins, Inter, Lato, Ubuntu, etc.) included in the Font Picker with instant preview and on-demand registration into `FontDb::global()`.

6. **Automated Server Resource Monitoring & Limits**
   - cgroups v2 memory monitoring (`/sys/fs/cgroup/memory.current` & `memory.max`) and process RSS tracking.
   - Max rooms and max clients limits, memory threshold alerts with cooldown, and Bearer-protected `/status` and `/metrics`.

7. **Production Deployment & Tooling**
   - Multi-stage Dockerfile, Coolify / Railpack deployment configurations (`nixpacks.toml`, `Procfile`).
   - Automated target disk cleaner (`scripts/clean_target.sh`) preventing build crashes.
   - Extensive adversarial, end-to-end and empirical test suites.

---

## 2. Inventory of Modified & Added Files

### A. New Modules & Crates (Fork Owned)
| Path | Layer / Role | Description |
|---|---|---|
| `apps/vectorcraft-server/` | L7 Server App | Axum WebSocket collaboration, MCP SSE/RPC, Font Proxy, resource limits & metrics |
| `crates/engine/src/collab.rs` | L5 Engine | CRDT state machine (`CollabEngine`), document diffing, shape mapping, tab synchronization |
| `crates/ui-egui/src/collab.rs` | L6 UI | Presence badges, remote cursors, ghost drag bounds, prioritized outbox queue |
| `crates/ui-egui/src/dialogs/legal.rs` | L6 UI | In-app Impressum & Privacy policy dialogs |
| `crates/engine/tests/crdt_sync.rs` | Tests | Multi-tab sync, compound paths, text outlines, reload stability tests |
| `crates/ui-egui/tests/collab_adversarial.rs` | Tests | Two-peer UI loop, priority outbox, drag previews, PIN auth tests |
| `apps/vectorcraft-server/tests/` | Tests | MCP remote tests, font proxy tests, resource monitor tests |
| `scripts/clean_target.sh` | Scripts | Automated disk cleaner (clears incremental cache / cargo clean when > 15 GB) |
| `Dockerfile`, `.env.example`, `nixpacks.toml` | Deployment | Containerization, deployment configs |

### B. Modified Upstream Files (Integration Points)

When upstream merges occur, these are the **only** existing files that have modifications:

#### 1. `Cargo.toml` & `Cargo.lock`
- **Changes**: Adds `apps/vectorcraft-server` to workspace members; adds `yrs = "0.28.0"` to dependencies.
- **Merge Strategy**: Keep `apps/vectorcraft-server` in `members` and `yrs` in workspace dependencies.

#### 2. `crates/engine/src/lib.rs`
- **Changes**: Exports `pub mod collab;`.
- **Merge Strategy**: Trivial, retain `pub mod collab;`.

#### 3. `crates/ui-egui/src/lib.rs`
- **Changes**:
  - Adds collaboration queues to `Services`: `collab_inbox`, `collab_outbox`, and `request_font`.
  - Re-exports `collab` items (`CollabIncoming`, `CollabOutgoing`, `PeerPresence`, `prioritize_and_coalesce_outbox`).
- **Merge Strategy**: Preserve the added fields in `Services` struct and default implementation.

#### 4. `crates/ui-egui/src/canvas.rs`
- **Changes**: Calls `paint_remote_cursors` in `Canvas::paint` to render remote peer pointers, names, and ghost drag boxes.
- **Merge Strategy**: Retain remote cursor rendering call before local overlays.

#### 5. `crates/ui-egui/src/chrome.rs`
- **Changes**:
  - Adds Share button (`🔗 Teilen`) and Room Lock button (`🔒`/`🔓`) to topbar.
  - Adds Room Password modal (`show_room_password`).
- **Merge Strategy**: Retain the collab topbar widgets and room password dialog call.

#### 6. `crates/ui-egui/src/font_menu.rs`
- **Changes**:
  - Appends `POPULAR_GOOGLE_FONTS` to search results.
  - In `apply(FontPick::Chosen(f, style))`: invokes `app.services.request_font` if the chosen font is not yet in `FontDb`.
- **Merge Strategy**: Retain the Google Fonts list and `request_font` hook.

#### 7. `crates/ui-egui/src/dialogs/missing_fonts.rs`
- **Changes**:
  - In `after_open_for`: on web mode, invokes `app.services.request_font` for all missing fonts in the document.
- **Merge Strategy**: Retain the on-demand font fetching loop.

#### 8. `crates/ui-egui/src/dialogs/about.rs`
- **Changes**:
  - Replaces old Discord link with C4NDY Digital fork documentation.
  - Adds "KI & MCP Connect" tab with copy buttons for Claude SSE/RPC URLs.
- **Merge Strategy**: Keep C4NDY Digital fork branding and MCP tab.

#### 9. `apps/vectorcraft-web/src/web.rs`
- **Changes**:
  - Resolves WebSocket URL (`resolve_ws_url`), initializes room ID tokens, wires `collab_inbox`/`collab_outbox`.
  - Implements `drain_collab_outbox`.
  - Wires `request_font` service to `crate::fonts::fetch_font_on_demand`.
- **Merge Strategy**: Retain collab connection, WebSocket send tasks, and font service wiring.

#### 10. `apps/vectorcraft-web/src/fonts.rs`
- **Changes**:
  - Implements `fetch_font_on_demand` using browser `fetch`.
- **Merge Strategy**: Retain `fetch_font_on_demand` and `fetch_simple`.

#### 11. `contributors/contributors.json` & `contributors/people.toml`
- **Changes**: Adds C4NDY Digital, Adrian Jehne, and AI models (Gemini 4 Argon, Gemini 3.8/3.7 Flash).
- **Merge Strategy**: Append C4NDY Digital contributors.

---

## 3. Step-by-Step Upstream Merge Protocol

To pull new releases or commits from `upstream/main` into `main`:

### Step 1: Fetch Upstream
```bash
git fetch upstream main
```

### Step 2: Create a Merge Branch
```bash
git checkout -b merge-upstream-$(date +%Y%m%d) main
```

### Step 3: Perform Merge
```bash
git merge upstream/main
```

### Step 4: Resolve Any Conflicts by Component
- **If conflict in `crates/engine/src/lib.rs`**: Ensure `pub mod collab;` is present.
- **If conflict in `crates/ui-egui/src/lib.rs`**: Keep `collab_inbox`, `collab_outbox`, `request_font` on `Services`.
- **If conflict in `crates/ui-egui/src/chrome.rs`**: Ensure Share and Lock buttons remain in the topbar.
- **If conflict in `crates/ui-egui/src/canvas.rs`**: Ensure `crate::collab::paint_remote_cursors(...)` remains at the end of canvas painting.
- **If conflict in `crates/ui-egui/src/font_menu.rs`**: Ensure `POPULAR_GOOGLE_FONTS` and `app.services.request_font` call are kept.
- **If conflict in `apps/vectorcraft-web/src/web.rs`**: Ensure WebSocket loop and services initialization include collab and font hooks.

### Step 5: Run Verification Suite
```bash
# 1. Test engine & collab CRDT
cargo test -p vectorcraft-engine --test crdt_sync

# 2. Test UI & adversarial collab
cargo test -p vectorcraft-ui-egui --test collab_adversarial

# 3. Test server (WebSocket, MCP, Font CDN, Resource Monitor)
cargo test -p vectorcraft-server

# 4. Test WebAssembly build
(cd apps/vectorcraft-web && trunk build --release)

# 5. Clean target directory to save space
/srv/photoshop/scripts/clean_target.sh
```

### Step 6: Commit and Push
```bash
git checkout main
git merge merge-upstream-$(date +%Y%m%d) --ff-only
git push origin main
```

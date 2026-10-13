//! Remote MCP (Model Context Protocol) integration for VectorCraft rooms.
//!
//! Exposes each collaboration room as a real-time, zero-setup MCP server over:
//! - SSE transport: `GET /mcp/sse?room=ROOM_ID` + `POST /mcp/message?sessionId=...&room=ROOM_ID`
//! - Direct JSON-RPC POST: `POST /mcp?room=ROOM_ID`
//! - Tool & endpoint discovery: `GET /mcp?room=ROOM_ID`

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;
use vectorcraft_mcp::{Backend, tool_definitions};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use super::{AppState, PeerMessage};

/// RAII guard to automatically clean up active SSE sessions upon client disconnect.
struct SessionCleanup {
    state: AppState,
    session_id: String,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let mut sessions = self.state.mcp_sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.remove(&self.session_id);
    }
}

/// Backend connecting an in-memory VectorCraft [`Headless`] session and [`CollabBridge`]
/// directly to a live room's CRDT document and WebSocket broadcast channel.
pub struct RoomBackend {
    pub room_id: String,
    pub client_id: u64,
    pub headless: vectorcraft_mcp::Headless,
    pub bridge: vectorcraft_engine::collab::CollabBridge,
    pub room_doc: Arc<std::sync::Mutex<Doc>>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<PeerMessage>,
}

impl RoomBackend {
    pub fn new(
        room_id: String,
        client_id: u64,
        room_doc: Arc<std::sync::Mutex<Doc>>,
        broadcast_tx: tokio::sync::broadcast::Sender<PeerMessage>,
    ) -> Self {
        let mut headless = vectorcraft_mcp::Headless::new();
        headless.ensure_document();
        let mut bridge = vectorcraft_engine::collab::CollabBridge::with_client_id(client_id);

        let snapshot_bytes = {
            let doc_guard = room_doc.lock().unwrap_or_else(|e| e.into_inner());
            doc_guard.transact().encode_state_as_update_v1(&StateVector::default())
        };
        if !snapshot_bytes.is_empty() {
            let _ = bridge.apply_remote_update(&mut headless.session, &snapshot_bytes);
        }

        Self {
            room_id,
            client_id,
            headless,
            bridge,
            room_doc,
            broadcast_tx,
        }
    }

    fn sync_incoming_updates(&mut self) {
        let snapshot_bytes = {
            let doc_guard = self.room_doc.lock().unwrap_or_else(|e| e.into_inner());
            doc_guard.transact().encode_state_as_update_v1(&StateVector::default())
        };
        if !snapshot_bytes.is_empty() {
            self.headless.ensure_document();
            let _ = self.bridge.apply_remote_update(&mut self.headless.session, &snapshot_bytes);
        }
    }

    fn broadcast_cursor(&self, x: f64, y: f64, name: &str, color: Value) {
        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let presence = json!({
            "client_id": self.client_id,
            "name": name,
            "color": color,
            "cursor": {"x": x, "y": y},
            "last_seen": now_sec,
            "active_tab": 0
        });
        let _ = self.broadcast_tx.send(PeerMessage::Text {
            sender_id: self.client_id,
            data: presence.to_string(),
        });
    }
}

impl Backend for RoomBackend {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.sync_incoming_updates();

        if method == "collab.cursor" || method == "ui.cursor" || method == "set_cursor" {
            let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("Antigravity AI (MCP)");
            let color = params.get("color").cloned().unwrap_or_else(|| json!([242, 78, 30, 255]));
            self.broadcast_cursor(x, y, name, color);
            return Ok(json!({"status": "ok", "cursor": {"x": x, "y": y}}));
        }

        // For drawing operations, also animate cursor to the target coordinates so humans see the AI pointing!
        if method == "shape.rectangle" || method == "shape.ellipse" || method == "draw_shape" {
            let x = params.get("x").and_then(Value::as_f64).or_else(|| params.get("cx").and_then(Value::as_f64)).unwrap_or(200.0);
            let y = params.get("y").and_then(Value::as_f64).or_else(|| params.get("cy").and_then(Value::as_f64)).unwrap_or(200.0);
            self.broadcast_cursor(x, y, "Antigravity AI (MCP)", json!([242, 78, 30, 255]));
        }

        let res = self.headless.call(method, params);

        if res.is_ok() {
            if let Ok(update) = self.bridge.sync_from_session(&self.headless.session) {
                if !update.is_empty() {
                    if let Ok(u) = Update::decode_v1(&update) {
                        let doc_guard = self.room_doc.lock().unwrap_or_else(|e| e.into_inner());
                        let mut txn = doc_guard.transact_mut();
                        let _ = txn.apply_update(u);
                    }
                    let _ = self.broadcast_tx.send(PeerMessage::Binary {
                        sender_id: self.client_id,
                        data: Bytes::copy_from_slice(&update),
                    });
                }
            }
        }

        res
    }

    fn has_ui(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        format!("collab-room:{}", self.room_id)
    }
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
pub struct McpQuery {
    pub room: Option<String>,
    pub sessionId: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpDiscoveryResponse {
    pub name: String,
    pub title: String,
    pub version: String,
    pub room: String,
    pub protocol: String,
    pub endpoints: McpEndpoints,
    pub instructions: String,
    pub tools_count: usize,
    pub tools: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct McpEndpoints {
    pub sse: String,
    pub message: String,
    pub rpc: String,
    pub websocket: String,
    pub web_canvas: String,
}

pub async fn mcp_discovery_impl(
    headers: HeaderMap,
    room_id: String,
    query: McpQuery,
    state: AppState,
) -> Response {
    let accept = headers.get("accept").and_then(|h| h.to_str().ok()).unwrap_or("");
    if accept.contains("text/event-stream") {
        return mcp_sse_impl(room_id, query, state).await.into_response();
    }

    if let Err(status) = state.get_or_create_room(&room_id) {
        return status.into_response();
    }

    let tools = tool_definitions();
    let tools_count = tools.len();

    let resp = McpDiscoveryResponse {
        name: "vectorcraft".to_string(),
        title: "VectorCraft Multiplayer Canvas MCP".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        room: room_id.clone(),
        protocol: "Model Context Protocol (2025-06-18, 2026-07-28)".to_string(),
        endpoints: McpEndpoints {
            sse: format!("/mcp/sse?room={room_id}"),
            message: format!("/mcp/message?room={room_id}"),
            rpc: format!("/mcp?room={room_id}"),
            websocket: format!("/ws?room={room_id}"),
            web_canvas: format!("/?room={room_id}"),
        },
        instructions: format!(
            "You are connected to VectorCraft live collaborative room '{room_id}'. \
             Any tool call you make (draw_shape, draw_path, set_paint, transform, set_cursor, etc.) \
             is immediately synced via CRDT into the canvas, and visible live to human users. \
             Use 'render_preview' or 'screenshot' to visually inspect what is currently drawn on the canvas."
        ),
        tools_count,
        tools,
    };

    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, "application/json"),
            (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        serde_json::to_string_pretty(&resp).unwrap_or_default(),
    )
        .into_response()
}

pub async fn mcp_discovery_handler(
    headers: HeaderMap,
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
) -> Response {
    let room_id = query.room.clone().unwrap_or_else(|| "default".to_string());
    mcp_discovery_impl(headers, room_id, query, state).await
}

pub async fn mcp_discovery_room_handler(
    AxumPath(room_id): AxumPath<String>,
    headers: HeaderMap,
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
) -> Response {
    mcp_discovery_impl(headers, room_id, query, state).await
}

pub async fn mcp_sse_impl(
    room_id: String,
    query: McpQuery,
    state: AppState,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let session_id = query
        .sessionId
        .or(query.session_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let (tx, rx) = mpsc::channel::<String>(128);

    {
        let mut sessions = state.mcp_sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.insert(session_id.clone(), tx.clone());
    }

    let endpoint_url = format!("/mcp/message?sessionId={session_id}&room={room_id}");
    let cleanup = Arc::new(SessionCleanup {
        state: state.clone(),
        session_id: session_id.clone(),
    });

    let stream = futures_util::stream::unfold(
        (rx, Some(endpoint_url), cleanup),
        |(mut rx, mut initial, cleanup)| async move {
            if let Some(endpoint) = initial.take() {
                let evt = Event::default().event("endpoint").data(endpoint);
                return Some((Ok(evt), (rx, initial, cleanup)));
            }
            match rx.recv().await {
                Some(msg) => {
                    let evt = Event::default().event("message").data(msg);
                    Some((Ok(evt), (rx, initial, cleanup)))
                }
                None => None,
            }
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

pub async fn mcp_sse_handler(
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let room_id = query.room.clone().unwrap_or_else(|| "default".to_string());
    mcp_sse_impl(room_id, query, state).await
}

pub async fn mcp_sse_room_handler(
    AxumPath(room_id): AxumPath<String>,
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    mcp_sse_impl(room_id, query, state).await
}

pub async fn mcp_message_impl(
    room_id: String,
    query: McpQuery,
    state: AppState,
    body: Bytes,
) -> Response {
    let session_id = query.sessionId.or(query.session_id);

    let room = match state.get_or_create_room(&room_id) {
        Ok(r) => r,
        Err(status) => return status.into_response(),
    };

    let text = match std::str::from_utf8(&body) {
        Ok(t) => t.trim(),
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid utf-8 body").into_response(),
    };

    let (reply, notes) = {
        let mut server_guard = room.mcp_server.lock().unwrap_or_else(|e| e.into_inner());
        let reply = server_guard.handle_line(text);
        let notes = server_guard.take_notifications();
        (reply, notes)
    };

    if let Some(ref sid) = session_id {
        let sessions = state.mcp_sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = sessions.get(sid) {
            if let Some(ref r) = reply {
                let _ = tx.try_send(r.clone());
            }
            for n in notes {
                let _ = tx.try_send(n);
            }
            return (
                StatusCode::ACCEPTED,
                [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
                "Accepted\n",
            )
                .into_response();
        }
    }

    if let Some(r) = reply {
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "application/json"),
                (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            r,
        )
            .into_response()
    } else {
        (
            StatusCode::NO_CONTENT,
            [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
            "",
        )
            .into_response()
    }
}

pub async fn mcp_message_handler(
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    let room_id = query.room.clone().unwrap_or_else(|| "default".to_string());
    mcp_message_impl(room_id, query, state, body).await
}

pub async fn mcp_message_room_handler(
    AxumPath(room_id): AxumPath<String>,
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    mcp_message_impl(room_id, query, state, body).await
}

pub async fn mcp_direct_rpc_impl(
    room_id: String,
    state: AppState,
    body: Bytes,
) -> Response {
    let room = match state.get_or_create_room(&room_id) {
        Ok(r) => r,
        Err(status) => return status.into_response(),
    };

    let text = match std::str::from_utf8(&body) {
        Ok(t) => t.trim(),
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid utf-8 body").into_response(),
    };

    let reply = {
        let mut server_guard = room.mcp_server.lock().unwrap_or_else(|e| e.into_inner());
        server_guard.handle_line(text)
    };

    if let Some(r) = reply {
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "application/json"),
                (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            r,
        )
            .into_response()
    } else {
        (
            StatusCode::NO_CONTENT,
            [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
            "",
        )
            .into_response()
    }
}

pub async fn mcp_direct_rpc_handler(
    Query(query): Query<McpQuery>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    let room_id = query.room.clone().unwrap_or_else(|| "default".to_string());
    mcp_direct_rpc_impl(room_id, state, body).await
}

pub async fn mcp_direct_rpc_room_handler(
    AxumPath(room_id): AxumPath<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    mcp_direct_rpc_impl(room_id, state, body).await
}

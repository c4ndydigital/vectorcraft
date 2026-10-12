//! `vectorcraft-server`: VectorCraft multiplayer collaboration sync backend and static asset server.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use vectorcraft_server::{
    AuthResult, LegalConfig, REJECTED_BY_LIMIT_COUNT, ResourceMetrics, UmamiConfig, check_bearer_auth, inject_umami, read_cgroup_memory,
    read_process_rss, render_datenschutz, render_impressum,
};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// CLI and server configuration options.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    pub dist: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let port = match std::env::var("PORT") {
            Ok(val) => match val.parse::<u16>() {
                Ok(p) if p > 0 => p,
                _ => {
                    eprintln!("[server] warning: invalid PORT='{val}', falling back to 8767");
                    8767
                }
            },
            Err(_) => 8767,
        };
        let bind = std::env::var("BIND")
            .or_else(|_| std::env::var("BIND_ADDR"))
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "0.0.0.0".to_string());
        let dist = std::env::var("DIST_PATH")
            .or_else(|_| std::env::var("DIST"))
            .or_else(|_| std::env::var("DIST_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("dist/web"));
        Self { bind, port, dist }
    }
}

/// Resource limits and ephemerality settings (R12 & R14).
#[derive(Debug, Clone)]
pub struct ServerLimits {
    pub room_grace_seconds: u64,
    pub max_rooms: usize,
    pub max_clients_per_room: usize,
    pub max_message_bytes: usize,
    pub max_room_bytes: usize,
}

impl Default for ServerLimits {
    fn default() -> Self {
        let room_grace_seconds = std::env::var("ROOM_GRACE_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        let max_rooms = std::env::var("MAX_ROOMS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
        let max_clients_per_room = std::env::var("MAX_CLIENTS_PER_ROOM").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
        let max_message_bytes = std::env::var("MAX_MESSAGE_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(10 * 1024 * 1024); // 10 MB
        let max_room_bytes = std::env::var("MAX_ROOM_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(50 * 1024 * 1024); // 50 MB
        Self { room_grace_seconds, max_rooms, max_clients_per_room, max_message_bytes, max_room_bytes }
    }
}

/// Message fanned out across connected collaboration peers.
#[derive(Clone, Debug)]
pub enum PeerMessage {
    Binary { sender_id: u64, data: Bytes },
    Text { sender_id: u64, data: String },
}

/// Shared room state holding the CRDT document, active client counter, blobs, and last empty timestamp.
#[derive(Clone)]
pub struct RoomState {
    pub doc: Arc<Mutex<Doc>>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<PeerMessage>,
    pub active_clients: Arc<AtomicUsize>,
    pub last_empty_at: Arc<Mutex<Option<Instant>>>,
    pub blobs: Arc<Mutex<HashMap<String, (String, Bytes)>>>,
}

impl RoomState {
    pub fn new() -> Self {
        let (broadcast_tx, _rx) = tokio::sync::broadcast::channel(1024);
        Self {
            doc: Arc::new(Mutex::new(Doc::new())),
            broadcast_tx,
            active_clients: Arc::new(AtomicUsize::new(0)),
            last_empty_at: Arc::new(Mutex::new(Some(Instant::now()))),
            blobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn estimated_bytes(&self) -> usize {
        let doc_bytes = {
            let doc_guard = self.doc.lock().unwrap_or_else(|e| e.into_inner());
            doc_guard.transact().encode_state_as_update_v1(&StateVector::default()).len()
        };
        let blob_bytes: usize = {
            let blobs = self.blobs.lock().unwrap_or_else(|e| e.into_inner());
            blobs.values().map(|(_, b)| b.len()).sum()
        };
        doc_bytes + blob_bytes
    }
}

impl Default for RoomState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct WsQuery {
    pub room: Option<String>,
}

/// Application state shared across Axum routes.
#[derive(Clone)]
pub struct AppState {
    pub rooms: Arc<Mutex<HashMap<String, RoomState>>>,
    pub limits: ServerLimits,
    pub dist_path: PathBuf,
    pub active_connections: Arc<AtomicUsize>,
    pub start_time: Instant,
}

impl AppState {
    pub fn new(dist_path: PathBuf) -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            limits: ServerLimits::default(),
            dist_path,
            active_connections: Arc::new(AtomicUsize::new(0)),
            start_time: Instant::now(),
        }
    }

    pub fn get_or_create_room(&self, room_id: &str) -> Result<RoomState, StatusCode> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(room) = rooms.get(room_id) {
            return Ok(room.clone());
        }
        if rooms.len() >= self.limits.max_rooms {
            REJECTED_BY_LIMIT_COUNT.fetch_add(1, Ordering::Relaxed);
            eprintln!("[server] rejection: max rooms limit ({}) reached", self.limits.max_rooms);
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let room = RoomState::new();
        rooms.insert(room_id.to_string(), room.clone());
        Ok(room)
    }

    pub fn collect_metrics(&self) -> ResourceMetrics {
        let (cgroup_used_bytes, cgroup_max_bytes) = read_cgroup_memory();
        let process_rss_bytes = read_process_rss();
        let memory_percent = match (cgroup_used_bytes, cgroup_max_bytes) {
            (Some(used), Some(max)) if max > 0 => (used as f64 / max as f64) * 100.0,
            _ => 0.0,
        };
        let rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        let rooms_count = rooms.len();
        let max_rooms = self.limits.max_rooms;
        let rooms_percent = if max_rooms > 0 { (rooms_count as f64 / max_rooms as f64) * 100.0 } else { 0.0 };
        let active_connections = self.active_connections.load(Ordering::Relaxed);
        let largest_room_bytes = rooms.values().map(|r| r.estimated_bytes()).max().unwrap_or(0);
        let rejected_by_limit_count = REJECTED_BY_LIMIT_COUNT.load(Ordering::Relaxed);
        let uptime_seconds = self.start_time.elapsed().as_secs();

        ResourceMetrics {
            process_rss_bytes,
            cgroup_used_bytes,
            cgroup_max_bytes,
            memory_percent,
            rooms_count,
            max_rooms,
            rooms_percent,
            active_connections,
            largest_room_bytes,
            rejected_by_limit_count,
            uptime_seconds,
        }
    }
}

/// Spawns the background room reaper task that sweeps empty rooms past grace period.
pub fn spawn_reaper_task(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let grace = Duration::from_secs(state.limits.room_grace_seconds);
            let now = Instant::now();
            let mut rooms = state.rooms.lock().unwrap_or_else(|e| e.into_inner());
            rooms.retain(|room_id, room| {
                if room.active_clients.load(Ordering::Relaxed) == 0 {
                    let last_empty = *room.last_empty_at.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(empty_time) = last_empty {
                        if now.duration_since(empty_time) >= grace {
                            eprintln!("[server] room '{room_id}' purged after grace period ({grace:?})");
                            return false;
                        }
                    }
                }
                true
            });
        }
    });
}

fn print_usage() {
    println!(
        "\
VectorCraft Multiplayer Collaboration Server

USAGE:
    vectorcraft-server [OPTIONS]

OPTIONS:
    --bind <ADDR>       Host address to bind (env: BIND / BIND_ADDR / HOST, default: 0.0.0.0)
    --port <PORT>       Port number to listen on (env: PORT, default: 8767)
    --dist <PATH>       Path to static web assets directory (env: DIST_PATH / DIST / DIST_DIR, default: dist/web)
    -h, --help          Print this help message
    -V, --version       Print version information
"
    );
}

fn parse_cli_args(args: &[String]) -> Result<Option<ServerConfig>, String> {
    let mut config = ServerConfig::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            print_usage();
            return Ok(None);
        } else if arg == "-V" || arg == "--version" {
            println!("vectorcraft-server {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        } else if arg == "--bind" {
            i += 1;
            if i >= args.len() {
                return Err("missing value for --bind flag".to_string());
            }
            config.bind = args[i].clone();
        } else if let Some(val) = arg.strip_prefix("--bind=") {
            config.bind = val.to_string();
        } else if arg == "--port" {
            i += 1;
            if i >= args.len() {
                return Err("missing value for --port flag".to_string());
            }
            let parsed_port = args[i].parse::<u16>().map_err(|e| format!("invalid port `{}`: {e}", args[i]))?;
            config.port = parsed_port;
        } else if let Some(val) = arg.strip_prefix("--port=") {
            let parsed_port = val.parse::<u16>().map_err(|e| format!("invalid port `{val}`: {e}"))?;
            config.port = parsed_port;
        } else if arg == "--dist" {
            i += 1;
            if i >= args.len() {
                return Err("missing value for --dist flag".to_string());
            }
            config.dist = PathBuf::from(&args[i]);
        } else if let Some(val) = arg.strip_prefix("--dist=") {
            config.dist = PathBuf::from(val);
        } else {
            return Err(format!("unknown argument `{arg}`. Run with --help for usage."));
        }
        i += 1;
    }
    Ok(Some(config))
}

async fn health_handler() -> &'static str {
    "OK"
}

async fn impressum_handler() -> impl IntoResponse {
    let config = LegalConfig::from_env();
    let html = render_impressum(&config);
    Html(html)
}

async fn datenschutz_handler() -> impl IntoResponse {
    let config = LegalConfig::from_env();
    let umami = UmamiConfig::from_env();
    let html = render_datenschutz(&config, umami.as_ref());
    Html(html)
}

async fn metrics_or_status_handler(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    let auth_header = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok());
    let expected_token = std::env::var("METRICS_TOKEN").ok();
    match check_bearer_auth(auth_header, expected_token.as_deref()) {
        AuthResult::Authorized => {
            let metrics = state.collect_metrics();
            (StatusCode::OK, [(CONTENT_TYPE, "application/json")], serde_json::to_string_pretty(&metrics).unwrap_or_default()).into_response()
        }
        AuthResult::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response(),
        AuthResult::Disabled => (StatusCode::NOT_FOUND, "Metrics endpoint disabled\n").into_response(),
    }
}

async fn upload_blob_handler(
    AxumPath((room_id, hash)): AxumPath<(String, String)>,
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    if body.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.len() > state.limits.max_message_bytes {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let room = state.get_or_create_room(&room_id)?;
    if room.estimated_bytes() + body.len() > state.limits.max_room_bytes {
        return Err(StatusCode::INSUFFICIENT_STORAGE);
    }

    let mime = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("image/png").to_string();

    {
        let mut blobs = room.blobs.lock().unwrap_or_else(|e| e.into_inner());
        blobs.insert(hash.clone(), (mime, body));
    }

    Ok((StatusCode::CREATED, format!("Blob {hash} stored")))
}

async fn get_blob_handler(
    AxumPath((room_id, hash)): AxumPath<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let rooms = state.rooms.lock().unwrap_or_else(|e| e.into_inner());
    let room = rooms.get(&room_id).ok_or(StatusCode::NOT_FOUND)?;

    let blobs = room.blobs.lock().unwrap_or_else(|e| e.into_inner());
    let (mime, bytes) = blobs.get(&hash).cloned().ok_or(StatusCode::NOT_FOUND)?;

    let mut response_headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&mime) {
        response_headers.insert(CONTENT_TYPE, val);
    }
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400, immutable"));

    Ok((StatusCode::OK, response_headers, bytes))
}

async fn index_handler(State(state): State<AppState>) -> impl IntoResponse {
    let index_file = state.dist_path.join("index.html");
    match tokio::fs::read_to_string(&index_file).await {
        Ok(raw_html) => {
            let umami = UmamiConfig::from_env();
            let body = inject_umami(&raw_html, umami.as_ref());
            Html(body).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found\n").into_response(),
    }
}

async fn ws_handler(ws: WebSocketUpgrade, Query(query): Query<WsQuery>, State(state): State<AppState>) -> Result<impl IntoResponse, StatusCode> {
    let room_id = query.room.unwrap_or_else(|| "default".to_string());
    let room = state.get_or_create_room(&room_id)?;

    if room.active_clients.load(Ordering::Relaxed) >= state.limits.max_clients_per_room {
        REJECTED_BY_LIMIT_COUNT.fetch_add(1, Ordering::Relaxed);
        eprintln!("[server] rejection: room '{room_id}' reached max clients ({})", state.limits.max_clients_per_room);
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, room, room_id, state)))
}

async fn handle_socket(socket: WebSocket, room: RoomState, room_id: String, state: AppState) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    room.active_clients.fetch_add(1, Ordering::Relaxed);
    state.active_connections.fetch_add(1, Ordering::Relaxed);
    *room.last_empty_at.lock().unwrap_or_else(|e| e.into_inner()) = None;

    eprintln!("[server] Client {client_id} connected to room '{room_id}'");
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let snapshot_bytes = {
        let doc_guard = match room.doc.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        doc_guard.transact().encode_state_as_update_v1(&StateVector::default())
    };

    if !snapshot_bytes.is_empty()
        && let Err(e) = ws_sender.send(Message::Binary(snapshot_bytes.into())).await
    {
        eprintln!("[server] failed sending initial snapshot to client {client_id}: {e}");
        state.active_connections.fetch_sub(1, Ordering::Relaxed);
        if room.active_clients.fetch_sub(1, Ordering::Relaxed) <= 1 {
            *room.last_empty_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        }
        return;
    }

    let mut broadcast_rx = room.broadcast_tx.subscribe();
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_interval.tick().await;

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if ws_sender.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
                recv_res = broadcast_rx.recv() => {
                    match recv_res {
                        Ok(PeerMessage::Binary { sender_id, data }) if sender_id != client_id => {
                            if ws_sender.send(Message::Binary(data)).await.is_err() {
                                break;
                            }
                        }
                        Ok(PeerMessage::Text { sender_id, data }) if sender_id != client_id => {
                            if ws_sender.send(Message::Text(data.into())).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            eprintln!("[server] broadcast lagged by {skipped} messages for client {client_id}");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            }
        }
    });

    while let Some(msg_result) = ws_receiver.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[server] ws read error from client {client_id}: {e}");
                break;
            }
        };

        match msg {
            Message::Binary(bytes) => {
                if bytes.len() > state.limits.max_message_bytes {
                    eprintln!("[server] message size {} exceeds MAX_MESSAGE_BYTES", bytes.len());
                    REJECTED_BY_LIMIT_COUNT.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                if room.estimated_bytes() + bytes.len() > state.limits.max_room_bytes {
                    eprintln!("[server] room size limit exceeded for room '{room_id}'");
                    REJECTED_BY_LIMIT_COUNT.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                match Update::decode_v1(&bytes) {
                    Ok(update) => {
                        let doc_guard = match room.doc.lock() {
                            Ok(g) => g,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        let mut txn = doc_guard.transact_mut();
                        if let Err(e) = txn.apply_update(update) {
                            eprintln!("[server] error applying update from client {client_id}: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[server] error decoding update from client {client_id}: {e}");
                    }
                }

                let _ = room.broadcast_tx.send(PeerMessage::Binary { sender_id: client_id, data: bytes });
            }
            Message::Text(text) => {
                if text.len() > state.limits.max_message_bytes {
                    eprintln!("[server] text size {} exceeds MAX_MESSAGE_BYTES", text.len());
                    REJECTED_BY_LIMIT_COUNT.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let _ = room.broadcast_tx.send(PeerMessage::Text { sender_id: client_id, data: text.to_string() });
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    send_task.abort();
    state.active_connections.fetch_sub(1, Ordering::Relaxed);
    if room.active_clients.fetch_sub(1, Ordering::Relaxed) <= 1 {
        *room.last_empty_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }
    eprintln!("[server] Client {client_id} disconnected from room '{room_id}'");
}

fn build_csp(umami: Option<&UmamiConfig>) -> String {
    let umami_host = umami.and_then(|u| {
        if let Some(stripped) = u.script_url.strip_prefix("https://") {
            stripped.split('/').next().map(|h| format!("https://{h}"))
        } else if let Some(stripped) = u.script_url.strip_prefix("http://") {
            stripped.split('/').next().map(|h| format!("http://{h}"))
        } else {
            None
        }
    });

    let extra = umami_host.as_deref().unwrap_or("");
    let script_src = if extra.is_empty() {
        "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'".to_string()
    } else {
        format!("script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' {extra}")
    };
    let connect_src = if extra.is_empty() { "connect-src 'self' ws: wss:".to_string() } else { format!("connect-src 'self' ws: wss: {extra}") };

    format!(
        "default-src 'self'; {script_src}; style-src 'self' 'unsafe-inline'; {connect_src}; img-src 'self' data: blob:; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none';"
    )
}

async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    let umami = UmamiConfig::from_env();
    let csp = build_csp(umami.as_ref());

    if let Ok(val) = HeaderValue::from_str(&csp) {
        headers.insert(CONTENT_SECURITY_POLICY, val);
    }
    headers.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    headers.insert(HeaderName::from_static("referrer-policy"), HeaderValue::from_static("strict-origin-when-cross-origin"));
    headers.insert(HeaderName::from_static("permissions-policy"), HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));

    if path == "/"
        || path == "/index.html"
        || path.ends_with(".html")
        || path == "/health"
        || path == "/status"
        || path == "/metrics"
        || path == "/impressum"
        || path == "/datenschutz"
    {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache, must-revalidate"));
    } else if path.ends_with(".wasm") || path.ends_with(".js") || path.ends_with(".css") || path.ends_with(".svg") || path.ends_with(".ico") {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }

    response
}

pub fn build_app(state: AppState, dist_path: &Path) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    let compression = CompressionLayer::new();

    let serve_dir = ServeDir::new(dist_path).append_index_html_on_directories(true).precompressed_br().precompressed_gzip();

    Router::new()
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/health", get(health_handler))
        .route("/impressum", get(impressum_handler))
        .route("/datenschutz", get(datenschutz_handler))
        .route("/status", get(metrics_or_status_handler))
        .route("/metrics", get(metrics_or_status_handler))
        .route("/ws", get(ws_handler))
        .route("/api/rooms/{room_id}/blobs/{hash}", get(get_blob_handler).post(upload_blob_handler))
        .fallback_service(serve_dir)
        .layer(from_fn(security_headers_middleware))
        .layer(compression)
        .layer(cors)
        .with_state(state)
}

fn print_banner(bind: &str, port: u16, dist: &Path) {
    println!("==================================================================");
    println!("  VectorCraft Multiplayer Collaboration Server");
    println!("==================================================================");
    println!("  Local address:   http://{bind}:{port}");
    println!("  WebSocket sync:  ws://{bind}:{port}/ws");
    println!("  Static dist:     {}", dist.display());
    println!("  Health check:    http://{bind}:{port}/health");
    println!("  Impressum:       http://{bind}:{port}/impressum");
    println!("  Datenschutz:     http://{bind}:{port}/datenschutz");
    println!("  Metrics/Status:  http://{bind}:{port}/status (Bearer protected)");
    println!("==================================================================");
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse_cli_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vectorcraft-server: {e}");
            return ExitCode::FAILURE;
        }
    };

    if !config.dist.exists()
        && let Err(e) = std::fs::create_dir_all(&config.dist)
    {
        eprintln!("vectorcraft-server: warning: could not create dist dir `{}`: {e}", config.dist.display());
    }

    let addr_str = format!("{}:{}", config.bind, config.port);
    let socket_addr: SocketAddr = match addr_str.parse() {
        Ok(sa) => sa,
        Err(e) => {
            eprintln!("vectorcraft-server: invalid bind address `{addr_str}`: {e}");
            return ExitCode::FAILURE;
        }
    };

    let state = AppState::new(config.dist.clone());
    spawn_reaper_task(state.clone());

    let app = build_app(state, &config.dist);

    let listener = match tokio::net::TcpListener::bind(socket_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("vectorcraft-server: failed to bind to {socket_addr}: {e}");
            return ExitCode::FAILURE;
        }
    };

    print_banner(&config.bind, config.port, &config.dist);

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("vectorcraft-server: server terminated with error: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args_flags() {
        let args = vec![
            "--bind".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "9050".to_string(),
            "--dist".to_string(),
            "/tmp/assets".to_string(),
        ];
        let cfg = parse_cli_args(&args).expect("valid args").expect("some config");
        assert_eq!(cfg.bind, "127.0.0.1");
        assert_eq!(cfg.port, 9050);
        assert_eq!(cfg.dist, PathBuf::from("/tmp/assets"));
    }

    #[test]
    fn test_parse_cli_args_equals_syntax() {
        let args = vec!["--bind=192.0.2.1".to_string(), "--port=8080".to_string(), "--dist=custom/dist".to_string()];
        let cfg = parse_cli_args(&args).expect("valid args").expect("some config");
        assert_eq!(cfg.bind, "192.0.2.1");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.dist, PathBuf::from("custom/dist"));
    }

    #[test]
    fn test_parse_cli_args_invalid_port_returns_error() {
        let args = vec!["--port".to_string(), "not_a_number".to_string()];
        let res = parse_cli_args(&args);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_cli_args_help_returns_none() {
        let args = vec!["--help".to_string()];
        let res = parse_cli_args(&args).expect("parse ok");
        assert!(res.is_none());
    }

    #[test]
    fn test_parse_cli_args_version_returns_none() {
        let args = vec!["--version".to_string()];
        let res = parse_cli_args(&args).expect("parse ok");
        assert!(res.is_none());
    }
}

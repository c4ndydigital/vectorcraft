//! `vectorcraft-server`: VectorCraft multiplayer collaboration sync backend and static asset server.
//!
//! Provides:
//! - CRDT state synchronization over WebSocket (`/ws`) using `yrs`
//! - Real-time cursor/presence fan-out
//! - Static web asset hosting for the WASM client (`dist/web`)
//! - Liveness/health probe endpoint (`/health`)

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
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

/// Message fanned out across connected collaboration peers.
#[derive(Clone, Debug)]
pub enum PeerMessage {
    /// Binary CRDT update bytes.
    Binary { sender_id: u64, data: Bytes },
    /// JSON text payload (e.g. cursor positions, presence).
    Text { sender_id: u64, data: String },
}

/// Shared room state holding the CRDT document and the broadcast fan-out channel.
#[derive(Clone)]
pub struct RoomState {
    pub doc: Arc<Mutex<Doc>>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<PeerMessage>,
}

impl RoomState {
    pub fn new() -> Self {
        let (broadcast_tx, _rx) = tokio::sync::broadcast::channel(1024);
        Self { doc: Arc::new(Mutex::new(Doc::new())), broadcast_tx }
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

/// Application state shared with Axum route handlers.
#[derive(Clone, Default)]
pub struct AppState {
    pub rooms: Arc<Mutex<HashMap<String, RoomState>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_or_create_room(&self, room_id: &str) -> RoomState {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        rooms.entry(room_id.to_string()).or_default().clone()
    }
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

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let room_id = query.room.unwrap_or_else(|| "default".to_string());
    let room = state.get_or_create_room(&room_id);
    ws.on_upgrade(move |socket| handle_socket(socket, room, room_id))
}

async fn handle_socket(socket: WebSocket, room: RoomState, room_id: String) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    eprintln!("[server] Client {client_id} connected to room '{room_id}'");
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 1. Send initial snapshot of room CRDT state to the freshly connected client
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
        return;
    }

    // 2. Spawn task to forward broadcast messages from other peers to this client
    //    and send 30s ping keep-alive frames to prevent Cloudflare/reverse proxy timeouts.
    let mut broadcast_rx = room.broadcast_tx.subscribe();
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume immediate first tick so ping is sent after 30s of connection
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

    // 3. Receive incoming messages from this client and broadcast to others
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
                // Decode update and apply to room's yrs::Doc
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

                // Broadcast binary frame to all other connected peers
                let _ = room.broadcast_tx.send(PeerMessage::Binary { sender_id: client_id, data: bytes });
            }
            Message::Text(text) => {
                // Fan out cursor or presence text directly to all other peers
                let _ = room.broadcast_tx.send(PeerMessage::Text { sender_id: client_id, data: text.to_string() });
            }
            Message::Close(_) => {
                break;
            }
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    send_task.abort();
}

pub fn build_app(state: AppState, dist_path: &Path) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    let serve_dir = ServeDir::new(dist_path).append_index_html_on_directories(true);

    Router::new()
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .fallback_service(serve_dir)
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
        eprintln!(
            "vectorcraft-server: warning: could not create dist dir `{}`: {e}",
            config.dist.display()
        );
    }

    let addr_str = format!("{}:{}", config.bind, config.port);
    let socket_addr: SocketAddr = match addr_str.parse() {
        Ok(sa) => sa,
        Err(e) => {
            eprintln!("vectorcraft-server: invalid bind address `{addr_str}`: {e}");
            return ExitCode::FAILURE;
        }
    };

    let state = AppState::new();

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
        let args = vec![
            "--bind=192.0.2.1".to_string(),
            "--port=8080".to_string(),
            "--dist=custom/dist".to_string(),
        ];
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

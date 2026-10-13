use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use vectorcraft_server::FontCdnConfig;
use yrs::{Map, Transact};

// Mirror main.rs struct for test harness
#[path = "../src/main.rs"]
mod server_main;

static TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn create_test_app() -> (axum::Router, server_main::AppState, std::path::PathBuf) {
    let id = TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_dist = std::env::temp_dir().join(format!("test-dist-mcp-{}-{}", std::process::id(), id));
    let temp_fonts = temp_dist.join("fonts");
    let _ = std::fs::remove_dir_all(&temp_dist);
    std::fs::create_dir_all(&temp_fonts).unwrap();

    // Create a mock index.html for index_handler fallback
    std::fs::write(
        temp_dist.join("index.html"),
        "<!DOCTYPE html><html><head><title>VectorCraft</title></head><body>Canvas</body></html>",
    )
    .unwrap();

    let state = server_main::AppState {
        rooms: Arc::new(Mutex::new(std::collections::HashMap::new())),
        limits: server_main::ServerLimits::default(),
        dist_path: temp_dist.clone(),
        fonts_config: FontCdnConfig {
            enabled: true,
            cache_dir: temp_fonts.clone(),
            upstream_base: "https://cdn.jsdelivr.net/fontsource/fonts".to_string(),
        },
        active_connections: Arc::new(AtomicUsize::new(0)),
        start_time: Instant::now(),
        mcp_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };

    let app = server_main::build_app(state.clone(), &temp_dist);
    (app, state, temp_dist)
}

#[tokio::test]
async fn test_mcp_discovery_endpoint() {
    let (app, _state, temp_dist) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/mcp?room=test-discovery-room")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(
        response.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["name"], "vectorcraft");
    assert_eq!(json["room"], "test-discovery-room");
    assert!(json["tools"].as_array().unwrap().len() > 10);
    assert!(json["endpoints"]["sse"].as_str().unwrap().contains("/mcp/sse?room=test-discovery-room"));
    assert!(json["endpoints"]["message"].as_str().unwrap().contains("/mcp/message?room=test-discovery-room"));

    let _ = std::fs::remove_dir_all(&temp_dist);
}

#[tokio::test]
async fn test_mcp_direct_rpc_tools_list() {
    let (app, _state, temp_dist) = create_test_app();

    let rpc_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp?room=test-tools-room")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&rpc_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["id"], 1);
    let tools = json["result"]["tools"].as_array().expect("tools array in result");
    assert!(tools.iter().any(|t| t["name"] == "draw_shape"));
    assert!(tools.iter().any(|t| t["name"] == "draw_path"));
    assert!(tools.iter().any(|t| t["name"] == "set_paint"));

    let _ = std::fs::remove_dir_all(&temp_dist);
}

#[tokio::test]
async fn test_mcp_draw_shape_syncs_to_room_crdt() {
    let (app, state, temp_dist) = create_test_app();

    let room_id = "test-draw-sync";

    // 1. Send draw_shape request for a red rectangle
    let draw_req = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/call",
        "params": {
            "name": "draw_shape",
            "arguments": {
                "shape": "rectangle",
                "x": 100.0,
                "y": 150.0,
                "width": 250.0,
                "height": 200.0,
                "fill": "#ff0000"
            }
        }
    });

    eprintln!(">>> [test] Step 1 start: sending draw_shape request");
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mcp?room={room_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&draw_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    eprintln!(">>> [test] Step 1 response status: {:?}", response.status());
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    eprintln!(">>> [test] Step 1 response json: {:?}", json);
    assert_eq!(json["id"], 42);
    assert!(json["error"].is_null(), "RPC error: {:?}", json["error"]);

    // 2. Verify CRDT doc in RoomState has received the update
    eprintln!(">>> [test] Step 2: checking room doc");
    {
        let rooms = state.rooms.lock().unwrap();
        let room = rooms.get(room_id).expect("room was created");
        let doc = room.doc.lock().unwrap();
        let shapes = doc.get_or_insert_map("shapes");
        let txn = doc.transact();
        eprintln!(">>> [test] Step 2: shapes len = {}", shapes.len(&txn));
        assert_eq!(shapes.len(&txn), 1, "Element must be synced into room CRDT doc shapes map");
    }

    // 3. Inspect via MCP tool doc_inspect
    eprintln!(">>> [test] Step 3: sending doc_inspect request");
    let inspect_req = json!({
        "jsonrpc": "2.0",
        "id": 43,
        "method": "tools/call",
        "params": {
            "name": "doc_inspect",
            "arguments": {}
        }
    });

    let inspect_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mcp?room={room_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&inspect_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    eprintln!(">>> [test] Step 3 response status: {:?}", inspect_resp.status());
    assert_eq!(inspect_resp.status(), StatusCode::OK);
    let inspect_body = inspect_resp.into_body().collect().await.unwrap().to_bytes();
    let inspect_json: serde_json::Value = serde_json::from_slice(&inspect_body).unwrap();
    eprintln!(">>> [test] Step 3 response json: {:?}", inspect_json);
    let text_content = inspect_json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text_content.contains("Rectangle") || text_content.contains("layers") || text_content.contains("artboards"));

    let _ = std::fs::remove_dir_all(&temp_dist);
    eprintln!(">>> [test] Completed successfully");
}

#[tokio::test]
async fn test_index_content_negotiation_for_mcp() {
    let (app, _state, temp_dist) = create_test_app();

    // 1. Accept: application/json returns MCP discovery
    let json_req = app.clone()
        .oneshot(
            Request::builder()
                .uri("/?room=c4ndy-1404")
                .header("Accept", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(json_req.status(), StatusCode::OK);
    assert_eq!(
        json_req.headers().get("content-type").unwrap(),
        "application/json"
    );
    let body = json_req.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "vectorcraft");
    assert_eq!(json["room"], "c4ndy-1404");

    // 2. Accept: text/html injects discovery headers & meta links
    let html_req = app
        .oneshot(
            Request::builder()
                .uri("/?room=c4ndy-1404")
                .header("Accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(html_req.status(), StatusCode::OK);
    assert_eq!(
        html_req.headers().get("X-MCP-Endpoint").unwrap(),
        "/mcp?room=c4ndy-1404"
    );
    let html_body = String::from_utf8(html_req.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html_body.contains(r#"rel="mcp-server""#));
    assert!(html_body.contains(r#"href="/mcp?room=c4ndy-1404""#));
    assert!(html_body.contains(r#"name="mcp-endpoint""#));
    assert!(html_body.contains(r#"content="/mcp?room=c4ndy-1404""#));

    let _ = std::fs::remove_dir_all(&temp_dist);
}

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vectorcraft_server::FontCdnConfig;

// Mirror main.rs struct for test harness
#[path = "../src/main.rs"]
mod server_main;

#[tokio::test]
async fn test_font_status_endpoint() {
    let temp_dist = std::env::temp_dir().join(format!("test-dist-status-{}", std::process::id()));
    let temp_fonts = temp_dist.join("fonts");
    let _ = std::fs::remove_dir_all(&temp_dist);
    std::fs::create_dir_all(&temp_fonts).unwrap();

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

    let app = server_main::build_app(state, &temp_dist);

    let response = app
        .oneshot(Request::builder().uri("/api/fonts/status").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["enabled"], true);
    assert_eq!(json["dsgvo_compliant"], true);
    assert_eq!(json["cached_fonts_count"], 0);

    let _ = std::fs::remove_dir_all(&temp_dist);
}

#[tokio::test]
async fn test_font_cache_hit_and_headers() {
    let temp_dist = std::env::temp_dir().join(format!("test-dist-hit-{}", std::process::id()));
    let temp_fonts = temp_dist.join("fonts");
    let _ = std::fs::remove_dir_all(&temp_dist);
    std::fs::create_dir_all(temp_fonts.join("inter")).unwrap();

    let font_bytes = b"\x00\x01\x00\x00mock-ttf-data";
    std::fs::write(temp_fonts.join("inter/inter-latin-400.ttf"), font_bytes).unwrap();

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

    let app = server_main::build_app(state, &temp_dist);

    let response = app
        .oneshot(Request::builder().uri("/fonts/inter/inter-latin-400.ttf").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "font/ttf");
    assert_eq!(response.headers().get("x-cache").unwrap(), "HIT");
    assert_eq!(response.headers().get("cache-control").unwrap(), "public, max-age=31536000, immutable");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], font_bytes);

    let _ = std::fs::remove_dir_all(&temp_dist);
}

#[tokio::test]
async fn test_font_path_traversal_rejection() {
    let temp_dist = std::env::temp_dir().join(format!("test-dist-trav-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dist);
    std::fs::create_dir_all(&temp_dist).unwrap();

    let state = server_main::AppState {
        rooms: Arc::new(Mutex::new(std::collections::HashMap::new())),
        limits: server_main::ServerLimits::default(),
        dist_path: temp_dist.clone(),
        fonts_config: FontCdnConfig {
            enabled: true,
            cache_dir: temp_dist.join("fonts"),
            upstream_base: "https://cdn.jsdelivr.net/fontsource/fonts".to_string(),
        },
        active_connections: Arc::new(AtomicUsize::new(0)),
        start_time: Instant::now(),
        mcp_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };

    let app = server_main::build_app(state, &temp_dist);

    let response = app
        .oneshot(Request::builder().uri("/fonts/../../etc/passwd").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // Either 400 Bad Request or 404 from routing layer, never 200
    assert!(response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&temp_dist);
}

#[tokio::test]
async fn test_font_css2_on_demand_ttf_fetch() {
    let temp_dist = std::env::temp_dir().join(format!("test-dist-css2-{}", std::process::id()));
    let temp_fonts = temp_dist.join("fonts");
    let _ = std::fs::remove_dir_all(&temp_dist);
    std::fs::create_dir_all(&temp_fonts).unwrap();

    let state = server_main::AppState {
        rooms: Arc::new(Mutex::new(std::collections::HashMap::new())),
        limits: server_main::ServerLimits::default(),
        dist_path: temp_dist.clone(),
        fonts_config: FontCdnConfig {
            enabled: true,
            cache_dir: temp_fonts.clone(),
            upstream_base: "https://cdn.jsdelivr.net/gh/google/fonts@main".to_string(),
        },
        active_connections: Arc::new(AtomicUsize::new(0)),
        start_time: Instant::now(),
        mcp_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };

    let app = server_main::build_app(state, &temp_dist);

    let response = app
        .oneshot(Request::builder().uri("/fonts/css2/Roboto+Slab/Regular.ttf").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "font/ttf");
    assert_eq!(response.headers().get("x-cache").unwrap(), "MISS");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(body.len() > 10_000, "downloaded TrueType font should have substantial size");
    // TrueType sfnt version 0x00010000
    assert_eq!(&body[..4], &[0x00, 0x01, 0x00, 0x00], "must be a valid uncompressed TrueType font");

    let _ = std::fs::remove_dir_all(&temp_dist);
}


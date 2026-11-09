use std::sync::Arc;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt;

use airlinkd::config::Config;
use airlinkd::fs_handler::FsManager;
use airlinkd::stats::StatsManager;
use airlinkd::routes::core::{AppState, core_routes};
use airlinkd::security::security_middleware;
use airlinkd::storage::load_meta_config;

// We will test basic auth, hmac, and routing
#[test]
fn test_meta_config_loads() {
    let meta = load_meta_config();
    assert_eq!(meta.meta.version, "1.0.1");
    assert_eq!(meta.meta.codename, "Glazzer Fridge");
}

#[tokio::test]
async fn test_root_route_basic_auth() {
    let cfg = Arc::new(Config {
        remote: "127.0.0.1".to_string(),
        key: "testsecretkey".to_string(),
        port: 3002,
        debug: false,
        environment: "test".to_string(),
        version: "1.0.0".to_string(),
        stats_interval: 10000,
        require_hmac: false,
        allowed_ips: vec![],
    });

    let stats = StatsManager::new();
    let fs = Arc::new(FsManager::new());
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let event_bus = airlinkd::event_bus::EventBus::new();
    let state_mgr = airlinkd::container_state::ContainerStateManager::new(docker.clone());
    let container_mgr = airlinkd::container_manager::ContainerManager::new(docker.clone(), event_bus.clone(), state_mgr);
    let sftp_mgr = airlinkd::sftp_manager::SftpManager::new(docker, "127.0.0.1".to_string());

    let state = Arc::new(AppState {
        config: cfg.clone(),
        stats,
        fs,
        container_mgr,
        event_bus,
        sftp_mgr,
    });

    let app = axum::Router::new()
        .merge(core_routes())
        .layer(axum::middleware::from_fn_with_state(cfg.clone(), security_middleware))
        .with_state(state);

    // Request without auth should fail
    let req = Request::builder()
        .uri("/")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Request with correct Basic Auth should succeed
    let auth_str = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, "Airlink:testsecretkey");
    let req = Request::builder()
        .uri("/")
        .method(Method::GET)
        .header(header::AUTHORIZATION, format!("Basic {}", auth_str))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

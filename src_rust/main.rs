pub mod config;
pub mod container_manager;
pub mod container_state;
pub mod event_bus;
pub mod fs_handler;
pub mod install_logs;
pub mod logger;
pub mod minecraft_ping;
pub mod radar_handler;
pub mod routes;
pub mod security;
pub mod sftp_manager;
pub mod stats;
pub mod storage;
pub mod validation;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use axum::{middleware, Router};
use bollard::Docker;
use routes::core::AppState;

#[tokio::main]
async fn main() {
    let cfg = Arc::new(config::Config::load());
    logger::init_logger();
    let stats_mgr = stats::StatsManager::new();
    stats_mgr.clone().start_collector(cfg.stats_interval);
    let fs_mgr = Arc::new(fs_handler::FsManager::new());

    let docker = Docker::connect_with_unix_defaults().unwrap_or_else(|_| {
        Docker::connect_with_local_defaults().expect("Failed to connect to docker")
    });

    let event_bus = event_bus::EventBus::new();
    let state_mgr = container_state::ContainerStateManager::new(docker.clone());
    state_mgr.clone().init().await;

    let container_mgr = container_manager::ContainerManager::new(
        docker.clone(),
        event_bus.clone(),
        state_mgr.clone(),
    );

    let sftp_mgr = sftp_manager::SftpManager::new(docker.clone(), cfg.remote.clone());

    let state = Arc::new(AppState {
        config: cfg.clone(),
        stats: stats_mgr,
        fs: fs_mgr,
        container_mgr,
        event_bus,
        sftp_mgr,
    });

    // Unprotected or WebSocket routes that do their own WebSocket authentication
    let ws_routes = routes::ws::websocket_routes().with_state(state.clone());

    // Protected HTTP routes with Basic Auth and HMAC verification
    let protected_routes = Router::new()
        .merge(routes::core::core_routes())
        .merge(routes::file_system::fs_routes())
        .merge(routes::instances::container_routes())
        .merge(routes::extra::sftp_routes())
        .merge(routes::extra::radar_routes())
        .merge(routes::extra::minecraft_routes())
        .layer(middleware::from_fn_with_state(cfg.clone(), security::security_middleware))
        .with_state(state.clone());

    let app = Router::new()
        .merge(ws_routes)
        .merge(protected_routes);

    let addr = format!("0.0.0.0:{}", cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("Daemon running on {}", addr);
    axum::serve(listener, app).await.unwrap();
}

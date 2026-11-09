use std::sync::Arc;
use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};
use serde_json::{json, Value};

use crate::config::Config;
use crate::container_manager::ContainerManager;
use crate::event_bus::EventBus;
use crate::fs_handler::FsManager;
use crate::sftp_manager::SftpManager;
use crate::stats::StatsManager;
use crate::storage::load_meta_config;

pub struct AppState {
    pub config: Arc<Config>,
    pub stats: Arc<StatsManager>,
    pub fs: Arc<FsManager>,
    pub container_mgr: Arc<ContainerManager>,
    pub event_bus: Arc<EventBus>,
    pub sftp_mgr: Arc<SftpManager>,
}

pub fn core_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(root_handler))
        .route("/stats", get(stats_handler))
}

async fn root_handler(State(state): State<Arc<AppState>>) -> Json<Value> {
    let meta = load_meta_config();
    Json(json!({
        "versionFamily": 1,
        "versionRelease": format!("Airlinkd {}", meta.meta.version),
        "status": "Online",
        "remote": state.config.remote,
    }))
}

async fn stats_handler(State(state): State<Arc<AppState>>) -> Json<Value> {
    let total_stats = state.stats.get_total_stats();
    let uptime = state.stats.get_uptime_formatted();

    Json(json!({
        "totalStats": total_stats,
        "uptime": uptime,
    }))
}

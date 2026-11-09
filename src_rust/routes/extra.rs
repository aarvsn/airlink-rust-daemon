use std::sync::Arc;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::minecraft_ping::ping_minecraft_server;
use crate::radar_handler::{scan_volume, zip_scan_volume, RadarScript};
use crate::routes::core::AppState;
use crate::validation::validate_container_id;

#[derive(Deserialize)]
pub struct SftpCredBody {
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct RadarScanBody {
    pub id: Option<String>,
    pub script: Option<RadarScript>,
}

#[derive(Deserialize)]
pub struct RadarZipBody {
    pub id: Option<String>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    #[serde(rename = "maxFileSizeMb")]
    pub max_file_size_mb: Option<u64>,
}

#[derive(Deserialize)]
pub struct MinecraftPlayersQuery {
    pub id: Option<String>,
    pub host: Option<String>,
    pub port: Option<String>,
}

pub fn sftp_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sftp/credentials", post(sftp_create_handler).delete(sftp_delete_handler))
        .route("/sftp/status", get(sftp_status_handler))
}

pub fn radar_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/radar/scan", post(radar_scan_handler))
        .route("/radar/zip", post(radar_zip_handler))
}

pub fn minecraft_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/minecraft/players", get(minecraft_players_handler))
}

async fn sftp_create_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SftpCredBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;

    match state.sftp_mgr.generate_credential(&id).await {
        Ok(cred) => Ok(Json(json!({
            "username": cred.username,
            "password": cred.password,
            "host": cred.host,
            "port": cred.port,
            "expiresAt": cred.expires_at,
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn sftp_delete_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SftpCredBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;

    match state.sftp_mgr.revoke_for_container(&id).await {
        Ok(_) => Ok(Json(json!({ "message": "SFTP credentials revoked." }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn sftp_status_handler(
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    Json(json!({ "activeSessions": state.sftp_mgr.get_active_count() }))
}

async fn radar_scan_handler(
    Json(body): Json<RadarScanBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and script are required." }))))?;
    let script = body.script.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and script are required." }))))?;

    match scan_volume(&id, script) {
        Ok(results) => Ok(Json(json!({
            "success": true,
            "message": format!("Scan completed for container {}", id),
            "results": results,
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
            "success": false,
            "error": format!("Failed to scan container: {}", e),
        })))),
    }
}

async fn radar_zip_handler(
    Json(body): Json<RadarZipBody>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;

    if !validate_container_id(&id) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid container ID format." }))));
    }

    match zip_scan_volume(&id, body.include, body.exclude, body.max_file_size_mb) {
        Ok(bytes) => {
            let mut response = Response::new(axum::body::Body::from(bytes.clone()));
            response.headers_mut().insert(header::CONTENT_TYPE, "application/zip".parse().unwrap());
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"scan-{}.zip\"", id).parse().unwrap(),
            );
            response.headers_mut().insert(header::CONTENT_LENGTH, bytes.len().to_string().parse().unwrap());
            Ok(response)
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "success": false, "error": format!("Failed to zip container: {}", e) })))),
    }
}

async fn minecraft_players_handler(
    Query(q): Query<MinecraftPlayersQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let empty_resp = json!({
        "players": [],
        "maxPlayers": 0,
        "onlinePlayers": 0,
        "description": "",
        "version": "",
        "online": false,
    });

    let _id = match q.id {
        Some(i) => i,
        None => {
            let mut val = empty_resp.clone();
            val["error"] = json!("Container ID, host, and port are required.");
            return Err((StatusCode::BAD_REQUEST, Json(val)));
        }
    };
    let host = match q.host {
        Some(h) => h,
        None => {
            let mut val = empty_resp.clone();
            val["error"] = json!("Container ID, host, and port are required.");
            return Err((StatusCode::BAD_REQUEST, Json(val)));
        }
    };
    let port_str = match q.port {
        Some(p) => p,
        None => {
            let mut val = empty_resp.clone();
            val["error"] = json!("Container ID, host, and port are required.");
            return Err((StatusCode::BAD_REQUEST, Json(val)));
        }
    };

    let port: u16 = match port_str.parse() {
        Ok(p) => p,
        Err(_) => {
            let mut val = empty_resp.clone();
            val["error"] = json!("Port must be a valid number.");
            return Err((StatusCode::BAD_REQUEST, Json(val)));
        }
    };

    match ping_minecraft_server(&host, port, 5000).await {
        Ok(res) => Ok(Json(json!(res))),
        Err(_) => Ok(Json(empty_resp)),
    }
}

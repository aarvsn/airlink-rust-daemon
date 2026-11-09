use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::install_logs::{get_server_state, set_server_state};
use crate::routes::core::AppState;

#[derive(Deserialize)]
pub struct ContainerInstallerBody {
    pub id: Option<String>,
    pub script: Option<String>,
    pub container: Option<String>,
    pub entrypoint: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
pub struct ContainerInstallBody {
    pub id: Option<String>,
    pub image: Option<String>,
    pub scripts: Option<Vec<Value>>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
pub struct ContainerStartBody {
    pub id: Option<String>,
    pub image: Option<String>,
    pub ports: Option<String>,
    pub env: Option<HashMap<String, String>>,
    #[serde(rename = "Memory", default)]
    pub memory: u64,
    #[serde(rename = "Cpu", default)]
    pub cpu: u64,
    #[serde(rename = "StartCommand")]
    pub start_command: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct ContainerStopBody {
    pub id: Option<String>,
    pub stopCmd: Option<String>,
}

#[derive(Deserialize)]
pub struct ContainerKillBody {
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct ContainerDeleteBody {
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct ContainerCommandBody {
    pub id: Option<String>,
    pub command: Option<String>,
}

#[derive(Deserialize)]
pub struct ContainerQuery {
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct BackupCreateBody {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct BackupRestoreBody {
    pub id: Option<String>,
    pub backupPath: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct BackupDeleteBody {
    pub backupPath: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct BackupDownloadQuery {
    pub backupPath: Option<String>,
}

pub fn container_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/container/installer", post(installer_handler))
        .route("/container/install", post(install_handler))
        .route("/container/status/{id}", get(status_id_handler))
        .route("/container/start", post(start_handler))
        .route("/container/stop", post(stop_handler))
        .route("/container/kill", delete(kill_handler))
        .route("/container/command", post(command_handler))
        .route("/container", delete(delete_handler))
        .route("/container/status", get(status_query_handler))
        .route("/container/stats", get(stats_query_handler))
        .route("/container/backup", post(backup_create_handler).delete(backup_delete_handler))
        .route("/container/restore", post(backup_restore_handler))
        .route("/container/backup/download", get(backup_download_handler))
}

async fn installer_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerInstallerBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    let script = body.script.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Script and Container are required." }))))?;
    let container = body.container.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Script and Container are required." }))))?;
    let entrypoint = body.entrypoint.unwrap_or_else(|| "bash".to_string());
    let env = body.env.unwrap_or_default();

    state.container_mgr.init_container_volume(&id);
    set_server_state(&id, "installing");

    match state.container_mgr.create_installer(&id, &container, &script, env, &entrypoint).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Container {} installed successfully.", id) }))),
        Err(e) => {
            set_server_state(&id, "failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to install container {}: {}", id, e) }))))
        }
    }
}

async fn install_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerInstallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    let env = body.env.unwrap_or_default();

    set_server_state(&id, "installing");
    state.container_mgr.init_container_volume(&id);

    if let Some(ref image) = body.image {
        let _ = state.container_mgr.pull_image_if_needed(&id, image).await;
    }

    if let Some(scripts) = body.scripts {
        for script_val in scripts {
            let url = script_val.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let file_name = script_val.get("fileName").and_then(|f| f.as_str()).unwrap_or("");
            if url.is_empty() || file_name.is_empty() {
                continue;
            }

            let mut resolved_url = url.to_string();
            let re = regex::Regex::new(r"\$ALVKT\((\w+)\)").unwrap();
            resolved_url = re.replace_all(&resolved_url, |caps: &regex::Captures| {
                env.get(&caps[1]).cloned().unwrap_or_default()
            }).to_string();

            if let Ok(resp) = reqwest::get(&resolved_url).await {
                if let Ok(bytes) = resp.bytes().await {
                    let mut file_content = bytes.to_vec();
                    if script_val.get("ALVKT").and_then(|b| b.as_bool()).unwrap_or(false) {
                        let text = String::from_utf8_lossy(&file_content).to_string();
                        let replaced = re.replace_all(&text, |caps: &regex::Captures| {
                            env.get(&caps[1]).cloned().unwrap_or_default()
                        }).to_string();
                        file_content = replaced.into_bytes();
                    }
                    let _ = state.fs.write_file_content(&id, file_name, &file_content);
                }
            }
        }
    }

    set_server_state(&id, "installed");
    Ok(Json(json!({ "message": format!("Container {} installed successfully.", id) })))
}

async fn status_id_handler(
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some(s) = get_server_state(&id) {
        Ok(Json(json!({ "containerId": id, "state": s })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(json!({ "message": format!("No install state found for container {}.", id) }))))
    }
}

async fn start_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerStartBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and Image are required." }))))?;
    let image = body.image.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and Image are required." }))))?;
    let mut env = body.env.unwrap_or_default();
    let ports = body.ports.unwrap_or_default();

    if let Some(cmd) = body.start_command {
        let re_braces = regex::Regex::new(r"\{\{(\w+)\}\}").unwrap();
        let re_alvkt = regex::Regex::new(r"\$ALVKT\((\w+)\)").unwrap();
        let mut updated = re_braces.replace_all(&cmd, |caps: &regex::Captures| {
            env.get(&caps[1]).cloned().unwrap_or_default()
        }).to_string();
        updated = re_alvkt.replace_all(&updated, |caps: &regex::Captures| {
            env.get(&caps[1]).cloned().unwrap_or_default()
        }).to_string();

        env.insert("START".to_string(), updated.clone());
        env.insert("STARTUP".to_string(), updated);
    }

    match state.container_mgr.start_container(&id, &image, env, &ports, body.memory, body.cpu).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Container {} started successfully.", id) }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to start container {}: {}", id, e) })))),
    }
}

async fn stop_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerStopBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    match state.container_mgr.stop_container(&id, body.stopCmd.as_deref()).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Container {} stopped successfully.", id) }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to stop container {}: {}", id, e) })))),
    }
}

async fn kill_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerKillBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    match state.container_mgr.kill_container(&id).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Container {} killed successfully.", id) }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to kill container {}: {}", id, e) })))),
    }
}

async fn command_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerCommandBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and Command are required." }))))?;
    let command = body.command.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID and Command are required." }))))?;

    match state.container_mgr.send_command(&id, &command).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Command sent to container {}: {}", id, command) }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to send command to container {}: {}", id, e) })))),
    }
}

async fn delete_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ContainerDeleteBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    match state.container_mgr.delete_container_and_volume(&id).await {
        Ok(_) => Ok(Json(json!({ "message": format!("Container {} deleted successfully.", id) }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to delete container {}: {}", id, e) })))),
    }
}

async fn status_query_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ContainerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;

    if let Some(running) = state.container_mgr.state_mgr.is_running(&id) {
        return Ok(Json(json!({
            "running": running,
            "exists": true,
            "source": "cache",
        })));
    }

    if let Ok(info) = state.container_mgr.docker.inspect_container(&id, None).await {
        let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
        let status = info.state.as_ref().and_then(|s| s.status.map(|st| format!("{:?}", st).to_lowercase())).unwrap_or_default();
        let started_at = info.state.as_ref().and_then(|s| s.started_at.clone());
        let finished_at = info.state.as_ref().and_then(|s| s.finished_at.clone());

        Ok(Json(json!({
            "running": running,
            "exists": true,
            "status": status,
            "startedAt": started_at,
            "finishedAt": finished_at,
            "source": "inspect",
        })))
    } else {
        Ok(Json(json!({ "running": false, "exists": false })))
    }
}

async fn stats_query_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ContainerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;

    if let Some(stats) = state.container_mgr.get_container_stats(&id).await {
        Ok(Json(stats))
    } else {
        Ok(Json(json!({ "running": false, "exists": false })))
    }
}

async fn backup_create_handler(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<BackupCreateBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    let name = body.name.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Backup name is required." }))))?;

    let volume_path = PathBuf::from("volumes").join(&id);
    if !volume_path.exists() {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": "Container volume not found." }))));
    }

    let backups_dir = PathBuf::from("backups").join(&id);
    let _ = fs::create_dir_all(&backups_dir);

    let backup_uuid = Uuid::new_v4().to_string();
    let backup_file_name = format!("{}.tar.gz", backup_uuid);
    let backup_path = backups_dir.join(&backup_file_name);

    let tar_gz = File::create(&backup_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);

    tar.append_dir_all(".", &volume_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
    let enc = tar.into_inner().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
    enc.finish().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    let size = fs::metadata(&backup_path).map(|m| m.len()).unwrap_or(0);

    Ok(Json(json!({
        "success": true,
        "message": "Backup created successfully",
        "backup": {
            "uuid": backup_uuid,
            "name": name,
            "filePath": format!("backups/{}/{}", id, backup_file_name),
            "size": size,
            "createdAt": chrono::Utc::now().to_rfc3339(),
        }
    })))
}

async fn backup_restore_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BackupRestoreBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Container ID is required." }))))?;
    let backup_path_str = body.backupPath.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Backup path is required." }))))?;

    let allowed_dir = PathBuf::from("backups").join(&id).canonicalize().map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))))?;
    let full_backup = PathBuf::from(&backup_path_str).canonicalize().map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))))?;

    if !full_backup.starts_with(&allowed_dir) || !full_backup.is_file() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))));
    }

    let _ = state.container_mgr.stop_container(&id, None).await;

    let volume_path = PathBuf::from("volumes").join(&id);
    if volume_path.exists() {
        let _ = fs::remove_dir_all(&volume_path);
    }
    let _ = fs::create_dir_all(&volume_path);

    let tar_gz = File::open(&full_backup).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
    let tar = GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(tar);
    archive.unpack(&volume_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({
        "success": true,
        "message": "Backup restored successfully",
    })))
}

async fn backup_delete_handler(
    Json(body): Json<BackupDeleteBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path_str = body.backupPath.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Backup path is required." }))))?;

    let allowed_root = PathBuf::from("backups").canonicalize().map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))))?;
    let full_path = PathBuf::from(&path_str).canonicalize().map_err(|_| (StatusCode::NOT_FOUND, Json(json!({ "error": "Backup file not found." }))))?;

    if !full_path.starts_with(&allowed_root) || !full_path.is_file() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))));
    }

    fs::remove_file(&full_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;

    Ok(Json(json!({
        "success": true,
        "message": "Backup deleted successfully",
    })))
}

async fn backup_download_handler(
    Query(q): Query<BackupDownloadQuery>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let path_str = q.backupPath.ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Backup path is required." }))))?;

    let allowed_root = PathBuf::from("backups").canonicalize().map_err(|_| (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))))?;
    let full_path = PathBuf::from(&path_str).canonicalize().map_err(|_| (StatusCode::NOT_FOUND, Json(json!({ "error": "Backup file not found." }))))?;

    if !full_path.starts_with(&allowed_root) || !full_path.is_file() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid backup path." }))));
    }

    let file = tokio::fs::File::open(&full_path).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?;
    let file_name = full_path.file_name().and_then(|n| n.to_str()).unwrap_or("backup.tar.gz");

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/gzip".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", file_name).parse().unwrap(),
    );

    Ok(response)
}

use std::sync::Arc;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::io::ReaderStream;

use crate::routes::core::AppState;
use crate::validation::{validate_container_id, validate_file_name, validate_path};

#[derive(Deserialize)]
pub struct FsListQuery {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub filter: Option<String>,
}

fn default_path() -> String {
    "/".to_string()
}

#[derive(Deserialize)]
pub struct FsSizeQuery {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
}

#[derive(Deserialize)]
pub struct FsInfoQuery {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
}

#[derive(Deserialize)]
pub struct FsContentQuery {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
}

#[derive(Deserialize)]
pub struct FsContentPost {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub content: Option<Value>,
}

#[derive(Deserialize)]
pub struct FsDownloadQuery {
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct FsRmBody {
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct FsZipBody {
    pub id: Option<String>,
    pub files: Option<Value>,
    pub zipname: Option<String>,
}

#[derive(Deserialize)]
pub struct FsUnzipBody {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub zipname: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct FsRenameBody {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub newName: Option<String>,
    pub newPath: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct FsUploadBody {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub fileName: Option<String>,
    pub fileContent: Option<Value>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct FsCreateEmptyFileBody {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub fileName: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct FsAppendFileBody {
    pub id: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    pub fileName: Option<String>,
    pub fileContent: Option<Value>,
    #[serde(default)]
    pub chunkIndex: usize,
    #[serde(default = "default_total_chunks")]
    pub totalChunks: usize,
}

fn default_total_chunks() -> usize {
    1
}

pub fn fs_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/fs/list", get(list_handler))
        .route("/fs/size", get(size_handler))
        .route("/fs/info", get(info_handler))
        .route("/fs/file/content", get(get_content_handler).post(post_content_handler))
        .route("/fs/download", get(download_handler))
        .route("/fs/rm", delete(rm_handler))
        .route("/fs/zip", post(zip_handler))
        .route("/fs/unzip", post(unzip_handler))
        .route("/fs/rename", post(rename_handler))
        .route("/fs/upload", post(upload_handler))
        .route("/fs/create-empty-file", post(create_empty_file_handler))
        .route("/fs/append-file", post(append_file_handler))
}

async fn list_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    match state.fs.list(&id, &q.path, q.filter.as_deref()) {
        Ok(items) => Ok(Json(json!(items))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn size_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsSizeQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    let p = state.fs.sanitize_path(&id, &q.path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))
    })?;

    let size = if p.is_dir() {
        state.fs.get_directory_size(&p, 0, 20)
    } else if p.is_file() {
        p.metadata().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    Ok(Json(json!({ "size": size })))
}

async fn info_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsInfoQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    let p = state.fs.sanitize_path(&id, &q.path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))
    })?;

    if !p.exists() {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": "Path not found" }))));
    }

    let is_dir = p.is_dir();
    let size = if is_dir {
        state.fs.get_directory_size(&p, 0, 20)
    } else {
        p.metadata().map(|m| m.len()).unwrap_or(0)
    };

    Ok(Json(json!({
        "name": p.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        "type": if is_dir { "directory" } else { "file" },
        "size": size,
    })))
}

async fn get_content_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsContentQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    match state.fs.read_file_content(&id, &q.path) {
        Ok(content) => Ok(Json(json!({ "content": content }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn post_content_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsContentPost>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    let content_bytes = match body.content {
        Some(Value::String(s)) => s.into_bytes(),
        Some(other) => other.to_string().into_bytes(),
        None => Vec::new(),
    };

    match state.fs.write_file_content(&id, &body.path, &content_bytes) {
        Ok(_) => Ok(Json(json!({ "message": "File successfully written." }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn download_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FsDownloadQuery>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let id = q.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let path = q.path.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Path is required." })),
        )
    })?;

    let full_path = state.fs.sanitize_path(&id, &path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))
    })?;

    if !full_path.exists() || !full_path.is_file() {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": "File not found" }))));
    }

    let file = tokio::fs::File::open(&full_path).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
    })?;

    let file_name = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", file_name).parse().unwrap(),
    );

    Ok(response)
}

async fn rm_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsRmBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let path = body.path.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Path is required." })),
        )
    })?;

    match state.fs.remove(&id, &path) {
        Ok(_) => Ok(Json(json!({ "message": "File or folder successfully deleted." }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn zip_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsZipBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let zipname = body.zipname.unwrap_or_else(|| "archive".to_string());

    let files: Vec<String> = match body.files {
        Some(Value::Array(arr)) => arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
        Some(Value::String(s)) => s.split(',').map(|p| p.trim().to_string()).collect(),
        _ => Vec::new(),
    };

    if files.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Files are required" }))));
    }

    match state.fs.zip_files(&id, &files, &zipname) {
        Ok(path) => Ok(Json(json!({ "message": "Zip created successfully", "path": path.to_string_lossy() }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn unzip_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsUnzipBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let zipname = body.zipname.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "zipname is required" })))
    })?;

    match state.fs.unzip_file(&id, &body.path, &zipname) {
        Ok(_) => Ok(Json(json!({ "message": "File successfully unzipped." }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn rename_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsRenameBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let new_path = body.newPath.or(body.newName).ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "newPath is required" })))
    })?;

    match state.fs.rename(&id, &body.path, &new_path) {
        Ok(_) => Ok(Json(json!({ "message": "File successfully renamed." }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn upload_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsUploadBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;

    if !validate_container_id(&id) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid container ID format." }))));
    }

    let file_name = body.fileName.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "File name is required." })))
    })?;

    if !validate_file_name(&file_name) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid file name." }))));
    }

    if !validate_path(&body.path) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid file path." }))));
    }

    let content_val = body.fileContent.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "File content is required." })))
    })?;

    let content_bytes = decode_content(content_val).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": e })))
    })?;

    let target_path = if body.path == "/" {
        file_name.clone()
    } else {
        format!("{}/{}", body.path.trim_end_matches('/'), file_name)
    };

    match state.fs.write_file_content(&id, &target_path, &content_bytes) {
        Ok(_) => Ok(Json(json!({
            "message": "File successfully uploaded.",
            "fileName": file_name,
            "path": target_path,
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn create_empty_file_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsCreateEmptyFileBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let file_name = body.fileName.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "File name is required." })))
    })?;

    let target_path = if body.path == "/" {
        file_name.clone()
    } else {
        format!("{}/{}", body.path.trim_end_matches('/'), file_name)
    };

    match state.fs.write_file_content(&id, &target_path, b"") {
        Ok(_) => Ok(Json(json!({
            "message": "Empty file successfully created.",
            "fileName": file_name,
            "path": target_path,
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

async fn append_file_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsAppendFileBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let id = body.id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Container ID is required and must be a string." })),
        )
    })?;
    let file_name = body.fileName.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "File name is required." })))
    })?;

    let content_val = body.fileContent.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": "File content is required." })))
    })?;

    let content_bytes = decode_content(content_val).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": e })))
    })?;

    let target_path = if body.path == "/" {
        file_name.clone()
    } else {
        format!("{}/{}", body.path.trim_end_matches('/'), file_name)
    };

    match state.fs.append_file_content(&id, &target_path, &content_bytes) {
        Ok(_) => Ok(Json(json!({
            "message": "Chunk successfully appended.",
            "fileName": file_name,
            "path": target_path,
            "chunkIndex": body.chunkIndex,
            "totalChunks": body.totalChunks,
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))),
    }
}

fn decode_content(val: Value) -> Result<Vec<u8>, String> {
    match val {
        Value::String(s) => {
            if s.contains("base64") {
                if let Some(pos) = s.find("base64,") {
                    let b64_part = &s[pos + 7..];
                    base64::prelude::BASE64_STANDARD
                        .decode(b64_part.trim())
                        .map_err(|e| format!("Failed to decode base64 content: {}", e))
                } else {
                    Err("Invalid base64 format".to_string())
                }
            } else {
                Ok(s.into_bytes())
            }
        }
        Value::Array(arr) => {
            let bytes: Result<Vec<u8>, _> = arr.into_iter().map(|v| {
                v.as_u64().map(|n| n as u8).ok_or_else(|| "Invalid byte array".to_string())
            }).collect();
            bytes
        }
        _ => Err("Unsupported content type".to_string()),
    }
}

use std::sync::Arc;
use std::time::Duration;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, State,
    },
    response::Response,
    routing::get,
    Router,
};
use bollard::container::LogsOptions;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;

use crate::routes::core::AppState;
use crate::validation::validate_container_id;

#[derive(Deserialize, Debug)]
struct WsIncoming {
    event: String,
    args: Option<Vec<String>>,
    command: Option<String>,
}

pub fn websocket_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/container/{id}", get(ws_container_handler))
        .route("/containerstatus/{id}", get(ws_status_handler))
        .route("/containerevents/{id}", get(ws_events_handler))
}

async fn ws_container_handler(
    ws: WebSocketUpgrade,
    AxumPath(id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws_container(socket, id, state))
}

async fn ws_status_handler(
    ws: WebSocketUpgrade,
    AxumPath(id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws_status(socket, id, state))
}

async fn ws_events_handler(
    ws: WebSocketUpgrade,
    AxumPath(id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws_events(socket, id, state))
}

async fn handle_ws_container(mut socket: WebSocket, id: String, state: Arc<AppState>) {
    if !validate_container_id(&id) {
        let _ = socket.send(Message::Text(json!({ "error": "Invalid container ID" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    let mut authenticated = false;

    // Authentication handshake
    while let Some(Ok(msg)) = socket.next().await {
        if let Message::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<WsIncoming>(&text) {
                if incoming.event == "auth" {
                    if let Some(args) = incoming.args {
                        if args.first().map(|k| k == &state.config.key).unwrap_or(false) {
                            authenticated = true;
                            break;
                        }
                    }
                }
            }
        }
        let _ = socket.send(Message::Text(json!({ "error": "Authentication required" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    if !authenticated {
        return;
    }

    // Attach to container logs
    let logs_options = LogsOptions::<String> {
        follow: true,
        stdout: true,
        stderr: true,
        tail: "100".to_string(),
        ..Default::default()
    };

    let log_stream = state.container_mgr.docker.logs(&id, Some(logs_options));
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let id_clone = id.clone();

    // Stream logs to WS
    let mut send_task = tokio::spawn(async move {
        tokio::pin!(log_stream);
        while let Some(item_res) = log_stream.next().await {
            match item_res {
                Ok(chunk) => {
                    let bytes = chunk.into_bytes();
                    if ws_sender.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Receive commands from WS
    let container_mgr = state.container_mgr.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(incoming) = serde_json::from_str::<WsIncoming>(&text) {
                    if incoming.event == "CMD" {
                        if let Some(cmd) = incoming.command {
                            let _ = container_mgr.send_command(&id_clone, &cmd).await;
                        }
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }
}

async fn handle_ws_status(mut socket: WebSocket, id: String, state: Arc<AppState>) {
    if !validate_container_id(&id) {
        let _ = socket.send(Message::Text(json!({ "error": "Invalid container ID" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    let mut authenticated = false;

    // Authentication handshake
    while let Some(Ok(msg)) = socket.next().await {
        if let Message::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<WsIncoming>(&text) {
                if incoming.event == "auth" {
                    if let Some(args) = incoming.args {
                        if args.first().map(|k| k == &state.config.key).unwrap_or(false) {
                            authenticated = true;
                            break;
                        }
                    }
                }
            }
        }
        let _ = socket.send(Message::Text(json!({ "error": "Authentication required" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    if !authenticated {
        return;
    }

    let known_running = state.container_mgr.state_mgr.is_running(&id).unwrap_or(false);
    let initial_payload = json!({
        "event": "state",
        "data": { "running": known_running }
    }).to_string();
    if socket.send(Message::Text(initial_payload.into())).await.is_err() {
        return;
    }

    if let Some(stats) = state.container_mgr.get_container_stats(&id).await {
        let payload = json!({ "event": "stats", "data": stats }).to_string();
        if socket.send(Message::Text(payload.into())).await.is_err() {
            return;
        }
    }

    let mut state_tick = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(3));

    loop {
        interval.tick().await;
        state_tick += 1;

        let running = state.container_mgr.state_mgr.is_running(&id).unwrap_or(false);
        let loop_payload = json!({
            "event": "state",
            "data": { "running": running }
        }).to_string();
        if socket.send(Message::Text(loop_payload.into())).await.is_err() {
            break;
        }

        if state_tick % 2 == 0 {
            if let Some(stats) = state.container_mgr.get_container_stats(&id).await {
                let payload = json!({ "event": "stats", "data": stats }).to_string();
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn handle_ws_events(mut socket: WebSocket, id: String, state: Arc<AppState>) {
    if !validate_container_id(&id) {
        let _ = socket.send(Message::Text(json!({ "error": "Invalid container ID" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    let mut authenticated = false;

    // Authentication handshake
    while let Some(Ok(msg)) = socket.next().await {
        if let Message::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<WsIncoming>(&text) {
                if incoming.event == "auth" {
                    if let Some(args) = incoming.args {
                        if args.first().map(|k| k == &state.config.key).unwrap_or(false) {
                            authenticated = true;
                            break;
                        }
                    }
                }
            }
        }
        let _ = socket.send(Message::Text(json!({ "error": "Authentication required" }).to_string().into())).await;
        let _ = socket.close().await;
        return;
    }

    if !authenticated {
        return;
    }

    let mut rx = state.event_bus.subscribe();

    while let Ok((target_id, event)) = rx.recv().await {
        if target_id == id {
            let payload = json!({
                "event": "lifecycle",
                "data": event,
            }).to_string();

            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    }
}

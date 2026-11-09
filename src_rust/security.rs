use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::{log_debug, log_warn};

type HmacSha256 = Hmac<Sha256>;

const SIGNATURE_WINDOW_S: i64 = 30;

fn calculate_hmac(key: &str, method: &Method, path: &str, body_str: &str, timestamp: i64) -> String {
    let payload = format!("{}:{}:{}:{}", timestamp, method.as_str().to_uppercase(), path, body_str);
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

pub async fn security_middleware(
    State(config): State<Arc<Config>>,
    req: Request,
    next: Next,
) -> Result<Response, Response> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let headers = req.headers().clone();

    // 1. IP allowlist
    if !config.allowed_ips.is_empty() {
        let client_ip = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim())
            .unwrap_or("");

        let normalized = client_ip.strip_prefix("::ffff:").unwrap_or(client_ip);
        if !config.allowed_ips.iter().any(|allowed| allowed == normalized) {
            log_warn!("Blocked connection from {} — not in ALLOWED_IPS", normalized);
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Access denied" })),
            )
                .into_response());
        }
    }

    // 2. Basic Auth
    let auth_header = headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok());
    let mut basic_auth_ok = false;

    if let Some(auth_val) = auth_header {
        if let Some(b64) = auth_val.strip_prefix("Basic ") {
            if let Ok(decoded) = base64::prelude::BASE64_STANDARD.decode(b64.trim()) {
                if let Ok(cred_str) = String::from_utf8(decoded) {
                    if let Some((user, pass)) = cred_str.split_once(':') {
                        log_debug!(config, "Login attempt: Username = {}, Password = [REDACTED]", user);
                        if user == "Airlink" && pass == config.key {
                            basic_auth_ok = true;
                        }
                    }
                }
            }
        }
    } else {
        log_debug!(config, "Login attempt: No Authorization header provided");
    }

    if !basic_auth_ok {
        let mut resp = (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Authentication required" })),
        )
            .into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"Airlink Daemon\"".parse().unwrap(),
        );
        return Err(resp);
    }

    // 3. Content-Type check
    if method != Method::GET && method != Method::DELETE {
        if let Some(ct) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
            if !ct.starts_with("application/json")
                && !ct.starts_with("application/octet-stream")
                && !ct.starts_with("text/")
                && !ct.starts_with("multipart/")
            {
                return Err((
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    Json(json!({ "error": "Unsupported media type" })),
                )
                    .into_response());
            }
        }
    }

    // 4. Buffer body to verify HMAC
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": "Payload too large" })),
            )
                .into_response());
        }
    };

    // 5. HMAC verification
    let ts_header = headers.get("x-airlink-timestamp").and_then(|h| h.to_str().ok());
    let sig_header = headers.get("x-airlink-signature").and_then(|h| h.to_str().ok());

    match (ts_header, sig_header) {
        (None, _) | (_, None) => {
            if config.require_hmac {
                log_warn!("Rejected unsigned request: {} {}", method, path);
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Missing HMAC signature headers" })),
                )
                    .into_response());
            }
            log_debug!(config, "Unsigned request allowed (REQUIRE_HMAC not set): {} {}", method, path);
        }
        (Some(ts_str), Some(sig_str)) => {
            let timestamp: i64 = match ts_str.parse() {
                Ok(ts) => ts,
                Err(_) => {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "Invalid timestamp header" })),
                    )
                        .into_response());
                }
            };

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let drift = (now - timestamp).abs();
            if drift > SIGNATURE_WINDOW_S {
                log_warn!("Rejected request with stale timestamp (drift={}s): {} {}", drift, method, path);
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Request timestamp out of window" })),
                )
                    .into_response());
            }

            let body_str = std::str::from_utf8(&body_bytes).unwrap_or("");
            let expected_sig = calculate_hmac(&config.key, &method, &path, body_str, timestamp);

            let expected_bytes = match hex::decode(&expected_sig) {
                Ok(b) => b,
                Err(_) => vec![],
            };
            let received_bytes = match hex::decode(sig_str) {
                Ok(b) => b,
                Err(_) => vec![],
            };

            if expected_bytes.is_empty()
                || expected_bytes.len() != received_bytes.len()
                || bool::from(expected_bytes.ct_eq(&received_bytes)) == false
            {
                log_warn!("Rejected request with invalid HMAC: {} {}", method, path);
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Invalid signature" })),
                )
                    .into_response());
            }
        }
    }

    // Reconstruct request and proceed
    let req = Request::from_parts(parts, Body::from(body_bytes));
    Ok(next.run(req).await)
}

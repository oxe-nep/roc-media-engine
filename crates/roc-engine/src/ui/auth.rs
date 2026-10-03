use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Clone)]
pub struct ApiKey(pub Option<Arc<str>>);

impl ApiKey {
    pub fn from_env() -> Self {
        let key = std::env::var("API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| Arc::from(s.into_boxed_str()));
        Self(key)
    }
}

pub async fn require_api_key(api: ApiKey, req: Request, next: Next) -> Response {
    let Some(expected) = api.0.as_ref() else {
        return next.run(req).await;
    };
    let path = req.uri().path();
    if path == "/health" || path == "/api/health" {
        return next.run(req).await;
    }
    let header = req
        .headers()
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let query = req.uri().query().unwrap_or("");
    let qkey = query
        .split('&')
        .find_map(|p| p.strip_prefix("api_key="))
        .unwrap_or("");
    if header == expected.as_ref() || qkey == expected.as_ref() {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

/// Discard body type helper for middleware that doesn't need Body rename.
#[allow(dead_code)]
pub type _Body = Body;

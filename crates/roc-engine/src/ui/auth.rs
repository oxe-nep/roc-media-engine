use std::sync::Arc;

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

    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }
}

/// Compare without short-circuiting on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Pull `api_key` out of a query string (used by WebSocket / <video> URLs that
/// cannot set headers).
fn query_key(query: &str) -> Option<&str> {
    query.split('&').find_map(|p| p.strip_prefix("api_key="))
}

pub async fn require_api_key(api: ApiKey, req: Request, next: Next) -> Response {
    let Some(expected) = api.0.as_ref() else {
        return next.run(req).await;
    };
    let path = req.uri().path();
    if path == "/health" || path == "/api/health" {
        return next.run(req).await;
    }
    let expected = expected.as_bytes();
    let header_ok = req
        .headers()
        .get("X-API-Key")
        .map(|v| constant_time_eq(v.as_bytes(), expected))
        .unwrap_or(false);
    let query_ok = req
        .uri()
        .query()
        .and_then(query_key)
        .map(|k| constant_time_eq(k.as_bytes(), expected))
        .unwrap_or(false);
    if header_ok || query_ok {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret2"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn query_key_parse() {
        assert_eq!(query_key("a=1&api_key=abc&b=2"), Some("abc"));
        assert_eq!(query_key("x_api_key=abc"), None);
        assert_eq!(query_key(""), None);
    }
}

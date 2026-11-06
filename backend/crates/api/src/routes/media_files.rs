//! `GET /media/{token}/m/{media_id}/{path}`: token-checked media files.
//!
//! Development stand-in for the Cloudflare media gateway (`edge/`), with the same URL
//! scheme and token check. `MEDIA_BASE_URL` points at a different origin than the app,
//! as the gateway's cookieless domain does in production. Not part of the JSON API.

use app_core::storage::{content_type_for, media_prefix};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;

pub async fn serve(
    State(state): State<AppState>,
    Path((token, media_id, path)): Path<(String, Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    if !state
        .tokens
        .verify(&token, media_id, Utc::now().timestamp())
    {
        return plain(StatusCode::FORBIDDEN, "invalid or expired media token");
    }
    // Derived files only: originals aren't downloadable through this route (yet).
    let safe = !path.is_empty()
        && path != "original"
        && !path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'));
    if !safe {
        return plain(StatusCode::NOT_FOUND, "not found");
    }

    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let key = format!("{}{path}", media_prefix(media_id));
    let object = match state.storage.get_object(&key, range).await {
        Ok(Some(object)) => object,
        Ok(None) => return plain(StatusCode::NOT_FOUND, "not found"),
        Err(err) => {
            tracing::error!(error = ?err, %key, "reading media file");
            return plain(StatusCode::BAD_GATEWAY, "storage unavailable");
        }
    };

    let status = if object.content_range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut res = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type_for(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Length, Content-Range",
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("Cross-Origin-Resource-Policy", "cross-origin");
    if let Some(len) = object.content_length {
        res = res.header(header::CONTENT_LENGTH, len);
    }
    if let Some(range) = object
        .content_range
        .as_deref()
        .and_then(|r| HeaderValue::from_str(r).ok())
    {
        res = res.header(header::CONTENT_RANGE, range);
    }
    if let Some(etag) = object
        .etag
        .as_deref()
        .and_then(|e| HeaderValue::from_str(e).ok())
    {
        res = res.header(header::ETAG, etag);
    }
    let body = Body::from_stream(ReaderStream::new(object.body.into_async_read()));
    res.body(body)
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))
}

/// CORS preflight for range requests from players (hls.js sends `Range`).
pub async fn preflight() -> impl IntoResponse {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS"),
            (header::ACCESS_CONTROL_ALLOW_HEADERS, "Range"),
            (header::ACCESS_CONTROL_MAX_AGE, "86400"),
        ],
    )
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
        ],
        message,
    )
        .into_response()
}

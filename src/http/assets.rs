//! Static asset handler. Serves files from the `web/` directory bundled at
//! compile time via `rust-embed`.
//!
//! Phase 3 includes only a placeholder `index.html`; Phase 4 fills in the
//! viewer.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/"]
struct WebAssets;

/// Serve `index.html` for `/`, otherwise look up by URI path. Unknown paths
/// return 404 with `Content-Type: text/plain` so attackers can't tell apart
/// "file we serve" from "file we don't".
pub async fn serve_asset(req: Request<Body>) -> Response {
    let path = req.uri().path();
    let trimmed = path.trim_start_matches('/');
    let key = if trimmed.is_empty() { "index.html" } else { trimmed };

    match WebAssets::get(key) {
        Some(asset) => {
            let mime = mime_guess::from_path(key).first_or_octet_stream();
            let body = Body::from(asset.data.into_owned());
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(body)
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "asset build error").into_response()
                })
        }
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "not found",
        )
            .into_response(),
    }
}

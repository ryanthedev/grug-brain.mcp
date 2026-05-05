//! Static asset handler. Serves files from the `web/` directory bundled at
//! compile time via `rust-embed`.
//!
//! Cache-busting: `index.html` contains `{{FOO_HASH}}` template placeholders.
//! On first request, `serve_asset` replaces each placeholder with the SHA-256
//! hex digest of the corresponding embedded asset, then caches the result in
//! `INDEX_CACHE`. Subsequent requests for `/` are served from the cache.
//!
//! Query strings are stripped before looking up keys in `WebAssets`; this
//! lets the browser cache assets by content-hash URL (`/app.js?v=abc123`)
//! while the server serves the plain `app.js` file.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "web/"]
struct WebAssets;

/// Cached rendered `index.html` with hash placeholders resolved.
static INDEX_CACHE: OnceLock<Vec<u8>> = OnceLock::new();

/// Serve `index.html` for `/`, otherwise look up by URI path (query stripped).
/// Unknown paths return 404 with `Content-Type: text/plain`.
pub async fn serve_asset(req: Request<Body>) -> Response {
    let path = req.uri().path();
    // Strip leading `/` to match rust-embed keys (e.g. "app.js").
    let trimmed = path.trim_start_matches('/');
    let key = if trimmed.is_empty() { "index.html" } else { trimmed };

    if key == "index.html" {
        let bytes = INDEX_CACHE.get_or_init(build_index);
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(bytes.clone()))
            .unwrap_or_else(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "asset build error").into_response()
            });
    }

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

/// Build the rendered `index.html` by replacing `{{KEY_HASH}}` placeholders
/// with the SHA-256 hex digest of each corresponding embedded file.
///
/// Called at most once (via `OnceLock`). Panics are impossible — if an asset
/// is missing the placeholder is left as-is (browser will cache-miss once).
fn build_index() -> Vec<u8> {
    let raw = match WebAssets::get("index.html") {
        Some(f) => f.data.into_owned(),
        None => return b"<!doctype html><html><body>index.html missing</body></html>".to_vec(),
    };

    let mut html = String::from_utf8_lossy(&raw).into_owned();

    // Replace each placeholder with the hash of the corresponding asset.
    let replacements = [
        ("{{STYLES_HASH}}", "styles.css"),
        ("{{GRAPHOLOGY_HASH}}", "vendor/graphology.min.js"),
        ("{{SIGMA_HASH}}", "vendor/sigma.min.js"),
        ("{{DOMPURIFY_HASH}}", "vendor/dompurify.min.js"),
        ("{{MARKED_HASH}}", "vendor/marked.min.js"),
        ("{{CODEMIRROR_HASH}}", "vendor/codemirror.min.js"),
        ("{{JSDIFF_HASH}}", "vendor/jsdiff.min.js"),
        ("{{APP_JS_HASH}}", "app.js"),
    ];

    for (placeholder, asset_key) in &replacements {
        if let Some(asset) = WebAssets::get(asset_key) {
            let hash = content_hash(&asset.data);
            html = html.replace(placeholder, &hash);
        }
    }

    html.into_bytes()
}

/// Compute a stable 64-bit FNV-1a content fingerprint and return it as a
/// 16-character lowercase hex string.
///
/// This is used only for cache-busting query parameters (`?v=HASH`) and has
/// no security requirements. FNV-1a is deterministic, fast, and requires no
/// external dependencies. Called at most once per asset per process lifetime.
fn content_hash(data: &[u8]) -> String {
    // FNV-1a 64-bit: https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify FNV-1a produces a known stable value for the empty slice.
    #[test]
    fn content_hash_empty() {
        // FNV-1a of empty input is the offset basis itself.
        assert_eq!(content_hash(b""), "cbf29ce484222325");
    }

    /// Verify FNV-1a produces the correct 16-char hex output for "abc".
    #[test]
    fn content_hash_abc() {
        let h = content_hash(b"abc");
        assert_eq!(h.len(), 16, "FNV-1a output must be 16 hex chars");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Known FNV-1a 64-bit value for "abc".
        assert_eq!(h, "e71fa2190541574b");
    }

    /// Verify two different inputs produce different hashes.
    #[test]
    fn content_hash_differs_for_different_input() {
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
    }

    /// Verify the index template substitution replaces all placeholders.
    #[test]
    fn build_index_substitutes_placeholders() {
        let rendered = build_index();
        let html = String::from_utf8_lossy(&rendered);
        // After substitution, no raw placeholders should remain.
        assert!(!html.contains("{{APP_JS_HASH}}"), "APP_JS_HASH placeholder not replaced");
        assert!(!html.contains("{{STYLES_HASH}}"), "STYLES_HASH placeholder not replaced");
        assert!(!html.contains("{{GRAPHOLOGY_HASH}}"), "GRAPHOLOGY_HASH placeholder not replaced");
        assert!(!html.contains("{{SIGMA_HASH}}"), "SIGMA_HASH placeholder not replaced");
        assert!(!html.contains("{{DOMPURIFY_HASH}}"), "DOMPURIFY_HASH placeholder not replaced");
        assert!(!html.contains("{{MARKED_HASH}}"), "MARKED_HASH placeholder not replaced");
        assert!(!html.contains("{{CODEMIRROR_HASH}}"), "CODEMIRROR_HASH placeholder not replaced");
        assert!(!html.contains("{{JSDIFF_HASH}}"), "JSDIFF_HASH placeholder not replaced");
        // The rendered index should still contain grug-brain.
        assert!(html.contains("grug-brain"), "title should be present");
        // Asset URLs should contain ?v= with hash values.
        assert!(html.contains("?v="), "cache-busting query param should be present");
    }
}

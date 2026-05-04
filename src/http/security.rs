//! HTTP security middleware: Host allowlist, CORS lockdown, CSRF defense, CSP.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_http::cors::CorsLayer;

/// Strict CSP header value used for HTML responses.
pub const CSP_HEADER: &str =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'";

/// Header value indicating the official web client. Required on mutating
/// routes as a CSRF defense (a CSRF attacker on another origin cannot set
/// custom headers without a CORS preflight, which our CORS layer denies).
pub const CLIENT_HEADER: &str = "x-grug-client";
pub const CLIENT_VALUE: &str = "web";

/// Build the same-origin CORS layer.
pub fn cors_layer() -> CorsLayer {
    // Same-origin only: we deliberately do NOT enable any cross-origin allow.
    // Browsers treat absent `Access-Control-Allow-Origin` as a CORS failure
    // for cross-origin fetches, which is what we want.
    CorsLayer::new()
        .allow_methods([Method::GET, Method::HEAD, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
}

/// Host allowlist: reject any Host header not in {localhost, 127.0.0.1}
/// (with optional port). Returns 403 on rejection.
pub async fn host_allowlist(req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if host_is_allowed(host) {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "host not allowed").into_response()
    }
}

fn host_is_allowed(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    // Strip optional :port suffix.
    let bare = host.split(':').next().unwrap_or(host);
    matches!(bare, "localhost" | "127.0.0.1")
}

/// CSRF defense: any mutating request (POST/PUT/DELETE/PATCH) must include
/// `X-Grug-Client: web`. The header is non-CORS-safelisted, so cross-origin
/// requests cannot set it without a preflight (and our CORS layer denies
/// preflights for mutating methods).
pub async fn require_client_header(req: Request<Body>, next: Next) -> Response {
    let method = req.method();
    let mutating = matches!(
        *method,
        Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    );
    if mutating {
        let ok = req
            .headers()
            .get(CLIENT_HEADER)
            .and_then(|h| h.to_str().ok())
            == Some(CLIENT_VALUE);
        if !ok {
            return (StatusCode::FORBIDDEN, "missing X-Grug-Client header").into_response();
        }
    }
    next.run(req).await
}

/// Add CSP header to every response. (CSP only meaningfully applies to HTML
/// but harmless on JSON; keeping it global simplifies the test surface.)
pub async fn add_csp_header(req: Request<Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(CSP_HEADER) {
        resp.headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dw_3_2_host_allowed_localhost() {
        assert!(host_is_allowed("localhost"));
        assert!(host_is_allowed("localhost:7777"));
        assert!(host_is_allowed("127.0.0.1"));
        assert!(host_is_allowed("127.0.0.1:7777"));
    }

    #[test]
    fn dw_3_2_host_rejected_other() {
        assert!(!host_is_allowed("evil.com"));
        assert!(!host_is_allowed("evil.com:7777"));
        assert!(!host_is_allowed("0.0.0.0"));
        assert!(!host_is_allowed("192.168.1.1"));
        assert!(!host_is_allowed(""));
    }

    #[test]
    fn dw_3_4_csp_header_value() {
        // Verbatim policy from plan.
        assert_eq!(
            CSP_HEADER,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'"
        );
    }

    // Layer is constructed lazily; ensure the constructor doesn't panic.
    #[test]
    fn cors_layer_constructs() {
        let _l = cors_layer();
    }

}

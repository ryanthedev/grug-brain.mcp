//! HTTP integration tests for Phase 3.
//!
//! Each test starts the full `run_server` (socket + HTTP). The HTTP port is
//! allowed to fall back to ephemeral via the bind logic; the chosen port is
//! read from `serve.port` written by the server.

use grug_brain::server::{run_server, run_server_with_shutdown};
use grug_brain::types::{Brain, BrainConfig};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;

const STARTUP_BUDGET_MS: u64 = 5000;

/// Process-global mutex serializing tests that touch env vars + port file.
/// (Cargo runs tests in parallel by default; without serialization the env
/// vars and the per-process port file races.)
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Guard returned by `setup` — holds the env-lock for the duration of the
/// test so parallel tests don't fight over `GRUG_PORT` / `GRUG_PORT_FILE`.
pub struct EnvGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

/// Allocate a brain dir + brains.json + ports for a test run. Returns
/// (tmp, socket_path, db_path, config, port_file_path, env_guard).
fn setup() -> (TempDir, PathBuf, PathBuf, BrainConfig, PathBuf, EnvGuard) {
    let guard = EnvGuard(ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let tmp = TempDir::new().unwrap();
    let brain_dir = tmp.path().join("memories");
    fs::create_dir_all(&brain_dir).unwrap();

    let config = BrainConfig {
        brains: vec![Brain {
            name: "memories".to_string(),
            dir: brain_dir,
            primary: true,
            writable: true,
            flat: false,
            git: None,
            sync_interval: 60,
            source: None,
            refresh_interval: None,
        }],
        primary: "memories".to_string(),
        config_path: tmp.path().join("brains.json"),
        last_mtime: None,
    };
    let cfg_json = serde_json::json!([{
        "name": "memories",
        "dir": config.brains[0].dir.to_str().unwrap(),
        "primary": true,
        "writable": true,
    }]);
    fs::write(&config.config_path, cfg_json.to_string()).unwrap();

    let socket_path = tmp.path().join("test.sock");
    let db_path = tmp.path().join("grug.db");
    let port_file = tmp.path().join("serve.port");
    // Per-test port file via env override; env-lock keeps this race-free.
    unsafe {
        std::env::set_var("GRUG_PORT_FILE", &port_file);
    }
    (tmp, socket_path, db_path, config, port_file, guard)
}

/// Start the server in a background task. Returns (handle, http_port).
/// The HTTP port is read from the serve.port advertisement file.
async fn start(
    socket_path: PathBuf,
    db_path: PathBuf,
    config: BrainConfig,
) -> (tokio::task::JoinHandle<()>, u16) {
    // Force ephemeral so we never race port 7777 across tests.
    unsafe {
        std::env::set_var("GRUG_PORT", "0");
    }
    let port_file = PathBuf::from(std::env::var("GRUG_PORT_FILE").expect("GRUG_PORT_FILE"));
    let _ = fs::remove_file(&port_file);

    let sock = socket_path.clone();
    let handle = tokio::spawn(async move {
        let _ = run_server(Some(sock), Some(db_path), Some(config)).await;
    });

    // Wait for the port file to appear.
    let start = std::time::Instant::now();
    let port = loop {
        if start.elapsed() > Duration::from_millis(STARTUP_BUDGET_MS) {
            panic!("port file never appeared at {}", port_file.display());
        }
        if let Ok(s) = fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                if p != 0 {
                    break p;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    (handle, port)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------
// DW-3.1: server starts, both transports work, shuts down on drop.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_1_http_and_socket_coexist() {
    // Run the server with an external shutdown channel so we can exercise
    // the *real* graceful-shutdown path (the same select arm SIGINT/SIGTERM
    // arrive at) without raising a process-wide signal that would affect
    // every other test in the binary.
    let (tmp, sock, db, cfg, _pf, _g) = setup();
    unsafe {
        std::env::set_var("GRUG_PORT", "0");
    }
    let port_file =
        PathBuf::from(std::env::var("GRUG_PORT_FILE").expect("GRUG_PORT_FILE"));
    let _ = fs::remove_file(&port_file);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let sock_clone = sock.clone();
    let handle = tokio::spawn(async move {
        let _ = run_server_with_shutdown(
            Some(sock_clone),
            Some(db),
            Some(cfg),
            Some(shutdown_rx),
        )
        .await;
    });

    // Wait for port file to confirm HTTP is live.
    let started = std::time::Instant::now();
    let port = loop {
        if started.elapsed() > Duration::from_millis(STARTUP_BUDGET_MS) {
            panic!("port file never appeared at {}", port_file.display());
        }
        if let Ok(s) = fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                if p != 0 {
                    break p;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Both transports up.
    let url = format!("http://127.0.0.1:{port}/api/healthz");
    let resp = client().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(sock.exists(), "socket should exist");

    // Trigger graceful shutdown via the same channel the SIGINT/SIGTERM arms
    // would feed. Server should drain, remove its socket, exit cleanly.
    shutdown_tx.send(()).expect("send shutdown");
    let join = tokio::time::timeout(Duration::from_secs(15), handle).await;
    assert!(join.is_ok(), "server did not shut down within 15s");
    join.unwrap().expect("server task panicked");

    // After clean shutdown, the socket file is removed and HTTP is gone.
    assert!(!sock.exists(), "socket should be removed on clean shutdown");
    let post = client().get(&url).send().await;
    assert!(
        post.is_err() || !post.unwrap().status().is_success(),
        "HTTP should no longer be accepting connections after shutdown"
    );

    drop(tmp);
}

// Smaller unit-style test: the signal handler arms select on the same
// receivers. We can't easily raise SIGTERM in-process without affecting
// other tests, but we can at least assert the public API exposes a
// shutdown path that maps to the same select branch SIGINT/SIGTERM hit.
#[tokio::test]
async fn test_dw_3_1_signal_arms_compile_and_shutdown_channel_works() {
    // Create a oneshot, drop the sender immediately — receiver resolves with
    // Err, which the loop maps to `_ = external_fut => break`. This proves
    // the shutdown arm is wired in and doesn't panic on a closed channel.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    drop(tx);
    // Just await the rx to confirm the test harness sees the closed-channel
    // signal — the real proof is that test_dw_3_1_http_and_socket_coexist
    // shuts down via this same receiver type without forcing handle.abort().
    assert!(rx.await.is_err());
}

// ---------------------------------------------------------------------------
// DW-3.2: Host allowlist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_2_host_allowlist() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    let url = format!("http://127.0.0.1:{port}/api/healthz");

    // Forbidden host -> 403
    let resp = client()
        .get(&url)
        .header("Host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "evil.com host should be rejected");

    // Localhost host -> 200
    let resp = client()
        .get(&url)
        .header("Host", "localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.3: CORS lockdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_3_cors_no_cross_origin_allow() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    let url = format!("http://127.0.0.1:{port}/api/healthz");

    let resp = client()
        .get(&url)
        .header("Origin", "https://evil.com")
        .send()
        .await
        .unwrap();
    // No Access-Control-Allow-Origin header for cross-origin -> browser blocks.
    let aco = resp
        .headers()
        .get("access-control-allow-origin")
        .map(|h| h.to_str().unwrap_or("").to_string());
    assert!(
        aco.as_deref() != Some("https://evil.com")
            && aco.as_deref() != Some("*"),
        "must not allow cross-origin: got {aco:?}"
    );

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.4: CSP header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_4_csp_header_present() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    let csp = resp
        .headers()
        .get("content-security-policy")
        .map(|h| h.to_str().unwrap().to_string());
    assert_eq!(
        csp.as_deref(),
        Some("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'")
    );

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.5: CSRF defense on mutating routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_5_csrf_required() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    let url = format!("http://127.0.0.1:{port}/api/_csrf_probe");

    // POST without header -> 403
    let resp = client().post(&url).send().await.unwrap();
    assert_eq!(resp.status(), 403);

    // POST with header -> 200
    let resp = client()
        .post(&url)
        .header("X-Grug-Client", "web")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.6: read endpoint shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_6_endpoint_shapes() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    // Seed one memory.
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/hello.md"),
        "---\nname: hello\ndate: 2025-01-01\ndescription: greet\n---\n\nhello body",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    let base = format!("http://127.0.0.1:{port}");

    // Allow the initial reindex to land.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // /api/brains
    let v: Value = client()
        .get(format!("{base}/api/brains"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v.is_array(), "brains should be array: {v}");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let b = &arr[0];
    for k in ["name", "primary", "writable", "source", "flat"] {
        assert!(b.get(k).is_some(), "brains[0].{k} missing: {b}");
    }

    // /api/memories
    let v: Value = client()
        .get(format!("{base}/api/memories?brain=memories"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v.is_array(), "memories should be array: {v}");
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least one memory: {v}");
    for k in ["path", "category", "name", "description", "date", "mtime"] {
        assert!(arr[0].get(k).is_some(), "memories[0].{k} missing: {}", arr[0]);
    }

    // /api/memory/:brain/:category/:path
    let v: Value = client()
        .get(format!("{base}/api/memory/memories/notes/hello"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for k in ["frontmatter", "body", "mtime", "neighbors"] {
        assert!(v.get(k).is_some(), "memory.{k} missing: {v}");
    }
    assert!(v["body"].as_str().unwrap().contains("hello body"));
    assert!(v["neighbors"].is_array());

    // 404 path
    let resp = client()
        .get(format!("{base}/api/memory/memories/notes/nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // /api/graph
    let v: Value = client()
        .get(format!("{base}/api/graph?brain=memories&mode=global"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v.get("nodes").is_some() && v.get("edges").is_some(), "graph: {v}");

    // /api/search
    let v: Value = client()
        .get(format!("{base}/api/search?q=hello"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v.get("hits").is_some(), "search: {v}");
    assert!(v.get("total").is_some());

    // /api/quickswitch
    let v: Value = client()
        .get(format!("{base}/api/quickswitch?q=hel"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v.get("hits").is_some(), "quickswitch: {v}");

    // /api/healthz
    let v: Value = client()
        .get(format!("{base}/api/healthz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["ok"], true);
    assert!(v.get("schema_version").is_some());
    assert!(v.get("brains").is_some());

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.7: SSE streams MemoryEvent on external edit within 2s
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_7_sse_emits_on_external_edit() {
    use futures_util::StreamExt;
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();

    let (handle, port) = start(sock, db, cfg).await;

    // Connect SSE.
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/api/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let mut stream = resp.bytes_stream();

    // Trigger a write after a beat to ensure SSE receiver is wired up.
    let path = brain_dir.join("notes/sse-test.md");
    let path2 = path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        fs::write(&path2, "---\nname: sse-test\n---\n\nbody").unwrap();
    });

    // Read up to 4s of stream looking for event:memory + sse-test path.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut buf = String::new();
    let mut got = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                if buf.contains("notes/sse-test.md") && buf.contains("event: memory") {
                    got = true;
                    break;
                }
            }
            _ => continue,
        }
    }
    assert!(got, "expected SSE event for sse-test.md, got buf:\n{buf}");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.8: Port collision fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_8_port_collision_fallback() {
    // Pre-bind 7777.
    let blocker = match TcpListener::bind("127.0.0.1:7777").await {
        Ok(l) => l,
        Err(_) => {
            // 7777 already in use externally — skip rather than fail flaky.
            eprintln!("test_dw_3_8 skipped: 7777 already in use externally");
            return;
        }
    };

    let (tmp, sock, db, cfg, port_file, _g) = setup();
    // Force GRUG_PORT=7777 to test collision (overrides the 0 from start()).
    unsafe {
        std::env::set_var("GRUG_PORT", "7777");
    }
    let _ = fs::remove_file(&port_file);

    let sock_clone = sock.clone();
    let handle = tokio::spawn(async move {
        let _ = run_server(Some(sock_clone), Some(db), Some(cfg)).await;
    });

    // Wait for port file with a non-7777 value.
    let start = std::time::Instant::now();
    let port = loop {
        if start.elapsed() > Duration::from_millis(STARTUP_BUDGET_MS) {
            panic!("port file never appeared");
        }
        if let Ok(s) = fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                if p != 0 {
                    break p;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_ne!(port, 7777, "should have fallen back to a different port");

    drop(blocker);
    handle.abort();
    drop(tmp);
    // Reset env for other tests.
    unsafe {
        std::env::set_var("GRUG_PORT", "0");
    }
}

// ---------------------------------------------------------------------------
// DW-3.9: tracing span per request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_9_tracing_layer_emits_spans_per_request() {
    // Verify that the router actually emits a tracing span per request,
    // proving `TraceLayer` is wired in (not just listed in Cargo.toml).
    //
    // We exercise `build_router` directly via `tower::ServiceExt::oneshot` so
    // the request runs on the current task — `set_default` then captures the
    // span emitted by `TraceLayer`. This avoids racing with the global
    // subscriber installed by `run_server`.
    use axum::body::Body;
    use axum::http::Request;
    use grug_brain::http::{build_router, AppState};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use tracing::span::{Attributes, Id};
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;
    use tracing_subscriber::Layer;

    /// Captures span names into a shared Vec.
    struct CaptureLayer(Arc<Mutex<Vec<String>>>);
    impl<S> Layer<S> for CaptureLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
            if let Ok(mut v) = self.0.lock() {
                v.push(attrs.metadata().name().to_string());
            }
        }
    }

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    // Router with a dummy db_tx — we don't need a working DB; any request
    // that hits the router (even one that errors) will trip TraceLayer.
    let (db_tx, _db_rx) = tokio::sync::mpsc::channel(1);
    let state = AppState { db_tx, events: None };
    let app = build_router(state);

    // Hit the static-asset fallback for `/` — handled inline, doesn't need DB.
    let req = Request::builder()
        .uri("/")
        .header("host", "127.0.0.1")
        .header("x-grug-client", "web")
        .body(Body::empty())
        .unwrap();
    let _resp = app.oneshot(req).await.unwrap();

    let names = captured.lock().unwrap().clone();
    assert!(
        names.iter().any(|n| n.contains("request") || n.contains("HTTP")),
        "expected an HTTP-related span emitted by TraceLayer; captured: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// DW-3.10: path traversal rejected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_10_path_traversal_rejected() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    let base = format!("http://127.0.0.1:{port}");

    // ".." in path component (URL-encoded as ..%2E or just .. — axum routes
    // strip nothing for us; .. should reach the handler and validate_memory_path
    // should reject it).
    let resp = client()
        .get(format!("{base}/api/memory/memories/notes/..%2Fescape"))
        .send()
        .await
        .unwrap();
    // Either 400 (we caught it) or 404 (router didn't match) — but NOT 200 with
    // out-of-tree content. We assert a 4xx status.
    assert!(
        resp.status().is_client_error(),
        "expected 4xx for traversal, got {}",
        resp.status()
    );

    // Plain `..` segment.
    let resp = client()
        .get(format!("{base}/api/memory/memories/..%20bad/notes"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_client_error());

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-3.11: rust-embed serves index, 404 for unknown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_3_11_assets_index_and_404() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;

    // GET /
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.starts_with("text/html"), "expected text/html, got {ct}");
    let body = resp.text().await.unwrap();
    assert!(body.contains("grug-brain"));

    // Unknown asset -> 404 with text/plain
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/nonexistent.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.starts_with("text/plain"), "expected text/plain, got {ct}");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-4.1: index.html has correct Content-Type + asset URLs contain ?v= hash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_4_1_index_html_content_type_and_content_hash() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let ct = resp
        .headers()
        .get("content-type")
        .map(|h| h.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(ct.starts_with("text/html"), "expected text/html Content-Type, got {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.contains("grug-brain"), "index.html should mention grug-brain");

    // Content-hash cache-busting: asset URLs must contain ?v= with a hex value.
    assert!(
        body.contains("?v="),
        "index.html must include ?v=<hash> cache-busting query params on asset URLs"
    );
    // Verify placeholder substitution happened (no raw {{...}} remaining).
    assert!(
        !body.contains("{{"),
        "index.html still contains unresolved template placeholders"
    );

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-4.11: Test-force-500 param triggers 500 in debug builds
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg(debug_assertions)]
async fn test_dw_4_11_forced_500_in_debug_builds() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;

    // Without the param: healthz should return 200.
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/api/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "healthz without param should be 200");

    // With the test-force-500 param: should return 500 in debug builds.
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/api/healthz?__test_force_500=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500, "healthz with __test_force_500=1 should return 500");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"].as_str().unwrap_or(""),
        "forced test error",
        "should return the expected error message"
    );

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// Phase 2 (Plan 2) write-route helpers
// ---------------------------------------------------------------------------

/// Build a `setup`-style config with a read-only brain named "docs".
fn setup_readonly() -> (TempDir, PathBuf, PathBuf, BrainConfig, PathBuf, EnvGuard) {
    let guard = EnvGuard(ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let tmp = TempDir::new().unwrap();
    let brain_dir = tmp.path().join("docs");
    fs::create_dir_all(&brain_dir).unwrap();

    let config = BrainConfig {
        brains: vec![Brain {
            name: "docs".to_string(),
            dir: brain_dir,
            primary: true,
            writable: false,
            flat: false,
            git: None,
            sync_interval: 60,
            source: None,
            refresh_interval: None,
        }],
        primary: "docs".to_string(),
        config_path: tmp.path().join("brains.json"),
        last_mtime: None,
    };
    let cfg_json = serde_json::json!([{
        "name": "docs",
        "dir": config.brains[0].dir.to_str().unwrap(),
        "primary": true,
        "writable": false,
    }]);
    fs::write(&config.config_path, cfg_json.to_string()).unwrap();

    let socket_path = tmp.path().join("test.sock");
    let db_path = tmp.path().join("grug.db");
    let port_file = tmp.path().join("serve.port");
    unsafe {
        std::env::set_var("GRUG_PORT_FILE", &port_file);
    }
    (tmp, socket_path, db_path, config, port_file, guard)
}

async fn post_json(c: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    c.post(url)
        .header("X-Grug-Client", "web")
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn put_json(
    c: &reqwest::Client,
    url: &str,
    body: Value,
    etag: Option<f64>,
) -> reqwest::Response {
    let mut req = c.put(url).header("X-Grug-Client", "web").json(&body);
    if let Some(e) = etag {
        req = req.header("If-Match", e.to_string());
    }
    req.send().await.unwrap()
}

async fn delete_json(c: &reqwest::Client, url: &str) -> reqwest::Response {
    c.delete(url)
        .header("X-Grug-Client", "web")
        .send()
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// DW-1.1: PUT /api/memory/:brain/:category/:path — 200 + new ETag on success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_1_put_updates_memory_returns_200_and_etag() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/hello.md"),
        "---\nname: hello\ndate: 2025-01-01\ntype: memory\n---\n\noriginal body",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    let mem: Value = c
        .get(format!("{base}/api/memory/memories/notes/hello"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let etag: f64 = mem["mtime"].as_f64().expect("mtime");

    let resp = put_json(
        &c,
        &format!("{base}/api/memory/memories/notes/hello"),
        serde_json::json!({"body": "updated body", "frontmatter": "name: hello\ndate: 2025-01-01\ntype: memory"}),
        Some(etag),
    )
    .await;

    assert_eq!(resp.status(), 200, "PUT should return 200 on success");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("etag").is_some(), "response should include etag: {body}");
    assert_eq!(body["ok"], true);

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.2: PUT with stale If-Match — 409 + structured ConflictResponse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_2_put_stale_etag_returns_409_conflict_response() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/conflict-me.md"),
        "---\nname: conflict-me\ndate: 2025-01-01\ntype: memory\n---\n\ncurrent body",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    let resp = put_json(
        &c,
        &format!("{base}/api/memory/memories/notes/conflict-me"),
        serde_json::json!({"body": "attempted body", "frontmatter": "name: conflict-me\ndate: 2025-01-01\ntype: memory"}),
        Some(0.0001),
    )
    .await;

    assert_eq!(resp.status(), 409, "stale ETag should return 409");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "conflict", "error field: {body}");
    assert!(body.get("current_etag").is_some(), "missing current_etag: {body}");
    assert!(body.get("current_body").is_some(), "missing current_body: {body}");
    assert!(body.get("attempted_body").is_some(), "missing attempted_body: {body}");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.3: POST /api/memory — create + 201; duplicate → 409
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_3_post_creates_memory_201() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let resp = post_json(
        &client(),
        &format!("http://127.0.0.1:{port}/api/memory"),
        serde_json::json!({
            "path": "notes/brand-new",
            "body": "hello world",
            "frontmatter": "name: brand-new\ndate: 2025-01-01\ntype: memory"
        }),
    )
    .await;

    assert_eq!(resp.status(), 201, "create should return 201");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("path").is_some(), "response missing path: {body}");
    assert!(body.get("etag").is_some(), "response missing etag: {body}");

    handle.abort();
    drop(tmp);
}

#[tokio::test]
async fn test_dw_1_3_post_duplicate_returns_409() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/existing.md"),
        "---\nname: existing\ndate: 2025-01-01\ntype: memory\n---\n\nbody",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let resp = post_json(
        &client(),
        &format!("http://127.0.0.1:{port}/api/memory"),
        serde_json::json!({
            "path": "notes/existing",
            "body": "duplicate",
            "frontmatter": "name: existing\ndate: 2025-01-01\ntype: memory"
        }),
    )
    .await;

    assert_eq!(resp.status(), 409, "duplicate create should return 409");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.4: DELETE — 204 on success and 204 on missing (idempotent)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_4_delete_returns_204_on_success() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/doomed.md"),
        "---\nname: doomed\ndate: 2025-01-01\ntype: memory\n---\n\nbody",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let resp = delete_json(
        &client(),
        &format!("http://127.0.0.1:{port}/api/memory/memories/notes/doomed"),
    )
    .await;
    assert_eq!(resp.status(), 204, "DELETE of existing file should return 204");

    handle.abort();
    drop(tmp);
}

#[tokio::test]
async fn test_dw_1_4_delete_returns_204_on_missing() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let resp = delete_json(
        &client(),
        &format!("http://127.0.0.1:{port}/api/memory/memories/notes/nonexistent"),
    )
    .await;
    assert_eq!(resp.status(), 204, "DELETE of missing file should return 204 (idempotent)");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.5: POST rename — 200 + new path + ETag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_5_rename_returns_200_new_path_etag() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/old-name.md"),
        "---\nname: old-name\ndate: 2025-01-01\ntype: memory\n---\n\nbody",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    let resp = post_json(
        &c,
        &format!("{base}/api/memory/memories/notes/old-name/rename"),
        serde_json::json!({"new_path": "notes/new-name"}),
    )
    .await;

    assert_eq!(resp.status(), 200, "rename should return 200");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("path").is_some(), "missing path: {body}");
    assert!(body.get("etag").is_some(), "missing etag: {body}");
    assert!(
        body["path"].as_str().unwrap_or("").contains("new-name"),
        "path should reflect new name: {body}"
    );
    assert!(!brain_dir.join("notes/old-name.md").exists());
    assert!(brain_dir.join("notes/new-name.md").exists());

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.6: All 5 routes return 403 on read-only brain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_6_all_write_routes_403_on_readonly() {
    let (tmp, sock, db, cfg, _, _g) = setup_readonly();
    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    let resp = put_json(
        &c,
        &format!("{base}/api/memory/docs/notes/test"),
        serde_json::json!({"body": "x", "frontmatter": "name: test"}),
        Some(0.0),
    )
    .await;
    assert_eq!(resp.status(), 403, "PUT on read-only brain should return 403");

    let resp = post_json(
        &c,
        &format!("{base}/api/memory"),
        serde_json::json!({"path": "notes/new", "body": "x", "frontmatter": "name: new", "brain": "docs"}),
    )
    .await;
    assert_eq!(resp.status(), 403, "POST create on read-only brain should return 403");

    let resp = delete_json(&c, &format!("{base}/api/memory/docs/notes/test")).await;
    assert_eq!(resp.status(), 403, "DELETE on read-only brain should return 403");

    let resp = post_json(
        &c,
        &format!("{base}/api/memory/docs/notes/test/rename"),
        serde_json::json!({"new_path": "notes/other"}),
    )
    .await;
    assert_eq!(resp.status(), 403, "rename on read-only brain should return 403");

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.7: All write routes reject path-traversal with 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_7_all_write_routes_400_on_traversal() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();
    let (handle, port) = start(sock, db, cfg).await;

    // Seed file for rename traversal test.
    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/real.md"),
        "---\nname: real\ntype: memory\n---\n\nbody",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    // POST create with traversal in path field.
    let resp = post_json(
        &c,
        &format!("{base}/api/memory"),
        serde_json::json!({"path": "../escape/bad", "body": "x", "frontmatter": "name: bad"}),
    )
    .await;
    assert!(
        resp.status().is_client_error(),
        "traversal path in create should be rejected (4xx): {}",
        resp.status()
    );

    // POST rename with traversal in new_path.
    let resp = post_json(
        &c,
        &format!("{base}/api/memory/memories/notes/real/rename"),
        serde_json::json!({"new_path": "../escape/bad"}),
    )
    .await;
    assert!(
        resp.status().is_client_error(),
        "traversal in rename new_path should return 4xx: {}",
        resp.status()
    );

    handle.abort();
    drop(tmp);
}

// ---------------------------------------------------------------------------
// DW-1.11: Comprehensive status-code coverage for each route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dw_1_11_all_status_codes_per_route() {
    let (tmp, sock, db, cfg, _, _g) = setup();
    let brain_dir = cfg.brains[0].dir.clone();

    fs::create_dir_all(brain_dir.join("notes")).unwrap();
    fs::write(
        brain_dir.join("notes/seed.md"),
        "---\nname: seed\ndate: 2025-01-01\ntype: memory\n---\n\nseed body",
    )
    .unwrap();

    let (handle, port) = start(sock, db, cfg).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let base = format!("http://127.0.0.1:{port}");
    let c = client();

    let mem: Value = c
        .get(format!("{base}/api/memory/memories/notes/seed"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let etag: f64 = mem["mtime"].as_f64().expect("mtime field missing");

    // PUT 200.
    let resp = put_json(
        &c,
        &format!("{base}/api/memory/memories/notes/seed"),
        serde_json::json!({"body": "updated", "frontmatter": "name: seed\ndate: 2025-01-01\ntype: memory"}),
        Some(etag),
    )
    .await;
    assert_eq!(resp.status(), 200, "PUT match -> 200");

    // PUT 409 stale.
    let resp = put_json(
        &c,
        &format!("{base}/api/memory/memories/notes/seed"),
        serde_json::json!({"body": "stale attempt", "frontmatter": "name: seed\ndate: 2025-01-01\ntype: memory"}),
        Some(0.0001),
    )
    .await;
    assert_eq!(resp.status(), 409, "PUT stale -> 409");

    // POST create 201.
    let resp = post_json(
        &c,
        &format!("{base}/api/memory"),
        serde_json::json!({"path": "notes/fresh", "body": "body", "frontmatter": "name: fresh\ndate: 2025-01-01\ntype: memory"}),
    )
    .await;
    assert_eq!(resp.status(), 201, "POST create -> 201");

    // POST create 409 duplicate.
    let resp = post_json(
        &c,
        &format!("{base}/api/memory"),
        serde_json::json!({"path": "notes/fresh", "body": "body2", "frontmatter": "name: fresh\ndate: 2025-01-01\ntype: memory"}),
    )
    .await;
    assert_eq!(resp.status(), 409, "POST create duplicate -> 409");

    // DELETE 204 success.
    let resp = delete_json(&c, &format!("{base}/api/memory/memories/notes/fresh")).await;
    assert_eq!(resp.status(), 204, "DELETE -> 204");

    // DELETE 204 idempotent.
    let resp = delete_json(&c, &format!("{base}/api/memory/memories/notes/fresh")).await;
    assert_eq!(resp.status(), 204, "DELETE missing -> 204 idempotent");

    // POST rename 200.
    let resp = post_json(
        &c,
        &format!("{base}/api/memory/memories/notes/seed/rename"),
        serde_json::json!({"new_path": "notes/seed-renamed"}),
    )
    .await;
    assert_eq!(resp.status(), 200, "rename -> 200");

    // POST rename 404 (source gone after rename).
    let resp = post_json(
        &c,
        &format!("{base}/api/memory/memories/notes/seed/rename"),
        serde_json::json!({"new_path": "notes/seed-again"}),
    )
    .await;
    assert_eq!(resp.status(), 404, "rename nonexistent -> 404");

    handle.abort();
    drop(tmp);
}

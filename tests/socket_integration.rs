use grug_brain::client::SocketClient;
use grug_brain::protocol::{SocketRequest, SocketResponse};
use grug_brain::server::run_server;
use grug_brain::types::{Brain, BrainConfig};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::Duration;

/// Set up a test environment with a temp directory, brain config, and socket path.
/// Returns (tmp_dir, socket_path, brain_config).
fn setup_test_env() -> (TempDir, PathBuf, BrainConfig) {
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

    let socket_path = tmp.path().join("test.sock");
    let db_path = tmp.path().join("grug.db");

    // Write a brains.json for the config module (needed by grug-config list)
    let config_json = serde_json::json!([{
        "name": "memories",
        "dir": config.brains[0].dir.to_str().unwrap(),
        "primary": true,
        "writable": true
    }]);
    fs::write(&config.config_path, serde_json::to_string_pretty(&config_json).unwrap()).unwrap();

    // Store db_path in the config_path parent for convention
    // (the server will use the explicit db_path parameter)
    let _ = db_path;

    (tmp, socket_path, config)
}

/// Start the server in a background task and wait until the socket is connectable.
async fn start_server(
    socket_path: PathBuf,
    db_path: PathBuf,
    config: BrainConfig,
) -> tokio::task::JoinHandle<()> {
    let sock = socket_path.clone();
    let handle = tokio::spawn(async move {
        let _ = run_server(Some(sock), Some(db_path), Some(config)).await;
    });

    // Wait for socket to be connectable (not just existing as a file)
    for _ in 0..100 {
        if UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    handle
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// DW-3.1: Server starts, creates socket, accepts connections.
#[tokio::test]
async fn test_server_starts_and_accepts() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    // Connect and send a simple request
    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let req = SocketRequest {
        id: "test-1".to_string(),
        tool: "grug-search".to_string(),
        params: json!({"query": "hello"}),
    };
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    stream.write_all(line.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    // Read response
    let (reader, _) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();
    buf_reader.read_line(&mut response_line).await.unwrap();
    let resp: SocketResponse = serde_json::from_str(&response_line).unwrap();

    assert_eq!(resp.id, "test-1");
    // Either result or error should be present
    assert!(resp.result.is_some() || resp.error.is_some());

    handle.abort();
}

/// DW-3.2, DW-3.6: All 9 tools work through the socket.
#[tokio::test]
async fn test_all_tools_through_socket() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    let client = SocketClient::connect(&socket_path).await.unwrap();
    let client = std::sync::Arc::new(tokio::sync::Mutex::new(client));

    // Helper to call and check result
    async fn call(
        client: &tokio::sync::Mutex<SocketClient>,
        tool: &str,
        params: serde_json::Value,
    ) -> String {
        let mut c = client.lock().await;
        c.call(tool, params)
            .await
            .unwrap_or_else(|e| panic!("{tool} failed: {e}"))
    }

    // 1. grug-write
    let result = call(
        &client,
        "grug-write",
        json!({"category": "test", "path": "note1", "content": "hello world"}),
    )
    .await;
    assert!(
        result.contains("created") || result.contains("updated"),
        "write result: {result}"
    );

    // 2. grug-search
    let result = call(&client, "grug-search", json!({"query": "hello"})).await;
    assert!(
        result.contains("note1") || result.contains("hello"),
        "search result: {result}"
    );

    // 3. grug-read (list all brains)
    let result = call(&client, "grug-read", json!({})).await;
    assert!(
        result.contains("memories"),
        "read result: {result}"
    );

    // 4. grug-read (specific file)
    let result = call(
        &client,
        "grug-read",
        json!({"brain": "memories", "category": "test", "path": "note1"}),
    )
    .await;
    assert!(
        result.contains("hello world"),
        "read file result: {result}"
    );

    // 5. grug-recall
    let result = call(&client, "grug-recall", json!({})).await;
    assert!(
        result.contains("note1") || result.contains("test"),
        "recall result: {result}"
    );

    // 6. grug-config list
    let result = call(&client, "grug-config", json!({"action": "list"})).await;
    assert!(
        result.contains("memories"),
        "config list result: {result}"
    );

    // 7. grug-sync
    let result = call(&client, "grug-sync", json!({})).await;
    assert!(
        result.contains("memories") || result.contains("synced") || result.contains("indexed"),
        "sync result: {result}"
    );

    // 8. grug-dream
    let result = call(&client, "grug-dream", json!({})).await;
    // Dream should work (may show various sections)
    assert!(!result.is_empty(), "dream result should not be empty");

    // 9. grug-docs
    let result = call(&client, "grug-docs", json!({})).await;
    // Docs should work (may say no documentation brains)
    assert!(!result.is_empty(), "docs result should not be empty");

    // 10. grug-delete
    let result = call(
        &client,
        "grug-delete",
        json!({"category": "test", "path": "note1"}),
    )
    .await;
    assert!(
        result.contains("deleted"),
        "delete result: {result}"
    );

    handle.abort();
}

/// DW-3.3: Multiple concurrent connections work without interference.
#[tokio::test]
async fn test_concurrent_connections() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    // Write some data first
    {
        let mut client = SocketClient::connect(&socket_path).await.unwrap();
        client
            .call(
                "grug-write",
                json!({"category": "test", "path": "shared", "content": "shared data"}),
            )
            .await
            .unwrap();
    }

    // Spawn 5 concurrent clients that all search
    let mut handles = Vec::new();
    for i in 0..5 {
        let path = socket_path.clone();
        handles.push(tokio::spawn(async move {
            let mut client = SocketClient::connect(&path).await.unwrap();
            let result = client
                .call("grug-search", json!({"query": "shared"}))
                .await
                .unwrap();
            assert!(
                result.contains("shared"),
                "concurrent client {i} failed: {result}"
            );
            result
        }));
    }

    // All should succeed
    for (i, h) in handles.into_iter().enumerate() {
        let result = h.await;
        assert!(result.is_ok(), "concurrent client {i} panicked");
    }

    handle.abort();
}

/// DW-3.4: Clean error message when server not running.
#[tokio::test]
async fn test_connect_error_when_server_not_running() {
    let result = SocketClient::connect(std::path::Path::new(
        "/tmp/nonexistent-grug-test-sock-12345.sock",
    ))
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("grug serve"),
        "error should mention grug serve: {err}"
    );
}

/// DW-3.5: Server removes stale socket file on startup.
#[tokio::test]
async fn test_stale_socket_cleanup() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");

    // Create a fake stale socket file
    fs::write(&socket_path, "stale").unwrap();
    assert!(socket_path.exists());

    // Server should start successfully by removing the stale file
    let handle = start_server(socket_path.clone(), db_path, config).await;

    // Should be able to connect
    let mut client = SocketClient::connect(&socket_path).await.unwrap();
    let result = client
        .call("grug-search", json!({"query": "test"}))
        .await
        .unwrap();
    assert!(!result.is_empty());

    handle.abort();
}

/// DW-3.6: Exercise write then read through the socket to verify end-to-end.
#[tokio::test]
async fn test_write_then_read_through_socket() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    let mut client = SocketClient::connect(&socket_path).await.unwrap();

    // Write a memory
    let write_result = client
        .call(
            "grug-write",
            json!({
                "category": "integration",
                "path": "socket-test",
                "content": "---\nname: socket-test\ndescription: Integration test memory\n---\nThis memory was written through the Unix socket."
            }),
        )
        .await
        .unwrap();
    assert!(write_result.contains("created"), "write: {write_result}");

    // Search for it
    let search_result = client
        .call("grug-search", json!({"query": "socket"}))
        .await
        .unwrap();
    assert!(
        search_result.contains("socket-test"),
        "search: {search_result}"
    );

    // Read it back
    let read_result = client
        .call(
            "grug-read",
            json!({"brain": "memories", "category": "integration", "path": "socket-test"}),
        )
        .await
        .unwrap();
    assert!(
        read_result.contains("Unix socket"),
        "read: {read_result}"
    );

    handle.abort();
}

/// Test that unknown tools return a proper error.
#[tokio::test]
async fn test_unknown_tool_error() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    let mut client = SocketClient::connect(&socket_path).await.unwrap();
    let result = client.call("grug-nonexistent", json!({})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown tool"));

    handle.abort();
}

/// Test multiple sequential requests on the same connection.
#[tokio::test]
async fn test_sequential_requests_same_connection() {
    let (tmp, socket_path, config) = setup_test_env();
    let db_path = tmp.path().join("grug.db");
    let handle = start_server(socket_path.clone(), db_path, config).await;

    let mut client = SocketClient::connect(&socket_path).await.unwrap();

    // Write 3 different memories
    for i in 0..3 {
        let result = client
            .call(
                "grug-write",
                json!({
                    "category": "sequential",
                    "path": format!("note-{i}"),
                    "content": format!("Content number {i}")
                }),
            )
            .await
            .unwrap();
        assert!(
            result.contains("created"),
            "write {i} failed: {result}"
        );
    }

    // Recall should show all 3 (or 2 most recent per category)
    let result = client
        .call(
            "grug-recall",
            json!({"category": "sequential"}),
        )
        .await
        .unwrap();
    assert!(
        result.contains("note-"),
        "recall should show notes: {result}"
    );

    handle.abort();
}

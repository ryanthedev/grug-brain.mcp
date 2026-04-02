# Pseudocode: Phase 3 - Unix Socket Server + MCP Stdio Client

## DW Verification

| DW-ID | Done-When Item | Status | Pseudocode Section |
|-------|---------------|--------|-------------------|
| DW-3.1 | `grug serve` starts, creates socket at `~/.grug-brain/grug.sock`, accepts connections | COVERED | protocol.rs, server.rs |
| DW-3.2 | `grug --stdio` connects to socket and correctly bridges all 9 MCP tools | COVERED | client.rs, protocol.rs |
| DW-3.3 | Multiple concurrent `--stdio` sessions work without interference | COVERED | server.rs (per-connection spawn, shared db via channel) |
| DW-3.4 | Clean error message when `--stdio` can't connect (server not running) | COVERED | client.rs |
| DW-3.5 | Server removes stale socket file on startup | COVERED | server.rs |
| DW-3.6 | Integration tests: start server, connect client, exercise all tools through the socket | COVERED | Integration tests section |

**All items COVERED:** YES

## Files to Create/Modify
- `src/protocol.rs` — NEW: wire protocol types for Unix socket communication
- `src/server.rs` — NEW: Unix socket listener, DB thread, tool dispatch
- `src/client.rs` — NEW: MCP stdio bridge (rmcp ServerHandler that forwards to socket)
- `src/main.rs` — REWRITE: clap CLI with `serve` and `--stdio` subcommands
- `src/lib.rs` — MODIFY: add `pub mod protocol; pub mod server; pub mod client;`
- `Cargo.toml` — MODIFY: add `uuid` dependency for request IDs

## Design Notes

### Key Architecture Decision: DB Thread Pattern

`rusqlite::Connection` is `!Send`, so we cannot share `GrugDb` across tasks.
Solution: a single dedicated thread owns `GrugDb` and receives tool requests via
a `tokio::sync::mpsc` channel. The server spawns a std::thread that runs a
synchronous loop pulling from a receiver. Each socket connection sends requests
through the channel and receives responses via a oneshot channel.

This means:
- Single writer to SQLite (no WAL contention)
- Connection stays on one thread (satisfies `!Send`)
- Concurrent socket connections are handled by tokio tasks
- Each connection sends (tool_name, params, oneshot_tx) to the DB thread
- DB thread calls the appropriate tool function and sends result back

### Request ID Strategy

Use a simple string UUID for each request. The client generates the ID, sends it
with the request, server echoes it back in the response. This allows pipelining
(though we won't pipeline in v1). Use `uuid` crate v1 for ID generation.

---

## Pseudocode

### Cargo.toml [DW-3.1, DW-3.2]

Add dependency:
```
uuid = { version = "1", features = ["v4"] }
```

### src/protocol.rs [DW-3.1, DW-3.2]

Wire protocol types for newline-delimited JSON over Unix socket.

```
/// Request from client to server over Unix socket
struct SocketRequest {
    id: String,         // UUID for request/response correlation
    tool: String,       // tool name, e.g. "grug-search"
    params: Value,      // JSON object of tool parameters
}

/// Response from server to client over Unix socket
struct SocketResponse {
    id: String,         // echoed from request
    result: Option<String>,  // tool output text (on success)
    error: Option<String>,   // error message (on failure)
}

// Both derive Serialize, Deserialize
// Both are sent as single-line JSON terminated by newline ('\n')
```

### src/server.rs [DW-3.1, DW-3.3, DW-3.5]

Unix socket server that owns the database.

```
/// Message sent to the DB worker thread
struct DbRequest {
    tool: String,
    params: Value,
    reply: oneshot::Sender<Result<String, String>>,
}

/// Start the DB worker thread. Returns a sender for submitting requests.
fn spawn_db_thread(db_path: &Path, config: BrainConfig) -> mpsc::Sender<DbRequest>
    // Create an mpsc channel (bounded, capacity 64)
    // Spawn a std::thread (not tokio task) that:
    //   1. Opens GrugDb::open(db_path, config)
    //   2. Loops: recv from channel
    //   3. For each DbRequest: dispatch_tool(&mut db, &req.tool, &req.params)
    //   4. Send result back via req.reply oneshot
    //   5. Break when channel closes (all senders dropped)
    // Return the sender

/// Dispatch a tool call to the appropriate function.
fn dispatch_tool(db: &mut GrugDb, tool: &str, params: &Value) -> Result<String, String>
    // Match on tool name:
    // "grug-search" => extract query, page from params, call grug_search(db, ...)
    //   grug_search returns String, wrap in Ok()
    // "grug-write" => extract category, path, content, brain, call grug_write(db, ...)
    // "grug-read" => extract brain, category, path, call grug_read(db, ...)
    // "grug-recall" => extract category, brain, call grug_recall(db, ...)
    // "grug-delete" => extract category, path, brain, call grug_delete(db, ...)
    // "grug-config" => extract action + all optional fields, call grug_config(db, ...)
    // "grug-sync" => extract brain, call grug_sync(db, ...)
    // "grug-dream" => call grug_dream(db)
    // "grug-docs" => extract category, path, page, call grug_docs(db, ...)
    // _ => Err("unknown tool: {tool}")
    //
    // Helper: extract_str(params, "field") -> Option<&str>
    // Helper: extract_u64(params, "field") -> Option<u64>
    // Helper: extract_bool(params, "field") -> Option<bool>

/// Default socket path: ~/.grug-brain/grug.sock
fn default_socket_path() -> PathBuf
    expand_home("~/.grug-brain/grug.sock")

/// Default PID file path: ~/.grug-brain/grug.pid
fn default_pid_path() -> PathBuf
    expand_home("~/.grug-brain/grug.pid")

/// Remove stale socket file if it exists. [DW-3.5]
fn cleanup_stale_socket(path: &Path)
    if path.exists():
        // Try to connect to verify if a server is running
        // If connect fails => stale, remove the file
        // If connect succeeds => another server is running, abort with error
        // Actually simpler: check PID file. If PID file exists and process is alive, abort.
        // Otherwise, remove socket file.
        remove_file(path)

/// Write PID file
fn write_pid_file(path: &Path)
    write path with current process PID (std::process::id())

/// Remove PID file on shutdown
fn remove_pid_file(path: &Path)
    remove_file(path) if it exists

/// Handle a single client connection. [DW-3.3]
async fn handle_connection(stream: UnixStream, db_tx: mpsc::Sender<DbRequest>)
    // Split stream into reader/writer
    let (reader, writer) = stream.into_split()
    let reader = BufReader::new(reader)
    // Read lines (newline-delimited JSON)
    for each line from reader.lines():
        // Parse line as SocketRequest
        let req: SocketRequest = serde_json::from_str(&line)?
        // Create oneshot channel for response
        let (tx, rx) = oneshot::channel()
        // Send to DB thread
        db_tx.send(DbRequest { tool: req.tool, params: req.params, reply: tx }).await?
        // Wait for response
        let result = rx.await?
        // Build SocketResponse
        let resp = match result:
            Ok(text) => SocketResponse { id: req.id, result: Some(text), error: None }
            Err(e) => SocketResponse { id: req.id, result: None, error: Some(e) }
        // Write response as JSON + newline
        writer.write_all(serde_json::to_string(&resp)?.as_bytes()).await?
        writer.write_all(b"\n").await?
        writer.flush().await?

/// Main server entry point. [DW-3.1]
pub async fn run_server(socket_path: Option<PathBuf>) -> Result<(), String>
    let path = socket_path.unwrap_or_else(default_socket_path)
    // Ensure parent directory exists
    ensure parent of path exists (create_dir_all)
    // Clean up stale socket [DW-3.5]
    cleanup_stale_socket(&path)
    // Write PID file
    write_pid_file(&default_pid_path())
    // Load brain config
    let config = load_brains()?
    // Determine DB path
    let db_path = expand_home("~/.grug-brain/grug.db")
    // Start DB worker thread
    let db_tx = spawn_db_thread(&db_path, config)
    // Bind Unix socket listener
    let listener = UnixListener::bind(&path)?
    eprintln!("grug serve: listening on {}", path.display())
    // Set up graceful shutdown on SIGTERM/SIGINT
    let shutdown = signal::ctrl_c() // or tokio signal
    // Accept loop
    loop:
        select!:
            Ok((stream, _)) = listener.accept() =>
                let db_tx = db_tx.clone()
                tokio::spawn(handle_connection(stream, db_tx))
            _ = shutdown =>
                break
    // Cleanup
    remove_pid_file(&default_pid_path())
    remove_file(&path)  // remove socket on clean shutdown
    drop(db_tx)  // closes channel, DB thread will exit
```

### src/client.rs [DW-3.2, DW-3.4]

MCP stdio bridge. Implements rmcp's ServerHandler to present tools to Claude Code,
but forwards every call_tool to the Unix socket server.

```
use rmcp::{ServerHandler, tool, tool_router, tool_handler}
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters}

/// Parameters for each tool — one struct per tool with schemars JsonSchema
/// These define the MCP tool schemas that Claude Code sees.

struct SearchParams { query: String, page: Option<u64> }
struct WriteParams { category: String, path: String, content: String, brain: Option<String> }
struct ReadParams { brain: Option<String>, category: Option<String>, path: Option<String> }
struct RecallParams { category: Option<String>, brain: Option<String> }
struct DeleteParams { category: String, path: String, brain: Option<String> }
struct ConfigParams { action: String, name: Option<String>, dir: Option<String>,
                      primary: Option<bool>, writable: Option<bool>, flat: Option<bool>,
                      git: Option<String>, sync_interval: Option<u64>,
                      source: Option<String>, refresh_interval: Option<u64> }
struct SyncParams { brain: Option<String> }
struct DreamParams {}  // no parameters
struct DocsParams { category: Option<String>, path: Option<String>, page: Option<u64> }

/// Socket client wrapper — holds a connected UnixStream.
/// Each GrugClient instance connects once and holds the connection.
struct SocketClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl SocketClient {
    /// Connect to the server socket. [DW-3.4]
    async fn connect(path: &Path) -> Result<Self, String>
        let stream = UnixStream::connect(path).await
            .map_err(|e| format!(
                "grug: cannot connect to server at {} — is `grug serve` running?\n  error: {e}",
                path.display()
            ))?
        let (reader, writer) = stream.into_split()
        Ok(Self { reader: BufReader::new(reader), writer })

    /// Send a tool call and wait for the response.
    async fn call(&mut self, tool: &str, params: Value) -> Result<String, String>
        let id = uuid::Uuid::new_v4().to_string()
        let req = SocketRequest { id: id.clone(), tool: tool.to_string(), params }
        let line = serde_json::to_string(&req).map_err(|e| e.to_string())?
        self.writer.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?
        self.writer.write_all(b"\n").await.map_err(|e| e.to_string())?
        self.writer.flush().await.map_err(|e| e.to_string())?
        // Read response line
        let mut response_line = String::new()
        self.reader.read_line(&mut response_line).await.map_err(|e| e.to_string())?
        let resp: SocketResponse = serde_json::from_str(&response_line).map_err(|e| e.to_string())?
        if resp.id != id:
            return Err("response ID mismatch".to_string())
        match (resp.result, resp.error):
            (Some(text), _) => Ok(text)
            (_, Some(err)) => Err(err)
            _ => Err("empty response from server".to_string())
}

/// The MCP server handler that Claude Code talks to.
/// Implements ServerHandler via rmcp macros.
/// Each tool method forwards to the socket via a shared SocketClient.
struct GrugMcp {
    tool_router: ToolRouter<Self>,
    socket: Arc<Mutex<SocketClient>>,  // tokio::sync::Mutex for async
}

impl GrugMcp {
    async fn new(socket_path: &Path) -> Result<Self, String>
        let client = SocketClient::connect(socket_path).await?
        Ok(Self {
            tool_router: Self::tool_router(),
            socket: Arc::new(Mutex::new(client)),
        })

    /// Helper: forward a tool call to the server
    async fn forward(&self, tool: &str, params: Value) -> String
        let mut sock = self.socket.lock().await
        match sock.call(tool, params).await:
            Ok(text) => text
            Err(e) => format!("error: {e}")
}

#[tool_router]
impl GrugMcp {
    #[tool(name = "grug-search", description = "Search memories by keyword (FTS5 full-text search)")]
    async fn search(&self, params: Parameters<SearchParams>) -> String
        self.forward("grug-search", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-write", description = "Store a new memory")]
    async fn write(&self, params: Parameters<WriteParams>) -> String
        self.forward("grug-write", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-read", description = "Read a specific memory by category/path")]
    async fn read(&self, params: Parameters<ReadParams>) -> String
        self.forward("grug-read", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-recall", description = "Get up to speed at the start of a conversation")]
    async fn recall(&self, params: Parameters<RecallParams>) -> String
        self.forward("grug-recall", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-delete", description = "Remove a memory")]
    async fn delete(&self, params: Parameters<DeleteParams>) -> String
        self.forward("grug-delete", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-config", description = "Manage brain configuration")]
    async fn config(&self, params: Parameters<ConfigParams>) -> String
        self.forward("grug-config", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-sync", description = "Reindex a brain from disk")]
    async fn sync(&self, params: Parameters<SyncParams>) -> String
        self.forward("grug-sync", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-dream", description = "Review memory health across all brains")]
    async fn dream(&self, params: Parameters<DreamParams>) -> String
        self.forward("grug-dream", serde_json::to_value(&params.0).unwrap()).await

    #[tool(name = "grug-docs", description = "[Deprecated: use grug-read] Browse documentation brains")]
    async fn docs(&self, params: Parameters<DocsParams>) -> String
        self.forward("grug-docs", serde_json::to_value(&params.0).unwrap()).await
}

#[tool_handler]
impl ServerHandler for GrugMcp {
    fn get_info(&self) -> ServerInfo
        ServerInfo {
            name: "grug-brain".to_string(),
            version: "0.1.0".to_string(),
            ..Default::default()
        }
}

/// Main client entry point.
pub async fn run_stdio(socket_path: Option<PathBuf>) -> Result<(), String>
    let path = socket_path.unwrap_or_else(default_socket_path)
    // Connect to server (fails with clean message if not running) [DW-3.4]
    let mcp = GrugMcp::new(&path).await?
    // Serve MCP over stdio
    let (stdin, stdout) = rmcp::transport::io::stdio()
    let service = mcp.serve((stdin, stdout)).await
        .map_err(|e| format!("grug: MCP initialization failed: {e}"))?
    // Wait for the service to complete (Claude Code will close stdin when done)
    service.waiting().await
        .map_err(|e| format!("grug: MCP service error: {e}"))?
    Ok(())
```

### src/main.rs [DW-3.1, DW-3.2]

CLI entry point using clap.

```
use clap::{Parser, Subcommand}

#[derive(Parser)]
#[command(name = "grug", version, about = "grug-brain memory server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run as MCP stdio client (connects to running server)
    #[arg(long)]
    stdio: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the grug-brain server
    Serve {
        /// Custom socket path (default: ~/.grug-brain/grug.sock)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main()
    let cli = Cli::parse()

    if cli.stdio:
        // MCP stdio mode
        if let Err(e) = run_stdio(None).await:
            eprintln!("{e}")
            std::process::exit(1)
        return

    match cli.command:
        Some(Commands::Serve { socket }) =>
            if let Err(e) = run_server(socket).await:
                eprintln!("{e}")
                std::process::exit(1)
        None =>
            // No command and no --stdio: print help
            Cli::parse_from(["grug", "--help"])
```

### src/lib.rs [DW-3.1, DW-3.2]

Add new module declarations:

```
pub mod client;
pub mod protocol;
pub mod server;
```

---

## Integration Tests [DW-3.6]

File: `tests/integration.rs`

```
/// Test setup helper: start server on a temp socket, return (socket_path, server_handle)
async fn start_test_server() -> (PathBuf, JoinHandle<()>)
    let tmp = TempDir::new()
    let sock_path = tmp.path().join("test.sock")
    let db_path = tmp.path().join("grug.db")
    // Create a minimal brains.json
    let brain_dir = tmp.path().join("memories")
    create_dir_all(&brain_dir)
    let config_path = tmp.path().join("brains.json")
    write config_path with [{"name":"memories","dir":"<brain_dir>","primary":true}]
    // Start server in background task
    // (Need to pass custom paths — may need to refactor run_server slightly
    //  or expose lower-level start function)

/// Test: server starts and accepts connections [DW-3.1]
#[tokio::test]
async fn test_server_starts_and_accepts()
    start server
    connect UnixStream to socket path
    send a grug-search request
    verify response is valid JSON with matching id

/// Test: all 9 tools work through socket [DW-3.2, DW-3.6]
#[tokio::test]
async fn test_all_tools_through_socket()
    start server
    connect, send each tool:
    - grug-write (category, path, content)
    - grug-search (query)
    - grug-read (brain, category, path)
    - grug-recall (no args)
    - grug-delete (category, path)
    - grug-config (action: "list")
    - grug-sync (no args)
    - grug-dream (no args)
    - grug-docs (no args)
    verify each returns a result (not an error)

/// Test: concurrent connections [DW-3.3]
#[tokio::test]
async fn test_concurrent_connections()
    start server
    // First write some data
    connect client A, send grug-write
    // Spawn multiple concurrent reads
    let handles = (0..5).map(|_| {
        tokio::spawn(async {
            connect new client to socket
            send grug-search
            verify response
        })
    })
    join_all(handles).await
    // All should succeed without interference

/// Test: stale socket cleanup [DW-3.5]
#[tokio::test]
async fn test_stale_socket_cleanup()
    create a file at the socket path (simulating stale socket)
    start server (should succeed by removing stale file)
    connect and verify it works

/// Test: clean error when server not running [DW-3.4]
#[tokio::test]
async fn test_connect_error_when_server_not_running()
    let path = PathBuf::from("/tmp/nonexistent-grug-test.sock")
    let result = SocketClient::connect(&path).await
    assert result is Err
    assert error message contains "is `grug serve` running?"
```

---

## Design Notes

### Why Not rmcp #[tool_router] on the Server Side?

The Unix socket server does NOT use rmcp. It speaks our own newline-delimited JSON protocol.
rmcp is only used on the `--stdio` client side to implement the MCP protocol that Claude Code
speaks. The server is a plain tokio `UnixListener` with JSON lines.

### Concurrency Model

```
Claude A  --stdio-->  grug --stdio (process A)  --unix socket-->  grug serve
Claude B  --stdio-->  grug --stdio (process B)  --unix socket-->     |
                                                                     v
                                                              [DB worker thread]
                                                              GrugDb (single owner)
```

Each `--stdio` process connects to the server via a persistent Unix socket connection.
The server spawns a tokio task per connection. All tasks send requests to the single DB
worker thread via an mpsc channel. Responses come back via oneshot channels.

This guarantees:
- No concurrent SQLite access (single writer)
- No blocking the tokio runtime with DB operations (separate thread)
- No interference between sessions (each has its own connection and response channel)

### Error Propagation

Tool functions return `Result<String, String>` (or `String`). The socket protocol
carries both success and error as `SocketResponse` fields. The MCP client converts:
- `Ok(text)` -> tool result text (is_error: false)
- `Err(msg)` -> tool result text with the error message (rmcp's default String return
  already wraps this in a CallToolResult with text content)

### Parameter Serialization

Tool params go through two serialization steps:
1. Claude Code sends JSON params -> rmcp deserializes into typed struct (e.g., SearchParams)
2. Client re-serializes to JSON Value and sends over socket
3. Server extracts fields from the JSON Value to call the tool function

This double-serialization is intentional: it validates params on the client side (via schemars)
before forwarding, and keeps the server protocol simple (just JSON values).

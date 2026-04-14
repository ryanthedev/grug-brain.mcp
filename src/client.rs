use crate::protocol::{SocketRequest, SocketResponse};
use crate::server::default_socket_path;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Tool parameter structs — these define the MCP schemas Claude Code sees.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchParams {
    /// Search terms.
    pub query: String,
    /// Page number (20 results per page).
    pub page: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct WriteParams {
    /// Folder to store in, e.g. loopback, feedback, react-native.
    pub category: String,
    /// Filename for the memory, e.g. no-db-mocks.
    pub path: String,
    /// Memory content in markdown.
    pub content: String,
    /// Brain to write to (defaults to primary brain).
    pub brain: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReadParams {
    /// Brain name to browse (omit to list all brains).
    pub brain: Option<String>,
    /// Category to browse or read from.
    pub category: Option<String>,
    /// Filename within the category to read.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RecallParams {
    /// Filter to a specific category.
    pub category: Option<String>,
    /// Brain to recall from (defaults to primary brain).
    pub brain: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DeleteParams {
    /// Category the memory is in.
    pub category: String,
    /// Filename to delete.
    pub path: String,
    /// Brain to delete from (defaults to primary brain).
    pub brain: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct EditEntry {
    /// Exact text to find.
    pub old: String,
    /// Replacement text.
    pub new: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct UpdateParams {
    /// Category the memory is in.
    pub category: String,
    /// Filename to update.
    pub path: String,
    /// List of edits to apply sequentially. Each edit replaces the first occurrence of `old` with `new`.
    pub edits: Vec<EditEntry>,
    /// Brain to update in (defaults to primary brain).
    pub brain: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ConfigParams {
    /// Config action to perform.
    pub action: String,
    /// Brain name (required for add/remove).
    pub name: Option<String>,
    /// Brain directory (required for add).
    pub dir: Option<String>,
    /// Mark as primary brain (add only, default false).
    pub primary: Option<bool>,
    /// Mark as writable (add only, default true).
    pub writable: Option<bool>,
    /// Flat layout (add only, default false).
    pub flat: Option<bool>,
    /// Git remote URL (add only, optional).
    pub git: Option<String>,
    /// Sync interval in seconds (add only, default 60).
    pub sync_interval: Option<u64>,
    /// Source identifier for doc refresh (add only).
    pub source: Option<String>,
    /// Auto-refresh interval in seconds for read-only brains (add only, minimum 3600).
    pub refresh_interval: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SyncParams {
    /// Brain to reindex (omit to reindex all brains).
    pub brain: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DreamParams {}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DocsParams {
    /// Doc category to browse.
    pub category: Option<String>,
    /// File path to read.
    pub path: Option<String>,
    /// Page number for long files.
    pub page: Option<u64>,
}

// ---------------------------------------------------------------------------
// Socket client — persistent connection to the server.
// ---------------------------------------------------------------------------

/// Holds a connected Unix socket to the grug-brain server.
pub struct SocketClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl SocketClient {
    /// Connect to the server socket.
    /// Returns a clean error message when the server is not running.
    pub async fn connect(path: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(path).await.map_err(|e| {
            format!(
                "grug: cannot connect to server at {} \u{2014} is `grug serve` running?\n  error: {e}",
                path.display()
            )
        })?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    /// Send a tool call and wait for the response.
    pub async fn call(&mut self, tool: &str, params: serde_json::Value) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let req = SocketRequest {
            id: id.clone(),
            tool: tool.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("socket write failed: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| format!("socket flush failed: {e}"))?;

        let mut response_line = String::new();
        self.reader
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("socket read failed: {e}"))?;

        if response_line.is_empty() {
            return Err("server closed connection".to_string());
        }

        let resp: SocketResponse =
            serde_json::from_str(&response_line).map_err(|e| format!("invalid response: {e}"))?;

        if resp.id != id {
            return Err(format!(
                "response ID mismatch: expected {id}, got {}",
                resp.id
            ));
        }

        match (resp.result, resp.error) {
            (Some(text), _) => Ok(text),
            (_, Some(err)) => Err(err),
            _ => Err("empty response from server".to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP server handler — the struct rmcp presents to Claude Code.
// ---------------------------------------------------------------------------

/// MCP server handler that forwards all tool calls to the grug-brain server.
#[derive(Debug, Clone)]
pub struct GrugMcp {
    tool_router: ToolRouter<Self>,
    socket: Arc<Mutex<SocketClient>>,
}

impl GrugMcp {
    /// Create a new MCP handler connected to the server.
    pub async fn new(socket_path: &Path) -> Result<Self, String> {
        let client = SocketClient::connect(socket_path).await?;
        Ok(Self {
            tool_router: Self::tool_router(),
            socket: Arc::new(Mutex::new(client)),
        })
    }

    /// Forward a tool call to the server.
    async fn forward(&self, tool: &str, params: serde_json::Value) -> String {
        let mut sock = self.socket.lock().await;
        match sock.call(tool, params).await {
            Ok(text) => text,
            Err(e) => format!("error: {e}"),
        }
    }
}

// We need Debug for SocketClient to satisfy derive(Debug) on GrugMcp.
impl std::fmt::Debug for SocketClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketClient").finish()
    }
}

#[tool_router]
impl GrugMcp {
    #[tool(
        name = "grug-search",
        description = "Search across all brains. BM25 ranked, porter stemming. Results show [category] [brain] tags — use these when calling grug-read or grug-recall."
    )]
    async fn search(&self, params: Parameters<SearchParams>) -> String {
        self.forward("grug-search", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-write",
        description = "Store a memory. Saved as markdown with frontmatter, indexed for search. Add sync: false to frontmatter to keep local-only."
    )]
    async fn write(&self, params: Parameters<WriteParams>) -> String {
        self.forward("grug-write", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-read",
        description = "Read and browse brains. No args = list all brains. Brain only = list categories. Brain + category = list files. Brain + category + path = read file. Omitting brain searches primary brain first."
    )]
    async fn read(&self, params: Parameters<ReadParams>) -> String {
        self.forward("grug-read", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-recall",
        description = "Get up to speed. Shows 2 most recent per category across ALL brains. Specify brain to filter to one brain. Results show [brain] tags for non-primary entries."
    )]
    async fn recall(&self, params: Parameters<RecallParams>) -> String {
        self.forward("grug-recall", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-delete",
        description = "Delete a memory."
    )]
    async fn delete(&self, params: Parameters<DeleteParams>) -> String {
        self.forward("grug-delete", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-update",
        description = "Edit a memory in place. Applies substring find-and-replace edits sequentially. All edits are validated before writing — if any old string is not found, no changes are made."
    )]
    async fn update(&self, params: Parameters<UpdateParams>) -> String {
        self.forward("grug-update", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-config",
        description = "Manage brain configuration. list: show all brains. add: create a new brain entry. remove: delete a brain entry (cannot remove the primary brain)."
    )]
    async fn config(&self, params: Parameters<ConfigParams>) -> String {
        self.forward("grug-config", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-sync",
        description = "Reindex a brain (or all brains) from disk. Use after adding files outside of grug-write."
    )]
    async fn sync(&self, params: Parameters<SyncParams>) -> String {
        self.forward("grug-sync", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-dream",
        description = "Dream: review memory health across all brains. Finds cross-links, flags stale memories and conflicts."
    )]
    async fn dream(&self, params: Parameters<DreamParams>) -> String {
        self.forward("grug-dream", serde_json::to_value(&params.0).unwrap())
            .await
    }

    #[tool(
        name = "grug-docs",
        description = "[Deprecated: use grug-read] Browse documentation brains (non-primary brains)."
    )]
    async fn docs(&self, params: Parameters<DocsParams>) -> String {
        self.forward("grug-docs", serde_json::to_value(&params.0).unwrap())
            .await
    }
}

#[tool_handler]
impl ServerHandler for GrugMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("grug-brain", env!("CARGO_PKG_VERSION")))
    }
}

/// Run the MCP stdio client, bridging stdio to the server via Unix socket.
pub async fn run_stdio(socket_path: Option<PathBuf>) -> Result<(), String> {
    use rmcp::ServiceExt;

    let path = socket_path.unwrap_or_else(default_socket_path);
    let mcp = GrugMcp::new(&path).await?;

    let (stdin, stdout) = rmcp::transport::io::stdio();
    let service = mcp
        .serve((stdin, stdout))
        .await
        .map_err(|e| format!("grug: MCP initialization failed: {e}"))?;

    service
        .waiting()
        .await
        .map_err(|e| format!("grug: MCP service error: {e}"))?;

    Ok(())
}

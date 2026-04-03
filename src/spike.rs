//! Compile-time spike for rmcp transport-io forwarding pattern.
//!
//! This module proves that:
//! 1. rmcp's `#[tool]` macro expands correctly
//! 2. `#[tool_router]` and `#[tool_handler]` wire up `ServerHandler`
//! 3. The `serve` pattern compiles with tokio duplex transport
//!
//! This is NOT used at runtime in Phase 1. It will be replaced by the real
//! server implementation in Phase 3.

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for the stub tool.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct StubParams {
    /// The content to echo.
    pub content: String,
}

/// Stub MCP server to verify rmcp compile.
#[derive(Debug, Clone)]
pub struct GrugStub {
    tool_router: ToolRouter<Self>,
}

impl GrugStub {
    /// Create a new stub server.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for GrugStub {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl GrugStub {
    /// Stub tool that echoes input. Used only to verify rmcp macro expansion.
    #[tool(name = "grug-stub", description = "Stub tool for compile check")]
    pub async fn stub_tool(&self, params: Parameters<StubParams>) -> String {
        format!("echo: {}", params.0.content)
    }
}

#[tool_handler]
impl ServerHandler for GrugStub {}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ClientHandler;
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ClientInfo};

    #[derive(Debug, Clone, Default)]
    struct StubClient;

    impl ClientHandler for StubClient {
        fn get_info(&self) -> ClientInfo {
            ClientInfo::default()
        }
    }

    #[test]
    fn test_spike_types_exist() {
        // If this compiles, the rmcp macros expanded correctly
        let server = GrugStub::new();
        let _attr = GrugStub::stub_tool_tool_attr();
        assert_eq!(&*_attr.name, "grug-stub");
        drop(server);
    }

    #[tokio::test]
    async fn test_spike_serve_compiles() {
        // Verify the full serve pattern works with tokio duplex
        let (server_transport, client_transport) = tokio::io::duplex(4096);

        let server = GrugStub::new();
        let server_handle = tokio::spawn(async move {
            server.serve(server_transport).await.unwrap().waiting().await.unwrap();
        });

        let client = StubClient::default();
        let client_peer = client.serve(client_transport).await.unwrap();

        // Call the stub tool
        let result = client_peer
            .call_tool(
                CallToolRequestParams::new("grug-stub").with_arguments(
                    serde_json::json!({ "content": "hello" })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();

        let text = result
            .content
            .first()
            .and_then(|c| c.raw.as_text())
            .map(|t| t.text.as_str())
            .unwrap();
        assert_eq!(text, "echo: hello");

        client_peer.cancel().await.unwrap();
        server_handle.await.unwrap();
    }
}

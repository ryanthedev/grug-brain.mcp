use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request from client to server over Unix socket.
/// Serialized as a single JSON line terminated by '\n'.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketRequest {
    /// Unique request ID for correlation.
    pub id: String,
    /// Tool name, e.g. "grug-search".
    pub tool: String,
    /// Tool parameters as a JSON object.
    pub params: Value,
}

/// Response from server to client over Unix socket.
/// Serialized as a single JSON line terminated by '\n'.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketResponse {
    /// Echoed from the request.
    pub id: String,
    /// Tool output text on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error message on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SocketResponse {
    /// Create a success response.
    pub fn ok(id: String, text: String) -> Self {
        Self {
            id,
            result: Some(text),
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(id: String, msg: String) -> Self {
        Self {
            id,
            result: None,
            error: Some(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_roundtrip() {
        let req = SocketRequest {
            id: "abc-123".to_string(),
            tool: "grug-search".to_string(),
            params: serde_json::json!({"query": "foo"}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: SocketRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "abc-123");
        assert_eq!(parsed.tool, "grug-search");
        assert_eq!(parsed.params["query"], "foo");
    }

    #[test]
    fn test_response_ok_roundtrip() {
        let resp = SocketResponse::ok("abc-123".to_string(), "found 5 results".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "abc-123");
        assert_eq!(parsed.result.as_deref(), Some("found 5 results"));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_response_err_roundtrip() {
        let resp = SocketResponse::err("def-456".to_string(), "unknown brain".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SocketResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "def-456");
        assert!(parsed.result.is_none());
        assert_eq!(parsed.error.as_deref(), Some("unknown brain"));
    }

    #[test]
    fn test_response_skip_none_fields() {
        let resp = SocketResponse::ok("id".to_string(), "ok".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        // error field should be absent (not "error": null)
        assert!(!json.contains("error"));
    }
}

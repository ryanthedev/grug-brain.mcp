//! Domain layer — operation ports (trait definitions) that both the MCP and
//! HTTP transports route through. Implementations live in `src/adapters/`
//! (Phase 2). For now this module is a contract-only map: read `ports.rs` to
//! see every operation the system exposes.
pub mod ports;

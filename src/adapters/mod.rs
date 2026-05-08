//! Adapter layer — concrete implementations of the domain ports.
//!
//! Currently houses a single backend (`sqlite`) that implements every port
//! on `GrugDb`. Future backends would slot in alongside as sibling
//! submodules.
pub mod sqlite;

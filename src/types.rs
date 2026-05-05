use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A single brain -- a directory of categorized markdown memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brain {
    pub name: String,
    pub dir: PathBuf,
    pub primary: bool,
    pub writable: bool,
    pub flat: bool,
    pub git: Option<String>,
    pub sync_interval: u64,
    /// Source URL for flat brains (e.g. "github:org/repo/path").
    /// Preserved for config round-trips but not used by the core engine.
    pub source: Option<String>,
    /// Auto-refresh interval in seconds for read-only brains.
    /// Only meaningful for non-writable brains with a source field.
    pub refresh_interval: Option<u64>,
}

/// Loaded and validated brain configuration.
#[derive(Debug, Clone)]
pub struct BrainConfig {
    pub brains: Vec<Brain>,
    /// Name of the primary brain (convenience accessor).
    pub primary: String,
    pub config_path: PathBuf,
    /// Last mtime of brains.json, for hot-reload detection.
    pub last_mtime: Option<f64>,
}

impl BrainConfig {
    /// Get the primary brain.
    pub fn primary_brain(&self) -> &Brain {
        self.brains
            .iter()
            .find(|b| b.name == self.primary)
            .expect("BrainConfig invariant: primary brain must exist")
    }

    /// Find a brain by name.
    pub fn get(&self, name: &str) -> Option<&Brain> {
        self.brains.iter().find(|b| b.name == name)
    }
}

/// A row from the brain_fts virtual table (all indexed fields).
#[derive(Debug, Clone)]
pub struct FtsRow {
    pub path: String,
    pub brain: String,
    pub category: String,
    pub name: String,
    pub date: String,
    pub description: String,
    pub body: String,
}

/// A search result with highlighted snippet and BM25 rank.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: String,
    pub brain: String,
    pub category: String,
    pub name: String,
    pub date: String,
    pub description: String,
    pub snippet: String,
    pub rank: f64,
}

/// A parsed memory file with all extracted metadata.
#[derive(Debug, Clone)]
pub struct Memory {
    pub brain: String,
    pub path: String,
    pub category: String,
    pub name: String,
    pub frontmatter: HashMap<String, String>,
    pub body: String,
    pub description: String,
    pub mtime: f64,
}

/// A recall row (subset of FtsRow used by recall/dream queries).
#[derive(Debug, Clone)]
pub struct RecallRow {
    pub path: String,
    pub brain: String,
    pub category: String,
    pub name: String,
    pub date: String,
    pub description: String,
}

/// A filesystem-originated change to a memory file. Emitted by the
/// `Watcher` after debouncing and self-write suppression. UI subscribers
/// (HTTP SSE in Phase 3) consume this enum.
///
/// `Lagged(u64)` is a sentinel that watcher subscribers receive when they
/// fall behind on the broadcast channel; it is NOT produced directly by the
/// watcher (the broadcast channel hands it back from `recv()`).
#[derive(Debug, Clone)]
pub enum MemoryEvent {
    /// File appeared on disk for the first time.
    Created { brain: String, path: String, mtime: f64 },
    /// File contents changed.
    Modified { brain: String, path: String, mtime: f64 },
    /// File no longer exists on disk.
    Deleted { brain: String, path: String },
    /// File renamed within the brain. `from` and `to` are brain-relative.
    Renamed { brain: String, from: String, to: String, mtime: f64 },
    /// Reload hint: a multi-file backend operation (e.g.
    /// rename-with-link-rewrite) touched several paths atomically. Carriers
    /// should reload all affected views in one batch instead of reacting to
    /// per-file events.
    Reload { brain: String, paths: Vec<String>, reason: String },
    /// Subscriber lagged: missed `n` events. Subscribers should reload state.
    Lagged(u64),
}

/// A document similar to a query document, with cosine similarity score.
#[derive(Debug, Clone)]
pub struct SimilarDoc {
    pub brain: String,
    pub path: String,
    pub category: String,
    pub name: String,
    pub score: f64,
}

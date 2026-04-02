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

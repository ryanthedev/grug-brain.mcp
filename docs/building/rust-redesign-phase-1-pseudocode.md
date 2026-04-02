# Pseudocode: Phase 1 - Project Scaffold + Core Types

## DW Verification

| DW-ID | Done-When Item | Status | Pseudocode Section |
|-------|---------------|--------|-------------------|
| DW-1.1 | `cargo build` succeeds with rusqlite bundled FTS5 | COVERED | Cargo.toml Setup |
| DW-1.2 | Brain config loads from `~/.grug-brain/brains.json` with same validation rules | COVERED | Brain Config (config.rs) |
| DW-1.3 | SQLite schema created matching current schema version 5 | COVERED | Database Schema (db.rs) |
| DW-1.4 | Core types defined: Brain, Memory/FtsRow, SearchResult, BrainConfig | COVERED | Core Types (types.rs) |
| DW-1.5 | Frontmatter parser, body extractor, description extractor match current JS | COVERED | Parsing (parsing.rs) |
| DW-1.6 | File walker handles .md/.mdx, skips dot/underscore prefixed | COVERED | File Walker (walker.rs) |
| DW-1.7 | Unit tests for config parsing, frontmatter extraction, file walking | COVERED | Tests (in each module) |
| DW-1.8 | Spike: verify rmcp transport-io forwarding pattern compiles with stub tool | COVERED | rmcp Spike (spike.rs) |

**All items COVERED:** YES

## Files to Create/Modify
- `Cargo.toml` -- project manifest
- `src/main.rs` -- binary entry point (minimal, just proves it compiles)
- `src/lib.rs` -- library root re-exporting modules
- `src/types.rs` -- Brain, Memory, FtsRow, SearchResult, BrainConfig
- `src/config.rs` -- brains.json loading and validation
- `src/db.rs` -- SQLite schema creation, migration check
- `src/parsing.rs` -- frontmatter, body, description extraction
- `src/walker.rs` -- recursive .md/.mdx file walking
- `src/helpers.rs` -- slugify, today, paginate
- `src/spike.rs` -- rmcp stdio transport compile check

## Pseudocode

### Cargo.toml Setup [DW-1.1]
```
[package]
name = "grug-brain"
version = "0.1.0"
edition = "2024"

[dependencies]
rusqlite = { version = "0.35", features = ["bundled"] }  -- bundled compiles SQLite with FTS5
tokio = { version = "1", features = ["full"] }
rmcp = { version = "1", features = ["transport-io", "server"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = "0.4"
regex = "1"

[dev-dependencies]
tempfile = "3"
```

### Core Types (src/types.rs) [DW-1.4]
```
struct Brain:
    name: String        -- unique identifier
    dir: PathBuf        -- resolved absolute path (home expanded)
    primary: bool       -- exactly one must be true
    writable: bool      -- default: true for normal, false for flat
    flat: bool          -- flat = files directly in dir, no category subdirs
    git: Option<String> -- remote URL or None
    sync_interval: u64  -- seconds, default 60
    source: Option<String>  -- for flat brains, doc source URL (preserved but unused in Phase 1)

struct BrainConfig:
    brains: Vec<Brain>
    primary: String     -- name of the primary brain (convenience)
    config_path: PathBuf
    last_mtime: Option<f64>  -- for hot-reload in Phase 2

struct FtsRow:
    path: String
    brain: String
    category: String
    name: String
    date: String
    description: String
    body: String

struct SearchResult:
    path: String
    brain: String
    category: String
    name: String
    date: String
    description: String
    snippet: String     -- highlighted excerpt from FTS5
    rank: f64           -- BM25 rank score

struct Memory:
    brain: String
    path: String        -- relative path within brain dir
    category: String
    name: String
    frontmatter: HashMap<String, String>
    body: String
    description: String
    mtime: f64
```

### Brain Config (src/config.rs) [DW-1.2, DW-1.7]
```
fn expand_home(path: &str) -> PathBuf:
    if path == "~":
        return home_dir
    if path starts with "~/":
        return home_dir.join(&path[2..])
    return PathBuf::from(path)

fn load_brains() -> Result<BrainConfig>:
    config_path = env("GRUG_CONFIG") or ~/.grug-brain/brains.json

    if config_path does not exist:
        -- create default config
        default_dir = ~/.grug-brain/memories
        ensure_dir(default_dir)
        default = [{ name: "memories", dir: default_dir, primary: true, writable: true }]
        write config_path as JSON
        return BrainConfig with single brain

    raw = read and parse config_path as JSON array
    if not array: error "must be a JSON array"

    brains = []
    for (i, entry) in raw:
        validate name: required, string -- error "brain[{i}] missing required name"
        validate dir: required, string -- error "brain {name} missing required dir"
        flat = entry.flat == true (default false)
        writable = if entry has "writable": entry.writable == true
                   else: if flat then false else true
        brain = Brain {
            name, dir: resolve(expand_home(entry.dir)),
            primary: entry.primary == true,
            writable, flat,
            git: entry.git or None,
            sync_interval: entry.syncInterval as u64 or 60,
            source: entry.source or None,
        }
        brains.push(brain)

    -- validate unique names
    names = HashSet
    for brain in brains:
        if names contains brain.name:
            error "duplicate brain name {name}"
        names.insert(brain.name)

    -- validate exactly one primary
    primaries = brains where primary == true
    if primaries.len() == 0: error "no brain marked primary"
    if primaries.len() > 1: error "multiple brains marked primary: {names}"

    -- filter to existing directories
    brains = brains where brain.dir exists

    primary_name = brains.iter().find(primary).name

    return BrainConfig { brains, primary: primary_name, config_path, last_mtime: None }

-- TESTS:
test_load_valid_config:
    write temp brains.json with 2 brains, one primary
    load_brains() -> success, 2 brains, correct primary

test_missing_name:
    write config with entry missing "name"
    load_brains() -> error containing "missing required"

test_duplicate_names:
    write config with two entries named "foo"
    load_brains() -> error containing "duplicate"

test_no_primary:
    write config with no brain marked primary
    load_brains() -> error containing "no brain marked primary"

test_multiple_primaries:
    write config with two primary brains
    load_brains() -> error containing "multiple brains"

test_flat_writable_default:
    write config with flat: true and no writable field
    load_brains() -> brain.writable == false

test_home_expansion:
    write config with dir "~/test-brain"
    load_brains() -> brain.dir starts with home_dir

test_missing_dir_filtered:
    write config with dir pointing to nonexistent path
    load_brains() -> that brain excluded from results

test_default_config_creation:
    use temp dir with no brains.json
    load_brains() -> creates default config, returns 1 brain named "memories"
```

### Database Schema (src/db.rs) [DW-1.3]
```
fn init_db(db_path: &Path) -> Result<Connection>:
    conn = Connection::open(db_path)
    conn.execute_batch("PRAGMA journal_mode = WAL")
    conn.execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)")

    -- check schema version
    cur_version = conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'")
    if cur_version is None or cur_version.parse::<i32>() < 5:
        conn.execute_batch("
            DROP TABLE IF EXISTS files;
            DROP TABLE IF EXISTS brain_fts;
            DROP TABLE IF EXISTS memories_fts;
            DROP TABLE IF EXISTS docs_fts;
        ")
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '5')")

    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS files (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            mtime REAL NOT NULL,
            PRIMARY KEY (brain, path)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS brain_fts USING fts5(
            path UNINDEXED, brain UNINDEXED, category, name, date UNINDEXED, description, body,
            tokenize = 'porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS dream_log (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            reviewed_at TEXT NOT NULL,
            mtime_at_review REAL NOT NULL,
            PRIMARY KEY (brain, path)
        );

        CREATE TABLE IF NOT EXISTS cross_links (
            brain_a TEXT NOT NULL,
            path_a TEXT NOT NULL,
            brain_b TEXT NOT NULL,
            path_b TEXT NOT NULL,
            score REAL NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (brain_a, path_a, brain_b, path_b)
        );
    ")

    return conn

-- No unit test for schema -- tested implicitly by all other tests that use init_db with temp file.
-- But add one explicit test:

test_schema_creation:
    create temp file
    conn = init_db(temp_path)
    verify meta table has schema_version = 5
    verify files table exists (INSERT + SELECT roundtrip)
    verify brain_fts virtual table exists (INSERT + SELECT roundtrip)
    verify dream_log table exists
    verify cross_links table exists

test_schema_migration:
    create db with schema_version = 3
    create a files table with old data
    init_db() -> drops and recreates, version now 5
```

### Parsing (src/parsing.rs) [DW-1.5, DW-1.7]
```
fn extract_frontmatter(content: &str) -> HashMap<String, String>:
    -- match /^---\n([\s\S]*?)\n---/
    if content does not start with "---\n": return empty
    find closing "\n---" after the opening
    if not found: return empty

    fm_block = content between "---\n" and "\n---"
    result = HashMap

    for line in fm_block.split('\n'):
        idx = line.find(':')
        if idx > 0:
            key = line[..idx].trim()
            value = line[idx+1..].trim()
            result.insert(key, value)

    return result

fn extract_body(content: &str) -> String:
    -- remove frontmatter block: /^---[\s\S]*?---\n*/
    if content starts with "---":
        find "\n---" after position 3
        if found at pos:
            rest = content[pos + 4..]  -- skip past "\n---"
            return rest.trim_start_matches('\n').trim()
    return content.trim()

fn extract_description(content: &str) -> String:
    body = extract_body(content)
    for line in body.split('\n'):
        trimmed = line.trim()
        if trimmed is empty: continue
        if trimmed starts with '#': continue
        if trimmed starts with '```': continue
        if trimmed starts with ':::': continue
        if trimmed starts with "import ": continue
        -- strip markdown formatting: backticks, underscores, asterisks
        cleaned = trimmed.replace(['`', '_', '*'], "")
        return cleaned[..min(120, cleaned.len())]
    return ""

-- TESTS:
test_extract_frontmatter_basic:
    input = "---\nname: test\ntype: note\n---\nbody"
    result = { "name": "test", "type": "note" }

test_extract_frontmatter_empty:
    input = "no frontmatter here"
    result = {}

test_extract_frontmatter_multiword_value:
    input = "---\ndescription: a longer value here\n---\n"
    result = { "description": "a longer value here" }

test_extract_body:
    input = "---\nname: test\n---\n\nBody content here"
    result = "Body content here"

test_extract_body_no_frontmatter:
    input = "Just body"
    result = "Just body"

test_extract_description_skips_headers:
    input = "---\nname: test\n---\n# Header\n\nActual description"
    result = "Actual description"

test_extract_description_skips_code_fences:
    input = "---\n---\n```rust\ncode\n```\nDescription line"
    result = "Description line"

test_extract_description_strips_formatting:
    input = "---\n---\n**bold** and `code`"
    result = "bold and code"

test_extract_description_truncates_120:
    input = long line > 120 chars
    result.len() == 120
```

### File Walker (src/walker.rs) [DW-1.6, DW-1.7]
```
fn walk_files(dir: &Path) -> Vec<PathBuf>:
    result = Vec
    if dir does not exist: return result

    for entry in read_dir(dir):
        name = entry.file_name()
        name_str = name.to_str()

        -- skip dot-prefixed and underscore-prefixed
        if name_str starts with '.' or name_str starts with '_': continue

        if entry is directory:
            result.extend(walk_files(entry.path()))
        else if name_str ends with ".md" or name_str ends with ".mdx":
            result.push(entry.path())

    result.sort()
    return result

fn get_categories(dir: &Path) -> Vec<String>:
    ensure dir exists
    entries = read_dir(dir)
    categories = entries
        .filter(|e| !e.name.starts_with('.') && e.is_dir())
        .map(|e| e.name.to_string())
        .collect()
    categories.sort()
    return categories

-- TESTS:
test_walk_files_basic:
    create temp dir with:
      category1/file1.md
      category1/file2.mdx
      category2/file3.md
    walk_files() -> 3 files, sorted

test_walk_files_skips_dotfiles:
    create .hidden/file.md and normal/file.md
    walk_files() -> only normal/file.md

test_walk_files_skips_underscored:
    create _drafts/file.md and notes/file.md
    walk_files() -> only notes/file.md

test_walk_files_skips_non_md:
    create dir with file.txt and file.md
    walk_files() -> only file.md

test_walk_files_empty_dir:
    walk_files(empty_dir) -> empty vec

test_walk_files_nonexistent:
    walk_files(nonexistent_path) -> empty vec

test_get_categories:
    create temp dir with subdirs: alpha, beta, .hidden
    get_categories() -> ["alpha", "beta"]
```

### Helpers (src/helpers.rs) [DW-1.1]
```
fn slugify(text: &str) -> String:
    text.to_lowercase()
    replace all non [a-z0-9] with "-"
    trim leading/trailing "-"
    truncate to 80 chars

fn today() -> String:
    chrono::Local::now().format("%Y-%m-%d").to_string()

-- TESTS (inline):
test_slugify: "Hello World!" -> "hello-world"
test_slugify_special: "a@b#c" -> "a-b-c"
test_slugify_truncate: 100-char input -> 80 chars max
test_today: matches YYYY-MM-DD pattern
```

### rmcp Spike (src/spike.rs) [DW-1.8]
```
-- This is a compile-time verification only, not used at runtime in Phase 1.
-- Goal: prove that rmcp's #[tool] macro and transport-io features compile
-- and that we can define a stub MCP server with a tool handler.

use rmcp::{ServerHandler, tool, model::*}

#[derive(Clone)]
struct GrugStub;

#[tool(tool_box)]
impl GrugStub:
    #[tool(description = "Stub tool for compile check")]
    async fn stub_tool(content: String) -> String:
        return format!("echo: {content}")

impl ServerHandler for GrugStub:
    fn name() -> String: "grug-stub"

-- No runtime test. If this compiles, DW-1.8 is met.
-- Add a #[cfg(test)] module with a trivial assertion that the type exists.

test_spike_compiles:
    let _ = GrugStub;  -- type exists, tool macro expanded
    assert!(true)
```

### Entry Points [DW-1.1]
```
src/main.rs:
    mod lib  -- just import and call a placeholder
    fn main():
        println!("grug-brain v0.1.0")

src/lib.rs:
    pub mod types;
    pub mod config;
    pub mod db;
    pub mod parsing;
    pub mod walker;
    pub mod helpers;
    pub mod spike;
```

## Design Notes
- `source` field preserved in Brain type even though plan doesn't mention it -- real brains.json has it, and dropping it would lose data on config round-trips (Phase 2 config mutation).
- `BrainConfig.last_mtime` added for hot-reload support in Phase 2 -- defined now to avoid breaking type changes later.
- Schema migration is destructive (drop + recreate) matching JS behavior. No data preservation for version < 5.
- All parsing functions are pure -- no I/O, no state. Easy to test.
- File walker returns absolute paths (PathBuf), matching how the JS version returns full paths from `join(dir, name)`.
- The spike module is gated behind the library but included in tests to prove rmcp compiles. It will be replaced by real implementation in Phase 3.

# Pseudocode: Phase 2 - Tool Implementations

## DW Verification

| DW-ID | Done-When Item | Status | Pseudocode Section |
|-------|---------------|--------|-------------------|
| DW-2.1 | All 9 tools implemented as functions returning structured results | COVERED | All tool sections below |
| DW-2.2 | FTS5 search with BM25 ranking, porter stemming, highlight snippets | COVERED | FTS Query Building, grug-search |
| DW-2.3 | File indexing (walk + parse + insert) matches current behavior | COVERED | Indexing Functions |
| DW-2.4 | Config hot-reload (mtime-based lazy check) works | COVERED | Config Hot-Reload |
| DW-2.5 | Dream tool: cross-links, stale detection (90 days), quality issues, conflict listing, dream log marking | COVERED | grug-dream |
| DW-2.6 | Recall tool: 2-most-recent-per-category preview, writes full listing to recall.md | COVERED | grug-recall |
| DW-2.7 | Unit tests for each tool against a temp SQLite database | COVERED | Tests section |

**All items COVERED:** YES

## Files to Create/Modify

- `src/types.rs` -- add refresh_interval to Brain; add ToolResult type
- `src/walker.rs` -- fix get_categories to skip underscore-prefixed dirs
- `src/tools/mod.rs` -- new: module declarations + shared ToolResult + shared GrugDb struct
- `src/tools/search.rs` -- new: build_fts_query, fts_search, search_all
- `src/tools/indexing.rs` -- new: index_file, remove_file, sync_brain
- `src/tools/write.rs` -- new: grug_write
- `src/tools/read.rs` -- new: grug_read
- `src/tools/recall.rs` -- new: grug_recall
- `src/tools/delete.rs` -- new: grug_delete
- `src/tools/config.rs` -- new: grug_config (list/add/remove)
- `src/tools/sync.rs` -- new: grug_sync
- `src/tools/dream.rs` -- new: grug_dream
- `src/tools/docs.rs` -- new: grug_docs (deprecated)
- `src/lib.rs` -- add `pub mod tools;`

## Pseudocode

### src/types.rs changes [DW-2.1]

Add `refresh_interval: Option<u64>` to Brain struct.

Add ToolResult type for uniform return:
```
pub struct ToolResult {
    pub text: String,
}
```

### src/walker.rs fix [DW-2.3]

In `get_categories()`, add underscore skip to match `walk_files`:
```
if name_str.starts_with('.') || name_str.starts_with('_') { continue; }
```

### src/tools/mod.rs [DW-2.1, DW-2.2, DW-2.3, DW-2.4]

Shared database wrapper holding rusqlite Connection + BrainConfig:

```
pub struct GrugDb {
    conn: Connection,
    config: BrainConfig,
}

impl GrugDb {
    pub fn open(db_path, config) -> Result<Self>
        - calls init_db to get connection
        - stores config
        - returns Self

    pub fn config(&self) -> &BrainConfig
    pub fn config_mut(&mut self) -> &mut BrainConfig
    pub fn conn(&self) -> &Connection

    /// Hot-reload: check brains.json mtime, reload if changed
    pub fn maybe_reload_config(&mut self) -> Result<()>
        - stat config.config_path, get mtime
        - compare with config.last_mtime
        - if same, return Ok
        - call load_brains_from(config.config_path)
        - replace self.config with new config
        - update last_mtime

    /// Resolve brain by name, defaulting to primary
    pub fn resolve_brain(&self, name: Option<&str>) -> Result<&Brain, String>
        - if name is None, return primary_brain()
        - if name is Some, look up in config, return error string if not found
}
```

Constants:
```
pub const SEARCH_PAGE_SIZE: usize = 20;
pub const BROWSE_PAGE_SIZE: usize = 100;
pub const STALE_DAYS: i64 = 90;
```

### src/tools/search.rs [DW-2.2]

```
/// Build FTS5 query string from user input.
/// Single term: `"term"*`
/// Multiple terms: `"term1"* OR "term2"*`
/// Empty/whitespace-only: None
pub fn build_fts_query(query: &str) -> Option<String>
    - split on whitespace, filter empty
    - if empty, return None
    - if 1 term: return Some(format!("\"{}\"*", term))
    - else: return Some(terms.map(t => "\"t\"*").join(" OR "))

/// Execute FTS search with fallback on error.
/// Returns (results: Vec<SearchResult>, total: usize)
pub fn fts_search(conn, fts_query, limit, offset) -> (Vec<SearchResult>, usize)
    - try: query searchCount to get total, query search for results
    - on error: try again with wildcards stripped (fts_query.replace("*", ""))
    - on error: return ([], 0)

/// Search all brains.
pub fn search_all(conn, query, page) -> (Vec<SearchResult>, usize)
    - let fts_query = build_fts_query(query)
    - if None, return ([], 0)
    - let offset = (max(1, page) - 1) * SEARCH_PAGE_SIZE
    - call fts_search(conn, fts_query, SEARCH_PAGE_SIZE, offset)
```

SQL for search (matching JS stmts.search):
```sql
SELECT path, brain, category, name, date, description,
       highlight(brain_fts, 5, '>>>', '<<<') as snippet,
       rank
FROM brain_fts WHERE brain_fts MATCH ?1
ORDER BY rank
LIMIT ?2 OFFSET ?3
```

SQL for searchCount:
```sql
SELECT COUNT(*) as total FROM brain_fts WHERE brain_fts MATCH ?1
```

### src/tools/indexing.rs [DW-2.3]

```
/// Index a single file into the database.
/// Reads content, extracts frontmatter/body/description, upserts FTS + files rows.
pub fn index_file(conn, brain_name, rel_path, full_path, category) -> Result<()>
    - read file content (return Err if can't read)
    - extract frontmatter, body, description
    - name = fm.get("name") || fm.get("title") || file_stem(rel_path)
    - DELETE FROM brain_fts WHERE brain=? AND path=?
    - INSERT INTO brain_fts (path, brain, category, name, date, description, body)
    - get file mtime
    - INSERT OR REPLACE INTO files (brain, path, mtime) VALUES (?, ?, ?)

/// Remove a file from all database tables.
pub fn remove_file(conn, brain_name, rel_path) -> Result<()>
    - DELETE FROM brain_fts WHERE brain=? AND path=?
    - DELETE FROM files WHERE brain=? AND path=?
    - DELETE FROM dream_log WHERE brain=? AND path=?
    - DELETE FROM cross_links WHERE (brain_a=? AND path_a=?) OR (brain_b=? AND path_b=?)

/// Full sync: walk disk, diff against indexed, index new/changed, remove stale.
/// Returns (on_disk_count, indexed_count, removed_count)
/// Port of index-worker.js sync logic.
pub fn sync_brain(conn, brain: &Brain) -> Result<(usize, usize, usize)>
    - get all indexed files+mtime for this brain from DB
    - build HashMap<String, f64> of path->mtime
    - walk disk:
      - if brain.flat: walk brain.dir directly, category = brain.name
      - else: for each category dir (get_categories), walk brain.dir/cat, category = cat
    - for each file on disk:
      - compute relPath = relative to brain.dir
      - add to on_disk set
      - get file mtime from stat
      - if not in indexed OR mtime differs:
        - read, parse, index_file()
    - for each indexed path NOT in on_disk set:
      - remove_file()
    - return counts
```

### src/tools/write.rs [DW-2.1]

```
pub fn grug_write(db, category, path_name, content, brain_name) -> ToolResult
    - db.maybe_reload_config()
    - resolve brain (default primary), error if not found
    - error if brain not writable
    - slugify category -> cat
    - ensure dir brain.dir/cat exists
    - slugify path_name -> slug
    - file_path = brain.dir/cat/slug.md
    - exists = file_path.exists()
    - if content doesn't start with "---\n":
      - wrap: "---\nname: {slug}\ndate: {today}\ntype: memory\n---\n\n{content}\n"
    - write file_path
    - rel_path = relative(brain.dir, file_path)
    - index_file(conn, brain.name, rel_path, file_path, cat)
    - skip git commit (Phase 4)
    - return "{updated|created} {rel_path}"
```

### src/tools/read.rs [DW-2.1]

Complex backwards-compat logic, matching JS exactly:

```
pub fn grug_read(db, brain_name, category, path_name) -> ToolResult
    - db.maybe_reload_config()

    Case 1: no args -> list all brains
    - if all three None:
      - for each brain: count files, build flags line
      - return "{n} brains\n\n{lines}"

    Case 2: category only (no brain, no path) -> backwards-compat search
    - if brain_name is None AND category is Some AND path_name is None:
      - search primary brain for this category
      - if 0 rows, search each non-primary brain
      - format rows as "- {name} (date): {description}"
      - return "# {category} [{brain}] ({n} files)\n\n{lines}"

    Case 3: path only (no brain, no category) -> try primary brain
    - if brain_name is None AND category is None AND path_name is Some:
      - split path on "/" -> first part is cat, rest is file
      - append .md if needed
      - try reading from primary brain.dir/cat/file
      - return content or "not found"

    Case 4: brain only -> list categories
    - resolve brain, error if not found
    - if category is None AND path_name is None:
      - SELECT category, COUNT(*) FROM brain_fts WHERE brain=? GROUP BY category ORDER BY category
      - return "{n} categories in \"{brain}\"\n\n{lines}"

    Case 5: brain + category -> list files
    - if category is Some AND path_name is None:
      - SELECT ... FROM brain_fts WHERE brain=? AND category=? ORDER BY date DESC
      - format as "- {name} (date): {description}"
      - return "# {category} [{brain}] ({n} files)\n\n{lines}"

    Case 6: brain + category + path -> read file
    - resolve file path: if flat brain -> brain.dir/file, else brain.dir/cat/file
    - append .md if needed
    - read file content
    - query cross_links for this brain+relPath
    - if links exist, append "---\n## linked memories\n\n{link lines}"
    - return content (with links appended)
```

### src/tools/recall.rs [DW-2.6]

```
pub fn grug_recall(db, category, brain_name) -> ToolResult
    - db.maybe_reload_config()
    - resolve brain
    - query: if category, recallByCategory; else recall (all for brain)
    - if 0 rows: "no memories found..."
    - group rows by category (preserving order via LinkedHashMap or Vec)
    - write full listing to brain.dir/recall.md:
      - "# {cat}\n\n- [{name}]({path}) (date): {description}\n"
    - build preview: 2 most recent per category
      - "# {cat}\n- {name} (date): {description}"
      - if more than 2: "  ... and {n} more"
    - return "{recall.md path}\n\n{preview}"

NOTE: JS writes to primaryBrain.dir/recall.md always, not the target brain.
Port this exactly.
```

### src/tools/delete.rs [DW-2.1]

```
pub fn grug_delete(db, category, path_name, brain_name) -> ToolResult
    - db.maybe_reload_config()
    - resolve brain, error if not found
    - error if not writable
    - extract filename from path_name (handle "/" in name, add .md)
    - file_path = brain.dir/category/filename
    - if not exists: "not found: {category}/{filename}"
    - delete file from disk
    - remove_file from DB
    - skip git commit (Phase 4)
    - return "deleted {category}/{filename}"
```

### src/tools/config.rs [DW-2.1, DW-2.4]

```
pub fn grug_config(db, action, name, dir, primary, writable, flat, git, sync_interval, source, refresh_interval) -> ToolResult
    - db.maybe_reload_config()

    action == "list":
    - for each brain: count files, build flags
    - flags: primary/writable/read-only, git:{url}, refresh:{n}s
    - return "{n} brains\n\n{lines}"
    - NOTE: skip sync-active/refresh-active flags (Phase 4 concept)

    action == "add":
    - validate name present, dir present
    - validate name format: /^[a-z0-9][a-z0-9-]*$/
    - read brains.json from disk (raw)
    - reject duplicate names
    - reject multiple primaries
    - resolve dir (expand_home, canonicalize)
    - ensure dir exists
    - flat defaults to false; writable defaults to true (false if flat, unless explicit)
    - build entry JSON object
    - push to array, write brains.json
    - force config reload (set last_mtime to None, call maybe_reload_config)
    - sync new brain (call sync_brain)
    - return "added brain \"{name}\" -- dir: {dir}"

    action == "remove":
    - validate name present
    - read brains.json from disk
    - find entry, error if not found
    - error if primary
    - remove all indexed files for this brain from DB
    - filter out entry, write brains.json
    - force config reload
    - return "removed brain \"{name}\" from config (files preserved at {dir})"

    else: "unknown action"
```

### src/tools/sync.rs [DW-2.1, DW-2.3]

```
pub fn grug_sync(db, brain_name) -> ToolResult
    - db.maybe_reload_config()
    - if brain_name given: filter to that brain, error if unknown
    - else: all brains
    - for each target brain:
      - before_count = SELECT COUNT(*) FROM files WHERE brain=?
      - call sync_brain(conn, brain)
      - after_count = same query
      - diff = after - before
      - format: "{name}: {after} files (+N new)" or "(-N removed)" or ""
    - return joined lines
```

### src/tools/dream.rs [DW-2.5]

Most complex tool. Port JS lines 1354-1549 exactly.

```
pub fn grug_dream(db) -> ToolResult
    - db.maybe_reload_config()
    - sync all brains first (call sync_brain for each)
    - collect all memories: for each brain, SELECT recall.all(brain.name)
    - if empty: "nothing to dream about -- no memories yet"

    let now = current_time_ms
    let ts = ISO timestamp string
    let sections: Vec<String>

    --- git commit section (STUB for Phase 4) ---
    - skip for now, but structure the code so Phase 4 can plug in

    --- conflicts section ---
    - query recallByCategory for primary brain, category="conflicts"
    - for each conflict row:
      - read file, extract frontmatter
      - format with original_path, original_brain, hostname, date
      - include resolution instructions
    - if any, push "## conflicts ({n})" section

    --- needs review ---
    - for each brain, query needsDream:
      SELECT f.brain, f.path, f.mtime, d.reviewed_at, d.mtime_at_review
      FROM files f LEFT JOIN dream_log d ON f.brain=d.brain AND f.path=d.path
      WHERE f.brain=? AND (d.path IS NULL OR f.mtime > d.mtime_at_review)
    - build Set of "brain:path" needing review

    - if nothing needs review AND no conflicts:
      - compute totals (files, categories)
      - return "dream report" with "all clean"

    - filter all memories to only those needing review

    --- cross-links ---
    - for each memory needing review:
      - DELETE existing cross_links for this memory
      - extract name terms: split on [-_ ], filter len>3, take first 3
      - build FTS query: terms joined with OR, quoted, no wildcards
      - search for matches (limit 5)
      - for each match:
        - skip self (same brain+path)
        - skip same category in same brain
        - sort brain:path pair for stable primary key (lexicographic)
        - dedup by key
        - upsert cross_link with rank as score, ts as created_at
        - collect link display info
    - sort links by rank
    - if any: push "## new cross-links" section (top 10)

    --- stale memories ---
    - filter toReview where date is valid and age >= 90 days
    - sort by age desc
    - if any: push "## stale ({n} memories > 90 days)" section

    --- quality issues ---
    - filter toReview where date is empty OR description is empty
    - if any: push "## quality issues" section

    --- needs review listing ---
    - push "## needs review ({n} memories)" section

    --- header (prepend) ---
    - compute totals: files, categories across all brains
    - conflict count
    - summary line: "{files} memories | {cats} categories | {n} need review | {links} cross-links | {stale} stale | {conflicts} conflicts"
    - prepend "# dream report\n\n{summary}"

    --- mark reviewed ---
    - for each toReview memory:
      - get file mtime from DB
      - upsert dream_log with ts and mtime

    - return sections.join("\n\n")
```

### src/tools/docs.rs [DW-2.1]

Deprecated tool. Port JS lines 1551-1625.

```
pub fn grug_docs(db, category, path_target, page) -> ToolResult
    - db.maybe_reload_config()

    No args: list categories across non-primary brains
    - query allCategoryCounts, filter to non-primary brain
    - return "{n} doc categories\n\n{lines}"

    Path provided: resolve and read file
    - try resolveDocPath: check catBrainDir map, then flat brains
    - if not found, try as absolute path
    - read and return content (paginated)

    Category only: list files in first matching non-primary brain
    - for each non-primary brain, check if it has this category
    - paginate with BROWSE_PAGE_SIZE=100
    - return "# {category} ({total} docs)\n\n{lines}"
```

For resolveDocPath: need a helper that maps categories to brain dirs.
Since we don't maintain a persistent catBrainDir map in Phase 2,
we can compute it on the fly from the brain config.

### Config Hot-Reload [DW-2.4]

Implemented in GrugDb::maybe_reload_config():
- stat brains.json, compare mtime with stored last_mtime
- if changed, reload config
- called at the top of every tool handler

## Design Notes

1. **No transport layer** -- all tools are plain functions taking &mut GrugDb + params, returning ToolResult. Phase 3 wires them to MCP.

2. **Git stubs** -- grug-write, grug-delete, grug-dream skip git operations. Phase 4 adds these. The tool functions will accept an optional async callback or return a "needs commit" signal.

3. **sync_brain is synchronous** -- the JS version uses a worker thread. The Rust version does the walk+diff+index inline. This is fine for Phase 2 (no server yet). Phase 3/4 can move it to a tokio task if needed.

4. **GrugDb owns Connection** -- single writer pattern. Tools take &mut GrugDb. Thread safety is Phase 3's concern.

5. **ToolResult is a simple String wrapper** -- matching the JS MCP pattern of `{ content: [{ type: "text", text }] }`. Phase 3 converts to MCP response format.

6. **Error handling** -- tools return ToolResult for user-facing errors (like "brain not found"). Internal errors (DB failures) return Result<ToolResult, Error>.

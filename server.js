import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Database } from "bun:sqlite";
import {
  readdirSync, readFileSync, writeFileSync, existsSync,
  statSync, mkdirSync, unlinkSync,
} from "fs";
import { join, resolve, relative, basename, dirname, extname } from "path";
import { fileURLToPath } from "url";
import { execFileSync } from "child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));

// --- brain config ---

function expandHome(p) {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  if (p === "~") return home;
  if (p.startsWith("~/")) return join(home, p.slice(2));
  return p;
}

// brains.json schema:
// [
//   {
//     "name": "memories",        -- unique identifier
//     "dir": "~/.grug-brain/memories",
//     "primary": true,           -- exactly one brain must be primary
//     "writable": true,          -- defaults: true for normal brains, false for flat:true brains
//     "flat": false,             -- flat:true means dir contains files directly (no category subdirs)
//     "git": null,               -- remote URL or null
//     "syncInterval": 60         -- sync interval in seconds (default 60)
//   }
// ]
function loadBrains() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const defaultConfigPath = join(home, ".grug-brain", "brains.json");
  const configPath = process.env.GRUG_CONFIG || defaultConfigPath;

  if (existsSync(configPath)) {
    let raw;
    try {
      raw = JSON.parse(readFileSync(configPath, "utf-8"));
    } catch (err) {
      throw new Error(`grug: failed to parse ${configPath}: ${err.message}`);
    }

    if (!Array.isArray(raw)) {
      throw new Error(`grug: ${configPath} must be a JSON array`);
    }

    const brains = raw.map((entry, i) => {
      if (!entry.name || typeof entry.name !== "string") {
        throw new Error(`grug: brain[${i}] missing required "name" field`);
      }
      if (!entry.dir || typeof entry.dir !== "string") {
        throw new Error(`grug: brain "${entry.name}" missing required "dir" field`);
      }
      const flat = entry.flat === true;
      const writableDefault = flat ? false : true;
      return {
        name: entry.name,
        dir: resolve(expandHome(entry.dir)),
        primary: entry.primary === true,
        writable: entry.writable !== undefined ? entry.writable === true : writableDefault,
        flat,
        git: entry.git || null,
        syncInterval: typeof entry.syncInterval === "number" ? entry.syncInterval : 60,
      };
    });

    // Validate: unique names
    const names = new Set();
    for (const brain of brains) {
      if (names.has(brain.name)) {
        throw new Error(`grug: duplicate brain name "${brain.name}" in ${configPath}`);
      }
      names.add(brain.name);
    }

    // Validate: exactly one primary
    const primaries = brains.filter(b => b.primary);
    if (primaries.length === 0) {
      throw new Error(`grug: no brain marked "primary: true" in ${configPath}`);
    }
    if (primaries.length > 1) {
      throw new Error(`grug: multiple brains marked "primary: true" in ${configPath}: ${primaries.map(b => b.name).join(", ")}`);
    }

    // Filter out brains whose dirs don't exist
    return brains.filter(b => existsSync(b.dir));
  }

  // No config file — build from env vars (backwards compat for existing users)
  const brains = [];

  // Primary brain from MEMORY_DIR
  const memoryDir = resolve(expandHome(
    process.env.MEMORY_DIR || join(home, ".grug-brain", "memories")
  ));
  brains.push({
    name: "memories",
    dir: memoryDir,
    primary: true,
    writable: true,
    flat: false,
    git: null,
    syncInterval: 60,
  });

  // Doc brains from DOCS_DIRS / DOCS_DIR
  // Supports two formats:
  //   /path/to/dir           — each subdirectory is a category (multi brain)
  //   name=/path/to/dir      — entire directory is one category named "name" (flat brain)
  const docsRaw = process.env.DOCS_DIRS || process.env.DOCS_DIR || "";
  if (docsRaw) {
    for (const entry of docsRaw.split(":")) {
      const eq = entry.indexOf("=");
      if (eq > 0) {
        const name = entry.slice(0, eq);
        const dir = resolve(expandHome(entry.slice(eq + 1)));
        brains.push({ name, dir, primary: false, writable: false, flat: true, git: null, syncInterval: 60 });
      } else {
        const dir = resolve(expandHome(entry));
        const name = basename(dir);
        brains.push({ name, dir, primary: false, writable: false, flat: false, git: null, syncInterval: 60 });
      }
    }
  }

  return brains.filter(b => existsSync(b.dir));
}

const brains = loadBrains();
const primaryBrain = brains.find(b => b.primary);

if (!primaryBrain) {
  process.stderr.write("grug: fatal — no primary brain directory found. Check MEMORY_DIR or ~/.grug-brain/brains.json\n");
  process.exit(1);
}

// Backwards-compat shims — existing code continues to use these until later phases
const MEMORY_DIR = primaryBrain.dir;

// DOCS_ENTRIES: collect non-primary, non-flat brains as "multi" entries and flat brains as "named" entries
// This mirrors the old DOCS_ENTRIES shape so the docs DB section needs no changes in Phase 1
const DOCS_ENTRIES = brains
  .filter(b => !b.primary)
  .map(b => b.flat
    ? { type: "named", name: b.name, dir: b.dir }
    : { type: "multi", dir: b.dir }
  );
const PAGE_SIZE = 50;
const BROWSE_PAGE_SIZE = 100;
const SEARCH_PAGE_SIZE = 20;

// --- helpers ---

function paginate(text, page = 1) {
  const lines = text.split("\n");
  if (lines.length <= PAGE_SIZE) return text;
  const totalPages = Math.ceil(lines.length / PAGE_SIZE);
  const p = Math.max(1, Math.min(page, totalPages));
  const start = (p - 1) * PAGE_SIZE;
  const slice = lines.slice(start, start + PAGE_SIZE);
  return `${slice.join("\n")}\n--- page ${p}/${totalPages} (${lines.length} lines) | page:${p + 1} for more ---`;
}

function isDir(p) {
  try { return statSync(p).isDirectory(); } catch { return false; }
}

function readFile(p) {
  try { return readFileSync(p, "utf-8"); } catch { return null; }
}

function ensureDir(p) {
  if (!existsSync(p)) mkdirSync(p, { recursive: true });
}

function slugify(text) {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").substring(0, 80);
}

function today() {
  return new Date().toISOString().slice(0, 10);
}

// --- git ---

function git(...args) {
  try {
    return execFileSync("git", args, {
      cwd: MEMORY_DIR, encoding: "utf-8", timeout: 10000,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch { return null; }
}

function ensureGitRepo() {
  // Must be its own repo root, not a subdirectory of a parent repo
  if (git("rev-parse", "--git-dir") === ".git") return true;
  if (git("init") === null) return false;
  const ignore = "*.db\n*.db-wal\n*.db-shm\nrecall.md\nlocal/\n.grugignore\n";
  writeFileSync(join(MEMORY_DIR, ".gitignore"), ignore, "utf-8");
  git("add", ".gitignore");
  git("commit", "-m", "grug: init");
  return true;
}

function hasRemote() {
  const remote = git("remote");
  return remote !== null && remote.length > 0;
}

function loadGrugIgnore() {
  const content = readFile(join(MEMORY_DIR, ".grugignore"));
  if (!content) return [];
  return content.split("\n").map(l => l.trim()).filter(l => l && !l.startsWith("#"));
}

function isLocalMemory(relPath, content) {
  // frontmatter sync: false
  if (content) {
    const fm = extractFrontmatter(content);
    if (fm.sync === "false") return true;
  }
  // .grugignore patterns
  for (const pattern of loadGrugIgnore()) {
    if (pattern.endsWith("/") && relPath.startsWith(pattern)) return true;
    if (pattern.includes("*")) {
      const regex = new RegExp("^" + pattern.replace(/\./g, "\\.").replace(/\*/g, ".*") + "$");
      if (regex.test(relPath)) return true;
    }
    if (relPath === pattern || relPath.startsWith(pattern + "/")) return true;
  }
  return false;
}

function syncGitExclude() {
  if (!ensureGitRepo()) return;
  const lines = ["# managed by grug-brain", ".grugignore"];
  lines.push(...loadGrugIgnore());
  // find sync:false memories
  for (const { path } of memStmts.allFiles.all()) {
    const content = readFile(join(MEMORY_DIR, path));
    if (content && extractFrontmatter(content).sync === "false") lines.push(path);
  }
  ensureDir(join(MEMORY_DIR, ".git", "info"));
  writeFileSync(join(MEMORY_DIR, ".git", "info", "exclude"), lines.join("\n") + "\n", "utf-8");
}

function gitCommitMemory(relPath, action) {
  if (!ensureGitRepo()) return;
  if (action !== "delete") {
    const content = readFile(join(MEMORY_DIR, relPath));
    if (isLocalMemory(relPath, content)) {
      syncGitExclude();
      return;
    }
  }
  git("add", "--", relPath);
  git("commit", "-m", `grug: ${action} ${relPath}`, "--quiet");
}

function gitSync() {
  if (!ensureGitRepo() || !hasRemote()) return;
  const before = git("rev-parse", "HEAD");
  git("pull", "--rebase", "--quiet");
  const after = git("rev-parse", "HEAD");
  git("push", "--quiet");
  if (before !== after) syncMemories();
}

// --- parsing ---

function extractFrontmatter(content) {
  const m = content.match(/^---\n([\s\S]*?)\n---/);
  if (!m) return {};
  const fm = {};
  for (const line of m[1].split("\n")) {
    const idx = line.indexOf(":");
    if (idx > 0) {
      fm[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
    }
  }
  return fm;
}

function extractBody(content) {
  return content.replace(/^---[\s\S]*?---\n*/, "").trim();
}

function extractDescription(content) {
  const body = extractBody(content);
  for (const line of body.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("```")
      || trimmed.startsWith(":::") || trimmed.startsWith("import ")) continue;
    return trimmed.replace(/[`_*]/g, "").substring(0, 120);
  }
  return "";
}

// --- file walkers ---

function getCategories(dir) {
  ensureDir(dir);
  return readdirSync(dir)
    .filter(name => !name.startsWith(".") && isDir(join(dir, name)))
    .sort();
}

function walkMemoryFiles(dir) {
  const files = [];
  if (!existsSync(dir)) return files;
  for (const name of readdirSync(dir)) {
    if (name.startsWith(".")) continue;
    const full = join(dir, name);
    if (isDir(full)) {
      files.push(...walkMemoryFiles(full));
    } else if (name.endsWith(".md")) {
      files.push(full);
    }
  }
  return files.sort();
}

function walkDocFiles(dir) {
  const files = [];
  if (!existsSync(dir)) return files;
  for (const name of readdirSync(dir)) {
    if (name.startsWith(".") || name.startsWith("_")) continue;
    const full = join(dir, name);
    if (isDir(full)) {
      files.push(...walkDocFiles(full));
    } else if (name.endsWith(".md") || name.endsWith(".mdx")) {
      files.push(full);
    }
  }
  return files.sort();
}

// --- init DB helper ---

function initDb(dbPath, schemaVersion, tableName, ftsColumns) {
  const db = new Database(dbPath);
  db.run("PRAGMA journal_mode = WAL");
  db.run("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)");
  const cur = db.prepare("SELECT value FROM meta WHERE key = 'schema_version'").get();
  if (!cur || parseInt(cur.value) < schemaVersion) {
    db.run("DROP TABLE IF EXISTS files");
    db.run(`DROP TABLE IF EXISTS ${tableName}`);
    db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)").run(String(schemaVersion));
  }
  db.run(`CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY, mtime REAL NOT NULL)`);
  db.run(`CREATE VIRTUAL TABLE IF NOT EXISTS ${tableName} USING fts5(${ftsColumns}, tokenize = 'porter unicode61')`);
  return db;
}

// ============================================================
// MEMORY DATABASE
// ============================================================

ensureDir(MEMORY_DIR);
const memDb = initDb(
  join(MEMORY_DIR, ".grug-brain.db"), 4, "memories_fts",
  "path UNINDEXED, category, name, date UNINDEXED, description, body"
);

memDb.run("CREATE TABLE IF NOT EXISTS dream_log (path TEXT PRIMARY KEY, reviewed_at TEXT NOT NULL, mtime_at_review REAL NOT NULL)");
memDb.run(`CREATE TABLE IF NOT EXISTS cross_links (
  path_a TEXT NOT NULL,
  path_b TEXT NOT NULL,
  score REAL NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (path_a, path_b)
)`);

const memStmts = {
  getFile: memDb.prepare("SELECT mtime FROM files WHERE path = ?"),
  upsertFile: memDb.prepare("INSERT OR REPLACE INTO files (path, mtime) VALUES (?, ?)"),
  deleteFile: memDb.prepare("DELETE FROM files WHERE path = ?"),
  allFiles: memDb.prepare("SELECT path FROM files"),
  insertFts: memDb.prepare("INSERT INTO memories_fts (path, category, name, date, description, body) VALUES (?, ?, ?, ?, ?, ?)"),
  deleteFts: memDb.prepare("DELETE FROM memories_fts WHERE path = ?"),
  searchCount: memDb.prepare("SELECT COUNT(*) as total FROM memories_fts WHERE memories_fts MATCH ?"),
  search: memDb.prepare(`
    SELECT path, category, name, date, description,
           highlight(memories_fts, 4, '>>>', '<<<') as snippet,
           rank
    FROM memories_fts
    WHERE memories_fts MATCH ?
    ORDER BY rank
    LIMIT ? OFFSET ?
  `),
  recall: memDb.prepare("SELECT path, category, name, date, description FROM memories_fts ORDER BY category, date DESC"),
  recallByCategory: memDb.prepare("SELECT path, category, name, date, description FROM memories_fts WHERE category = ? ORDER BY date DESC"),
  categoryCounts: memDb.prepare("SELECT category, COUNT(*) as count FROM memories_fts GROUP BY category ORDER BY category"),
  upsertLink: memDb.prepare("INSERT OR REPLACE INTO cross_links (path_a, path_b, score, created_at) VALUES (?, ?, ?, ?)"),
  deleteLinks: memDb.prepare("DELETE FROM cross_links WHERE path_a = ? OR path_b = ?"),
  getLinks: memDb.prepare(`
    SELECT path_a, path_b, score,
           m1.name as name_a, m1.category as cat_a,
           m2.name as name_b, m2.category as cat_b
    FROM cross_links
    JOIN memories_fts m1 ON m1.path = path_a
    JOIN memories_fts m2 ON m2.path = path_b
    WHERE path_a = ? OR path_b = ?
    ORDER BY score
    LIMIT 10
  `),
  allLinks: memDb.prepare(`
    SELECT path_a, path_b, score,
           m1.name as name_a, m1.category as cat_a,
           m2.name as name_b, m2.category as cat_b
    FROM cross_links
    JOIN memories_fts m1 ON m1.path = path_a
    JOIN memories_fts m2 ON m2.path = path_b
    ORDER BY score
    LIMIT 20
  `),
  getDreamLog: memDb.prepare("SELECT reviewed_at, mtime_at_review FROM dream_log WHERE path = ?"),
  upsertDreamLog: memDb.prepare("INSERT OR REPLACE INTO dream_log (path, reviewed_at, mtime_at_review) VALUES (?, ?, ?)"),
  deleteDreamLog: memDb.prepare("DELETE FROM dream_log WHERE path = ?"),
  needsDream: memDb.prepare(`
    SELECT f.path, f.mtime, d.reviewed_at, d.mtime_at_review
    FROM files f
    LEFT JOIN dream_log d ON f.path = d.path
    WHERE d.path IS NULL
       OR f.mtime > d.mtime_at_review
  `),
};

function indexMemory(relPath, fullPath) {
  const content = readFile(fullPath);
  if (!content) return;
  const fm = extractFrontmatter(content);
  const body = extractBody(content);
  const desc = extractDescription(content);
  const category = relPath.split("/")[0];
  memStmts.deleteFts.run(relPath);
  memStmts.insertFts.run(relPath, category, fm.name || basename(relPath, ".md"), fm.date || "", desc, body);
  memStmts.upsertFile.run(relPath, statSync(fullPath).mtimeMs);
}

function removeMemory(relPath) {
  memStmts.deleteFts.run(relPath);
  memStmts.deleteFile.run(relPath);
  memStmts.deleteDreamLog.run(relPath);
  memStmts.deleteLinks.run(relPath, relPath);
}

function syncMemories() {
  const indexed = new Set(memStmts.allFiles.all().map(r => r.path));
  const onDisk = new Set();
  for (const cat of getCategories(MEMORY_DIR)) {
    for (const fullPath of walkMemoryFiles(join(MEMORY_DIR, cat))) {
      const relPath = relative(MEMORY_DIR, fullPath);
      onDisk.add(relPath);
      const row = memStmts.getFile.get(relPath);
      const mtime = statSync(fullPath).mtimeMs;
      if (!row || row.mtime !== mtime) indexMemory(relPath, fullPath);
    }
  }
  for (const path of indexed) {
    if (!onDisk.has(path)) removeMemory(path);
  }
}

syncMemories();
syncGitExclude();

// ============================================================
// DOCS DATABASE
// ============================================================

let docsDb = null;
let docStmts = null;
let docsTotal = 0;

// Map category -> base dir for resolving file reads
const docCatDirs = new Map();

const hasAnyDocs = DOCS_ENTRIES.some(e =>
  e.type === "named" || getCategories(e.dir).length > 0
);

if (hasAnyDocs) {
  const dbPath = join(MEMORY_DIR, ".docs.db");
  docsDb = initDb(dbPath, 2, "docs_fts",
    "path UNINDEXED, category, title, description, body"
  );

  docStmts = {
    getFile: docsDb.prepare("SELECT mtime FROM files WHERE path = ?"),
    upsertFile: docsDb.prepare("INSERT OR REPLACE INTO files (path, mtime) VALUES (?, ?)"),
    deleteFile: docsDb.prepare("DELETE FROM files WHERE path = ?"),
    allFiles: docsDb.prepare("SELECT path FROM files"),
    insertFts: docsDb.prepare("INSERT INTO docs_fts (path, category, title, description, body) VALUES (?, ?, ?, ?, ?)"),
    deleteFts: docsDb.prepare("DELETE FROM docs_fts WHERE path = ?"),
    searchCount: docsDb.prepare("SELECT COUNT(*) as total FROM docs_fts WHERE docs_fts MATCH ?"),
    search: docsDb.prepare(`
      SELECT path, category, title, description,
             snippet(docs_fts, 4, '>>>', '<<<', '…', 40) as snippet,
             rank
      FROM docs_fts
      WHERE docs_fts MATCH ?
      ORDER BY rank
      LIMIT ? OFFSET ?
    `),
    categoryCounts: docsDb.prepare("SELECT category, COUNT(*) as count FROM docs_fts GROUP BY category ORDER BY category"),
    listByCategory: docsDb.prepare("SELECT path, title, description FROM docs_fts WHERE category = ? ORDER BY title LIMIT ? OFFSET ?"),
    countByCategory: docsDb.prepare("SELECT COUNT(*) as total FROM docs_fts WHERE category = ?"),
  };

  function indexDoc(relPath, fullPath) {
    const content = readFile(fullPath);
    if (!content) return;
    const fm = extractFrontmatter(content);
    const body = extractBody(content);
    const desc = extractDescription(content);
    const category = relPath.split("/")[0];
    const title = fm.title || basename(relPath, extname(relPath));
    docStmts.deleteFts.run(relPath);
    docStmts.insertFts.run(relPath, category, title, desc, body);
    docStmts.upsertFile.run(relPath, statSync(fullPath).mtimeMs);
  }

  function removeDoc(relPath) {
    docStmts.deleteFts.run(relPath);
    docStmts.deleteFile.run(relPath);
  }

  function resolveDocPath(relPath) {
    const cat = relPath.split("/")[0];
    const baseDir = docCatDirs.get(cat);
    if (baseDir) {
      // named entries: relPath is "cat/file", but files live at baseDir/file
      const withinCat = relPath.slice(cat.length + 1);
      const full = join(baseDir, withinCat);
      if (existsSync(full)) return full;
      // multi entries: relPath maps directly
      const full2 = join(baseDir, relPath);
      if (existsSync(full2)) return full2;
    }
    return null;
  }

  function syncDocs() {
    const indexed = new Set(docStmts.allFiles.all().map(r => r.path));
    const onDisk = new Set();
    let added = 0, updated = 0, removed = 0;

    for (const entry of DOCS_ENTRIES) {
      if (entry.type === "named") {
        // entire directory is one category
        docCatDirs.set(entry.name, entry.dir);
        for (const fullPath of walkDocFiles(entry.dir)) {
          const relFile = relative(entry.dir, fullPath);
          const relPath = `${entry.name}/${relFile}`;
          onDisk.add(relPath);
          const row = docStmts.getFile.get(relPath);
          const mtime = statSync(fullPath).mtimeMs;
          if (!row) { indexDoc(relPath, fullPath); added++; }
          else if (row.mtime !== mtime) { indexDoc(relPath, fullPath); updated++; }
        }
      } else {
        // each subdirectory is a category
        for (const cat of getCategories(entry.dir)) {
          docCatDirs.set(cat, entry.dir);
          for (const fullPath of walkDocFiles(join(entry.dir, cat))) {
            const relPath = relative(entry.dir, fullPath);
            onDisk.add(relPath);
            const row = docStmts.getFile.get(relPath);
            const mtime = statSync(fullPath).mtimeMs;
            if (!row) { indexDoc(relPath, fullPath); added++; }
            else if (row.mtime !== mtime) { indexDoc(relPath, fullPath); updated++; }
          }
        }
      }
    }

    for (const path of indexed) {
      if (!onDisk.has(path)) { removeDoc(path); removed++; }
    }
    return { added, updated, removed };
  }

  const docSync = syncDocs();
  docsTotal = docStmts.allFiles.all().length;
  const catSummary = docStmts.categoryCounts.all().map(r => `${r.category}(${r.count})`).join(", ");
  process.stderr.write(`grug: docs ${docsTotal} files in [${catSummary}] from ${DOCS_ENTRIES.length} source(s)`);
  if (docSync.added || docSync.updated || docSync.removed) {
    process.stderr.write(` — +${docSync.added} ~${docSync.updated} -${docSync.removed}`);
  }
  process.stderr.write("\n");
}

// ============================================================
// SEARCH (both databases, merged by rank)
// ============================================================

function buildFtsQuery(query) {
  const terms = query.trim().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return null;
  return terms.length === 1
    ? `"${terms[0]}"*`
    : terms.map(t => `"${t}"*`).join(" OR ");
}

function ftsSearch(stmts, ftsQuery, limit, offset) {
  try {
    const { total } = stmts.searchCount.get(ftsQuery);
    const results = stmts.search.all(ftsQuery, limit, offset);
    return { results, total };
  } catch {
    try {
      const simple = ftsQuery.replace(/\*/g, "");
      const { total } = stmts.searchCount.get(simple);
      const results = stmts.search.all(simple, limit, offset);
      return { results, total };
    } catch {
      return { results: [], total: 0 };
    }
  }
}

function searchAll(query, page = 1) {
  const ftsQuery = buildFtsQuery(query);
  if (!ftsQuery) return { memories: [], docs: [], memTotal: 0, docTotal: 0 };

  const offset = (Math.max(1, page) - 1) * SEARCH_PAGE_SIZE;
  const half = Math.floor(SEARCH_PAGE_SIZE / 2);

  const mem = ftsSearch(memStmts, ftsQuery, half, offset);

  let doc = { results: [], total: 0 };
  if (docStmts) {
    doc = ftsSearch(docStmts, ftsQuery, half, offset);
  }

  return {
    memories: mem.results, docs: doc.results,
    memTotal: mem.total, docTotal: doc.total,
  };
}

// ============================================================
// SERVER + TOOLS
// ============================================================

const server = new McpServer({ name: "grug-brain", version: "2.0.0" });

// --- grug-write ---

server.tool(
  "grug-write",
  "Store a memory. Saved as markdown with frontmatter, indexed for search. Add sync: false to frontmatter to keep local-only.",
  {
    category: z.string().describe("Folder to store in, e.g. loopback, feedback, react-native"),
    path: z.string().describe("Filename for the memory, e.g. no-db-mocks"),
    content: z.string().describe("Memory content in markdown"),
  },
  async ({ category, path: name, content }) => {
    const cat = slugify(category);
    const catDir = join(MEMORY_DIR, cat);
    ensureDir(catDir);

    const slug = slugify(name);
    const filePath = join(catDir, `${slug}.md`);
    const exists = existsSync(filePath);

    let fileContent = content;
    if (!content.startsWith("---\n")) {
      fileContent = `---\nname: ${slug}\ndate: ${today()}\ntype: memory\n---\n\n${content}\n`;
    }

    writeFileSync(filePath, fileContent, "utf-8");
    const relPath = relative(MEMORY_DIR, filePath);
    indexMemory(relPath, filePath);
    gitCommitMemory(relPath, exists ? "update" : "write");

    return { content: [{ type: "text", text: `${exists ? "updated" : "created"} ${relPath}` }] };
  }
);

// --- grug-search ---

server.tool(
  "grug-search",
  `Search across memories${docStmts ? " and docs" : ""}. BM25 ranked, porter stemming.`,
  {
    query: z.string().describe("Search terms"),
    page: z.number().optional().describe("Page number (20 results per page)"),
  },
  async ({ query, page }) => {
    const { memories, docs, memTotal, docTotal } = searchAll(query, page);
    const total = memTotal + docTotal;
    if (total === 0) return { content: [{ type: "text", text: `no matches for "${query}"` }] };

    const lines = [];
    const p = Math.max(1, page || 1);

    if (memories.length > 0) {
      lines.push(`## memories (${memTotal} matches)\n`);
      for (const r of memories) {
        const date = r.date ? ` date:${r.date}` : "";
        lines.push(`${r.path}${date} [${r.category}]\n  ${r.snippet || r.description}`);
      }
    }

    if (docs.length > 0) {
      if (lines.length > 0) lines.push("");
      lines.push(`## docs (${docTotal} matches)\n`);
      for (const r of docs) {
        lines.push(`${r.path} [${r.category}] — ${r.title || ""}\n  ${r.snippet || r.description}`);
      }
    }

    const totalPages = Math.ceil(Math.max(memTotal, docTotal) / (SEARCH_PAGE_SIZE / 2));
    const paging = totalPages > 1
      ? `\n--- page ${p}/${totalPages} | page:${p + 1} for more ---`
      : "";

    return { content: [{ type: "text", text: `${total} matches for "${query}"\n\n${lines.join("\n")}${paging}` }] };
  }
);

// --- grug-read ---

server.tool(
  "grug-read",
  "Read memories. No args = list categories. Category = list files. Category + path = read file.",
  {
    category: z.string().optional().describe("Category to browse or read from"),
    path: z.string().optional().describe("Filename within the category to read"),
  },
  async ({ category, path: name }) => {
    if (!category && !name) {
      const rows = memStmts.categoryCounts.all();
      if (rows.length === 0) return { content: [{ type: "text", text: "no categories yet" }] };
      const lines = rows.map(r => `  ${r.category}  (${r.count} memories)`);
      return { content: [{ type: "text", text: `${rows.length} categories\n\n${lines.join("\n")}` }] };
    }

    if (category && !name) {
      const rows = memStmts.recallByCategory.all(category);
      if (rows.length === 0) return { content: [{ type: "text", text: `no memories in "${category}"` }] };
      const lines = rows.map(r => {
        const date = r.date ? ` (${r.date})` : "";
        return `- ${r.name}${date}: ${r.description}`;
      });
      return { content: [{ type: "text", text: `# ${category} (${rows.length} memories)\n\n${lines.join("\n")}` }] };
    }

    const cat = category || name.split("/")[0];
    const file = name.includes("/") ? name.split("/").pop() : name;
    let t = file.endsWith(".md") ? file : `${file}.md`;
    const filePath = join(MEMORY_DIR, cat, t);
    if (!existsSync(filePath)) return { content: [{ type: "text", text: `not found: ${cat}/${file}` }] };

    const content = readFile(filePath);
    if (content === null) return { content: [{ type: "text", text: `could not read: ${cat}/${file}` }] };

    const relPath = `${cat}/${t}`;
    const linked = memStmts.getLinks.all(relPath, relPath);
    let text = content;
    if (linked.length > 0) {
      const linkLines = linked.map(l => {
        const other = l.path_a === relPath
          ? `${l.name_b} [${l.cat_b}]`
          : `${l.name_a} [${l.cat_a}]`;
        return `- ${other}`;
      });
      text += `\n\n---\n## linked memories\n\n${linkLines.join("\n")}`;
    }

    return { content: [{ type: "text", text }] };
  }
);

// --- grug-recall ---

server.tool(
  "grug-recall",
  "Get up to speed. Shows 2 most recent per category, writes full listing to recall.md.",
  {
    category: z.string().optional().describe("Filter to a specific category"),
  },
  async ({ category }) => {
    const rows = category
      ? memStmts.recallByCategory.all(category)
      : memStmts.recall.all();

    if (rows.length === 0) return { content: [{ type: "text", text: `no memories found${category ? ` in "${category}"` : ""}` }] };

    const groups = new Map();
    for (const r of rows) {
      if (!groups.has(r.category)) groups.set(r.category, []);
      groups.get(r.category).push(r);
    }

    const fullLines = [];
    for (const [cat, entries] of groups) {
      fullLines.push(`# ${cat}\n`);
      for (const e of entries) {
        const date = e.date ? ` (${e.date})` : "";
        fullLines.push(`- [${e.name}](${e.path})${date}: ${e.description}`);
      }
      fullLines.push("");
    }
    const outPath = join(MEMORY_DIR, "recall.md");
    writeFileSync(outPath, fullLines.join("\n"), "utf-8");

    const preview = [];
    for (const [cat, entries] of groups) {
      preview.push(`# ${cat}`);
      for (const e of entries.slice(0, 2)) {
        const date = e.date ? ` (${e.date})` : "";
        preview.push(`- ${e.name}${date}: ${e.description}`);
      }
      if (entries.length > 2) preview.push(`  … and ${entries.length - 2} more`);
    }

    return { content: [{ type: "text", text: `${outPath}\n\n${preview.join("\n")}` }] };
  }
);

// --- grug-delete ---

server.tool(
  "grug-delete",
  "Delete a memory.",
  {
    category: z.string().describe("Category the memory is in"),
    path: z.string().describe("Filename to delete"),
  },
  async ({ category, path: name }) => {
    const file = name.includes("/") ? name.split("/").pop() : name;
    let t = file.endsWith(".md") ? file : `${file}.md`;
    const filePath = join(MEMORY_DIR, category, t);
    if (!existsSync(filePath)) return { content: [{ type: "text", text: `not found: ${category}/${file}` }] };

    unlinkSync(filePath);
    removeMemory(`${category}/${t}`);
    gitCommitMemory(`${category}/${t}`, "delete");

    return { content: [{ type: "text", text: `deleted ${category}/${t}` }] };
  }
);

// --- grug-dream ---

server.tool(
  "grug-dream",
  "Dream: review memory health. Commits pending changes to git, shows history, finds cross-links across categories, flags stale memories. Use with /loop for periodic maintenance.",
  {},
  async () => {
    syncMemories();
    const all = memStmts.recall.all();
    if (all.length === 0) {
      return { content: [{ type: "text", text: "nothing to dream about — no memories yet" }] };
    }

    // --- which memories need attention? ---
    const needsReview = new Set(memStmts.needsDream.all().map(r => r.path));
    const now = Date.now();
    const ts = new Date().toISOString();

    const sections = [];
    const hasGit = ensureGitRepo();

    // --- commit pending & show history ---
    if (hasGit) {
      syncGitExclude();
      git("add", "-A");
      git("commit", "-m", "grug: dream sync", "--quiet");
      const log = git("log", "--oneline", "--name-status", "-15", "--", ".");
      sections.push(log
        ? `## recent history\n\n\`\`\`\n${log}\n\`\`\``
        : "## recent history\n\nno commits yet"
      );
    }

    if (needsReview.size === 0) {
      const catCount = memStmts.categoryCounts.all().length;
      sections.unshift(`# dream report\n\n${all.length} memories | ${catCount} categories | all clean — nothing needs review`);
      return { content: [{ type: "text", text: sections.join("\n\n") }] };
    }

    // filter to only memories needing review
    const toReview = all.filter(m => needsReview.has(m.path));

    // --- cross-links (rebuild for reviewed memories) ---
    const links = [];
    const seen = new Set();

    for (const mem of toReview) {
      memStmts.deleteLinks.run(mem.path, mem.path);
      const terms = mem.name.replace(/[-_]/g, " ").split(/\s+/).filter(t => t.length > 3);
      if (terms.length === 0) continue;
      const q = terms.slice(0, 3).map(t => `"${t}"`).join(" OR ");
      try {
        const matches = memStmts.search.all(q, 5, 0);
        for (const m of matches) {
          if (m.path === mem.path || m.category === mem.category) continue;
          const [a, b] = [mem.path, m.path].sort();
          const key = `${a}|${b}`;
          if (seen.has(key)) continue;
          seen.add(key);
          memStmts.upsertLink.run(a, b, m.rank, ts);
          links.push({ a: `${mem.name} [${mem.category}]`, b: `${m.name} [${m.category}]`, rank: m.rank });
        }
      } catch { /* skip bad queries */ }
    }

    if (links.length > 0) {
      links.sort((a, b) => a.rank - b.rank);
      const top = links.slice(0, 10);
      sections.push(`## new cross-links (${links.length} found, top ${top.length})\n\n${top.map(l => `- ${l.a} ↔ ${l.b}`).join("\n")}`);
    }

    // --- stale memories (only unreviewed) ---
    const STALE_DAYS = 90;
    const stale = toReview
      .filter(m => m.date && !isNaN(new Date(m.date)))
      .map(m => ({ ...m, age: Math.floor((now - new Date(m.date).getTime()) / 86400000) }))
      .filter(m => m.age >= STALE_DAYS)
      .sort((a, b) => b.age - a.age);

    if (stale.length > 0) {
      sections.push(`## stale (${stale.length} memories > ${STALE_DAYS} days)\n\n${stale.map(s =>
        `- ${s.name} [${s.category}] — ${s.age}d (${s.date}): ${s.description}`
      ).join("\n")}`);
    }

    // --- quality issues (only unreviewed) ---
    const issues = toReview.filter(m => !m.date || !m.description);
    if (issues.length > 0) {
      sections.push(`## quality issues\n\n${issues.map(m =>
        `- ${m.name} [${m.category}]: ${!m.date ? "no date" : "no description"}`
      ).join("\n")}`);
    }

    // --- needs review ---
    const reviewLines = toReview.map(m => {
      const date = m.date ? ` ${m.date}` : "";
      return `- ${m.name} [${m.category}]${date}: ${m.description}`;
    });
    sections.push(`## needs review (${toReview.length} memories)\n\n${reviewLines.join("\n")}`);

    // --- header ---
    const catCount = memStmts.categoryCounts.all().length;
    const summary = `${all.length} memories | ${catCount} categories | ${toReview.length} need review | ${links.length} cross-links | ${stale.length} stale`;
    sections.unshift(`# dream report\n\n${summary}\n\nOnly showing memories that are new or changed since last dream. Use grug-write to update, grug-delete to remove.`);

    // --- mark reviewed ---
    for (const m of toReview) {
      const file = memStmts.getFile.get(m.path);
      if (file) memStmts.upsertDreamLog.run(m.path, ts, file.mtime);
    }

    return { content: [{ type: "text", text: sections.join("\n\n") }] };
  }
);

// --- grug-docs ---

if (docStmts) {
  server.tool(
    "grug-docs",
    `Browse and read documentation. ${docsTotal} docs across: ${docStmts.categoryCounts.all().map(r => `${r.category} (${r.count})`).join(", ")}.`,
    {
      category: z.string().optional().describe("Doc category to browse"),
      path: z.string().optional().describe("File path to read (relative to docs dir)"),
      page: z.number().optional().describe("Page number for long files"),
    },
    async ({ category, path: target, page }) => {
      if (!category && !target) {
        const rows = docStmts.categoryCounts.all();
        if (rows.length === 0) return { content: [{ type: "text", text: "no docs found" }] };
        const lines = rows.map(r => `  ${r.category}  (${r.count} docs)`);
        return { content: [{ type: "text", text: `${rows.length} doc categories\n\n${lines.join("\n")}` }] };
      }

      if (target) {
        let filePath = resolveDocPath(target);
        if (!filePath) filePath = resolve(target);
        if (!filePath || !existsSync(filePath)) return { content: [{ type: "text", text: `file not found: ${target}` }] };
        const content = readFile(filePath);
        if (content === null) return { content: [{ type: "text", text: `could not read: ${target}` }] };
        return { content: [{ type: "text", text: paginate(content, page) }] };
      }

      const p = Math.max(1, page || 1);
      const limit = BROWSE_PAGE_SIZE;
      const offset = (p - 1) * limit;
      const { total } = docStmts.countByCategory.get(category);
      if (total === 0) return { content: [{ type: "text", text: `no docs in "${category}"` }] };
      const rows = docStmts.listByCategory.all(category, limit, offset);
      const lines = rows.map(r => `- [${r.title}](${r.path}): ${r.description || ""}`);
      const totalPages = Math.ceil(total / limit);
      const paging = totalPages > 1
        ? `\n--- page ${p}/${totalPages} (${total} docs) | page:${p + 1} for more ---`
        : "";
      return { content: [{ type: "text", text: `# ${category} (${total} docs)\n\n${lines.join("\n")}${paging}` }] };
    }
  );
}

const transport = new StdioServerTransport();
await server.connect(transport);

// --- sync timer ---

const SYNC_INTERVAL = 60_000;
if (ensureGitRepo() && hasRemote()) {
  setInterval(gitSync, SYNC_INTERVAL);
  process.stderr.write("grug: sync enabled (1 min interval)\n");
}

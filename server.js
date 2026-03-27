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
  // find sync:false memories in the primary brain
  for (const { path } of stmts.allFilesForBrain.all(primaryBrain.name)) {
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
  if (before !== after) syncBrain(primaryBrain);
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

// Single walker matching both .md and .mdx, skipping dot/underscore-prefixed entries
function walkFiles(dir) {
  const files = [];
  if (!existsSync(dir)) return files;
  for (const name of readdirSync(dir)) {
    if (name.startsWith(".") || name.startsWith("_")) continue;
    const full = join(dir, name);
    if (isDir(full)) {
      files.push(...walkFiles(full));
    } else if (name.endsWith(".md") || name.endsWith(".mdx")) {
      files.push(full);
    }
  }
  return files.sort();
}

// ============================================================
// UNIFIED DATABASE
// ============================================================

const SCHEMA_VERSION = 5;
const grugBrainDir = join(expandHome("~"), ".grug-brain");
ensureDir(grugBrainDir);
ensureDir(MEMORY_DIR);

const db = new Database(join(grugBrainDir, "grug.db"));
db.run("PRAGMA journal_mode = WAL");
db.run("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)");

const curVersion = db.prepare("SELECT value FROM meta WHERE key = 'schema_version'").get();
if (!curVersion || parseInt(curVersion.value) < SCHEMA_VERSION) {
  db.run("DROP TABLE IF EXISTS files");
  db.run("DROP TABLE IF EXISTS brain_fts");
  db.run("DROP TABLE IF EXISTS memories_fts");
  db.run("DROP TABLE IF EXISTS docs_fts");
  db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)").run(String(SCHEMA_VERSION));
}

db.run(`CREATE TABLE IF NOT EXISTS files (
  brain TEXT NOT NULL,
  path TEXT NOT NULL,
  mtime REAL NOT NULL,
  PRIMARY KEY (brain, path)
)`);

db.run(`CREATE VIRTUAL TABLE IF NOT EXISTS brain_fts USING fts5(
  path UNINDEXED, brain UNINDEXED, category, name, date UNINDEXED, description, body,
  tokenize = 'porter unicode61'
)`);

db.run(`CREATE TABLE IF NOT EXISTS dream_log (
  brain TEXT NOT NULL,
  path TEXT NOT NULL,
  reviewed_at TEXT NOT NULL,
  mtime_at_review REAL NOT NULL,
  PRIMARY KEY (brain, path)
)`);

db.run(`CREATE TABLE IF NOT EXISTS cross_links (
  brain_a TEXT NOT NULL,
  path_a TEXT NOT NULL,
  brain_b TEXT NOT NULL,
  path_b TEXT NOT NULL,
  score REAL NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (brain_a, path_a, brain_b, path_b)
)`);

const stmts = {
  getFile: db.prepare("SELECT mtime FROM files WHERE brain = ? AND path = ?"),
  upsertFile: db.prepare("INSERT OR REPLACE INTO files (brain, path, mtime) VALUES (?, ?, ?)"),
  deleteFile: db.prepare("DELETE FROM files WHERE brain = ? AND path = ?"),
  allFiles: db.prepare("SELECT brain, path FROM files"),
  allFilesForBrain: db.prepare("SELECT path FROM files WHERE brain = ?"),
  insertFts: db.prepare("INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?, ?, ?, ?, ?, ?, ?)"),
  deleteFts: db.prepare("DELETE FROM brain_fts WHERE brain = ? AND path = ?"),
  searchCount: db.prepare("SELECT COUNT(*) as total FROM brain_fts WHERE brain_fts MATCH ?"),
  search: db.prepare(`
    SELECT path, brain, category, name, date, description,
           highlight(brain_fts, 5, '>>>', '<<<') as snippet,
           rank
    FROM brain_fts
    WHERE brain_fts MATCH ?
    ORDER BY rank
    LIMIT ? OFFSET ?
  `),
  recall: db.prepare("SELECT path, brain, category, name, date, description FROM brain_fts WHERE brain = ? ORDER BY category, date DESC"),
  recallByCategory: db.prepare("SELECT path, brain, category, name, date, description FROM brain_fts WHERE brain = ? AND category = ? ORDER BY date DESC"),
  categoryCounts: db.prepare("SELECT category, COUNT(*) as count FROM brain_fts WHERE brain = ? GROUP BY category ORDER BY category"),
  allCategoryCounts: db.prepare("SELECT brain, category, COUNT(*) as count FROM brain_fts GROUP BY brain, category ORDER BY brain, category"),
  countForBrain: db.prepare("SELECT COUNT(*) as count FROM files WHERE brain = ?"),
  upsertLink: db.prepare("INSERT OR REPLACE INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES (?, ?, ?, ?, ?, ?)"),
  deleteLinks: db.prepare("DELETE FROM cross_links WHERE (brain_a = ? AND path_a = ?) OR (brain_b = ? AND path_b = ?)"),
  getLinks: db.prepare(`
    SELECT brain_a, path_a, brain_b, path_b, score,
           m1.name as name_a, m1.category as cat_a,
           m2.name as name_b, m2.category as cat_b
    FROM cross_links
    JOIN brain_fts m1 ON m1.brain = brain_a AND m1.path = path_a
    JOIN brain_fts m2 ON m2.brain = brain_b AND m2.path = path_b
    WHERE (brain_a = ? AND path_a = ?) OR (brain_b = ? AND path_b = ?)
    ORDER BY score
    LIMIT 10
  `),
  allLinks: db.prepare(`
    SELECT brain_a, path_a, brain_b, path_b, score,
           m1.name as name_a, m1.category as cat_a,
           m2.name as name_b, m2.category as cat_b
    FROM cross_links
    JOIN brain_fts m1 ON m1.brain = brain_a AND m1.path = path_a
    JOIN brain_fts m2 ON m2.brain = brain_b AND m2.path = path_b
    ORDER BY score
    LIMIT 20
  `),
  getDreamLog: db.prepare("SELECT reviewed_at, mtime_at_review FROM dream_log WHERE brain = ? AND path = ?"),
  upsertDreamLog: db.prepare("INSERT OR REPLACE INTO dream_log (brain, path, reviewed_at, mtime_at_review) VALUES (?, ?, ?, ?)"),
  deleteDreamLog: db.prepare("DELETE FROM dream_log WHERE brain = ? AND path = ?"),
  needsDream: db.prepare(`
    SELECT f.brain, f.path, f.mtime, d.reviewed_at, d.mtime_at_review
    FROM files f
    LEFT JOIN dream_log d ON f.brain = d.brain AND f.path = d.path
    WHERE f.brain = ?
      AND (d.path IS NULL OR f.mtime > d.mtime_at_review)
  `),
  listByCategory: db.prepare("SELECT path, name, description FROM brain_fts WHERE brain = ? AND category = ? ORDER BY name LIMIT ? OFFSET ?"),
  countByCategory: db.prepare("SELECT COUNT(*) as total FROM brain_fts WHERE brain = ? AND category = ?"),
};

// Backwards-compat shims — tools still use these names; Phase 4 will clean up
const memStmts = stmts;
const docStmts = stmts;

// Map: brain name -> brain config, for resolving file paths in grug-docs
const brainByName = new Map(brains.map(b => [b.name, b]));

// Map category -> brain name (for flat brains, category === brain name)
// Used by resolveDocPath to find the on-disk base dir
const catBrainDir = new Map();

function indexFile(brainName, relPath, fullPath, category) {
  const content = readFile(fullPath);
  if (!content) return;
  const fm = extractFrontmatter(content);
  const body = extractBody(content);
  const desc = extractDescription(content);
  const name = fm.name || fm.title || basename(relPath, extname(relPath));
  stmts.deleteFts.run(brainName, relPath);
  stmts.insertFts.run(relPath, brainName, category, name, fm.date || "", desc, body);
  stmts.upsertFile.run(brainName, relPath, statSync(fullPath).mtimeMs);
}

function removeFile(brainName, relPath) {
  stmts.deleteFts.run(brainName, relPath);
  stmts.deleteFile.run(brainName, relPath);
  stmts.deleteDreamLog.run(brainName, relPath);
  stmts.deleteLinks.run(brainName, relPath, brainName, relPath);
}

function syncBrain(brain) {
  const indexed = new Set(stmts.allFilesForBrain.all(brain.name).map(r => r.path));
  const onDisk = new Set();

  if (brain.flat) {
    // Flat brain: all files in dir get category = brain name
    catBrainDir.set(brain.name, brain.dir);
    for (const fullPath of walkFiles(brain.dir)) {
      const relPath = relative(brain.dir, fullPath);
      onDisk.add(relPath);
      const row = stmts.getFile.get(brain.name, relPath);
      const mtime = statSync(fullPath).mtimeMs;
      if (!row || row.mtime !== mtime) indexFile(brain.name, relPath, fullPath, brain.name);
    }
  } else {
    // Category brain: each subdirectory is a category
    for (const cat of getCategories(brain.dir)) {
      catBrainDir.set(cat, brain.dir);
      for (const fullPath of walkFiles(join(brain.dir, cat))) {
        const relPath = relative(brain.dir, fullPath);
        onDisk.add(relPath);
        const row = stmts.getFile.get(brain.name, relPath);
        const mtime = statSync(fullPath).mtimeMs;
        if (!row || row.mtime !== mtime) indexFile(brain.name, relPath, fullPath, cat);
      }
    }
  }

  for (const path of indexed) {
    if (!onDisk.has(path)) removeFile(brain.name, path);
  }
}

for (const brain of brains) syncBrain(brain);

// Startup log: show all brains and their file counts
for (const brain of brains) {
  const { count } = stmts.countForBrain.get(brain.name);
  const cats = stmts.categoryCounts.all(brain.name).map(r => `${r.category}(${r.count})`).join(", ");
  const detail = cats ? ` [${cats}]` : "";
  process.stderr.write(`grug: brain "${brain.name}" — ${count} files${detail}\n`);
}

syncGitExclude();

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

function ftsSearch(db_stmts, ftsQuery, limit, offset) {
  try {
    const { total } = db_stmts.searchCount.get(ftsQuery);
    const results = db_stmts.search.all(ftsQuery, limit, offset);
    return { results, total };
  } catch {
    try {
      const simple = ftsQuery.replace(/\*/g, "");
      const { total } = db_stmts.searchCount.get(simple);
      const results = db_stmts.search.all(simple, limit, offset);
      return { results, total };
    } catch {
      return { results: [], total: 0 };
    }
  }
}

function searchAll(query, page = 1) {
  const ftsQuery = buildFtsQuery(query);
  if (!ftsQuery) return { results: [], total: 0 };

  const offset = (Math.max(1, page) - 1) * SEARCH_PAGE_SIZE;
  const { results, total } = ftsSearch(stmts, ftsQuery, SEARCH_PAGE_SIZE, offset);
  return { results, total };
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
    indexFile(primaryBrain.name, relPath, filePath, cat);
    gitCommitMemory(relPath, exists ? "update" : "write");

    return { content: [{ type: "text", text: `${exists ? "updated" : "created"} ${relPath}` }] };
  }
);

// --- grug-search ---

server.tool(
  "grug-search",
  "Search across all brains. BM25 ranked, porter stemming.",
  {
    query: z.string().describe("Search terms"),
    page: z.number().optional().describe("Page number (20 results per page)"),
  },
  async ({ query, page }) => {
    const { results, total } = searchAll(query, page);
    if (total === 0) return { content: [{ type: "text", text: `no matches for "${query}"` }] };

    const lines = [];
    const p = Math.max(1, page || 1);

    for (const r of results) {
      const date = r.date ? ` date:${r.date}` : "";
      const brainLabel = r.brain !== primaryBrain.name ? ` {${r.brain}}` : "";
      lines.push(`${r.path}${date} [${r.category}]${brainLabel}\n  ${r.snippet || r.description}`);
    }

    const totalPages = Math.ceil(total / SEARCH_PAGE_SIZE);
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
      const rows = stmts.categoryCounts.all(primaryBrain.name);
      if (rows.length === 0) return { content: [{ type: "text", text: "no categories yet" }] };
      const lines = rows.map(r => `  ${r.category}  (${r.count} memories)`);
      return { content: [{ type: "text", text: `${rows.length} categories\n\n${lines.join("\n")}` }] };
    }

    if (category && !name) {
      const rows = stmts.recallByCategory.all(primaryBrain.name, category);
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
    const linked = stmts.getLinks.all(primaryBrain.name, relPath, primaryBrain.name, relPath);
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
      ? stmts.recallByCategory.all(primaryBrain.name, category)
      : stmts.recall.all(primaryBrain.name);

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
    removeFile(primaryBrain.name, `${category}/${t}`);
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
    syncBrain(primaryBrain);
    const all = stmts.recall.all(primaryBrain.name);
    if (all.length === 0) {
      return { content: [{ type: "text", text: "nothing to dream about — no memories yet" }] };
    }

    // --- which memories need attention? ---
    const needsReview = new Set(stmts.needsDream.all(primaryBrain.name).map(r => r.path));
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
      const catCount = stmts.categoryCounts.all(primaryBrain.name).length;
      sections.unshift(`# dream report\n\n${all.length} memories | ${catCount} categories | all clean — nothing needs review`);
      return { content: [{ type: "text", text: sections.join("\n\n") }] };
    }

    // filter to only memories needing review
    const toReview = all.filter(m => needsReview.has(m.path));

    // --- cross-links (rebuild for reviewed memories) ---
    const links = [];
    const seen = new Set();

    for (const mem of toReview) {
      stmts.deleteLinks.run(primaryBrain.name, mem.path, primaryBrain.name, mem.path);
      const terms = mem.name.replace(/[-_]/g, " ").split(/\s+/).filter(t => t.length > 3);
      if (terms.length === 0) continue;
      const q = terms.slice(0, 3).map(t => `"${t}"`).join(" OR ");
      try {
        const matches = stmts.search.all(q, 5, 0);
        for (const m of matches) {
          if (m.path === mem.path || m.category === mem.category) continue;
          // Sort by path for stable primary key; track which brain belongs to which
          const memBrain = primaryBrain.name;
          const mBrain = m.brain || primaryBrain.name;
          const [[pA, bA], [pB, bB]] = mem.path <= m.path
            ? [[mem.path, memBrain], [m.path, mBrain]]
            : [[m.path, mBrain], [mem.path, memBrain]];
          const key = `${bA}:${pA}|${bB}:${pB}`;
          if (seen.has(key)) continue;
          seen.add(key);
          stmts.upsertLink.run(bA, pA, bB, pB, m.rank, ts);
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
    const catCount = stmts.categoryCounts.all(primaryBrain.name).length;
    const summary = `${all.length} memories | ${catCount} categories | ${toReview.length} need review | ${links.length} cross-links | ${stale.length} stale`;
    sections.unshift(`# dream report\n\n${summary}\n\nOnly showing memories that are new or changed since last dream. Use grug-write to update, grug-delete to remove.`);

    // --- mark reviewed ---
    for (const m of toReview) {
      const file = stmts.getFile.get(primaryBrain.name, m.path);
      if (file) stmts.upsertDreamLog.run(primaryBrain.name, m.path, ts, file.mtime);
    }

    return { content: [{ type: "text", text: sections.join("\n\n") }] };
  }
);

// --- grug-docs ---

// Resolve a relPath (as stored in brain_fts) to an on-disk absolute path.
// For category brains: relPath is "cat/file", baseDir = catBrainDir[cat].
// For flat brains: relPath is just the filename; check all flat brain dirs.
function resolveDocPath(relPath) {
  const parts = relPath.split("/");
  const firstPart = parts[0];

  // Category brain: first path segment is a category name
  const baseDir = catBrainDir.get(firstPart);
  if (baseDir) {
    const full = join(baseDir, relPath);
    if (existsSync(full)) return full;
  }

  // Flat brain: relPath has no category prefix — check each flat brain's dir
  for (const brain of brains) {
    if (!brain.flat) continue;
    const full = join(brain.dir, relPath);
    if (existsSync(full)) return full;
  }

  return null;
}

const nonPrimaryBrains = brains.filter(b => !b.primary);

if (nonPrimaryBrains.length > 0) {
  const docCatSummary = stmts.allCategoryCounts.all()
    .filter(r => r.brain !== primaryBrain.name)
    .map(r => `${r.category} (${r.count})`)
    .join(", ");
  const docTotal = nonPrimaryBrains.reduce((sum, b) => sum + stmts.countForBrain.get(b.name).count, 0);

  server.tool(
    "grug-docs",
    `Browse and read documentation. ${docTotal} docs across: ${docCatSummary || "no docs yet"}.`,
    {
      category: z.string().optional().describe("Doc category to browse"),
      path: z.string().optional().describe("File path to read (relative to docs dir)"),
      page: z.number().optional().describe("Page number for long files"),
    },
    async ({ category, path: target, page }) => {
      if (!category && !target) {
        const rows = stmts.allCategoryCounts.all().filter(r => r.brain !== primaryBrain.name);
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

      // Find which brain(s) have this category
      const matchingBrain = nonPrimaryBrains.find(b => {
        const cats = stmts.categoryCounts.all(b.name).map(r => r.category);
        return cats.includes(category);
      });
      if (!matchingBrain) return { content: [{ type: "text", text: `no docs in "${category}"` }] };

      const p = Math.max(1, page || 1);
      const limit = BROWSE_PAGE_SIZE;
      const offset = (p - 1) * limit;
      const { total } = stmts.countByCategory.get(matchingBrain.name, category);
      if (total === 0) return { content: [{ type: "text", text: `no docs in "${category}"` }] };
      const rows = stmts.listByCategory.all(matchingBrain.name, category, limit, offset);
      const lines = rows.map(r => `- [${r.name}](${r.path}): ${r.description || ""}`);
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

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
const MEMORY_DIR = process.env.MEMORY_DIR || join(__dirname, "memories");
const DOCS_DIR = process.env.DOCS_DIR || join(__dirname, "docs");
const PAGE_SIZE = 50;
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
  const ignore = "*.db\n*.db-wal\n*.db-shm\nrecall.md\nlocal/\n";
  writeFileSync(join(MEMORY_DIR, ".gitignore"), ignore, "utf-8");
  git("add", ".gitignore");
  git("commit", "-m", "grug: init");
  return true;
}

function hasRemote() {
  const remote = git("remote");
  return remote !== null && remote.length > 0;
}

function gitCommitMemory(relPath, action) {
  if (!ensureGitRepo()) return;
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

// ============================================================
// DOCS DATABASE
// ============================================================

let docsDb = null;
let docStmts = null;
let docsTotal = 0;

if (existsSync(DOCS_DIR) && getCategories(DOCS_DIR).length > 0) {
  docsDb = initDb(
    join(DOCS_DIR, ".docs.db"), 1, "docs_fts",
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

  function syncDocs() {
    const indexed = new Set(docStmts.allFiles.all().map(r => r.path));
    const onDisk = new Set();
    let added = 0, updated = 0, removed = 0;
    for (const cat of getCategories(DOCS_DIR)) {
      for (const fullPath of walkDocFiles(join(DOCS_DIR, cat))) {
        const relPath = relative(DOCS_DIR, fullPath);
        onDisk.add(relPath);
        const row = docStmts.getFile.get(relPath);
        const mtime = statSync(fullPath).mtimeMs;
        if (!row) { indexDoc(relPath, fullPath); added++; }
        else if (row.mtime !== mtime) { indexDoc(relPath, fullPath); updated++; }
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
  process.stderr.write(`grug: docs ${docsTotal} files in [${catSummary}]`);
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
  "Store a memory. Saved as markdown with frontmatter, indexed for search.",
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

    return { content: [{ type: "text", text: content }] };
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

    const sections = [];
    const hasGit = ensureGitRepo();

    // --- commit pending & show history ---
    if (hasGit) {
      git("add", "-A");
      git("commit", "-m", "grug: dream sync", "--quiet");
      const log = git("log", "--oneline", "--name-status", "-15", "--", ".");
      sections.push(log
        ? `## recent history\n\n\`\`\`\n${log}\n\`\`\``
        : "## recent history\n\nno commits yet"
      );
    }

    // --- cross-links ---
    const links = [];
    const seen = new Set();

    for (const mem of all) {
      const terms = mem.name.replace(/[-_]/g, " ").split(/\s+/).filter(t => t.length > 3);
      if (terms.length === 0) continue;
      const q = terms.slice(0, 3).map(t => `"${t}"`).join(" OR ");
      try {
        const matches = memStmts.search.all(q, 5, 0);
        for (const m of matches) {
          if (m.path === mem.path || m.category === mem.category) continue;
          const key = [mem.path, m.path].sort().join("|");
          if (seen.has(key)) continue;
          seen.add(key);
          links.push({ a: `${mem.name} [${mem.category}]`, b: `${m.name} [${m.category}]`, rank: m.rank });
        }
      } catch { /* skip bad queries */ }
    }

    if (links.length > 0) {
      links.sort((a, b) => a.rank - b.rank);
      const top = links.slice(0, 10);
      sections.push(`## cross-links (${links.length} found, top ${top.length})\n\n${top.map(l => `- ${l.a} ↔ ${l.b}`).join("\n")}`);
    }

    // --- stale memories ---
    const now = Date.now();
    const STALE_DAYS = 90;
    const stale = all
      .filter(m => m.date && !isNaN(new Date(m.date)))
      .map(m => ({ ...m, age: Math.floor((now - new Date(m.date).getTime()) / 86400000) }))
      .filter(m => m.age >= STALE_DAYS)
      .sort((a, b) => b.age - a.age);

    if (stale.length > 0) {
      sections.push(`## stale (${stale.length} memories > ${STALE_DAYS} days)\n\n${stale.map(s =>
        `- ${s.name} [${s.category}] — ${s.age}d (${s.date}): ${s.description}`
      ).join("\n")}`);
    }

    // --- quality issues ---
    const issues = all.filter(m => !m.date || !m.description);
    if (issues.length > 0) {
      sections.push(`## quality issues\n\n${issues.map(m =>
        `- ${m.name} [${m.category}]: ${!m.date ? "no date" : "no description"}`
      ).join("\n")}`);
    }

    // --- full inventory ---
    const groups = new Map();
    for (const r of all) {
      if (!groups.has(r.category)) groups.set(r.category, []);
      groups.get(r.category).push(r);
    }
    const inv = [];
    for (const [cat, entries] of groups) {
      inv.push(`**${cat}** (${entries.length})`);
      for (const e of entries) inv.push(`  - ${e.name}${e.date ? ` ${e.date}` : ""}: ${e.description}`);
    }
    sections.push(`## inventory\n\n${inv.join("\n")}`);

    // --- header ---
    const catCount = memStmts.categoryCounts.all().length;
    const summary = `${all.length} memories | ${catCount} categories | ${links.length} cross-links | ${stale.length} stale`;
    sections.unshift(`# dream report\n\n${summary}\n\nReview below. Use grug-write to update, grug-delete to remove. Check stale memories for accuracy. Add cross-reference notes where useful.`);

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
        if (rows.length === 0) return { content: [{ type: "text", text: `no docs found in ${DOCS_DIR}` }] };
        const lines = rows.map(r => `  ${r.category}  (${r.count} docs)`);
        return { content: [{ type: "text", text: `${rows.length} doc categories\n\n${lines.join("\n")}` }] };
      }

      if (target) {
        let filePath = join(DOCS_DIR, target);
        if (!existsSync(filePath)) filePath = resolve(target);
        if (!existsSync(filePath)) return { content: [{ type: "text", text: `file not found: ${target}` }] };
        const content = readFile(filePath);
        if (content === null) return { content: [{ type: "text", text: `could not read: ${target}` }] };
        return { content: [{ type: "text", text: paginate(content, page) }] };
      }

      const rows = docStmts.search.all(`"${category}"*`, 30, 0);
      if (rows.length === 0) return { content: [{ type: "text", text: `no docs in "${category}"` }] };
      const lines = rows.map(r => `- [${r.title}](${r.path}): ${r.description || ""}`);
      return { content: [{ type: "text", text: `# ${category} (${rows.length} docs)\n\n${lines.join("\n")}` }] };
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

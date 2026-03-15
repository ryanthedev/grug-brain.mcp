import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import Database from "better-sqlite3";
import {
  readdirSync, readFileSync, writeFileSync, existsSync,
  statSync, mkdirSync, unlinkSync,
} from "fs";
import { join, resolve, relative, basename, dirname, extname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const MEMORY_DIR = process.env.MEMORY_DIR || join(__dirname, "memories");
const PAGE_SIZE = 50;

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

function readFile(path) {
  try { return readFileSync(path, "utf-8"); } catch { return null; }
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

// --- categories ---

function getCategories() {
  ensureDir(MEMORY_DIR);
  return readdirSync(MEMORY_DIR, { withFileTypes: true })
    .filter(d => d.isDirectory() && !d.name.startsWith("."))
    .map(d => d.name)
    .sort();
}

function walkFiles(dir) {
  const files = [];
  if (!existsSync(dir)) return files;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "llms.txt") continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...walkFiles(full));
    } else if (entry.name.endsWith(".md")) {
      files.push(full);
    }
  }
  return files.sort();
}

// --- frontmatter ---

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
    if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("```")) continue;
    return trimmed.replace(/[`_*]/g, "").substring(0, 120);
  }
  return "";
}

// --- FTS database ---

ensureDir(MEMORY_DIR);
const DB_PATH = join(MEMORY_DIR, ".grug-brain.db");
const db = new Database(DB_PATH);
db.pragma("journal_mode = WAL");

// schema version check — rebuild if schema changed
const SCHEMA_VERSION = 3;
db.exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)");
const currentVersion = db.prepare("SELECT value FROM meta WHERE key = 'schema_version'").get();
if (!currentVersion || parseInt(currentVersion.value) < SCHEMA_VERSION) {
  db.exec("DROP TABLE IF EXISTS files");
  db.exec("DROP TABLE IF EXISTS memories_fts");
  db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)").run(String(SCHEMA_VERSION));
}

db.exec(`
  CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    mtime REAL NOT NULL
  );
  CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    path UNINDEXED,
    category,
    name,
    date UNINDEXED,
    project,
    description,
    body,
    tokenize = 'porter unicode61'
  );
`);

const stmtGetFile = db.prepare("SELECT mtime FROM files WHERE path = ?");
const stmtUpsertFile = db.prepare("INSERT OR REPLACE INTO files (path, mtime) VALUES (?, ?)");
const stmtDeleteFile = db.prepare("DELETE FROM files WHERE path = ?");
const stmtAllFiles = db.prepare("SELECT path FROM files");

const stmtInsertFts = db.prepare(
  "INSERT INTO memories_fts (path, category, name, date, project, description, body) VALUES (?, ?, ?, ?, ?, ?, ?)"
);
const stmtDeleteFts = db.prepare("DELETE FROM memories_fts WHERE path = ?");
const SEARCH_PAGE_SIZE = 20;
const stmtSearchCount = db.prepare(`
  SELECT COUNT(*) as total FROM memories_fts WHERE memories_fts MATCH ?
`);
const stmtSearch = db.prepare(`
  SELECT path, category, name, date, project, description,
         highlight(memories_fts, 5, '>>>', '<<<') as snippet,
         rank
  FROM memories_fts
  WHERE memories_fts MATCH ?
  ORDER BY rank
  LIMIT ? OFFSET ?
`);
const stmtRecall = db.prepare(`
  SELECT path, category, name, date, project, description
  FROM memories_fts
  ORDER BY category, date DESC
`);
const stmtRecallByProject = db.prepare(`
  SELECT path, category, name, date, project, description
  FROM memories_fts
  WHERE project = ?
  ORDER BY category, date DESC
`);
const stmtCategoryCounts = db.prepare(`
  SELECT category, COUNT(*) as count
  FROM memories_fts
  GROUP BY category
  ORDER BY category
`);

function indexFile(relPath, fullPath) {
  const content = readFile(fullPath);
  if (!content) return;
  const fm = extractFrontmatter(content);
  const body = extractBody(content);
  const desc = extractDescription(content);
  const category = relPath.split("/")[0];

  stmtDeleteFts.run(relPath);
  stmtInsertFts.run(relPath, category, fm.name || "", fm.date || "", fm.project || category, desc, body);
  stmtUpsertFile.run(relPath, statSync(fullPath).mtimeMs);
}

function removeFromIndex(relPath) {
  stmtDeleteFts.run(relPath);
  stmtDeleteFile.run(relPath);
}

// sync on startup — only re-index changed files
function syncIndex() {
  const indexed = new Set(stmtAllFiles.all().map(r => r.path));
  const onDisk = new Set();

  for (const cat of getCategories()) {
    for (const fullPath of walkFiles(join(MEMORY_DIR, cat))) {
      const relPath = relative(MEMORY_DIR, fullPath);
      onDisk.add(relPath);

      const row = stmtGetFile.get(relPath);
      const mtime = statSync(fullPath).mtimeMs;
      if (!row || row.mtime !== mtime) {
        indexFile(relPath, fullPath);
      }
    }
  }

  for (const path of indexed) {
    if (!onDisk.has(path)) removeFromIndex(path);
  }
}

syncIndex();

// --- search ---

function search(query, page = 1) {
  const terms = query.trim().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return { results: [], total: 0 };

  const offset = (Math.max(1, page) - 1) * SEARCH_PAGE_SIZE;
  const ftsQuery = terms.length === 1
    ? `"${terms[0]}"*`
    : terms.map(t => `"${t}"*`).join(" OR ");

  try {
    const { total } = stmtSearchCount.get(ftsQuery);
    const results = stmtSearch.all(ftsQuery, SEARCH_PAGE_SIZE, offset);
    return { results, total };
  } catch {
    try {
      const simple = terms.map(t => `"${t}"`).join(" OR ");
      const { total } = stmtSearchCount.get(simple);
      const results = stmtSearch.all(simple, SEARCH_PAGE_SIZE, offset);
      return { results, total };
    } catch {
      return `search error — try simpler terms`;
    }
  }
}

// --- help text ---

const USAGE = `grug-brain — grug remember thing so not forget



actions:
  topics                          list memory categories
  search   text:"query"           search across all memories (FTS5 + BM25 ranked)
  read     target:"path"          read a memory file (paginated)
  write    project:"p" text:"…"   write a memory (target: override folder, name: optional)
  delete   target:"cat/file.md"   delete a memory
  recall   project:"name"         dump all indexes (optional project filter)

MEMORY_DIR: ${MEMORY_DIR}`;

const HELP = {
  search: `search: find memories across all categories (FTS5 + BM25 ranked)
  text: search query — natural language or FTS5 syntax (required)`,
  read: `read: read a memory file
  target: path relative to memory dir (required)`,
  write: `write: create or update a memory
  project: project/repo name (required — also used as folder name)
  target: override folder when it differs from project (optional, e.g. "feedback")
  name: filename slug (optional, derived from first line if omitted)
  text: memory content in markdown (required)
  if text starts with --- frontmatter is preserved as-is
  otherwise wraps in frontmatter with name/date/type/project fields`,
  delete: `delete: remove a memory and rebuild index
  target: path relative to memory dir, e.g. "feedback/no-summaries.md" (required)`,
};

// --- dispatch ---

function dispatch(action, target, text, name, project, page) {
  if (!action) return USAGE;

  switch (action) {
    case "topics": {
      const rows = stmtCategoryCounts.all();
      if (rows.length === 0) return `no categories yet in ${MEMORY_DIR}\nuse write to create your first memory`;
      const lines = rows.map(r => `  ${r.category}  (${r.count} memories)`);
      return `${rows.length} categories\n\n${lines.join("\n")}`;
    }

    case "search": {
      if (!text) return HELP.search;
      const result = search(text, page);
      if (typeof result === "string") return result;
      const { results, total } = result;
      if (results.length === 0) return `no matches for "${text}"`;
      const p = Math.max(1, page || 1);
      const totalPages = Math.ceil(total / SEARCH_PAGE_SIZE);
      const out = results.map(r => {
        const date = r.date ? ` date:${r.date}` : "";
        const proj = r.project ? ` project:${r.project}` : "";
        return `${r.path}${date}${proj} category:${r.category}\n  ${r.snippet || r.description}`;
      });
      const paging = totalPages > 1 ? `\n--- page ${p}/${totalPages} (${total} total) | page:${p + 1} for more ---` : "";
      return `${total} match${total === 1 ? "" : "es"} for "${text}"\n\n${out.join("\n")}${paging}`;
    }

    case "read": {
      if (!target) return HELP.read;
      let t = target.endsWith(".md") ? target : `${target}.md`;
      let filePath = join(MEMORY_DIR, t);
      if (!existsSync(filePath)) filePath = resolve(t);
      if (!existsSync(filePath)) return `not found: ${target}`;
      const content = readFile(filePath);
      if (content === null) return `could not read: ${target}`;
      return paginate(content, page);
    }

    case "write": {
      if (!project) return HELP.write;
      if (!text) return HELP.write;

      // folder defaults to project, target overrides
      const folder = target || project;
      const catDir = join(MEMORY_DIR, slugify(folder));
      ensureDir(catDir);

      let slug = name ? slugify(name) : null;
      if (!slug) {
        const firstLine = text.replace(/^---[\s\S]*?---\n*/, "")
          .split("\n").find(l => l.trim() && !l.startsWith("#")) || folder;
        slug = slugify(firstLine.substring(0, 60));
      }
      const filePath = join(catDir, `${slug}.md`);
      const exists = existsSync(filePath);

      let content = text;
      if (!text.startsWith("---\n")) {
        content = `---\nname: ${slug}\ndate: ${today()}\ntype: memory\nproject: ${project}\n---\n\n${text}\n`;
      }

      writeFileSync(filePath, content, "utf-8");

      const relPath = relative(MEMORY_DIR, filePath);
      indexFile(relPath, filePath);

      return `${exists ? "updated" : "created"} ${relPath}`;
    }

    case "delete": {
      if (!target) return HELP.delete;
      let t = target.endsWith(".md") ? target : `${target}.md`;
      const filePath = join(MEMORY_DIR, t);
      if (!existsSync(filePath)) return `not found: ${target}`;

      unlinkSync(filePath);
      removeFromIndex(t);

      return `deleted ${t}`;
    }

    case "recall": {
      const rows = project
        ? stmtRecallByProject.all(project)
        : stmtRecall.all();

      if (rows.length === 0) return `no memories found${project ? ` for project "${project}"` : ""}`;

      // group by category
      const groups = new Map();
      for (const r of rows) {
        if (!groups.has(r.category)) groups.set(r.category, []);
        groups.get(r.category).push(r);
      }

      // full dump to file
      const fullLines = [];
      for (const [cat, entries] of groups) {
        fullLines.push(`# ${cat}\n`);
        for (const e of entries) {
          const date = e.date ? ` (${e.date})` : "";
          const proj = e.project ? ` [${e.project}]` : "";
          fullLines.push(`- [${e.name || e.path}](${e.path})${date}${proj}: ${e.description}`);
        }
        fullLines.push("");
      }
      const outPath = join(MEMORY_DIR, "recall.md");
      writeFileSync(outPath, fullLines.join("\n"), "utf-8");

      // preview: 2 most recent per category
      const preview = [];
      for (const [cat, entries] of groups) {
        preview.push(`# ${cat}`);
        for (const e of entries.slice(0, 2)) {
          const date = e.date ? ` (${e.date})` : "";
          const proj = e.project ? ` [${e.project}]` : "";
          preview.push(`- ${e.name || e.path}${date}${proj}: ${e.description}`);
        }
        if (entries.length > 2) preview.push(`  … and ${entries.length - 2} more`);
      }

      return `${outPath}\n\n${preview.join("\n")}`;
    }

    default:
      return `unknown action: ${action}\n\n${USAGE}`;
  }
}

// --- server ---

const ALL_ACTIONS = ["topics", "search", "read", "write", "delete", "recall"];
const server = new McpServer({ name: "grug-brain", version: "0.2.0" });

server.tool(
  "grug-brain",
  "Grug remember thing. Actions: topics, search, read, write, delete, recall. No args for usage.",
  {
    action: z.enum(ALL_ACTIONS).optional(),
    target: z.string().optional(),
    text: z.string().optional(),
    name: z.string().optional(),
    project: z.string().optional(),
    page: z.number().optional(),
  },
  async ({ action, target, text, name, project, page }) => {
    const result = dispatch(action, target, text, name, project, page);
    return { content: [{ type: "text", text: result }] };
  }
);

const transport = new StdioServerTransport();
await server.connect(transport);

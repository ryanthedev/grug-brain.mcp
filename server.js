import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Database } from "bun:sqlite";
import {
  readdirSync, readFileSync, writeFileSync, existsSync,
  statSync, mkdirSync, unlinkSync,
} from "fs";
import { join, relative, basename, dirname, extname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const MEMORY_DIR = process.env.MEMORY_DIR || join(__dirname, "memories");

// --- helpers ---

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
    if (entry.name.startsWith(".")) continue;
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
const db = new Database(join(MEMORY_DIR, ".grug-brain.db"));
db.exec("PRAGMA journal_mode = WAL");

const SCHEMA_VERSION = 4;
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
  "INSERT INTO memories_fts (path, category, name, date, description, body) VALUES (?, ?, ?, ?, ?, ?)"
);
const stmtDeleteFts = db.prepare("DELETE FROM memories_fts WHERE path = ?");

const SEARCH_PAGE_SIZE = 20;
const stmtSearchCount = db.prepare(
  "SELECT COUNT(*) as total FROM memories_fts WHERE memories_fts MATCH ?"
);
const stmtSearch = db.prepare(`
  SELECT path, category, name, date, description,
         highlight(memories_fts, 4, '>>>', '<<<') as snippet,
         rank
  FROM memories_fts
  WHERE memories_fts MATCH ?
  ORDER BY rank
  LIMIT ? OFFSET ?
`);

const stmtRecall = db.prepare(`
  SELECT path, category, name, date, description
  FROM memories_fts
  ORDER BY category, date DESC
`);
const stmtRecallByCategory = db.prepare(`
  SELECT path, category, name, date, description
  FROM memories_fts
  WHERE category = ?
  ORDER BY date DESC
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
  stmtInsertFts.run(relPath, category, fm.name || basename(relPath, ".md"), fm.date || "", desc, body);
  stmtUpsertFile.run(relPath, statSync(fullPath).mtimeMs);
}

function removeFromIndex(relPath) {
  stmtDeleteFts.run(relPath);
  stmtDeleteFile.run(relPath);
}

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

// --- server ---

const server = new McpServer({ name: "grug-brain", version: "1.0.0" });

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
    indexFile(relPath, filePath);

    return { content: [{ type: "text", text: `${exists ? "updated" : "created"} ${relPath}` }] };
  }
);

// --- grug-search ---

server.tool(
  "grug-search",
  "Full-text search across all memories. BM25 ranked, porter stemming, prefix matching.",
  {
    query: z.string().describe("Search terms"),
    page: z.number().optional().describe("Page number for results (20 per page)"),
  },
  async ({ query, page }) => {
    const result = search(query, page);
    if (typeof result === "string") return { content: [{ type: "text", text: result }] };

    const { results, total } = result;
    if (results.length === 0) return { content: [{ type: "text", text: `no matches for "${query}"` }] };

    const p = Math.max(1, page || 1);
    const totalPages = Math.ceil(total / SEARCH_PAGE_SIZE);
    const out = results.map(r => {
      const date = r.date ? ` date:${r.date}` : "";
      return `${r.path}${date} category:${r.category}\n  ${r.snippet || r.description}`;
    });
    const paging = totalPages > 1 ? `\n--- page ${p}/${totalPages} (${total} total) | page:${p + 1} for more ---` : "";

    return { content: [{ type: "text", text: `${total} match${total === 1 ? "" : "es"} for "${query}"\n\n${out.join("\n")}${paging}` }] };
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
    // no args: list categories
    if (!category && !name) {
      const rows = stmtCategoryCounts.all();
      if (rows.length === 0) return { content: [{ type: "text", text: "no categories yet" }] };
      const lines = rows.map(r => `  ${r.category}  (${r.count} memories)`);
      return { content: [{ type: "text", text: `${rows.length} categories\n\n${lines.join("\n")}` }] };
    }

    // category only: list files in category
    if (category && !name) {
      const rows = stmtRecallByCategory.all(category);
      if (rows.length === 0) return { content: [{ type: "text", text: `no memories in "${category}"` }] };
      const lines = rows.map(r => {
        const date = r.date ? ` (${r.date})` : "";
        return `- ${r.name}${date}: ${r.description}`;
      });
      return { content: [{ type: "text", text: `# ${category} (${rows.length} memories)\n\n${lines.join("\n")}` }] };
    }

    // category + path: read specific file
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
      ? stmtRecallByCategory.all(category)
      : stmtRecall.all();

    if (rows.length === 0) return { content: [{ type: "text", text: `no memories found${category ? ` in "${category}"` : ""}` }] };

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
        fullLines.push(`- [${e.name}](${e.path})${date}: ${e.description}`);
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
    removeFromIndex(`${category}/${t}`);

    return { content: [{ type: "text", text: `deleted ${category}/${t}` }] };
  }
);

const transport = new StdioServerTransport();
await server.connect(transport);

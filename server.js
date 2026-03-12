import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFileSync } from "child_process";
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

function extractDescription(content) {
  const body = content.replace(/^---[\s\S]*?---\n*/, "");
  for (const line of body.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("```")) continue;
    return trimmed.replace(/[`_*]/g, "").substring(0, 120);
  }
  return "";
}

// --- build index ---

function buildIndex(category) {
  const catDir = join(MEMORY_DIR, category);
  const files = walkFiles(catDir);
  if (files.length === 0) {
    // empty category, write minimal index
    const out = `# ${category}\n\n(no memories yet)\n`;
    writeFileSync(join(catDir, "llms.txt"), out, "utf-8");
    return out;
  }

  const lines = [`# ${category}`, ""];
  for (const file of files) {
    const rel = relative(catDir, file);
    const content = readFile(file);
    const fm = content ? extractFrontmatter(content) : {};
    const desc = content ? extractDescription(content) : "";
    const title = fm.name || fm.title || basename(file, extname(file));
    const date = fm.date ? ` (${fm.date})` : "";
    const proj = fm.project ? ` [${fm.project}]` : "";
    const descStr = desc ? `: ${desc}` : "";
    lines.push(`- [${title}](${category}/${rel})${date}${proj}${descStr}`);
  }
  lines.push("");

  const out = lines.join("\n");
  writeFileSync(join(catDir, "llms.txt"), out, "utf-8");
  return out;
}

// --- search ---

function search(query) {
  const cats = getCategories();
  const llmsFiles = cats
    .map(cat => join(MEMORY_DIR, cat, "llms.txt"))
    .filter(f => existsSync(f));

  if (llmsFiles.length === 0) return [];

  try {
    const out = execFileSync("rg", [
      "--ignore-case", "--line-number", "--no-heading", "-H",
      query,
      ...llmsFiles,
    ], { encoding: "utf-8", timeout: 10000 });

    const results = [];
    for (const line of out.split("\n")) {
      if (!line.trim()) continue;
      // format: /path/to/cat/llms.txt:linenum:match
      const m = line.match(/^(.+?):(\d+):(.*)/);
      if (!m) continue;
      const cat = relative(MEMORY_DIR, m[1]).split("/")[0];
      results.push({ category: cat, line: parseInt(m[2]), match: m[3].trim() });
    }
    return results;
  } catch (e) {
    if (e.status === 1) return [];
    throw e;
  }
}

// --- help text ---

const USAGE = `grug-brain — grug remember thing so not forget



actions:
  topics                          list memory categories
  search   text:"query"           search across all memories
  read     target:"path"          read a memory file (paginated)
  write    target:"cat" text:"…"  write a memory (name: optional)
  delete   target:"cat/file.md"   delete a memory
  build    target:"cat"           rebuild llms.txt index

MEMORY_DIR: ${MEMORY_DIR}`;

const HELP = {
  search: `search: find memories across all categories
  text: search query — supports rg regex (required)`,
  read: `read: read a memory file
  target: path relative to memory dir (required)`,
  write: `write: create or update a memory
  target: category name (required, created if missing)
  name: filename slug (optional, derived from first line if omitted)
  project: project/repo name for context (optional, stored in frontmatter)
  text: memory content in markdown (required)
  if text starts with --- frontmatter is preserved as-is
  otherwise wraps in frontmatter with name/description/type/project fields`,
  delete: `delete: remove a memory and rebuild index
  target: path relative to memory dir, e.g. "feedback/no-summaries.md" (required)`,
  build: `build: rebuild llms.txt index for a category
  target: category name (required)`,
};

// --- dispatch ---

function dispatch(action, target, text, name, project, page) {
  if (!action) return USAGE;

  switch (action) {
    case "topics": {
      const cats = getCategories();
      if (cats.length === 0) return `no categories yet in ${MEMORY_DIR}\nuse write to create your first memory`;
      const lines = cats.map(cat => {
        const files = walkFiles(join(MEMORY_DIR, cat));
        const hasIndex = existsSync(join(MEMORY_DIR, cat, "llms.txt"));
        return `  ${cat}  (${files.length} memories${hasIndex ? "" : ", needs build"})`;
      });
      return `${cats.length} categories\n\n${lines.join("\n")}`;
    }

    case "search": {
      if (!text) return HELP.search;
      const results = search(text);
      if (results.length === 0) return `no matches for "${text}"`;
      const out = results.map(r => `[${r.category}] line ${r.line}: ${r.match}`);
      return `${results.length} match${results.length === 1 ? "" : "es"} for "${text}"\n\n${out.join("\n")}`;
    }

    case "read": {
      if (!target) return HELP.read;
      let filePath = join(MEMORY_DIR, target);
      if (!existsSync(filePath)) filePath = resolve(target);
      if (!existsSync(filePath)) return `not found: ${target}`;
      const content = readFile(filePath);
      if (content === null) return `could not read: ${target}`;
      return paginate(content, page);
    }

    case "write": {
      if (!target) return HELP.write;
      if (!text) return HELP.write;

      const catDir = join(MEMORY_DIR, slugify(target));
      ensureDir(catDir);

      // determine filename
      let slug = name ? slugify(name) : null;
      if (!slug) {
        // derive from first meaningful line
        const firstLine = text.replace(/^---[\s\S]*?---\n*/, "")
          .split("\n").find(l => l.trim() && !l.startsWith("#")) || target;
        slug = slugify(firstLine.substring(0, 60));
      }
      const filePath = join(catDir, `${slug}.md`);
      const exists = existsSync(filePath);

      // write content — add frontmatter if not present
      let content = text;
      if (!text.startsWith("---\n")) {
        const projectLine = project ? `\nproject: ${project}` : "";
        content = `---\nname: ${slug}\ndate: ${today()}\ntype: memory${projectLine}\n---\n\n${text}\n`;
      }

      writeFileSync(filePath, content, "utf-8");
      buildIndex(slugify(target));

      const rel = relative(MEMORY_DIR, filePath);
      return `${exists ? "updated" : "created"} ${rel}`;
    }

    case "delete": {
      if (!target) return HELP.delete;
      const filePath = join(MEMORY_DIR, target);
      if (!existsSync(filePath)) return `not found: ${target}`;

      unlinkSync(filePath);

      // rebuild index for the category
      const cat = target.split("/")[0];
      if (existsSync(join(MEMORY_DIR, cat))) buildIndex(cat);

      return `deleted ${target}`;
    }

    case "build": {
      if (!target) return HELP.build;
      const catDir = join(MEMORY_DIR, target);
      if (!isDir(catDir)) return `category not found: ${target}`;
      const index = buildIndex(target);
      const files = walkFiles(catDir);
      return `rebuilt index (${files.length} files)\n\n${index}`;
    }

    default:
      return `unknown action: ${action}\n\n${USAGE}`;
  }
}

// --- server ---

const ALL_ACTIONS = ["topics", "search", "read", "write", "delete", "build"];
const server = new McpServer({ name: "grug-brain", version: "0.1.0" });

server.tool(
  "grug-brain",
  "Grug remember thing. Call with no args for usage.",
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

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { isInitializeRequest } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import { Database } from "bun:sqlite";
import {
  readdirSync, readFileSync, writeFileSync, existsSync,
  statSync, mkdirSync, unlinkSync,
} from "fs";
import { join, resolve, relative, basename, dirname, extname } from "path";
import { fileURLToPath } from "url";
import { execFile as execFileCb } from "child_process";
import { promisify } from "util";
import { hostname as osHostname } from "os";
import { appendFileSync } from "fs";

const GRUG_LOG = join(process.env.HOME || "/tmp", ".grug-brain", "grug.log");
function grugLog(msg) {
  const line = `${new Date().toISOString()} ${msg}\n`;
  process.stderr.write(line);
  try { appendFileSync(GRUG_LOG, line); } catch {}
}

const __dirname = dirname(fileURLToPath(import.meta.url));
const execFileAsync = promisify(execFileCb);

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

  // No config file — create a default one
  const defaultDir = join(home, ".grug-brain", "memories");
  ensureDir(defaultDir);
  // Write a JSON array (not an object) — the parser above expects an array
  const defaultConfig = [
    { name: "memories", dir: defaultDir, primary: true, writable: true }
  ];
  ensureDir(dirname(configPath));
  writeFileSync(configPath, JSON.stringify(defaultConfig, null, 2) + "\n", "utf-8");
  process.stderr.write(`grug: created default config at ${configPath}\n`);
  return [{ name: "memories", dir: defaultDir, primary: true, writable: true, flat: false, git: null, syncInterval: 60 }];
}

// --- lazy config reload ---

// Cached state for lazy reload (mtime-based: only re-parses when file changes)
let brains = loadBrains();
let primaryBrain = brains.find(b => b.primary);

if (!primaryBrain) {
  process.stderr.write("grug: fatal — no primary brain directory found. Check ~/.grug-brain/brains.json\n");
  process.exit(1);
}

// Last-seen mtime of brains.json. Used by reloadBrains() to skip re-parsing unchanged files.
let brainsJsonMtime = 0;
{
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  try { brainsJsonMtime = statSync(configPath).mtimeMs; } catch { /* file may not exist yet */ }
}

// Re-reads brains.json when mtime changes. Updates module-level brains and primaryBrain.
// Failures are logged to stderr but never thrown — stale config is better than a crash.
function reloadBrains() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  let currentMtime;
  try {
    currentMtime = statSync(configPath).mtimeMs;
  } catch {
    return; // Config file gone; keep current brains in memory
  }
  if (currentMtime === brainsJsonMtime) return; // File unchanged — skip re-parse

  let newBrains;
  try {
    newBrains = loadBrains();
  } catch (err) {
    process.stderr.write(`grug: config reload failed — ${err.message}\n`);
    return; // Keep current brains on parse error
  }

  const newPrimary = newBrains.find(b => b.primary);
  if (!newPrimary) {
    process.stderr.write("grug: config reload skipped — no primary brain in updated config\n");
    return;
  }

  // Start timers for newly added brains (brains not in the current set)
  const currentNames = new Set(brains.map(b => b.name));
  for (const brain of newBrains) {
    if (!currentNames.has(brain.name)) {
      syncBrain(brain);
      startBrainTimers(brain);
    }
  }

  // Stop timers for removed brains
  const newNames = new Set(newBrains.map(b => b.name));
  for (const brain of brains) {
    if (!newNames.has(brain.name)) {
      stopBrainTimers(brain.name);
      // Remove from FTS
      const indexed = stmts.allFilesForBrain.all(brain.name).map(r => r.path);
      for (const relPath of indexed) removeFile(brain.name, relPath);
    }
  }

  brains = newBrains;
  primaryBrain = newPrimary;
  brainsJsonMtime = currentMtime;
  process.stderr.write(`grug: config reloaded — ${brains.length} brain(s)\n`);
}

// Returns current brains array after checking for config changes.
// Call this at the start of every tool handler.
function getBrains() {
  reloadBrains();
  return brains;
}

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

function getHostname() {
  const full = osHostname();
  const first = full.split(".")[0];
  const sanitized = first.replace(/[^a-zA-Z0-9-]/g, "");
  return sanitized || "unknown";
}

const syncLocks = new Map();

function acquireSyncLock(brain) {
  if (syncLocks.get(brain.name) === true) return false;
  syncLocks.set(brain.name, true);
  return true;
}

function releaseSyncLock(brain) {
  syncLocks.set(brain.name, false);
}

async function git(brain, ...args) {
  const t0 = Date.now();
  try {
    const { stdout } = await execFileAsync("git", args, {
      cwd: brain.dir, encoding: "utf-8", timeout: 10000,
    });
    const elapsed = Date.now() - t0;
    if (elapsed > 1000) grugLog(`[git] ${brain.name} ${args[0]} — slow ${elapsed}ms`);
    return stdout.trim();
  } catch (err) {
    const elapsed = Date.now() - t0;
    if (elapsed > 1000) grugLog(`[git] ${brain.name} ${args[0]} — failed ${elapsed}ms`);
    return null;
  }
}

async function ensureGitRepo(brain) {
  if (await git(brain, "rev-parse", "--git-dir") === ".git") return true;
  if (await git(brain, "init") === null) return false;
  const ignore = "*.db\n*.db-wal\n*.db-shm\nrecall.md\nlocal/\n.grugignore\n";
  writeFileSync(join(brain.dir, ".gitignore"), ignore, "utf-8");
  await git(brain, "add", ".gitignore");
  await git(brain, "commit", "-m", "grug: init");
  return true;
}

async function hasRemote(brain) {
  const remote = await git(brain, "remote");
  return remote !== null && remote.length > 0;
}

function loadGrugIgnore(brain) {
  const content = readFile(join(brain.dir, ".grugignore"));
  if (!content) return [];
  return content.split("\n").map(l => l.trim()).filter(l => l && !l.startsWith("#"));
}

function isLocalFile(brain, relPath, content) {
  if (content) {
    const fm = extractFrontmatter(content);
    if (fm.sync === "false") return true;
  }
  for (const pattern of loadGrugIgnore(brain)) {
    if (pattern.endsWith("/") && relPath.startsWith(pattern)) return true;
    if (pattern.includes("*")) {
      const regex = new RegExp("^" + pattern.replace(/\./g, "\\.").replace(/\*/g, ".*") + "$");
      if (regex.test(relPath)) return true;
    }
    if (relPath === pattern || relPath.startsWith(pattern + "/")) return true;
  }
  return false;
}

async function syncGitExclude(brain) {
  if (!await ensureGitRepo(brain)) return;
  const lines = ["# managed by grug-brain", ".grugignore"];
  lines.push(...loadGrugIgnore(brain));
  // Walk brain directory to find sync:false files
  for (const fullPath of walkFiles(brain.dir)) {
    const content = readFile(fullPath);
    if (content && extractFrontmatter(content).sync === "false") {
      lines.push(relative(brain.dir, fullPath));
    }
  }
  ensureDir(join(brain.dir, ".git", "info"));
  writeFileSync(join(brain.dir, ".git", "info", "exclude"), lines.join("\n") + "\n", "utf-8");
}

async function gitCommitFile(brain, relPath, action) {
  if (!await ensureGitRepo(brain)) return;
  if (syncLocks.get(brain.name) === true) return;
  if (action !== "delete") {
    const content = readFile(join(brain.dir, relPath));
    if (isLocalFile(brain, relPath, content)) {
      await syncGitExclude(brain);
      return;
    }
  }
  await git(brain, "add", "--", relPath);
  await git(brain, "commit", "-m", `grug: ${action} ${relPath}`, "--quiet");
}

async function resolveRebaseConflict(brain) {
  const unmergedOutput = await git(brain, "diff", "--name-only", "--diff-filter=U");
  if (!unmergedOutput || unmergedOutput.length === 0) {
    process.stderr.write(`grug: conflict detected in ${brain.name} but no unmerged files found\n`);
    await git(brain, "rebase", "--abort");
    return;
  }

  const conflictFiles = unmergedOutput.split("\n").filter(Boolean);
  const conflictBrain = brains.find(b => b.primary);
  const host = getHostname();
  const dateStr = today();

  for (const filePath of conflictFiles) {
    const localContent = await git(brain, "show", `REBASE_HEAD:${filePath}`);
    if (localContent === null) {
      process.stderr.write(`grug: could not retrieve local version of ${filePath} in ${brain.name}\n`);
      continue;
    }

    let conflictFileName = slugify(brain.name) + "--" + filePath.replace(/\//g, "--");
    if (!conflictFileName.endsWith(".md")) {
      conflictFileName = conflictFileName + ".md";
    }
    const conflictDir = join(conflictBrain.dir, "conflicts");
    ensureDir(conflictDir);
    const conflictFullPath = join(conflictDir, conflictFileName);

    const nameSlug = slugify(basename(filePath, extname(filePath)));
    const frontmatter = [
      "---",
      `name: conflict-${slugify(brain.name)}-${nameSlug}`,
      `date: ${dateStr}`,
      "type: memory",
      "conflict: true",
      `original_path: ${filePath}`,
      `original_brain: ${brain.name}`,
      `hostname: ${host}`,
      "---",
    ].join("\n");

    const body = localContent.startsWith("---\n")
      ? extractBody(localContent)
      : localContent;

    const fileContent = frontmatter + "\n\n" + body + "\n";

    try {
      writeFileSync(conflictFullPath, fileContent, "utf-8");
      process.stderr.write(`grug: conflict saved — ${conflictFullPath}\n`);
      const relConflictPath = relative(conflictBrain.dir, conflictFullPath);
      indexFile(conflictBrain.name, relConflictPath, conflictFullPath, "conflicts");
    } catch (err) {
      process.stderr.write(`grug: FAILED to save conflict file for ${filePath}: ${err.message}\n`);
      process.stderr.write(`grug: leaving ${brain.name} in rebase state for manual resolution\n`);
      return;
    }
  }

  await git(brain, "rebase", "--abort");

  const remoteBranch = await git(brain, "rev-parse", "--abbrev-ref", "@{upstream}");
  if (remoteBranch !== null) {
    await git(brain, "reset", "--hard", remoteBranch);
  } else {
    const mainRef = await git(brain, "rev-parse", "--verify", "origin/main");
    if (mainRef !== null) {
      await git(brain, "reset", "--hard", "origin/main");
    } else {
      const masterRef = await git(brain, "rev-parse", "--verify", "origin/master");
      if (masterRef !== null) {
        await git(brain, "reset", "--hard", "origin/master");
      }
    }
  }

  syncBrain(brain);
}

async function gitSync(brain) {
  if (!await ensureGitRepo(brain)) return;
  if (!await hasRemote(brain)) return;
  if (!acquireSyncLock(brain)) {
    grugLog(`[gitSync] ${brain.name} — skipped (lock held)`);
    return;
  }

  const t0 = Date.now();
  grugLog(`[gitSync] ${brain.name} — start`);
  try {
    await syncGitExclude(brain);
    const before = await git(brain, "rev-parse", "HEAD");

    const pullResult = await git(brain, "pull", "--rebase", "--quiet");

    if (pullResult === null) {
      const rebaseHeadPath = join(brain.dir, ".git", "REBASE_HEAD");
      if (existsSync(rebaseHeadPath)) {
        process.stderr.write(`grug: rebase conflict detected in ${brain.name}\n`);
        await resolveRebaseConflict(brain);
      }
      grugLog(`[gitSync] ${brain.name} — done (pull failed) ${Date.now() - t0}ms`);
      return;
    }

    const after = await git(brain, "rev-parse", "HEAD");
    await git(brain, "push", "--quiet");
    // Reindex if remote changed OR local has new/modified files.
    // git status is cheap; syncBrain walks every file so skip it when idle.
    const dirty = before !== after || await git(brain, "status", "--porcelain") !== "";
    if (dirty) {
      grugLog(`[gitSync] ${brain.name} — dirty, running syncBrain`);
      syncBrain(brain);
    }
    grugLog(`[gitSync] ${brain.name} — done ${Date.now() - t0}ms`);
  } finally {
    releaseSyncLock(brain);
  }
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
ensureDir(primaryBrain.dir);

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

// Map: brain name -> brain config, for resolving file paths
const brainByName = new Map(brains.map(b => [b.name, b]));

// Resolve a brain by name, defaulting to primaryBrain.
// Returns { brain } on success, or { error: string } if not found.
function resolveBrain(name) {
  if (!name) return { brain: primaryBrain };
  const brain = brainByName.get(name);
  if (!brain) return { error: `unknown brain "${name}"` };
  return { brain };
}

// Map category -> brain dir (for flat brains, category === brain name)
// Used by resolveDocPath to find the on-disk absolute path
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
  const t0 = Date.now();
  grugLog(`[syncBrain] ${brain.name} — start`);
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

  let removed = 0;
  for (const path of indexed) {
    if (!onDisk.has(path)) { removeFile(brain.name, path); removed++; }
  }
  grugLog(`[syncBrain] ${brain.name} — done ${Date.now() - t0}ms (${onDisk.size} on disk, ${removed} removed)`);
}

// --- timer lifecycle ---

// Maps keyed by brain name to track active interval IDs.
// Sync timers: periodic git pull+push for writable brains with git remotes.
// Refresh timers: periodic git pull for read-only brains with source + refreshInterval.
const syncTimers = new Map();
const refreshTimers = new Map();

// Minimum allowed refresh interval (1 hour). Prevents runaway refresh storms.
const MIN_REFRESH_INTERVAL_S = 3600;

// Pulls latest content for a read-only brain from its git source, then reindexes.
// Only runs for non-writable brains with a source field and a configured refreshInterval.
// Failures are logged to stderr and never thrown — the server stays running.
async function refreshBrain(brain) {
  if (brain.writable) return; // Sync (not refresh) handles writable brains
  if (!brain.source) return;  // No source to pull from

  try {
    // git pull --ff-only: safe for read-only clones; fails cleanly if upstream rebased
    const result = await git(brain, "pull", "--ff-only", "--quiet");
    if (result === null) {
      process.stderr.write(`grug: refresh skipped for ${brain.name} (ff-only failed — upstream may have rebased)\n`);
      return;
    }
    syncBrain(brain);
    process.stderr.write(`grug: refreshed ${brain.name}\n`);
  } catch (err) {
    process.stderr.write(`grug: refresh failed for ${brain.name}: ${err.message}\n`);
  }
}

// Starts sync and/or refresh timers for a single brain.
// Idempotent: if timers already exist for this brain, they are replaced.
async function startBrainTimers(brain) {
  stopBrainTimers(brain.name); // Clear any existing timers first

  if (brain.git !== null || await hasRemote(brain)) {
    if (!await ensureGitRepo(brain)) return;
    let syncIntervalMs = brain.syncInterval * 1000;
    if (syncIntervalMs < 10000) syncIntervalMs = 10000; // Minimum 10s to prevent hammering git
    const timerId = setInterval(() => gitSync(brain).catch(err => grugLog(`[gitSync] ${brain.name} — error: ${err.message}`)), syncIntervalMs);
    syncTimers.set(brain.name, timerId);
    process.stderr.write(`grug: sync enabled for ${brain.name} (${brain.syncInterval}s interval)\n`);
  }

  if (!brain.writable && brain.source && typeof brain.refreshInterval === "number") {
    let refreshIntervalS = brain.refreshInterval;
    if (refreshIntervalS < MIN_REFRESH_INTERVAL_S) {
      process.stderr.write(`grug: refresh interval for ${brain.name} clamped to ${MIN_REFRESH_INTERVAL_S}s (was ${refreshIntervalS}s)\n`);
      refreshIntervalS = MIN_REFRESH_INTERVAL_S;
    }
    const timerId = setInterval(() => refreshBrain(brain).catch(err => grugLog(`[refreshBrain] ${brain.name} — error: ${err.message}`)), refreshIntervalS * 1000);
    refreshTimers.set(brain.name, timerId);
    process.stderr.write(`grug: refresh enabled for ${brain.name} (${refreshIntervalS}s interval)\n`);
  }
}

// Stops and removes all timers for a brain. Safe to call on unknown brain names.
function stopBrainTimers(brainName) {
  const syncId = syncTimers.get(brainName);
  if (syncId !== undefined) {
    clearInterval(syncId);
    syncTimers.delete(brainName);
  }
  const refreshId = refreshTimers.get(brainName);
  if (refreshId !== undefined) {
    clearInterval(refreshId);
    refreshTimers.delete(brainName);
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

await syncGitExclude(primaryBrain);

// Event loop block detector — logs when the loop was blocked for >500ms
let lastTick = Date.now();
setInterval(() => {
  const now = Date.now();
  const gap = now - lastTick;
  if (gap > 500) grugLog(`[heartbeat] event loop blocked for ${gap}ms`);
  lastTick = now;
}, 200);

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

function createServer() {
  return new McpServer({ name: "grug-brain", version: "3.2.0" });
}

// Default server for stdio mode; HTTP mode creates one per session.
const server = createServer();

function registerTools(server) {

// --- grug-write ---

server.tool(
  "grug-write",
  "Store a memory. Saved as markdown with frontmatter, indexed for search. Add sync: false to frontmatter to keep local-only.",
  {
    category: z.string().describe("Folder to store in, e.g. loopback, feedback, react-native"),
    path: z.string().describe("Filename for the memory, e.g. no-db-mocks"),
    content: z.string().describe("Memory content in markdown"),
    brain: z.string().optional().describe("Brain to write to (defaults to primary brain)"),
  },
  async ({ category, path: name, content, brain: brainName }) => {
    getBrains(); // Lazy reload before accessing brains
    const { brain, error } = resolveBrain(brainName);
    if (error) return { content: [{ type: "text", text: error }] };
    if (!brain.writable) return { content: [{ type: "text", text: `brain "${brain.name}" is read-only` }] };

    const cat = slugify(category);
    const catDir = join(brain.dir, cat);
    ensureDir(catDir);

    const slug = slugify(name);
    const filePath = join(catDir, `${slug}.md`);
    const exists = existsSync(filePath);

    let fileContent = content;
    if (!content.startsWith("---\n")) {
      fileContent = `---\nname: ${slug}\ndate: ${today()}\ntype: memory\n---\n\n${content}\n`;
    }

    writeFileSync(filePath, fileContent, "utf-8");
    const relPath = relative(brain.dir, filePath);
    indexFile(brain.name, relPath, filePath, cat);
    await gitCommitFile(brain, relPath, exists ? "update" : "write");

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
    const t0 = Date.now();
    grugLog(`[grug-search] query="${query}" — enter`);
    getBrains(); // Lazy reload before accessing brains
    const { results, total } = searchAll(query, page);
    grugLog(`[grug-search] query="${query}" — ${total} results ${Date.now() - t0}ms`);
    if (total === 0) return { content: [{ type: "text", text: `no matches for "${query}"` }] };

    const lines = [];
    const p = Math.max(1, page || 1);

    for (const r of results) {
      const date = r.date ? ` date:${r.date}` : "";
      lines.push(`${r.path}${date} [${r.category}] [${r.brain}]\n  ${r.snippet || r.description}`);
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
  "Read and browse brains. No args = list all brains. Brain only = list categories. Brain + category = list files. Brain + category + path = read file. Omitting brain searches primary brain first.",
  {
    brain: z.string().optional().describe("Brain name to browse (omit to list all brains)"),
    category: z.string().optional().describe("Category to browse or read from"),
    path: z.string().optional().describe("Filename within the category to read"),
  },
  async ({ brain: brainName, category, path: name }) => {
    const t0 = Date.now();
    grugLog(`[grug-read] brain=${brainName || "-"} cat=${category || "-"} path=${name || "-"} — enter`);
    const currentBrains = getBrains(); // Lazy reload before accessing brains
    // No args: list all brains with status
    if (!brainName && !category && !name) {
      if (currentBrains.length === 0) return { content: [{ type: "text", text: "no brains configured" }] };
      const lines = currentBrains.map(b => {
        const { count } = stmts.countForBrain.get(b.name);
        const flags = [
          b.primary ? "primary" : null,
          b.writable ? "writable" : "read-only",
          b.git ? "git-synced" : null,
        ].filter(Boolean).join(", ");
        return `  ${b.name}  (${count} files, ${flags})`;
      });
      return { content: [{ type: "text", text: `${currentBrains.length} brains\n\n${lines.join("\n")}` }] };
    }

    // Backwards compat: category provided without brain — search primary brain first, then all brains
    if (!brainName && category && !name) {
      let targetBrain = primaryBrain;
      const primaryRows = stmts.recallByCategory.all(primaryBrain.name, category);
      if (primaryRows.length === 0) {
        // Fall back to any brain that has this category
        for (const b of brains) {
          if (b.primary) continue;
          const rows = stmts.recallByCategory.all(b.name, category);
          if (rows.length > 0) { targetBrain = b; break; }
        }
      }
      const rows = stmts.recallByCategory.all(targetBrain.name, category);
      if (rows.length === 0) return { content: [{ type: "text", text: `no files in "${category}"` }] };
      const lines = rows.map(r => {
        const date = r.date ? ` (${r.date})` : "";
        return `- ${r.name}${date}: ${r.description}`;
      });
      return { content: [{ type: "text", text: `# ${category} [${targetBrain.name}] (${rows.length} files)\n\n${lines.join("\n")}` }] };
    }

    // Backwards compat: path provided without brain/category — try to read from primary brain
    if (!brainName && !category && name) {
      const cat = name.split("/")[0];
      const file = name.includes("/") ? name.split("/").pop() : name;
      const t = file.endsWith(".md") ? file : `${file}.md`;
      const filePath = join(primaryBrain.dir, cat, t);
      if (!existsSync(filePath)) return { content: [{ type: "text", text: `not found: ${name}` }] };
      const content = readFile(filePath);
      if (content === null) return { content: [{ type: "text", text: `could not read: ${name}` }] };
      return { content: [{ type: "text", text: content }] };
    }

    const { brain, error } = resolveBrain(brainName);
    if (error) return { content: [{ type: "text", text: error }] };

    // Brain only: list categories
    if (!category && !name) {
      const rows = stmts.categoryCounts.all(brain.name);
      if (rows.length === 0) return { content: [{ type: "text", text: `no categories in brain "${brain.name}"` }] };
      const lines = rows.map(r => `  ${r.category}  (${r.count} files)`);
      return { content: [{ type: "text", text: `${rows.length} categories in "${brain.name}"\n\n${lines.join("\n")}` }] };
    }

    // Brain + category: list files
    if (category && !name) {
      const rows = stmts.recallByCategory.all(brain.name, category);
      if (rows.length === 0) return { content: [{ type: "text", text: `no files in "${brain.name}/${category}"` }] };
      const lines = rows.map(r => {
        const date = r.date ? ` (${r.date})` : "";
        return `- ${r.name}${date}: ${r.description}`;
      });
      return { content: [{ type: "text", text: `# ${category} [${brain.name}] (${rows.length} files)\n\n${lines.join("\n")}` }] };
    }

    // Brain + category + path: read file
    const cat = category || name.split("/")[0];
    const file = name.includes("/") ? name.split("/").pop() : name;
    const t = file.endsWith(".md") ? file : `${file}.md`;
    // Flat brains: files live directly in brain.dir, not in a category subdir
    const filePath = brain.flat ? join(brain.dir, t) : join(brain.dir, cat, t);
    if (!existsSync(filePath)) {
      grugLog(`[grug-read] ${brain.name}/${cat}/${file} — not found ${Date.now() - t0}ms`);
      return { content: [{ type: "text", text: `not found: ${brain.name}/${cat}/${file}` }] };
    }

    const content = readFile(filePath);
    if (content === null) {
      grugLog(`[grug-read] ${brain.name}/${cat}/${file} — read failed ${Date.now() - t0}ms`);
      return { content: [{ type: "text", text: `could not read: ${brain.name}/${cat}/${file}` }] };
    }

    const relPath = `${cat}/${t}`;
    const linked = stmts.getLinks.all(brain.name, relPath, brain.name, relPath);
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

    grugLog(`[grug-read] ${brain.name}/${cat}/${file} — ok ${text.length} bytes ${Date.now() - t0}ms`);
    return { content: [{ type: "text", text }] };
  }
);

// --- grug-recall ---

server.tool(
  "grug-recall",
  "Get up to speed. Shows 2 most recent per category, writes full listing to recall.md in the target brain's directory.",
  {
    category: z.string().optional().describe("Filter to a specific category"),
    brain: z.string().optional().describe("Brain to recall from (defaults to primary brain)"),
  },
  async ({ category, brain: brainName }) => {
    getBrains(); // Lazy reload before accessing brains
    const { brain, error } = resolveBrain(brainName);
    if (error) return { content: [{ type: "text", text: error }] };

    const rows = category
      ? stmts.recallByCategory.all(brain.name, category)
      : stmts.recall.all(brain.name);

    if (rows.length === 0) return { content: [{ type: "text", text: `no memories found${category ? ` in "${category}"` : ""} in brain "${brain.name}"` }] };

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
    const outPath = join(primaryBrain.dir, "recall.md");
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
    brain: z.string().optional().describe("Brain to delete from (defaults to primary brain)"),
  },
  async ({ category, path: name, brain: brainName }) => {
    getBrains(); // Lazy reload before accessing brains
    const { brain, error } = resolveBrain(brainName);
    if (error) return { content: [{ type: "text", text: error }] };
    if (!brain.writable) return { content: [{ type: "text", text: `brain "${brain.name}" is read-only` }] };

    const file = name.includes("/") ? name.split("/").pop() : name;
    const t = file.endsWith(".md") ? file : `${file}.md`;
    const filePath = join(brain.dir, category, t);
    if (!existsSync(filePath)) return { content: [{ type: "text", text: `not found: ${category}/${file}` }] };

    unlinkSync(filePath);
    removeFile(brain.name, `${category}/${t}`);
    await gitCommitFile(brain, `${category}/${t}`, "delete");

    return { content: [{ type: "text", text: `deleted ${category}/${t}` }] };
  }
);

// --- grug-config ---

// Reads brains.json for config mutations. Returns the parsed array or throws.
function readBrainsJson() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  if (!existsSync(configPath)) return [];
  let raw;
  try {
    raw = JSON.parse(readFileSync(configPath, "utf-8"));
  } catch (err) {
    throw new Error(`failed to parse ${configPath}: ${err.message}`);
  }
  if (!Array.isArray(raw)) throw new Error(`${configPath} must be a JSON array`);
  return raw;
}

// Writes brains array back to brains.json.
function writeBrainsJson(brainsArray) {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  ensureDir(dirname(configPath));
  writeFileSync(configPath, JSON.stringify(brainsArray, null, 2) + "\n", "utf-8");
}

server.tool(
  "grug-config",
  "Manage brain configuration. list: show all brains. add: create a new brain entry. remove: delete a brain entry (cannot remove the primary brain).",
  {
    action: z.enum(["list", "add", "remove"]).describe("Config action to perform"),
    name: z.string().optional().describe("Brain name (required for add/remove)"),
    dir: z.string().optional().describe("Brain directory (required for add)"),
    primary: z.boolean().optional().describe("Mark as primary brain (add only, default false)"),
    writable: z.boolean().optional().describe("Mark as writable (add only, default true)"),
    flat: z.boolean().optional().describe("Flat layout — files directly in dir, no category subdirs (add only, default false)"),
    git: z.string().optional().describe("Git remote URL (add only, optional)"),
    syncInterval: z.number().optional().describe("Sync interval in seconds (add only, default 60)"),
    source: z.string().optional().describe("Source identifier for doc refresh, e.g. github:owner/repo/path (add only)"),
    refreshInterval: z.number().optional().describe("Auto-refresh interval in seconds for read-only brains (add only, minimum 3600)"),
  },
  async ({ action, name, dir, primary, writable, flat, git: gitRemote, syncInterval, source, refreshInterval }) => {
    getBrains(); // Trigger lazy reload before reading config

    if (action === "list") {
      const currentBrains = getBrains();
      if (currentBrains.length === 0) return { content: [{ type: "text", text: "no brains configured" }] };
      const lines = currentBrains.map(b => {
        const { count } = stmts.countForBrain.get(b.name);
        const flags = [
          b.primary ? "primary" : null,
          b.writable ? "writable" : "read-only",
          b.git ? `git:${b.git}` : null,
          (!b.writable && b.source && typeof b.refreshInterval === "number") ? `refresh:${b.refreshInterval}s` : null,
          syncTimers.has(b.name) ? "sync-active" : null,
          refreshTimers.has(b.name) ? "refresh-active" : null,
        ].filter(Boolean).join(", ");
        return `  ${b.name}  (${count} files, ${flags})`;
      });
      return { content: [{ type: "text", text: `${currentBrains.length} brains\n\n${lines.join("\n")}` }] };
    }

    if (action === "add") {
      if (!name) return { content: [{ type: "text", text: "add requires: name" }] };
      if (!dir) return { content: [{ type: "text", text: "add requires: dir" }] };

      // Validate name format (lowercase, hyphens, letters, digits only)
      if (!/^[a-z0-9][a-z0-9-]*$/.test(name)) {
        return { content: [{ type: "text", text: `invalid brain name "${name}": use lowercase letters, digits, and hyphens only` }] };
      }

      let existing;
      try {
        existing = readBrainsJson();
      } catch (err) {
        return { content: [{ type: "text", text: `cannot read config: ${err.message}` }] };
      }

      // Reject duplicate names
      if (existing.some(b => b.name === name)) {
        return { content: [{ type: "text", text: `brain "${name}" already exists` }] };
      }

      // Reject multiple primaries
      if (primary && existing.some(b => b.primary)) {
        return { content: [{ type: "text", text: `a primary brain already exists — set primary: false or remove the existing primary first` }] };
      }

      const resolvedDir = resolve(expandHome(dir));
      ensureDir(resolvedDir);

      const isFlat = flat === true;
      const isWritable = writable !== undefined ? writable === true : !isFlat;

      const entry = {
        name,
        dir: resolvedDir,
        primary: primary === true,
        writable: isWritable,
        flat: isFlat,
        git: gitRemote || null,
        syncInterval: typeof syncInterval === "number" ? syncInterval : 60,
      };
      if (source) entry.source = source;
      if (typeof refreshInterval === "number") entry.refreshInterval = refreshInterval;

      existing.push(entry);
      try {
        writeBrainsJson(existing);
      } catch (err) {
        return { content: [{ type: "text", text: `failed to write config: ${err.message}` }] };
      }

      // Force a reload so the new brain is live immediately
      brainsJsonMtime = 0;
      reloadBrains();

      // Start timers for the new brain (reloadBrains already called startBrainTimers)
      const newBrain = getBrains().find(b => b.name === name);
      if (!newBrain) {
        return { content: [{ type: "text", text: `added brain "${name}" to config but dir may not exist yet` }] };
      }
      return { content: [{ type: "text", text: `added brain "${name}" — dir: ${resolvedDir}` }] };
    }

    if (action === "remove") {
      if (!name) return { content: [{ type: "text", text: "remove requires: name" }] };

      let existing;
      try {
        existing = readBrainsJson();
      } catch (err) {
        return { content: [{ type: "text", text: `cannot read config: ${err.message}` }] };
      }

      const entry = existing.find(b => b.name === name);
      if (!entry) return { content: [{ type: "text", text: `no brain named "${name}"` }] };
      if (entry.primary) return { content: [{ type: "text", text: `cannot remove the primary brain "${name}"` }] };

      // Stop timers before removing from config
      stopBrainTimers(name);

      // Remove from FTS index
      const indexed = stmts.allFilesForBrain.all(name).map(r => r.path);
      for (const relPath of indexed) removeFile(name, relPath);

      // Write updated config (files on disk are preserved)
      const updated = existing.filter(b => b.name !== name);
      try {
        writeBrainsJson(updated);
      } catch (err) {
        return { content: [{ type: "text", text: `failed to write config: ${err.message}` }] };
      }

      // Force reload to pick up the removal
      brainsJsonMtime = 0;
      reloadBrains();

      return { content: [{ type: "text", text: `removed brain "${name}" from config (files preserved at ${entry.dir || "?"})` }] };
    }

    return { content: [{ type: "text", text: `unknown action "${action}"` }] };
  }
);

// --- grug-sync ---

server.tool(
  "grug-sync",
  "Reindex a brain (or all brains) from disk. Use after adding files outside of grug-write, e.g. copying transcriptions into a brain directory.",
  {
    brain: z.string().optional().describe("Brain to reindex (omit to reindex all brains)"),
  },
  async ({ brain: brainName }) => {
    getBrains();
    const targets = brainName
      ? brains.filter(b => b.name === brainName)
      : brains;
    if (brainName && targets.length === 0) {
      return { content: [{ type: "text", text: `unknown brain "${brainName}"` }] };
    }
    const results = [];
    for (const brain of targets) {
      const before = stmts.countForBrain.get(brain.name).count;
      syncBrain(brain);
      const after = stmts.countForBrain.get(brain.name).count;
      const diff = after - before;
      const delta = diff > 0 ? ` (+${diff} new)` : diff < 0 ? ` (${diff} removed)` : "";
      results.push(`${brain.name}: ${after} files${delta}`);
    }
    return { content: [{ type: "text", text: results.join("\n") }] };
  }
);

// --- grug-dream ---

// Collect all memories across all brains that need review.
function collectAllMemories() {
  const all = [];
  for (const brain of brains) {
    for (const row of stmts.recall.all(brain.name)) {
      all.push({ ...row, brainName: brain.name });
    }
  }
  return all;
}

// Commit pending changes for a single writable brain with git.
// Acquires sync lock to prevent concurrent git operations.
// Returns the git log string, or null if git is unavailable.
async function dreamCommitBrain(brain) {
  if (!await ensureGitRepo(brain)) return null;
  if (!acquireSyncLock(brain)) return null;
  try {
    await syncGitExclude(brain);
    await git(brain, "add", "-A");
    await git(brain, "commit", "-m", "grug: dream sync", "--quiet");
    return await git(brain, "log", "--oneline", "--name-status", "-15", "--", ".");
  } finally {
    releaseSyncLock(brain);
  }
}

server.tool(
  "grug-dream",
  "Dream: review memory health across all brains. Commits pending changes to git, shows history, finds cross-links, flags stale memories and conflicts. Use with /loop for periodic maintenance.",
  {},
  async () => {
    getBrains(); // Lazy reload before accessing brains
    // Sync all brains before inspecting
    for (const brain of brains) syncBrain(brain);

    const all = collectAllMemories();
    if (all.length === 0) {
      return { content: [{ type: "text", text: "nothing to dream about — no memories yet" }] };
    }

    const now = Date.now();
    const ts = new Date().toISOString();
    const sections = [];

    // --- commit pending changes per writable brain with git ---
    const writableGitBrains = [];
    for (const b of brains) {
      if (b.writable && await ensureGitRepo(b)) writableGitBrains.push(b);
    }
    if (writableGitBrains.length > 0) {
      const historyLines = [];
      for (const brain of writableGitBrains) {
        const log = await dreamCommitBrain(brain);
        if (log) {
          historyLines.push(`### ${brain.name}\n\n\`\`\`\n${log}\n\`\`\``);
        } else {
          historyLines.push(`### ${brain.name}\n\nno commits yet`);
        }
      }
      sections.push(`## recent history\n\n${historyLines.join("\n\n")}`);
    }

    // --- conflicts: entries in the conflicts/ category ---
    const conflictRows = stmts.recallByCategory.all(primaryBrain.name, "conflicts");
    if (conflictRows.length > 0) {
      const conflictLines = conflictRows.map(r => {
        // Read frontmatter for original_path, original_brain, hostname, date
        const filePath = join(primaryBrain.dir, r.path);
        const content = readFile(filePath);
        const fm = content ? extractFrontmatter(content) : {};
        const origin = fm.original_path
          ? `${fm.original_brain || "?"}/${fm.original_path}`
          : r.path;
        const host = fm.hostname ? ` (from ${fm.hostname})` : "";
        const date = fm.date ? ` — ${fm.date}` : "";
        return `- **${r.name}**${date}${host}: original: \`${origin}\`\n  Resolve: read with \`grug-read brain:${primaryBrain.name} category:conflicts path:${basename(r.path, ".md")}\`, then \`grug-write\` to the original location and \`grug-delete\` the conflict entry.`;
      });
      sections.push(`## conflicts (${conflictRows.length})\n\nThese files had git merge conflicts and were saved here. Review each, write the correct version to the original location, then delete the conflict entry.\n\n${conflictLines.join("\n\n")}`);
    }

    // --- which memories need attention? ---
    const needsReview = new Set();
    for (const brain of brains) {
      for (const row of stmts.needsDream.all(brain.name)) {
        needsReview.add(`${brain.name}:${row.path}`);
      }
    }

    if (needsReview.size === 0 && conflictRows.length === 0) {
      let totalFiles = 0;
      let totalCats = 0;
      for (const brain of brains) {
        totalFiles += stmts.countForBrain.get(brain.name).count;
        totalCats += stmts.categoryCounts.all(brain.name).length;
      }
      sections.unshift(`# dream report\n\n${totalFiles} memories | ${totalCats} categories | all clean — nothing needs review`);
      return { content: [{ type: "text", text: sections.join("\n\n") }] };
    }

    // Filter to only memories needing review
    const toReview = all.filter(m => needsReview.has(`${m.brainName}:${m.path}`));

    // --- cross-links across all brains (rebuild for reviewed memories) ---
    const links = [];
    const seen = new Set();

    for (const mem of toReview) {
      stmts.deleteLinks.run(mem.brainName, mem.path, mem.brainName, mem.path);
      const terms = mem.name.replace(/[-_]/g, " ").split(/\s+/).filter(t => t.length > 3);
      if (terms.length === 0) continue;
      const q = terms.slice(0, 3).map(t => `"${t}"`).join(" OR ");
      try {
        const matches = stmts.search.all(q, 5, 0);
        for (const m of matches) {
          if (m.path === mem.path && m.brain === mem.brainName) continue;
          if (m.category === mem.category && m.brain === mem.brainName) continue;
          // Sort by brain+path for stable primary key
          const [[pA, bA], [pB, bB]] = `${mem.brainName}:${mem.path}` <= `${m.brain}:${m.path}`
            ? [[mem.path, mem.brainName], [m.path, m.brain]]
            : [[m.path, m.brain], [mem.path, mem.brainName]];
          const key = `${bA}:${pA}|${bB}:${pB}`;
          if (seen.has(key)) continue;
          seen.add(key);
          stmts.upsertLink.run(bA, pA, bB, pB, m.rank, ts);
          const brainTagA = bA !== primaryBrain.name ? ` [${bA}]` : "";
          const brainTagB = bB !== primaryBrain.name ? ` [${bB}]` : "";
          links.push({
            a: `${mem.name} [${mem.category}]${brainTagA}`,
            b: `${m.name} [${m.category}]${brainTagB}`,
            rank: m.rank,
          });
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
      sections.push(`## stale (${stale.length} memories > ${STALE_DAYS} days)\n\n${stale.map(s => {
        const brainTag = s.brainName !== primaryBrain.name ? ` [${s.brainName}]` : "";
        return `- ${s.name} [${s.category}]${brainTag} — ${s.age}d (${s.date}): ${s.description}`;
      }).join("\n")}`);
    }

    // --- quality issues (only unreviewed) ---
    const issues = toReview.filter(m => !m.date || !m.description);
    if (issues.length > 0) {
      sections.push(`## quality issues\n\n${issues.map(m => {
        const brainTag = m.brainName !== primaryBrain.name ? ` [${m.brainName}]` : "";
        return `- ${m.name} [${m.category}]${brainTag}: ${!m.date ? "no date" : "no description"}`;
      }).join("\n")}`);
    }

    // --- needs review ---
    const reviewLines = toReview.map(m => {
      const date = m.date ? ` ${m.date}` : "";
      const brainTag = m.brainName !== primaryBrain.name ? ` [${m.brainName}]` : "";
      return `- ${m.name} [${m.category}]${brainTag}${date}: ${m.description}`;
    });
    sections.push(`## needs review (${toReview.length} memories)\n\n${reviewLines.join("\n")}`);

    // --- header ---
    let totalFiles = 0;
    let totalCats = 0;
    for (const brain of brains) {
      totalFiles += stmts.countForBrain.get(brain.name).count;
      totalCats += stmts.categoryCounts.all(brain.name).length;
    }
    const conflictNote = conflictRows.length > 0 ? ` | ${conflictRows.length} conflicts` : "";
    const summary = `${totalFiles} memories | ${totalCats} categories | ${toReview.length} need review | ${links.length} cross-links | ${stale.length} stale${conflictNote}`;
    sections.unshift(`# dream report\n\n${summary}\n\nOnly showing memories that are new or changed since last dream. Use grug-write to update, grug-delete to remove.`);

    // --- mark reviewed ---
    for (const m of toReview) {
      const file = stmts.getFile.get(m.brainName, m.path);
      if (file) stmts.upsertDreamLog.run(m.brainName, m.path, ts, file.mtime);
    }

    return { content: [{ type: "text", text: sections.join("\n\n") }] };
  }
);

// --- grug-docs (deprecated alias for grug-read, filters to non-primary brains) ---

// Resolve a relPath (as stored in brain_fts) to an on-disk absolute path.
// For category brains: relPath is "cat/file", baseDir is derived from catBrainDir.
// For flat brains: relPath has no category prefix — check each flat brain's dir.
function resolveDocPath(relPath) {
  const firstPart = relPath.split("/")[0];

  // Category brain: first path segment is a category, catBrainDir maps it to base dir
  const baseDir = catBrainDir.get(firstPart);
  if (baseDir) {
    const full = join(baseDir, relPath);
    if (existsSync(full)) return full;
  }

  // Flat brain: no category prefix — check each flat brain's dir
  for (const b of brains) {
    if (!b.flat) continue;
    const full = join(b.dir, relPath);
    if (existsSync(full)) return full;
  }

  return null;
}

// Deprecated: use grug-read with a brain parameter instead.
server.tool(
  "grug-docs",
  "[Deprecated: use grug-read] Browse documentation brains (non-primary brains).",
  {
    category: z.string().optional().describe("Doc category to browse"),
    path: z.string().optional().describe("File path to read"),
    page: z.number().optional().describe("Page number for long files"),
  },
  async ({ category, path: target, page }) => {
    // Use getBrains() so this reflects the current config after lazy reload
    const currentNonPrimary = getBrains().filter(b => !b.primary);

    // No args: list categories across all non-primary brains
    if (!category && !target) {
      const rows = stmts.allCategoryCounts.all().filter(r => r.brain !== primaryBrain.name);
      if (rows.length === 0) return { content: [{ type: "text", text: "no docs found" }] };
      const lines = rows.map(r => `  ${r.category}  (${r.count} docs)`);
      return { content: [{ type: "text", text: `${rows.length} doc categories\n\n${lines.join("\n")}` }] };
    }

    // Path provided: resolve and read file
    if (target) {
      let filePath = resolveDocPath(target);
      if (!filePath) filePath = resolve(target);
      if (!filePath || !existsSync(filePath)) return { content: [{ type: "text", text: `file not found: ${target}` }] };
      const content = readFile(filePath);
      if (content === null) return { content: [{ type: "text", text: `could not read: ${target}` }] };
      return { content: [{ type: "text", text: paginate(content, page) }] };
    }

    // Category only: list files in first matching non-primary brain
    const matchingBrain = currentNonPrimary.find(b => {
      return stmts.categoryCounts.all(b.name).some(r => r.category === category);
    });
    if (!matchingBrain) return { content: [{ type: "text", text: `no docs in "${category}"` }] };

    const p = Math.max(1, page || 1);
    const offset = (p - 1) * BROWSE_PAGE_SIZE;
    const { total } = stmts.countByCategory.get(matchingBrain.name, category);
    if (total === 0) return { content: [{ type: "text", text: `no docs in "${category}"` }] };
    const rows = stmts.listByCategory.all(matchingBrain.name, category, BROWSE_PAGE_SIZE, offset);
    const lines = rows.map(r => `- [${r.name}](${r.path}): ${r.description || ""}`);
    const totalPages = Math.ceil(total / BROWSE_PAGE_SIZE);
    const paging = totalPages > 1
      ? `\n--- page ${p}/${totalPages} (${total} docs) | page:${p + 1} for more ---`
      : "";
    return { content: [{ type: "text", text: `# ${category} (${total} docs)\n\n${lines.join("\n")}${paging}` }] };
  }
);

} // end registerTools

registerTools(server);

// --- transport setup ---
// Use --stdio flag for stdio transport, otherwise default to HTTP.

const useStdio = process.argv.includes("--stdio");
const GRUG_PORT = parseInt(process.env.GRUG_PORT || "6483", 10);

if (useStdio) {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  grugLog(`[transport] stdio connected`);

  // Exit when stdin closes — parent session (e.g. Claude Code) disconnected.
  // Without this, setInterval timers keep the process alive as a zombie,
  // and dozens of stale processes pile up fighting over git and SQLite.
  function shutdown() {
    grugLog(`[transport] stdin closed, shutting down`);
    for (const brain of brains) stopBrainTimers(brain.name);
    process.exit(0);
  }
  process.stdin.on("end", shutdown);
  process.stdin.on("close", shutdown);
} else {
  // HTTP transport: one McpServer + transport per session.
  // McpServer only allows one transport, so each client session gets its own instance.
  const sessions = new Map(); // sessionId -> { server, transport }

  Bun.serve({
    port: GRUG_PORT,
    idleTimeout: 255, // seconds — max allowed by Bun; prevents timeout during git sync
    async fetch(req) {
      const url = new URL(req.url);
      if (url.pathname !== "/mcp") {
        return new Response(JSON.stringify({ error: "not found" }), {
          status: 404,
          headers: { "Content-Type": "application/json" },
        });
      }

      const sessionId = req.headers.get("mcp-session-id");

      if (sessionId && sessions.has(sessionId)) {
        const { transport } = sessions.get(sessionId);
        return transport.handleRequest(req);
      }

      if (!sessionId && req.method === "POST") {
        const body = await req.json();
        if (isInitializeRequest(body)) {
          const sessionServer = createServer();
          registerTools(sessionServer);
          const transport = new WebStandardStreamableHTTPServerTransport({
            sessionIdGenerator: () => crypto.randomUUID(),
            onsessioninitialized: (sid) => {
              sessions.set(sid, { server: sessionServer, transport });
              grugLog(`[transport] session created ${sid}`);
            },
          });
          transport.onclose = () => {
            const sid = transport.sessionId;
            if (sid) sessions.delete(sid);
            grugLog(`[transport] session closed ${sid}`);
          };
          await sessionServer.connect(transport);
          return transport.handleRequest(req, { parsedBody: body });
        }
      }

      return new Response(JSON.stringify({
        jsonrpc: "2.0",
        error: { code: -32000, message: "Bad Request: no valid session" },
        id: null,
      }), { status: 400, headers: { "Content-Type": "application/json" } });
    },
  });

  grugLog(`[transport] HTTP listening on port ${GRUG_PORT}`);
}

// --- per-brain sync and refresh timers ---
// startBrainTimers handles both git sync (writable) and doc refresh (read-only) timers.

for (const brain of brains) {
  startBrainTimers(brain);
}

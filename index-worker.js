// Background worker for file indexing.
// Handles CPU-intensive file walking, reading, and parsing off the main thread.
// Main thread sends sync requests, worker sends back parsed data for SQLite insertion.

import { readdirSync, readFileSync, statSync, existsSync } from "fs";
import { join, relative, basename, extname } from "path";

function isDir(p) {
  try { return statSync(p).isDirectory(); } catch { return false; }
}

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

function getCategories(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter(name => !name.startsWith(".") && isDir(join(dir, name)))
    .sort();
}

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

self.onmessage = (event) => {
  const { id, type, brain, indexed } = event.data;

  if (type === "sync") {
    const t0 = Date.now();
    const indexedMap = new Map(indexed);
    const onDisk = new Set();
    const toIndex = [];
    const categories = [];

    if (brain.flat) {
      categories.push(brain.name);
      for (const fullPath of walkFiles(brain.dir)) {
        const relPath = relative(brain.dir, fullPath);
        onDisk.add(relPath);
        let mtime;
        try { mtime = statSync(fullPath).mtimeMs; } catch { continue; }
        if (indexedMap.get(relPath) !== mtime) {
          let content;
          try { content = readFileSync(fullPath, "utf-8"); } catch { continue; }
          const fm = extractFrontmatter(content);
          const body = extractBody(content);
          const desc = extractDescription(content);
          const name = fm.name || fm.title || basename(relPath, extname(relPath));
          toIndex.push({ relPath, category: brain.name, name, date: fm.date || "", desc, body, mtime });
        }
      }
    } else {
      for (const cat of getCategories(brain.dir)) {
        categories.push(cat);
        for (const fullPath of walkFiles(join(brain.dir, cat))) {
          const relPath = relative(brain.dir, fullPath);
          onDisk.add(relPath);
          let mtime;
          try { mtime = statSync(fullPath).mtimeMs; } catch { continue; }
          if (indexedMap.get(relPath) !== mtime) {
            let content;
            try { content = readFileSync(fullPath, "utf-8"); } catch { continue; }
            const fm = extractFrontmatter(content);
            const body = extractBody(content);
            const desc = extractDescription(content);
            const name = fm.name || fm.title || basename(relPath, extname(relPath));
            toIndex.push({ relPath, category: cat, name, date: fm.date || "", desc, body, mtime });
          }
        }
      }
    }

    const toRemove = [];
    for (const [path] of indexedMap) {
      if (!onDisk.has(path)) toRemove.push(path);
    }

    self.postMessage({
      id,
      type: "sync-result",
      brainName: brain.name,
      brainDir: brain.dir,
      brainFlat: brain.flat,
      toIndex,
      toRemove,
      categories,
      onDiskCount: onDisk.size,
      duration: Date.now() - t0,
    });
  }
};

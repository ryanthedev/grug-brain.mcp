/**
 * Data loaders — loadMemories, findCategory, loadPreview, loadGraph.
 *
 * loadBrains lives in router.js because it calls router.navigate().
 * These loaders are imported by router.js (onRoute calls them), so they
 * must NOT import router.js to avoid a circular dependency.
 *
 * Exported API:
 *   loadMemories(brain)               — fetch + store memory list
 *   findCategory(path)                — look up category from cached memories
 *   loadPreview(brain, path, category)— fetch memory detail + build editor buffer
 *   loadGraph(brain)                  — fetch graph data + trigger graph render
 */
import { api } from './api.js';
import { state } from './state.js';
import { toast } from './toast.js';
import { frontmatter } from './frontmatter.js';
import { graph } from './graph.js';

export async function loadMemories(brain) {
  const r = await api.memories(brain);
  if (!r.ok) { toast.show(r.error); return; }
  const memories = Array.isArray(r.data) ? r.data : [];
  state.set({ memories, activeBrain: brain });
}

export function findCategory(path) {
  const m = state.get().memories.find(m => m.path === path);
  return m ? m.category : null;
}

export async function loadPreview(brain, path, category) {
  const cat = category || findCategory(path);
  if (!cat) return;
  // Strip category prefix and .md extension to get the API filename.
  const filename = path.replace(/^[^/]+\//, "").replace(/\.md$/, "");
  const r = await api.memory(brain, cat, filename);
  if (!r.ok) { toast.show(r.error); return; }
  // Build a fresh editor buffer from the preview payload. Server returns
  // frontmatter as a string-keyed map; we lift name/description/tags out and
  // keep tags as a string[] on the client.
  const fmRaw = (r.data && r.data.frontmatter) || {};
  const tagsStr = fmRaw.tags || "";
  const fmObj = {
    name: fmRaw.name || "",
    description: fmRaw.description || "",
    tags: frontmatter.parseTags(tagsStr),
  };
  const buf = {
    body: r.data.body || "",
    frontmatter: fmObj,
    etag: typeof r.data.mtime === "number" ? r.data.mtime : 0,
    originalBody: r.data.body || "",
    originalFrontmatter: JSON.parse(JSON.stringify(fmObj)),
  };
  state.set({
    preview: r.data,
    activeMemoryPath: path,
    buffer: buf,
    dirty: false,
  });
}

export async function loadGraph(brain) {
  const r = await api.graph(brain);
  if (!r.ok) { toast.show(r.error); return; }
  state.set({ graphData: r.data });
  graph.render(r.data);
}

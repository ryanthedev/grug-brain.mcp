/**
 * Hash-based router — parse/navigate/onRoute.
 *
 * loadBrains lives here (not in loaders.js) because it calls router.navigate()
 * and importing router from loaders.js would create a cycle.
 *
 * Exported API:
 *   router.parse(hash)    — parse hash string into {brain, category, memoryPath}
 *   router.navigate(patch)— async; build hash, guard dirty state, push to location
 *   router.onRoute()      — async; sync state from current URL hash
 *   loadBrains()          — fetch brains + navigate to primary
 */
import { state } from './state.js';
import { api } from './api.js';
import { toast } from './toast.js';
import { HASH_PREFIX } from './constants.js';
import { nav } from './nav.js';
import { loadMemories, findCategory, loadPreview, loadGraph } from './loaders.js';

/** Parse #/brain/X/category/Y/memory/Z into {brain, category, memoryPath}. */
function parse(hash) {
  const s = hash.startsWith(HASH_PREFIX) ? hash.slice(HASH_PREFIX.length) : hash.slice(1);
  const parts = s.split("/");
  const result = { brain: null, category: null, memoryPath: null };
  for (let i = 0; i < parts.length - 1; i += 2) {
    const key = parts[i];
    const val = decodeURIComponent(parts[i + 1] || "");
    if (key === "brain") result.brain = val;
    if (key === "category") result.category = val;
    if (key === "memory") result.memoryPath = val;
  }
  return result;
}

/** Build a hash and push it. Missing params default to current state. */
async function navigate(patch) {
  const s = state.get();
  const brain = patch.brain !== undefined ? patch.brain : s.activeBrain;
  const category = patch.category !== undefined ? patch.category : s.activeCategory;
  const memoryPath = patch.memoryPath !== undefined ? patch.memoryPath : s.activeMemoryPath;

  // Unsaved-changes guard: only block when the navigation actually leaves
  // the current memory or brain (category-only changes don't risk loss).
  const leavesMemory = memoryPath !== s.activeMemoryPath || brain !== s.activeBrain;
  if (leavesMemory && s.dirty) {
    const proceed = await nav.guard();
    if (!proceed) return;
    // User chose Discard — clear dirty so the route handler doesn't trigger
    // the guard again on subsequent state syncs.
    state.set({ dirty: false });
  }

  let hash = `#/brain/${encodeURIComponent(brain)}`;
  if (category) hash += `/category/${encodeURIComponent(category)}`;
  if (memoryPath) hash += `/memory/${encodeURIComponent(memoryPath)}`;
  window.location.hash = hash;
}

/** Called on hashchange and initial load. Syncs state to URL. */
async function onRoute() {
  const route = parse(window.location.hash);
  const s = state.get();

  const brainChanged = route.brain && route.brain !== s.activeBrain;
  if (brainChanged) {
    state.set({
      activeBrain: route.brain,
      activeCategory: route.category,
      activeMemoryPath: route.memoryPath,
      preview: null,
    });
    await loadMemories(route.brain);
    await loadGraph(route.brain);
  } else {
    state.set({ activeCategory: route.category, activeMemoryPath: route.memoryPath });
  }

  if (route.memoryPath && (brainChanged || route.memoryPath !== s.activeMemoryPath)) {
    const cat = route.category || findCategory(route.memoryPath);
    if (cat) loadPreview(route.brain || s.activeBrain, route.memoryPath, cat);
  } else if (!route.memoryPath) {
    state.set({ preview: null });
  }
}

export async function loadBrains() {
  const r = await api.brains();
  if (!r.ok) { toast.show(r.error); return; }
  const brains = Array.isArray(r.data) ? r.data : [];
  state.set({ brains });

  const s = state.get();
  if (!s.activeBrain && brains.length > 0) {
    const primary = brains.find(b => b.primary) || brains[0];
    navigate({ brain: primary.name });
  }
}

export const router = { parse, navigate, onRoute };

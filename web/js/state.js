/**
 * Application state — pub-sub store.
 *
 * All mutations go through state.set(). Subscribers are notified after
 * every set() call with the new state snapshot.
 *
 * The window.__grugState Playwright hook is wired in app.js after import,
 * not here, so this module has no DOM or window dependencies.
 */
export const state = (() => {
  let s = {
    brains: [],       // [{name, primary, writable, source, flat}]
    activeBrain: "",  // currently-selected brain name
    memories: [],     // [{path, brain, category, name, description, date, mtime}]
    activeCategory: null, // string or null (all categories)
    activeMemoryPath: null,
    preview: null,    // {frontmatter, body, mtime, neighbors} or null
    graphData: null,  // {nodes, edges} or null
    loading: false,
    // editor state
    mode: "edit",     // "edit" | "read"
    buffer: null,     // {body, frontmatter:{name,description,tags[]}, etag, originalBody, originalFrontmatter}
    dirty: false,     // true when buffer differs from original
    scrollPositions: { read: 0, edit: 0 },
  };

  /** Subscribers notified on every state.set() call. */
  const subs = [];

  return {
    get: () => s,
    set(patch) {
      s = Object.assign({}, s, patch);
      subs.forEach(fn => fn(s));
    },
    subscribe(fn) { subs.push(fn); },
  };
})();

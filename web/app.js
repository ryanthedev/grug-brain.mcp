/**
 * grug-brain viewer — single-file vanilla JS SPA.
 *
 * Security model:
 *   - All user-controlled data (memory names, descriptions, body text) is
 *     either escaped via escapeHtml() before insertion into innerHTML, or
 *     inserted via textContent. The sole exception is the markdown preview
 *     pane, which MUST render HTML — that path goes through DOMPurify.sanitize()
 *     with an explicit allowlist before any innerHTML assignment.
 *   - Raw memory body HTML is NEVER inserted without DOMPurify.
 *
 * Architecture: IIFE with internal pub-sub state machine.
 *   api.*    — HTTP helpers, always return {ok, data, error}
 *   state.*  — application state + subscriber notification
 *   render.* — DOM rendering from state
 *   router.* — hash-based routing (#/brain/NAME/category/CAT/memory/PATH)
 *   sse.*    — SSE client for live reload
 *   graph.*  — cytoscape graph panel
 *   toast.*  — error toast component
 *   theme.*  — light/dark/system theme toggle
 */
(function () {
  "use strict";

  // ── Constants ──────────────────────────────────────────────────────────────

  const HASH_PREFIX = "#/";
  const RELOAD_DEBOUNCE_MS = 500;
  const SSE_RECONNECT_MS = 3000;

  // ── State ──────────────────────────────────────────────────────────────────

  /** Application state. All mutations go through state.set(). */
  const state = (() => {
    let s = {
      brains: [],       // [{name, primary, writable, source, flat}]
      activeBrain: "",  // currently-selected brain name
      memories: [],     // [{path, brain, category, name, description, date, mtime}]
      activeCategory: null, // string or null (all categories)
      activeMemoryPath: null,
      preview: null,    // {frontmatter, body, mtime, neighbors} or null
      graphData: null,  // {nodes, edges} or null
      loading: false,
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

  // ── API helpers ────────────────────────────────────────────────────────────

  const api = {
    /** Fetch JSON from the grug API. Returns {ok, data, error}. */
    async get(path) {
      try {
        const resp = await fetch(path);
        if (!resp.ok) {
          let msg = `HTTP ${resp.status}`;
          try { const j = await resp.json(); msg = j.error || msg; } catch (_) {}
          return { ok: false, error: msg };
        }
        const data = await resp.json();
        return { ok: true, data };
      } catch (e) {
        return { ok: false, error: e.message || "network error" };
      }
    },

    async brains() { return this.get("/api/brains"); },
    async memories(brain) {
      return this.get(`/api/memories?brain=${encodeURIComponent(brain)}`);
    },
    async memory(brain, category, path) {
      return this.get(
        `/api/memory/${encodeURIComponent(brain)}/${encodeURIComponent(category)}/${encodeURIComponent(path)}`
      );
    },
    async graph(brain) {
      return this.get(`/api/graph?brain=${encodeURIComponent(brain)}&mode=global`);
    },
  };

  // ── Toast ──────────────────────────────────────────────────────────────────

  const toast = (() => {
    /** Show an error toast. msg is a server error string — escaped before insertion. */
    function show(msg) {
      const container = document.getElementById("toast-container");
      if (!container) return;

      const el = document.createElement("div");
      el.className = "toast";
      el.setAttribute("role", "alert");
      el.setAttribute("aria-live", "assertive");

      const body = document.createElement("div");
      body.className = "toast-body";

      const title = document.createElement("div");
      title.className = "toast-title";
      title.textContent = "Error"; // static string, safe

      const message = document.createElement("div");
      message.className = "toast-message";
      message.textContent = msg; // textContent — no XSS risk

      body.appendChild(title);
      body.appendChild(message);

      const copyBtn = document.createElement("button");
      copyBtn.className = "toast-copy";
      copyBtn.setAttribute("aria-label", "Copy error to clipboard");
      copyBtn.textContent = "Copy"; // static
      copyBtn.addEventListener("click", () => {
        navigator.clipboard.writeText(msg).catch(() => {});
      });

      const closeBtn = document.createElement("button");
      closeBtn.className = "toast-close";
      closeBtn.setAttribute("aria-label", "Dismiss error");
      closeBtn.textContent = "×"; // × via unicode — no HTML injection
      closeBtn.addEventListener("click", () => el.remove());

      el.appendChild(body);
      el.appendChild(copyBtn);
      el.appendChild(closeBtn);
      container.appendChild(el);

      // Auto-dismiss after 8s.
      setTimeout(() => { if (el.parentNode) el.remove(); }, 8000);
    }

    return { show };
  })();

  // ── Theme ──────────────────────────────────────────────────────────────────

  const theme = (() => {
    const MODES = ["system", "light", "dark"];
    const KEY = "grug-theme";

    function apply(mode) {
      const root = document.documentElement;
      if (mode === "dark") {
        root.dataset.theme = "dark";
      } else if (mode === "light") {
        root.dataset.theme = "light";
      } else {
        // System: check prefers-color-scheme.
        const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
        root.dataset.theme = prefersDark ? "dark" : "light";
      }
      root.dataset.themeMode = mode;
    }

    function current() {
      return localStorage.getItem(KEY) || "system";
    }

    function init() {
      apply(current());
      // Respond to OS theme changes when in system mode.
      window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
        if (current() === "system") apply("system");
      });
    }

    /** Cycle: system → light → dark → system. */
    function toggle() {
      const idx = MODES.indexOf(current());
      const next = MODES[(idx + 1) % MODES.length];
      localStorage.setItem(KEY, next);
      apply(next);
      updateToggleLabel();
    }

    function updateToggleLabel() {
      const btn = document.getElementById("theme-toggle");
      if (!btn) return;
      const m = current();
      const icons = { system: "auto", light: "light", dark: "dark" };
      btn.textContent = icons[m] || "auto"; // static keys, safe
      btn.setAttribute("aria-label", `Switch theme (current: ${m})`);
    }

    return { init, toggle, updateToggleLabel };
  })();

  // ── Render ─────────────────────────────────────────────────────────────────

  const render = {
    /** Render the brain switcher buttons via safe DOM construction. */
    brains(brains, activeBrain) {
      const el = document.getElementById("brain-switcher");
      if (!el) return;

      // Preserve the SSE status dot before clearing.
      const dot = document.getElementById("sse-status");

      // Clear children except the dot (which we re-append at the end).
      while (el.firstChild) el.removeChild(el.firstChild);

      brains.forEach(b => {
        const btn = document.createElement("button");
        btn.className = "brain-btn" +
          (b.name === activeBrain ? " active" : "") +
          (b.primary ? " primary" : "");
        btn.setAttribute("aria-pressed", b.name === activeBrain ? "true" : "false");
        btn.dataset.brain = b.name;

        // Brain name — inserted via textContent (no XSS risk).
        const nameSpan = document.createElement("span");
        nameSpan.textContent = b.name;
        btn.appendChild(nameSpan);

        if (!b.writable) {
          const badge = document.createElement("span");
          badge.className = "brain-badge";
          badge.textContent = "read-only"; // static
          btn.appendChild(badge);
        }
        if (b.source) {
          // Source URL in title — used as tooltip text, not rendered as HTML.
          btn.title = b.source;
        }

        btn.addEventListener("click", () => {
          router.navigate({ brain: b.name, category: null, memoryPath: null });
        });
        el.appendChild(btn);
      });

      if (dot) el.appendChild(dot);
    },

    /** Render the category tree via safe DOM construction. */
    categories(memories, activeCategory) {
      const el = document.getElementById("category-tree");
      if (!el) return;

      // Count memories per category.
      const cats = {};
      memories.forEach(m => {
        cats[m.category] = (cats[m.category] || 0) + 1;
      });

      while (el.firstChild) el.removeChild(el.firstChild);

      // "All" item.
      el.appendChild(makeCategoryItem(null, "All memories", memories.length, activeCategory));

      // Per-category items.
      Object.keys(cats).sort().forEach(cat => {
        el.appendChild(makeCategoryItem(cat, cat, cats[cat], activeCategory));
      });
    },

    /** Render the memory list via safe DOM construction. */
    memoryList(memories, activeCategory, activeMemoryPath, searchTerm) {
      const panel = document.getElementById("memory-list-panel");
      const listEl = document.getElementById("memory-list");
      if (!listEl) return;

      // Apply category filter.
      let filtered = activeCategory
        ? memories.filter(m => m.category === activeCategory)
        : memories;

      // Apply client-side search filter.
      if (searchTerm) {
        const q = searchTerm.toLowerCase();
        filtered = filtered.filter(m =>
          (m.name || "").toLowerCase().includes(q) ||
          (m.description || "").toLowerCase().includes(q)
        );
      }

      while (listEl.firstChild) listEl.removeChild(listEl.firstChild);

      // Empty state: show when the brain has no memories at all.
      const emptyState = document.getElementById("empty-state");
      if (memories.length === 0) {
        if (emptyState) emptyState.classList.add("visible");
        if (panel) panel.style.display = "none";
        return;
      }
      if (emptyState) emptyState.classList.remove("visible");
      if (panel) panel.style.display = "";

      filtered.forEach(m => {
        const li = document.createElement("li");
        li.className = "memory-item" + (m.path === activeMemoryPath ? " active" : "");
        li.setAttribute("role", "option");
        li.setAttribute("aria-selected", m.path === activeMemoryPath ? "true" : "false");
        li.setAttribute("tabindex", "0");
        li.dataset.path = m.path;
        li.dataset.category = m.category;

        // All user content inserted via textContent.
        const nameDiv = document.createElement("div");
        nameDiv.className = "mem-name";
        nameDiv.textContent = m.name || m.path;
        li.appendChild(nameDiv);

        if (m.description) {
          const descDiv = document.createElement("div");
          descDiv.className = "mem-desc";
          descDiv.textContent = m.description;
          li.appendChild(descDiv);
        }
        if (m.date) {
          const dateDiv = document.createElement("div");
          dateDiv.className = "mem-date";
          dateDiv.textContent = m.date;
          li.appendChild(dateDiv);
        }

        const openMemory = () => {
          router.navigate({ memoryPath: m.path, memoryCategory: m.category });
        };
        li.addEventListener("click", openMemory);
        li.addEventListener("keydown", e => {
          if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openMemory(); }
        });

        listEl.appendChild(li);
      });
    },

    /**
     * Render the markdown preview pane.
     *
     * Security: the only innerHTML assignment in the entire app. Guarded by:
     *   1. marked.parse() converts markdown to HTML (may include attacker content)
     *   2. DOMPurify.sanitize() with explicit ALLOWED_TAGS/ALLOWED_ATTR removes
     *      all script tags, event handlers, and javascript: URIs before insertion.
     * If DOMPurify is not loaded, we fall back to textContent (no HTML rendering).
     */
    preview(preview) {
      const el = document.getElementById("preview-content");
      if (!el) return;

      if (!preview) {
        while (el.firstChild) el.removeChild(el.firstChild);
        const p = document.createElement("p");
        p.className = "preview-placeholder";
        p.textContent = "Select a memory to read it.";
        el.appendChild(p);
        return;
      }

      let html = "";
      const rawBody = preview.body || "";

      if (typeof marked !== "undefined") {
        html = marked.parse(rawBody);
      } else {
        // Fallback: render as plain text (no HTML).
        while (el.firstChild) el.removeChild(el.firstChild);
        const pre = document.createElement("pre");
        pre.textContent = rawBody; // safe — textContent
        el.appendChild(pre);
        return;
      }

      if (typeof DOMPurify !== "undefined") {
        // Sanitize: only allow safe structural/semantic HTML.
        // Script tags, event handlers, and javascript: URLs are removed.
        html = DOMPurify.sanitize(html, {
          ALLOWED_TAGS: [
            "h1","h2","h3","h4","h5","h6",
            "p","br","hr","blockquote","pre","code",
            "ul","ol","li","dl","dt","dd",
            "a","em","strong","del","mark","sub","sup",
            "table","thead","tbody","tr","th","td",
            "img","figure","figcaption",
            "details","summary",
          ],
          ALLOWED_ATTR: ["href","src","alt","title","class","id","colspan","rowspan"],
          FORBID_TAGS: ["script","style","iframe","object","embed","form","input"],
          FORBID_ATTR: ["onerror","onload","onclick","onmouseover","onmouseout",
                        "onkeydown","onkeyup","onfocus","onblur","onchange"],
        });
        // After sanitization the HTML is safe — DOMPurify is the sanitization boundary.
        el.innerHTML = html; // safe: sanitized by DOMPurify above
      } else {
        // DOMPurify not available — degrade to textContent.
        while (el.firstChild) el.removeChild(el.firstChild);
        const pre = document.createElement("pre");
        pre.textContent = rawBody;
        el.appendChild(pre);
      }
    },
  };

  // ── Category item helper ───────────────────────────────────────────────────

  /**
   * Build a category list item via safe DOM construction.
   * cat is null for "All", string for a real category.
   */
  function makeCategoryItem(cat, label, count, activeCategory) {
    const li = document.createElement("li");
    li.className = "category-item";

    const btn = document.createElement("button");
    btn.className = "category-btn" + (cat === activeCategory ? " active" : "");
    btn.setAttribute("aria-pressed", cat === activeCategory ? "true" : "false");

    const labelSpan = document.createElement("span");
    labelSpan.textContent = label; // user category name — textContent is safe

    const countSpan = document.createElement("span");
    countSpan.className = "count";
    countSpan.textContent = String(count); // number — safe

    btn.appendChild(labelSpan);
    btn.appendChild(countSpan);

    btn.addEventListener("click", () => {
      router.navigate({ category: cat, memoryPath: null });
    });

    li.appendChild(btn);
    return li;
  }

  // ── Graph ──────────────────────────────────────────────────────────────────

  const graph = (() => {
    let cy = null;

    /** Deterministic category → color from a fixed palette. */
    function categoryColor(cat) {
      const PALETTE = [
        "#7aa2f7","#9ece6a","#e0af68","#bb9af7","#f7768e",
        "#73daca","#0db9d7","#ff9e64","#c3e88d","#89ddff",
      ];
      let h = 5381;
      for (let i = 0; i < cat.length; i++) h = ((h << 5) + h) + cat.charCodeAt(i) | 0;
      return PALETTE[Math.abs(h) % PALETTE.length];
    }

    function renderGraph(data) {
      const container = document.getElementById("cy");
      if (!container) return;

      if (!data || !data.nodes || data.nodes.length === 0) {
        while (container.firstChild) container.removeChild(container.firstChild);
        const msg = document.createElement("div");
        msg.style.cssText = "padding:20px;color:var(--text-muted);font-size:var(--text-sm)";
        msg.textContent = "No graph data.";
        container.appendChild(msg);
        return;
      }

      const elements = [];

      data.nodes.forEach(n => {
        elements.push({
          group: "nodes",
          data: {
            id: n.path,
            label: n.name || n.path,
            category: n.category || "",
            color: categoryColor(n.category || ""),
          },
        });
      });

      // Deduplicate edges (similarity is symmetric).
      const seen = new Set();
      data.edges.forEach(e => {
        const src = e.src && e.src.path ? e.src.path : e.src;
        const dst = e.dst && e.dst.path ? e.dst.path : e.dst;
        const key = [src, dst].sort().join("|");
        if (seen.has(key)) return;
        seen.add(key);
        elements.push({
          group: "edges",
          data: {
            id: `e-${src}-${dst}`,
            source: src,
            target: dst,
            kind: e.kind,
            score: e.score,
          },
        });
      });

      if (cy) { cy.destroy(); cy = null; }

      cy = cytoscape({
        container,
        elements,
        style: [
          {
            selector: "node",
            style: {
              "background-color": "data(color)",
              "label": "data(label)",
              "font-size": "10px",
              "color": "#c0caf5",
              "text-outline-width": 1,
              "text-outline-color": "#1a1b26",
              "width": 18,
              "height": 18,
              "text-max-width": "80px",
              "text-wrap": "ellipsis",
            },
          },
          {
            selector: "edge[kind='similarity']",
            style: { "width": 1, "line-color": "#3b4261", "opacity": 0.6 },
          },
          {
            selector: "edge[kind='explicit']",
            style: {
              "width": 2,
              "line-color": "#7aa2f7",
              "target-arrow-color": "#7aa2f7",
              "target-arrow-shape": "triangle",
              "curve-style": "bezier",
              "opacity": 0.8,
            },
          },
        ],
        layout: {
          name: "cose",
          animate: false,
          nodeRepulsion: 4500,
          idealEdgeLength: 80,
          padding: 16,
        },
      });

      // Click node → navigate to that memory.
      cy.on("tap", "node", evt => {
        const nodeId = evt.target.id();
        const node = data.nodes.find(n => n.path === nodeId);
        if (node) {
          router.navigate({ memoryPath: node.path, memoryCategory: node.category });
        }
      });
    }

    return { render: renderGraph };
  })();

  // ── SSE ───────────────────────────────────────────────────────────────────

  const sse = (() => {
    let es = null;
    let reconnectTimer = null;
    let reloadDebounce = null;

    function setStatus(status) {
      const dot = document.getElementById("sse-status");
      if (dot) {
        dot.className = status === "connected" ? "connected" :
                         status === "error" ? "error" : "";
      }
    }

    function showReloadIndicator() {
      const el = document.getElementById("reload-indicator");
      if (!el) return;
      el.classList.add("visible");
      setTimeout(() => el.classList.remove("visible"), 2000);
    }

    function scheduleReload() {
      if (reloadDebounce) clearTimeout(reloadDebounce);
      reloadDebounce = setTimeout(async () => {
        const s = state.get();
        if (s.activeBrain) {
          await loadMemories(s.activeBrain);
          if (s.activeMemoryPath) {
            loadPreview(s.activeBrain, s.activeMemoryPath, null);
          }
        }
        showReloadIndicator();
        // Marker for Playwright SSE reload test.
        document.body.dataset.sseReloaded = Date.now();
      }, RELOAD_DEBOUNCE_MS);
    }

    function connect() {
      if (es) { es.close(); es = null; }
      try {
        es = new EventSource("/api/events");
        es.addEventListener("open", () => setStatus("connected"));
        es.addEventListener("memory", () => scheduleReload());
        es.addEventListener("message", () => scheduleReload());
        es.addEventListener("error", () => {
          setStatus("error");
          es.close();
          es = null;
          if (reconnectTimer) clearTimeout(reconnectTimer);
          reconnectTimer = setTimeout(connect, SSE_RECONNECT_MS);
        });
      } catch (_) {
        setStatus("error");
      }
    }

    return { connect };
  })();

  // ── Router ─────────────────────────────────────────────────────────────────

  const router = (() => {
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
    function navigate(patch) {
      const s = state.get();
      const brain = patch.brain !== undefined ? patch.brain : s.activeBrain;
      const category = patch.category !== undefined ? patch.category : s.activeCategory;
      const memoryPath = patch.memoryPath !== undefined ? patch.memoryPath : s.activeMemoryPath;

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

    return { parse, navigate, onRoute };
  })();

  // ── Data loaders ───────────────────────────────────────────────────────────

  async function loadBrains() {
    const r = await api.brains();
    if (!r.ok) { toast.show(r.error); return; }
    const brains = Array.isArray(r.data) ? r.data : [];
    state.set({ brains });

    const s = state.get();
    if (!s.activeBrain && brains.length > 0) {
      const primary = brains.find(b => b.primary) || brains[0];
      router.navigate({ brain: primary.name });
    }
  }

  async function loadMemories(brain) {
    const r = await api.memories(brain);
    if (!r.ok) { toast.show(r.error); return; }
    const memories = Array.isArray(r.data) ? r.data : [];
    state.set({ memories, activeBrain: brain });
  }

  function findCategory(path) {
    const m = state.get().memories.find(m => m.path === path);
    return m ? m.category : null;
  }

  async function loadPreview(brain, path, category) {
    const cat = category || findCategory(path);
    if (!cat) return;
    // Strip category prefix and .md extension to get the API filename.
    const filename = path.replace(/^[^/]+\//, "").replace(/\.md$/, "");
    const r = await api.memory(brain, cat, filename);
    if (!r.ok) { toast.show(r.error); return; }
    state.set({ preview: r.data, activeMemoryPath: path });
  }

  async function loadGraph(brain) {
    const r = await api.graph(brain);
    if (!r.ok) { toast.show(r.error); return; }
    state.set({ graphData: r.data });
    graph.render(r.data);
  }

  // ── Search wiring ──────────────────────────────────────────────────────────

  let searchTerm = "";

  function wireSearch() {
    const input = document.getElementById("search-input");
    if (!input) return;
    input.addEventListener("input", () => {
      searchTerm = input.value;
      const s = state.get();
      render.memoryList(s.memories, s.activeCategory, s.activeMemoryPath, searchTerm);
    });
  }

  // ── State subscription ─────────────────────────────────────────────────────

  state.subscribe(s => {
    render.brains(s.brains, s.activeBrain);
    render.categories(s.memories, s.activeCategory);
    render.memoryList(s.memories, s.activeCategory, s.activeMemoryPath, searchTerm);
    render.preview(s.preview);
  });

  // ── Boot ───────────────────────────────────────────────────────────────────

  function boot() {
    theme.init();
    theme.updateToggleLabel();

    const themeBtn = document.getElementById("theme-toggle");
    if (themeBtn) themeBtn.addEventListener("click", () => theme.toggle());

    wireSearch();
    window.addEventListener("hashchange", () => router.onRoute());
    sse.connect();

    loadBrains().then(() => router.onRoute());
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();

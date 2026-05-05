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
      // ── Plan 2 Phase 4: editor state ─────────────────────────────────────
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

  // Test/debug surface: expose current state read-only for Playwright assertions.
  // Updated below via state.subscribe().
  Object.defineProperty(window, "__grugState", {
    get: () => state.get(),
    configurable: true,
  });

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

    /**
     * PUT JSON to a path with required If-Match ETag header.
     * Returns {ok, status, data, error}. Never throws. Always sends the
     * X-Grug-Client header required by the server CSRF middleware.
     */
    async put(path, payload, etag) {
      try {
        const resp = await fetch(path, {
          method: "PUT",
          headers: {
            "Content-Type": "application/json",
            "X-Grug-Client": "web",
            "If-Match": String(etag),
          },
          body: JSON.stringify(payload),
        });
        let data = null;
        try { data = await resp.json(); } catch (_) {}
        if (!resp.ok) {
          const err = (data && data.error) || `HTTP ${resp.status}`;
          return { ok: false, status: resp.status, data, error: err };
        }
        return { ok: true, status: resp.status, data };
      } catch (e) {
        return { ok: false, status: 0, error: e.message || "network error" };
      }
    },

    /** PUT helper specialized for the memory write route. */
    async writeMemory(brain, category, path, body, frontmatter, etag) {
      const url = `/api/memory/${encodeURIComponent(brain)}/${encodeURIComponent(category)}/${encodeURIComponent(path)}`;
      return this.put(url, { body, frontmatter }, etag);
    },

    /**
     * POST JSON. Always sends X-Grug-Client: web (CSRF middleware requirement).
     * Returns {ok, status, data, error}. Never throws.
     */
    async post(path, payload) {
      try {
        const resp = await fetch(path, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Grug-Client": "web",
          },
          body: JSON.stringify(payload || {}),
        });
        let data = null;
        try { data = await resp.json(); } catch (_) {}
        if (!resp.ok) {
          const err = (data && data.error) || `HTTP ${resp.status}`;
          return { ok: false, status: resp.status, data, error: err };
        }
        return { ok: true, status: resp.status, data };
      } catch (e) {
        return { ok: false, status: 0, error: e.message || "network error" };
      }
    },

    /**
     * DELETE. 204 returns {ok:true} with no data. CSRF header required.
     */
    async delete(path) {
      try {
        const resp = await fetch(path, {
          method: "DELETE",
          headers: { "X-Grug-Client": "web" },
        });
        if (!resp.ok) {
          let data = null;
          try { data = await resp.json(); } catch (_) {}
          const err = (data && data.error) || `HTTP ${resp.status}`;
          return { ok: false, status: resp.status, data, error: err };
        }
        return { ok: true, status: resp.status, data: null };
      } catch (e) {
        return { ok: false, status: 0, error: e.message || "network error" };
      }
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

    /** Briefly show a success toast (auto-dismisses). */
    function success(msg) {
      const container = document.getElementById("toast-container");
      if (!container) return;
      const el = document.createElement("div");
      el.className = "toast toast-success";
      el.setAttribute("role", "status");
      el.setAttribute("aria-live", "polite");
      const body = document.createElement("div");
      body.className = "toast-body";
      const message = document.createElement("div");
      message.className = "toast-message";
      message.textContent = msg;
      body.appendChild(message);
      el.appendChild(body);
      container.appendChild(el);
      setTimeout(() => { if (el.parentNode) el.remove(); }, 3000);
    }

    return { show, success, error: show };
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
      // Notify graph to update its colors for the new theme.
      if (typeof graph !== "undefined" && graph.updateTheme) {
        graph.updateTheme();
      }
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

    // Phase 5 DW-5.4: per-category "+" button opens an unsaved-draft editor.
    // Only attached for real categories (not the "All" pseudo-category).
    if (cat !== null) {
      const addBtn = document.createElement("button");
      addBtn.className = "category-add";
      addBtn.setAttribute("aria-label", `New memory in ${cat}`);
      addBtn.textContent = "+"; // static
      addBtn.addEventListener("click", e => {
        e.stopPropagation();
        if (typeof crud !== "undefined" && crud.openDraft) crud.openDraft(cat);
      });
      li.appendChild(addBtn);
    }
    return li;
  }

  // ── Graph ──────────────────────────────────────────────────────────────────

  /**
   * graph.* namespace — sigma.js-based similarity graph.
   *
   * Public API (preserved from Plan 1 / cytoscape implementation):
   *   graph.render(data)  — render {nodes, edges} data into #cy container
   *
   * Internal:
   *   graph.updateTheme() — refresh sigma colors from current CSS vars (called
   *                         by theme.toggle() after each theme change)
   *
   * Library globals used: window.Sigma (sigma@2.4.0), window.graphology (graphology@0.25.4)
   */
  const graph = (() => {
    let sigmaInstance = null;
    let graphData = null; // last-rendered {nodes, edges}

    /** Deterministic category → color from a fixed palette (unchanged from Plan 1). */
    function categoryColor(cat) {
      const PALETTE = [
        "#7aa2f7","#9ece6a","#e0af68","#bb9af7","#f7768e",
        "#73daca","#0db9d7","#ff9e64","#c3e88d","#89ddff",
      ];
      let h = 5381;
      for (let i = 0; i < cat.length; i++) h = ((h << 5) + h) + cat.charCodeAt(i) | 0;
      return PALETTE[Math.abs(h) % PALETTE.length];
    }

    /**
     * Minimal Fruchterman-Reingold force layout.
     * Assigns x/y attributes directly on a graphology Graph instance.
     * Runs synchronously (animate: false equivalent). Suitable for ≤200 nodes.
     *
     * @param {object} g  — graphology Graph instance with nodes already added
     * @param {number} iterations — number of force iterations (default 100)
     */
    function applyForceLayout(g, iterations) {
      iterations = iterations || 100;
      const nodes = g.nodes();
      const n = nodes.length;
      if (n === 0) return;

      // Initialize random positions in [-1, 1].
      nodes.forEach(id => {
        g.setNodeAttribute(id, "x", Math.random() * 2 - 1);
        g.setNodeAttribute(id, "y", Math.random() * 2 - 1);
      });

      if (n === 1) return; // single node: center it

      // Fruchterman-Reingold constants.
      const area = 4; // bounding box area
      const k = Math.sqrt(area / n); // ideal spring length
      const kSq = k * k;

      for (let iter = 0; iter < iterations; iter++) {
        const temp = 1.0 * (1 - iter / iterations); // cooling factor

        // Build displacement accumulators.
        const dx = new Float64Array(n);
        const dy = new Float64Array(n);

        // Repulsion: all pairs.
        for (let i = 0; i < n; i++) {
          for (let j = i + 1; j < n; j++) {
            const xi = g.getNodeAttribute(nodes[i], "x");
            const yi = g.getNodeAttribute(nodes[i], "y");
            const xj = g.getNodeAttribute(nodes[j], "x");
            const yj = g.getNodeAttribute(nodes[j], "y");
            const ddx = xi - xj;
            const ddy = yi - yj;
            const dist = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
            const force = kSq / dist;
            const fx = (ddx / dist) * force;
            const fy = (ddy / dist) * force;
            dx[i] += fx; dy[i] += fy;
            dx[j] -= fx; dy[j] -= fy;
          }
        }

        // Attraction: edges.
        g.forEachEdge((edge, attrs, source, target) => {
          const si = nodes.indexOf(source);
          const ti = nodes.indexOf(target);
          if (si < 0 || ti < 0) return;
          const xi = g.getNodeAttribute(source, "x");
          const yi = g.getNodeAttribute(source, "y");
          const xj = g.getNodeAttribute(target, "x");
          const yj = g.getNodeAttribute(target, "y");
          const ddx = xi - xj;
          const ddy = yi - yj;
          const dist = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
          const force = (dist * dist) / k;
          const fx = (ddx / dist) * force;
          const fy = (ddy / dist) * force;
          dx[si] -= fx; dy[si] -= fy;
          dx[ti] += fx; dy[ti] += fy;
        });

        // Apply displacement with temperature clamping.
        for (let i = 0; i < n; i++) {
          const dispLen = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 0.01;
          const clamped = Math.min(dispLen, temp);
          const newX = g.getNodeAttribute(nodes[i], "x") + (dx[i] / dispLen) * clamped;
          const newY = g.getNodeAttribute(nodes[i], "y") + (dy[i] / dispLen) * clamped;
          g.setNodeAttribute(nodes[i], "x", newX);
          g.setNodeAttribute(nodes[i], "y", newY);
        }
      }

      // Normalize to [0, 1] bounding box.
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      nodes.forEach(id => {
        const x = g.getNodeAttribute(id, "x");
        const y = g.getNodeAttribute(id, "y");
        if (x < minX) minX = x; if (x > maxX) maxX = x;
        if (y < minY) minY = y; if (y > maxY) maxY = y;
      });
      const rangeX = maxX - minX || 1;
      const rangeY = maxY - minY || 1;
      nodes.forEach(id => {
        g.setNodeAttribute(id, "x", (g.getNodeAttribute(id, "x") - minX) / rangeX);
        g.setNodeAttribute(id, "y", (g.getNodeAttribute(id, "y") - minY) / rangeY);
      });
    }

    /**
     * Read theme-driven colors from CSS vars.
     * Returns {labelColor, edgeSimilarityColor, edgeExplicitColor, bgColor}.
     */
    function themeColors() {
      const s = getComputedStyle(document.documentElement);
      return {
        labelColor:          s.getPropertyValue("--text-0").trim()  || "#c0caf5",
        edgeSimilarityColor: s.getPropertyValue("--border").trim()  || "#3b4261",
        edgeExplicitColor:   s.getPropertyValue("--accent").trim()  || "#7aa2f7",
        bgColor:             s.getPropertyValue("--bg-0").trim()    || "#1a1b26",
      };
    }

    function renderGraph(data) {
      graphData = data;
      const container = document.getElementById("cy");
      if (!container) return;

      // Destroy previous sigma instance.
      if (sigmaInstance) {
        sigmaInstance.kill();
        sigmaInstance = null;
      }

      if (!data || !data.nodes || data.nodes.length === 0) {
        while (container.firstChild) container.removeChild(container.firstChild);
        const msg = document.createElement("div");
        msg.style.cssText = "padding:20px;color:var(--text-muted);font-size:var(--text-sm)";
        msg.textContent = "No graph data.";
        container.appendChild(msg);
        return;
      }

      // Bail if sigma or graphology not loaded.
      if (typeof Sigma === "undefined" || typeof graphology === "undefined") return;

      // Build graphology graph.
      const g = new graphology.Graph({ type: "undirected", multi: false });

      data.nodes.forEach(n => {
        if (!g.hasNode(n.path)) {
          g.addNode(n.path, {
            label: n.name || n.path,
            color: categoryColor(n.category || ""),
            size: 5,
            // x/y set by layout below
          });
        }
      });

      // Deduplicate edges (similarity is symmetric).
      const seen = new Set();
      data.edges.forEach(e => {
        const src = e.src && e.src.path ? e.src.path : e.src;
        const dst = e.dst && e.dst.path ? e.dst.path : e.dst;
        const key = [src, dst].sort().join("|");
        if (seen.has(key)) return;
        seen.add(key);
        if (!g.hasNode(src) || !g.hasNode(dst)) return;
        try {
          g.addEdge(src, dst, {
            kind: e.kind,
            score: e.score,
            color: e.kind === "explicit" ? themeColors().edgeExplicitColor : themeColors().edgeSimilarityColor,
            size: e.kind === "explicit" ? 2 : 1,
          });
        } catch (_) { /* duplicate edge guard */ }
      });

      // Apply force-directed layout synchronously.
      applyForceLayout(g, 100);

      const colors = themeColors();

      // Instantiate sigma renderer.
      sigmaInstance = new Sigma(g, container, {
        renderLabels: true,
        labelColor: { color: colors.labelColor },
        labelSize: 10,
        labelWeight: "normal",
        defaultNodeColor: "#7aa2f7",
        defaultEdgeColor: colors.edgeSimilarityColor,
        allowInvalidContainer: true,
      });

      // Expose sigma instance for Playwright tests.
      window.__grugSigma = sigmaInstance;

      // Click node → navigate to that memory (preserves Plan 1 behavior).
      sigmaInstance.on("clickNode", evt => {
        const nodeId = evt.node;
        const node = data.nodes.find(n => n.path === nodeId);
        if (node) {
          router.navigate({ memoryPath: node.path, memoryCategory: node.category });
        }
      });
    }

    /**
     * Update sigma colors from current CSS vars after theme change.
     * Called by theme.toggle() to keep graph in sync with light/dark/system mode.
     */
    function updateTheme() {
      if (!sigmaInstance) return;
      const colors = themeColors();
      sigmaInstance.setSetting("labelColor", { color: colors.labelColor });
      sigmaInstance.refresh();
    }

    return { render: renderGraph, updateTheme };
  })();

  // ── Frontmatter form ──────────────────────────────────────────────────────

  /**
   * frontmatter.* — render + read + assemble + validate the structured
   * frontmatter form. Tags are array-shaped on the client; serialized to a
   * comma-joined string on save (matches existing parsing.rs semantics).
   */
  const frontmatter = (() => {
    const NAME_INPUT = "fm-name";
    const DESC_INPUT = "fm-description";
    const TAGS_INPUT = "fm-tags";
    const NAME_ERR = "fm-name-error";

    function parseTags(s) {
      if (!s) return [];
      return s.split(",").map(t => t.trim()).filter(Boolean);
    }

    function render(fm) {
      const n = document.getElementById(NAME_INPUT);
      const d = document.getElementById(DESC_INPUT);
      const t = document.getElementById(TAGS_INPUT);
      if (!n || !d || !t) return;
      n.value = fm.name || "";
      d.value = fm.description || "";
      const tagStr = Array.isArray(fm.tags) ? fm.tags.join(", ") : (fm.tags || "");
      t.value = tagStr;
      t.dataset.tagsCount = String(parseTags(tagStr).length);
      const err = document.getElementById(NAME_ERR);
      if (err) { err.hidden = true; err.textContent = ""; }
    }

    function read() {
      const n = document.getElementById(NAME_INPUT);
      const d = document.getElementById(DESC_INPUT);
      const t = document.getElementById(TAGS_INPUT);
      return {
        name: n ? n.value.trim() : "",
        description: d ? d.value.trim() : "",
        tags: parseTags(t ? t.value : ""),
      };
    }

    function validate(fm) {
      const errors = {};
      if (!fm.name || !fm.name.trim()) errors.name = "Name is required.";
      return { ok: Object.keys(errors).length === 0, errors };
    }

    function showErrors(errors) {
      const err = document.getElementById(NAME_ERR);
      if (!err) return;
      if (errors.name) {
        err.textContent = errors.name;
        err.hidden = false;
      } else {
        err.hidden = true;
        err.textContent = "";
      }
    }

    function assemble(fm) {
      const lines = ["---"];
      if (fm.name) lines.push("name: " + fm.name);
      if (fm.description) lines.push("description: " + fm.description);
      if (fm.tags && fm.tags.length) lines.push("tags: " + fm.tags.join(", "));
      lines.push("---");
      return lines.join("\n") + "\n";
    }

    function wire() {
      const inputs = [NAME_INPUT, DESC_INPUT, TAGS_INPUT];
      inputs.forEach(id => {
        const el = document.getElementById(id);
        if (!el) return;
        el.addEventListener("input", () => {
          const s = state.get();
          if (!s.buffer) return;
          const fm = read();
          if (id === TAGS_INPUT) el.dataset.tagsCount = String(fm.tags.length);
          const next = Object.assign({}, s.buffer, { frontmatter: fm });
          state.set({ buffer: next, dirty: computeDirty(next) });
        });
      });
    }

    return { parseTags, render, read, validate, showErrors, assemble, wire };
  })();

  function computeDirty(buf) {
    if (!buf) return false;
    if (buf.body !== buf.originalBody) return true;
    return JSON.stringify(buf.frontmatter) !== JSON.stringify(buf.originalFrontmatter);
  }

  // ── Editor (CodeMirror 6) ─────────────────────────────────────────────────

  const editor = (() => {
    let currentView = null;

    function buildDecorationsPlugin(CMns) {
      const wikilinkRe = /\[\[[^\]\n]+\]\]/g;
      const tagRe = /(^|\s)(#[A-Za-z][\w-]*)/g;

      function buildCombined(view) {
        const wikilinkDeco = CMns.Decoration.mark({ class: "cm-wikilink" });
        const tagDeco = CMns.Decoration.mark({ class: "cm-tag" });
        const marks = [];
        for (const r of view.visibleRanges) {
          const text = view.state.doc.sliceString(r.from, r.to);
          let m;
          wikilinkRe.lastIndex = 0;
          while ((m = wikilinkRe.exec(text)) !== null) {
            marks.push({ from: r.from + m.index, to: r.from + m.index + m[0].length, deco: wikilinkDeco });
          }
          tagRe.lastIndex = 0;
          while ((m = tagRe.exec(text)) !== null) {
            const s2 = r.from + m.index + m[1].length;
            const e2 = s2 + m[2].length;
            marks.push({ from: s2, to: e2, deco: tagDeco });
          }
        }
        marks.sort((a, b) => a.from - b.from || a.to - b.to);
        const b = new CMns.RangeSetBuilder();
        let lastTo = -1;
        for (const m of marks) {
          if (m.from < lastTo) continue;
          b.add(m.from, m.to, m.deco);
          lastTo = m.to;
        }
        return b.finish();
      }

      return CMns.ViewPlugin.fromClass(
        class {
          constructor(view) { this.decorations = buildCombined(view); }
          update(u) {
            if (u.docChanged || u.viewportChanged) {
              this.decorations = buildCombined(u.view);
            }
          }
        },
        { decorations: v => v.decorations }
      );
    }

    function saveKeymap(CMns) {
      return CMns.keymap.of([
        { key: "Mod-s", preventDefault: true, run: () => { save.run(); return true; } },
      ]);
    }

    function mount(container, doc, onChange) {
      if (typeof CM === "undefined") {
        const ta = document.createElement("textarea");
        ta.value = doc;
        ta.style.cssText = "width:100%;height:100%;font-family:var(--font-mono);";
        ta.addEventListener("input", () => onChange(ta.value));
        container.appendChild(ta);
        currentView = { _fallback: true, dom: ta, state: { doc: { toString: () => ta.value, length: ta.value.length } } };
        return currentView;
      }

      const updateListener = CM.EditorView.updateListener.of(u => {
        if (u.docChanged) onChange(u.state.doc.toString());
      });

      const startState = CM.EditorState.create({
        doc,
        extensions: [
          CM.basicSetup,
          CM.markdown(),
          buildDecorationsPlugin(CM),
          saveKeymap(CM),
          updateListener,
          CM.EditorView.theme({
            "&": { height: "100%" },
            ".cm-scroller": { fontFamily: "var(--font-mono)" },
          }),
        ],
      });

      const view = new CM.EditorView({ state: startState, parent: container });
      currentView = view;
      window.__grugEditorView = view;
      return view;
    }

    function unmount() {
      if (currentView) {
        if (currentView._fallback && currentView.dom && currentView.dom.parentNode) {
          currentView.dom.parentNode.removeChild(currentView.dom);
        } else if (currentView.destroy) {
          currentView.destroy();
        }
      }
      currentView = null;
      window.__grugEditorView = null;
    }

    function setDoc(text) {
      if (!currentView) return;
      if (currentView._fallback) { currentView.dom.value = text; return; }
      currentView.dispatch({
        changes: { from: 0, to: currentView.state.doc.length, insert: text },
      });
    }

    function getView() { return currentView; }

    return { mount, unmount, setDoc, getView };
  })();

  // ── Save flow ─────────────────────────────────────────────────────────────

  const save = (() => {
    let inFlight = false;

    async function run() {
      if (inFlight) return;
      const s = state.get();
      const buf = s.buffer;
      if (!buf) return;

      // Phase 5: a draft buffer (no activeMemoryPath) routes through the
      // Create modal to collect a filename, then POSTs to /api/memory.
      if (buf.draft) {
        // Pull current frontmatter from the form — name (if filled) becomes the
        // proposed filename; the modal still confirms.
        const fmDraft = frontmatter.read();
        // Sync draft body from editor view.
        const view = editor.getView && editor.getView();
        if (view && view.state && view.state.doc) {
          buf.body = view.state.doc.toString();
        }
        const next = Object.assign({}, buf, { frontmatter: fmDraft });
        state.set({ buffer: next });
        const proposed = (fmDraft.name || "").trim().replace(/[^A-Za-z0-9_-]/g, "-");
        const chosen = await crud.showCreate(proposed);
        if (!chosen) return;
        await crud.submitCreate(chosen);
        return;
      }

      const fm = frontmatter.read();
      const v = frontmatter.validate(fm);
      if (!v.ok) {
        frontmatter.showErrors(v.errors);
        toast.show(v.errors.name || "Validation failed");
        const nameInput = document.getElementById("fm-name");
        if (nameInput) nameInput.focus();
        return;
      }
      const fmText = frontmatter.assemble(fm);
      const memPath = s.activeMemoryPath;
      if (!memPath) return;
      const mem = s.memories.find(m => m.path === memPath);
      if (!mem) { toast.show("memory metadata missing"); return; }
      const filename = memPath.replace(/^[^/]+\//, "").replace(/\.md$/, "");

      inFlight = true;
      window.__grugLastSaveStatus = null;
      const resp = await api.writeMemory(s.activeBrain, mem.category, filename, buf.body, fmText, buf.etag);
      inFlight = false;
      window.__grugLastSaveStatus = resp.status;

      if (resp.ok) {
        const newEtag = (resp.data && typeof resp.data.etag === "number") ? resp.data.etag : buf.etag;
        const next = Object.assign({}, buf, {
          frontmatter: fm,
          etag: newEtag,
          originalBody: buf.body,
          originalFrontmatter: JSON.parse(JSON.stringify(fm)),
        });
        state.set({ buffer: next, dirty: false });
        toast.success("Saved");
        return;
      }
      if (resp.status === 409) {
        // Phase 5: open the structured 3-pane conflict modal.
        if (resp.data && resp.data.error === "conflict" &&
            (resp.data.current_body !== undefined || resp.data.current_etag !== undefined)) {
          conflict.show(resp.data);
        } else {
          toast.show("Conflict — reload to merge changes");
        }
      } else if (resp.status === 403) {
        toast.show("Brain is read-only");
      } else {
        toast.show(resp.error || "Save failed");
      }
    }

    return { run };
  })();

  // ── Navigation guard ──────────────────────────────────────────────────────

  const nav = (() => {
    let pendingResolve = null;

    function init() {
      window.addEventListener("beforeunload", e => {
        if (state.get().dirty) {
          e.preventDefault();
          e.returnValue = "";
        }
      });
      const cancel = document.getElementById("unsaved-cancel");
      const discard = document.getElementById("unsaved-discard");
      if (cancel) cancel.addEventListener("click", () => closeModal(false));
      if (discard) discard.addEventListener("click", () => closeModal(true));
      document.addEventListener("keydown", e => {
        const modal = document.getElementById("unsaved-modal");
        if (e.key === "Escape" && modal && !modal.hidden) closeModal(false);
      });
    }

    function closeModal(result) {
      const modal = document.getElementById("unsaved-modal");
      if (modal) modal.hidden = true;
      if (pendingResolve) { pendingResolve(result); pendingResolve = null; }
    }

    function guard() {
      if (!state.get().dirty) return Promise.resolve(true);
      const modal = document.getElementById("unsaved-modal");
      if (!modal) return Promise.resolve(true);
      modal.hidden = false;
      const cancel = document.getElementById("unsaved-cancel");
      if (cancel) cancel.focus();
      return new Promise(res => { pendingResolve = res; });
    }

    return { init, guard };
  })();

  // ── Modal infrastructure (Phase 5) ─────────────────────────────────────────

  /**
   * modal.* — generic focus-trapped modal helper.
   *
   * Usage:
   *   const handle = modal.open(el, { focusTarget, onEscape });
   *   handle.close();    // hide and restore focus
   *
   * Behaviour:
   *   - Sets el.hidden = false.
   *   - Moves focus to focusTarget (or first focusable inside el).
   *   - Tab cycles focus inside el (Shift+Tab wraps backwards).
   *   - Escape calls onEscape (default: close).
   *   - On close, restores focus to whatever was active before open.
   */
  const modal = (() => {
    const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

    function focusables(root) {
      return Array.from(root.querySelectorAll(FOCUSABLE))
        .filter(el => !el.disabled && !el.hidden && el.offsetParent !== null);
    }

    function open(el, options) {
      if (!el) return { close: () => {} };
      options = options || {};
      const prevFocus = document.activeElement;
      el.hidden = false;

      const initial = options.focusTarget || focusables(el)[0] || el;
      setTimeout(() => { try { initial.focus(); } catch (_) {} }, 0);

      function onKeydown(e) {
        if (e.key === "Escape") {
          e.preventDefault();
          if (options.onEscape) options.onEscape();
          else close();
          return;
        }
        if (e.key === "Tab") {
          const list = focusables(el);
          if (list.length === 0) return;
          const first = list[0];
          const last = list[list.length - 1];
          if (e.shiftKey && document.activeElement === first) {
            e.preventDefault(); last.focus();
          } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault(); first.focus();
          }
        }
      }

      document.addEventListener("keydown", onKeydown);

      function close() {
        el.hidden = true;
        document.removeEventListener("keydown", onKeydown);
        if (prevFocus && typeof prevFocus.focus === "function") {
          try { prevFocus.focus(); } catch (_) {}
        }
      }

      return { close };
    }

    return { open };
  })();

  // ── Conflict modal (Phase 5) ──────────────────────────────────────────────

  /**
   * conflict.* — render the 3-pane conflict modal when PUT returns 409.
   *
   * Inputs: ConflictResponse from server: {error, current_etag, current_body, attempted_body}.
   * Panes: "yours" (attempted_body), "theirs" (current_body), "merged-preview" (line diff).
   *
   * Actions:
   *   - "Reload theirs": replace editor buffer with current_body + current_etag, close.
   *   - "Overwrite":     PUT yours' body again with theirs' etag; close on success.
   *   - "Cancel":        close the modal, leave buffer dirty.
   *
   * Diff rendering uses textContent + CSS classes (no innerHTML), per code-standards.
   */
  const conflict = (() => {
    let currentResponse = null;
    let handle = null;

    /** Render diff into a <pre> using textContent + spans with diff classes. */
    function renderDiff(targetEl, yours, theirs) {
      while (targetEl.firstChild) targetEl.removeChild(targetEl.firstChild);
      if (typeof Diff === "undefined" || !Diff.diffLines) {
        // Fallback: plain concatenation.
        const fallback = document.createElement("span");
        fallback.className = "diff-context";
        fallback.textContent = "[diff library unavailable]\n" + yours + "\n---\n" + theirs;
        targetEl.appendChild(fallback);
        return;
      }
      const chunks = Diff.diffLines(yours || "", theirs || "");
      chunks.forEach(c => {
        const span = document.createElement("span");
        if (c.added) span.className = "diff-add";
        else if (c.removed) span.className = "diff-remove";
        else span.className = "diff-context";
        span.textContent = c.value;
        targetEl.appendChild(span);
      });
    }

    function show(resp) {
      const el = document.getElementById("conflict-modal");
      if (!el) return;
      currentResponse = resp;
      const yoursEl = document.getElementById("conflict-yours");
      const theirsEl = document.getElementById("conflict-theirs");
      const mergedEl = document.getElementById("conflict-merged");
      if (yoursEl) yoursEl.textContent = resp.attempted_body || "";
      if (theirsEl) theirsEl.textContent = resp.current_body || "";
      if (mergedEl) renderDiff(mergedEl, resp.attempted_body || "", resp.current_body || "");
      handle = modal.open(el, {
        focusTarget: document.getElementById("conflict-cancel"),
      });
      // Marker for Playwright.
      window.__grugConflictOpen = true;
    }

    function close() {
      if (handle) handle.close();
      handle = null;
      currentResponse = null;
      window.__grugConflictOpen = false;
    }

    /** Replace editor buffer with theirs + new etag. */
    function reloadTheirs() {
      if (!currentResponse) { close(); return; }
      const s = state.get();
      if (!s.buffer) { close(); return; }
      const next = Object.assign({}, s.buffer, {
        body: currentResponse.current_body || "",
        etag: currentResponse.current_etag || 0,
        originalBody: currentResponse.current_body || "",
      });
      state.set({ buffer: next, dirty: false });
      // Push the new doc into CodeMirror.
      if (editor.setDoc) editor.setDoc(currentResponse.current_body || "");
      toast.success("Reloaded from disk");
      close();
    }

    /** Overwrite: re-PUT yours' body with theirs' etag. */
    async function overwrite() {
      if (!currentResponse) { close(); return; }
      const s = state.get();
      const buf = s.buffer;
      if (!buf) { close(); return; }
      const memPath = s.activeMemoryPath;
      if (!memPath) { close(); return; }
      const mem = s.memories.find(m => m.path === memPath);
      if (!mem) { close(); return; }
      const filename = memPath.replace(/^[^/]+\//, "").replace(/\.md$/, "");
      const fm = frontmatter.read();
      const fmText = frontmatter.assemble(fm);
      const resp = await api.writeMemory(s.activeBrain, mem.category, filename,
        buf.body, fmText, currentResponse.current_etag);
      if (resp.ok) {
        const newEtag = (resp.data && typeof resp.data.etag === "number") ? resp.data.etag : currentResponse.current_etag;
        const next = Object.assign({}, buf, {
          etag: newEtag,
          originalBody: buf.body,
          originalFrontmatter: JSON.parse(JSON.stringify(fm)),
          frontmatter: fm,
        });
        state.set({ buffer: next, dirty: false });
        toast.success("Saved (overwrite)");
        close();
      } else {
        toast.show(resp.error || "Overwrite failed");
      }
    }

    function wire() {
      const cancel = document.getElementById("conflict-cancel");
      const reload = document.getElementById("conflict-reload");
      const over = document.getElementById("conflict-overwrite");
      if (cancel) cancel.addEventListener("click", close);
      if (reload) reload.addEventListener("click", reloadTheirs);
      if (over) over.addEventListener("click", overwrite);
    }

    return { show, close, wire, _renderDiff: renderDiff };
  })();

  // ── CRUD: create / delete / rename modals (Phase 5) ───────────────────────

  /**
   * crud.* — UI for create / delete / rename.
   *
   * Create flow (DW-5.4):
   *   crud.openDraft(category) — sets state.draft, no activeMemoryPath, mounts editor empty.
   *   First save (save.run while draft active) opens the create modal to collect a name.
   *   On submit, POST /api/memory with {path: "<category>/<name>", body, frontmatter}.
   *
   * Delete flow (DW-5.6):
   *   crud.openDelete() — confirm modal; Delete enabled only when typed-name matches.
   *
   * Rename flow (DW-5.7):
   *   crud.openRename() — input new path; POST .../rename?rewrite_links=true.
   */
  const crud = (() => {
    let createHandle = null;
    let deleteHandle = null;
    let renameHandle = null;
    /** Pending callback: called with the chosen filename on Create submit. */
    let pendingCreateResolve = null;

    // ── Draft state ──────────────────────────────────────────────────
    function openDraft(category) {
      const s = state.get();
      const cat = category || s.activeCategory || "notes";
      // Seed an empty draft buffer. Frontmatter defaults are intentionally
      // empty; the user can fill them in via the form before naming the file.
      const draft = {
        category: cat,
        body: "",
        frontmatter: { name: "", description: "", tags: [] },
        originalBody: "",
        originalFrontmatter: { name: "", description: "", tags: [] },
        etag: 0,
        draft: true,
      };
      state.set({
        buffer: draft,
        activeMemoryPath: null,
        preview: null,
        dirty: false,
        mode: "edit",
      });
    }

    // ── Create modal ──────────────────────────────────────────────────
    function showCreate(prefill) {
      const el = document.getElementById("create-modal");
      if (!el) return Promise.resolve(null);
      const input = document.getElementById("create-name");
      if (input) input.value = prefill || "";
      return new Promise(resolve => {
        pendingCreateResolve = resolve;
        createHandle = modal.open(el, {
          focusTarget: input,
          onEscape: () => closeCreate(null),
        });
      });
    }

    function closeCreate(name) {
      if (createHandle) createHandle.close();
      createHandle = null;
      if (pendingCreateResolve) { pendingCreateResolve(name); pendingCreateResolve = null; }
    }

    /** Returns true if the file was created. */
    async function submitCreate(name) {
      const s = state.get();
      const draft = s.buffer && s.buffer.draft ? s.buffer : null;
      if (!draft) return false;
      if (!name || !name.trim()) {
        toast.show("Name is required");
        return false;
      }
      // Update the draft frontmatter name field to match.
      const fm = frontmatter.read();
      if (!fm.name) fm.name = name.trim();
      const fmText = frontmatter.assemble(fm);

      const path = `${draft.category}/${name.trim()}`;
      const payload = {
        path,
        body: draft.body,
        frontmatter: fmText,
        brain: s.activeBrain || undefined,
      };
      const resp = await api.post("/api/memory", payload);
      if (!resp.ok) {
        toast.show(resp.error || "Create failed");
        return false;
      }
      toast.success("Created");
      // Refresh memories then navigate to the new path.
      const newPath = (resp.data && resp.data.path) || `${path}.md`;
      await loadMemories(s.activeBrain);
      router.navigate({ memoryPath: newPath, category: draft.category });
      return true;
    }

    // ── Delete modal ──────────────────────────────────────────────────
    function openDelete() {
      const s = state.get();
      const memPath = s.activeMemoryPath;
      if (!memPath) return;
      const mem = s.memories.find(m => m.path === memPath);
      if (!mem) return;
      const targetName = mem.name || mem.path;
      const el = document.getElementById("delete-modal");
      const targetEl = document.getElementById("delete-target-name");
      const input = document.getElementById("delete-confirm");
      const submit = document.getElementById("delete-submit");
      if (targetEl) targetEl.textContent = targetName;
      if (input) input.value = "";
      if (submit) submit.disabled = true;

      function onInput() {
        if (!submit || !input) return;
        submit.disabled = input.value.trim() !== targetName;
      }
      if (input) input.addEventListener("input", onInput);

      async function onSubmit() {
        if (submit && submit.disabled) return;
        await runDelete();
      }
      if (submit) submit.addEventListener("click", onSubmit);

      deleteHandle = modal.open(el, {
        focusTarget: input,
        onEscape: closeDelete,
      });
      // Stash unbinders.
      el.__cleanup = () => {
        if (input) input.removeEventListener("input", onInput);
        if (submit) submit.removeEventListener("click", onSubmit);
      };
    }

    function closeDelete() {
      const el = document.getElementById("delete-modal");
      if (el && el.__cleanup) { el.__cleanup(); el.__cleanup = null; }
      if (deleteHandle) deleteHandle.close();
      deleteHandle = null;
    }

    async function runDelete() {
      const s = state.get();
      const memPath = s.activeMemoryPath;
      const mem = memPath ? s.memories.find(m => m.path === memPath) : null;
      if (!memPath || !mem) { closeDelete(); return; }
      const filename = memPath.replace(/^[^/]+\//, "").replace(/\.md$/, "");
      const url = `/api/memory/${encodeURIComponent(s.activeBrain)}/${encodeURIComponent(mem.category)}/${encodeURIComponent(filename)}`;
      const resp = await api.delete(url);
      closeDelete();
      if (!resp.ok) {
        toast.show(resp.error || "Delete failed");
        return;
      }
      toast.success("Deleted");
      // Drop preview + buffer + nav back to category.
      state.set({ buffer: null, preview: null, activeMemoryPath: null });
      await loadMemories(s.activeBrain);
      router.navigate({ memoryPath: null });
    }

    // ── Rename modal ──────────────────────────────────────────────────
    function openRename() {
      const s = state.get();
      const memPath = s.activeMemoryPath;
      if (!memPath) return;
      const mem = s.memories.find(m => m.path === memPath);
      if (!mem) return;
      const el = document.getElementById("rename-modal");
      const input = document.getElementById("rename-new-path");
      const submit = document.getElementById("rename-submit");
      const rewriteCb = document.getElementById("rename-rewrite-links");
      // Pre-fill with the current path (without .md extension for usability).
      const stripped = memPath.replace(/\.md$/, "");
      if (input) input.value = stripped;
      if (rewriteCb) rewriteCb.checked = true;

      async function onSubmit() {
        await runRename();
      }
      if (submit) submit.addEventListener("click", onSubmit);

      renameHandle = modal.open(el, {
        focusTarget: input,
        onEscape: closeRename,
      });
      el.__cleanup = () => {
        if (submit) submit.removeEventListener("click", onSubmit);
      };
    }

    function closeRename() {
      const el = document.getElementById("rename-modal");
      if (el && el.__cleanup) { el.__cleanup(); el.__cleanup = null; }
      if (renameHandle) renameHandle.close();
      renameHandle = null;
    }

    async function runRename() {
      const s = state.get();
      const memPath = s.activeMemoryPath;
      const mem = memPath ? s.memories.find(m => m.path === memPath) : null;
      if (!memPath || !mem) { closeRename(); return; }
      const input = document.getElementById("rename-new-path");
      const rewriteCb = document.getElementById("rename-rewrite-links");
      const newPath = (input && input.value || "").trim();
      if (!newPath) { toast.show("New path is required"); return; }
      const rewrite = !rewriteCb || !!rewriteCb.checked;
      const filename = memPath.replace(/^[^/]+\//, "").replace(/\.md$/, "");
      const url = `/api/memory/${encodeURIComponent(s.activeBrain)}/${encodeURIComponent(mem.category)}/${encodeURIComponent(filename)}/rename?rewrite_links=${rewrite ? "true" : "false"}`;
      const resp = await api.post(url, { new_path: newPath });
      closeRename();
      if (!resp.ok) {
        toast.show(resp.error || "Rename failed");
        return;
      }
      const affected = (resp.data && Array.isArray(resp.data.affected_paths))
        ? resp.data.affected_paths.length
        : 0;
      const rewrittenCount = Math.max(0, affected - 1); // affected includes the renamed file
      toast.success(`Renamed (${rewrittenCount} link${rewrittenCount === 1 ? "" : "s"} rewritten)`);
      const newCanonical = (resp.data && resp.data.path) || newPath;
      const newCategory = newCanonical.split("/")[0] || mem.category;
      await loadMemories(s.activeBrain);
      router.navigate({ memoryPath: newCanonical, category: newCategory });
    }

    function wire() {
      const createCancel = document.getElementById("create-cancel");
      const createSubmit = document.getElementById("create-submit");
      const createInput = document.getElementById("create-name");
      if (createCancel) createCancel.addEventListener("click", () => closeCreate(null));
      if (createSubmit) createSubmit.addEventListener("click", () => {
        const name = createInput ? createInput.value.trim() : "";
        closeCreate(name);
      });
      if (createInput) createInput.addEventListener("keydown", e => {
        if (e.key === "Enter") {
          e.preventDefault();
          const name = createInput.value.trim();
          closeCreate(name);
        }
      });

      const deleteCancel = document.getElementById("delete-cancel");
      if (deleteCancel) deleteCancel.addEventListener("click", closeDelete);

      const renameCancel = document.getElementById("rename-cancel");
      if (renameCancel) renameCancel.addEventListener("click", closeRename);

      // Toolbar buttons.
      const renameBtn = document.getElementById("editor-rename");
      if (renameBtn) renameBtn.addEventListener("click", openRename);
      const deleteBtn = document.getElementById("editor-delete");
      if (deleteBtn) deleteBtn.addEventListener("click", openDelete);
    }

    return { openDraft, showCreate, submitCreate, openDelete, openRename, wire };
  })();

  // ── Commands (Cmd-K stub for Phase 5; palette UI in Phase 6) ──────────────

  const commands = (() => {
    const registry = {};
    function register(name, fn) { registry[name] = fn; }
    function run(name, ...args) {
      const fn = registry[name];
      if (!fn) return false;
      fn(...args);
      return true;
    }
    return { register, run };
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

  /** Render dirty marker + Save button enabled state. */
  function renderToolbar(s) {
    const dirtyEl = document.getElementById("dirty-indicator");
    const saveBtn = document.getElementById("editor-save");
    const modeBtn = document.getElementById("mode-toggle");
    if (dirtyEl) dirtyEl.hidden = !s.dirty;
    if (saveBtn) saveBtn.disabled = !s.buffer;
    if (modeBtn) modeBtn.textContent = s.mode === "edit" ? "read" : "edit";
  }

  function activeBrainWritable(s) {
    const b = s.brains.find(x => x.name === s.activeBrain);
    if (!b) return true;
    return b.writable !== false;
  }

  /** Remove all children from a node (safe; no innerHTML). */
  function clearNode(node) { while (node.firstChild) node.removeChild(node.firstChild); }

  /**
   * Render the editor / read-only banner / preview surface based on current
   * state. Owns the lifecycle of CodeMirror's mount/unmount and the read-only
   * banner DOM. Idempotent: safe to call on every state change.
   */
  function renderEditSurface(s) {
    const toolbar = document.getElementById("editor-toolbar");
    const fmForm = document.getElementById("frontmatter-form");
    const editorHost = document.getElementById("editor-host");
    const previewEl = document.getElementById("preview-content");
    const banner = document.getElementById("readonly-banner-host");

    if (!editorHost || !previewEl || !banner) return;

    const writable = activeBrainWritable(s);

    if (!writable) {
      if (toolbar) toolbar.hidden = true;
      if (fmForm) fmForm.hidden = true;
      editor.unmount();
      clearNode(editorHost);
      clearNode(banner);
      const div = document.createElement("div");
      div.className = "readonly-banner";
      div.setAttribute("role", "status");
      const strong = document.createElement("strong");
      strong.textContent = "Read-only brain.";
      const tail = document.createTextNode(" Open in a writable brain to edit memories.");
      div.appendChild(strong);
      div.appendChild(tail);
      banner.appendChild(div);
      previewEl.style.display = "";
      return;
    }

    clearNode(banner);
    if (toolbar) toolbar.hidden = false;

    if (!s.buffer) {
      if (fmForm) fmForm.hidden = true;
      editor.unmount();
      clearNode(editorHost);
      previewEl.style.display = "";
      return;
    }

    if (s.mode === "edit") {
      if (fmForm) fmForm.hidden = false;
      previewEl.style.display = "none";
      editorHost.style.display = "";
      if (!editor.getView()) {
        editor.mount(editorHost, s.buffer.body, (newDoc) => {
          const cur = state.get();
          if (!cur.buffer) return;
          const next = Object.assign({}, cur.buffer, { body: newDoc });
          state.set({ buffer: next, dirty: computeDirty(next) });
        });
        frontmatter.render(s.buffer.frontmatter);
        // Restore edit-mode scroll position after CM has laid out.
        // Double-rAF: first frame triggers CM's initial layout measurement;
        // second frame applies after CM has rendered all visible lines.
        const savedEditScroll = s.scrollPositions.edit;
        if (savedEditScroll) {
          requestAnimationFrame(() => requestAnimationFrame(() => {
            const p = document.getElementById("preview-panel");
            if (p) p.scrollTop = savedEditScroll;
          }));
        }
      } else {
        const view = editor.getView();
        const liveText = view && view.state ? view.state.doc.toString() : "";
        if (liveText !== s.buffer.body && !s.dirty) {
          editor.setDoc(s.buffer.body);
          frontmatter.render(s.buffer.frontmatter);
        }
      }
    } else {
      if (fmForm) fmForm.hidden = true;
      editor.unmount();
      clearNode(editorHost);
      editorHost.style.display = "none";
      previewEl.style.display = "";
      // Restore read-mode scroll position after the preview has rendered.
      const savedReadScroll = s.scrollPositions.read;
      if (savedReadScroll) {
        requestAnimationFrame(() => {
          const p = document.getElementById("preview-panel");
          if (p) p.scrollTop = savedReadScroll;
        });
      }
    }
  }

  state.subscribe(s => {
    render.brains(s.brains, s.activeBrain);
    render.categories(s.memories, s.activeCategory);
    render.memoryList(s.memories, s.activeCategory, s.activeMemoryPath, searchTerm);
    render.preview(s.preview);
    renderToolbar(s);
    renderEditSurface(s);
  });

  // ── Boot ───────────────────────────────────────────────────────────────────

  function boot() {
    theme.init();
    theme.updateToggleLabel();

    const themeBtn = document.getElementById("theme-toggle");
    if (themeBtn) themeBtn.addEventListener("click", () => theme.toggle());

    // Editor toolbar wiring.
    const modeBtn = document.getElementById("mode-toggle");
    if (modeBtn) modeBtn.addEventListener("click", () => {
      const s = state.get();
      const oldMode = s.mode;
      const next = oldMode === "edit" ? "read" : "edit";
      // Capture outgoing pane's scroll position before switching.
      // Both edit and read modes scroll via #preview-panel (the outer scroll
      // container). CM's scrollDOM grows to fill its flex host and does not
      // overflow independently, so the panel scroll is the canonical position.
      const scrollPositions = Object.assign({}, s.scrollPositions);
      const panel = document.getElementById("preview-panel");
      if (oldMode === "edit") {
        scrollPositions.edit = panel ? panel.scrollTop : 0;
      } else {
        scrollPositions.read = panel ? panel.scrollTop : 0;
      }
      state.set({ mode: next, scrollPositions });
    });
    const saveBtn = document.getElementById("editor-save");
    if (saveBtn) saveBtn.addEventListener("click", () => save.run());

    // Frontmatter inputs sync to state.buffer on every keystroke.
    frontmatter.wire();
    nav.init();

    // Phase 5: wire conflict + CRUD modals + commands.
    conflict.wire();
    crud.wire();
    commands.register("new-memory", (cat) => crud.openDraft(cat));
    window.__grugCommands = commands;

    // Window-level Cmd-S / Ctrl-S — fires save.run from anywhere in the page
    // (form fields, toolbar, etc). The CodeMirror keymap handles in-editor.
    window.addEventListener("keydown", e => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "s" || e.key === "S")) {
        e.preventDefault();
        save.run();
      }
    });

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

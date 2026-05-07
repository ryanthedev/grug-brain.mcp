/**
 * Render functions — update DOM from application state.
 *
 * render.brains(brains, activeBrain)
 * render.categories(memories, activeCategory)
 * render.memoryList(memories, activeCategory, activeMemoryPath, searchTerm)
 * render.preview(preview)
 * makeCategoryItem(cat, label, count, activeCategory) — also exported for testing
 */
import { router } from './router.js';
import { crud } from './crud.js';

/**
 * Build a category list item via safe DOM construction.
 * cat is null for "All", string for a real category.
 */
export function makeCategoryItem(cat, label, count, activeCategory) {
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

  // Per-category "+" button opens an unsaved-draft editor.
  // Only attached for real categories (not the "All" pseudo-category).
  if (cat !== null) {
    const addBtn = document.createElement("button");
    addBtn.className = "category-add";
    addBtn.setAttribute("aria-label", `New memory in ${cat}`);
    addBtn.textContent = "+"; // static
    addBtn.addEventListener("click", e => {
      e.stopPropagation();
      crud.openDraft(cat);
    });
    li.appendChild(addBtn);
  }
  return li;
}

export const render = {
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
      // Tags drive tagpane filter; comma-separated in dataset.
      if (Array.isArray(m.tags)) li.dataset.tags = m.tags.join(",");

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
   * Security boundary: user markdown body goes through two sanitization steps
   * before any DOM insertion:
   *   1. marked.parse() converts markdown to HTML
   *   2. DOMPurify.sanitize() removes all scripts, event handlers, javascript: URIs
   * The el.innerHTML assignment below is ONLY reached after DOMPurify sanitization.
   * If DOMPurify is unavailable, we fall back to textContent (no HTML rendering).
   */
  preview(previewData) {
    const el = document.getElementById("preview-content");
    if (!el) return;

    if (!previewData) {
      while (el.firstChild) el.removeChild(el.firstChild);
      const p = document.createElement("p");
      p.className = "preview-placeholder";
      p.textContent = "Select a memory to read it.";
      el.appendChild(p);
      return;
    }

    const rawBody = previewData.body || "";

    if (typeof marked === "undefined") {
      // Fallback: render as plain text (no HTML).
      while (el.firstChild) el.removeChild(el.firstChild);
      const pre = document.createElement("pre");
      pre.textContent = rawBody; // safe — textContent
      el.appendChild(pre);
      return;
    }

    let html = marked.parse(rawBody);

    if (typeof DOMPurify !== "undefined") {
      // Sanitize with DOMPurify before setting innerHTML.
      // Script tags, event handlers, and javascript: URLs are stripped.
      const safe = DOMPurify.sanitize(html, {
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
      // DOMPurify.sanitize() output is safe to assign to innerHTML.
      el.innerHTML = safe; // safe: DOMPurify-sanitized above
    } else {
      // DOMPurify not available — degrade to textContent.
      while (el.firstChild) el.removeChild(el.firstChild);
      const pre = document.createElement("pre");
      pre.textContent = rawBody;
      el.appendChild(pre);
    }
  },
};

/**
 * Cmd-K command palette — fuzzy search over memories, categories, and commands.
 *
 * Exported API:
 *   palette.open()    — open the palette modal
 *   palette.close()   — close the palette modal
 *   palette.toggle()  — open if closed, close if open
 *   palette._filter   — exposed for testing
 *   palette._gather   — exposed for testing
 */
import { state } from './state.js';
import { modal } from './modal.js';
import { commands } from './commands.js';
import { router } from './router.js';

export const palette = (() => {
  let handle = null;
  let items = [];          // current filtered items
  let selectedIdx = 0;

  /** Score how well `query` matches `title`. Returns null if no match. */
  function fuzzyScore(query, title) {
    if (!query) return 0;
    const q = query.toLowerCase();
    const t = title.toLowerCase();
    // 1. Initials match
    const initials = title.split(/[\s\-_/.]+/).filter(Boolean)
      .map(w => w[0] || "").join("").toLowerCase();
    if (initials.startsWith(q)) return 100 - (initials.length - q.length);
    // 2. Substring match
    const idx = t.indexOf(q);
    if (idx >= 0) {
      const wordBoundary = idx === 0 || /[\s\-_/.]/.test(t[idx - 1]);
      return 50 - idx + (wordBoundary ? 25 : 0);
    }
    // 3. Char subsequence
    let qi = 0;
    for (let i = 0; i < t.length && qi < q.length; i++) {
      if (t[i] === q[qi]) qi++;
    }
    if (qi === q.length) return 10 - title.length / 100;
    return null;
  }

  function gatherAll() {
    const s = state.get();
    const out = [];
    // Memories
    (s.memories || []).forEach(m => {
      out.push({
        kind: "memory",
        title: m.name,
        subtitle: m.path,
        action: () => router.navigate({
          memoryPath: m.path,
          memoryCategory: m.category,
        }),
      });
    });
    // Categories (unique)
    const cats = new Set();
    (s.memories || []).forEach(m => { if (m.category) cats.add(m.category); });
    cats.forEach(c => {
      out.push({
        kind: "category",
        title: c,
        action: () => router.navigate({ category: c }),
      });
    });
    // Commands
    commands.list().forEach(c => {
      out.push({
        kind: "command",
        title: c.title,
        name: c.name,
        action: () => commands.run(c.name),
      });
    });
    return out;
  }

  function filter(query) {
    const all = gatherAll();
    const scored = all
      .map(item => ({ item, score: fuzzyScore(query, item.title) }))
      .filter(x => x.score !== null)
      .sort((a, b) => b.score - a.score || a.item.title.length - b.item.title.length)
      .slice(0, 50);
    return scored.map(x => x.item);
  }

  function renderList() {
    const ul = document.getElementById("palette-list");
    if (!ul) return;
    while (ul.firstChild) ul.removeChild(ul.firstChild);
    items.forEach((item, i) => {
      const li = document.createElement("li");
      li.className = "palette-item" + (i === selectedIdx ? " active" : "");
      li.id = `palette-item-${i}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", i === selectedIdx ? "true" : "false");
      const kind = document.createElement("span");
      kind.className = "palette-kind";
      kind.textContent = item.kind;
      const title = document.createElement("span");
      title.className = "palette-title";
      title.textContent = item.title;
      li.appendChild(kind);
      li.appendChild(title);
      li.addEventListener("click", () => {
        selectedIdx = i;
        dispatchSelected();
      });
      ul.appendChild(li);
    });
    const input = document.getElementById("palette-input");
    if (input) {
      if (items.length > 0) {
        input.setAttribute("aria-activedescendant", `palette-item-${selectedIdx}`);
      } else {
        input.removeAttribute("aria-activedescendant");
      }
    }
  }

  function dispatchSelected() {
    const sel = items[selectedIdx];
    if (!sel) return;
    close();
    // Defer so close()'s focus restoration completes first.
    setTimeout(() => sel.action(), 0);
  }

  function open() {
    const el = document.getElementById("palette-modal");
    if (!el || handle) return;
    const input = document.getElementById("palette-input");
    if (input) input.value = "";
    items = gatherAll().slice(0, 50);
    selectedIdx = 0;
    renderList();
    handle = modal.open(el, { focusTarget: input });
    if (input) {
      input.addEventListener("input", onInput);
      input.addEventListener("keydown", onKeydown);
    }
  }

  function close() {
    if (!handle) return;
    const input = document.getElementById("palette-input");
    if (input) {
      input.removeEventListener("input", onInput);
      input.removeEventListener("keydown", onKeydown);
    }
    handle.close();
    handle = null;
  }

  function toggle() {
    if (handle) close(); else open();
  }

  function onInput(e) {
    items = filter(e.target.value);
    selectedIdx = 0;
    renderList();
  }

  function onKeydown(e) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (items.length === 0) return;
      selectedIdx = (selectedIdx + 1) % items.length;
      renderList();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (items.length === 0) return;
      selectedIdx = (selectedIdx - 1 + items.length) % items.length;
      renderList();
    } else if (e.key === "Enter") {
      e.preventDefault();
      dispatchSelected();
    }
  }

  return { open, close, toggle, _filter: filter, _gather: gatherAll };
})();

/**
 * Tag pane — shows all tags for the active brain, with click-to-filter.
 *
 * `router` is not needed here (tagpane filters the existing memory list,
 * not navigates). State mutations go through state.set() in the IIFE.
 *
 * Exported API:
 *   tagpane.render()                        — async; update #panel-tags-body
 *   tagpane.getActiveTag()                  — current active tag string or null
 *   tagpane.renderFilteredMemoryList(s)     — hide/show memory list items by tag
 */
import { state } from './state.js';
import { api } from './api.js';

export const tagpane = (() => {
  let activeTag = null;
  let token = 0;

  function getActiveTag() { return activeTag; }

  async function render() {
    const myToken = ++token;
    const body = document.getElementById("panel-tags-body");
    if (!body) return;
    const s = state.get();
    while (body.firstChild) body.removeChild(body.firstChild);
    if (!s.activeBrain) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "No active brain.";
      body.appendChild(p);
      return;
    }
    const r = await api.tags(s.activeBrain);
    if (myToken !== token) return;
    // Re-clear after await — state could have changed.
    while (body.firstChild) body.removeChild(body.firstChild);
    const rows = (r.ok && Array.isArray(r.data)) ? r.data : [];
    if (rows.length === 0) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "No tags yet.";
      body.appendChild(p);
      return;
    }
    rows.forEach(row => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "side-panel-item tag-item";
      if (row.tag === activeTag) btn.setAttribute("aria-pressed", "true");
      const name = document.createElement("span");
      name.className = "tag-name";
      name.textContent = "#" + row.tag;
      const count = document.createElement("span");
      count.className = "tag-count";
      count.textContent = "(" + row.count + ")";
      btn.appendChild(name);
      btn.appendChild(count);
      btn.dataset.tag = row.tag;
      btn.addEventListener("click", () => {
        activeTag = (activeTag === row.tag) ? null : row.tag;
        // Re-render memory list with tag filter applied.
        const s2 = state.get();
        render(); // refresh aria-pressed
        renderFilteredMemoryList(s2);
      });
      body.appendChild(btn);
    });
  }

  function renderFilteredMemoryList(s) {
    // Hook into render.memoryList — we filter the in-state memories array
    // for display but don't mutate state. The simpler approach: subscribe-
    // friendly is to use the existing render but filter via dataset.
    // For now: hide memory-list items whose tags don't include activeTag.
    const list = document.querySelectorAll("#memory-list .memory-item");
    list.forEach(li => {
      if (!activeTag) { li.hidden = false; return; }
      const tagsAttr = li.dataset.tags || "";
      const has = tagsAttr.split(",").map(t => t.trim()).includes(activeTag);
      li.hidden = !has;
    });
  }

  return { render, getActiveTag, renderFilteredMemoryList };
})();

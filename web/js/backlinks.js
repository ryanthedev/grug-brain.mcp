/**
 * Backlinks panel — shows which memories link to the active memory.
 *
 * `router` is not yet extracted — uses `window.router` at click-time.
 *
 * Exported API:
 *   backlinks.render() — async; updates the #panel-backlinks-body DOM
 */
import { state } from './state.js';
import { api } from './api.js';

export const backlinks = (() => {
  let token = 0;

  async function render() {
    const myToken = ++token;
    const body = document.getElementById("panel-backlinks-body");
    if (!body) return;
    const s = state.get();
    while (body.firstChild) body.removeChild(body.firstChild);
    if (!s.activeMemoryPath || !s.activeBrain) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "Open a memory to see who links to it.";
      body.appendChild(p);
      return;
    }
    const r = await api.backlinks(s.activeBrain, s.activeMemoryPath);
    if (myToken !== token) return; // a newer render superseded us
    // Re-clear in case state changed while awaiting.
    while (body.firstChild) body.removeChild(body.firstChild);
    if (!r.ok) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "Could not load backlinks.";
      body.appendChild(p);
      return;
    }
    const rows = Array.isArray(r.data) ? r.data : [];
    if (rows.length === 0) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "No backlinks.";
      body.appendChild(p);
      return;
    }
    rows.forEach(row => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "side-panel-item backlink-item";
      // Plain textContent — never trust user data with innerHTML.
      btn.textContent = row.name || row.path;
      btn.dataset.path = row.path;
      btn.dataset.category = row.category || "";
      btn.addEventListener("click", () => {
        // router is not yet extracted — use window.router for forward compatibility.
        if (window.router) {
          window.router.navigate({
            memoryPath: row.path,
            memoryCategory: row.category,
          });
        }
      });
      body.appendChild(btn);
    });
  }

  return { render };
})();

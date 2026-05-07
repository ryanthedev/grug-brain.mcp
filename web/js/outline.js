/**
 * Outline panel — shows heading hierarchy from the current buffer or preview.
 *
 * Reads the live CodeMirror view via `getView()` imported from editor.js
 * instead of the window global, resolving the window coupling (DW-2.2).
 *
 * Exported API:
 *   outline.render()              — update #panel-outline-body DOM
 *   outline.parseHeadings(text)   — pure: [{level, text, line}]
 */
import { state } from './state.js';
import { getView } from './editor.js';

export const outline = (() => {
  /** Parse markdown headings from the buffer body. Skips fenced code blocks. */
  function parseHeadings(text) {
    if (!text) return [];
    const lines = text.split("\n");
    const out = [];
    let inFence = false;
    const fenceRe = /^```/;
    const headingRe = /^(#{1,6})\s+(.+?)\s*#*\s*$/;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (fenceRe.test(line)) { inFence = !inFence; continue; }
      if (inFence) continue;
      const m = headingRe.exec(line);
      if (m) out.push({ level: m[1].length, text: m[2], line: i + 1 });
    }
    return out;
  }

  function render() {
    const body = document.getElementById("panel-outline-body");
    if (!body) return;
    while (body.firstChild) body.removeChild(body.firstChild);
    const s = state.get();
    const text = (s.buffer && s.buffer.body) || (s.preview && s.preview.body) || "";
    const headings = parseHeadings(text);
    if (headings.length === 0) {
      const p = document.createElement("p");
      p.className = "side-panel-empty";
      p.textContent = "No headings.";
      body.appendChild(p);
      return;
    }
    headings.forEach(h => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "side-panel-item outline-item outline-h" + h.level;
      btn.textContent = h.text;
      btn.dataset.line = String(h.line);
      btn.addEventListener("click", () => {
        // Use getView() from editor.js instead of window.__grugEditorView.
        const view = getView();
        if (view && view.state && view.dispatch) {
          try {
            const lineInfo = view.state.doc.line(Math.min(h.line, view.state.doc.lines));
            view.dispatch({
              selection: { anchor: lineInfo.from },
              effects: [],
              scrollIntoView: true,
            });
            view.focus();
          } catch (_) {}
        } else {
          // Read mode: try to scroll the preview pane to a heading element by text.
          const preview = document.getElementById("preview-content");
          if (!preview) return;
          const headers = preview.querySelectorAll("h1,h2,h3,h4,h5,h6");
          for (const el of headers) {
            if (el.textContent.trim() === h.text) {
              el.scrollIntoView({ behavior: "smooth", block: "start" });
              break;
            }
          }
        }
      });
      body.appendChild(btn);
    });
  }

  return { render, parseHeadings };
})();

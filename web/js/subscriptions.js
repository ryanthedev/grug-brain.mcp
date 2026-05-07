/**
 * Subscriptions — wires the central state.subscribe pipeline plus the
 * memory-list search input.
 *
 * Owns the grey-zone functions that don't fit any single widget module:
 *   renderToolbar(s)       — toolbar dirty + save button + mode label
 *   renderEditSurface(s)   — editor host / readonly banner / preview lifecycle
 *   activeBrainWritable(s) — brain readonly check
 *   clearNode(node)        — DOM helper
 *
 * Module-private `searchTerm` is mutated by the search input listener and
 * read by the state subscribe callback — they must stay together.
 *
 * Exported API:
 *   subscriptions.init() — register the state subscriber + wire #search-input
 */
import { state } from './state.js';
import { render } from './render.js';
import { editor } from './editor.js';
import { frontmatter } from './frontmatter.js';
import { backlinks } from './backlinks.js';
import { outline } from './outline.js';
import { tagpane } from './tagpane.js';
import { computeDirty } from './save.js';

export const subscriptions = (() => {
  // Module-level state. Written by wireSearch's input listener; read by the
  // state.subscribe callback below to filter the memory list.
  let searchTerm = "";

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

  function wireSearch() {
    const input = document.getElementById("search-input");
    if (!input) return;
    input.addEventListener("input", () => {
      searchTerm = input.value;
      const s = state.get();
      render.memoryList(s.memories, s.activeCategory, s.activeMemoryPath, searchTerm);
    });
  }

  function init() {
    state.subscribe(s => {
      render.brains(s.brains, s.activeBrain);
      render.categories(s.memories, s.activeCategory);
      render.memoryList(s.memories, s.activeCategory, s.activeMemoryPath, searchTerm);
      render.preview(s.preview);
      renderToolbar(s);
      renderEditSurface(s);
      // Side panels.
      backlinks.render();
      outline.render();
      tagpane.render();
      // Re-apply tag filter (memory list may have just re-rendered).
      tagpane.renderFilteredMemoryList(s);
    });
    wireSearch();
  }

  return { init };
})();

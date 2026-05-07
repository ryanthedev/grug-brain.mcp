/**
 * Conflict resolution modal — 3-pane diff view.
 *
 * Exported API:
 *   conflict.show(resp)    — open conflict modal with server 409 payload
 *   conflict.close()       — close the modal
 *   conflict.wire()        — attach button event listeners
 *   conflict._renderDiff   — exposed for testing
 */
import { state } from './state.js';
import { api } from './api.js';
import { toast } from './toast.js';
import { modal } from './modal.js';
import { frontmatter } from './frontmatter.js';
import { editor } from './editor.js';

export const conflict = (() => {
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

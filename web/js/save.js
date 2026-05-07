/**
 * Save flow — Cmd-S handler and draft-create routing.
 *
 * Exported API:
 *   save.run()        — async; saves active buffer or routes draft through create modal
 *   computeDirty(buf) — pure predicate; true when buffer differs from original snapshot
 *
 * Circular-import note: save.js ↔ editor.js ↔ (no save dep) and
 * save.js → crud.js (runtime showCreate/submitCreate).
 * save.js → conflict.js (runtime .show on 409).
 * All cross-references are inside function bodies — safe for native ES modules.
 */
import { state } from './state.js';
import { api } from './api.js';
import { toast } from './toast.js';
import { frontmatter } from './frontmatter.js';
import { editor } from './editor.js';
import { crud } from './crud.js';
import { conflict } from './conflict.js';

/** True when buffer differs from the original snapshot. */
export function computeDirty(buf) {
  if (!buf) return false;
  if (buf.body !== buf.originalBody) return true;
  return JSON.stringify(buf.frontmatter) !== JSON.stringify(buf.originalFrontmatter);
}

export const save = (() => {
  let inFlight = false;

  async function run() {
    if (inFlight) return;
    const s = state.get();
    const buf = s.buffer;
    if (!buf) return;

    // A draft buffer (no activeMemoryPath) routes through the Create modal to
    // collect a filename, then POSTs to /api/memory.
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
      // Open the structured 3-pane conflict modal.
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

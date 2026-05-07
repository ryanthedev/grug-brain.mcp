/**
 * CRUD: create / delete / rename modals.
 *
 * Create flow:
 *   crud.openDraft(category) — sets state.draft, no activeMemoryPath, mounts editor empty.
 *   First save (save.run while draft active) opens the create modal to collect a name.
 *   On submit, POST /api/memory with {path: "<category>/<name>", body, frontmatter}.
 *
 * Delete flow:
 *   crud.openDelete() — confirm modal; Delete enabled only when typed-name matches.
 *
 * Rename flow:
 *   crud.openRename() — input new path; POST .../rename?rewrite_links=true.
 *
 * Exported API:
 *   crud.openDraft(category)     — open a blank draft buffer
 *   crud.showCreate(prefill)     — Promise<string|null>; opens the create modal
 *   crud.submitCreate(name)      — POST create + navigate to new memory
 *   crud.openDelete()            — open delete-confirm modal
 *   crud.openRename()            — open rename modal
 *   crud.wire()                  — attach button event listeners
 */
import { state } from './state.js';
import { api } from './api.js';
import { toast } from './toast.js';
import { modal } from './modal.js';
import { frontmatter } from './frontmatter.js';
import { router } from './router.js';
import { loadMemories } from './loaders.js';

export const crud = (() => {
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
    // Test-surface: expose last rename response for Playwright assertions.
    window.__lastRenameResponse = resp.data;
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

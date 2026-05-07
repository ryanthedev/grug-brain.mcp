/**
 * Boot — wires DOM listeners and orchestrates module initialization.
 *
 * Called once from web/app.js on module load. Module scripts are deferred,
 * so the DOM is parsed before this runs — no DOMContentLoaded guard needed.
 *
 * Wiring is intentionally flat: one wiring per module/handler. Order matches
 * the original IIFE boot sequence to preserve behavior.
 *
 * Exposes the Playwright hooks window.__grugCommands and window.__grugPalette.
 * window.__grugState is exposed by app.js (so the getter doesn't depend on
 * boot running first); other __grug* hooks are wired inside their own modules.
 */
import { state } from './state.js';
import { api } from './api.js';
import { theme } from './theme.js';
import { graph } from './graph.js';
import { frontmatter } from './frontmatter.js';
import { save } from './save.js';
import { conflict } from './conflict.js';
import { crud } from './crud.js';
import { commands } from './commands.js';
import { palette } from './palette.js';
import { nav } from './nav.js';
import { sse } from './sse.js';
import { router, loadBrains } from './router.js';
import { toast } from './toast.js';
import { subscriptions } from './subscriptions.js';

function wireMode() {
  const modeBtn = document.getElementById('mode-toggle');
  if (!modeBtn) return;
  modeBtn.addEventListener('click', () => {
    const s = state.get();
    const oldMode = s.mode;
    const next = oldMode === 'edit' ? 'read' : 'edit';
    // Capture outgoing pane's scroll position before switching. Both modes
    // scroll via #preview-panel (the outer scroll container); CM's scrollDOM
    // grows to fill its flex host and does not overflow independently.
    const scrollPositions = Object.assign({}, s.scrollPositions);
    const panel = document.getElementById('preview-panel');
    if (oldMode === 'edit') scrollPositions.edit = panel ? panel.scrollTop : 0;
    else scrollPositions.read = panel ? panel.scrollTop : 0;
    state.set({ mode: next, scrollPositions });
  });
}

function registerCommands() {
  commands.register('new-memory', (cat) => crud.openDraft(cat),
    { title: 'New memory', kind: 'command' });
  commands.register('toggle-theme', () => theme.toggle(),
    { title: 'Toggle theme', kind: 'command' });
  commands.register('rename', () => {
    const s = state.get();
    if (s.activeMemoryPath) crud.openRename();
    else toast.show('Open a memory to rename it');
  }, { title: 'Rename memory', kind: 'command' });
  commands.register('delete', () => {
    const s = state.get();
    if (s.activeMemoryPath) crud.openDelete();
    else toast.show('Open a memory to delete it');
  }, { title: 'Delete memory', kind: 'command' });
  commands.register('jump-to-category', () => {
    const s = state.get();
    const cats = Array.from(new Set((s.memories || []).map(m => m.category).filter(Boolean)));
    if (cats.length === 0) { toast.show('No categories'); return; }
    router.navigate({ category: cats[0] });
  }, { title: 'Jump to category', kind: 'command' });
}

function wireGraphMode() {
  const btn = document.getElementById('graph-mode-toggle');
  if (!btn) return;
  btn.addEventListener('click', async () => {
    const pressed = btn.getAttribute('aria-pressed') === 'true';
    const next = !pressed;
    btn.setAttribute('aria-pressed', next ? 'true' : 'false');
    btn.textContent = next ? 'local' : 'global';
    const s = state.get();
    if (next && s.activeMemoryPath) {
      const r = await api.graphLocal(s.activeBrain, s.activeMemoryPath, 2);
      if (r.ok) graph.renderLocal(r.data, { focusPath: s.activeMemoryPath });
    } else if (s.activeBrain) {
      const r = await api.graph(s.activeBrain);
      if (r.ok) graph.render(r.data);
    }
  });
}

function wirePanelExpand() {
  // Shared handler for all .panel-expand-btn buttons.
  document.querySelectorAll('.panel-expand-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const panelId = btn.dataset.panel;
      const panel = document.getElementById(panelId);
      if (!panel) return;
      const expanded = panel.classList.toggle('panel-fullscreen-active');
      btn.setAttribute('aria-pressed', expanded ? 'true' : 'false');
      btn.setAttribute('aria-label', expanded ? 'Collapse panel' : 'Expand panel');
      if (panelId === 'graph-panel') graph.refresh();
    });
  });
}

function wireKeymaps() {
  // Cmd-K opens the palette.
  window.addEventListener('keydown', e => {
    if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
      e.preventDefault();
      palette.toggle();
    }
  });
  // Window-level Cmd-S / Ctrl-S — fires save.run from anywhere on the page
  // (form fields, toolbar, etc). The CodeMirror keymap handles in-editor.
  window.addEventListener('keydown', e => {
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) {
      e.preventDefault();
      save.run();
    }
  });
}

export function boot() {
  theme.init();
  theme.updateToggleLabel();
  const themeBtn = document.getElementById('theme-toggle');
  if (themeBtn) themeBtn.addEventListener('click', () => theme.toggle());
  wireMode();
  const saveBtn = document.getElementById('editor-save');
  if (saveBtn) saveBtn.addEventListener('click', () => save.run());
  frontmatter.wire();
  nav.init();
  conflict.wire();
  crud.wire();
  registerCommands();
  window.__grugCommands = commands;
  window.__grugPalette = palette;
  wireGraphMode();
  wirePanelExpand();
  wireKeymaps();
  subscriptions.init();
  window.addEventListener('hashchange', () => router.onRoute());
  sse.connect();
  loadBrains().then(() => router.onRoute());
}

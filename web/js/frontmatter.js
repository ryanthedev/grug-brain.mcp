/**
 * Frontmatter form — read/write the name/description/tags inputs.
 *
 * Exported API:
 *   frontmatter.parseTags(s)       — parse comma-separated tag string
 *   frontmatter.render(fm)         — populate form inputs from frontmatter object
 *   frontmatter.read()             — read current form values into frontmatter object
 *   frontmatter.validate(fm)       — validate; returns {ok, errors}
 *   frontmatter.showErrors(errors) — display validation errors in DOM
 *   frontmatter.assemble(fm)       — produce raw key-value lines for server
 *   frontmatter.wire()             — attach input listeners that update state
 */
import { state } from './state.js';

const NAME_INPUT = "fm-name";
const DESC_INPUT = "fm-description";
const TAGS_INPUT = "fm-tags";
const NAME_ERR = "fm-name-error";

/** True when buffer differs from the original snapshot. */
function computeDirty(buf) {
  if (!buf) return false;
  if (buf.body !== buf.originalBody) return true;
  return JSON.stringify(buf.frontmatter) !== JSON.stringify(buf.originalFrontmatter);
}

export const frontmatter = (() => {
  function parseTags(s) {
    if (!s) return [];
    return s.split(",").map(t => t.trim()).filter(Boolean);
  }

  function render(fm) {
    const n = document.getElementById(NAME_INPUT);
    const d = document.getElementById(DESC_INPUT);
    const t = document.getElementById(TAGS_INPUT);
    if (!n || !d || !t) return;
    n.value = fm.name || "";
    d.value = fm.description || "";
    const tagStr = Array.isArray(fm.tags) ? fm.tags.join(", ") : (fm.tags || "");
    t.value = tagStr;
    t.dataset.tagsCount = String(parseTags(tagStr).length);
    const err = document.getElementById(NAME_ERR);
    if (err) { err.hidden = true; err.textContent = ""; }
  }

  function read() {
    const n = document.getElementById(NAME_INPUT);
    const d = document.getElementById(DESC_INPUT);
    const t = document.getElementById(TAGS_INPUT);
    return {
      name: n ? n.value.trim() : "",
      description: d ? d.value.trim() : "",
      tags: parseTags(t ? t.value : ""),
    };
  }

  function validate(fm) {
    const errors = {};
    if (!fm.name || !fm.name.trim()) errors.name = "Name is required.";
    return { ok: Object.keys(errors).length === 0, errors };
  }

  function showErrors(errors) {
    const err = document.getElementById(NAME_ERR);
    if (!err) return;
    if (errors.name) {
      err.textContent = errors.name;
      err.hidden = false;
    } else {
      err.hidden = true;
      err.textContent = "";
    }
  }

  function assemble(fm) {
    // The server wraps the returned string in "---\n...\n---\n\n" itself,
    // so this function must return raw key-value lines only — no delimiters.
    const lines = [];
    if (fm.name) lines.push("name: " + fm.name);
    if (fm.description) lines.push("description: " + fm.description);
    if (fm.tags && fm.tags.length) lines.push("tags: " + fm.tags.join(", "));
    return lines.join("\n");
  }

  function wire() {
    const inputs = [NAME_INPUT, DESC_INPUT, TAGS_INPUT];
    inputs.forEach(id => {
      const el = document.getElementById(id);
      if (!el) return;
      el.addEventListener("input", () => {
        const s = state.get();
        if (!s.buffer) return;
        const fm = read();
        if (id === TAGS_INPUT) el.dataset.tagsCount = String(fm.tags.length);
        const next = Object.assign({}, s.buffer, { frontmatter: fm });
        state.set({ buffer: next, dirty: computeDirty(next) });
      });
    });
  }

  return { parseTags, render, read, validate, showErrors, assemble, wire };
})();

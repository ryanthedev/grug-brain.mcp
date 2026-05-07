// Entry point for the CodeMirror 6 IIFE bundle consumed by web/app.js.
//
// Re-exports the CM6 APIs needed by grug-brain Phase 4 (and the autocomplete
// pieces planned for Phase 6). Rollup wraps this in an IIFE that assigns the
// exported namespace to `window.CM`.
//
// To regenerate the vendored bundle:
//   cd web/build && npm ci && npm run bundle
// The committed output is web/vendor/codemirror.min.js — that's what runtime loads.

export { EditorState, StateEffect, StateField, RangeSetBuilder } from "@codemirror/state";
export {
  EditorView,
  Decoration,
  ViewPlugin,
  keymap,
  highlightActiveLine,
} from "@codemirror/view";
export { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
export { syntaxHighlighting, defaultHighlightStyle, HighlightStyle } from "@codemirror/language";
export { markdown } from "@codemirror/lang-markdown";
export { basicSetup } from "codemirror";
export {
  autocompletion,
  completionKeymap,
  startCompletion,
  closeCompletion,
  acceptCompletion,
  CompletionContext,
} from "@codemirror/autocomplete";

# web/build/ — one-shot CodeMirror 6 bundler

This directory exists for one purpose: produce `web/vendor/codemirror.min.js`,
a single IIFE bundle that exposes the CM6 APIs grug-brain uses on `window.CM`.

CodeMirror 6 ships ESM-only on npm, so we cannot vendor an upstream UMD/IIFE
file directly. We do the rollup ourselves, commit the OUTPUT, and never load
anything from `web/build/` at runtime. The "no npm at the frontend root"
constraint applies to runtime, not build — see `docs/code-standards.md`.

## Regenerate

```sh
cd web/build
npm ci
npm run bundle
```

This writes `../vendor/codemirror.min.js`. After regenerating, commit the new
vendor file and update `web/vendor/VERSIONS.txt` with the new size.

## Globals exposed

After loading the bundle, `window.CM` has:

- Document model: `EditorState`, `StateEffect`, `StateField`, `RangeSetBuilder`
- View: `EditorView`, `Decoration`, `ViewPlugin`, `keymap`, `highlightActiveLine`
- Commands: `defaultKeymap`, `history`, `historyKeymap`
- Language: `syntaxHighlighting`, `defaultHighlightStyle`, `HighlightStyle`,
  `markdown`
- Convenience: `basicSetup`

## Pinned versions

Versions are pinned in `package.json`. The lockfile is committed. Bumps
require a deliberate PR with size delta in the commit message.

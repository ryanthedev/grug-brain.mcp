---
description: Add documentation to grug-brain for FTS-indexed search. Ingest from local directories or GitHub repos. Run /ingest [source] where source is a path or github:owner/repo.
argument-hint: [source]
allowed-tools: Bash, Read, Grep, Glob, Write
---

Add documentation to grug-brain's `docs/` directory at `${CLAUDE_PLUGIN_ROOT}/docs/`. Each doc set goes in a category subdirectory: `docs/<category>/`. Docs are indexed by FTS5 on server restart and searchable via `grug-search` and `grug-docs`.

## 1. Determine the source

`$ARGUMENTS` contains the source. Handle each type:

- **Local directory** (path exists on disk): Use directly.
- **GitHub** (`github:<owner>/<repo>` or a GitHub URL): Clone and find markdown.
- **Empty or topic name only** (e.g., "tailwind"): Ask the user for either a local path or a GitHub repo URL. Do not guess.

## 2. Pick a category name

Short, lowercase, hyphens only (e.g., `react-native`, `drizzle`, `tailwind`). Derive from the repo/directory name. Ask the user to confirm.

## 3. Check for existing

```bash
ls ${CLAUDE_PLUGIN_ROOT}/docs/<category>/ 2>/dev/null | head -5
```

If the category already exists, ask: update (overwrite), or cancel?

## 4. Get the files

### Local path

```bash
rsync -av --include='*/' --include='*.md' --include='*.mdx' --exclude='_*' --exclude='.*' --exclude='*' <source>/ ${CLAUDE_PLUGIN_ROOT}/docs/<category>/
```

### GitHub repo

First, clone shallowly and find where the markdown lives:

```bash
git clone --depth 1 <repo-url> /tmp/grug-ingest-<category>
find /tmp/grug-ingest-<category> -maxdepth 3 -type d -name docs -o -name documentation -o -name content -o -name pages | head -5
```

If no obvious docs directory, list the top-level structure and ask the user which directory to ingest.

Once the source directory is identified:

```bash
rsync -av --include='*/' --include='*.md' --include='*.mdx' --exclude='_*' --exclude='.*' --exclude='*' /tmp/grug-ingest-<category>/<docs-dir>/ ${CLAUDE_PLUGIN_ROOT}/docs/<category>/
rm -rf /tmp/grug-ingest-<category>
```

### Verify

If zero files were copied, report the error and do not proceed. Check:

```bash
find ${CLAUDE_PLUGIN_ROOT}/docs/<category> -name '*.md' -o -name '*.mdx' | wc -l
```

## 5. Report

Tell the user:
- How many files were added
- The category name
- Docs will be indexed on next MCP server restart (restart Claude Code or run `/setup`)

## Tips

- Focused doc sets (one framework, one library) work better than dumping an entire site.
- Prose-heavy guides and API references search well. Changelogs and auto-generated tables search poorly.
- Porter stemming handles plurals and word forms automatically.

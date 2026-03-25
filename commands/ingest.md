---
description: Add documentation to grug-brain for FTS-indexed search. Ingest from local directories or GitHub repos. Supports subfolder paths like github:owner/repo/path/to/section.
argument-hint: [source]
allowed-tools: Bash, Read, Grep, Glob, Write
---

Add documentation to grug-brain's `docs/` directory at `${CLAUDE_PLUGIN_ROOT}/docs/`. Each doc set goes in a category subdirectory: `docs/<category>/`. Docs are indexed by FTS5 on server restart and searchable via `grug-search` and `grug-docs`.

## 1. Determine the source

`$ARGUMENTS` contains the source. Handle each type:

- **Local directory** (path exists on disk): Use directly.
- **GitHub repo** (`github:<owner>/<repo>`): Clone entire repo, ingest all markdown.
- **GitHub subfolder** (`github:<owner>/<repo>/<path>`): Clone repo, ingest only the specified subfolder. Examples:
  - `github:ryanthedev/grug-docs/agentic-planning` — entire category
  - `github:ryanthedev/grug-docs/agentic-planning/extractions` — just extractions
- **Empty or topic name only** (e.g., "tailwind"): Ask the user for either a local path or a GitHub repo URL. Do not guess.

Parse the GitHub source by splitting on `/` after `github:` — first two segments are owner/repo, the rest is the path within the repo.

## 2. Pick a category name

Short, lowercase, hyphens only (e.g., `react-native`, `agentic-planning`).

- If ingesting a full repo: derive from the repo name.
- If ingesting a subfolder: use the first path segment as category (e.g., `agentic-planning` from `agentic-planning/extractions`).
- Ask the user to confirm.

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

### GitHub repo (full or subfolder)

Clone shallowly:

```bash
git clone --depth 1 <repo-url> /tmp/grug-ingest
```

Where `<repo-url>` is `https://github.com/<owner>/<repo>.git` (only owner/repo, not the subfolder path).

Determine the source directory inside the clone:
- **Full repo**: Find the docs directory (`find /tmp/grug-ingest -maxdepth 3 -type d -name docs -o -name documentation -o -name content | head -5`). If no obvious docs dir, list top-level and ask the user.
- **Subfolder specified**: Use `/tmp/grug-ingest/<path>` directly. If it doesn't exist, list what's available and ask.

Copy the files, preserving structure relative to the source:

```bash
rsync -av --include='*/' --include='*.md' --include='*.mdx' --exclude='_*' --exclude='.*' --exclude='*' /tmp/grug-ingest/<source-dir>/ ${CLAUDE_PLUGIN_ROOT}/docs/<category>/
rm -rf /tmp/grug-ingest
```

### Verify

If zero files were copied, report the error and do not proceed. Check:

```bash
find ${CLAUDE_PLUGIN_ROOT}/docs/<category> -name '*.md' -o -name '*.mdx' | wc -l
```

## 5. Report

Tell the user:
- How many files were added
- The category name and source path
- Docs will be indexed on next MCP server restart (restart Claude Code or run `/setup`)

## Tips

- Focused doc sets work better than dumping an entire site.
- Use subfolder paths to ingest specific sections: `github:org/grug-docs/agentic-planning/extractions`
- Prose-heavy guides and API references search well. Changelogs and auto-generated tables search poorly.
- Porter stemming handles plurals and word forms automatically.

---
description: Add documentation to grug-brain for FTS-indexed search. Ingest from local directories or GitHub repos. Supports subfolder paths like github:owner/repo/path/to/section.
argument-hint: [source]
allowed-tools: Bash, Read, Grep, Glob, Write
---

Add documentation to grug-brain as a brain entry. Each doc set lives in a directory that grug-brain indexes on startup. Docs are searchable via `grug-search` and `grug-read`.

## 1. Determine the source

`$ARGUMENTS` contains the source. Handle each type:

- **Local directory** (path exists on disk): Use directly.
- **GitHub repo** (`github:<owner>/<repo>`): Clone entire repo, ingest all markdown.
- **GitHub subfolder** (`github:<owner>/<repo>/<path>`): Clone repo, ingest only the specified subfolder. Examples:
  - `github:ryanthedev/grug-docs/agentic-planning` — entire category
  - `github:ryanthedev/grug-docs/agentic-planning/extractions` — just extractions
- **Empty or topic name only** (e.g., "tailwind"): Ask the user for either a local path or a GitHub repo URL. Do not guess.

Parse the GitHub source by splitting on `/` after `github:` — first two segments are owner/repo, the rest is the path within the repo.

## 2. Pick a brain name and target directory

Short, lowercase, hyphens only (e.g., `react-native`, `agentic-planning`).

- If ingesting a full repo: derive from the repo name.
- If ingesting a subfolder: use the first path segment as the name (e.g., `agentic-planning` from `agentic-planning/extractions`).
- Ask the user to confirm.

The target directory is where docs will live on disk. Default: `~/.grug-brain/<brain-name>/`. Ask the user if they prefer a different location.

## 3. Check for existing brain entry

Check if a brain with this name already exists in `~/.grug-brain/brains.json`:

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

If a brain with this name already exists and has a `source` field, offer to re-ingest from the stored source without asking for the source again. If the user confirms, skip step 1 and use the stored source. If no source is stored, ask: update (provide source again), or cancel?

## 4. Get the files

### Local path

```bash
rsync -av --include='*/' --include='*.md' --include='*.mdx' --exclude='_*' --exclude='.*' --exclude='*' <source>/ <target-dir>/
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
rsync -av --include='*/' --include='*.md' --include='*.mdx' --exclude='_*' --exclude='.*' --exclude='*' /tmp/grug-ingest/<source-dir>/ <target-dir>/
rm -rf /tmp/grug-ingest
```

### Verify

If zero files were copied, report the error and do not proceed. Check:

```bash
find <target-dir> -name '*.md' -o -name '*.mdx' | wc -l
```

## 5. Add brain entry to brains.json

After files are in place, add (or update) the brain entry in `~/.grug-brain/brains.json`.

Read the current config:

```bash
cat ~/.grug-brain/brains.json 2>/dev/null || echo '[]'
```

Add the new entry. Ingested docs brains are read-only and non-primary. Include the `source` field using the original source argument so re-ingestion can use it without re-asking:

```json
{
  "name": "<brain-name>",
  "dir": "<target-dir>",
  "primary": false,
  "writable": false,
  "flat": false,
  "git": null,
  "source": "<original source argument, e.g. github:owner/repo/path or local absolute path>"
}
```

If the directory structure is flat (all files at top level, no category subdirectories), use `"flat": true` instead.

Write the updated array back to `~/.grug-brain/brains.json`. Preserve all existing entries — only add or replace the entry for this brain name.

## 6. Report

Tell the user:
- How many files were added
- The brain name and target directory
- The entry added to brains.json
- Docs will be indexed on next MCP server restart (restart Claude Code or run `/setup`)

## Tips

- Focused doc sets work better than dumping an entire site.
- Use subfolder paths to ingest specific sections: `github:org/grug-docs/agentic-planning/extractions`
- Prose-heavy guides and API references search well. Changelogs and auto-generated tables search poorly.
- Porter stemming handles plurals and word forms automatically.
- To remove a doc brain, delete its entry from `~/.grug-brain/brains.json` and restart the MCP server.

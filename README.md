# grug-brain

Persistent memory for LLMs. Point it at any number of directories, and grug indexes every markdown file into one FTS5 search index. Git sync keeps brains in lockstep across machines. Conflicts get caught, not lost.

## Install

```bash
claude plugin add grug-brain
/install
```

`/install` builds from source, installs the background service, configures your brains, and verifies everything works. Run it again after plugin updates to pick up the new version.

## Web Viewer

Once the server is running, open `http://localhost:7777` (or check `~/.grug-brain/serve.port` if the default port was taken).

### Read mode

Brain switcher, category tree, memory list with live search, DOMPurify-sanitized markdown preview, and a global similarity graph (sigma.js + Fruchterman-Reingold layout). Live-reloads via SSE when any file changes on disk. Theme follows OS preference and can be toggled manually.

### Edit mode (Plan 2)

Every memory opens in edit mode by default:

- **Frontmatter form** — name (required), description, tags (comma-separated).
- **CodeMirror 6 editor** — markdown body with `[[wikilink]]` and `#tag` syntax highlighting.
- **Wikilink + tag autocomplete** — type `[[` to complete from memory titles; type `#` to complete from the tag index.
- **Cmd-S / Save button** — saves via `PUT /api/memory/{path}` with ETag concurrency control.
- **Conflict modal** — stale-ETag 409 opens a 3-pane diff (yours / theirs / merged preview via jsdiff).
- **Read/edit toggle** — preserves scroll position and unsaved buffer.
- **Unsaved-changes guard** — blocks navigation with a confirm modal; fires `beforeunload` for browser-tab close.
- **Read-only brain banner** — replaces the editor on non-writable brains.

### Create / Delete / Rename (Plan 2)

- **Create** — click `+` next to any category, or use Cmd-K "New memory". First save prompts for a filename.
- **Delete** — toolbar button opens a confirm modal; type the memory name to enable the Delete button.
- **Rename** — toolbar button opens a rename modal; `rewrite_links=true` by default rewrites every `[[wikilink]]` across the brain atomically.

### Navigation panels (Plan 2)

- **Backlinks panel** — memories that wikilink to the current memory.
- **Outline panel** — heading tree parsed from the current buffer; click to scroll.
- **Tag pane** — all tags with counts; click to filter the memory list.
- **Local N-hop graph** — sigma.js graph of the N-hop neighborhood around the current memory.

### Cmd-K palette (Plan 2)

Fuzzy quick-switcher over memories, categories, and commands:

| Command | Action |
|---------|--------|
| Toggle theme | Cycle light / dark / system |
| New memory | Open a blank draft editor |
| Rename | Open rename modal for current memory |
| Delete | Open delete confirm modal for current memory |
| Jump to category | Navigate to the first category |

### Keyboard shortcuts

| Shortcut | Surface | Action |
|----------|---------|--------|
| Cmd-S / Ctrl-S | Anywhere | Save current memory |
| Cmd-K / Ctrl-K | Anywhere | Open / close Cmd-K palette |
| Escape | Any open modal or palette | Close / dismiss |
| Enter / Space | Memory list item | Open memory |
| Arrow Up / Down | Palette list | Navigate results |
| Enter | Palette | Dispatch selected item |
| Tab / Shift-Tab | Any modal | Cycle focus within modal (trapped) |

All modals restore focus to the element that was active when the modal opened.

### VoiceOver smoke-test checklist

Manual screen-reader verification — expected announcements with VoiceOver on macOS (Safari):

**Edit save flow:**
1. Open a memory. VoiceOver should announce: "Memory body editor" when focus lands on the CodeMirror editor.
2. Edit text and press Cmd-S. VoiceOver should announce the success toast: "Saved" (role=status, aria-live=polite).
3. If there is a conflict, VoiceOver should announce "Conflict — file changed on disk" (dialog announced on open, focus moves to Cancel button).

**Conflict modal:**
1. On modal open, VoiceOver announces dialog label "Conflict — file changed on disk".
2. Tab cycles through Cancel, Reload theirs, Overwrite buttons.
3. Press Escape — dialog closes, VoiceOver returns to the element that opened the modal.

**Palette navigate:**
1. Press Cmd-K. VoiceOver announces: "Command palette, dialog" and focus moves to the search input ("Palette search, combobox").
2. Typing filters the list; each selection change announces the item via aria-activedescendant.
3. Press Escape — dialog closes, focus returns to opener.

**Delete confirm modal:**
1. VoiceOver announces "Delete memory, dialog" on open.
2. The confirm input has aria-describedby pointing to the target memory name.
3. Typing the name enables the Delete button; VoiceOver announces the button state change.

### Bundle audit (Plan 2)

Total `web/vendor/` size: **1,035,743 bytes (1.01 MB)**

| File | Bytes | Library |
|------|-------|---------|
| `codemirror.min.js` | 788,143 | CodeMirror 6 (state + view + commands + language + lang-markdown + search + autocomplete + codemirror) |
| `sigma.min.js` | 97,311 | sigma.js 2.4.0 |
| `graphology.min.js` | 74,221 | graphology 0.25.4 |
| `marked.min.js` | 36,521 | marked (markdown renderer) |
| `dompurify.min.js` | 22,068 | DOMPurify (XSS sanitizer) |
| `jsdiff.min.js` | 17,479 | jsdiff 5.2.0 (conflict diff) |

Plan 1 baseline: 432 KB (cytoscape 373 KB + dompurify 22 KB + marked 37 KB).
Plan 2 delta: +604 KB (CodeMirror 788 KB − cytoscape 373 KB + sigma 97 KB + graphology 74 KB + jsdiff 17 KB).
Total Plan 2 vendor footprint: 1.01 MB (within the 1.2 MB informational cap).

### Binary size

Plan 1: 11.0 MB | Plan 2: **12.5 MB** (12,508,720 bytes).

The delta (+1.5 MB) corresponds to the new `web/vendor/` files (CodeMirror 788 KB +
sigma 97 KB + graphology 74 KB + jsdiff 17 KB, less the cytoscape −373 KB removal).
`web/build/` is excluded from the binary embed — only the runtime `web/` assets ship.

## Architecture

```
Claude Code                grug serve (background service)
    |                           |
    v                      SQLite FTS5
grug --stdio               Git sync
    |                      File indexing
    +--- unix socket ---+  Background timers
```

Two modes in one binary:

- **`grug serve`** -- background server (brew service). Owns the SQLite database, runs git sync timers, indexes files. Listens on `~/.grug-brain/grug.sock`.
- **`grug --stdio`** -- thin MCP client for Claude Code. Forwards every tool call to the running server over the Unix socket. Near-zero startup time (~1ms).

The server runs as a launchd agent (macOS) or systemd user service (Linux). Install it with:

```bash
grug serve --install-service
```

## Brains

A brain is a directory of markdown files. You split them however you want.

```json
[
  {
    "name": "self",
    "dir": "~/.grug-brain/self",
    "writable": true,
    "primary": true
  },
  {
    "name": "hive",
    "dir": "~/.grug-brain/memories",
    "git": "git@github.com:you/grug-memories.git",
    "writable": true,
    "syncInterval": 60
  },
  {
    "name": "docs",
    "dir": "~/repos/project-docs",
    "flat": true
  }
]
```

Config lives at `~/.grug-brain/brains.json`. First run creates it for you.

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | yes | | Unique identifier |
| `dir` | yes | | Directory path. `~` works. |
| `primary` | no | `false` | Where writes land when you don't specify a brain. One brain only. |
| `writable` | no | `true` (`false` if flat) | Controls grug-write and grug-delete access |
| `flat` | no | `false` | No category subdirectories. The brain name becomes the category. |
| `git` | no | `null` | Remote URL for sync |
| `syncInterval` | no | `60` | Seconds between git push/pull |
| `source` | no | `null` | Origin URL for /ingest refresh |
| `refreshInterval` | no | `null` | Seconds between doc refresh. Read-only brains only. Minimum 3600. |

## Tools

**grug-write** stores a memory. Goes to the primary brain unless you say otherwise.

```
grug-write category:"feedback" path:"no-mocks" content:"don't mock the database"
grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"..."
```

**grug-search** hits every brain at once. Results carry a tag showing where they came from.

```
grug-search query:"sqlite"
> [hive] loopback/powersync-patch-vs-upsert.md
> [research] bun/runtime-sqlite.mdx
> [docs] connect-bun-sqlite.mdx
```

**grug-read** lets you drill into any brain.

```
grug-read                                    # list all brains
grug-read brain:"docs"                       # categories
grug-read brain:"docs" category:"api"        # files
grug-read brain:"hive" category:"feedback" path:"no-mocks"  # content
```

**grug-recall** catches you up on a brain. Writes a full listing to recall.md and returns the highlights.

```
grug-recall                    # primary brain
grug-recall brain:"hive"       # pick one
```

**grug-delete** removes a file. Writable brains only.

```
grug-delete brain:"self" category:"scratch" path:"old-note"
```

**grug-config** adds and removes brains without restarting the server.

```
grug-config action:"list"
grug-config action:"add" name:"tailwind" dir:"~/.grug-brain/tailwind" flat:true
grug-config action:"remove" name:"tailwind"
```

**grug-sync** triggers a git sync for one or all brains.

```
grug-sync                      # sync all brains with git remotes
grug-sync brain:"hive"         # sync one brain
```

**grug-dream** does maintenance. Commits pending changes across writable brains, surfaces git conflicts, finds cross-links between memories, flags anything that's gone stale.

```
/dream
/loop 30m /dream
```

## Conflicts

Two machines edit the same file between syncs. Grug saves your local version to `conflicts/` in the primary brain with frontmatter recording the original path, brain, hostname, and date.

Dream tells you when conflicts exist. Three steps to fix one:

1. Read the conflict file
2. Write the correct version to the original path
3. Delete the conflict entry

## File Layout

```
~/.grug-brain/
  brains.json
  grug.db
  grug.sock
  grug.pid
  launchd-stdout.log
  launchd-stderr.log
  self/
    scratch/
      i-am-alive.md
  memories/
    feedback/
    loopback/
    conflicts/
```

## Service Management

### macOS (launchd)

```bash
# Install / reinstall
grug serve --install-service

# Restart
launchctl kickstart -k gui/$(id -u)/com.grug-brain.server

# Stop
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.server.plist

# Logs
cat ~/.grug-brain/launchd-stderr.log
```

### Linux (systemd)

```bash
# Install / reinstall
grug serve --install-service

# Restart
systemctl --user restart grug-brain.service

# Stop
systemctl --user stop grug-brain.service

# Logs
journalctl --user -u grug-brain.service
```

## License

MIT

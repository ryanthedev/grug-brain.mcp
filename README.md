# grug-brain

Persistent memory for LLMs. Point it at any number of directories, and grug indexes every markdown file into one FTS5 search index. Git sync keeps brains in lockstep across machines. Conflicts get caught, not lost.

## Install

```bash
brew install rtd/grug/grug-brain
claude plugin add grug-brain
/setup
```

`/setup` installs the background service, configures your brains, and verifies everything works.

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

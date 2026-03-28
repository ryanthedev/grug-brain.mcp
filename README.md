# grug-brain

Persistent memory for LLMs. Any number of knowledge sources ("brains") unified in one searchable index. Git-synced across machines. Conflict resolution built in.

## Install

```bash
claude plugin add grug-brain
/setup
```

## Brains

Everything is a brain. A brain is a directory of markdown files that grug indexes and searches. You decide how to split them up.

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
    "git": "git@github.com:ryanthedev/grug-memories.git",
    "writable": true,
    "syncInterval": 60
  },
  {
    "name": "research",
    "dir": "~/repos/grug-docs",
    "git": "git@github.com:ryanthedev/grug-docs.git",
    "writable": true,
    "syncInterval": 300
  },
  {
    "name": "drizzle",
    "dir": "~/repos/drizzle-orm-docs/src/content/docs",
    "flat": true
  }
]
```

Config lives at `~/.grug-brain/brains.json`. Auto-created on first run.

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | yes | | Unique identifier |
| `dir` | yes | | Directory path. Supports `~`. |
| `primary` | no | `false` | Default target for writes. Exactly one brain should be primary. |
| `writable` | no | `true` (`false` if flat) | Whether grug-write and grug-delete work here |
| `flat` | no | `false` | Files directly in dir, no category subdirectories |
| `git` | no | `null` | Git remote URL for sync |
| `syncInterval` | no | `60` | Seconds between git sync |
| `source` | no | `null` | Origin URL for /ingest refresh |
| `refreshInterval` | no | `null` | Seconds between doc refresh (read-only brains only, minimum 3600) |

## Tools

**grug-write** stores a memory. Defaults to the primary brain.

```
grug-write category:"feedback" path:"no-mocks" content:"don't mock the database"
grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"..."
```

**grug-search** searches all brains at once. Results tagged with the source brain.

```
grug-search query:"sqlite"
> [hive] loopback/powersync-patch-vs-upsert.md
> [research] bun/runtime-sqlite.mdx
> [drizzle] connect-bun-sqlite.mdx
```

**grug-read** browses brains.

```
grug-read                                    # list all brains
grug-read brain:"research"                   # list categories in research
grug-read brain:"research" category:"bun"    # list files
grug-read brain:"hive" category:"feedback" path:"no-mocks"  # read file
```

**grug-recall** gets you up to speed on a brain.

```
grug-recall                    # primary brain
grug-recall brain:"hive"       # specific brain
```

**grug-delete** removes a memory. Only works on writable brains.

```
grug-delete brain:"self" category:"scratch" path:"old-note"
```

**grug-config** manages brains at runtime. No restart needed.

```
grug-config action:"list"
grug-config action:"add" name:"tailwind" dir:"~/.grug-brain/tailwind" flat:true
grug-config action:"remove" name:"tailwind"
```

**grug-dream** runs maintenance across all writable brains. Commits pending changes, surfaces conflicts, finds cross-links, flags stale memories.

```
/dream
/loop 30m /dream
```

## Conflict Resolution

When two machines edit the same file between syncs, grug saves your local version to the `conflicts/` category in the primary brain. Each conflict has frontmatter with the original path, brain, hostname, and date.

Dream surfaces conflicts. To resolve:

1. Read the conflict file
2. Write the correct version to the original path
3. Delete the conflict entry

## Adding Third-Party Docs

```
/ingest github:sveltejs/kit/documentation/docs
```

This clones the repo, copies markdown to `~/.grug-brain/<name>/`, and adds a brain entry. Set `refreshInterval` on the brain to keep it current.

## File Layout

```
~/.grug-brain/
  brains.json
  grug.db
  self/
    scratch/
      i-am-alive.md
  memories/
    feedback/
    loopback/
    conflicts/
```

## License

MIT

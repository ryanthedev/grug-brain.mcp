# Getting Started

## Install

```bash
claude plugin add grug-brain
/setup
```

Setup registers the MCP server and walks you through creating `~/.grug-brain/brains.json`.

## First Run

No config file? Grug creates one with a single `self` brain at `~/.grug-brain/self/`. That's your personal brain. Writes land here by default. Nothing leaves your machine until you connect a git remote.

## Write Something

```
grug-write category:"scratch" path:"first-note" content:"testing grug-brain"
```

Creates `~/.grug-brain/self/scratch/first-note.md`. Searchable immediately.

## Search

```
grug-search query:"testing"
```

Pulls results from every brain. Tags tell you the source: `[self]`, `[hive]`, `[research]`.

## Add a Shared Brain

Memories that sync across machines need a git remote.

```
grug-config action:"add" name:"hive" dir:"~/.grug-brain/memories" git:"git@github.com:you/memories.git" writable:true syncInterval:60
```

Write to it:

```
grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"don't mock the database in integration tests"
```

Grug commits and pushes every 60 seconds. Another machine with the same remote picks it up automatically.

## Third-Party Docs

```
/ingest github:drizzle-team/drizzle-orm/docs
```

Clones the repo, copies markdown to `~/.grug-brain/drizzle/`, adds a read-only brain entry. Now `grug-search query:"insert"` returns hits from your notes and the drizzle docs together.

Want them to stay current? Edit `~/.grug-brain/brains.json` and add `"refreshInterval": 86400` to the entry. Grug pulls fresh copies daily.

## Personal Notes

`self` is primary. No git remote. Nothing syncs.

```
grug-write category:"wip" path:"half-baked-idea" content:"what if we..."
```

When something belongs in the shared brain:

```
grug-write brain:"hive" category:"architecture" path:"auth-decision" content:"chose JWT for stateless scaling"
```

## Browse

```
grug-read                                        # all brains
grug-read brain:"hive"                           # categories
grug-read brain:"hive" category:"feedback"       # files
grug-read brain:"hive" category:"feedback" path:"no-mocks"  # one file
```

## Maintenance

```
/dream
```

Commits pending changes, surfaces conflicts, flags stale memories. Run it on a loop if you want:

```
/loop 30m /dream
```

## Conflicts

Two machines wrote the same file between syncs. Grug stashes your local version in `conflicts/` inside the primary brain. Dream flags it. To fix:

1. `grug-read brain:"self" category:"conflicts" path:"the-conflict-file"`
2. `grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"merged version"`
3. `grug-delete brain:"self" category:"conflicts" path:"the-conflict-file"`

# Getting Started

## Install

```bash
claude plugin add grug-brain
/setup
```

Setup registers the MCP server, creates `~/.grug-brain/brains.json`, and walks you through connecting brains.

## First Run

On first start with no config, grug creates a default `self` brain at `~/.grug-brain/self/`. This is your personal brain. Writes go here by default. Nothing syncs until you set up git.

## Write Something

```
grug-write category:"scratch" path:"first-note" content:"testing grug-brain"
```

This creates `~/.grug-brain/self/scratch/first-note.md`. Searchable immediately.

## Search

```
grug-search query:"testing"
```

Returns results from all brains. Each result shows which brain it came from: `[self]`, `[hive]`, `[research]`, etc.

## Add a Shared Brain

Want memories that sync across machines? Add a hive brain with a git remote.

```
grug-config action:"add" name:"hive" dir:"~/.grug-brain/memories" git:"git@github.com:you/memories.git" writable:true syncInterval:60
```

Now write to it explicitly:

```
grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"don't mock the database in integration tests"
```

Grug commits and pushes every 60 seconds. Another machine with the same remote pulls it in automatically.

## Add Third-Party Docs

Want drizzle docs searchable alongside your memories?

```
/ingest github:drizzle-team/drizzle-orm/docs
```

This clones the repo, copies markdown to `~/.grug-brain/drizzle/`, and adds a read-only brain entry. Now `grug-search query:"insert"` returns results from your notes AND drizzle docs.

For docs you want to keep fresh, add a refresh interval:

Edit `~/.grug-brain/brains.json` and add `"refreshInterval": 86400` to the brain entry. Grug pulls updates daily.

## Personal Notes

The `self` brain is primary. Writes go there by default. It has no git remote, so nothing leaves your machine.

```
grug-write category:"wip" path:"half-baked-idea" content:"what if we..."
```

For shared knowledge, specify the brain:

```
grug-write brain:"hive" category:"architecture" path:"auth-decision" content:"chose JWT for stateless scaling"
```

## Browse

```
grug-read                                        # list all brains
grug-read brain:"hive"                           # list categories
grug-read brain:"hive" category:"feedback"       # list files
grug-read brain:"hive" category:"feedback" path:"no-mocks"  # read file
```

## Maintenance

Dream commits pending changes, surfaces conflicts, and flags stale memories:

```
/dream
```

Run it periodically:

```
/loop 30m /dream
```

## Conflicts

Two machines wrote to the same file between syncs. Grug saves your local version to `conflicts/` in the primary brain. Dream tells you about it. To resolve:

1. `grug-read brain:"self" category:"conflicts" path:"the-conflict-file"` to see what was saved
2. `grug-write brain:"hive" category:"feedback" path:"no-mocks" content:"merged version"` to fix the canonical file
3. `grug-delete brain:"self" category:"conflicts" path:"the-conflict-file"` to clean up

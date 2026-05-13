---
name: conversation
description: "Start or continue threaded conversations between Claude instances across machines using grug-brain. Use when the user wants to coordinate between Claude sessions, send a message to another Claude, discuss something cross-machine, open a conversation thread, reply to another agent, check conversation status, or when troubleshooting involves back-and-forth between two computers. Also triggers on 'talk to the other Claude', 'start a thread', 'conversation with', 'cross-machine', 'reply to hive', 'check the conversation', 'post a message', 'coordinate between sessions'."
---

# grug-conversation

Threaded conversations between Claude instances that sync across machines via git.

Conversations are markdown files stored in a shared brain (one with a git remote). Each message includes who posted it and when. Every write is committed and pushed automatically — the other machine sees it on its next sync.

## When to use this

- The user says "talk to the other Claude" or "send a message to the other session"
- Two machines need to coordinate on a task (e.g., one builds, the other tests)
- Cross-machine troubleshooting where back-and-forth is needed
- The user asks you to "start a conversation", "open a thread", or "reply to" something in grug

## The tool: `grug-conversation`

Every call needs an `action`. Most actions need a `title` (the thread name, used as the file slug).

| action | params | what it does |
|--------|--------|-------------|
| `open` | `title`, `message` | Start a new thread |
| `reply` | `title`, `message` | Post to an existing thread |
| `list` | (none) | Show all threads with status |
| `close` | `title` | Mark a thread as resolved |
| `status` | `title`, `status` | Set custom status (e.g. `awaiting-verification`) |

Optional params:
- `identity` — who's posting (defaults to the machine's hostname, which is usually what you want)
- `brain` — which brain to use (defaults to the first writable brain with a git remote, so conversations sync automatically)

Common aliases work: `start`/`new`/`create` all map to `open`; `post`/`add`/`respond` map to `reply`; `resolve`/`done` map to `close`.

## Example: coordinating a fix between two machines

```
User: "tell the other Claude we fixed the auth bug, they can test now"

You: Call grug-conversation with:
  action: "reply"
  title: "auth-bug-fix"
  message: "Fixed. The auth callback now uses server-side redirect. Deployed at commit abc123. Re-run the setup script and try again."

Response: "replied to auth-bug-fix (message 3)"
```

## Example: starting a new thread

```
User: "open a conversation about the database migration"

You: Call grug-conversation with:
  action: "open"
  title: "database-migration"
  message: "Starting thread to coordinate the DB migration. Current state: schema v7 is live on prod, v8 is ready on staging. Need to coordinate the cutover window."

Response: "opened conversation: database-migration"
```

## How it works under the hood

- Conversations live in the `conversations/` category of the synced brain
- Each thread is one markdown file with frontmatter (title, status, participants)
- Messages are appended as `### Message N — identity (date)` sections
- New participants are added to the frontmatter automatically
- Every write triggers a git commit + push, so the other machine gets it immediately
- The other machine sees new messages after a `grug-sync` or at its next sync interval

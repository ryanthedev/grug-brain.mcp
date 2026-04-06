# grug-brain

Rust MCP server for persistent memory across Claude Code sessions.

## Dev workflow

After making code changes, rebuild and restart the running service:

```bash
./scripts/dev-reload.sh
```

This builds `cargo build --release`, restarts the launchd service, and verifies the socket. The installed binary at `~/.grug-brain/bin/grug` is a symlink to `target/release/grug`, so the build is picked up immediately on restart.

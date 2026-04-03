# Pseudocode: Phase 5 - Plugin + Brew Formula + Setup

## DW Verification

| DW-ID | Done-When Item | Status | Pseudocode Section |
|-------|---------------|--------|-------------------|
| DW-5.1 | Homebrew formula builds the Rust binary and installs to PATH | COVERED | Formula/grug-brain.rb |
| DW-5.2 | plugin.json mcpServers entry points to `grug --stdio` (no bun dependency) | COVERED | .claude-plugin/plugin.json |
| DW-5.3 | `grug serve --install-service` creates correct plist/unit file AND service is enabled | COVERED | src/main.rs + install_service module |
| DW-5.4 | setup.md rewritten for: brew install, service installation, MCP registration, brain config | COVERED | commands/setup.md |
| DW-5.5 | README updated with new install flow, architecture diagram, tool reference | COVERED | README.md |
| DW-5.6 | In a clean CI environment, full install chain returns valid MCP response | COVERED | scripts/smoke-test.sh |

**All items COVERED:** YES

## Files to Create/Modify

### New files
- `Formula/grug-brain.rb` — Homebrew formula
- `scripts/smoke-test.sh` — CI smoke test documenting the install chain

### Modified files
- `src/main.rs` — add `--install-service` flag to Serve command
- `src/lib.rs` — add `pub mod service_install;`
- `src/service_install.rs` — new module for plist/systemd generation and loading
- `.claude-plugin/plugin.json` — point to `grug --stdio`
- `commands/setup.md` — full rewrite
- `README.md` — full rewrite

## Pseudocode

### src/service_install.rs [DW-5.3]

```
module service_install

const LABEL = "com.grug-brain.server"

/// Returns the path where the service file should be written.
/// macOS: ~/Library/LaunchAgents/com.grug-brain.server.plist
/// Linux: ~/.config/systemd/user/grug-brain.service
fn service_file_path() -> Result<PathBuf, String>:
    if cfg!(target_os = "macos"):
        home = env::var("HOME")
        return home / "Library/LaunchAgents/com.grug-brain.server.plist"
    elif cfg!(target_os = "linux"):
        home = env::var("HOME")
        return home / ".config/systemd/user/grug-brain.service"
    else:
        return Err("unsupported platform")

/// Find the grug binary path (current_exe or "grug" if in PATH).
fn grug_binary_path() -> Result<PathBuf, String>:
    // First try current_exe (works when running from cargo build or brew install)
    if let Ok(exe) = std::env::current_exe():
        if exe.exists():
            return Ok(exe)
    // Fallback: assume "grug" is in PATH
    return Ok(PathBuf::from("grug"))

/// Generate macOS launchd plist XML content.
fn generate_plist(binary_path: &Path, socket_path: Option<&Path>) -> String:
    let grug_home = expand_home("~/.grug-brain")
    let args = if socket_path:
        [binary_path, "serve", "--socket", socket_path]
    else:
        [binary_path, "serve"]
    
    return XML plist with:
        Label: LABEL
        ProgramArguments: args array
        KeepAlive: true
        RunAtLoad: true
        StandardOutPath: grug_home / "launchd-stdout.log"
        StandardErrorPath: grug_home / "launchd-stderr.log"
        EnvironmentVariables: { HOME: env HOME }

/// Generate Linux systemd unit file content.
fn generate_systemd_unit(binary_path: &Path, socket_path: Option<&Path>) -> String:
    let exec_start = if socket_path:
        format!("{} serve --socket {}", binary_path, socket_path)
    else:
        format!("{} serve", binary_path)
    
    return unit file with:
        [Unit]
        Description=grug-brain memory server
        After=network.target

        [Service]
        Type=simple
        ExecStart=exec_start
        Restart=always
        RestartSec=5
        Environment=HOME={HOME}

        [Install]
        WantedBy=default.target

/// Install and enable the service.
fn install_service(socket_path: Option<&Path>) -> Result<(), String>:
    binary = grug_binary_path()?
    service_path = service_file_path()?
    
    // Ensure parent directory exists
    create_dir_all(service_path.parent())
    
    if cfg!(target_os = "macos"):
        content = generate_plist(&binary, socket_path)
        
        // Unload existing service (ignore errors — may not be loaded)
        run_command("launchctl", ["bootout", "gui/{uid}", &service_path])
            .ok()  // ignore error
        
        // Write plist file
        write(service_path, content)?
        
        // Load the new service
        run_command("launchctl", ["bootstrap", "gui/{uid}", &service_path])?
        
        // Verify: launchctl list | grep grug
        verify = run_command("launchctl", ["list"])?
        if output contains LABEL:
            println!("grug: service installed and running")
        else:
            return Err("service installed but not listed — check logs")
        
    elif cfg!(target_os = "linux"):
        content = generate_systemd_unit(&binary, socket_path)
        
        // Write unit file
        write(service_path, content)?
        
        // Reload, enable, start
        run_command("systemctl", ["--user", "daemon-reload"])?
        run_command("systemctl", ["--user", "enable", "grug-brain.service"])?
        run_command("systemctl", ["--user", "restart", "grug-brain.service"])?
        
        // Enable linger (survive logout)
        run_command("loginctl", ["enable-linger", &whoami()]).ok()
        
        // Verify
        status = run_command("systemctl", ["--user", "is-enabled", "grug-brain.service"])?
        if status.trim() == "enabled":
            println!("grug: service installed and enabled")
        else:
            return Err("service installed but not enabled")

// Tests:
// - test_generate_plist: produces valid XML with correct label, binary path, KeepAlive, RunAtLoad
// - test_generate_plist_custom_socket: socket path appears in ProgramArguments
// - test_generate_systemd_unit: produces valid unit with correct ExecStart
// - test_generate_systemd_unit_custom_socket: --socket flag in ExecStart
// - test_service_file_path_macos: correct path under ~/Library/LaunchAgents (cfg test)
// - test_service_file_path_linux: correct path under ~/.config/systemd/user (cfg test)
```

### src/main.rs [DW-5.3]

```
// Add --install-service flag to Serve variant

enum Commands {
    Serve {
        socket: Option<PathBuf>,
        install_service: bool,  // NEW: --install-service flag
    },
}

fn main():
    let cli = Cli::parse()
    
    if cli.stdio:
        run_stdio(None).await
        return
    
    match cli.command:
        Some(Commands::Serve { socket, install_service }):
            if install_service:
                // Install service and exit — do NOT start server
                service_install::install_service(socket.as_deref())?
                return
            // Normal serve path (unchanged)
            run_server(socket, None, None).await
        None:
            // Print usage (unchanged)
```

### src/lib.rs [DW-5.3]

```
// Add: pub mod service_install;
```

### Formula/grug-brain.rb [DW-5.1]

```ruby
class GrugBrain < Formula
  desc "Persistent memory for LLMs — FTS5 search, git sync, markdown storage"
  homepage "https://github.com/rtd/grug-brain.mcp"
  url "https://github.com/rtd/grug-brain.mcp/archive/refs/tags/v{VERSION}.tar.gz"
  sha256 "PLACEHOLDER"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    # Binary name is "grug-brain" from Cargo.toml — rename to "grug"
    # Actually check what cargo produces, might need bin rename
  end

  def caveats
    <<~EOS
      To start grug-brain as a background service:
        grug serve --install-service

      To register with Claude Code:
        claude plugin add grug-brain
        /setup

      Configuration: ~/.grug-brain/brains.json
    EOS
  end

  test do
    assert_match "grug", shell_output("#{bin}/grug --help")
  end
end
```

NOTE: The binary name from `cargo install` defaults to the package name "grug-brain".
Need to add `[[bin]] name = "grug"` to Cargo.toml OR rename in formula.
Check: the current binary is already named "grug" via `cargo build` target name.
Verify by checking target/debug/ for the binary name.

### .claude-plugin/plugin.json [DW-5.2]

```json
{
  "name": "grug-brain",
  "version": "4.0.0",
  "description": "Persistent memory for LLMs with search, multi-source docs, and dreaming. SQLite FTS5, markdown storage, git-tracked history.",
  "author": { "name": "r" },
  "license": "MIT",
  "keywords": ["memory", "search", "fts5", "knowledge-base", "mcp", "dreaming"],
  "mcpServers": {
    "grug-brain": {
      "command": "grug",
      "args": ["--stdio"]
    }
  }
}
```

No bun reference. Binary "grug" must be in PATH (installed via brew).

### commands/setup.md [DW-5.4]

```markdown
# Setup command structure:

## 0. Update check
- Connect to grug.sock to see if server is already running
- If running: note "update" mode, continue to verify all steps

## 1. Binary check
- `grug --version` must work
- If not found, tell user: `brew install rtd/grug/grug-brain`

## 2. Service installation
- Run `grug serve --install-service`
- This creates and loads the launchd plist (macOS) or systemd unit (Linux)
- Verify service is running:
  macOS: `launchctl list | grep grug`
  Linux: `systemctl --user is-active grug-brain.service`

## 3. Server health
- Wait 2 seconds for server to start
- Test socket connectivity: connect to ~/.grug-brain/grug.sock
  Simple test: `echo '{"id":"test","tool":"grug-search","params":{"query":"test"}}' | socat - UNIX-CONNECT:$HOME/.grug-brain/grug.sock`
  Or just check socket file exists and service is listed

## 4. MCP registration
- `claude mcp list 2>/dev/null | grep -i grug`
- If registered with old bun reference: remove and re-add
- If not registered: plugin.json handles it (plugin add)
- If already correct: no action

## 5. Brain configuration
- Check ~/.grug-brain/brains.json
- If missing: interactive setup (self brain, optional hive brain)
- If exists: show current config, ask about additions

## 6. Git setup
- For each brain with git remote: check git init, pull

## 7. Summary
- Binary version
- Service: installed + running / failed
- MCP: registered via plugin
- Brains: list with counts
- Service management commands
```

### README.md [DW-5.5]

```markdown
# grug-brain

Persistent memory for LLMs. [one-line description]

## Install

brew install rtd/grug/grug-brain
claude plugin add grug-brain
/setup

## Architecture

```
Claude Code <--stdio--> grug --stdio <--unix socket--> grug serve
                                                          |
                                                     SQLite FTS5
                                                     Git sync
                                                     File indexing
```

Two modes:
- `grug serve` — background server (brew service), owns database + git sync
- `grug --stdio` — thin MCP client, forwards tool calls to server over unix socket

## Brains
[same content as current — brain config table, examples]

## Tools
[same tool reference as current — all 9 tools with examples]

## Conflicts
[same conflict resolution docs]

## File Layout
[updated layout showing grug.sock]

## Service Management
macOS: launchctl commands
Linux: systemctl commands

## License
MIT
```

### scripts/smoke-test.sh [DW-5.6]

```bash
#!/usr/bin/env bash
# Smoke test for grug-brain install chain.
# Run in a clean environment (CI or fresh machine).
# Documents the expected install flow and validates each step.

set -euo pipefail

echo "=== grug-brain smoke test ==="

# Step 1: Verify binary is installed
echo "1. Checking binary..."
grug --version || { echo "FAIL: grug not in PATH"; exit 1; }

# Step 2: Ensure clean state
echo "2. Cleaning previous state..."
rm -f ~/.grug-brain/grug.sock ~/.grug-brain/grug.pid
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.server.plist 2>/dev/null || true

# Step 3: Create minimal brain config
echo "3. Creating test brain config..."
mkdir -p ~/.grug-brain/self
cat > ~/.grug-brain/brains.json << 'BRAINS'
[{"name":"self","dir":"~/.grug-brain/self","primary":true,"writable":true}]
BRAINS

# Step 4: Install service
echo "4. Installing service..."
grug serve --install-service

# Step 5: Wait for server
echo "5. Waiting for server..."
sleep 2

# Step 6: Test via stdio bridge
echo "6. Testing MCP tool call via stdio..."
RESPONSE=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"grug-recall","arguments":{}}}' | timeout 5 grug --stdio 2>/dev/null)
echo "Response: $RESPONSE"

# Check for valid MCP response structure
if echo "$RESPONSE" | grep -q '"result"'; then
    echo "PASS: Got valid MCP response"
else
    echo "FAIL: Invalid MCP response"
    exit 1
fi

echo "=== All checks passed ==="
```

## Design Notes

1. **Service label**: Using `com.grug-brain.server` (not `com.grug-brain.mcp`) to distinguish
   the Rust server from any lingering JS service entries.

2. **Binary name**: Need to verify Cargo produces `grug` not `grug-brain`. The `[[bin]]` section
   in Cargo.toml may need updating, or the formula handles the rename.

3. **install-service exits after install**: It does NOT start the server inline. The service
   manager (launchd/systemd) starts it. This is important — the process that writes the plist
   is not the process that runs as the service.

4. **Plugin version bump**: Going to 4.0.0 since this is a complete architecture change (Bun -> Rust).

5. **Homebrew tap naming**: `rtd/grug` tap means the formula lives at
   `github.com/rtd/homebrew-grug/Formula/grug-brain.rb`. The file in this repo is a copy/template.

6. **Smoke test is documentation**: The script documents the expected flow. Running it requires
   a built binary. In CI, this would run after `cargo build --release`.

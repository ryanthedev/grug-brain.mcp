/**
 * Shared Playwright fixtures for grug-brain tests.
 *
 * `grugServer` fixture:
 *   - Creates a temp directory with one brain ("testbrain") pre-seeded with
 *     test memories.
 *   - Builds the grug binary via `cargo build` if it doesn't exist.
 *   - Spawns `grug serve` as a child process with the temp brain config.
 *   - Waits for the serve.port file to appear (max 10s).
 *   - Yields {baseUrl, brainDir, portFile, port} to the test.
 *   - Kills the process on teardown.
 *
 * Usage:
 *   const { test, expect } = require("./fixtures");
 *   test("my test", async ({ page, grugServer }) => { ... });
 */

const { test: base, expect } = require("@playwright/test");
const { spawn, execFileSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

// Path to the repo root (two levels up from tests/playwright/).
const REPO_ROOT = path.resolve(__dirname, "../..");
// Debug binary path (faster to build; use release for golden-read test).
const GRUG_BIN = path.join(REPO_ROOT, "target/debug/grug");

/** Wait up to `maxMs` for `predicate()` to return a truthy value. */
async function waitFor(predicate, maxMs = 10000, intervalMs = 100) {
  const deadline = Date.now() + maxMs;
  while (Date.now() < deadline) {
    const result = predicate();
    if (result) return result;
    await new Promise(r => setTimeout(r, intervalMs));
  }
  throw new Error(`waitFor timed out after ${maxMs}ms`);
}

/**
 * Build the grug binary if it doesn't already exist.
 * Uses execFileSync with a static argument array — no shell interpolation.
 */
function ensureBinary() {
  if (fs.existsSync(GRUG_BIN)) return;
  console.log("Building grug binary (cargo build)...");
  // execFileSync with array args avoids shell injection (args are static here).
  execFileSync("cargo", ["build"], { cwd: REPO_ROOT, stdio: "inherit" });
}

/**
 * Create a temp directory with a seeded testbrain and config files.
 * Returns {tmp, brainDir, brainsJson, portFile}.
 */
function createTestBrain() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "grug-playwright-"));
  const brainDir = path.join(tmp, "testbrain");
  const notesDir = path.join(brainDir, "notes");
  const tagsDir = path.join(brainDir, "tags");
  fs.mkdirSync(notesDir, { recursive: true });
  fs.mkdirSync(tagsDir, { recursive: true });

  // Seed memories for tests.
  fs.writeFileSync(
    path.join(notesDir, "hello.md"),
    "---\nname: Hello World\ndate: 2025-01-01\ndescription: A greeting memory\n---\n\n# Hello World\n\nThis is the **hello** memory with some markdown.\n\n- item one\n- item two\n"
  );
  fs.writeFileSync(
    path.join(notesDir, "script-test.md"),
    "---\nname: Script Test\ndate: 2025-01-02\ndescription: Tests XSS sanitization\n---\n\n<script>alert(1)</script>\n\nThis tests script tag sanitization.\n"
  );
  fs.writeFileSync(
    path.join(tagsDir, "tagged.md"),
    "---\nname: Tagged Memory\ndate: 2025-01-03\ndescription: Has tags\n---\n\n#testing #memory\n\nA tagged memory.\n"
  );

  // Write brains.json with static paths (no user input).
  const brainsJson = path.join(tmp, "brains.json");
  fs.writeFileSync(brainsJson, JSON.stringify([
    { name: "testbrain", dir: brainDir, primary: true, writable: true }
  ]));

  const portFile = path.join(tmp, "serve.port");
  return { tmp, brainDir, brainsJson, portFile };
}

/** The base test extended with a `grugServer` fixture. */
const test = base.extend({
  // eslint-disable-next-line no-empty-pattern
  grugServer: [async ({}, use) => {
    ensureBinary();
    const { tmp, brainDir, brainsJson, portFile } = createTestBrain();
    const dbPath = path.join(tmp, "grug.db");
    const sockPath = path.join(tmp, "grug.sock");

    // Spawn grug serve with all-static arguments and environment.
    // No user-controlled values are interpolated into command args.
    const env = {
      ...process.env,
      GRUG_CONFIG: brainsJson,  // overrides ~/.grug-brain/brains.json
      GRUG_PORT: "0",           // ephemeral port avoids conflicts
      GRUG_PORT_FILE: portFile,
      GRUG_DB: dbPath,
      GRUG_SOCKET: sockPath,
      RUST_LOG: "warn",
    };
    // Pass socket path via CLI arg (the binary supports --socket).
    // DB path and config are set via env vars (GRUG_CONFIG, GRUG_DB).
    const proc = spawn(GRUG_BIN, ["serve", "--socket", sockPath], { env, cwd: REPO_ROOT });
    proc.stderr.on("data", d => {
      if (process.env.DEBUG_GRUG) process.stderr.write(d);
    });
    proc.stdout.on("data", d => {
      if (process.env.DEBUG_GRUG) process.stdout.write(d);
    });

    // Wait for the port file — written by grug after the HTTP server binds.
    const port = await waitFor(() => {
      try {
        const s = fs.readFileSync(portFile, "utf8").trim();
        const p = parseInt(s, 10);
        return p > 0 ? p : null;
      } catch (_) { return null; }
    }, 10000);

    const baseUrl = `http://127.0.0.1:${port}`;

    // Provide the fixture value to the test, then run teardown after it completes.
    await use({ baseUrl, brainDir, portFile, port });

    // Teardown: kill server and clean up temp dir.
    proc.kill("SIGTERM");
    await new Promise(r => setTimeout(r, 500));
    try { fs.rmSync(tmp, { recursive: true, force: true }); } catch (_) {}
  }, { scope: "test" }],
});

module.exports = { test, expect, waitFor, createTestBrain, ensureBinary, GRUG_BIN, REPO_ROOT };

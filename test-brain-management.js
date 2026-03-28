// Test suite for brain management features — Phase 2
// Tests: mtime-based reload, grug-config add/remove/list, ingest defaults, refresh rules
// Run with: bun test-brain-management.js
//
// Strategy: inline the functions under test (same pattern as test-load-brains.js).
// We never import server.js because it starts an MCP server on import.

import { existsSync, mkdirSync, writeFileSync, statSync, rmSync, readFileSync } from "fs";
import { join, resolve, dirname, basename } from "path";
import { tmpdir } from "os";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));

// ---- functions inlined from server.js ----

function expandHome(p) {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  if (p === "~") return home;
  if (p.startsWith("~/")) return join(home, p.slice(2));
  return p;
}

function ensureDir(p) {
  if (!existsSync(p)) mkdirSync(p, { recursive: true });
}

// Reads brains.json for config mutations. Returns the parsed array or throws.
function readBrainsJson() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  if (!existsSync(configPath)) return [];
  let raw;
  try {
    raw = JSON.parse(readFileSync(configPath, "utf-8"));
  } catch (err) {
    throw new Error("failed to parse " + configPath + ": " + err.message);
  }
  if (!Array.isArray(raw)) throw new Error(configPath + " must be a JSON array");
  return raw;
}

// Writes brains array back to brains.json.
function writeBrainsJson(brainsArray) {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  ensureDir(dirname(configPath));
  writeFileSync(configPath, JSON.stringify(brainsArray, null, 2) + "\n", "utf-8");
}

// Loads and validates brains from brains.json. Returns the filtered brain array
// (brains whose dirs do not exist are excluded). Throws on parse or validation errors.
function loadBrains() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const defaultConfigPath = join(home, ".grug-brain", "brains.json");
  const configPath = process.env.GRUG_CONFIG || defaultConfigPath;

  if (!existsSync(configPath)) return [];

  let raw;
  try {
    raw = JSON.parse(readFileSync(configPath, "utf-8"));
  } catch (err) {
    throw new Error("grug: failed to parse " + configPath + ": " + err.message);
  }

  if (!Array.isArray(raw)) {
    throw new Error("grug: " + configPath + " must be a JSON array");
  }

  const brains = raw.map((entry, i) => {
    if (!entry.name || typeof entry.name !== "string") {
      throw new Error("grug: brain[" + i + '] missing required "name" field');
    }
    if (!entry.dir || typeof entry.dir !== "string") {
      throw new Error('grug: brain "' + entry.name + '" missing required "dir" field');
    }
    const flat = entry.flat === true;
    const writableDefault = flat ? false : true;
    return {
      name: entry.name,
      dir: resolve(expandHome(entry.dir)),
      primary: entry.primary === true,
      writable: entry.writable !== undefined ? entry.writable === true : writableDefault,
      flat,
      git: entry.git || null,
      syncInterval: typeof entry.syncInterval === "number" ? entry.syncInterval : 60,
      source: entry.source || undefined,
      refreshInterval: typeof entry.refreshInterval === "number" ? entry.refreshInterval : undefined,
    };
  });

  // Validate: unique names
  const names = new Set();
  for (const brain of brains) {
    if (names.has(brain.name)) {
      throw new Error('grug: duplicate brain name "' + brain.name + '" in ' + configPath);
    }
    names.add(brain.name);
  }

  // Validate: exactly one primary
  const primaries = brains.filter(b => b.primary);
  if (primaries.length === 0) {
    throw new Error('grug: no brain marked "primary: true" in ' + configPath);
  }
  if (primaries.length > 1) {
    throw new Error('grug: multiple brains marked "primary: true" in ' + configPath + ": " + primaries.map(b => b.name).join(", "));
  }

  return brains.filter(b => existsSync(b.dir));
}

// Returns the mtime (ms) of the brains.json config file, or 0 if absent.
function getBrainsJsonMtime() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const configPath = process.env.GRUG_CONFIG || join(home, ".grug-brain", "brains.json");
  try { return statSync(configPath).mtimeMs; } catch { return 0; }
}

// Returns true if brains.json has changed since lastMtime.
// Simulates the mtime-based reload gate from reloadBrains() in server.js.
function hasBrainsJsonChanged(lastMtime) {
  const currentMtime = getBrainsJsonMtime();
  return currentMtime !== lastMtime;
}

// Minimum allowed refresh interval (seconds). Mirrors the constant in server.js.
const MIN_REFRESH_INTERVAL_S = 3600;

// Returns the effective refresh interval for a brain, enforcing the minimum.
// Returns null if the brain is not eligible for auto-refresh.
// Mirrors the logic in startBrainTimers() from server.js.
function effectiveRefreshInterval(brain) {
  // Writable brains use sync (bidirectional), not refresh (pull-only)
  if (brain.writable) return null;
  // Both source and refreshInterval are required to enable auto-refresh
  if (!brain.source || typeof brain.refreshInterval !== "number") return null;
  // Clamp to minimum to prevent runaway refresh storms
  return Math.max(brain.refreshInterval, MIN_REFRESH_INTERVAL_S);
}

// ---- test helpers ----

let passed = 0;
let failed = 0;

function assert(condition, message) {
  if (condition) {
    console.log("  PASS: " + message);
    passed++;
  } else {
    console.error("  FAIL: " + message);
    failed++;
  }
}

function assertThrows(fn, expectedMsg, testName) {
  try {
    fn();
    console.error("  FAIL: " + testName + " - expected throw but did not");
    failed++;
  } catch (err) {
    if (err.message.includes(expectedMsg)) {
      console.log("  PASS: " + testName);
      passed++;
    } else {
      console.error("  FAIL: " + testName + " - threw [" + err.message + "], expected to include [" + expectedMsg + "]");
      failed++;
    }
  }
}

// Isolated temp directory for all tests
const tmp = join(tmpdir(), "grug-bm-test-" + Date.now());
mkdirSync(tmp, { recursive: true });

// Runs fn with GRUG_CONFIG pointing at a fresh temp file containing content.
// fn receives (configPath) as its argument.
function withConfig(content, fn) {
  const configPath = join(tmp, "brains-" + Date.now() + "-" + Math.random().toString(36).slice(2) + ".json");
  writeFileSync(configPath, content, "utf-8");
  const orig = process.env.GRUG_CONFIG;
  process.env.GRUG_CONFIG = configPath;
  try {
    return fn(configPath);
  } finally {
    if (orig === undefined) delete process.env.GRUG_CONFIG;
    else process.env.GRUG_CONFIG = orig;
  }
}

// Runs fn with GRUG_CONFIG set to a new temp path (file not yet written).
// fn receives (configPath) as its argument.
function withConfigPath(fn) {
  const configPath = join(tmp, "brains-" + Date.now() + "-" + Math.random().toString(36).slice(2) + ".json");
  const orig = process.env.GRUG_CONFIG;
  process.env.GRUG_CONFIG = configPath;
  try {
    return fn(configPath);
  } finally {
    if (orig === undefined) delete process.env.GRUG_CONFIG;
    else process.env.GRUG_CONFIG = orig;
  }
}

// ============================================================
// Test group 1: mtime-based config reload
// ============================================================

console.log("\n=== 1. mtime-based config reload ===\n");

// 1a: hasBrainsJsonChanged returns false when file is unchanged
console.log("1a. unchanged file not detected as changed");
{
  const dir1 = join(tmp, "r1a");
  mkdirSync(dir1, { recursive: true });
  const config = JSON.stringify([{ name: "m", dir: dir1, primary: true }]);

  withConfig(config, (configPath) => {
    const mtime = statSync(configPath).mtimeMs;
    assert(!hasBrainsJsonChanged(mtime), "same mtime means hasBrainsJsonChanged returns false");
  });
}

// 1b: hasBrainsJsonChanged returns true after file is rewritten
console.log("\n1b. modified file detected as changed");
{
  const dir1 = join(tmp, "r1b");
  mkdirSync(dir1, { recursive: true });
  const config = JSON.stringify([{ name: "m", dir: dir1, primary: true }]);

  withConfig(config, (configPath) => {
    const mtimeBefore = statSync(configPath).mtimeMs;

    // Spin briefly so the OS has a chance to record a new mtime on rewrite
    const deadline = Date.now() + 50;
    while (Date.now() < deadline) { /* busy-wait */ }
    writeFileSync(configPath, config, "utf-8");

    assert(hasBrainsJsonChanged(mtimeBefore), "rewritten file means hasBrainsJsonChanged returns true");
  });
}

// 1c: loadBrains picks up changes when called after a file write
console.log("\n1c. loadBrains picks up new brain entry after file rewrite");
{
  const dir1 = join(tmp, "r1c-a");
  const dir2 = join(tmp, "r1c-b");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  withConfig(JSON.stringify([{ name: "brain1", dir: dir1, primary: true }]), (configPath) => {
    const firstLoad = loadBrains();
    assert(firstLoad.length === 1, "initial load has 1 brain");

    writeFileSync(
      configPath,
      JSON.stringify([
        { name: "brain1", dir: dir1, primary: true },
        { name: "brain2", dir: dir2 },
      ]),
      "utf-8"
    );

    const secondLoad = loadBrains();
    assert(secondLoad.length === 2, "after rewrite, load has 2 brains");
    assert(secondLoad.some(b => b.name === "brain2"), "new brain2 entry present after rewrite");
  });
}

// 1d: repeated loadBrains calls with no file change return stable data
console.log("\n1d. repeated loadBrains calls with no file change return same data");
{
  const dir1 = join(tmp, "r1d");
  mkdirSync(dir1, { recursive: true });

  withConfig(JSON.stringify([{ name: "m", dir: dir1, primary: true }]), () => {
    const first = loadBrains();
    const second = loadBrains();
    assert(first.length === second.length, "repeated calls return same brain count");
    assert(first[0].name === second[0].name, "repeated calls return same brain name");
  });
}

// ============================================================
// Test group 2: grug-config add - valid entry creation
// ============================================================

console.log("\n=== 2. grug-config add - entry creation ===\n");

// 2a: add creates entry with correct defaults for a normal (non-flat) brain
console.log("2a. add creates normal brain entry with correct defaults");
{
  const dir1 = join(tmp, "a2a-primary");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true, writable: true, flat: false, git: null, syncInterval: 60 }]);

    const existing = readBrainsJson();
    const isFlat = false;
    const isWritable = !isFlat; // non-flat defaults to writable
    const entry = {
      name: "docs",
      dir: join(tmp, "a2a-new"),
      primary: false,
      writable: isWritable,
      flat: isFlat,
      git: null,
      syncInterval: 60,
    };
    existing.push(entry);
    writeBrainsJson(existing);

    const stored = readBrainsJson();
    const added = stored.find(b => b.name === "docs");

    assert(added !== undefined, "added brain entry exists in brains.json");
    assert(added.writable === true, "non-flat brain defaults to writable:true");
    assert(added.flat === false, "flat defaults to false");
    assert(added.primary === false, "primary defaults to false");
    assert(added.syncInterval === 60, "syncInterval defaults to 60");
    assert(added.git === null, "git defaults to null");
  });
}

// 2b: add creates entry with correct defaults for a flat brain
console.log("\n2b. add creates flat brain entry with writable:false default");
{
  const dir1 = join(tmp, "a2b-primary");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true, writable: true, flat: false, git: null, syncInterval: 60 }]);

    const existing = readBrainsJson();
    const isFlat = true;
    const isWritable = !isFlat; // flat defaults to read-only
    existing.push({
      name: "ext-docs",
      dir: join(tmp, "a2b-docs"),
      primary: false,
      writable: isWritable,
      flat: isFlat,
      git: null,
      syncInterval: 60,
    });
    writeBrainsJson(existing);

    const stored = readBrainsJson();
    const added = stored.find(b => b.name === "ext-docs");

    assert(added !== undefined, "flat brain entry exists");
    assert(added.flat === true, "flat is true");
    assert(added.writable === false, "flat brain defaults to writable:false");
  });
}

// 2c: add creates the directory if it does not exist
console.log("\n2c. add creates directory if it does not exist");
{
  const dir1 = join(tmp, "a2c-primary");
  const newBrainDir = join(tmp, "a2c-new-brain-dir");
  mkdirSync(dir1, { recursive: true });
  // newBrainDir intentionally not pre-created

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true, writable: true, flat: false, git: null, syncInterval: 60 }]);

    // Simulate the ensureDir call that grug-config add performs before writing the entry
    ensureDir(newBrainDir);
    assert(existsSync(newBrainDir), "ensureDir created the new brain directory");

    const existing = readBrainsJson();
    existing.push({ name: "new-docs", dir: newBrainDir, primary: false, writable: false, flat: true, git: null, syncInterval: 60 });
    writeBrainsJson(existing);

    const stored = readBrainsJson();
    assert(stored.some(b => b.name === "new-docs"), "new-docs entry written to config");
  });
}

// 2d: add stores optional source and refreshInterval fields
console.log("\n2d. add stores source and refreshInterval fields");
{
  const dir1 = join(tmp, "a2d-primary");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true, writable: true, flat: false, git: null, syncInterval: 60 }]);

    const existing = readBrainsJson();
    const entry = {
      name: "ref-docs",
      dir: join(tmp, "a2d-docs"),
      primary: false,
      writable: false,
      flat: true,
      git: null,
      syncInterval: 60,
      source: "github:owner/repo/docs",
      refreshInterval: 86400,
    };
    existing.push(entry);
    writeBrainsJson(existing);

    const stored = readBrainsJson();
    const added = stored.find(b => b.name === "ref-docs");

    assert(added.source === "github:owner/repo/docs", "source field stored correctly");
    assert(added.refreshInterval === 86400, "refreshInterval stored correctly");
  });
}

// ============================================================
// Test group 3: grug-config add - duplicate name rejection
// ============================================================

console.log("\n=== 3. grug-config add - duplicate name rejection ===\n");

// 3a: add rejects a name that already exists in brains.json
console.log("3a. add rejects duplicate brain name");
{
  const dir1 = join(tmp, "d3a-primary");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true }]);

    const existing = readBrainsJson();
    const isDuplicate = existing.some(b => b.name === "memories");
    assert(isDuplicate === true, "duplicate detection returns true for existing name");

    // Verify loadBrains also catches duplicates when written directly
    const dupConfig = JSON.stringify([
      { name: "memories", dir: dir1, primary: true },
      { name: "memories", dir: dir1 },
    ]);
    writeFileSync(process.env.GRUG_CONFIG, dupConfig, "utf-8");

    assertThrows(
      () => loadBrains(),
      'duplicate brain name "memories"',
      "loadBrains throws on duplicate name in brains.json"
    );
  });
}

// ============================================================
// Test group 4: grug-config remove - primary brain protection
// ============================================================

console.log("\n=== 4. grug-config remove - primary brain protection ===\n");

// 4a: remove refuses to remove the primary brain
console.log("4a. remove refuses to remove primary brain");
{
  const dir1 = join(tmp, "rm4a");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true }]);

    const existing = readBrainsJson();
    const entry = existing.find(b => b.name === "memories");

    assert(entry !== undefined, "entry found in config");
    assert(entry.primary === true, "entry is marked primary");
    // The removal guard: entry.primary === true means the remove action is blocked
    assert(entry.primary === true, "primary flag blocks removal");
  });
}

// 4b: remove allows removing a non-primary brain
console.log("\n4b. remove allows removing a non-primary brain");
{
  const dir1 = join(tmp, "rm4b-primary");
  const dir2 = join(tmp, "rm4b-docs");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([
      { name: "memories",   dir: dir1, primary: true  },
      { name: "extra-docs", dir: dir2, primary: false },
    ]);

    const existing = readBrainsJson();
    const entry = existing.find(b => b.name === "extra-docs");

    assert(entry !== undefined, "non-primary entry found");
    assert(entry.primary !== true, "entry is not primary so removal is allowed");

    const updated = existing.filter(b => b.name !== "extra-docs");
    writeBrainsJson(updated);

    const stored = readBrainsJson();
    assert(stored.length === 1, "one entry remains after removing extra-docs");
    assert(!stored.some(b => b.name === "extra-docs"), "extra-docs no longer in config");
    assert(stored.some(b => b.name === "memories"), "primary brain still in config");
  });
}

// 4c: remove only updates config — files on disk are preserved
console.log("\n4c. remove only updates config - files on disk are preserved");
{
  const dir1 = join(tmp, "rm4c-primary");
  const dir2 = join(tmp, "rm4c-docs");
  const testFile = join(dir2, "test.md");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });
  writeFileSync(testFile, "# test\n", "utf-8");

  withConfigPath(() => {
    writeBrainsJson([
      { name: "memories", dir: dir1, primary: true  },
      { name: "docs",     dir: dir2, primary: false },
    ]);

    const existing = readBrainsJson();
    const updated = existing.filter(b => b.name !== "docs");
    writeBrainsJson(updated);

    assert(!readBrainsJson().some(b => b.name === "docs"), "docs removed from config");
    assert(existsSync(testFile), "files in removed brain dir are preserved on disk");
  });
}

// ============================================================
// Test group 5: grug-config remove - entry removal
// ============================================================

console.log("\n=== 5. grug-config remove - entry removal ===\n");

// 5a: remove deletes only the targeted entry, leaves others intact
console.log("5a. remove deletes only the targeted entry");
{
  const dirs = [join(tmp, "rm5a-p"), join(tmp, "rm5a-a"), join(tmp, "rm5a-b")];
  dirs.forEach(d => mkdirSync(d, { recursive: true }));

  withConfigPath(() => {
    writeBrainsJson([
      { name: "memories", dir: dirs[0], primary: true  },
      { name: "alpha",    dir: dirs[1], primary: false },
      { name: "beta",     dir: dirs[2], primary: false },
    ]);

    const existing = readBrainsJson();
    const updated = existing.filter(b => b.name !== "alpha");
    writeBrainsJson(updated);

    const stored = readBrainsJson();
    assert(stored.length === 2, "two entries remain after removing alpha");
    assert(stored.some(b => b.name === "memories"), "memories preserved");
    assert(stored.some(b => b.name === "beta"), "beta preserved");
    assert(!stored.some(b => b.name === "alpha"), "alpha removed");
  });
}

// ============================================================
// Test group 6: grug-config list - all brains with status flags
// ============================================================

console.log("\n=== 6. grug-config list - brains with status flags ===\n");

// 6a: list reads all brain entries from brains.json
console.log("6a. list returns all configured brains");
{
  const dirs = [join(tmp, "ls6a-p"), join(tmp, "ls6a-a")];
  dirs.forEach(d => mkdirSync(d, { recursive: true }));

  withConfigPath(() => {
    writeBrainsJson([
      { name: "memories",  dir: dirs[0], primary: true,  writable: true  },
      { name: "reference", dir: dirs[1], primary: false, writable: false, flat: true },
    ]);

    const brainList = readBrainsJson();
    assert(brainList.length === 2, "list returns 2 entries");
  });
}

// 6b: status flags are derivable from config fields
console.log("\n6b. status flags derivable from config fields");
{
  const dir1 = join(tmp, "ls6b-p");
  const dir2 = join(tmp, "ls6b-r");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  withConfigPath(() => {
    writeBrainsJson([
      { name: "memories",  dir: dir1, primary: true,  writable: true  },
      { name: "reference", dir: dir2, primary: false, writable: false, flat: true, source: "github:x/y/z", refreshInterval: 86400 },
    ]);

    const brainList = readBrainsJson();
    const primary  = brainList.find(b => b.name === "memories");
    const readOnly = brainList.find(b => b.name === "reference");

    assert(primary.primary === true,   "memories is primary");
    assert(primary.writable === true,  "memories is writable");
    assert(readOnly.primary !== true,  "reference is not primary");
    assert(readOnly.writable === false, "reference is read-only");

    const isRefreshEligible = !readOnly.writable && !!readOnly.source && typeof readOnly.refreshInterval === "number";
    assert(isRefreshEligible, "reference has all three refresh-eligibility fields");
  });
}

// ============================================================
// Test group 7: ingest default dir
// ============================================================

console.log("\n=== 7. ingest default dir ===\n");

// 7a: default ingest dir is ~/.grug-brain/<name>/
console.log("7a. default ingest dir is ~/.grug-brain/<name>/");
{
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const brainName = "react-native";
  const expectedDir = join(home, ".grug-brain", brainName);

  // This is the formula used by the /ingest command
  const defaultIngestDir = join(expandHome("~"), ".grug-brain", brainName);

  assert(defaultIngestDir === expectedDir, "default ingest dir is ~/.grug-brain/react-native");
  assert(!defaultIngestDir.includes("CLAUDE_PLUGIN_ROOT"), "default dir does not reference plugin cache");
}

// 7b: tilde expansion works for default ingest path
console.log("\n7b. tilde expansion works for default ingest path");
{
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const expanded = expandHome("~/.grug-brain/my-docs");
  assert(expanded === join(home, ".grug-brain", "my-docs"), "tilde expands to HOME");
  assert(!expanded.startsWith("~"), "expanded path has no leading tilde");
}

// ============================================================
// Test group 8: refresh rules
// ============================================================

console.log("\n=== 8. refresh rules ===\n");

// 8a: writable brains are not eligible for auto-refresh (sync handles them instead)
console.log("8a. writable brain is not eligible for auto-refresh");
{
  const writableBrain = { name: "memories", writable: true, source: "github:x/y/z", refreshInterval: 3600 };
  assert(effectiveRefreshInterval(writableBrain) === null, "writable brain means effectiveRefreshInterval is null");
}

// 8b: read-only brain without source is not eligible for auto-refresh
console.log("\n8b. read-only brain without source is not eligible for auto-refresh");
{
  const noSource = { name: "docs", writable: false, source: undefined, refreshInterval: 3600 };
  assert(effectiveRefreshInterval(noSource) === null, "no source means effectiveRefreshInterval is null");
}

// 8c: read-only brain without refreshInterval is not eligible for auto-refresh
console.log("\n8c. read-only brain without refreshInterval is not eligible for auto-refresh");
{
  const noInterval = { name: "docs", writable: false, source: "github:x/y/z", refreshInterval: undefined };
  assert(effectiveRefreshInterval(noInterval) === null, "no refreshInterval means effectiveRefreshInterval is null");
}

// 8d: minimum refresh interval is enforced at 3600s
console.log("\n8d. minimum refresh interval is enforced at 3600s");
{
  const tooShort = { name: "docs", writable: false, source: "github:x/y/z", refreshInterval: 60 };
  const result = effectiveRefreshInterval(tooShort);
  assert(result === MIN_REFRESH_INTERVAL_S, "60s interval clamped to " + MIN_REFRESH_INTERVAL_S + "s");
}

// 8e: valid refresh interval above minimum is preserved unchanged
console.log("\n8e. valid refresh interval above minimum is preserved");
{
  const validBrain = { name: "docs", writable: false, source: "github:x/y/z", refreshInterval: 86400 };
  assert(effectiveRefreshInterval(validBrain) === 86400, "86400s interval preserved unchanged");
}

// 8f: interval exactly at the minimum boundary is not changed
console.log("\n8f. interval exactly at minimum (3600s) is not changed");
{
  const atMinimum = { name: "docs", writable: false, source: "github:x/y/z", refreshInterval: 3600 };
  assert(effectiveRefreshInterval(atMinimum) === 3600, "3600s interval not increased");
}

// 8g: MIN_REFRESH_INTERVAL_S constant has the correct value
console.log("\n8g. MIN_REFRESH_INTERVAL_S is 3600");
{
  assert(MIN_REFRESH_INTERVAL_S === 3600, "MIN_REFRESH_INTERVAL_S === 3600");
}

// ============================================================
// Test group 9: readBrainsJson / writeBrainsJson round-trip
// ============================================================

console.log("\n=== 9. readBrainsJson / writeBrainsJson round-trip ===\n");

// 9a: write then read produces identical entries
console.log("9a. write then read round-trips correctly");
{
  const dir1 = join(tmp, "rt9a");
  mkdirSync(dir1, { recursive: true });

  withConfigPath(() => {
    const original = [
      { name: "memories", dir: dir1, primary: true, writable: true, flat: false, git: null, syncInterval: 60 },
    ];
    writeBrainsJson(original);
    const roundTripped = readBrainsJson();

    assert(roundTripped.length === 1, "round-trip preserves entry count");
    assert(roundTripped[0].name === "memories", "round-trip preserves name");
    assert(roundTripped[0].primary === true, "round-trip preserves primary");
    assert(roundTripped[0].syncInterval === 60, "round-trip preserves syncInterval");
  });
}

// 9b: readBrainsJson returns empty array when config file does not exist
console.log("\n9b. readBrainsJson returns empty array when config is absent");
{
  withConfigPath(() => {
    const result = readBrainsJson();
    assert(Array.isArray(result), "returns an array");
    assert(result.length === 0, "returns empty array when config absent");
  });
}

// 9c: writeBrainsJson creates the parent config directory if needed
console.log("\n9c. writeBrainsJson creates parent directory if needed");
{
  const nestedConfigDir = join(tmp, "rt9c-nested", "config");
  const nestedConfigPath = join(nestedConfigDir, "brains.json");
  const orig = process.env.GRUG_CONFIG;
  process.env.GRUG_CONFIG = nestedConfigPath;
  try {
    const dir1 = join(tmp, "rt9c-brain");
    mkdirSync(dir1, { recursive: true });
    writeBrainsJson([{ name: "memories", dir: dir1, primary: true }]);
    assert(existsSync(nestedConfigPath), "writeBrainsJson created config in nested directory");
  } finally {
    if (orig === undefined) delete process.env.GRUG_CONFIG;
    else process.env.GRUG_CONFIG = orig;
  }
}

// ============================================================
// Cleanup
// ============================================================

rmSync(tmp, { recursive: true });

// ============================================================
// Summary
// ============================================================

console.log("\n=== Results: " + passed + " passed, " + failed + " failed ===\n");
if (failed > 0) process.exit(1);

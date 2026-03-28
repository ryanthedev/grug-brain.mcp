// Test suite for loadBrains() — Phase 1
// Run with: bun test-load-brains.js

import { existsSync, mkdirSync, writeFileSync, rmSync } from "fs";
import { join, resolve } from "path";
import { tmpdir } from "os";

// ---- inline the functions under test ----
// (copied from server.js so we can test in isolation without starting the full server)

import { dirname, basename } from "path";
import { fileURLToPath } from "url";
import { readFileSync } from "fs";

const __dirname = dirname(fileURLToPath(import.meta.url));

function expandHome(p) {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  if (p === "~") return home;
  if (p.startsWith("~/")) return join(home, p.slice(2));
  return p;
}

function loadBrains() {
  const home = process.env.HOME || process.env.USERPROFILE || __dirname;
  const defaultConfigPath = join(home, ".grug-brain", "brains.json");
  const configPath = process.env.GRUG_CONFIG || defaultConfigPath;

  if (existsSync(configPath)) {
    let raw;
    try {
      raw = JSON.parse(readFileSync(configPath, "utf-8"));
    } catch (err) {
      throw new Error(`grug: failed to parse ${configPath}: ${err.message}`);
    }

    if (!Array.isArray(raw)) {
      throw new Error(`grug: ${configPath} must be a JSON array`);
    }

    const brains = raw.map((entry, i) => {
      if (!entry.name || typeof entry.name !== "string") {
        throw new Error(`grug: brain[${i}] missing required "name" field`);
      }
      if (!entry.dir || typeof entry.dir !== "string") {
        throw new Error(`grug: brain "${entry.name}" missing required "dir" field`);
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
      };
    });

    // Validate: unique names
    const names = new Set();
    for (const brain of brains) {
      if (names.has(brain.name)) {
        throw new Error(`grug: duplicate brain name "${brain.name}" in ${configPath}`);
      }
      names.add(brain.name);
    }

    // Validate: exactly one primary
    const primaries = brains.filter(b => b.primary);
    if (primaries.length === 0) {
      throw new Error(`grug: no brain marked "primary: true" in ${configPath}`);
    }
    if (primaries.length > 1) {
      throw new Error(`grug: multiple brains marked "primary: true" in ${configPath}: ${primaries.map(b => b.name).join(", ")}`);
    }

    // Filter out brains whose dirs don't exist
    return brains.filter(b => existsSync(b.dir));
  }

  // No config file — build from env vars (backwards compat for existing users)
  const brains = [];

  const memoryDir = resolve(expandHome(
    process.env.MEMORY_DIR || join(home, ".grug-brain", "memories")
  ));
  brains.push({
    name: "memories",
    dir: memoryDir,
    primary: true,
    writable: true,
    flat: false,
    git: null,
    syncInterval: 60,
  });

  const docsRaw = process.env.DOCS_DIRS || process.env.DOCS_DIR || "";
  if (docsRaw) {
    for (const entry of docsRaw.split(":")) {
      const eq = entry.indexOf("=");
      if (eq > 0) {
        const name = entry.slice(0, eq);
        const dir = resolve(expandHome(entry.slice(eq + 1)));
        brains.push({ name, dir, primary: false, writable: false, flat: true, git: null, syncInterval: 60 });
      } else {
        const dir = resolve(expandHome(entry));
        const name = basename(dir);
        brains.push({ name, dir, primary: false, writable: false, flat: false, git: null, syncInterval: 60 });
      }
    }
  }

  return brains.filter(b => existsSync(b.dir));
}

// ---- test helpers ----

let passed = 0;
let failed = 0;

function assert(condition, message) {
  if (condition) {
    console.log(`  PASS: ${message}`);
    passed++;
  } else {
    console.error(`  FAIL: ${message}`);
    failed++;
  }
}

function assertThrows(fn, expectedMsg, testName) {
  try {
    fn();
    console.error(`  FAIL: ${testName} — expected throw but didn't`);
    failed++;
  } catch (err) {
    if (err.message.includes(expectedMsg)) {
      console.log(`  PASS: ${testName}`);
      passed++;
    } else {
      console.error(`  FAIL: ${testName} — threw "${err.message}", expected to include "${expectedMsg}"`);
      failed++;
    }
  }
}

// Create a temp directory for isolated tests
const tmp = join(tmpdir(), `grug-test-${Date.now()}`);
mkdirSync(tmp, { recursive: true });

function withConfig(content, fn) {
  const configPath = join(tmp, `brains-${Date.now()}.json`);
  writeFileSync(configPath, content, "utf-8");
  const orig = process.env.GRUG_CONFIG;
  process.env.GRUG_CONFIG = configPath;
  try {
    return fn();
  } finally {
    if (orig === undefined) delete process.env.GRUG_CONFIG;
    else process.env.GRUG_CONFIG = orig;
  }
}

function withEnv(vars, fn) {
  const saved = {};
  for (const [k, v] of Object.entries(vars)) {
    saved[k] = process.env[k];
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
  try {
    return fn();
  } finally {
    for (const [k, v] of Object.entries(saved)) {
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
    }
  }
}

// ---- tests ----

console.log("\n=== loadBrains() tests ===\n");

// Test 1: parses brains.json correctly
console.log("1. parses brains.json with existing dirs");
{
  const dir1 = join(tmp, "mem1");
  const dir2 = join(tmp, "docs1");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true },
    { name: "drizzle", dir: dir2, flat: true },
  ]);

  const result = withConfig(config, () => loadBrains());

  assert(result.length === 2, "returns 2 brains");
  assert(result[0].name === "memories", "first brain name is memories");
  assert(result[0].primary === true, "first brain is primary");
  assert(result[0].writable === true, "non-flat brain defaults to writable");
  assert(result[1].name === "drizzle", "second brain name is drizzle");
  assert(result[1].flat === true, "second brain is flat");
  assert(result[1].writable === false, "flat brain defaults to writable:false");
  assert(result[1].syncInterval === 60, "default syncInterval is 60");
  assert(result[1].git === null, "default git is null");
}

// Test 2: filters out non-existent dirs
console.log("\n2. filters out brains whose dirs don't exist");
{
  const dir1 = join(tmp, "mem2");
  mkdirSync(dir1, { recursive: true });
  // dir2 intentionally not created

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true },
    { name: "missing", dir: join(tmp, "nonexistent-xyz"), flat: false },
  ]);

  const result = withConfig(config, () => loadBrains());
  assert(result.length === 1, "only existing brain returned");
  assert(result[0].name === "memories", "memories brain kept");
}

// Test 3: rejects duplicate names
console.log("\n3. rejects duplicate names");
{
  const dir1 = join(tmp, "mem3a");
  const dir2 = join(tmp, "mem3b");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true },
    { name: "memories", dir: dir2 },
  ]);

  withConfig(config, () =>
    assertThrows(
      () => loadBrains(),
      'duplicate brain name "memories"',
      "duplicate name throws"
    )
  );
}

// Test 4: rejects missing primary
console.log("\n4. rejects missing primary");
{
  const dir1 = join(tmp, "mem4");
  mkdirSync(dir1, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1 },
  ]);

  withConfig(config, () =>
    assertThrows(
      () => loadBrains(),
      'no brain marked "primary: true"',
      "missing primary throws"
    )
  );
}

// Test 5: rejects multiple primaries
console.log("\n5. rejects multiple primaries");
{
  const dir1 = join(tmp, "mem5a");
  const dir2 = join(tmp, "mem5b");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true },
    { name: "other", dir: dir2, primary: true },
  ]);

  withConfig(config, () =>
    assertThrows(
      () => loadBrains(),
      'multiple brains marked "primary: true"',
      "multiple primaries throws"
    )
  );
}

// Test 6: tilde expansion
console.log("\n6. ~ expansion in paths");
{
  const home = process.env.HOME;
  // Use the actual grug-brain memories dir if it exists, else just test expansion logic
  const expandedPath = expandHome("~/test-expand-path");
  assert(expandedPath === join(home, "test-expand-path"), "~/path expands to HOME/path");
  assert(expandHome("~") === home, "~ alone expands to HOME");
  assert(expandHome("/absolute/path") === "/absolute/path", "absolute path unchanged");
}

// Test 7: builds from env vars when no config file exists
console.log("\n7. builds from MEMORY_DIR env var (no config file)");
{
  const memDir = join(tmp, "mem7");
  mkdirSync(memDir, { recursive: true });

  const result = withEnv(
    { GRUG_CONFIG: "/tmp/definitely-does-not-exist-grug-config.json", MEMORY_DIR: memDir, DOCS_DIRS: undefined, DOCS_DIR: undefined },
    () => loadBrains()
  );

  assert(result.length === 1, "one brain from env var");
  assert(result[0].name === "memories", "brain named memories");
  assert(result[0].dir === memDir, "brain dir matches MEMORY_DIR");
  assert(result[0].primary === true, "brain is primary");
  assert(result[0].writable === true, "brain is writable");
}

// Test 8: builds flat doc brain from DOCS_DIRS name=path format
console.log("\n8. builds flat doc brain from DOCS_DIRS name=path");
{
  const memDir = join(tmp, "mem8");
  const docDir = join(tmp, "docs8");
  mkdirSync(memDir, { recursive: true });
  mkdirSync(docDir, { recursive: true });

  const result = withEnv(
    { GRUG_CONFIG: "/tmp/definitely-does-not-exist-grug-config.json", MEMORY_DIR: memDir, DOCS_DIRS: `drizzle=${docDir}`, DOCS_DIR: undefined },
    () => loadBrains()
  );

  assert(result.length === 2, "two brains from env vars");
  const docBrain = result.find(b => b.name === "drizzle");
  assert(docBrain !== undefined, "doc brain present");
  assert(docBrain.flat === true, "named entry creates flat brain");
  assert(docBrain.writable === false, "named/flat brain is read-only");
  assert(docBrain.primary === false, "doc brain is not primary");
}

// Test 9: builds multi doc brain from DOCS_DIRS path-only format
console.log("\n9. builds multi doc brain from DOCS_DIRS path-only");
{
  const memDir = join(tmp, "mem9");
  const docDir = join(tmp, "docs9");
  mkdirSync(memDir, { recursive: true });
  mkdirSync(docDir, { recursive: true });

  const result = withEnv(
    { GRUG_CONFIG: "/tmp/definitely-does-not-exist-grug-config.json", MEMORY_DIR: memDir, DOCS_DIRS: docDir, DOCS_DIR: undefined },
    () => loadBrains()
  );

  assert(result.length === 2, "two brains from env vars");
  const docBrain = result.find(b => b.name === "docs9");
  assert(docBrain !== undefined, "doc brain uses dir basename as name");
  assert(docBrain.flat === false, "path-only entry creates multi brain");
  assert(docBrain.writable === false, "multi doc brain is read-only");
}

// Test 10: explicit writable:true overrides flat default
console.log("\n10. explicit writable:true overrides flat default");
{
  const dir1 = join(tmp, "mem10");
  const dir2 = join(tmp, "docs10");
  mkdirSync(dir1, { recursive: true });
  mkdirSync(dir2, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true },
    { name: "myflat", dir: dir2, flat: true, writable: true },
  ]);

  const result = withConfig(config, () => loadBrains());
  const flatBrain = result.find(b => b.name === "myflat");
  assert(flatBrain.writable === true, "explicit writable:true overrides flat default");
}

// Test 11: GRUG_CONFIG env var overrides default config path
console.log("\n11. GRUG_CONFIG env var overrides default config path");
{
  const dir1 = join(tmp, "mem11");
  mkdirSync(dir1, { recursive: true });
  const customConfig = join(tmp, "custom-config.json");
  writeFileSync(customConfig, JSON.stringify([
    { name: "custom", dir: dir1, primary: true }
  ]), "utf-8");

  const result = withEnv({ GRUG_CONFIG: customConfig }, () => loadBrains());
  assert(result[0].name === "custom", "GRUG_CONFIG path used for loading");
}

// Test 12: invalid JSON throws helpful error
console.log("\n12. invalid JSON throws helpful error");
{
  withConfig("{ not valid json", () =>
    assertThrows(
      () => loadBrains(),
      "failed to parse",
      "invalid JSON throws parse error"
    )
  );
}

// Test 13: missing name field throws
console.log("\n13. missing name field throws");
{
  const dir1 = join(tmp, "mem13");
  mkdirSync(dir1, { recursive: true });

  const config = JSON.stringify([
    { dir: dir1, primary: true }
  ]);

  withConfig(config, () =>
    assertThrows(
      () => loadBrains(),
      'missing required "name" field',
      "missing name throws"
    )
  );
}

// Test 14: custom syncInterval and git preserved
console.log("\n14. custom syncInterval and git preserved");
{
  const dir1 = join(tmp, "mem14");
  mkdirSync(dir1, { recursive: true });

  const config = JSON.stringify([
    { name: "memories", dir: dir1, primary: true, git: "git@github.com:user/brain.git", syncInterval: 300 }
  ]);

  const result = withConfig(config, () => loadBrains());
  assert(result[0].git === "git@github.com:user/brain.git", "git URL preserved");
  assert(result[0].syncInterval === 300, "custom syncInterval preserved");
}

// Cleanup
rmSync(tmp, { recursive: true });

// Summary
console.log(`\n=== Results: ${passed} passed, ${failed} failed ===\n`);
if (failed > 0) process.exit(1);

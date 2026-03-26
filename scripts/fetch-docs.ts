#!/usr/bin/env bun
/**
 * Fetch Bun and TypeScript docs from GitHub, save as markdown for grug-brain indexing.
 *
 * Usage:
 *   bun scripts/fetch-docs.ts            # fetch both
 *   bun scripts/fetch-docs.ts bun        # fetch only bun
 *   bun scripts/fetch-docs.ts typescript  # fetch only typescript
 */

import { mkdirSync, writeFileSync, existsSync } from "fs";
import { join } from "path";

const DOCS_DIR = process.env.DOCS_DIR || join(import.meta.dir, "..", "..", "grug-docs");
const DELAY_MS = 100; // polite delay between fetches

function slugify(name: string): string {
  return name
    .replace(/\.mdx?$/, "")
    .replace(/\s+/g, "-")
    .toLowerCase()
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

function stripMdxImports(content: string): string {
  // Remove import lines and JSX-style component tags like <Foo /> or <Foo>...</Foo>
  return content
    .replace(/^import\s+.*$/gm, "")
    .replace(/<[A-Z]\w+\s*\/>/g, "")
    .replace(/<[A-Z]\w+[^>]*>[\s\S]*?<\/[A-Z]\w+>/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

async function fetchRaw(url: string): Promise<string | null> {
  try {
    const res = await fetch(url);
    if (!res.ok) {
      console.error(`  SKIP ${res.status}: ${url}`);
      return null;
    }
    return await res.text();
  } catch (e) {
    console.error(`  ERROR: ${url} — ${e}`);
    return null;
  }
}

async function delay() {
  await new Promise((r) => setTimeout(r, DELAY_MS));
}

// ============================================================
// BUN DOCS
// ============================================================

interface BunPage {
  slug: string;
  githubPath: string;
  title: string;
  tier: number;
}

const BUN_PAGES: BunPage[] = [
  // Tier 1 — core runtime APIs
  { slug: "runtime-sqlite", githubPath: "runtime/sqlite.mdx", title: "SQLite (bun:sqlite)", tier: 1 },
  { slug: "runtime-http-server", githubPath: "runtime/http/server.mdx", title: "HTTP Server (Bun.serve)", tier: 1 },
  { slug: "runtime-file-io", githubPath: "runtime/file-io.mdx", title: "File I/O", tier: 1 },
  { slug: "runtime-shell", githubPath: "runtime/shell.mdx", title: "Shell ($)", tier: 1 },
  { slug: "runtime-streams", githubPath: "runtime/streams.mdx", title: "Streams", tier: 1 },
  { slug: "runtime-child-process", githubPath: "runtime/child-process.mdx", title: "Child Processes (Bun.spawn)", tier: 1 },
  { slug: "runtime-workers", githubPath: "runtime/workers.mdx", title: "Workers", tier: 1 },
  { slug: "runtime-module-resolution", githubPath: "runtime/module-resolution.mdx", title: "Module Resolution", tier: 1 },
  { slug: "runtime-bunfig", githubPath: "runtime/bunfig.mdx", title: "bunfig.toml", tier: 1 },
  { slug: "runtime-typescript", githubPath: "runtime/typescript.mdx", title: "TypeScript Support", tier: 1 },
  { slug: "test-index", githubPath: "test/index.mdx", title: "Test Runner", tier: 1 },
  { slug: "test-writing", githubPath: "test/writing.mdx", title: "Writing Tests", tier: 1 },
  { slug: "test-mocks", githubPath: "test/mocks.mdx", title: "Mocks", tier: 1 },
  { slug: "bundler-index", githubPath: "bundler/index.mdx", title: "Bundler (Bun.build)", tier: 1 },

  // Tier 2 — secondary runtime APIs
  { slug: "runtime-globals", githubPath: "runtime/globals.mdx", title: "Globals", tier: 2 },
  { slug: "runtime-web-apis", githubPath: "runtime/web-apis.mdx", title: "Web APIs", tier: 2 },
  { slug: "runtime-nodejs-compat", githubPath: "runtime/nodejs-compat.mdx", title: "Node.js Compatibility", tier: 2 },
  { slug: "runtime-hashing", githubPath: "runtime/hashing.mdx", title: "Hashing", tier: 2 },
  { slug: "runtime-glob", githubPath: "runtime/glob.mdx", title: "Glob", tier: 2 },
  { slug: "runtime-plugins", githubPath: "runtime/plugins.mdx", title: "Plugins", tier: 2 },
  { slug: "runtime-sql", githubPath: "runtime/sql.mdx", title: "SQL (Bun.SQL)", tier: 2 },
  { slug: "runtime-redis", githubPath: "runtime/redis.mdx", title: "Redis", tier: 2 },
  { slug: "runtime-s3", githubPath: "runtime/s3.mdx", title: "S3", tier: 2 },
  { slug: "runtime-environment-variables", githubPath: "runtime/environment-variables.mdx", title: "Environment Variables", tier: 2 },
  { slug: "runtime-watch-mode", githubPath: "runtime/watch-mode.mdx", title: "Watch Mode", tier: 2 },
  { slug: "runtime-binary-data", githubPath: "runtime/binary-data.mdx", title: "Binary Data", tier: 2 },
  { slug: "runtime-cookies", githubPath: "runtime/cookies.mdx", title: "Cookies", tier: 2 },
  { slug: "runtime-utils", githubPath: "runtime/utils.mdx", title: "Utilities", tier: 2 },
  { slug: "runtime-html-rewriter", githubPath: "runtime/html-rewriter.mdx", title: "HTMLRewriter", tier: 2 },
  { slug: "runtime-cron", githubPath: "runtime/cron.mdx", title: "Cron", tier: 2 },
  { slug: "runtime-ffi", githubPath: "runtime/ffi.mdx", title: "FFI", tier: 2 },
  { slug: "runtime-http-websockets", githubPath: "runtime/http/websockets.mdx", title: "WebSockets", tier: 2 },
  { slug: "runtime-http-routing", githubPath: "runtime/http/routing.mdx", title: "HTTP Routing", tier: 2 },
  { slug: "runtime-http-tls", githubPath: "runtime/http/tls.mdx", title: "TLS", tier: 2 },
  { slug: "bundler-plugins", githubPath: "bundler/plugins.mdx", title: "Bundler Plugins", tier: 2 },
  { slug: "bundler-loaders", githubPath: "bundler/loaders.mdx", title: "Bundler Loaders", tier: 2 },
  { slug: "pm-install", githubPath: "pm/cli/install.mdx", title: "bun install", tier: 2 },
  { slug: "pm-workspaces", githubPath: "pm/workspaces.mdx", title: "Workspaces", tier: 2 },
  { slug: "pm-add", githubPath: "pm/cli/add.mdx", title: "bun add", tier: 2 },

  // Tier 2 — test runner extras
  { slug: "test-lifecycle", githubPath: "test/lifecycle.mdx", title: "Test Lifecycle Hooks", tier: 2 },
  { slug: "test-snapshots", githubPath: "test/snapshots.mdx", title: "Snapshots", tier: 2 },
  { slug: "test-coverage", githubPath: "test/coverage.mdx", title: "Code Coverage", tier: 2 },
  { slug: "test-dom", githubPath: "test/dom.mdx", title: "DOM Testing", tier: 2 },
];

const BUN_RAW_BASE = "https://raw.githubusercontent.com/oven-sh/bun/main/docs/";

async function fetchBunDocs(tier: number = 2) {
  const outDir = join(DOCS_DIR, "bun");
  mkdirSync(outDir, { recursive: true });

  const pages = BUN_PAGES.filter((p) => p.tier <= tier);
  console.log(`\nFetching ${pages.length} Bun docs (tier 1-${tier})...`);

  const fetched: BunPage[] = [];

  for (const page of pages) {
    const url = BUN_RAW_BASE + page.githubPath;
    const content = await fetchRaw(url);
    if (!content) continue;

    const cleaned = stripMdxImports(content);
    const outPath = join(outDir, `${page.slug}.mdx`);
    writeFileSync(outPath, cleaned + "\n", "utf-8");
    console.log(`  ${page.slug}.mdx (${Math.round(cleaned.length / 1024)}KB)`);
    fetched.push(page);
    await delay();
  }

  // Generate _meta.json
  const meta: (string | [string, string])[] = [];
  const t1 = fetched.filter((p) => p.tier === 1);
  const t2 = fetched.filter((p) => p.tier === 2);

  if (t1.length) {
    meta.push("Runtime & Core");
    for (const p of t1) meta.push([p.slug, p.title]);
  }
  if (t2.length) {
    meta.push("Extended APIs");
    for (const p of t2) meta.push([p.slug, p.title]);
  }

  writeFileSync(join(outDir, "_meta.json"), JSON.stringify(meta, null, 2) + "\n", "utf-8");
  console.log(`\nBun: ${fetched.length}/${pages.length} docs saved to docs/bun/`);
}

// ============================================================
// TYPESCRIPT DOCS
// ============================================================

interface TsPage {
  slug: string;
  githubPath: string;
  title: string;
  tier: number;
}

const TS_DOC_BASE = "https://raw.githubusercontent.com/microsoft/TypeScript-Website/v2/packages/documentation/copy/en/";
const TS_TSCONFIG_BASE = "https://raw.githubusercontent.com/microsoft/TypeScript-Website/v2/packages/tsconfig-reference/copy/en/options/";

const TS_PAGES: TsPage[] = [
  // Tier 1 — Type manipulation (highest signal for LLM coding)
  { slug: "mapped-types", githubPath: "handbook-v2/Type Manipulation/Mapped Types.md", title: "Mapped Types", tier: 1 },
  { slug: "conditional-types", githubPath: "handbook-v2/Type Manipulation/Conditional Types.md", title: "Conditional Types", tier: 1 },
  { slug: "template-literal-types", githubPath: "handbook-v2/Type Manipulation/Template Literal Types.md", title: "Template Literal Types", tier: 1 },
  { slug: "generics", githubPath: "handbook-v2/Type Manipulation/Generics.md", title: "Generics", tier: 1 },
  { slug: "keyof-type-operator", githubPath: "handbook-v2/Type Manipulation/Keyof Type Operator.md", title: "Keyof Type Operator", tier: 1 },
  { slug: "typeof-type-operator", githubPath: "handbook-v2/Type Manipulation/Typeof Type Operator.md", title: "Typeof Type Operator", tier: 1 },
  { slug: "indexed-access-types", githubPath: "handbook-v2/Type Manipulation/Indexed Access Types.md", title: "Indexed Access Types", tier: 1 },
  { slug: "utility-types", githubPath: "reference/Utility Types.md", title: "Utility Types", tier: 1 },
  { slug: "narrowing", githubPath: "handbook-v2/Narrowing.md", title: "Narrowing", tier: 1 },

  // Tier 2 — essential building blocks
  { slug: "everyday-types", githubPath: "handbook-v2/Everyday Types.md", title: "Everyday Types", tier: 2 },
  { slug: "more-on-functions", githubPath: "handbook-v2/More on Functions.md", title: "More on Functions", tier: 2 },
  { slug: "object-types", githubPath: "handbook-v2/Object Types.md", title: "Object Types", tier: 2 },
  { slug: "classes", githubPath: "handbook-v2/Classes.md", title: "Classes", tier: 2 },
  { slug: "modules", githubPath: "handbook-v2/Modules.md", title: "Modules", tier: 2 },
  { slug: "enums", githubPath: "reference/Enums.md", title: "Enums", tier: 2 },
  { slug: "decorators", githubPath: "reference/Decorators.md", title: "Decorators", tier: 2 },
  { slug: "type-compatibility", githubPath: "reference/Type Compatibility.md", title: "Type Compatibility", tier: 2 },
  { slug: "type-inference", githubPath: "reference/Type Inference.md", title: "Type Inference", tier: 2 },
  { slug: "declaration-merging", githubPath: "reference/Declaration Merging.md", title: "Declaration Merging", tier: 2 },
  { slug: "modules-reference", githubPath: "modules-reference/Reference.md", title: "Modules Reference", tier: 2 },
  { slug: "jsdoc-reference", githubPath: "javascript/JSDoc Reference.md", title: "JSDoc Reference", tier: 2 },

  // Tier 3 — project config and extras
  { slug: "tsconfig-json", githubPath: "project-config/tsconfig.json.md", title: "tsconfig.json", tier: 3 },
  { slug: "compiler-options", githubPath: "project-config/Compiler Options.md", title: "Compiler Options (CLI)", tier: 3 },
  { slug: "advanced-types", githubPath: "reference/Advanced Types.md", title: "Advanced Types", tier: 3 },
  { slug: "iterators-generators", githubPath: "reference/Iterators and Generators.md", title: "Iterators and Generators", tier: 3 },
  { slug: "jsx", githubPath: "reference/JSX.md", title: "JSX", tier: 3 },
  { slug: "mixins", githubPath: "reference/Mixins.md", title: "Mixins", tier: 3 },
  { slug: "symbols", githubPath: "reference/Symbols.md", title: "Symbols", tier: 3 },
];

// Key tsconfig options to fetch individually
const TSCONFIG_OPTIONS = [
  "strict", "noUncheckedIndexedAccess", "exactOptionalPropertyTypes",
  "moduleResolution", "verbatimModuleSyntax", "isolatedDeclarations",
  "esModuleInterop", "skipLibCheck", "resolveJsonModule",
  "noEmit", "declaration", "sourceMap",
  "target", "module", "lib",
  "paths", "baseUrl", "rootDir", "outDir",
  "jsx", "allowJs", "checkJs",
];

async function fetchTypeScriptDocs(tier: number = 2) {
  const outDir = join(DOCS_DIR, "typescript");
  mkdirSync(outDir, { recursive: true });

  const pages = TS_PAGES.filter((p) => p.tier <= tier);
  console.log(`\nFetching ${pages.length} TypeScript docs (tier 1-${tier})...`);

  const fetched: TsPage[] = [];

  for (const page of pages) {
    const encodedPath = page.githubPath.split("/").map(encodeURIComponent).join("/");
    const url = TS_DOC_BASE + encodedPath;
    const content = await fetchRaw(url);
    if (!content) continue;

    const outPath = join(outDir, `${page.slug}.md`);
    writeFileSync(outPath, content, "utf-8");
    console.log(`  ${page.slug}.md (${Math.round(content.length / 1024)}KB)`);
    fetched.push(page);
    await delay();
  }

  // Fetch key tsconfig options and combine into one file
  console.log(`\nFetching ${TSCONFIG_OPTIONS.length} tsconfig option docs...`);
  const optionDocs: string[] = [
    "---\ntitle: TSConfig Reference (Key Options)\nlayout: docs\n---\n",
  ];

  for (const opt of TSCONFIG_OPTIONS) {
    const url = TS_TSCONFIG_BASE + `${opt}.md`;
    const content = await fetchRaw(url);
    if (!content) continue;

    // Extract body after frontmatter
    const body = content.replace(/^---[\s\S]*?---\n*/, "").trim();
    optionDocs.push(`## \`${opt}\`\n\n${body}`);
    await delay();
  }

  if (optionDocs.length > 1) {
    const outPath = join(outDir, "tsconfig-reference.md");
    writeFileSync(outPath, optionDocs.join("\n\n") + "\n", "utf-8");
    console.log(`  tsconfig-reference.md (${Math.round(optionDocs.join("\n\n").length / 1024)}KB — ${optionDocs.length - 1} options)`);
    fetched.push({ slug: "tsconfig-reference", githubPath: "", title: "TSConfig Reference (Key Options)", tier: 2 });
  }

  // Generate _meta.json
  const meta: (string | [string, string])[] = [];
  const t1 = fetched.filter((p) => p.tier === 1);
  const t2 = fetched.filter((p) => p.tier === 2);
  const t3 = fetched.filter((p) => p.tier === 3);

  if (t1.length) {
    meta.push("Type System");
    for (const p of t1) meta.push([p.slug, p.title]);
  }
  if (t2.length) {
    meta.push("Language Features");
    for (const p of t2) meta.push([p.slug, p.title]);
  }
  if (t3.length) {
    meta.push("Project Configuration");
    for (const p of t3) meta.push([p.slug, p.title]);
  }

  writeFileSync(join(outDir, "_meta.json"), JSON.stringify(meta, null, 2) + "\n", "utf-8");
  console.log(`\nTypeScript: ${fetched.length}/${pages.length + 1} docs saved to docs/typescript/`);
}

// ============================================================
// MAIN
// ============================================================

const target = process.argv[2]?.toLowerCase();

if (!target || target === "bun") {
  await fetchBunDocs();
}
if (!target || target === "typescript") {
  await fetchTypeScriptDocs();
}

console.log("\nDone. Restart the MCP server to index new docs.");

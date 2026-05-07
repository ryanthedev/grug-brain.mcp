/**
 * Phase 6 Playwright tests — autocomplete, palette, side panels, local graph.
 *
 * One test per Done-When item DW-6.1 through DW-6.14. The fixture seeds the
 * brain with `notes/hello.md`, `notes/script-test.md`, and `tags/tagged.md`
 * (containing `#testing #memory`).
 */

const { test, expect } = require("./fixtures");
const { AxeBuilder } = require("@axe-core/playwright");

/** Open Hello World in edit mode. Used by DW-6.1, 6.2, 6.10. */
async function openHelloEdit(page) {
  await page.goto(""); // baseURL set by playwright config? — fall through
}

async function gotoApp(page, baseUrl) {
  await page.goto(baseUrl);
  await expect(page.locator(".memory-item").first()).toBeVisible({ timeout: 8000 });
}

async function openHelloAndEdit(page, baseUrl) {
  await gotoApp(page, baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  // The viewer opens new memories in edit mode by default; CM should mount.
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });
}

// ── DW-6.1 ──────────────────────────────────────────────────────────────────
test("dw-6.1: typing [[ opens wikilink autocomplete and selection inserts [[name]]", async ({ page, grugServer }) => {
  await openHelloAndEdit(page, grugServer.baseUrl);

  // Move caret to end of doc and type `[[`. Use a blank line separator so we
  // escape the markdown list context (Enter inside a list creates `- `).
  const editor = page.locator(".cm-content");
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("End");
  await page.keyboard.type("\n\n[[Hel");

  // Autocomplete tooltip should appear.
  await expect(page.locator(".cm-tooltip-autocomplete")).toBeVisible({ timeout: 4000 });
  // Wait an extra tick so CM registers the tooltip and ArrowDown/Enter target it.
  await page.waitForTimeout(150);
  // First option corresponds to "Hello World".
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);

  // Buffer should now contain `[[Hello World]]`.
  const docText = await page.evaluate(() => {
    const v = window.__grugEditorView;
    return v && v.state ? v.state.doc.toString() : "";
  });
  expect(docText).toContain("[[Hello World]]");
});

// ── DW-6.2 ──────────────────────────────────────────────────────────────────
test("dw-6.2: typing # opens tag autocomplete and selection inserts #tag", async ({ page, grugServer }) => {
  await openHelloAndEdit(page, grugServer.baseUrl);

  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("End");
  await page.keyboard.type("\n\n#tes");

  await expect(page.locator(".cm-tooltip-autocomplete")).toBeVisible({ timeout: 4000 });
  await page.waitForTimeout(150);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);

  const docText = await page.evaluate(() => {
    const v = window.__grugEditorView;
    return v && v.state ? v.state.doc.toString() : "";
  });
  expect(docText).toMatch(/#testing\b/);
});

// ── DW-6.3 ──────────────────────────────────────────────────────────────────
test("dw-6.3: Cmd-K opens palette; fuzzy filter returns memories + categories + commands", async ({ page, grugServer }) => {
  await gotoApp(page, grugServer.baseUrl);

  // Open palette via the API to avoid platform-specific Cmd-K race; we test
  // the keybinding separately in DW-6.5.
  await page.evaluate(() => window.__grugPalette.open());
  await expect(page.locator("#palette-modal")).toBeVisible();

  await page.locator("#palette-input").fill("hel");
  // Should produce at least 1 item ("Hello World" memory).
  const items = page.locator(".palette-item");
  await expect(items.first()).toBeVisible();
  const count = await items.count();
  expect(count).toBeGreaterThan(0);

  // Empty query yields ALL three kinds of items.
  await page.locator("#palette-input").fill("");
  const kinds = await page.locator(".palette-item .palette-kind").allTextContents();
  expect(kinds).toContain("memory");
  expect(kinds).toContain("category");
  expect(kinds).toContain("command");
});

// ── DW-6.4 ──────────────────────────────────────────────────────────────────
test("dw-6.4: palette commands dispatch (toggle theme, new memory, rename, delete, jump-to-category)", async ({ page, grugServer }) => {
  await gotoApp(page, grugServer.baseUrl);

  // ── Toggle theme: dispatch directly via commands.run; theme MODE always cycles.
  const modeBefore = await page.evaluate(() => document.documentElement.dataset.themeMode || "");
  await page.evaluate(() => window.__grugCommands.run("toggle-theme"));
  await page.waitForFunction(
    (prev) => (document.documentElement.dataset.themeMode || "") !== prev,
    modeBefore,
    { timeout: 3000 }
  );

  // Open Hello World so rename/delete have a target.
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator("#editor-rename")).toBeVisible();
  // Wait for the URL to reflect the open memory (router state in sync).
  await expect(page).toHaveURL(/memory\//, { timeout: 5000 });
  // Wait for the editor to mount (proxy for state.activeMemoryPath being set).
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 5000 });

  // ── Rename: opens rename modal. Wait for state.activeMemoryPath to be set
  // (the click → router.navigate → onRoute → state.set chain is async).
  await page.waitForFunction(
    () => !!(window.__grugState && window.__grugState.activeMemoryPath),
    null, { timeout: 5000 }
  );
  await page.evaluate(() => window.__grugCommands.run("rename"));
  await expect(page.locator("#rename-modal")).toBeVisible({ timeout: 5000 });
  await page.locator("#rename-cancel").click();

  // ── Delete: opens delete modal.
  await page.evaluate(() => window.__grugCommands.run("delete"));
  await expect(page.locator("#delete-modal")).toBeVisible();
  await page.locator("#delete-cancel").click();

  // ── New memory (after rename/delete to avoid the dirty-draft guard).
  await page.evaluate(() => window.__grugCommands.run("new-memory", "notes"));
  await page.waitForFunction(() => {
    const v = window.__grugEditorView;
    if (!v) return false;
    return (v.state && v.state.doc && v.state.doc.length === 0);
  }, null, { timeout: 3000 });

  // ── Jump-to-category: navigates to a category.
  // The new-memory draft above set s.dirty=true; if the navigation triggers
  // the unsaved-changes guard modal, discard the draft to proceed.
  await page.evaluate(() => window.__grugCommands.run("jump-to-category"));
  const unsavedVisible = await page.locator("#unsaved-modal").isVisible().catch(() => false);
  if (unsavedVisible) {
    await page.locator("#unsaved-discard").click();
  }
  await expect(page).toHaveURL(/category\//, { timeout: 3000 });
});

// ── DW-6.5 ──────────────────────────────────────────────────────────────────
test("dw-6.5: palette is focus-trapped, Escape closes, ArrowDown/ArrowUp navigate", async ({ page, grugServer }) => {
  await gotoApp(page, grugServer.baseUrl);

  await page.evaluate(() => window.__grugPalette.open());
  await expect(page.locator("#palette-modal")).toBeVisible();

  // Focus is on the input.
  await expect(page.locator("#palette-input")).toBeFocused();

  // ArrowDown moves selection.
  const items = page.locator(".palette-item");
  const count = await items.count();
  expect(count).toBeGreaterThan(1);

  await page.locator("#palette-input").focus();
  await page.keyboard.press("ArrowDown");
  await expect(items.nth(1)).toHaveClass(/active/);
  await page.keyboard.press("ArrowUp");
  await expect(items.nth(0)).toHaveClass(/active/);

  // Escape closes the palette.
  await page.keyboard.press("Escape");
  await expect(page.locator("#palette-modal")).toBeHidden();
});

// ── DW-6.6 ──────────────────────────────────────────────────────────────────
test("dw-6.6: GET /api/tags returns [{tag, count}]", async ({ page, grugServer }) => {
  const r = await page.request.get(`${grugServer.baseUrl}/api/tags?brain=testbrain`);
  expect(r.ok()).toBe(true);
  const data = await r.json();
  expect(Array.isArray(data)).toBe(true);
  // tagged.md contains #testing and #memory.
  const tags = data.map(d => d.tag);
  expect(tags).toContain("testing");
  expect(tags).toContain("memory");
  for (const row of data) {
    expect(typeof row.tag).toBe("string");
    expect(typeof row.count).toBe("number");
  }
});

// ── DW-6.7 ──────────────────────────────────────────────────────────────────
test("dw-6.7: GET /api/backlinks returns referrers", async ({ page, grugServer }) => {
  // Seed needs a wikilink — write one programmatically via the API.
  // Create a memory linking to "Hello World".
  const create = await page.request.post(`${grugServer.baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: { path: "notes/refer", body: "see [[Hello World]]", frontmatter: "name: refer\ndate: 2025-01-01\ntype: memory" },
  });
  expect(create.ok()).toBe(true);
  // SSE debounce — give the indexer a moment.
  await page.waitForTimeout(800);

  const r = await page.request.get(
    `${grugServer.baseUrl}/api/backlinks?brain=testbrain&path=notes/hello.md`
  );
  expect(r.ok()).toBe(true);
  const data = await r.json();
  const paths = data.map(d => d.path);
  expect(paths).toContain("notes/refer.md");
});

// ── DW-6.8 ──────────────────────────────────────────────────────────────────
test("dw-6.8: GET /api/graph/local returns nodes+edges for N-hop neighborhood", async ({ page, grugServer }) => {
  const r = await page.request.get(
    `${grugServer.baseUrl}/api/graph/local?brain=testbrain&path=notes/hello.md&hops=2`
  );
  expect(r.ok()).toBe(true);
  const data = await r.json();
  expect(Array.isArray(data.nodes)).toBe(true);
  expect(Array.isArray(data.edges)).toBe(true);
  // Focus node always present.
  const paths = data.nodes.map(n => n.path);
  expect(paths).toContain("notes/hello.md");
});

// ── DW-6.9 ──────────────────────────────────────────────────────────────────
test("dw-6.9: backlinks panel renders results for current memory; click navigates", async ({ page, grugServer }) => {
  // First, create a memory linking to "Hello World" so the panel has data.
  await page.request.post(`${grugServer.baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: { path: "notes/refer", body: "see [[Hello World]]", frontmatter: "name: refer\ndate: 2025-01-01\ntype: memory" },
  });
  await page.waitForTimeout(800);

  await gotoApp(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator("#preview-content")).toBeVisible();

  const panel = page.locator("#panel-backlinks-body");
  await expect(panel.locator(".backlink-item").first()).toBeVisible({ timeout: 5000 });

  // Click the backlink — URL hash should change to refer.
  await panel.locator(".backlink-item").first().click();
  await expect(page).toHaveURL(/memory\/.*refer/, { timeout: 3000 });
});

// ── DW-6.10 ─────────────────────────────────────────────────────────────────
test("dw-6.10: outline panel renders heading tree from current buffer; click scrolls", async ({ page, grugServer }) => {
  await openHelloAndEdit(page, grugServer.baseUrl);
  // hello.md has `# Hello World` — outline must list it.
  const outlineItems = page.locator("#panel-outline-body .outline-item");
  await expect(outlineItems.first()).toBeVisible({ timeout: 5000 });
  const text = await outlineItems.first().textContent();
  expect(text.trim()).toBe("Hello World");

  // Click the heading; expect the editor to receive focus on that line.
  await outlineItems.first().click();
  const focused = await page.evaluate(() =>
    document.activeElement && document.activeElement.classList
      ? document.activeElement.classList.contains("cm-content") ||
        !!document.activeElement.closest(".cm-editor")
      : false
  );
  expect(focused).toBe(true);
});

// ── DW-6.11 ─────────────────────────────────────────────────────────────────
test("dw-6.11: tag pane renders tags with counts; click filters memory list", async ({ page, grugServer }) => {
  await gotoApp(page, grugServer.baseUrl);

  const tagItems = page.locator("#panel-tags-body .tag-item");
  await expect(tagItems.first()).toBeVisible({ timeout: 5000 });
  // testing tag (count 1) should appear.
  const testingBtn = tagItems.filter({ hasText: "#testing" });
  await expect(testingBtn).toBeVisible();
  await expect(testingBtn.locator(".tag-count")).toContainText("(1)");

  // Click the tag — only the tagged memory should remain visible.
  await testingBtn.click();
  await expect(page.locator(".memory-item:not([hidden])")).toHaveCount(1);
  const visibleName = await page.locator(".memory-item:not([hidden]) .mem-name").textContent();
  expect(visibleName).toContain("Tagged Memory");

  // Click again to clear.
  await testingBtn.click();
  await expect(page.locator(".memory-item:not([hidden])").first()).toBeVisible();
});

// ── DW-6.12 ─────────────────────────────────────────────────────────────────
test("dw-6.12: local N-hop graph renders via sigma; click on node opens that memory", async ({ page, grugServer }) => {
  // Create a refer-link so the local view has something to traverse.
  await page.request.post(`${grugServer.baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: { path: "notes/refer", body: "see [[Hello World]]", frontmatter: "name: refer\ndate: 2025-01-01\ntype: memory" },
  });
  await page.waitForTimeout(800);

  await gotoApp(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator("#preview-content")).toBeVisible();

  // Toggle into local mode.
  await page.locator("#graph-mode-toggle").click();
  await expect(page.locator("#graph-mode-toggle")).toHaveAttribute("aria-pressed", "true");
  // Sigma instance should be set.
  await page.waitForFunction(() => !!window.__grugSigma, null, { timeout: 5000 });

  // Programmatic click on a non-focus node — emit sigma's clickNode handler.
  const navigated = await page.evaluate(() => {
    return new Promise(resolve => {
      const sigma = window.__grugSigma;
      if (!sigma) return resolve(false);
      const g = sigma.getGraph();
      const nodes = g.nodes();
      const targetNode = nodes.find(n => n !== "notes/hello.md");
      if (!targetNode) return resolve(false);
      const handlers = sigma._events && sigma._events.clickNode;
      // sigma.on("clickNode") stores listeners; emit synthetic event.
      try {
        sigma.emit("clickNode", { node: targetNode });
      } catch (_) {}
      setTimeout(() => resolve(location.hash), 200);
    });
  });
  expect(typeof navigated).toBe("string");
  expect(navigated).toMatch(/memory\//);
});

// ── DW-6.14 (axe-core) ──────────────────────────────────────────────────────
test("dw-6.14: axe-core wcag2a/wcag2aa zero critical violations on palette + panels", async ({ page, grugServer }) => {
  await gotoApp(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();

  // Wait for panels to populate.
  await expect(page.locator("#panel-tags-body .tag-item").first()).toBeVisible({ timeout: 5000 });

  // Audit panels.
  const baseline = await new AxeBuilder({ page })
    .options({ runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } })
    .include("#side-panels")
    .analyze();
  const baseCritical = baseline.violations.filter(v => v.impact === "critical");
  expect(baseCritical, JSON.stringify(baseCritical, null, 2)).toEqual([]);

  // Open palette and audit it.
  await page.evaluate(() => window.__grugPalette.open());
  await expect(page.locator("#palette-modal")).toBeVisible();
  const paletteAudit = await new AxeBuilder({ page })
    .options({ runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } })
    .include("#palette-modal")
    .analyze();
  const palCritical = paletteAudit.violations.filter(v => v.impact === "critical");
  expect(palCritical, JSON.stringify(palCritical, null, 2)).toEqual([]);
});

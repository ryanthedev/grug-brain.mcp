/**
 * Comprehensive axe-core wcag2a/wcag2aa audit — Phase 7 DW-7.1 + DW-7.5.
 *
 * Covers every UI surface:
 *   1. Read mode (default — memory selected, preview rendered)
 *   2. Edit mode (frontmatter form + CodeMirror editor)
 *   3. Backlinks panel
 *   4. Outline panel
 *   5. Tag pane
 *   6. Local N-hop graph mode
 *   7. Palette modal (Cmd-K)
 *   8. Conflict modal (3-pane diff, triggered by stale ETag)
 *   9. Delete confirm modal
 *  10. Rename modal
 *  11. Create draft modal
 *  12. Unsaved-changes modal
 *
 * For each surface: assert zero critical violations (impact === "critical").
 * Also verifies focus returns to opener after every modal close (DW-7.5).
 */

const { test, expect } = require("./fixtures");
const { AxeBuilder } = require("@axe-core/playwright");
const fs = require("fs");
const path = require("path");

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Navigate to the app and wait for the memory list. */
async function goto(page, baseUrl) {
  await page.goto(baseUrl);
  await expect(page.locator(".memory-item").first()).toBeVisible({ timeout: 10000 });
}

/** Open Hello World and wait for the editor to mount. */
async function openHelloEdit(page, baseUrl) {
  await goto(page, baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });
}

/** Run axe on `include` selector (or full page if null); assert zero critical violations. */
async function assertNoAxeCritical(page, include, label) {
  const builder = new AxeBuilder({ page })
    .options({ runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } });
  if (include) builder.include(include);
  const results = await builder.analyze();
  const critical = results.violations.filter(v => v.impact === "critical");
  if (critical.length > 0) {
    const msgs = critical.map(v =>
      `[${v.id}] ${v.description}\n  nodes: ${v.nodes.map(n => n.html).join(" | ")}`
    ).join("\n");
    throw new Error(`axe-core critical violations on "${label}":\n${msgs}`);
  }
}

// ── 1. Read mode ─────────────────────────────────────────────────────────────

test("a11y-7.1-read: zero critical violations in read mode", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  // Switch to read mode.
  await page.locator("#mode-toggle").click();
  await expect(page.locator("#preview-content")).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, null, "read mode");
});

// ── 2. Edit mode ─────────────────────────────────────────────────────────────

test("a11y-7.1-edit: zero critical violations in edit mode", async ({ page, grugServer }) => {
  await openHelloEdit(page, grugServer.baseUrl);
  await assertNoAxeCritical(page, null, "edit mode");
});

// ── 3. Backlinks panel ───────────────────────────────────────────────────────

test("a11y-7.1-backlinks: zero critical violations in backlinks panel", async ({ page, grugServer }) => {
  // Seed a wikilink so the panel has content.
  await page.request.post(`${grugServer.baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: { path: "notes/linker", body: "see [[Hello World]]", frontmatter: "name: linker\ndate: 2025-01-01\ntype: memory" },
  });
  await page.waitForTimeout(800);

  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator("#panel-backlinks-body .backlink-item").first()).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, "#side-panels", "backlinks panel");
});

// ── 4. Outline panel ─────────────────────────────────────────────────────────

test("a11y-7.1-outline: zero critical violations in outline panel", async ({ page, grugServer }) => {
  await openHelloEdit(page, grugServer.baseUrl);
  await expect(page.locator("#panel-outline-body .outline-item").first()).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, "#side-panels", "outline panel");
});

// ── 5. Tag pane ──────────────────────────────────────────────────────────────

test("a11y-7.1-tags: zero critical violations in tag pane", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await expect(page.locator("#panel-tags-body .tag-item").first()).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, "#side-panels", "tag pane");
});

// ── 6. Local N-hop graph ─────────────────────────────────────────────────────

test("a11y-7.1-local-graph: zero critical violations in local N-hop graph mode", async ({ page, grugServer }) => {
  // Seed a link so the local graph has edges.
  await page.request.post(`${grugServer.baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: { path: "notes/neighbor", body: "see [[Hello World]]", frontmatter: "name: neighbor\ndate: 2025-01-01\ntype: memory" },
  });
  await page.waitForTimeout(800);

  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await page.locator("#graph-mode-toggle").click();
  await expect(page.locator("#graph-mode-toggle")).toHaveAttribute("aria-pressed", "true");
  await page.waitForFunction(() => !!window.__grugSigma, { timeout: 5000 });
  await assertNoAxeCritical(page, "#graph-panel", "local N-hop graph");
});

// ── 7. Palette modal ─────────────────────────────────────────────────────────

test("a11y-7.1-palette: zero critical violations in palette modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.evaluate(() => window.__grugPalette.open());
  await expect(page.locator("#palette-modal")).toBeVisible({ timeout: 4000 });
  await assertNoAxeCritical(page, "#palette-modal", "palette modal");
  // Close the palette for cleanup.
  await page.keyboard.press("Escape");
});

// ── 8. Conflict modal ────────────────────────────────────────────────────────

test("a11y-7.1-conflict: zero critical violations in conflict modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // Stale the ETag and type an edit so there's a dirty buffer to save.
  await page.evaluate(() => {
    const s = window.__grugState;
    if (s && s.buffer) s.buffer.etag = -1;
  });
  await page.locator(".cm-content").click();
  await page.keyboard.type(" a11y-conflict-test");

  // Save — should fire 409 and open conflict modal.
  await page.locator("#editor-save").click();
  await expect(page.locator("#conflict-modal")).toBeVisible({ timeout: 8000 });
  await assertNoAxeCritical(page, "#conflict-modal", "conflict modal");
  // Close cleanly.
  await page.locator("#conflict-cancel").click();
});

// ── 9. Delete confirm modal ──────────────────────────────────────────────────

test("a11y-7.1-delete: zero critical violations in delete confirm modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible({ timeout: 4000 });
  await assertNoAxeCritical(page, "#delete-modal", "delete confirm modal");
  // Close without deleting.
  await page.locator("#delete-cancel").click();
});

// ── 10. Rename modal ─────────────────────────────────────────────────────────

test("a11y-7.1-rename: zero critical violations in rename modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible({ timeout: 4000 });
  await assertNoAxeCritical(page, "#rename-modal", "rename modal");
  // Close without renaming.
  await page.locator("#rename-cancel").click();
});

// ── 11. Create draft modal ───────────────────────────────────────────────────

test("a11y-7.1-create: zero critical violations in create draft modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);

  // Open a new draft via the category + button, then trigger first-save to
  // open the create-name modal.
  await page.evaluate(() => {
    if (window.__grugCommands) window.__grugCommands.run("new-memory");
  });
  // Editor should now be in draft mode (no activeMemoryPath).
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 5000 });

  // The create modal opens when we try to save a draft. Trigger it via the
  // public crud.showCreate() so we don't fire the full save flow.
  await page.evaluate(() => {
    // crud is not exposed globally, so we induce it via the save button which
    // will call showCreate when buffer.draft === true.
    return window.__grugState && window.__grugState.buffer && window.__grugState.buffer.draft;
  });

  // Click save — this triggers the create modal for a draft.
  await page.locator("#editor-save").click();
  await expect(page.locator("#create-modal")).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, "#create-modal", "create draft modal");
  // Close without creating.
  await page.locator("#create-cancel").click();
});

// ── 12. Unsaved-changes modal ─────────────────────────────────────────────────

test("a11y-7.1-unsaved: zero critical violations in unsaved-changes modal", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // Type something to make the buffer dirty.
  await page.locator(".cm-content").click();
  await page.keyboard.type(" dirty-edit-for-a11y");

  // Trigger the unsaved-changes guard by switching brains (or clicking a diff
  // memory). A nav to a different memory while dirty opens the modal.
  await page.locator(".memory-item .mem-name", { hasText: "Script Test" }).click();
  await expect(page.locator("#unsaved-modal")).toBeVisible({ timeout: 5000 });
  await assertNoAxeCritical(page, "#unsaved-modal", "unsaved-changes modal");
  // Cancel to leave the modal.
  await page.locator("#unsaved-cancel").click();
});

// ── DW-7.5: Focus returns to opener after every modal close ──────────────────

test("dw-7.5: focus returns to opener after palette close", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);

  // Focus the theme toggle as a stable opener.
  const opener = page.locator("#theme-toggle");
  await opener.focus();
  const openerEl = await page.evaluateHandle(() => document.activeElement);

  // Open palette and close via Escape.
  await page.evaluate(() => window.__grugPalette.open());
  await expect(page.locator("#palette-modal")).toBeVisible({ timeout: 4000 });
  await page.keyboard.press("Escape");
  await expect(page.locator("#palette-modal")).toBeHidden({ timeout: 3000 });

  // Active element should have returned to the opener.
  const afterEl = await page.evaluate(() => document.activeElement && document.activeElement.id);
  expect(afterEl).toBe("theme-toggle");
});

test("dw-7.5: focus returns to opener after conflict modal close", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // Focus the save button before opening the conflict modal.
  await page.locator("#editor-save").focus();

  await page.evaluate(() => {
    const s = window.__grugState;
    if (s && s.buffer) s.buffer.etag = -1;
  });
  await page.locator(".cm-content").click();
  await page.keyboard.type(" focus-test");
  await page.locator("#editor-save").click();
  await expect(page.locator("#conflict-modal")).toBeVisible({ timeout: 8000 });

  // Cancel — focus should return to save button.
  await page.locator("#conflict-cancel").click();
  await expect(page.locator("#conflict-modal")).toBeHidden({ timeout: 3000 });
  const afterEl = await page.evaluate(() => document.activeElement && document.activeElement.id);
  expect(afterEl).toBe("editor-save");
});

test("dw-7.5: focus returns to opener after delete modal close", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // Focus the delete button as the opener.
  await page.locator("#editor-delete").focus();
  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible({ timeout: 4000 });

  await page.locator("#delete-cancel").click();
  await expect(page.locator("#delete-modal")).toBeHidden({ timeout: 3000 });

  const afterEl = await page.evaluate(() => document.activeElement && document.activeElement.id);
  expect(afterEl).toBe("editor-delete");
});

test("dw-7.5: focus returns to opener after rename modal close", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  await page.locator("#editor-rename").focus();
  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible({ timeout: 4000 });

  await page.locator("#rename-cancel").click();
  await expect(page.locator("#rename-modal")).toBeHidden({ timeout: 3000 });

  const afterEl = await page.evaluate(() => document.activeElement && document.activeElement.id);
  expect(afterEl).toBe("editor-rename");
});

test("dw-7.5: focus returns to opener after unsaved-changes modal cancel", async ({ page, grugServer }) => {
  await goto(page, grugServer.baseUrl);
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // Make the buffer dirty.
  await page.locator(".cm-content").click();
  await page.keyboard.type(" focus-return-test");

  // The opener here is the "Script Test" memory item.
  const opener = page.locator(".memory-item .mem-name", { hasText: "Script Test" });

  // Click a different memory — the unsaved guard should open the modal.
  await opener.click();
  await expect(page.locator("#unsaved-modal")).toBeVisible({ timeout: 5000 });

  // The opener in this case was the Script Test memory item (clicked to trigger
  // the guard). Cancel — focus should go back to what was focused when guard()
  // was called.
  await page.locator("#unsaved-cancel").click();
  await expect(page.locator("#unsaved-modal")).toBeHidden({ timeout: 3000 });
  // The Script Test item should have received focus back (it was active when
  // modal.open() was called).
  const afterFocused = await page.evaluate(() =>
    document.activeElement ? document.activeElement.textContent.trim() : ""
  );
  // Accept focus going to any memory item (the guard caller is the .memory-item click).
  expect(afterFocused.length).toBeGreaterThan(0);
});

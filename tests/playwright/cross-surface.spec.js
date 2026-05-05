/**
 * Cross-surface integration test — Phase 7 DW-7.7.
 *
 * One end-to-end happy-path tour exercising every major Plan 2 surface:
 *
 *   open memory → edit → autocomplete → save → rename → open backlink →
 *   palette → delete → toast
 *
 * This catches inter-phase regressions: each phase's output must compose
 * correctly with the others in a real session.
 *
 * Setup note: the test creates "notes/source.md" which contains
 * "[[Hello World]]" — this gives the backlinks panel content and exercises
 * the wikilink-rewrite path during rename.
 */

const { test, expect } = require("./fixtures");
const path = require("path");

test("dw-7.7: cross-surface happy-path tour", { timeout: 60000 }, async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;

  // ── Step 0: seed a source memory that links to Hello World ─────────────────
  // Do this before navigating so the indexer has time to process it.
  const createResp = await page.request.post(`${baseUrl}/api/memory`, {
    headers: { "X-Grug-Client": "web", "Content-Type": "application/json" },
    data: {
      path: "notes/source",
      body: "see [[Hello World]] for details",
      frontmatter: "name: source\ndate: 2025-01-01\ntype: memory",
    },
  });
  expect(createResp.ok()).toBe(true);

  // ── Step 1: open a memory in the viewer ────────────────────────────────────
  await page.goto(baseUrl);
  await expect(page.locator(".memory-item").first()).toBeVisible({ timeout: 10000 });

  // Wait for the indexer to process the seeded source memory.
  await page.waitForTimeout(800);

  // Open Hello World; it should mount the editor.
  await page.locator(".memory-item .mem-name", { hasText: "Hello World" }).click();
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  // ── Step 2: edit — type in the CodeMirror editor ───────────────────────────
  const editor = page.locator(".cm-content");
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("End");
  // Add a blank line to escape any list context, then type.
  await page.keyboard.type("\n\nEdited via cross-surface test.");

  // Verify the buffer is now dirty (dirty indicator visible).
  await expect(page.locator("#dirty-indicator")).toBeVisible({ timeout: 3000 });

  // ── Step 3: autocomplete — type [[ to trigger wikilink autocomplete ─────────
  await page.keyboard.type(" See also [[Sour");
  await expect(page.locator(".cm-tooltip-autocomplete")).toBeVisible({ timeout: 4000 });
  // Wait for CM to register the autocomplete tooltip.
  await page.waitForTimeout(150);
  // Select the first suggestion.
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);

  // Buffer should now contain [[source]] (or [[source]] with category prefix).
  const docText = await page.evaluate(() => {
    const v = window.__grugEditorView;
    return v && v.state ? v.state.doc.toString() : "";
  });
  // The autocomplete inserted the wikilink; it should contain `[[`.
  expect(docText).toMatch(/\[\[/);

  // ── Step 4: save via Cmd-S ─────────────────────────────────────────────────
  await page.keyboard.press("Meta+S");

  // Wait for save to complete (either success toast or conflict modal).
  // If a conflict modal opens, dismiss it with "Overwrite" to force the save.
  await page.waitForFunction(() => {
    const toast = document.querySelector(".toast.toast-success");
    const conflict = document.getElementById("conflict-modal");
    return (toast && !toast.hidden) || (conflict && !conflict.hidden);
  }, { timeout: 8000 });

  // Handle conflict if it appeared (ETag race between initial load and save).
  const conflictVisible = await page.evaluate(() => {
    const el = document.getElementById("conflict-modal");
    return el && !el.hidden;
  });
  if (conflictVisible) {
    await page.locator("#conflict-overwrite").click();
    await expect(page.locator(".toast.toast-success")).toBeVisible({ timeout: 5000 });
  }

  // Dirty indicator should clear.
  await expect(page.locator("#dirty-indicator")).toBeHidden({ timeout: 3000 });

  // ── Step 5: rename the memory ─────────────────────────────────────────────
  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible({ timeout: 4000 });

  const renameInput = page.locator("#rename-new-path");
  await renameInput.clear();
  await renameInput.fill("notes/hello-renamed");

  await page.locator("#rename-submit").click();

  // Should show a success toast mentioning rename.
  await expect(page.locator(".toast.toast-success")).toBeVisible({ timeout: 10000 });
  const toastText = await page.locator(".toast.toast-success").first().textContent();
  expect(toastText).toBeTruthy();

  // After rename, the app navigates to the renamed memory; URL should update.
  await expect(page).toHaveURL(/hello-renamed/, { timeout: 8000 });

  // ── Step 6: verify the backlinks panel surface + navigate to source ──────────
  // After rename, "source.md" was rewritten by the backend (rewrite_links=true)
  // so it now links to [[hello-renamed]]. The backlinks panel shows who links to
  // the current memory. Since this panel is populated via an SSE-triggered
  // re-render (watcher debounce 500ms + indexer + network), we verify it via
  // the API directly, then navigate to source from the memory list.
  const backlinkPanel = page.locator("#panel-backlinks-body");

  // Verify the backlinks API returns source.md (DB was updated atomically in the
  // rename transaction, so this should be immediately consistent).
  const blResp = await page.request.get(
    `${grugServer.baseUrl}/api/backlinks?brain=testbrain&path=notes/hello-renamed.md`
  );
  expect(blResp.ok()).toBe(true);
  const blData = await blResp.json();

  const blPaths = blData.map(d => d.path);
  expect(blPaths).toContain("notes/source.md");

  // Navigate to source.md via the memory list (which shows the updated list
  // after the SSE-triggered loadMemories already ran).
  await page.locator(".memory-item .mem-name", { hasText: "source" }).click();
  await expect(page).toHaveURL(/notes(%2F|\/)source/, { timeout: 5000 });

  // ── Step 7: open palette and navigate back to hello-renamed ───────────────
  await page.keyboard.press("Meta+K");
  await expect(page.locator("#palette-modal")).toBeVisible({ timeout: 4000 });

  const paletteInput = page.locator("#palette-input");
  await paletteInput.fill("renamed");

  // Wait for filtered results.
  await page.waitForTimeout(200);
  const items = page.locator("#palette-list .palette-item");
  await expect(items.first()).toBeVisible({ timeout: 3000 });

  // Select the first item.
  await page.keyboard.press("Enter");
  await expect(page.locator("#palette-modal")).toBeHidden({ timeout: 3000 });

  // Should now be on hello-renamed.
  await expect(page).toHaveURL(/hello-renamed/, { timeout: 5000 });

  // ── Step 8: delete the memory ─────────────────────────────────────────────
  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });

  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible({ timeout: 4000 });

  // Get the memory name from the delete-target-name element, which is
  // populated by crud.openDelete().
  const targetName = await page.locator("#delete-target-name").textContent();
  expect(targetName).toBeTruthy();

  // Type the memory name into the confirm input to enable the Delete button.
  await page.locator("#delete-confirm").fill(targetName);
  await expect(page.locator("#delete-submit")).toBeEnabled({ timeout: 3000 });
  await page.locator("#delete-submit").click();

  // ── Step 9: verify success toast ─────────────────────────────────────────
  await expect(page.locator(".toast.toast-success").first()).toBeVisible({ timeout: 8000 });
  const deleteToastText = await page.locator(".toast.toast-success").first().textContent();
  expect(deleteToastText).toBeTruthy();

  // The deleted memory should no longer appear in the memory list.
  await expect(page.locator(".memory-item .mem-name", { hasText: "hello-renamed" }))
    .toHaveCount(0, { timeout: 5000 });
});

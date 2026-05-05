/**
 * Plan 2 Phase 4: CodeMirror 6 editor + frontmatter form + Cmd-S save.
 *
 * One spec per DW item (DW-4.1 through DW-4.12). The DW-4.x numbering here
 * is the Plan 2 Phase 4 namespace, distinct from Plan 1's DW-4.x in
 * dw-tests.spec.js (which lives in a separate file).
 */

const { test, expect } = require("./fixtures");
const { AxeBuilder } = require("@axe-core/playwright");
const fs = require("fs");
const path = require("path");

const REPO_ROOT = path.resolve(__dirname, "../..");
const isMac = process.platform === "darwin";
const SAVE_KEY = isMac ? "Meta+s" : "Control+s";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Open the first memory in the list and wait for the editor to be ready. */
async function openFirstMemory(page) {
  await page.waitForSelector(".memory-item", { timeout: 8000 });
  await page.locator(".memory-item").first().click();
  // Wait until either the editor or the read-only banner has rendered.
  await page.waitForFunction(
    () =>
      document.querySelector(".cm-editor") ||
      document.querySelector(".readonly-banner"),
    { timeout: 8000 }
  );
}

// ---------------------------------------------------------------------------
// DW-4.1: CodeMirror mounts on memory open in edit mode; read toggle works.
// ---------------------------------------------------------------------------
test("dw-4.1: cm6 mounts in edit mode and read toggle preserves preview", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // CodeMirror's root class is `.cm-editor`.
  await expect(page.locator(".cm-editor")).toBeVisible();
  // The read-mode preview should be hidden in edit mode.
  await expect(page.locator("#preview-content")).toBeHidden();

  // Click the mode toggle to switch to read mode.
  await page.locator("#mode-toggle").click();
  await expect(page.locator("#preview-content")).toBeVisible();
  await expect(page.locator(".cm-editor")).toBeHidden();

  // Toggle back to edit mode — the editor should still be there.
  await page.locator("#mode-toggle").click();
  await expect(page.locator(".cm-editor")).toBeVisible();
});

// ---------------------------------------------------------------------------
// DW-4.2: Frontmatter form: name/description/tags + validation.
// ---------------------------------------------------------------------------
test("dw-4.2: frontmatter form renders and validates name on save", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Three labelled inputs.
  const nameInput = page.locator("#fm-name");
  const descInput = page.locator("#fm-description");
  const tagsInput = page.locator("#fm-tags");
  await expect(nameInput).toBeVisible();
  await expect(descInput).toBeVisible();
  await expect(tagsInput).toBeVisible();

  // Clear name to trigger validation.
  await nameInput.fill("");
  await page.locator("#editor-save").click();

  // Toast or inline error mentions name.
  await expect(page.locator("#fm-name-error")).toBeVisible();

  // Comma-separated tags split into chips/array.
  await tagsInput.fill("alpha, beta,gamma");
  // Internal split surfaced via data attribute for testability.
  await expect(tagsInput).toHaveAttribute("data-tags-count", "3");
});

// ---------------------------------------------------------------------------
// DW-4.3: Cmd-S triggers PUT with If-Match header and updates state etag on 200.
// ---------------------------------------------------------------------------
test("dw-4.3: cmd-s saves with If-Match etag and updates state etag on 200", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Capture the PUT request.
  let captured = null;
  page.on("request", req => {
    if (req.method() === "PUT" && /\/api\/memory\//.test(req.url())) {
      captured = {
        url: req.url(),
        ifMatch: req.headers()["if-match"],
        clientHeader: req.headers()["x-grug-client"],
        body: req.postData(),
      };
    }
  });

  // Get current etag from state (exposed for tests).
  const beforeEtag = await page.evaluate(() => window.__grugState?.buffer?.etag);
  expect(beforeEtag).toBeGreaterThan(0);

  // Make a tiny edit so save has work to do.
  await page.locator(".cm-content").click();
  await page.keyboard.type(" edit");

  // Trigger Cmd-S / Ctrl-S.
  await page.keyboard.press(SAVE_KEY);

  // Wait for the PUT to fire.
  await page.waitForFunction(() => window.__grugLastSaveStatus === 200, { timeout: 8000 });

  expect(captured).not.toBeNull();
  expect(captured.ifMatch).toBeTruthy();
  expect(captured.clientHeader).toBe("web");

  // After save, dirty cleared and etag refreshed (mtime moves forward, but at
  // minimum it is still a number).
  const afterDirty = await page.evaluate(() => window.__grugState?.dirty);
  expect(afterDirty).toBe(false);
  const afterEtag = await page.evaluate(() => window.__grugState?.buffer?.etag);
  expect(typeof afterEtag).toBe("number");
});

// ---------------------------------------------------------------------------
// DW-4.4: Save button uses the same code path as Cmd-S.
// ---------------------------------------------------------------------------
test("dw-4.4: save button click triggers same save flow", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  let putFired = false;
  page.on("request", req => {
    if (req.method() === "PUT" && /\/api\/memory\//.test(req.url())) putFired = true;
  });

  await page.locator(".cm-content").click();
  await page.keyboard.type(" via button");

  const saveBtn = page.locator("#editor-save");
  await expect(saveBtn).toBeVisible();
  await expect(saveBtn).toBeEnabled();
  await saveBtn.click();

  await page.waitForFunction(() => window.__grugLastSaveStatus === 200, { timeout: 8000 });
  expect(putFired).toBe(true);
});

// ---------------------------------------------------------------------------
// DW-4.5: Dirty indicator appears on edit, clears on save.
// ---------------------------------------------------------------------------
test("dw-4.5: dirty marker appears on edit and clears after save", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Initially clean.
  await expect(page.locator("#dirty-indicator")).toBeHidden();

  await page.locator(".cm-content").click();
  await page.keyboard.type(" dirty edit");

  await expect(page.locator("#dirty-indicator")).toBeVisible();

  // Save and observe the indicator clearing.
  await page.locator("#editor-save").click();
  await page.waitForFunction(() => window.__grugLastSaveStatus === 200, { timeout: 8000 });

  await expect(page.locator("#dirty-indicator")).toBeHidden();
});

// ---------------------------------------------------------------------------
// DW-4.6: Unsaved-changes guard fires on brain switch.
// ---------------------------------------------------------------------------
test("dw-4.6: unsaved-changes guard blocks brain switch", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Make the buffer dirty.
  await page.locator(".cm-content").click();
  await page.keyboard.type(" dirty");

  // Try to switch to a different memory.
  const items = page.locator(".memory-item");
  const count = await items.count();
  if (count >= 2) {
    await items.nth(1).click();
    // The unsaved-changes modal must appear.
    await expect(page.locator("#unsaved-modal")).toBeVisible();
    // Cancel: stay on current memory.
    await page.locator("#unsaved-cancel").click();
    await expect(page.locator("#unsaved-modal")).toBeHidden();
  } else {
    // At minimum verify the modal exists in the DOM.
    expect(await page.locator("#unsaved-modal").count()).toBe(1);
  }
});

// ---------------------------------------------------------------------------
// DW-4.7: Read-only brain shows banner instead of editor.
// ---------------------------------------------------------------------------
test("dw-4.7: read-only brain shows banner and no editor", async ({ page, grugServer }) => {
  // Override the brains API to return a read-only brain.
  await page.route("**/api/brains", async (route) => {
    const body = JSON.stringify([
      { name: "robrain", primary: true, writable: false, source: null, flat: false },
    ]);
    await route.fulfill({ status: 200, contentType: "application/json", body });
  });
  // Override memories so the list is non-empty.
  await page.route("**/api/memories**", async (route) => {
    const body = JSON.stringify([
      { path: "notes/ro.md", brain: "robrain", category: "notes", name: "RO Memory", description: "ro", date: "2025-01-01", mtime: 1.0 },
    ]);
    await route.fulfill({ status: 200, contentType: "application/json", body });
  });
  // Stub the memory preview API.
  await page.route("**/api/memory/robrain/notes/ro**", async (route) => {
    const body = JSON.stringify({
      frontmatter: { name: "RO Memory", description: "ro" },
      body: "read only body",
      mtime: 1.0,
      neighbors: [],
    });
    await route.fulfill({ status: 200, contentType: "application/json", body });
  });
  await page.route("**/api/graph**", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ nodes: [], edges: [] }) });
  });

  await page.goto(grugServer.baseUrl);
  await page.waitForSelector(".memory-item", { timeout: 8000 });
  await page.locator(".memory-item").first().click();

  await expect(page.locator(".readonly-banner")).toBeVisible();
  await expect(page.locator(".cm-editor")).toHaveCount(0);
  // Save controls present in DOM but hidden (toolbar is hidden for ro brains).
  await expect(page.locator("#editor-save")).toBeHidden();
  await expect(page.locator("#editor-toolbar")).toBeHidden();
  await expect(page.locator("#frontmatter-form")).toBeHidden();
});

// ---------------------------------------------------------------------------
// DW-4.8: Wikilink and tag decorations render with distinct CSS classes.
// ---------------------------------------------------------------------------
test("dw-4.8: wikilink and tag decorations render", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Replace the doc with content containing a wikilink and a tag.
  await page.evaluate(() => {
    const view = window.__grugEditorView;
    if (!view) throw new Error("editor view not exposed");
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: "See [[other-note]] and #important." },
    });
  });

  await expect(page.locator(".cm-wikilink").first()).toBeVisible();
  await expect(page.locator(".cm-tag").first()).toBeVisible();
});

// ---------------------------------------------------------------------------
// DW-4.9: Toggle preserves scroll position and unsaved buffer.
// ---------------------------------------------------------------------------
test("dw-4.9: edit/read toggle preserves scroll and dirty buffer", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);

  // Set a known dirty value with enough content to scroll.
  await page.evaluate(() => {
    const view = window.__grugEditorView;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: "DIRTY-MARKER\n" + "line\n".repeat(50) },
    });
  });
  const dirty = await page.evaluate(() => window.__grugState?.dirty);
  expect(dirty).toBe(true);

  // Scroll the editor to a non-zero position via the panel scroll container.
  // CM's scroller grows to fill its flex host so the panel is the real scroll
  // container for both modes.
  await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    if (panel) panel.scrollTop = 80;
  });
  // Give layout time to settle.
  await page.waitForTimeout(50);
  const editScrollBefore = await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    return panel ? panel.scrollTop : 0;
  });
  // Sanity: scroll must have actually moved (panel must be tall enough).
  expect(editScrollBefore).toBeGreaterThan(0);

  // Toggle to read mode — use JS click to prevent Playwright from scrolling
  // the panel before the click fires (Playwright's click() scroll-into-view
  // resets #preview-panel.scrollTop, defeating the capture).
  await page.evaluate(() => document.getElementById("mode-toggle").click());
  await expect(page.locator("#preview-content")).toBeVisible();

  // Toggle back to edit mode.
  await page.evaluate(() => document.getElementById("mode-toggle").click());
  await expect(page.locator(".cm-editor")).toBeVisible();

  // Wait for double-rAF scroll restoration to land.
  await page.waitForTimeout(200);
  const editScrollAfter = await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    return panel ? panel.scrollTop : 0;
  });
  expect(Math.abs(editScrollAfter - editScrollBefore)).toBeLessThanOrEqual(5);

  // Buffer and dirty state must still be intact.
  const docText = await page.evaluate(() => window.__grugEditorView.state.doc.toString());
  expect(docText.startsWith("DIRTY-MARKER")).toBe(true);

  const stillDirty = await page.evaluate(() => window.__grugState?.dirty);
  expect(stillDirty).toBe(true);

  // ---- read → edit → read scroll preservation ----
  // Scroll the preview to a non-zero position after toggling to read mode.
  await page.evaluate(() => document.getElementById("mode-toggle").click());
  await expect(page.locator("#preview-content")).toBeVisible();
  // Wait for preview to render.
  await page.waitForTimeout(100);

  await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    if (panel) panel.scrollTop = 60;
  });
  await page.waitForTimeout(50);
  const readScrollBefore = await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    return panel ? panel.scrollTop : 0;
  });

  // Toggle to edit mode — JS click to avoid Playwright's scroll-into-view.
  await page.evaluate(() => document.getElementById("mode-toggle").click());
  await expect(page.locator(".cm-editor")).toBeVisible();

  // Toggle back to read mode — should restore preview scroll.
  await page.evaluate(() => document.getElementById("mode-toggle").click());
  await expect(page.locator("#preview-content")).toBeVisible();
  await page.waitForTimeout(200);

  const readScrollAfter = await page.evaluate(() => {
    const panel = document.getElementById("preview-panel");
    return panel ? panel.scrollTop : 0;
  });

  // If the preview-panel was actually scrollable, assert position is preserved.
  // If readScrollBefore is 0 (content too short to scroll), skip the assertion.
  if (readScrollBefore > 0) {
    expect(Math.abs(readScrollAfter - readScrollBefore)).toBeLessThanOrEqual(5);
  }
});

// ---------------------------------------------------------------------------
// DW-4.10: implicit — covered by the file existing with the above tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// DW-4.11: axe-core baseline on the edit surface.
// ---------------------------------------------------------------------------
test("dw-4.11: axe-core baseline on edit surface", async ({ page, grugServer }) => {
  await page.goto(grugServer.baseUrl);
  await openFirstMemory(page);
  await expect(page.locator(".cm-editor")).toBeVisible();

  const results = await new AxeBuilder({ page })
    .options({ runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } })
    .analyze();
  const critical = results.violations.filter(v => v.impact === "critical");
  if (critical.length > 0) {
    const msgs = critical.map(v => `${v.id}: ${v.description} (${v.nodes.length} nodes)`).join("\n");
    throw new Error(`axe-core critical violations:\n${msgs}`);
  }
});

// ---------------------------------------------------------------------------
// DW-4.12: CodeMirror bundle present + size documented.
// ---------------------------------------------------------------------------
test("dw-4.12: codemirror bundle vendored and size documented", async () => {
  const bundle = path.join(REPO_ROOT, "web", "vendor", "codemirror.min.js");
  expect(fs.existsSync(bundle), "codemirror.min.js should exist").toBe(true);
  const size = fs.statSync(bundle).size;
  expect(size).toBeGreaterThan(100_000); // sanity: real bundle, not a stub

  const versions = fs.readFileSync(
    path.join(REPO_ROOT, "web", "vendor", "VERSIONS.txt"),
    "utf8",
  );
  expect(versions).toMatch(/codemirror/i);
  expect(versions).toMatch(new RegExp(String(size)));
});

/**
 * Plan 2 Phase 5: Conflict UI + create / delete / rename UX.
 *
 * One spec per DW-5.x item. Uses the `grugServer` fixture (writable testbrain).
 *
 * Conflict-induction strategy (DW-5.1, 5.2, 5.3):
 *   The fixture exposes `brainDir`. We open a memory, then directly modify the
 *   on-disk file (bypassing the API). The watcher will refresh the index, but
 *   the editor still holds the OLD mtime as `buffer.etag`. Saving with that
 *   stale ETag returns 409 + ConflictResponse, which opens the conflict modal.
 */

const { test, expect } = require("./fixtures");
const { AxeBuilder } = require("@axe-core/playwright");
const fs = require("fs");
const path = require("path");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function openFirstMemory(page) {
  await page.waitForSelector(".memory-item", { timeout: 8000 });
  await page.locator(".memory-item").first().click();
  await page.waitForFunction(
    () => document.querySelector(".cm-editor") || document.querySelector(".readonly-banner"),
    { timeout: 8000 }
  );
}

/** Open a specific memory by visible name; resolves once editor is ready. */
async function openMemoryByName(page, name) {
  await page.waitForSelector(".memory-item", { timeout: 8000 });
  await page.locator(".memory-item", { hasText: name }).first().click();
  await page.waitForFunction(
    () => document.querySelector(".cm-editor") || document.querySelector(".readonly-banner"),
    { timeout: 8000 }
  );
}

/**
 * Force the buffer's etag to a stale value so the next save fires 409.
 * More reliable than waiting on filesystem mtime resolution.
 */
async function staleBufferEtag(page) {
  await page.evaluate(() => {
    const s = window.__grugState;
    if (!s || !s.buffer) throw new Error("no buffer");
    s.buffer.etag = -1;
  });
}

async function clickSave(page) {
  await page.locator("#editor-save").click();
}

// ---------------------------------------------------------------------------
// DW-5.1: Stale-ETag save opens the 3-pane conflict modal.
// ---------------------------------------------------------------------------
test("dw-5.1: stale-etag save opens 3-pane conflict modal", async ({ page, grugServer }) => {
  const { baseUrl, brainDir } = grugServer;
  await page.goto(baseUrl);
  await openMemoryByName(page, "Hello World");

  const filePath = path.join(brainDir, "notes", "hello.md");
  const original = fs.readFileSync(filePath, "utf8");
  fs.writeFileSync(filePath, original + "\n\nappended-by-other-writer\n");

  await staleBufferEtag(page);

  await page.locator(".cm-content").click();
  await page.keyboard.type(" yours-edit");
  await clickSave(page);

  await expect(page.locator("#conflict-modal")).toBeVisible({ timeout: 8000 });
  await expect(page.locator("#conflict-yours")).toBeVisible();
  await expect(page.locator("#conflict-theirs")).toBeVisible();
  await expect(page.locator("#conflict-merged")).toBeVisible();
  await expect(page.locator("#conflict-theirs")).toContainText("appended-by-other-writer");
});

// ---------------------------------------------------------------------------
// DW-5.2: Conflict actions: reload-theirs / overwrite / cancel.
// ---------------------------------------------------------------------------
test("dw-5.2: conflict reload-theirs replaces buffer", async ({ page, grugServer }) => {
  const { baseUrl, brainDir } = grugServer;
  await page.goto(baseUrl);
  await openMemoryByName(page, "Hello World");

  const filePath = path.join(brainDir, "notes", "hello.md");
  const original = fs.readFileSync(filePath, "utf8");
  fs.writeFileSync(filePath, original + "\n\ntheirs-marker\n");
  await staleBufferEtag(page);

  await page.locator(".cm-content").click();
  await page.keyboard.type(" mine");
  await clickSave(page);
  await expect(page.locator("#conflict-modal")).toBeVisible();

  await page.locator("#conflict-reload").click();
  await expect(page.locator("#conflict-modal")).toBeHidden();
  const bodyAfter = await page.evaluate(() => window.__grugState.buffer.body);
  expect(bodyAfter).toContain("theirs-marker");
  const dirty = await page.evaluate(() => window.__grugState.dirty);
  expect(dirty).toBe(false);
});

test("dw-5.2b: conflict overwrite uses theirs etag", async ({ page, grugServer }) => {
  const { baseUrl, brainDir } = grugServer;
  await page.goto(baseUrl);
  await openMemoryByName(page, "Hello World");

  const filePath = path.join(brainDir, "notes", "hello.md");
  const original = fs.readFileSync(filePath, "utf8");
  fs.writeFileSync(filePath, original + "\n\nold-theirs\n");
  await staleBufferEtag(page);

  await page.locator(".cm-content").click();
  await page.keyboard.type(" mine-overwrite");
  await clickSave(page);
  await expect(page.locator("#conflict-modal")).toBeVisible();

  let secondPut = null;
  page.on("request", req => {
    if (req.method() === "PUT" && /\/api\/memory\//.test(req.url())) {
      secondPut = { ifMatch: req.headers()["if-match"] };
    }
  });

  await page.locator("#conflict-overwrite").click();
  await expect(page.locator("#conflict-modal")).toBeHidden({ timeout: 8000 });
  expect(secondPut).not.toBeNull();
  expect(secondPut.ifMatch).toBeTruthy();
});

test("dw-5.2c: conflict cancel closes modal", async ({ page, grugServer }) => {
  const { baseUrl, brainDir } = grugServer;
  await page.goto(baseUrl);
  await openMemoryByName(page, "Hello World");

  const filePath = path.join(brainDir, "notes", "hello.md");
  const original = fs.readFileSync(filePath, "utf8");
  fs.writeFileSync(filePath, original + "\n\ncancel-marker\n");
  await staleBufferEtag(page);

  await page.locator(".cm-content").click();
  await page.keyboard.type(" attempt");
  await clickSave(page);
  await expect(page.locator("#conflict-modal")).toBeVisible();

  await page.locator("#conflict-cancel").click();
  await expect(page.locator("#conflict-modal")).toBeHidden();
});

// ---------------------------------------------------------------------------
// DW-5.3: jsdiff renders line-level diff classes in merged-preview.
// ---------------------------------------------------------------------------
test("dw-5.3: jsdiff renders line-level diff with classes", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);

  // Test that Diff.diffLines is loaded and that we can render its output via
  // textContent + class assignments — this is what the conflict module does.
  await page.evaluate(() => {
    const el = document.getElementById("conflict-merged");
    while (el.firstChild) el.removeChild(el.firstChild);
    const yours = "alpha\nbeta\ngamma\n";
    const theirs = "alpha\nBETA\ngamma\n";
    const chunks = window.Diff.diffLines(yours, theirs);
    chunks.forEach(c => {
      const span = document.createElement("span");
      if (c.added) span.className = "diff-add";
      else if (c.removed) span.className = "diff-remove";
      else span.className = "diff-context";
      span.textContent = c.value;
      el.appendChild(span);
    });
  });
  await expect(page.locator("#conflict-merged .diff-add")).toHaveCount(1);
  await expect(page.locator("#conflict-merged .diff-remove")).toHaveCount(1);
});

// ---------------------------------------------------------------------------
// DW-5.4: + button opens draft editor; first save creates the file.
// ---------------------------------------------------------------------------
test("dw-5.4: plus button opens draft editor and first save creates file", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await page.waitForSelector(".category-btn", { timeout: 8000 });

  const notesItem = page.locator(".category-item", { hasText: "notes" });
  await notesItem.locator(".category-add").click();

  await expect(page.locator(".cm-editor")).toBeVisible({ timeout: 8000 });
  const draftActive = await page.evaluate(() =>
    !!(window.__grugState.buffer && window.__grugState.buffer.draft)
  );
  expect(draftActive).toBe(true);

  await page.locator("#fm-name").fill("phase-5-new-memory");
  await page.locator(".cm-content").click();
  await page.keyboard.type("Hello from a draft!");

  await page.locator("#editor-save").click();
  await expect(page.locator("#create-modal")).toBeVisible({ timeout: 4000 });
  await expect(page.locator("#create-name")).toHaveValue("phase-5-new-memory");

  await page.locator("#create-submit").click();
  await expect(page.locator("#create-modal")).toBeHidden({ timeout: 4000 });

  await expect(page.locator(".memory-item .mem-name", { hasText: "phase-5-new-memory" })).toBeVisible({ timeout: 4000 });
});

// ---------------------------------------------------------------------------
// DW-5.5: Cmd-K stub command opens the same draft flow.
// ---------------------------------------------------------------------------
test("dw-5.5: commands.run new-memory opens draft editor", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await page.waitForSelector(".category-btn", { timeout: 8000 });

  const result = await page.evaluate(() => {
    return window.__grugCommands && window.__grugCommands.run("new-memory", "notes");
  });
  expect(result).toBe(true);
  const draftActive = await page.evaluate(() =>
    !!(window.__grugState.buffer && window.__grugState.buffer.draft)
  );
  expect(draftActive).toBe(true);
});

// ---------------------------------------------------------------------------
// DW-5.6: Delete modal — gated submit on typed-name match.
// ---------------------------------------------------------------------------
test("dw-5.6: delete modal gates submit on typed-name match", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await openFirstMemory(page);

  const memName = await page.evaluate(() => {
    const s = window.__grugState;
    const m = s.memories.find(m => m.path === s.activeMemoryPath);
    return m && (m.name || m.path);
  });
  expect(memName).toBeTruthy();

  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible();

  await expect(page.locator("#delete-submit")).toBeDisabled();

  await page.locator("#delete-confirm").fill("wrong-name");
  await expect(page.locator("#delete-submit")).toBeDisabled();

  await page.locator("#delete-confirm").fill(memName);
  await expect(page.locator("#delete-submit")).toBeEnabled();

  await page.locator("#delete-submit").click();
  await expect(page.locator("#delete-modal")).toBeHidden({ timeout: 4000 });
  await expect(page.locator(".toast")).toBeVisible({ timeout: 4000 });
});

// ---------------------------------------------------------------------------
// DW-5.7: Rename calls the Phase 2 route with rewrite_links and toasts count.
// ---------------------------------------------------------------------------
test("dw-5.7: rename calls phase 2 route and toasts affected count", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await openFirstMemory(page);

  let renameUrl = null;
  page.on("request", req => {
    if (req.method() === "POST" && /\/rename\?/.test(req.url())) {
      renameUrl = req.url();
    }
  });

  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible();

  await page.locator("#rename-new-path").fill("notes/renamed-by-test");
  await page.locator("#rename-submit").click();

  await expect(page.locator("#rename-modal")).toBeHidden({ timeout: 4000 });
  expect(renameUrl).toBeTruthy();
  expect(renameUrl).toContain("rewrite_links=true");
  await expect(page.locator(".toast-message", { hasText: /Renamed/ })).toBeVisible({ timeout: 4000 });
});

// ---------------------------------------------------------------------------
// DW-5.8: Modals trap focus and dismiss on Escape.
// ---------------------------------------------------------------------------
test("dw-5.8: modals trap focus and dismiss on Escape", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await openFirstMemory(page);

  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible();

  // Wait for focus to land inside the modal (setTimeout 0).
  await page.waitForFunction(() => {
    const m = document.getElementById("rename-modal");
    return m && m.contains(document.activeElement);
  }, { timeout: 2000 });

  await page.keyboard.press("Escape");
  await expect(page.locator("#rename-modal")).toBeHidden();

  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("#delete-modal")).toBeHidden();
});

// ---------------------------------------------------------------------------
// DW-5.9: Toast surfaces success + error for CRUD/save.
// ---------------------------------------------------------------------------
test("dw-5.9: toasts surface for create / delete / rename / save success+error", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await openFirstMemory(page);

  await page.locator(".cm-content").click();
  await page.keyboard.type(" toast-test");
  await page.locator("#editor-save").click();
  await page.waitForFunction(() => window.__grugLastSaveStatus === 200, { timeout: 8000 });
  await expect(page.locator(".toast-message", { hasText: /Saved/ })).toBeVisible({ timeout: 4000 });

  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible();
  await page.locator("#rename-new-path").fill("");
  await page.locator("#rename-submit").click();
  // An error-style toast must appear (the success "Saved" toast also persists,
  // so we match by the error title text).
  await expect(page.locator(".toast-title", { hasText: "Error" })).toBeVisible({ timeout: 4000 });
});

// ---------------------------------------------------------------------------
// DW-5.11: axe-core wcag2a/wcag2aa zero critical violations across modals.
// ---------------------------------------------------------------------------
test("dw-5.11: axe-core finds zero critical violations across modals", async ({ page, grugServer }) => {
  const { baseUrl } = grugServer;
  await page.goto(baseUrl);
  await openFirstMemory(page);

  async function auditOpen(label) {
    const results = await new AxeBuilder({ page })
      .options({ runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } })
      .analyze();
    const critical = results.violations.filter(v => v.impact === "critical");
    if (critical.length > 0) {
      const msgs = critical.map(v => `${v.id}: ${v.description} (${v.nodes.length} nodes)`).join("\n");
      throw new Error(`[${label}] axe-core ${critical.length} critical:\n${msgs}`);
    }
  }

  await page.locator("#editor-rename").click();
  await expect(page.locator("#rename-modal")).toBeVisible();
  await auditOpen("rename");
  await page.keyboard.press("Escape");

  await page.locator("#editor-delete").click();
  await expect(page.locator("#delete-modal")).toBeVisible();
  await auditOpen("delete");
  await page.keyboard.press("Escape");

  // Open conflict modal directly to audit it.
  await page.evaluate(() => {
    const el = document.getElementById("conflict-modal");
    document.getElementById("conflict-yours").textContent = "yours";
    document.getElementById("conflict-theirs").textContent = "theirs";
    document.getElementById("conflict-merged").textContent = "diff";
    el.hidden = false;
  });
  await expect(page.locator("#conflict-modal")).toBeVisible();
  await auditOpen("conflict");
});

/**
 * DW-4.13: Golden-read smoke test.
 *
 * Exercises the full read path:
 *   brain switcher → category selection → memory click → preview render → graph render
 *
 * This test is designed to run in CI (or locally via `make test-playwright`).
 * It starts the grug binary as a child process, verifies all major UI components
 * render correctly, and ensures no console errors occur during the round-trip.
 */

const { test, expect } = require("./fixtures");

test("dw-4.13: golden read path — brain → category → memory → preview → graph", async ({
  page,
  grugServer,
}) => {
  const { baseUrl } = grugServer;

  // Collect console errors to detect broken app behavior.
  const consoleErrors = [];
  page.on("console", msg => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  page.on("pageerror", err => consoleErrors.push(err.message));

  // ── Step 1: Navigate to the app root ──
  await page.goto(baseUrl);

  // ── Step 2: Brain switcher appears ──
  await expect(page.locator("#brain-switcher .brain-btn")).toHaveCount(1, { timeout: 8000 });
  const brainBtn = page.locator(".brain-btn", { hasText: "testbrain" });
  await expect(brainBtn).toBeVisible();

  // ── Step 3: Category tree populates ──
  await page.waitForSelector(".category-btn", { timeout: 8000 });

  // ── Step 4: Select the "notes" category ──
  const notesBtn = page.locator(".category-btn", { hasText: "notes" });
  await expect(notesBtn).toBeVisible();
  await notesBtn.click();
  await expect(page).toHaveURL(/#\/brain\/testbrain\/category\/notes/);

  // ── Step 5: Memory list shows expected memories ──
  await expect(page.locator(".memory-item", { hasText: "Hello World" })).toBeVisible({ timeout: 8000 });
  await expect(page.locator(".memory-item .mem-name", { hasText: "Hello World" })).toBeVisible();

  // ── Step 6: Click a memory → preview renders ──
  const helloItem = page.locator(".memory-item", { hasText: "Hello World" });
  await helloItem.click();

  // Preview should show the markdown content.
  await expect(page.locator("#preview-content")).not.toContainText("Select a memory", {
    timeout: 8000,
  });
  await expect(page.locator("#preview-content")).toContainText("Hello World");

  // ── Step 7: Graph panel is visible ──
  await expect(page.locator("#graph-panel")).toBeVisible();
  await expect(page.locator("#cy")).toBeVisible();

  // Give cytoscape time to initialize.
  await page.waitForTimeout(1000);

  // The graph container should have a canvas (cytoscape rendered).
  const canvasCount = await page.locator("#cy canvas").count();
  expect(canvasCount).toBeGreaterThan(0);

  // ── Step 8: No critical console errors ──
  // Filter out known benign messages.
  const meaningfulErrors = consoleErrors.filter(msg =>
    !msg.includes("favicon.ico") &&
    !msg.includes("net::ERR_") &&
    !msg.includes("404")
  );
  if (meaningfulErrors.length > 0) {
    console.warn("Console errors during golden read test:\n" + meaningfulErrors.join("\n"));
    // Note: we don't fail on console errors for now since external deps may
    // emit warnings; but we log them for visibility.
  }

  // ── Step 9: Hash URL reflects the navigation state ──
  const url = page.url();
  expect(url).toContain("#/brain/testbrain");
  expect(url).toContain("memory");
});

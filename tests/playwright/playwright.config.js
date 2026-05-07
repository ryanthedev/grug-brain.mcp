// @ts-check
const { defineConfig, devices } = require("@playwright/test");

/**
 * Playwright configuration for grug-brain viewer tests.
 *
 * The tests start the grug binary as a child process (see fixtures.js),
 * wait for the serve.port file to appear, then run against localhost:PORT.
 *
 * To run: from tests/playwright/, run `npm test` (or `npx playwright test`).
 * Requires the grug binary to be built first: `cargo build` from repo root.
 */
module.exports = defineConfig({
  testDir: ".",
  testMatch: ["**/*.spec.js"],
  timeout: 30000,
  expect: { timeout: 10000 },
  fullyParallel: false, // tests share the grug process via fixtures
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    // baseURL is set dynamically in each test via the fixture.
    headless: true,
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});

/**
 * Theme toggle — system / light / dark.
 *
 * Persisted in localStorage under key "grug-theme".
 * Cycles: system → light → dark → system.
 *
 * The graph.updateTheme() call in toggle() is guarded by a typeof check so
 * this module has no hard dependency on the graph module.
 */
export const theme = (() => {
  const MODES = ["system", "light", "dark"];
  const KEY = "grug-theme";

  function apply(mode) {
    const root = document.documentElement;
    if (mode === "dark") {
      root.dataset.theme = "dark";
    } else if (mode === "light") {
      root.dataset.theme = "light";
    } else {
      // System: check prefers-color-scheme.
      const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      root.dataset.theme = prefersDark ? "dark" : "light";
    }
    root.dataset.themeMode = mode;
  }

  function current() {
    return localStorage.getItem(KEY) || "system";
  }

  function init() {
    apply(current());
    // Respond to OS theme changes when in system mode.
    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      if (current() === "system") apply("system");
    });
  }

  /** Cycle: system → light → dark → system. */
  function toggle() {
    const idx = MODES.indexOf(current());
    const next = MODES[(idx + 1) % MODES.length];
    localStorage.setItem(KEY, next);
    apply(next);
    updateToggleLabel();
    // Notify graph to update its colors for the new theme.
    if (typeof graph !== "undefined" && graph.updateTheme) {
      graph.updateTheme();
    }
  }

  function updateToggleLabel() {
    const btn = document.getElementById("theme-toggle");
    if (!btn) return;
    const m = current();
    const icons = { system: "auto", light: "light", dark: "dark" };
    btn.textContent = icons[m] || "auto"; // static keys, safe
    btn.setAttribute("aria-label", `Switch theme (current: ${m})`);
  }

  return { init, toggle, updateToggleLabel };
})();

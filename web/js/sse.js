/**
 * SSE (Server-Sent Events) client — connects to /api/events and triggers
 * memory reloads when the server broadcasts file changes.
 *
 * Exported API:
 *   sse.connect() — open the SSE connection (reconnects on error)
 */
import { state } from './state.js';
import { RELOAD_DEBOUNCE_MS, SSE_RECONNECT_MS } from './constants.js';
import { autocomplete } from './autocomplete.js';
import { loadMemories, loadPreview } from './loaders.js';

export const sse = (() => {
  let es = null;
  let reconnectTimer = null;
  let reloadDebounce = null;

  function setStatus(status) {
    const dot = document.getElementById("sse-status");
    if (dot) {
      dot.className = status === "connected" ? "connected" :
                       status === "error" ? "error" : "";
    }
  }

  function showReloadIndicator() {
    const el = document.getElementById("reload-indicator");
    if (!el) return;
    el.classList.add("visible");
    setTimeout(() => el.classList.remove("visible"), 2000);
  }

  function scheduleReload() {
    if (reloadDebounce) clearTimeout(reloadDebounce);
    reloadDebounce = setTimeout(async () => {
      // Invalidate autocomplete tag cache on any reload.
      autocomplete.invalidate();
      const s = state.get();
      if (s.activeBrain) {
        await loadMemories(s.activeBrain);
        if (s.activeMemoryPath) {
          loadPreview(s.activeBrain, s.activeMemoryPath, null);
        }
      }
      showReloadIndicator();
      // Marker for Playwright SSE reload test.
      document.body.dataset.sseReloaded = Date.now();
    }, RELOAD_DEBOUNCE_MS);
  }

  function connect() {
    if (es) { es.close(); es = null; }
    try {
      es = new EventSource("/api/events");
      es.addEventListener("open", () => setStatus("connected"));
      es.addEventListener("memory", () => scheduleReload());
      es.addEventListener("message", () => scheduleReload());
      es.addEventListener("error", () => {
        setStatus("error");
        es.close();
        es = null;
        if (reconnectTimer) clearTimeout(reconnectTimer);
        reconnectTimer = setTimeout(connect, SSE_RECONNECT_MS);
      });
    } catch (_) {
      setStatus("error");
    }
  }

  return { connect };
})();

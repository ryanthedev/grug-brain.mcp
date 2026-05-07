/**
 * Accessible modal focus manager.
 *
 * modal.open(el, options) → { close }
 *
 *   el          — the modal DOM element (already in the document, hidden).
 *   options     — optional:
 *     focusTarget  — element to focus on open (defaults to first focusable child).
 *     onEscape     — called on Escape key (defaults to close()).
 *
 * Behavior:
 *   - Sets el.hidden = false and moves focus to focusTarget (or first focusable).
 *   - Tab cycles focus inside el (Shift+Tab wraps backwards).
 *   - Escape calls onEscape (default: close).
 *   - On close, restores focus to whatever was active before open.
 */
export const modal = (() => {
  const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

  function focusables(root) {
    return Array.from(root.querySelectorAll(FOCUSABLE))
      .filter(el => !el.disabled && !el.hidden && el.offsetParent !== null);
  }

  function open(el, options) {
    if (!el) return { close: () => {} };
    options = options || {};
    const prevFocus = document.activeElement;
    el.hidden = false;

    const initial = options.focusTarget || focusables(el)[0] || el;
    setTimeout(() => { try { initial.focus(); } catch (_) {} }, 0);

    function onKeydown(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        if (options.onEscape) options.onEscape();
        else close();
        return;
      }
      if (e.key === "Tab") {
        const list = focusables(el);
        if (list.length === 0) return;
        const first = list[0];
        const last = list[list.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault(); last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault(); first.focus();
        }
      }
    }

    document.addEventListener("keydown", onKeydown);

    function close() {
      el.hidden = true;
      document.removeEventListener("keydown", onKeydown);
      if (prevFocus && typeof prevFocus.focus === "function") {
        try { prevFocus.focus(); } catch (_) {}
      }
    }

    return { close };
  }

  return { open };
})();

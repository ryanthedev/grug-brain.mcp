/**
 * Toast notification component.
 *
 * toast.show(msg) / toast.error(msg) — error toast, auto-dismisses after 8s.
 * toast.success(msg)                 — success toast, auto-dismisses after 3s.
 *
 * Security: all message text inserted via textContent — no XSS risk.
 */
export const toast = (() => {
  /** Show an error toast. msg is a server error string — escaped before insertion. */
  function show(msg) {
    const container = document.getElementById("toast-container");
    if (!container) return;

    const el = document.createElement("div");
    el.className = "toast";
    el.setAttribute("role", "alert");
    el.setAttribute("aria-live", "assertive");

    const body = document.createElement("div");
    body.className = "toast-body";

    const title = document.createElement("div");
    title.className = "toast-title";
    title.textContent = "Error"; // static string, safe

    const message = document.createElement("div");
    message.className = "toast-message";
    message.textContent = msg; // textContent — no XSS risk

    body.appendChild(title);
    body.appendChild(message);

    const copyBtn = document.createElement("button");
    copyBtn.className = "toast-copy";
    copyBtn.setAttribute("aria-label", "Copy error to clipboard");
    copyBtn.textContent = "Copy"; // static
    copyBtn.addEventListener("click", () => {
      navigator.clipboard.writeText(msg).catch(() => {});
    });

    const closeBtn = document.createElement("button");
    closeBtn.className = "toast-close";
    closeBtn.setAttribute("aria-label", "Dismiss error");
    closeBtn.textContent = "×"; // × via unicode — no HTML injection
    closeBtn.addEventListener("click", () => el.remove());

    el.appendChild(body);
    el.appendChild(copyBtn);
    el.appendChild(closeBtn);
    container.appendChild(el);

    // Auto-dismiss after 8s.
    setTimeout(() => { if (el.parentNode) el.remove(); }, 8000);
  }

  /** Briefly show a success toast (auto-dismisses). */
  function success(msg) {
    const container = document.getElementById("toast-container");
    if (!container) return;
    const el = document.createElement("div");
    el.className = "toast toast-success";
    el.setAttribute("role", "status");
    el.setAttribute("aria-live", "polite");
    const body = document.createElement("div");
    body.className = "toast-body";
    const message = document.createElement("div");
    message.className = "toast-message";
    message.textContent = msg;
    body.appendChild(message);
    el.appendChild(body);
    container.appendChild(el);
    setTimeout(() => { if (el.parentNode) el.remove(); }, 3000);
  }

  return { show, success, error: show };
})();

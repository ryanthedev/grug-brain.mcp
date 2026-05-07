/**
 * Navigation guard — unsaved-changes modal before leaving.
 *
 * Exports:
 *   nav.init()   — wire up beforeunload + unsaved modal buttons
 *   nav.guard()  — returns Promise<boolean>; false = user cancelled
 */
import { state } from './state.js';
import { modal } from './modal.js';

export const nav = (() => {
  let pendingResolve = null;
  let unsavedHandle = null;

  function init() {
    window.addEventListener("beforeunload", e => {
      if (state.get().dirty) {
        e.preventDefault();
        e.returnValue = "";
      }
    });
    const cancel = document.getElementById("unsaved-cancel");
    const discard = document.getElementById("unsaved-discard");
    if (cancel) cancel.addEventListener("click", () => closeModal(false));
    if (discard) discard.addEventListener("click", () => closeModal(true));
  }

  function closeModal(result) {
    // Delegate to modal.open handle so prior-focus is restored.
    if (unsavedHandle) { unsavedHandle.close(); unsavedHandle = null; }
    if (pendingResolve) { pendingResolve(result); pendingResolve = null; }
  }

  function guard() {
    if (!state.get().dirty) return Promise.resolve(true);
    const el = document.getElementById("unsaved-modal");
    if (!el) return Promise.resolve(true);
    const cancel = document.getElementById("unsaved-cancel");
    // Use the generic modal helper so focus-trap + Escape + prior-focus restore
    // all work consistently with every other modal surface (DW-7.5).
    unsavedHandle = modal.open(el, {
      focusTarget: cancel,
      onEscape: () => closeModal(false),
    });
    return new Promise(res => { pendingResolve = res; });
  }

  return { init, guard };
})();

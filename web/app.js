/**
 * grug-brain viewer — entry point.
 *
 * All logic lives in web/js/*.js. This file:
 *   1. Exposes window.__grugState (Playwright hook, read-only).
 *   2. Calls boot() to wire DOM listeners and start the app.
 *
 * Module scripts are deferred (HTML parsed before module body runs), so the
 * DOM is always ready when boot() is called — no DOMContentLoaded guard.
 */
import { state } from './js/state.js';
import { boot } from './js/boot.js';

Object.defineProperty(window, '__grugState', {
  get: () => state.get(),
  configurable: true,
});

boot();

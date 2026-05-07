/**
 * Command registry — register and run named commands.
 *
 * Exports:
 *   commands.register(name, fn, opts) — register a command
 *   commands.run(name, ...args)        — execute; returns false if not found
 *   commands.list()                    — [{name, title, kind}]
 */
export const commands = (() => {
  // registry[name] = { fn, title, kind }
  const registry = {};

  function register(name, fn, opts) {
    const o = opts || {};
    registry[name] = {
      fn,
      title: o.title || name,
      kind: o.kind || "command",
    };
  }

  function run(name, ...args) {
    const e = registry[name];
    if (!e) return false;
    e.fn(...args);
    return true;
  }

  function list() {
    return Object.entries(registry).map(([name, e]) => ({
      name, title: e.title, kind: e.kind,
    }));
  }

  return { register, run, list };
})();

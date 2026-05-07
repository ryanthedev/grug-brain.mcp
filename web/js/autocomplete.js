/**
 * CodeMirror autocompletion for `[[wikilinks]]` and `#tags`.
 *
 * Triggers on typing `[[` (wikilink) or `#` (tag). Suggestions are pulled
 * from `state.memories` (already loaded for the active brain) and a cached
 * tag list fetched lazily from `/api/tags`. The cache is invalidated by SSE
 * Reload (sse.connect already calls render fns; we hook here for tags).
 *
 * Exported API:
 *   autocomplete.extension(CMns) — returns CM extension array for wikilink + tag completion
 *   autocomplete.invalidate()    — clear the tags cache
 */
import { state } from './state.js';
import { api } from './api.js';

export const autocomplete = (() => {
  let tagsCache = null;
  let tagsCacheBrain = null;

  async function ensureTags(brain) {
    if (tagsCache && tagsCacheBrain === brain) return tagsCache;
    const r = await api.tags(brain);
    tagsCache = (r.ok && Array.isArray(r.data)) ? r.data : [];
    tagsCacheBrain = brain;
    return tagsCache;
  }

  function invalidate() {
    tagsCache = null;
    tagsCacheBrain = null;
  }

  function memoryOptions() {
    const s = state.get();
    return (s.memories || []).map(m => ({
      label: m.name,
      type: "wikilink",
      // CM gives us the matched range (`from`..`to`); we insert the bare
      // name there. Trigger source already positions `from` after the `[[`.
      // After the bare name, also append `]]` to close the wikilink (and
      // overwrite any auto-paired `]]` immediately following the cursor).
      apply: (view, completion, from, to) => {
        const doc = view.state.doc;
        const tail = doc.sliceString(to, Math.min(to + 2, doc.length));
        const insert = m.name + (tail === "]]" ? "" : "]]");
        // Move cursor past the closing `]]`.
        const cursor = from + insert.length + (tail === "]]" ? 2 : 0);
        view.dispatch({
          changes: { from, to, insert },
          selection: { anchor: cursor },
        });
      },
    }));
  }

  function tagOptionsFromCache() {
    return (tagsCache || []).map(t => ({
      label: t.tag,
      detail: String(t.count),
      type: "tag",
      apply: (view, completion, from, to) => {
        // `from` already points to the position after the `#` trigger.
        view.dispatch({
          changes: { from, to, insert: t.tag },
          selection: { anchor: from + t.tag.length },
        });
      },
    }));
  }

  /**
   * Build the CodeMirror autocompletion extension. Two complete sources:
   *   1. Wikilink: trigger /\[\[([\w-]*)$/ — completes to `[[name]]`
   *   2. Tag:      trigger /(?:^|\s)#([\w-]*)$/ — completes to `#tag`
   * Both return null when no match (so other completion sources still work).
   */
  function extension(CMns) {
    function wikilinkSource(ctx) {
      const m = ctx.matchBefore(/\[\[[\w \-]*/);
      if (!m) return null;
      if (m.from === m.to && !ctx.explicit) return null;
      // `from` skips the `[[` so CM's built-in filter compares the typed
      // partial against option labels (which are bare names, not `[[name]]`).
      const from = m.from + 2;
      const options = memoryOptions();
      return {
        from,
        // The full match including `[[` is replaced by `[[name]]` per option.apply.
        // To make the replacement cover the `[[` itself, we set `from` BEFORE
        // them via the `apply` callback below.
        options,
        validFor: /^[\w \-]*$/,
      };
    }
    async function tagSource(ctx) {
      // Match the `#` plus any word chars; require start-of-line or whitespace before.
      const m = ctx.matchBefore(/(^|\s)#[\w-]*/);
      if (!m) return null;
      const text = m.text;
      // `from` after the `#` so CM filters the partial against tag names.
      const hashIdx = text.lastIndexOf("#");
      const from = m.from + hashIdx + 1;
      if (from === ctx.pos && !ctx.explicit) return null;
      const s = state.get();
      await ensureTags(s.activeBrain);
      return {
        from,
        options: tagOptionsFromCache(),
        validFor: /^[\w-]*$/,
      };
    }
    return [
      CMns.autocompletion({
        override: [wikilinkSource, tagSource],
        activateOnTyping: true,
      }),
    ];
  }

  return { extension, invalidate };
})();

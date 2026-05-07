/**
 * Editor — CodeMirror 6 wrapper.
 *
 * Uses a module-private `editorView` variable. On mount, sets both
 * `editorView` and `window.__grugEditorView` (Playwright test hook).
 * On unmount, both are cleared to null.
 *
 * Circular-import note: editor.js ↔ save.js — editor imports save for the
 * Cmd-S keymap; save imports editor for getView(). All cross-references
 * are inside function bodies (runtime-only), so the cycle is safe for
 * native ES modules.
 *
 * Exported API:
 *   editor.mount(container, doc, onChange) — mount CodeMirror into container
 *   editor.unmount()                       — destroy and clean up
 *   editor.setDoc(text)                    — replace editor content
 *   editor.getView()                       — return current EditorView (or null)
 *   getView()                              — named export for outline.js import
 */
import { save } from './save.js';
import { autocomplete } from './autocomplete.js';

/** Module-private reference to the live EditorView. */
let editorView = null;

/** Return the current EditorView instance, or null when not mounted. */
export function getView() { return editorView; }

export const editor = (() => {
  function buildDecorationsPlugin(CMns) {
    const wikilinkRe = /\[\[[^\]\n]+\]\]/g;
    const tagRe = /(^|\s)(#[A-Za-z][\w-]*)/g;

    function buildCombined(view) {
      const wikilinkDeco = CMns.Decoration.mark({ class: "cm-wikilink" });
      const tagDeco = CMns.Decoration.mark({ class: "cm-tag" });
      const marks = [];
      for (const r of view.visibleRanges) {
        const text = view.state.doc.sliceString(r.from, r.to);
        let m;
        wikilinkRe.lastIndex = 0;
        while ((m = wikilinkRe.exec(text)) !== null) {
          marks.push({ from: r.from + m.index, to: r.from + m.index + m[0].length, deco: wikilinkDeco });
        }
        tagRe.lastIndex = 0;
        while ((m = tagRe.exec(text)) !== null) {
          const s2 = r.from + m.index + m[1].length;
          const e2 = s2 + m[2].length;
          marks.push({ from: s2, to: e2, deco: tagDeco });
        }
      }
      marks.sort((a, b) => a.from - b.from || a.to - b.to);
      const b = new CMns.RangeSetBuilder();
      let lastTo = -1;
      for (const m of marks) {
        if (m.from < lastTo) continue;
        b.add(m.from, m.to, m.deco);
        lastTo = m.to;
      }
      return b.finish();
    }

    return CMns.ViewPlugin.fromClass(
      class {
        constructor(view) { this.decorations = buildCombined(view); }
        update(u) {
          if (u.docChanged || u.viewportChanged) {
            this.decorations = buildCombined(u.view);
          }
        }
      },
      { decorations: v => v.decorations }
    );
  }

  function saveKeymap(CMns) {
    return CMns.keymap.of([
      { key: "Mod-s", preventDefault: true, run: () => { save.run(); return true; } },
    ]);
  }

  function mount(container, doc, onChange) {
    if (typeof CM === "undefined") {
      const ta = document.createElement("textarea");
      ta.value = doc;
      ta.style.cssText = "width:100%;height:100%;font-family:var(--font-mono);";
      ta.addEventListener("input", () => onChange(ta.value));
      container.appendChild(ta);
      editorView = { _fallback: true, dom: ta, state: { doc: { toString: () => ta.value, length: ta.value.length } } };
      window.__grugEditorView = editorView;
      return editorView;
    }

    const updateListener = CM.EditorView.updateListener.of(u => {
      if (u.docChanged) onChange(u.state.doc.toString());
    });

    const extensions = [
      CM.basicSetup,
      CM.markdown(),
      buildDecorationsPlugin(CM),
      saveKeymap(CM),
      updateListener,
      CM.EditorView.theme({
        "&": { height: "100%" },
        ".cm-scroller": { fontFamily: "var(--font-mono)" },
      }),
    ];
    // Wikilink + tag autocomplete. Bundle was re-rolled to include
    // @codemirror/autocomplete; CM.autocompletion is the runtime probe.
    if (CM.autocompletion && autocomplete) {
      try {
        extensions.push(...autocomplete.extension(CM));
        window.__grugACWired = true;
      } catch (e) {
        window.__grugACError = String(e);
      }
    }
    const startState = CM.EditorState.create({ doc, extensions });

    const view = new CM.EditorView({ state: startState, parent: container });
    editorView = view;
    window.__grugEditorView = view;
    return view;
  }

  function unmount() {
    if (editorView) {
      if (editorView._fallback && editorView.dom && editorView.dom.parentNode) {
        editorView.dom.parentNode.removeChild(editorView.dom);
      } else if (editorView.destroy) {
        editorView.destroy();
      }
    }
    editorView = null;
    window.__grugEditorView = null;
  }

  function setDoc(text) {
    if (!editorView) return;
    if (editorView._fallback) { editorView.dom.value = text; return; }
    editorView.dispatch({
      changes: { from: 0, to: editorView.state.doc.length, insert: text },
    });
  }

  return { mount, unmount, setDoc, getView };
})();

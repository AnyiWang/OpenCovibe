<script lang="ts">
  import { onMount } from "svelte";
  import {
    EditorView,
    lineNumbers,
    highlightActiveLineGutter,
    highlightActiveLine,
    keymap,
  } from "@codemirror/view";
  import { EditorState, Compartment, type Extension } from "@codemirror/state";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import {
    bracketMatching,
    foldGutter,
    syntaxHighlighting,
    defaultHighlightStyle,
    LanguageDescription,
  } from "@codemirror/language";
  import { languages } from "@codemirror/language-data";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { fileName } from "$lib/utils/format";
  import { resolveStaticLanguage, resolveByFirstLine } from "$lib/utils/codemirror-languages";

  let {
    content = $bindable(""),
    filePath = "",
    readonly = false,
    onsave,
    class: className = "",
  }: {
    content: string;
    filePath?: string;
    readonly?: boolean;
    onsave?: () => void;
    class?: string;
  } = $props();

  let editorEl: HTMLDivElement | undefined = $state();
  let view: EditorView | undefined = $state();
  let updating = false;

  const themeCompartment = new Compartment();
  const langCompartment = new Compartment();
  const readonlyCompartment = new Compartment();

  /** Race condition guard: only apply the latest language resolution. */
  let langSeq = 0;

  /**
   * Resolve language extensions for a file path.
   *
   * 1. Static mapping (sync) — covers ~20 common languages
   * 2. Dynamic fallback via @codemirror/language-data (async, with try/catch)
   * 3. Returns [] on failure (plain text, never throws)
   */
  async function resolveLanguage(path: string): Promise<Extension[]> {
    const name = fileName(path);

    // 1. Static mapping (sync — no dynamic chunk loading)
    const staticResult = resolveStaticLanguage(name);
    if (staticResult) {
      dbg("code-editor", "static-hit", { name });
      return staticResult;
    }

    // 2. Dynamic fallback: language-data auto-detection
    const desc = LanguageDescription.matchFilename(languages, name);
    if (desc) {
      try {
        const support = await desc.load();
        dbg("code-editor", "dynamic-hit", { name, lang: desc.name });
        return [support];
      } catch (e) {
        dbgWarn("code-editor", "dynamic-failed", { name, lang: desc.name, error: e });
        // Fall through to first-line detection below
      }
    }

    // 3. First-line detection (shebang, XML declaration, JSON brace)
    if (content) {
      const firstLine = content.trimStart().split("\n")[0] ?? "";
      const guess = resolveByFirstLine(firstLine);
      if (guess) {
        dbg("code-editor", "first-line-hit", { name, firstLine: firstLine.slice(0, 40) });
        return guess;
      }
    }

    dbg("code-editor", "plain-text-fallback", { name });
    return [];
  }

  /** Check if syntax highlighting styles are actually applied after language loads. Run once. */
  let styleCheckDone = false;
  function verifySyntaxStyles(v: EditorView) {
    if (styleCheckDone) return;
    styleCheckDone = true;
    // Give parser time to tokenize + style-mod to inject CSS
    requestAnimationFrame(() => {
      if (!v.dom.isConnected) return;
      const baseColor = getComputedStyle(v.contentDOM).color;
      // Sample up to 20 token spans — enough to detect missing styles without perf cost
      const spans = v.contentDOM.querySelectorAll(".cm-line span");
      const limit = Math.min(spans.length, 20);
      let hasHighlight = false;
      for (let i = 0; i < limit; i++) {
        if (getComputedStyle(spans[i]).color !== baseColor) {
          hasHighlight = true;
          break;
        }
      }
      if (limit > 0 && !hasHighlight) {
        dbgWarn("code-editor", "style-injection-failed", {
          baseColor,
          sampledSpans: limit,
          msg: "Language loaded but no token has distinct color — styles may not be injected",
        });
      }
    });
  }

  function isDarkMode(): boolean {
    return typeof document !== "undefined" && document.documentElement.classList.contains("dark");
  }

  onMount(() => {
    if (!editorEl) return;

    const dark = isDarkMode();
    dbg("code-editor", "mount", { filePath, readonly, dark });

    const state = EditorState.create({
      doc: content,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        bracketMatching(),
        foldGutter(),
        history(),
        keymap.of([
          {
            key: "Mod-s",
            run: () => {
              onsave?.();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        readonlyCompartment.of([
          EditorView.editable.of(!readonly),
          EditorState.readOnly.of(readonly),
        ]),
        themeCompartment.of(dark ? oneDark : []),
        langCompartment.of([]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && !updating) {
            updating = true;
            content = update.state.doc.toString();
            updating = false;
          }
        }),
      ],
    });

    view = new EditorView({ state, parent: editorEl });

    // Load language support
    const seq = ++langSeq;
    resolveLanguage(filePath).then((lang) => {
      if (seq !== langSeq || !view) return; // stale — user already switched files
      view.dispatch({ effects: langCompartment.reconfigure(lang) });
      if (lang.length > 0) verifySyntaxStyles(view);
    });

    // Watch dark mode changes via MutationObserver on <html> class
    const observer = new MutationObserver(() => {
      if (!view) return;
      const dark = isDarkMode();
      view.dispatch({
        effects: themeCompartment.reconfigure(dark ? oneDark : []),
      });
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => {
      observer.disconnect();
      view?.destroy();
      view = undefined;
    };
  });

  // Sync external content changes into CM6
  $effect(() => {
    if (view && !updating && content !== view.state.doc.toString()) {
      updating = true;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: content },
      });
      updating = false;
    }
  });

  // Reconfigure language when filePath changes
  $effect(() => {
    if (!view) return;
    const _path = filePath; // track dependency
    const seq = ++langSeq;
    resolveLanguage(_path).then((lang) => {
      if (seq !== langSeq || !view) return; // stale — user already switched files
      view.dispatch({ effects: langCompartment.reconfigure(lang) });
      if (lang.length > 0) verifySyntaxStyles(view);
    });
  });

  // Reconfigure readonly when prop changes
  $effect(() => {
    if (!view) return;
    view.dispatch({
      effects: readonlyCompartment.reconfigure([
        EditorView.editable.of(!readonly),
        EditorState.readOnly.of(readonly),
      ]),
    });
  });
</script>

<div bind:this={editorEl} class="code-editor-wrapper {className}"></div>

<style>
  .code-editor-wrapper {
    overflow: hidden;
  }
  .code-editor-wrapper :global(.cm-editor) {
    height: 100%;
  }
  /* scroller flex layout is enforced globally in app.css */
</style>

<script lang="ts">
  import { onMount } from "svelte";
  import {
    EditorView,
    lineNumbers,
    highlightActiveLineGutter,
    highlightActiveLine,
    keymap,
  } from "@codemirror/view";
  import { EditorState, Compartment } from "@codemirror/state";
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
  import { dbg } from "$lib/utils/debug";
  import { fileName } from "$lib/utils/format";

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

  // Fallback: map filenames/patterns that @codemirror/language-data doesn't cover
  // to a known language name in the languages array.
  const filenameFallbacks: Record<string, string> = {
    ".gitignore": "Shell",
    ".dockerignore": "Shell",
    ".npmignore": "Shell",
    ".prettierignore": "Shell",
    ".eslintignore": "Shell",
    ".env": "Shell",
    ".env.local": "Shell",
    ".env.example": "Shell",
    ".prettierrc": "JSON",
    ".eslintrc": "JSON",
    ".babelrc": "JSON",
    ".swcrc": "JSON",
    Makefile: "Shell",
    GNUmakefile: "Shell",
  };
  // Extension-based fallback for types language-data misses
  const extFallbacks: Record<string, string> = {
    lock: "TOML", // Cargo.lock
    env: "Shell",
    conf: "Shell",
    cfg: "Shell",
    ini: "Shell",
    properties: "Shell",
    editorconfig: "Shell",
  };

  /** Resolve language support for a file path.
   *  1. Try @codemirror/language-data (matchFilename — covers extensions + Dockerfile etc.)
   *  2. Fall back to filename/extension mapping for dotfiles and config files.
   *  Returns a promise — language modules are loaded on demand. */
  async function loadLanguage(path: string) {
    const filename = fileName(path);

    // Primary: language-data auto-detection
    let desc = LanguageDescription.matchFilename(languages, filename);

    // Fallback: filename match
    if (!desc) {
      const fallbackLang = filenameFallbacks[filename];
      if (fallbackLang) {
        desc = languages.find((l) => l.name === fallbackLang) ?? null;
      }
    }

    // Fallback: extension match (for .lock, .env.*, .editorconfig, etc.)
    if (!desc) {
      const ext = filename.split(".").pop()?.toLowerCase() ?? "";
      const fallbackLang = extFallbacks[ext];
      if (fallbackLang) {
        desc = languages.find((l) => l.name === fallbackLang) ?? null;
      }
    }

    // Fallback: first-line detection (shebang, XML declaration, JSON braces)
    if (!desc && content) {
      const firstLine = content.trimStart().split("\n")[0] ?? "";
      let guessLang: string | null = null;
      if (/^#!.*\b(bash|sh|zsh)\b/.test(firstLine)) guessLang = "Shell";
      else if (/^#!.*\b(python|python3)\b/.test(firstLine)) guessLang = "Python";
      else if (/^#!.*\bnode\b/.test(firstLine)) guessLang = "JavaScript";
      else if (/^<\?xml\b/.test(firstLine)) guessLang = "XML";
      else if (/^<!DOCTYPE\s+html/i.test(firstLine) || /^<html/i.test(firstLine))
        guessLang = "HTML";
      else if (/^\s*[{[]/.test(firstLine)) guessLang = "JSON";
      if (guessLang) {
        desc = languages.find((l) => l.name === guessLang) ?? null;
      }
    }

    if (!desc) {
      dbg("code-editor", "no language match", { filename });
      return [];
    }
    dbg("code-editor", "language matched", { filename, lang: desc.name });
    const support = await desc.load();
    return support;
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
        EditorView.editable.of(!readonly),
        EditorState.readOnly.of(readonly),
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

    // Load language support async
    loadLanguage(filePath).then((lang) => {
      if (view) {
        view.dispatch({ effects: langCompartment.reconfigure(lang) });
      }
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
    loadLanguage(_path).then((lang) => {
      if (view) {
        view.dispatch({ effects: langCompartment.reconfigure(lang) });
      }
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
  .code-editor-wrapper :global(.cm-scroller) {
    overflow: auto;
  }
</style>

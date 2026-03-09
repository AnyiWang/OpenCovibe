<script lang="ts">
  import { onMount } from "svelte";
  import { codeToHtml, type BundledLanguage, bundledLanguages } from "shiki";

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

  let textareaEl: HTMLTextAreaElement | undefined = $state();
  let highlightedHtml = $state("");
  let renderTimer: ReturnType<typeof setTimeout> | undefined;
  let renderSeq = 0;

  // ── Language resolution ──

  function resolveLanguage(path: string): string {
    const name = path.split("/").pop() ?? "";
    const ext = name.split(".").pop()?.toLowerCase() ?? "";

    const nameMap: Record<string, string> = {
      Makefile: "makefile",
      GNUmakefile: "makefile",
      Dockerfile: "dockerfile",
      Containerfile: "dockerfile",
      Jenkinsfile: "groovy",
      Vagrantfile: "ruby",
      Gemfile: "ruby",
      Rakefile: "ruby",
      ".gitignore": "shellscript",
      ".dockerignore": "shellscript",
      ".env": "dotenv",
      ".prettierrc": "json",
      ".eslintrc": "json",
      ".babelrc": "json",
      ".swcrc": "json",
    };
    if (nameMap[name]) return nameMap[name];
    if (/^\.env\..+$/.test(name)) return "dotenv";

    const extMap: Record<string, string> = {
      ts: "typescript",
      mts: "typescript",
      cts: "typescript",
      tsx: "tsx",
      js: "javascript",
      mjs: "javascript",
      cjs: "javascript",
      jsx: "jsx",
      json: "json",
      jsonc: "jsonc",
      json5: "json5",
      toml: "toml",
      lock: "toml",
      md: "markdown",
      markdown: "markdown",
      mdx: "mdx",
      html: "html",
      htm: "html",
      xml: "xml",
      svg: "xml",
      xsl: "xml",
      css: "css",
      scss: "scss",
      sass: "sass",
      less: "less",
      styl: "stylus",
      py: "python",
      pyw: "python",
      pyi: "python",
      rs: "rust",
      go: "go",
      java: "java",
      kt: "kotlin",
      kts: "kotlin",
      swift: "swift",
      c: "c",
      h: "c",
      cpp: "cpp",
      cc: "cpp",
      cxx: "cpp",
      hpp: "cpp",
      hxx: "cpp",
      cs: "csharp",
      rb: "ruby",
      php: "php",
      lua: "lua",
      r: "r",
      R: "r",
      pl: "perl",
      pm: "perl",
      dart: "dart",
      zig: "zig",
      nim: "nim",
      ex: "elixir",
      exs: "elixir",
      erl: "erlang",
      hrl: "erlang",
      hs: "haskell",
      ml: "ocaml",
      mli: "ocaml",
      fs: "fsharp",
      fsx: "fsharp",
      scala: "scala",
      clj: "clojure",
      cljs: "clojure",
      groovy: "groovy",
      gradle: "groovy",
      yaml: "yaml",
      yml: "yaml",
      sql: "sql",
      sh: "shellscript",
      bash: "shellscript",
      zsh: "shellscript",
      ksh: "shellscript",
      fish: "fish",
      ps1: "powershell",
      psm1: "powershell",
      bat: "bat",
      cmd: "bat",
      diff: "diff",
      patch: "diff",
      dockerfile: "dockerfile",
      tf: "hcl",
      hcl: "hcl",
      nix: "nix",
      vim: "viml",
      svelte: "svelte",
      vue: "vue",
      astro: "astro",
      graphql: "graphql",
      gql: "graphql",
      proto: "proto",
      prisma: "prisma",
      env: "dotenv",
      ini: "ini",
      conf: "ini",
      cfg: "ini",
      properties: "properties",
      tex: "latex",
      latex: "latex",
      makefile: "makefile",
      cmake: "cmake",
    };

    return extMap[ext] ?? "plaintext";
  }

  function isDarkMode(): boolean {
    return typeof document !== "undefined" && document.documentElement.classList.contains("dark");
  }

  // ── Shiki rendering ──

  async function renderHighlight(code: string, path: string) {
    const seq = ++renderSeq;
    const langId = resolveLanguage(path);
    const theme = isDarkMode() ? "github-dark" : "github-light";
    const lang = (langId in bundledLanguages ? langId : "plaintext") as BundledLanguage;

    try {
      const html = await codeToHtml(code, { lang, theme });
      if (seq !== renderSeq) return;
      highlightedHtml = html;
    } catch {
      if (seq !== renderSeq) return;
      try {
        const html = await codeToHtml(code, { lang: "plaintext", theme });
        if (seq !== renderSeq) return;
        highlightedHtml = html;
      } catch {
        if (seq !== renderSeq) return;
        highlightedHtml = `<pre class="shiki"><code>${escapeHtml(code)}</code></pre>`;
      }
    }
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  function scheduleRender() {
    clearTimeout(renderTimer);
    renderTimer = setTimeout(() => renderHighlight(content, filePath), 100);
  }

  // ── Input handling ──

  function handleInput(e: Event) {
    const ta = e.target as HTMLTextAreaElement;
    content = ta.value;
    scheduleRender();
  }

  function handleKeydown(e: KeyboardEvent) {
    const ta = e.target as HTMLTextAreaElement;

    // Tab → insert 2 spaces
    if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      const start = ta.selectionStart;
      const end = ta.selectionEnd;
      content = content.substring(0, start) + "  " + content.substring(end);
      scheduleRender();
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = start + 2;
      });
    }

    // Ctrl/Cmd+S → save
    if (e.key === "s" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      onsave?.();
    }
  }

  function syncScroll() {
    const ta = textareaEl;
    const pre = ta?.parentElement?.querySelector(".shiki-highlight") as HTMLElement | null;
    if (ta && pre) {
      pre.scrollTop = ta.scrollTop;
      pre.scrollLeft = ta.scrollLeft;
    }
  }

  // Render on content/filePath change
  $effect(() => {
    if (content !== undefined) {
      renderHighlight(content, filePath);
    }
  });

  // Dark mode observer
  onMount(() => {
    const observer = new MutationObserver(() => renderHighlight(content, filePath));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => {
      observer.disconnect();
      clearTimeout(renderTimer);
    };
  });
</script>

<div class="shiki-editor {className}" class:readonly>
  <!-- Highlighted layer (behind) -->
  <div class="shiki-highlight" aria-hidden="true">
    {@html highlightedHtml}
  </div>
  <!-- Input layer (on top, transparent text) -->
  {#if !readonly}
    <textarea
      bind:this={textareaEl}
      value={content}
      oninput={handleInput}
      onkeydown={handleKeydown}
      onscroll={syncScroll}
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      class="shiki-input"
    ></textarea>
  {/if}
</div>

<style>
  .shiki-editor {
    position: relative;
    overflow: hidden;
    height: 100%;
    font-family:
      ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
    font-size: 13px;
    line-height: 1.6;
  }

  .shiki-highlight,
  .shiki-input {
    position: absolute;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    overflow: auto;
    margin: 0;
    padding: 0.75rem 1rem;
    border: none;
    white-space: pre;
    word-wrap: normal;
    font: inherit;
    tab-size: 2;
    box-sizing: border-box;
  }

  .shiki-highlight {
    pointer-events: none;
  }

  .shiki-highlight :global(.shiki) {
    margin: 0;
    padding: 0;
    background: transparent !important;
    font: inherit;
    line-height: inherit;
  }

  .shiki-highlight :global(.shiki code) {
    font: inherit;
    line-height: inherit;
    counter-reset: line;
  }

  .shiki-highlight :global(.shiki code .line) {
    display: inline;
  }

  .shiki-input {
    color: transparent;
    caret-color: #000;
    background: transparent;
    resize: none;
    outline: none;
    z-index: 1;
    -webkit-text-fill-color: transparent;
    cursor: text;
  }

  :global(.dark) .shiki-input {
    caret-color: #e5e5e5;
  }

  /* Focus indicator on editor container */
  .shiki-editor:has(.shiki-input:focus) {
    outline: 2px solid hsl(var(--primary, 220 90% 56%) / 0.5);
    outline-offset: -2px;
  }

  /* Readonly mode: no textarea, highlight layer is scrollable */
  .shiki-editor.readonly .shiki-highlight {
    pointer-events: auto;
    position: relative;
    height: 100%;
  }
</style>

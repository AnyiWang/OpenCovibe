<script lang="ts">
  import { onMount } from "svelte";
  import { codeToHtml, type BundledLanguage, bundledLanguages } from "shiki";
  import { dbg } from "$lib/utils/debug";

  let {
    content = "",
    filePath = "",
    class: className = "",
  }: {
    content: string;
    filePath?: string;
    class?: string;
  } = $props();

  let containerEl: HTMLDivElement | undefined = $state();
  let htmlContent = $state("");
  let rendering = $state(false);

  /** Map file extension/name to Shiki language ID. */
  function resolveLanguage(path: string): string {
    const name = path.split("/").pop() ?? "";
    const ext = name.split(".").pop()?.toLowerCase() ?? "";

    // Exact filename matches
    const nameMap: Record<string, string> = {
      Makefile: "makefile",
      GNUmakefile: "makefile",
      Dockerfile: "dockerfile",
      Containerfile: "dockerfile",
      Jenkinsfile: "groovy",
      Vagrantfile: "ruby",
      Gemfile: "ruby",
      Rakefile: "ruby",
      ".gitignore": "gitignore",
      ".dockerignore": "dockerignore",
      ".env": "dotenv",
      ".prettierrc": "json",
      ".eslintrc": "json",
      ".babelrc": "json",
      ".swcrc": "json",
    };
    if (nameMap[name]) return nameMap[name];
    if (/^\.env\..+$/.test(name)) return "dotenv";

    // Extension map
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
      el: "lisp",
      svelte: "svelte",
      vue: "vue",
      "vue-html": "vue-html",
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
      lock: "toml",
      tex: "latex",
      latex: "latex",
      rst: "rst",
      csv: "csv",
      tsv: "csv",
      makefile: "makefile",
      cmake: "cmake",
      asm: "asm",
      s: "asm",
      wasm: "wasm",
      wat: "wasm",
    };

    return extMap[ext] ?? "plaintext";
  }

  function isDarkMode(): boolean {
    return typeof document !== "undefined" && document.documentElement.classList.contains("dark");
  }

  let renderSeq = 0;

  async function renderCode(code: string, path: string) {
    const seq = ++renderSeq;
    rendering = true;

    const langId = resolveLanguage(path);
    const dark = isDarkMode();
    const theme = dark ? "github-dark" : "github-light";

    // Check if Shiki supports this language
    const lang = (langId in bundledLanguages ? langId : "plaintext") as BundledLanguage;

    try {
      const html = await codeToHtml(code, {
        lang,
        theme,
      });
      if (seq !== renderSeq) return; // stale
      htmlContent = html;
      dbg("code-preview", "rendered", { path, lang, theme, lines: code.split("\n").length });
    } catch (e) {
      if (seq !== renderSeq) return;
      // Fallback: render as plaintext
      try {
        const html = await codeToHtml(code, { lang: "plaintext", theme });
        if (seq !== renderSeq) return;
        htmlContent = html;
      } catch {
        if (seq !== renderSeq) return;
        htmlContent = `<pre class="shiki"><code>${code.replace(/</g, "&lt;").replace(/>/g, "&gt;")}</code></pre>`;
      }
      dbg("code-preview", "fallback-plaintext", { path, lang, error: e });
    } finally {
      if (seq === renderSeq) rendering = false;
    }
  }

  // Render on content or filePath change
  $effect(() => {
    if (content !== undefined && filePath !== undefined) {
      renderCode(content, filePath);
    }
  });

  // Re-render on dark mode change
  onMount(() => {
    const observer = new MutationObserver(() => {
      renderCode(content, filePath);
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  });
</script>

<div bind:this={containerEl} class="code-preview-wrapper {className}">
  {#if rendering && !htmlContent}
    <div class="flex items-center justify-center py-12">
      <div
        class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
    </div>
  {:else}
    <div class="code-preview-content">
      {@html htmlContent}
    </div>
  {/if}
</div>

<style>
  .code-preview-wrapper {
    overflow: auto;
    height: 100%;
  }
  .code-preview-content :global(.shiki) {
    margin: 0;
    padding: 0.75rem 1rem;
    font-size: 13px;
    line-height: 1.6;
    overflow-x: auto;
    min-height: 100%;
  }
  .code-preview-content :global(.shiki code) {
    counter-reset: line;
  }
  .code-preview-content :global(.shiki code > span) {
    display: inline;
  }
  .code-preview-content :global(.shiki code .line) {
    display: inline-block;
    width: 100%;
  }
  .code-preview-content :global(.shiki code .line::before) {
    counter-increment: line;
    content: counter(line);
    display: inline-block;
    width: 3em;
    margin-right: 1em;
    text-align: right;
    color: var(--shiki-line-number, rgba(128, 128, 128, 0.4));
    user-select: none;
  }
</style>

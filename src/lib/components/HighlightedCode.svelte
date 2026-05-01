<script lang="ts">
  import hljs from "highlight.js/lib/core";
  // Side-effect: ensure hljs language registrations from markdown.ts have run.
  // markdown.ts registers js/ts/py/rust/go/cpp/yaml/json/sql/bash/etc at module load.
  import "$lib/utils/markdown";
  import { getExtension } from "$lib/utils/preview-ext";
  import { perfMark } from "$lib/utils/perf";

  let {
    content = "",
    filePath = "",
    class: className = "",
  }: {
    content?: string;
    filePath?: string;
    class?: string;
  } = $props();

  /** Map file extension → hljs language name. Undefined for unmapped extensions
   *  (caller falls back to `highlightAuto` which scans the content). */
  function langForPath(p: string): string | undefined {
    const ext = getExtension(p);
    const map: Record<string, string> = {
      ts: "typescript",
      tsx: "typescript",
      mts: "typescript",
      cts: "typescript",
      js: "javascript",
      jsx: "javascript",
      mjs: "javascript",
      cjs: "javascript",
      py: "python",
      rs: "rust",
      go: "go",
      java: "java",
      c: "cpp",
      cpp: "cpp",
      cc: "cpp",
      cxx: "cpp",
      h: "cpp",
      hpp: "cpp",
      json: "json",
      jsonc: "json",
      yaml: "yaml",
      yml: "yaml",
      sql: "sql",
      sh: "bash",
      bash: "bash",
      zsh: "bash",
      md: "markdown",
      markdown: "markdown",
      html: "xml",
      htm: "xml",
      xml: "xml",
      svg: "xml",
      css: "css",
      diff: "diff",
    };
    return map[ext];
  }

  function escapeHtml(s: string): string {
    return s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c] ?? c);
  }

  let highlighted = $derived.by(() => {
    if (!content) return "";
    return perfMark(
      "hljs-highlight",
      () => {
        const lang = langForPath(filePath);
        try {
          if (lang && hljs.getLanguage(lang)) {
            return hljs.highlight(content, { language: lang }).value;
          }
          // Fallback: let hljs auto-detect language. Slower than direct highlight but
          // produces reasonable output for unmapped extensions.
          return hljs.highlightAuto(content).value;
        } catch {
          // hljs failure: return escaped plain text so we never crash on bad content.
          return escapeHtml(content);
        }
      },
      { chars: content.length, lang: langForPath(filePath) ?? "auto" },
    );
  });
</script>

<pre
  class="hljs h-full overflow-auto m-0 p-3 text-xs font-mono leading-relaxed whitespace-pre {className}"><code
    >{@html highlighted}</code
  ></pre>

import { Marked } from "marked";
import { escapeHtml } from "$lib/utils/ansi";
import { perfMark } from "$lib/utils/perf";
import { hljs } from "$lib/utils/hljs-init";
import DOMPurify from "dompurify";
import "highlight.js/styles/github-dark.min.css";

const marked = new Marked();

marked.use({
  gfm: true,
  breaks: false,
  renderer: {
    // marked v15: table(token) receives a Token with header[] and rows[][]
    table(token: {
      header: Array<{ tokens: unknown[]; align: string | null; header: boolean }>;
      rows: Array<Array<{ tokens: unknown[]; align: string | null; header: boolean }>>;
    }) {
      // Build header cells
      let headerCells = "";
      for (const cell of token.header) {
        const content = this.parser.parseInline(cell.tokens);
        const tag = cell.align ? `<th align="${cell.align}">` : "<th>";
        headerCells += `${tag}${content}</th>\n`;
      }
      const headerRow = `<tr>\n${headerCells}</tr>\n`;

      // Build body rows
      let body = "";
      for (const row of token.rows) {
        let rowCells = "";
        for (const cell of row) {
          const content = this.parser.parseInline(cell.tokens);
          const tag = cell.align ? `<td align="${cell.align}">` : "<td>";
          rowCells += `${tag}${content}</td>\n`;
        }
        body += `<tr>\n${rowCells}</tr>\n`;
      }
      if (body) body = `<tbody>${body}</tbody>`;

      return `<div class="table-wrapper"><table><thead>${headerRow}</thead>${body}</table></div>`;
    },
    code({ text, lang }: { text: string; lang?: string }) {
      const language = lang || "";
      let highlighted: string;

      if (language && hljs.getLanguage(language)) {
        try {
          highlighted = hljs.highlight(text, { language }).value;
        } catch {
          highlighted = escapeHtml(text);
        }
      } else {
        // Skip highlightAuto() — it tries all ~190 languages synchronously
        // and can freeze the UI for seconds on large code blocks
        highlighted = escapeHtml(text);
      }

      const displayLang = language || "text";

      return `<div class="code-block"><div class="code-block-header"><span class="code-block-lang">${escapeHtml(displayLang)}</span><button class="code-block-copy" data-code-copy>Copy</button></div><pre><code class="hljs language-${escapeHtml(language)}">${highlighted}</code></pre></div>`;
    },
  },
});

export function renderMarkdown(text: string): string {
  return perfMark(
    "md-render",
    () => {
      const raw = marked.parse(text);
      if (typeof raw !== "string") return "";
      return DOMPurify.sanitize(raw, {
        ADD_ATTR: ["class", "target", "data-code-copy"],
      });
    },
    { chars: text.length, codeFenceCount: text.match(/```/g)?.length ?? 0 },
  );
}

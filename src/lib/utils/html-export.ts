/**
 * HTML Export — DOM snapshot + CSS inline for self-contained conversation export.
 *
 * Clones the rendered conversation DOM, inlines all stylesheets, converts blob images
 * to data URLs, and wraps into a standalone HTML document preserving visual fidelity.
 */
import { dbg, dbgWarn } from "$lib/utils/debug";

// ── Public interface ──

export interface ExportOptions {
  title: string;
  sessionInfo: {
    model?: string;
    cwd?: string;
    startedAt: string;
    turnCount: number;
  };
  selectedEntryIds?: Set<string>; // Phase 2
}

/**
 * Export the conversation DOM as a self-contained HTML string.
 * Caller must ensure the rootEl contains the full conversation
 * (clear toolFilter / renderLimit and await tick before calling).
 */
export async function exportConversationToHtml(
  rootEl: HTMLElement,
  options: ExportOptions,
): Promise<string> {
  dbg("html-export", "start", {
    title: options.title,
    turns: options.sessionInfo.turnCount,
  });

  // 1. Clone
  const clone = rootEl.cloneNode(true) as HTMLElement;

  // 2. Clean up
  await cleanupClonedDom(clone, options.selectedEntryIds);

  // 3. Extract CSS
  const styles = extractAllStyles();

  // 4. Theme + lang
  const themeClasses = document.documentElement.className;

  // 5. Assemble
  const html = wrapAsStandaloneHtml({
    body: clone.innerHTML,
    styles,
    themeClasses,
    options,
  });

  dbg("html-export", "done", { htmlLen: html.length });
  return html;
}

/** Build a sanitized default filename for the export. */
export function buildExportFilename(title: string): string {
  return `opencovibe-${sanitizeFilename(title)}-${dateStr()}.html`;
}

// ── Internal helpers ──

function escapeHtml(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

function sanitizeFilename(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/[\\/:*?"<>|\x00-\x1f]/g, "_").slice(0, 80) || "chat";
}

function dateStr(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}`;
}

async function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = reject;
    reader.readAsDataURL(blob);
  });
}

// ── DOM cleanup ──

async function cleanupClonedDom(clone: HTMLElement, selectedIds?: Set<string>) {
  // Remove interactive elements + exclude markers
  clone
    .querySelectorAll("input, textarea, select, [data-export-exclude]")
    .forEach((el) => el.remove());

  // Remove Svelte comment nodes (<!---->)
  const walker = document.createTreeWalker(clone, NodeFilter.SHOW_COMMENT);
  const comments: Comment[] = [];
  while (walker.nextNode()) comments.push(walker.currentNode as Comment);
  for (const c of comments) c.remove();

  // Remove scroll-container styles from root (prevents fixed-height scrollbox in export)
  clone.classList.remove("h-full", "overflow-y-auto", "overflow-hidden", "overflow-auto");
  clone.removeAttribute("style");

  // Inline blob: images → data URLs (fallback to placeholder)
  const blobImgs = Array.from(clone.querySelectorAll<HTMLImageElement>('img[src^="blob:"]'));
  await Promise.all(
    blobImgs.map(async (img) => {
      try {
        const resp = await fetch(img.src);
        const blob = await resp.blob();
        img.src = await blobToDataUrl(blob);
      } catch (e) {
        dbgWarn("html-export", "blob image inline failed", e);
        const span = document.createElement("span");
        span.textContent = "[image unavailable in export]";
        span.className = "text-xs text-muted-foreground italic";
        img.replaceWith(span);
      }
    }),
  );

  // Phase 2: selection filter
  if (selectedIds && selectedIds.size > 0) {
    clone.querySelectorAll("[data-entry-id]").forEach((el) => {
      const id = (el as HTMLElement).dataset.entryId;
      if (id && !selectedIds.has(id)) el.remove();
    });
  }
}

// ── CSS extraction ──

function extractAllStyles(): string {
  const parts: string[] = [];
  for (const sheet of Array.from(document.styleSheets)) {
    try {
      const rules = sheet.cssRules;
      if (!rules) continue;
      for (const rule of Array.from(rules)) parts.push(rule.cssText);
    } catch (e) {
      dbgWarn("html-export", "stylesheet skipped", {
        href: sheet.href,
        err: String(e),
      });
    }
  }
  // Defense: ensure CSSOM output doesn't close the <style> tag
  return parts.join("\n").replace(/<\/style/gi, "<\\/style");
}

// ── HTML template ──

function wrapAsStandaloneHtml(args: {
  body: string;
  styles: string;
  themeClasses: string;
  options: ExportOptions;
}): string {
  const { body, styles, themeClasses, options } = args;
  const lang = escapeHtml(document.documentElement.lang || "en");
  const title = escapeHtml(options.title);
  const model = escapeHtml(options.sessionInfo.model ?? "");
  const cwd = escapeHtml(options.sessionInfo.cwd ?? "");
  const turns = options.sessionInfo.turnCount;
  const date = escapeHtml(new Date().toLocaleString());

  return `<!DOCTYPE html>
<html class="${escapeHtml(themeClasses)}" lang="${lang}">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta name="generator" content="OpenCovibe">
  <title>${title} — OpenCovibe</title>
  <style>
${styles}
/* Export overrides */
body { overflow: auto !important; height: auto !important; }
.cv-auto { content-visibility: visible !important; contain-intrinsic-size: auto !important; }
  </style>
</head>
<body class="bg-background text-foreground">
  <header style="max-width:48rem;margin:0 auto;padding:1rem 1.5rem;border-bottom:1px solid hsl(var(--border))">
    <h1 style="font-size:1.125rem;font-weight:600;margin:0">${title}</h1>
    <div style="font-size:0.75rem;color:hsl(var(--muted-foreground));margin-top:0.25rem">
      ${model ? `Model: ${model} · ` : ""}${cwd ? `${cwd} · ` : ""}${turns} turns · Exported ${date}
    </div>
  </header>
  <main>
    ${body}
  </main>
  <footer style="max-width:48rem;margin:0 auto;padding:1.5rem;text-align:center;font-size:0.75rem;color:hsl(var(--muted-foreground))">
    Generated by <a href="https://github.com/AnyiWang/OpenCovibe" style="color:hsl(var(--primary))">OpenCovibe</a>
  </footer>
  <script>
    document.querySelectorAll('[data-code-copy]').forEach(function(btn) {
      btn.addEventListener('click', function() {
        var codeEl = btn.closest('.code-block') && btn.closest('.code-block').querySelector('pre code');
        if (!codeEl) return;
        var text = codeEl.textContent || '';
        var done = function() {
          var orig = btn.textContent;
          btn.textContent = 'Copied!';
          setTimeout(function(){ btn.textContent = orig; }, 1500);
        };
        if (navigator.clipboard && window.isSecureContext) {
          navigator.clipboard.writeText(text).then(done).catch(fallback);
        } else { fallback(); }
        function fallback() {
          var ta = document.createElement('textarea');
          ta.value = text;
          ta.style.position = 'fixed';
          ta.style.left = '-9999px';
          document.body.appendChild(ta);
          ta.select();
          try { document.execCommand('copy'); done(); } catch(e) {}
          document.body.removeChild(ta);
        }
      });
    });
  </script>
</body>
</html>`;
}

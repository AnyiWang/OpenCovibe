<script lang="ts">
  import { renderMarkdown } from "$lib/utils/markdown";
  import { readFileBase64 } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { onDestroy } from "svelte";

  let {
    text = "",
    streaming = false,
    basePath = "",
    class: className = "",
    lazy = true,
  }: {
    text?: string;
    streaming?: boolean;
    basePath?: string;
    class?: string;
    /** When true (default), defer markdown parsing until element enters viewport.
     *  Off-screen entries render raw <pre> until then. Pass false for places where
     *  the content is always visible and lazy-render would just add a re-render. */
    lazy?: boolean;
  } = $props();

  let container: HTMLDivElement | undefined = $state();
  let lazyEl: HTMLElement | undefined = $state();
  /** Set to true once element has been intersection-observed near the viewport.
   *  Sticky — once true, stays true (so scrolling away doesn't un-parse content). */
  let visibleOnce = $state(!lazy);

  // ── Lazy markdown rendering: skip parse until element is near viewport ──
  // Off-screen MarkdownContent shows raw <pre>{text}</pre>. When IntersectionObserver
  // detects approach (within 300px rootMargin), we flip visibleOnce → markdown parse.
  // This eliminates the ~150-200ms total of synchronous md-render at chat-page mount
  // when timeline has dozens of historical entries.
  $effect(() => {
    if (!lazy || streaming || visibleOnce) return;
    const el = lazyEl;
    if (!el) return;
    if (typeof IntersectionObserver === "undefined") {
      // No IntersectionObserver (e.g., very old WebView) — fall back to immediate parse.
      visibleOnce = true;
      return;
    }
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          visibleOnce = true;
          obs.disconnect();
        }
      },
      { rootMargin: "300px 0px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  });

  // ── Streaming display: rAF-coalesced raw <pre>; non-streaming: full markdown render ──
  // Streaming mode shows raw text in a <pre> (zero parse cost). DOM writes are coalesced
  // to one per animation frame so high-frequency token deltas don't thrash text nodes.
  // On streaming → false, $derived recomputes html once and the {#if/:else} branch swaps.
  // Init to empty — `$state(text)` would only capture text's value at component creation,
  // and Svelte 5 warns about that pattern. The effect below runs on mount and seeds
  // displayText from current `text` (either via the !streaming branch or firstSyncDone).
  let displayText = $state("");
  let rafId: number | null = null;
  // Non-reactive flag: set/read here doesn't trigger Svelte effect tracking.
  // We use this instead of reading `displayText` inside the effect — reading $state
  // would make the rAF callback's `displayText = text` trigger an effect rerun,
  // wasting one no-op frame per real text change.
  let firstSyncDone = false;

  function cancelPendingFrame() {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  }

  $effect(() => {
    if (!streaming) {
      // Leaving streaming: cancel any pending rAF, sync immediately.
      cancelPendingFrame();
      displayText = text;
      firstSyncDone = false; // reset for next streaming session
      return;
    }
    // First frame on (re)entering streaming with content: sync immediately to avoid
    // visible "first character delay one frame".
    if (!firstSyncDone && text !== "") {
      displayText = text;
      firstSyncDone = true;
      return;
    }
    // Streaming: at most one rAF-pending update; high-frequency tokens coalesce.
    // ⚠️ Do NOT cancel rAF in $effect cleanup — Svelte runs cleanup before each rerun, so
    //    if text ticks faster than vsync we'd repeatedly cancel→reschedule and starve the flush.
    if (rafId === null) {
      rafId = requestAnimationFrame(() => {
        rafId = null;
        displayText = text;
      });
    }
  });

  // Cancel pending rAF on unmount only (not on every effect rerun).
  onDestroy(cancelPendingFrame);

  // Markdown rendering gate: skip when streaming OR not yet visible (lazy).
  let renderMarkdownNow = $derived(!streaming && visibleOnce);
  let html = $derived(renderMarkdownNow && displayText ? renderMarkdown(displayText) : "");

  $effect(() => {
    if (!container || !html) return;

    const buttons = container.querySelectorAll<HTMLButtonElement>("[data-code-copy]");
    const cleanups: Array<() => void> = [];

    buttons.forEach((btn) => {
      const handler = async () => {
        const codeEl = btn.closest(".code-block")?.querySelector("pre code");
        if (!codeEl) return;
        try {
          await navigator.clipboard.writeText(codeEl.textContent || "");
          btn.textContent = "Copied!";
          btn.classList.add("copied");
          setTimeout(() => {
            btn.textContent = "Copy";
            btn.classList.remove("copied");
          }, 1500);
        } catch {
          // Silently fail
        }
      };
      btn.addEventListener("click", handler);
      cleanups.push(() => btn.removeEventListener("click", handler));
    });

    return () => {
      cleanups.forEach((fn) => fn());
    };
  });

  // Resolve relative image paths against basePath (for Explorer file preview)
  $effect(() => {
    if (!container || !html || !basePath) return;

    const imgs = container.querySelectorAll<HTMLImageElement>("img");
    for (const img of imgs) {
      const src = img.getAttribute("src");
      if (!src) continue;
      // Skip URLs, data URIs, and absolute paths
      if (/^(https?:|data:|blob:)/.test(src)) continue;
      if (src.startsWith("/") || /^[a-zA-Z]:/.test(src)) continue;

      // Construct absolute path: normalize to forward slashes for Rust PathBuf
      const abs = basePath.replace(/\\/g, "/") + "/" + src.replace(/\\/g, "/");
      dbg("markdown", "resolve-img", { src, abs });

      readFileBase64(abs)
        .then(([base64, mime]) => {
          img.src = `data:${mime};base64,${base64}`;
        })
        .catch((e) => {
          dbgWarn("markdown", "img-load-failed", { src, abs, error: e });
        });
    }
  });
</script>

{#if !renderMarkdownNow}
  <pre
    bind:this={lazyEl}
    class="whitespace-pre-wrap break-words font-sans text-sm leading-relaxed text-foreground/90 m-0 {className}">{displayText}</pre>
{:else}
  <div
    bind:this={container}
    class="prose prose-sm dark:prose-invert max-w-none
      prose-p:text-foreground prose-p:leading-relaxed
      prose-a:text-primary prose-a:underline prose-a:underline-offset-2
      prose-code:rounded prose-code:bg-muted/70 prose-code:px-1 prose-code:py-0.5 prose-code:text-xs prose-code:font-mono prose-code:before:content-none prose-code:after:content-none
      prose-pre:m-0 prose-pre:p-0 prose-pre:bg-transparent prose-pre:border-0
      prose-li:text-foreground
      {className}"
  >
    {@html html}
  </div>
{/if}

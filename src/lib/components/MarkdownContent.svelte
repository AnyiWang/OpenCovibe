<script lang="ts">
  import { renderMarkdown } from "$lib/utils/markdown";

  let {
    text = "",
    streaming = false,
    class: className = "",
  }: {
    text?: string;
    streaming?: boolean;
    class?: string;
  } = $props();

  let container: HTMLDivElement | undefined = $state();

  // Throttled text for streaming mode: updates at most every 150ms
  let throttledText = $state(text);
  $effect(() => {
    const t = text;
    if (streaming) {
      const timer = setTimeout(() => {
        throttledText = t;
      }, 150);
      return () => clearTimeout(timer);
    } else {
      throttledText = t;
    }
  });

  // Single render path: $derived ensures renderMarkdown is called exactly once per text change
  let html = $derived(throttledText ? renderMarkdown(throttledText) : "");

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
</script>

<div
  bind:this={container}
  class="prose prose-sm dark:prose-invert max-w-none
    prose-p:text-foreground prose-p:leading-relaxed
    prose-a:text-primary prose-a:underline prose-a:underline-offset-2
    prose-code:rounded prose-code:bg-muted/70 prose-code:px-1 prose-code:py-0.5 prose-code:text-xs prose-code:font-mono prose-code:before:content-none prose-code:after:content-none
    prose-pre:m-0 prose-pre:p-0 prose-pre:bg-transparent
    prose-li:text-foreground
    {className}"
>
  {@html html}
</div>

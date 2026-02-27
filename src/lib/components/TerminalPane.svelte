<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";

  let {
    lines = [],
    class: className = "",
  }: {
    lines?: Array<{ type: string; text: string }>;
    class?: string;
  } = $props();

  let scrollEl: HTMLDivElement | undefined = $state();
  let showScrollHint = $state(false);
  let isAutoScroll = $state(true);

  const colorMap: Record<string, string> = {
    stderr: "text-red-700 dark:text-red-300",
    system: "text-cyan-700 dark:text-cyan-300",
    command: "text-amber-700 dark:text-amber-200",
    stdout: "text-emerald-700 dark:text-emerald-200",
    reasoning: "text-violet-700 dark:text-violet-300",
    tool: "text-sky-700 dark:text-sky-300",
  };

  // Track user scroll position
  function handleScroll() {
    if (!scrollEl) return;
    const distanceFromBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
    isAutoScroll = distanceFromBottom < 40;
    showScrollHint = !isAutoScroll && lines.length > 0;
  }

  function scrollToBottom() {
    if (scrollEl) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
      showScrollHint = false;
      isAutoScroll = true;
    }
  }

  // Auto-scroll to bottom on new lines (only if user hasn't scrolled up)
  $effect(() => {
    if (lines.length > 0 && scrollEl && isAutoScroll) {
      requestAnimationFrame(() => {
        if (scrollEl) {
          scrollEl.scrollTop = scrollEl.scrollHeight;
        }
      });
    } else if (lines.length > 0 && !isAutoScroll) {
      showScrollHint = true;
    }
  });

  // Flatten lines for line numbering
  let flatLines = $derived.by(() => {
    const result: Array<{ type: string; text: string; label?: string; colorClass: string }> = [];
    for (const line of lines) {
      const codexParsed = parseCodexLine(line.text);
      if (codexParsed) {
        result.push({
          type: line.type,
          text: codexParsed.content,
          label: codexParsed.label,
          colorClass: codexParsed.colorClass,
        });
      } else {
        const lineColor = colorMap[line.type] ?? "text-foreground/80";
        for (const subline of line.text.split("\n").filter((l) => l.trim())) {
          result.push({
            type: line.type,
            text: subline,
            label: `[${line.type}]`,
            colorClass: lineColor,
          });
        }
      }
    }
    return result;
  });

  function parseCodexLine(
    text: string,
  ): { label: string; content: string; colorClass: string } | null {
    const trimmed = text.trim();
    if (!trimmed) return null;
    try {
      const parsed = JSON.parse(trimmed);
      const type = String(parsed.type ?? "");
      if (type.includes("command") || type.includes("tool")) {
        const item = parsed.item ?? parsed;
        const cmdText = typeof item === "string" ? item : JSON.stringify(item).slice(0, 200);
        return { label: "[tool]", content: cmdText, colorClass: colorMap.tool };
      }
      if (type.includes("reasoning")) {
        const text = parsed.summary ?? parsed.text ?? JSON.stringify(parsed);
        return {
          label: "[reasoning]",
          content: String(text),
          colorClass: colorMap.reasoning,
        };
      }
      if (type.includes("message") || type.includes("assistant")) {
        const text = parsed.text ?? parsed.content ?? "";
        return {
          label: "[assistant]",
          content: String(text),
          colorClass: colorMap.stdout,
        };
      }
      if (type.includes("error")) {
        return {
          label: "[error]",
          content: parsed.message ?? JSON.stringify(parsed),
          colorClass: colorMap.stderr,
        };
      }
    } catch {
      // Not JSON
    }
    return null;
  }
</script>

<div class="flex flex-col {className}">
  <!-- Terminal title bar -->
  <div class="flex items-center gap-2 border-b bg-muted/40 dark:bg-black/60 px-3 py-1.5">
    <div class="flex items-center gap-1.5">
      <span class="h-2.5 w-2.5 rounded-full bg-red-500/70"></span>
      <span class="h-2.5 w-2.5 rounded-full bg-yellow-500/70"></span>
      <span class="h-2.5 w-2.5 rounded-full bg-green-500/70"></span>
    </div>
    <span class="ml-2 text-[11px] font-medium text-muted-foreground">{t("terminal_title")}</span>
  </div>

  <!-- Terminal content -->
  <div class="relative flex-1 overflow-hidden">
    <div
      bind:this={scrollEl}
      onscroll={handleScroll}
      class="h-full overflow-y-auto bg-muted/60 dark:bg-black/95 p-3 font-mono text-xs"
    >
      {#each flatLines as line, i (i)}
        <div class="flex py-0.5 {line.colorClass}">
          <span class="mr-3 w-7 flex-shrink-0 text-right text-muted-foreground/40 select-none"
            >{i + 1}</span
          >
          <span>
            <span class="font-bold opacity-60">{line.label}</span>
            <!-- eslint-disable-next-line svelte/no-useless-mustaches -->
            {" "}
            {line.text}
          </span>
        </div>
      {/each}
    </div>

    <!-- New output scroll hint -->
    {#if showScrollHint}
      <button
        class="absolute bottom-3 left-1/2 -translate-x-1/2 flex items-center gap-1.5 rounded-full bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground shadow-lg transition-all duration-200 hover:bg-primary/90 animate-fade-in"
        onclick={scrollToBottom}
      >
        {t("terminal_newOutput")}
        <svg
          class="h-3 w-3"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg
        >
      </button>
    {/if}
  </div>
</div>

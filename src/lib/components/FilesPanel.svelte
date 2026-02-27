<script lang="ts">
  import type { FileEntry } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";

  let {
    fileEntries = [],
    onScrollToTool,
  }: {
    fileEntries: FileEntry[];
    onScrollToTool?: (toolUseId: string) => void;
  } = $props();

  function shortPath(p: string): string {
    const parts = p.split("/");
    return parts.length > 2 ? "\u2026/" + parts.slice(-2).join("/") : p;
  }

  function actionColor(action: FileEntry["action"]): { bg: string; text: string } {
    switch (action) {
      case "write":
        return { bg: "bg-amber-500/15", text: "text-amber-600 dark:text-amber-400" };
      case "edit":
        return { bg: "bg-amber-500/15", text: "text-amber-600 dark:text-amber-400" };
      case "read":
        return { bg: "bg-blue-500/15", text: "text-blue-600 dark:text-blue-400" };
      case "persisted":
        return { bg: "bg-emerald-500/15", text: "text-emerald-600 dark:text-emerald-400" };
    }
  }

  function actionLabel(action: FileEntry["action"]): string {
    switch (action) {
      case "write":
        return "W";
      case "edit":
        return "E";
      case "read":
        return "R";
      case "persisted":
        return "P";
    }
  }
</script>

<div class="flex-1 overflow-y-auto py-1">
  {#if fileEntries.length === 0}
    <div class="flex items-center justify-center h-32 text-xs text-muted-foreground/50">
      {t("filesPanel_noFiles")}
    </div>
  {:else}
    {#each fileEntries as entry, i (entry.path + "-" + i)}
      {@const color = actionColor(entry.action)}
      {@const canJump = !!entry.toolUseId}
      {#if canJump}
        <button
          class="w-full text-left px-2.5 py-1 hover:bg-accent/50 rounded-sm transition-colors group"
          onclick={() => onScrollToTool?.(entry.toolUseId!)}
          title={t("toolActivity_scrollToTool")}
        >
          <div class="flex items-center gap-1.5">
            <span
              class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded text-[9px] font-bold {color.bg} {color.text}"
            >
              {actionLabel(entry.action)}
            </span>
            <span class="text-[11px] text-foreground truncate min-w-0 group-hover:underline"
              >{shortPath(entry.path)}</span
            >
          </div>
        </button>
      {:else}
        <div class="px-2.5 py-1 cursor-default">
          <div class="flex items-center gap-1.5">
            <span
              class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded text-[9px] font-bold {color.bg} {color.text}"
            >
              {actionLabel(entry.action)}
            </span>
            <span class="text-[11px] text-muted-foreground truncate min-w-0"
              >{shortPath(entry.path)}</span
            >
            <!-- Not-locatable icon -->
            <svg
              class="h-3 w-3 shrink-0 text-muted-foreground/30"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              title={t("filesPanel_notLocatable")}
            >
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
              <line x1="8" y1="11" x2="14" y2="11" />
            </svg>
          </div>
        </div>
      {/if}
    {/each}
  {/if}
</div>

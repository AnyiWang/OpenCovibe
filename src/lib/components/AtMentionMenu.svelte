<script lang="ts">
  import type { DirEntry } from "$lib/types";
  import { onMount } from "svelte";

  let {
    entries,
    selectedIndex,
    loading,
    query,
    anchorEl,
    onSelect,
    onHover,
    onDismiss,
  }: {
    entries: DirEntry[];
    selectedIndex: number;
    loading: boolean;
    query: string;
    anchorEl: HTMLElement | undefined;
    onSelect: (entry: DirEntry) => void;
    onHover: (index: number) => void;
    onDismiss: () => void;
  } = $props();

  let menuEl: HTMLDivElement | undefined = $state();
  let bottom = $state(0);
  let left = $state(0);
  let width = $state(0);

  function updatePosition() {
    if (!anchorEl) return;
    const rect = anchorEl.getBoundingClientRect();
    bottom = window.innerHeight - rect.top + 4;
    left = rect.left;
    width = Math.min(rect.width, 400);
  }

  // Scroll selected item into view
  $effect(() => {
    const idx = selectedIndex;
    if (menuEl) {
      const item = menuEl.querySelector(`[data-at-index="${idx}"]`);
      item?.scrollIntoView({ block: "nearest" });
    }
  });

  onMount(() => {
    updatePosition();

    window.addEventListener("scroll", updatePosition, true);
    window.addEventListener("resize", updatePosition);

    function handleMousedown(e: MouseEvent) {
      const target = e.target as Node;
      if (menuEl && !menuEl.contains(target) && !anchorEl?.contains(target)) {
        onDismiss();
      }
    }
    document.addEventListener("mousedown", handleMousedown, true);

    return () => {
      window.removeEventListener("scroll", updatePosition, true);
      window.removeEventListener("resize", updatePosition);
      document.removeEventListener("mousedown", handleMousedown, true);
    };
  });

  function formatSize(size: number): string {
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div
  bind:this={menuEl}
  class="fixed z-50 rounded-lg border border-border bg-background shadow-lg animate-fade-in"
  style="bottom: {bottom}px; left: {left}px; width: {width}px;"
>
  <!-- Header -->
  <div class="flex items-center gap-2 px-3 py-1.5 border-b border-border">
    <span class="text-xs text-muted-foreground/60">@{query || "..."}</span>
    {#if loading}
      <div
        class="h-3 w-3 rounded-full border-2 border-border border-t-muted-foreground animate-spin ml-auto"
      ></div>
    {/if}
  </div>

  {#if entries.length > 0}
    <div class="max-h-[240px] overflow-y-auto">
      {#each entries as entry, i (entry.name)}
        <button
          data-at-index={i}
          class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors {i ===
          selectedIndex
            ? 'bg-accent text-foreground'
            : 'text-muted-foreground hover:bg-accent/50'}"
          onmouseenter={() => onHover(i)}
          onclick={() => onSelect(entry)}
        >
          <!-- Icon: folder or file -->
          {#if entry.is_dir}
            <svg
              class="h-3.5 w-3.5 shrink-0 text-blue-400"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path
                d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
              />
            </svg>
          {:else}
            <svg
              class="h-3.5 w-3.5 shrink-0 text-muted-foreground/60"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
              <path d="M14 2v4a2 2 0 0 0 2 2h4" />
            </svg>
          {/if}

          <span class="flex-1 min-w-0 truncate text-xs font-medium">
            {entry.name}{entry.is_dir ? "/" : ""}
          </span>

          {#if !entry.is_dir && entry.size > 0}
            <span class="shrink-0 text-[10px] text-muted-foreground/40">
              {formatSize(entry.size)}
            </span>
          {/if}
        </button>
      {/each}
    </div>
  {:else if !loading}
    <div class="flex items-center justify-center py-4">
      <span class="text-xs text-muted-foreground/50">No matches</span>
    </div>
  {:else}
    <div class="flex items-center justify-center py-4">
      <span class="text-xs text-muted-foreground/50">Searching...</span>
    </div>
  {/if}
</div>

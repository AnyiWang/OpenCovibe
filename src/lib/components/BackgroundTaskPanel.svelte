<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";

  let {
    tasks = new Map(),
    activeTasks = [],
    collapsed = $bindable(false),
  }: {
    tasks?: Map<
      string,
      { task_id: string; status: string; message: string; startedAt: number; data: unknown }
    >;
    activeTasks?: Array<{
      task_id: string;
      status: string;
      message: string;
      startedAt: number;
    }>;
    collapsed?: boolean;
  } = $props();

  function elapsed(startedAt: number): string {
    const ms = Date.now() - startedAt;
    if (ms < 1000) return "<1s";
    return `${Math.floor(ms / 1000)}s`;
  }

  // Sort: active first, then completed/failed (by most recent)
  let sortedTasks = $derived.by(() => {
    const items = [...tasks.values()];
    return items.sort((a, b) => {
      const aActive =
        a.status !== "completed" && a.status !== "failed" && a.status !== "error" ? 0 : 1;
      const bActive =
        b.status !== "completed" && b.status !== "failed" && b.status !== "error" ? 0 : 1;
      if (aActive !== bActive) return aActive - bActive;
      return b.startedAt - a.startedAt;
    });
  });
</script>

{#if tasks.size > 0}
  <div class="border-b border-border/50 bg-muted/30 text-xs">
    <button
      class="flex w-full items-center gap-1.5 px-3 py-1 text-foreground/50 hover:text-foreground/70 transition-colors"
      onclick={() => (collapsed = !collapsed)}
    >
      <svg
        class="h-3 w-3 transition-transform {collapsed ? '-rotate-90' : ''}"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="m6 9 6 6 6-6" />
      </svg>
      <span class="font-medium">{t("bgTask_title", { count: String(tasks.size) })}</span>
      {#if activeTasks.length > 0}
        <span class="inline-flex items-center gap-1 text-blue-400">
          <span class="inline-block h-1.5 w-1.5 rounded-full bg-blue-400 animate-pulse"></span>
          {activeTasks.length}
        </span>
      {/if}
    </button>

    {#if !collapsed}
      <div class="px-3 pb-2 space-y-1">
        {#each sortedTasks as item (item.task_id)}
          {@const isDone = item.status === "completed"}
          {@const isFailed = item.status === "failed" || item.status === "error"}
          {@const isActive = !isDone && !isFailed}
          <div
            class="flex items-center gap-2 rounded px-2 py-1 {isDone
              ? 'bg-emerald-500/5 text-foreground/40 animate-task-fadeout'
              : isFailed
                ? 'bg-destructive/5 text-foreground/50'
                : 'bg-blue-500/5 text-foreground/70'}"
          >
            <!-- Status icon -->
            {#if isActive}
              <svg
                class="h-3 w-3 shrink-0 text-blue-400 animate-spin"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
              >
                <path d="M21 12a9 9 0 1 1-6.219-8.56" />
              </svg>
            {:else if isDone}
              <svg
                class="h-3 w-3 shrink-0 text-emerald-500"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M20 6 9 17l-5-5" />
              </svg>
            {:else}
              <svg
                class="h-3 w-3 shrink-0 text-destructive"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            {/if}

            <span class="flex-1 min-w-0 truncate">{item.message}</span>

            {#if isActive}
              <span class="shrink-0 text-foreground/30 tabular-nums">{elapsed(item.startedAt)}</span
              >
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  @keyframes task-fadeout {
    0% {
      opacity: 1;
    }
    70% {
      opacity: 1;
    }
    100% {
      opacity: 0.3;
    }
  }
  .animate-task-fadeout {
    animation: task-fadeout 5s ease-out forwards;
  }
</style>

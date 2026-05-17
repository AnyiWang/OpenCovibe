<script lang="ts">
  import { onDestroy } from "svelte";
  import type { ScheduledTask } from "$lib/stores/session-store.svelte";
  import { t } from "$lib/i18n/index.svelte";

  type Props = {
    tasks: ScheduledTask[];
    /** Disable Cancel/List buttons (UI still readable). True during active turn. */
    busy: boolean;
    /** Post a "Cancel scheduled task <id>" user message. */
    onCancel: (id: string) => void;
    /** Post a "Show me my scheduled tasks" user message. */
    onList: () => void;
  };

  let { tasks, busy, onCancel, onList }: Props = $props();

  let open = $state(false);

  // Per-id UI state. Stays in component (not store) so replay never surfaces ghost rows.
  let pending = $state<Map<string, "pending" | "error">>(new Map());

  // 30s timeout cleanup handles
  let timers = new Map<string, ReturnType<typeof setTimeout>>();

  onDestroy(() => {
    for (const tm of timers.values()) clearTimeout(tm);
    timers.clear();
  });

  // Clear pending entry when the task vanishes from store (CronDelete succeeded).
  $effect(() => {
    const ids = new Set(tasks.map((t) => t.id));
    let dirty = false;
    const next = new Map(pending);
    for (const id of pending.keys()) {
      if (!ids.has(id)) {
        next.delete(id);
        const tm = timers.get(id);
        if (tm) {
          clearTimeout(tm);
          timers.delete(id);
        }
        dirty = true;
      }
    }
    if (dirty) pending = next;
  });

  function handleCancelClick(id: string) {
    if (busy) return;
    if (pending.has(id)) return;
    onCancel(id);
    const next = new Map(pending);
    next.set(id, "pending");
    pending = next;
    const tm = setTimeout(() => {
      // If still pending after 30s, flip to error (component-local only).
      if (pending.has(id) && pending.get(id) === "pending") {
        const n = new Map(pending);
        n.set(id, "error");
        pending = n;
      }
      timers.delete(id);
    }, 30_000);
    timers.set(id, tm);
  }

  function handleListClick() {
    if (busy) return;
    onList();
  }

  function truncate(s: string, max = 60): string {
    if (s.length <= max) return s;
    return s.slice(0, max - 1) + "…";
  }
</script>

{#if tasks.length > 0}
  <div class="mx-auto w-full max-w-3xl px-4 pb-2">
    <div class="rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm">
      <div class="flex items-center justify-between">
        <button
          class="flex items-center gap-2 text-amber-500 hover:text-amber-400"
          onclick={() => (open = !open)}
          aria-expanded={open}
        >
          <span aria-hidden="true">📋</span>
          <span class="font-medium">
            {t("scheduled_tasks_count", { count: String(tasks.length) })}
          </span>
          <span class="text-amber-500/70" aria-hidden="true">{open ? "▾" : "▸"}</span>
        </button>
        <button
          class="rounded px-2 py-0.5 text-xs text-amber-500 hover:bg-amber-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          onclick={handleListClick}
          disabled={busy}
          title={busy ? t("scheduled_tasks_waitForTurn") : t("scheduled_tasks_listTooltip")}
        >
          {t("scheduled_tasks_listButton")}
        </button>
      </div>

      {#if open}
        <ul class="mt-2 space-y-1.5 border-t border-amber-500/20 pt-2">
          {#each tasks as task (task.id)}
            {@const state = pending.get(task.id)}
            <li class="flex items-center justify-between gap-2">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2 text-amber-500/90">
                  <span class="font-mono text-xs">{task.humanSchedule}</span>
                  {#if !task.recurring}
                    <span class="rounded bg-amber-500/20 px-1 text-[10px] uppercase">
                      {t("scheduled_tasks_oneShot")}
                    </span>
                  {/if}
                </div>
                {#if task.prompt}
                  <div class="text-xs text-amber-500/60 truncate" title={task.prompt}>
                    {truncate(task.prompt)}
                  </div>
                {/if}
                {#if state === "pending"}
                  <div class="text-[11px] text-amber-500/60 italic">
                    {t("scheduled_tasks_cancelPending")}
                  </div>
                {:else if state === "error"}
                  <div class="text-[11px] text-red-400">
                    {t("scheduled_tasks_cancelTimeout")}
                  </div>
                {/if}
              </div>
              <button
                class="rounded px-2 py-0.5 text-xs text-red-400 hover:bg-red-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                onclick={() => handleCancelClick(task.id)}
                disabled={busy || state === "pending"}
                title={busy ? t("scheduled_tasks_waitForTurn") : t("scheduled_tasks_cancelTooltip")}
              >
                {t("scheduled_tasks_cancelButton")}
              </button>
            </li>
          {/each}
        </ul>
        <div class="mt-2 border-t border-amber-500/20 pt-1.5 text-[11px] text-amber-500/50">
          {t("scheduled_tasks_expiryNote")}
        </div>
      {/if}
    </div>
  </div>
{/if}

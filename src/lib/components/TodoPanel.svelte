<script lang="ts">
  import type { PanelTask } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg } from "$lib/utils/debug";

  type Props = {
    /** Current task list (Tasks system or legacy TodoWrite). Empty hides the panel. */
    tasks: PanelTask[];
  };

  let { tasks }: Props = $props();

  // Default expanded — mirror the CLI, which shows the full checklist inline.
  let open = $state(true);

  let doneCount = $derived(tasks.filter((td) => td.status === "completed").length);

  $effect(() => {
    if (tasks.length > 0) dbg("todo-panel", "render", { total: tasks.length, done: doneCount });
  });
</script>

{#if tasks.length > 0}
  <div class="mx-auto w-full max-w-3xl px-4 pb-2">
    <div class="rounded-lg border border-border bg-muted/40 px-4 py-2 text-sm">
      <button
        class="flex w-full items-center gap-2 text-muted-foreground hover:text-foreground transition-colors"
        onclick={() => (open = !open)}
        aria-expanded={open}
      >
        <span aria-hidden="true">📋</span>
        <span class="font-medium">{t("todos_panelHeader")}</span>
        <span class="text-muted-foreground/70">
          {t("todos_panelSummary", { done: String(doneCount), total: String(tasks.length) })}
        </span>
        <span class="ml-auto text-muted-foreground/70" aria-hidden="true">{open ? "▾" : "▸"}</span>
      </button>

      {#if open}
        <ul class="mt-2 max-h-60 space-y-1 overflow-y-auto border-t border-border pt-2">
          {#each tasks as task, i (i)}
            <li class="flex items-center gap-2 text-xs">
              <span
                class="rounded px-1.5 py-0.5 text-[10px] font-medium {task.status === 'completed'
                  ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
                  : task.status === 'in_progress'
                    ? 'bg-blue-500/15 text-blue-600 dark:text-blue-400'
                    : 'bg-neutral-500/15 text-muted-foreground'}"
              >
                {task.status === "completed"
                  ? t("tool_statusDone")
                  : task.status === "in_progress"
                    ? t("tool_statusWip")
                    : t("tool_statusTodo")}
              </span>
              <span
                class="text-muted-foreground {task.status === 'completed'
                  ? 'line-through opacity-60'
                  : ''}">{task.text}</span
              >
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
{/if}

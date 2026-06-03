<script lang="ts">
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n/index.svelte";

  /** A selectable conversation turn (one top-level user message). */
  export interface RewindTurn {
    /** 0-based index among top-level user messages. */
    index: number;
    /** Short preview of the user message that opened this turn. */
    preview: string;
  }

  let {
    open = $bindable(false),
    turns = [],
    busy = false,
    onConfirm,
  }: {
    open?: boolean;
    turns?: RewindTurn[];
    busy?: boolean;
    // numTurns = how many turns to DROP from history (>=1).
    onConfirm?: (opts: { dropFromTurnIndex: number; numTurns: number }) => void;
  } = $props();

  // Default selection = the earliest turn that still drops something (the last
  // turn keeps everything, so it's never the obvious target — pick second-last).
  let selected = $state<number>(0);

  $effect(() => {
    if (open) {
      // Reset to a sensible default whenever the modal re-opens.
      selected = turns.length > 1 ? turns[turns.length - 1].index : 0;
    }
  });

  // Dropping from turn `selected` removes that turn and everything after it.
  let numTurns = $derived(turns.length - selected);
  let canRun = $derived(turns.length > 0 && numTurns >= 1 && !busy);

  function run() {
    if (!canRun) return;
    onConfirm?.({ dropFromTurnIndex: selected, numTurns });
    open = false;
  }
</script>

<Modal bind:open title={t("codexRewind_title")}>
  <div class="space-y-3 min-w-[420px] max-w-[560px]">
    <!-- History-only warning: distinct from Claude snapshot rewind. -->
    <div
      class="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-600 dark:text-amber-400"
    >
      {t("codexRewind_warning")}
    </div>

    {#if turns.length === 0}
      <p class="text-sm text-muted-foreground">{t("codexRewind_noTurns")}</p>
    {:else}
      <p class="text-xs text-muted-foreground">{t("codexRewind_pickPrompt")}</p>
      <div class="max-h-[280px] space-y-1 overflow-y-auto">
        {#each turns as turn (turn.index)}
          <label
            class="flex items-start gap-3 rounded-md border p-2.5 cursor-pointer transition-colors
              {selected === turn.index ? 'border-primary bg-primary/5' : 'hover:bg-accent'}"
          >
            <input
              type="radio"
              name="codex-rewind-turn"
              value={turn.index}
              bind:group={selected}
              class="mt-0.5"
            />
            <div class="flex-1 min-w-0">
              <p class="text-xs font-medium text-muted-foreground">
                {t("codexRewind_turnLabel", { n: String(turn.index + 1) })}
              </p>
              <p class="text-sm truncate">{turn.preview || t("codexRewind_emptyTurn")}</p>
            </div>
          </label>
        {/each}
      </div>

      <p class="text-xs text-muted-foreground">
        {t("codexRewind_summary", { n: String(numTurns) })}
      </p>
    {/if}

    <div class="flex justify-end gap-2 pt-1">
      <button
        class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent"
        onclick={() => (open = false)}
      >
        {t("common_cancel")}
      </button>
      <button
        class="rounded-md px-3 py-1.5 text-sm bg-destructive text-destructive-foreground disabled:opacity-50"
        disabled={!canRun}
        onclick={run}
      >
        {t("codexRewind_confirm")}
      </button>
    </div>
  </div>
</Modal>

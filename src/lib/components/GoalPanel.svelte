<script lang="ts">
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { fmtNumber } from "$lib/i18n/format";
  import * as api from "$lib/api";
  import type { ThreadGoal, GoalStatus } from "$lib/types";
  import { dbgWarn } from "$lib/utils/debug";

  let {
    open = $bindable(false),
    runId,
    // Live goal from the session store (updated by the goal_update reducer).
    goal = null,
  }: {
    open?: boolean;
    runId: string | undefined;
    goal?: ThreadGoal | null;
  } = $props();

  let objective = $state("");
  let tokenBudget = $state<string>(""); // text input; parsed on save
  let loading = $state(false);
  let saving = $state(false);
  let error = $state("");
  // True once the user edits the objective, so live goal_update events don't
  // clobber an in-progress edit.
  let dirty = $state(false);

  const STATUS_LABELS: Record<GoalStatus, () => string> = {
    active: () => t("codexGoal_statusActive"),
    paused: () => t("codexGoal_statusPaused"),
    blocked: () => t("codexGoal_statusBlocked"),
    usageLimited: () => t("codexGoal_statusUsageLimited"),
    budgetLimited: () => t("codexGoal_statusBudgetLimited"),
    complete: () => t("codexGoal_statusComplete"),
  };

  function statusLabel(s: GoalStatus | undefined): string {
    return s ? (STATUS_LABELS[s]?.() ?? s) : t("codexGoal_statusNone");
  }

  // Populate the form from the latest goal whenever the modal opens or a fresh
  // goal arrives — but never overwrite an unsaved edit.
  $effect(() => {
    if (!open) return;
    if (dirty) return;
    objective = goal?.objective ?? "";
    tokenBudget = goal?.tokenBudget != null ? String(goal.tokenBudget) : "";
  });

  // On open, fetch the authoritative goal once (store may be stale/empty).
  $effect(() => {
    if (open && runId) void load();
  });

  async function load() {
    if (!runId) return;
    loading = true;
    error = "";
    try {
      const fetched = await api.getGoal(runId);
      if (!dirty) {
        objective = fetched?.objective ?? "";
        tokenBudget = fetched?.tokenBudget != null ? String(fetched.tokenBudget) : "";
      }
    } catch (e) {
      dbgWarn("goal", "getGoal failed", e);
      error = t("codexGoal_loadFailed");
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!runId) return;
    saving = true;
    error = "";
    try {
      const budget = tokenBudget.trim() ? parseInt(tokenBudget.trim(), 10) : undefined;
      await api.setGoal(runId, {
        objective: objective.trim(),
        ...(budget != null && !Number.isNaN(budget) ? { tokenBudget: budget } : {}),
      });
      dirty = false;
      // Live progress flows back via goal_update; no need to re-fetch.
    } catch (e) {
      dbgWarn("goal", "setGoal failed", e);
      error = t("codexGoal_saveFailed");
    } finally {
      saving = false;
    }
  }

  async function clear() {
    if (!runId) return;
    saving = true;
    error = "";
    try {
      await api.clearGoal(runId);
      objective = "";
      tokenBudget = "";
      dirty = false;
    } catch (e) {
      dbgWarn("goal", "clearGoal failed", e);
      error = t("codexGoal_clearFailed");
    } finally {
      saving = false;
    }
  }

  // Live progress (read from the store goal, not the editable form fields).
  let tokensUsed = $derived(goal?.tokensUsed ?? 0);
  let budgetForBar = $derived(goal?.tokenBudget ?? 0);
  let pct = $derived(
    budgetForBar > 0 ? Math.min(100, Math.round((tokensUsed / budgetForBar) * 100)) : 0,
  );
  let timeUsed = $derived(goal?.timeUsedSeconds ?? 0);

  function fmtDuration(seconds: number): string {
    if (seconds < 60) return `${Math.round(seconds)}s`;
    const m = Math.floor(seconds / 60);
    const s = Math.round(seconds % 60);
    return s > 0 ? `${m}m ${s}s` : `${m}m`;
  }
</script>

<Modal bind:open title={t("codexGoal_title")}>
  <div class="space-y-4 min-w-[440px] max-w-[560px]">
    <!-- Objective -->
    <div class="space-y-1.5">
      <label class="text-xs font-medium text-muted-foreground" for="codex-goal-objective">
        {t("codexGoal_objectiveLabel")}
      </label>
      <textarea
        id="codex-goal-objective"
        class="w-full rounded-md border px-3 py-2 text-sm bg-background min-h-[72px]"
        placeholder={t("codexGoal_objectivePlaceholder")}
        bind:value={objective}
        oninput={() => (dirty = true)}
        disabled={loading || saving}
      ></textarea>
    </div>

    <!-- Token budget -->
    <div class="space-y-1.5">
      <label class="text-xs font-medium text-muted-foreground" for="codex-goal-budget">
        {t("codexGoal_budgetLabel")}
      </label>
      <input
        id="codex-goal-budget"
        type="number"
        min="0"
        class="w-full rounded-md border px-3 py-2 text-sm bg-background font-mono"
        placeholder={t("codexGoal_budgetPlaceholder")}
        bind:value={tokenBudget}
        oninput={() => (dirty = true)}
        disabled={loading || saving}
      />
    </div>

    <!-- Live progress -->
    <div class="rounded-md border bg-muted/30 p-3 space-y-2">
      <div class="flex items-center justify-between text-xs">
        <span class="text-muted-foreground">{t("codexGoal_statusLabel")}</span>
        <span class="font-medium">{statusLabel(goal?.status)}</span>
      </div>
      <div class="flex items-center justify-between text-xs">
        <span class="text-muted-foreground">{t("codexGoal_tokensUsedLabel")}</span>
        <span class="font-mono">
          {fmtNumber(tokensUsed)}{#if budgetForBar > 0}
            / {fmtNumber(budgetForBar)}{/if}
        </span>
      </div>
      {#if budgetForBar > 0}
        <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
          <div
            class="h-full rounded-full transition-all {pct >= 100
              ? 'bg-destructive'
              : 'bg-primary'}"
            style="width: {pct}%"
          ></div>
        </div>
      {/if}
      <div class="flex items-center justify-between text-xs">
        <span class="text-muted-foreground">{t("codexGoal_timeUsedLabel")}</span>
        <span class="font-mono">{fmtDuration(timeUsed)}</span>
      </div>
    </div>

    {#if error}
      <p class="text-xs text-destructive">{error}</p>
    {/if}

    <div class="flex justify-between gap-2 pt-1">
      <button
        class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent disabled:opacity-50"
        onclick={clear}
        disabled={saving || loading || (!goal?.objective && !objective.trim())}
      >
        {t("codexGoal_clear")}
      </button>
      <div class="flex gap-2">
        <button
          class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent"
          onclick={() => (open = false)}
        >
          {t("common_close")}
        </button>
        <button
          class="rounded-md px-3 py-1.5 text-sm bg-primary text-primary-foreground disabled:opacity-50"
          disabled={saving || loading || !objective.trim()}
          onclick={save}
        >
          {saving ? t("codexGoal_saving") : t("codexGoal_save")}
        </button>
      </div>
    </div>
  </div>
</Modal>

<script lang="ts">
  /**
   * Context & Resources panel for the Tool Activity sidebar.
   * Shows: context snapshot + delta, resource summary (cost/tokens),
   * and per-turn history for both context and cost.
   */
  import type { ContextSnapshot, SessionInfoData } from "$lib/types";
  import type { TurnUsage } from "$lib/stores/types";
  import { getColor, getIcon, computeContextDelta } from "$lib/utils/context-parser";
  import { t } from "$lib/i18n/index.svelte";

  let {
    history = [],
    turnUsages = [],
    sessionInfo = null,
  }: {
    history: ContextSnapshot[];
    turnUsages?: TurnUsage[];
    sessionInfo?: SessionInfoData | null;
  } = $props();

  let latest = $derived(history.length > 0 ? history[history.length - 1] : null);
  let previous = $derived(history.length > 1 ? history[history.length - 2] : null);
  let delta = $derived(latest && previous ? computeContextDelta(previous.data, latest.data) : null);

  // ── Merged per-turn history (union of context snapshots + turnUsages) ──
  // Context snapshots and TurnUsages may use different turnIndex conventions,
  // so we merge by turnIndex and display whatever data is available for each turn.
  interface MergedTurnEntry {
    turnIndex: number;
    ts: string;
    snap?: ContextSnapshot;
    prevSnap?: ContextSnapshot;
    tu?: TurnUsage;
  }

  let mergedHistory = $derived.by((): MergedTurnEntry[] => {
    const map = new Map<number, MergedTurnEntry>();

    // Index context snapshots by turnIndex
    for (const snap of history) {
      map.set(snap.turnIndex, { turnIndex: snap.turnIndex, ts: snap.ts, snap });
    }

    // Merge turnUsages — add to existing entries or create new ones
    for (const tu of turnUsages) {
      const existing = map.get(tu.turnIndex);
      if (existing) {
        existing.tu = tu;
      } else {
        map.set(tu.turnIndex, { turnIndex: tu.turnIndex, ts: "", tu });
      }
    }

    // Sort ascending, then attach prevSnap for delta
    const sorted = [...map.values()].sort((a, b) => a.turnIndex - b.turnIndex);
    for (let i = 0; i < sorted.length; i++) {
      if (sorted[i].snap && i > 0) {
        // Find previous entry that also has a snap
        for (let j = i - 1; j >= 0; j--) {
          if (sorted[j].snap) {
            sorted[i].prevSnap = sorted[j].snap;
            break;
          }
        }
      }
    }
    return sorted;
  });

  // Track which history entries are expanded (keyed by turnIndex)
  let expandedEntries = $state<Set<number>>(new Set());

  function toggleEntry(index: number) {
    const next = new Set(expandedEntries);
    if (next.has(index)) next.delete(index);
    else next.add(index);
    expandedEntries = next;
  }

  function formatDelta(d: number): string {
    if (d === 0) return "\u2014";
    const sign = d > 0 ? "\u25b2" : "\u25bd";
    return `${sign}${Math.abs(d).toFixed(1)}%`;
  }

  function deltaColor(d: number): string {
    if (d > 0) return "text-amber-500";
    if (d < 0) return "text-emerald-500";
    return "text-muted-foreground/50";
  }

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    } catch {
      return "";
    }
  }

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
    if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
    return String(n);
  }

  function formatCost(n: number): string {
    if (n >= 1) return "$" + n.toFixed(2);
    if (n >= 0.01) return "$" + n.toFixed(3);
    if (n > 0) return "$" + n.toFixed(4);
    return "$0.00";
  }

  function formatDuration(ms: number): string {
    if (ms <= 0) return "—";
    const totalSec = Math.floor(ms / 1000);
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    if (m > 0) return `${m}m ${s}s`;
    return `${s}s`;
  }

  // Latest turn usage (for showing deltas in resource summary)
  let latestTurnUsage = $derived(turnUsages.length > 0 ? turnUsages[turnUsages.length - 1] : null);

  // Per-turn cost delta for RESOURCES section.
  // TurnUsage.cost is cumulative (total_cost_usd), so delta = current - previous.
  let latestCostDelta = $derived.by(() => {
    if (turnUsages.length === 0) return 0;
    if (turnUsages.length === 1) return turnUsages[0].cost;
    return turnUsages[turnUsages.length - 1].cost - turnUsages[turnUsages.length - 2].cost;
  });

  /** Compute per-turn cost from cumulative total_cost_usd values. */
  function getTurnCost(tu: TurnUsage): number {
    const idx = turnUsages.indexOf(tu);
    if (idx <= 0) return tu.cost; // first turn: cumulative = per-turn
    return Math.max(0, tu.cost - turnUsages[idx - 1].cost);
  }

  function formatResourceDelta(value: number): string {
    if (value <= 0) return "";
    return "+" + formatTokens(value);
  }

  function formatCostDelta(value: number): string {
    if (value <= 0) return "";
    return "+" + formatCost(value);
  }

  // History entries with actual TurnUsage (real turns, excluding context-only baseline)
  let displayHistory = $derived(mergedHistory.filter((e) => e.tu));

  // Whether we have any resource data to show
  let hasResourceData = $derived(
    (sessionInfo && (sessionInfo.cost > 0 || sessionInfo.inputTokens > 0)) || turnUsages.length > 0,
  );
</script>

<div class="flex flex-col h-full overflow-hidden">
  {#if !latest && !hasResourceData}
    <!-- Empty state -->
    <div
      class="flex items-center justify-center h-32 text-xs text-muted-foreground/50 px-4 text-center"
    >
      {t("contextPanel_noData")}
    </div>
  {:else}
    <div class="flex-1 overflow-y-auto">
      <!-- Context: Latest snapshot -->
      {#if latest}
        <div class="px-3 py-2 border-b border-border/50">
          <div
            class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5"
          >
            {t("contextPanel_latest")}
          </div>

          <!-- Model + overall percentage -->
          <div class="text-xs text-foreground">
            {latest.data.model}
            <span class="text-muted-foreground">
              &middot; {latest.data.percentage}% ({latest.data.usedTokens}/{latest.data.maxTokens})
            </span>
          </div>

          <!-- Progress bar -->
          <div class="mt-1.5 h-2 rounded-full bg-muted overflow-hidden flex">
            {#each latest.data.categories as cat}
              {#if cat.name !== "Free space" && cat.name !== "Autocompact buffer" && cat.percentage > 0}
                <div
                  class="h-full"
                  style="width: {cat.percentage}%; background-color: {getColor(cat.name)};"
                  title="{cat.name}: {cat.percentage}%"
                ></div>
              {/if}
            {/each}
          </div>

          <!-- Delta badge -->
          {#if delta}
            <div class="mt-1 text-[11px] {deltaColor(delta.pctDelta)}">
              {formatDelta(delta.pctDelta)}
            </div>
          {/if}

          <!-- Category breakdown -->
          <div class="mt-2 space-y-0.5">
            {#each latest.data.categories as cat}
              {@const catDelta = delta?.categoryDeltas.find((d) => d.name === cat.name)}
              <div class="flex items-center gap-1.5 text-[11px]">
                <span style="color: {getColor(cat.name)}">{getIcon(cat.name)}</span>
                <span class="text-muted-foreground flex-1 min-w-0 truncate">
                  {cat.name}
                </span>
                <span class="text-muted-foreground/70 tabular-nums">
                  {cat.tokens} ({cat.percentage}%)
                </span>
                {#if catDelta && catDelta.pctDelta !== 0}
                  <span
                    class="text-[10px] {deltaColor(catDelta.pctDelta)} tabular-nums w-12 text-right"
                  >
                    {formatDelta(catDelta.pctDelta)}
                  </span>
                {:else}
                  <span class="text-[10px] text-muted-foreground/30 w-12 text-right">&mdash;</span>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Resource summary (cumulative cost/tokens) -->
      {#if sessionInfo && (sessionInfo.cost > 0 || sessionInfo.inputTokens > 0)}
        <div class="px-3 py-2 border-b border-border/50">
          <div
            class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5"
          >
            {t("infoPanel_resources")}
          </div>
          <div class="space-y-0.5">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_cost")}</span>
              <span class="flex items-center gap-1">
                <span class="text-foreground/80 font-mono tabular-nums"
                  >{formatCost(sessionInfo.cost)}</span
                >
                {#if latestCostDelta > 0}
                  <span class="text-[10px] text-amber-500 font-mono tabular-nums"
                    >{formatCostDelta(latestCostDelta)}</span
                  >
                {/if}
              </span>
            </div>
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_inputTokens")}</span>
              <span class="flex items-center gap-1">
                <span class="text-foreground/80 font-mono tabular-nums">
                  {formatTokens(sessionInfo.inputTokens)}
                  {#if sessionInfo.tokensEstimated}<span class="text-muted-foreground/50 text-[9px]"
                      >{t("infoPanel_estimated")}</span
                    >{/if}
                </span>
                {#if latestTurnUsage && latestTurnUsage.inputTokens > 0}
                  <span class="text-[10px] text-amber-500 font-mono tabular-nums"
                    >{formatResourceDelta(latestTurnUsage.inputTokens)}</span
                  >
                {/if}
              </span>
            </div>
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_outputTokens")}</span>
              <span class="flex items-center gap-1">
                <span class="text-foreground/80 font-mono tabular-nums">
                  {formatTokens(sessionInfo.outputTokens)}
                  {#if sessionInfo.tokensEstimated}<span class="text-muted-foreground/50 text-[9px]"
                      >{t("infoPanel_estimated")}</span
                    >{/if}
                </span>
                {#if latestTurnUsage && latestTurnUsage.outputTokens > 0}
                  <span class="text-[10px] text-amber-500 font-mono tabular-nums"
                    >{formatResourceDelta(latestTurnUsage.outputTokens)}</span
                  >
                {/if}
              </span>
            </div>
            {#if sessionInfo.cacheReadTokens > 0 || sessionInfo.cacheWriteTokens > 0}
              <div class="flex items-center justify-between text-[11px]">
                <span class="text-muted-foreground">{t("infoPanel_cacheRead")}</span>
                <span class="flex items-center gap-1">
                  <span class="text-foreground/80 font-mono tabular-nums"
                    >{formatTokens(sessionInfo.cacheReadTokens)}</span
                  >
                  {#if latestTurnUsage && latestTurnUsage.cacheReadTokens > 0}
                    <span class="text-[10px] text-amber-500 font-mono tabular-nums"
                      >{formatResourceDelta(latestTurnUsage.cacheReadTokens)}</span
                    >
                  {/if}
                </span>
              </div>
              <div class="flex items-center justify-between text-[11px]">
                <span class="text-muted-foreground">{t("infoPanel_cacheWrite")}</span>
                <span class="flex items-center gap-1">
                  <span class="text-foreground/80 font-mono tabular-nums"
                    >{formatTokens(sessionInfo.cacheWriteTokens)}</span
                  >
                  {#if latestTurnUsage && latestTurnUsage.cacheWriteTokens > 0}
                    <span class="text-[10px] text-amber-500 font-mono tabular-nums"
                      >{formatResourceDelta(latestTurnUsage.cacheWriteTokens)}</span
                    >
                  {/if}
                </span>
              </div>
            {/if}
            {#if sessionInfo.contextWindow > 0}
              <div class="flex items-center justify-between text-[11px]">
                <span class="text-muted-foreground">{t("infoPanel_contextWindow")}</span>
                <span class="text-foreground/80 font-mono tabular-nums">
                  {formatTokens(sessionInfo.contextWindow)}
                  <span class="text-muted-foreground/50 text-[9px] ml-0.5"
                    >({Math.round(sessionInfo.contextUtilization * 100)}%)</span
                  >
                </span>
              </div>
            {/if}
            {#if sessionInfo.compactCount > 0 || sessionInfo.microcompactCount > 0}
              <div class="flex items-center justify-between text-[11px]">
                <span class="text-muted-foreground">{t("infoPanel_compactions")}</span>
                <span class="text-foreground/80"
                  >{sessionInfo.compactCount} + {sessionInfo.microcompactCount}μ</span
                >
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- CLI import usage incomplete hint (independent of Resources block) -->
      {#if sessionInfo?.cliUsageIncomplete && sessionInfo?.runSource === "cli_import"}
        <div class="px-3 py-1.5 border-b border-border/50">
          <span class="text-[10px] text-yellow-600 dark:text-yellow-400"
            >{t("cliSync_usageIncompleteHint")}</span
          >
        </div>
      {/if}

      <!-- Per-turn history (context + cost merged via union) -->
      <!-- Per-turn history: only entries with actual TurnUsage (real turns).
           Context-only baseline (T0) is used for delta computation but not displayed.
           Single-turn sessions don't show HISTORY (LATEST + RESOURCES suffice). -->
      {#if displayHistory.length > 1}
        <div class="px-3 py-2 border-b border-border/50">
          <div class="text-[11px] font-semibold text-muted-foreground uppercase tracking-wider">
            {t("contextPanel_history")}
          </div>
        </div>
        <div class="flex-1 overflow-y-auto">
          {#each displayHistory.toReversed() as entry (entry.turnIndex)}
            {@const entryDelta =
              entry.snap && entry.prevSnap
                ? computeContextDelta(entry.prevSnap.data, entry.snap.data)
                : null}
            {@const isExpanded = expandedEntries.has(entry.turnIndex)}
            <button
              class="w-full text-left px-3 py-1.5 hover:bg-accent/50 transition-colors text-[11px] border-b border-border/20"
              onclick={() => toggleEntry(entry.turnIndex)}
            >
              <div class="flex items-center gap-2">
                <span class="text-muted-foreground font-medium">
                  {t("contextPanel_turn", { index: String(entry.turnIndex) })}
                </span>
                {#if entry.ts}
                  <span class="text-muted-foreground/50">{formatTime(entry.ts)}</span>
                {/if}
                {#if entry.snap}
                  <span class="ml-auto tabular-nums text-foreground/70"
                    >{entry.snap.data.percentage}%</span
                  >
                {/if}
                {#if entryDelta}
                  <span class="text-[10px] {deltaColor(entryDelta.pctDelta)} tabular-nums">
                    {formatDelta(entryDelta.pctDelta)}
                  </span>
                {/if}
              </div>
              {#if entry.tu}
                <div class="flex items-center gap-2 mt-0.5 text-[10px] text-muted-foreground/50">
                  <span>{formatCost(getTurnCost(entry.tu))}</span>
                  <span>{formatTokens(entry.tu.inputTokens + entry.tu.outputTokens)} tok</span>
                  {#if entry.tu.durationMs}
                    <span>{formatDuration(entry.tu.durationMs)}</span>
                  {/if}
                </div>
              {/if}
            </button>
            {#if isExpanded}
              <div class="px-4 py-1.5 bg-muted/20 border-b border-border/20">
                {#if entry.snap}
                  {#each entry.snap.data.categories as cat}
                    <div class="flex items-center gap-1.5 text-[10px] py-0.5">
                      <span style="color: {getColor(cat.name)}">{getIcon(cat.name)}</span>
                      <span class="text-muted-foreground flex-1 min-w-0 truncate">{cat.name}</span>
                      <span class="text-muted-foreground/70 tabular-nums">
                        {cat.tokens} ({cat.percentage}%)
                      </span>
                    </div>
                  {/each}
                {/if}
                {#if entry.tu}
                  <div
                    class="{entry.snap ? 'mt-1 pt-1 border-t border-border/20' : ''} space-y-0.5"
                  >
                    <div class="flex items-center justify-between text-[10px]">
                      <span class="text-muted-foreground">{t("infoPanel_cost")}</span>
                      <span class="text-foreground/70 font-mono tabular-nums"
                        >{formatCost(getTurnCost(entry.tu))}</span
                      >
                    </div>
                    <div class="flex items-center justify-between text-[10px]">
                      <span class="text-muted-foreground">{t("infoPanel_inputTokens")}</span>
                      <span class="text-foreground/70 font-mono tabular-nums"
                        >{formatTokens(entry.tu.inputTokens)}</span
                      >
                    </div>
                    <div class="flex items-center justify-between text-[10px]">
                      <span class="text-muted-foreground">{t("infoPanel_outputTokens")}</span>
                      <span class="text-foreground/70 font-mono tabular-nums"
                        >{formatTokens(entry.tu.outputTokens)}</span
                      >
                    </div>
                    {#if entry.tu.durationMs}
                      <div class="flex items-center justify-between text-[10px]">
                        <span class="text-muted-foreground">{t("infoPanel_lastTurn")}</span>
                        <span class="text-foreground/70 font-mono tabular-nums"
                          >{formatDuration(entry.tu.durationMs)}</span
                        >
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            {/if}
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

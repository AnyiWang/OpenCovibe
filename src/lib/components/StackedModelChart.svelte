<script lang="ts">
  import type { DailyAggregate } from "$lib/types";
  import { getModelColorIndex } from "$lib/utils/chart-helpers";
  import { formatTokenCount } from "$lib/utils/format";

  let {
    daily,
    maxDays = 30,
  }: {
    daily: DailyAggregate[];
    maxDays?: number;
  } = $props();

  const MODEL_COLORS = [
    "rgb(16, 185, 129)", // emerald
    "rgb(59, 130, 246)", // blue
    "rgb(245, 158, 11)", // amber
    "rgb(244, 63, 94)", // rose
    "rgb(139, 92, 246)", // violet
    "rgb(6, 182, 212)", // cyan
    "rgb(249, 115, 22)", // orange
    "rgb(132, 204, 22)", // lime
  ];

  let sliced = $derived(daily.slice(-maxDays));

  // Collect all unique models
  let allModels = $derived.by(() => {
    const set = new Set<string>();
    for (const day of sliced) {
      if (day.modelBreakdown) {
        for (const model of Object.keys(day.modelBreakdown)) {
          set.add(model);
        }
      }
    }
    return [...set].sort();
  });

  // Max stacked value
  let maxValue = $derived.by(() => {
    let max = 1;
    for (const day of sliced) {
      if (day.modelBreakdown) {
        let total = 0;
        for (const mt of Object.values(day.modelBreakdown)) {
          total += mt.inputTokens + mt.outputTokens;
        }
        max = Math.max(max, total);
      } else {
        max = Math.max(max, day.inputTokens + day.outputTokens);
      }
    }
    return max;
  });

  function getColor(model: string): string {
    return MODEL_COLORS[getModelColorIndex(model)];
  }

  function formatShortDate(dateStr: string): string {
    return dateStr.slice(5); // "MM-DD"
  }

  function barTooltip(day: DailyAggregate): string {
    if (!day.modelBreakdown) {
      return `${day.date}\n${formatTokenCount(day.inputTokens + day.outputTokens)} tokens`;
    }
    const lines = [day.date];
    for (const [model, mt] of Object.entries(day.modelBreakdown)) {
      lines.push(`${model}: ${formatTokenCount(mt.inputTokens + mt.outputTokens)}`);
    }
    return lines.join("\n");
  }
</script>

<div class="space-y-3">
  <div class="flex h-40">
    <!-- Y-axis -->
    <div
      class="flex flex-col justify-between items-end pr-2 text-[9px] text-muted-foreground tabular-nums shrink-0 py-0.5"
    >
      <span>{formatTokenCount(maxValue)}</span>
      <span>{formatTokenCount(maxValue / 2)}</span>
      <span>0</span>
    </div>

    <!-- Bars -->
    <div class="flex-1 flex flex-col min-w-0">
      <div class="flex-1 flex gap-[2px] border-l border-b border-border/50 relative">
        <div class="absolute inset-x-0 top-1/2 border-t border-border/30 pointer-events-none"></div>
        {#each sliced as day}
          {@const hasBreakdown = day.modelBreakdown && Object.keys(day.modelBreakdown).length > 0}
          {@const totalTokens = hasBreakdown
            ? Object.values(day.modelBreakdown!).reduce(
                (s, mt) => s + mt.inputTokens + mt.outputTokens,
                0,
              )
            : day.inputTokens + day.outputTokens}
          {@const pct = Math.max((totalTokens / maxValue) * 100, 2)}
          <div class="flex-1 min-w-0 flex items-end group cursor-default" title={barTooltip(day)}>
            <div
              class="w-full rounded-t overflow-hidden flex flex-col-reverse"
              style="height: {pct}%"
            >
              {#if hasBreakdown}
                {#each allModels as model}
                  {@const mt = day.modelBreakdown![model]}
                  {#if mt}
                    {@const segTokens = mt.inputTokens + mt.outputTokens}
                    {@const segPct = totalTokens > 0 ? (segTokens / totalTokens) * 100 : 0}
                    <div
                      style="height: {segPct}%; background-color: {getColor(model)}; opacity: 0.7;"
                      class="w-full group-hover:opacity-100 transition-opacity min-h-0"
                    ></div>
                  {/if}
                {/each}
              {:else}
                <div
                  class="w-full h-full bg-primary/60 group-hover:bg-primary transition-colors"
                ></div>
              {/if}
            </div>
          </div>
        {/each}
      </div>

      <!-- X-axis -->
      <div class="flex gap-[2px] mt-1">
        {#each sliced as day, i}
          {@const showLabel = sliced.length <= 10 || i % Math.ceil(sliced.length / 10) === 0}
          <div class="flex-1 min-w-0 text-center">
            {#if showLabel}
              <span class="text-[8px] text-muted-foreground tabular-nums">
                {formatShortDate(day.date)}
              </span>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  </div>

  <!-- Legend -->
  {#if allModels.length > 0}
    <div class="flex flex-wrap gap-x-3 gap-y-1 text-[10px] text-muted-foreground">
      {#each allModels as model}
        <div class="flex items-center gap-1">
          <div
            class="w-2.5 h-2.5 rounded-sm shrink-0"
            style="background-color: {getColor(model)};"
          ></div>
          <span class="truncate max-w-[120px]" title={model}>{model}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<script lang="ts">
  import type { DailyAggregate } from "$lib/types";
  import { buildWeekGrid } from "$lib/utils/chart-helpers";
  import { formatCost, formatTokenCount } from "$lib/utils/format";
  import { t } from "$lib/i18n/index.svelte";
  import { fmtNumber } from "$lib/i18n/format";

  let {
    daily,
    metric = "cost",
  }: {
    daily: DailyAggregate[];
    metric: "cost" | "tokens" | "messages" | "sessions";
  } = $props();

  const CELL = 11;
  const GAP = 2;
  const STEP = CELL + GAP; // 13px per cell+gap
  const DAY_LABEL_W = 26; // width of day-label column
  const DAY_LABELS = ["Mon", "", "Wed", "", "Fri", "", ""];

  let grid = $derived(buildWeekGrid(daily, metric));

  function cellTooltip(date: string, value: number): string {
    let formatted: string;
    if (metric === "cost") formatted = formatCost(value);
    else if (metric === "tokens") formatted = formatTokenCount(value);
    else formatted = fmtNumber(value);
    return `${date}: ${formatted}`;
  }
</script>

<div class="space-y-1">
  <!-- Month labels row -->
  <div
    class="relative text-[9px] text-muted-foreground select-none"
    style="height: 14px; margin-left: {DAY_LABEL_W}px;"
  >
    {#each grid.monthLabels as ml}
      <span class="absolute top-0 whitespace-nowrap" style="left: {ml.col * STEP}px;">
        {ml.label}
      </span>
    {/each}
  </div>

  <!-- Day labels + Grid -->
  <div class="flex">
    <!-- Day labels -->
    <div
      class="shrink-0 flex flex-col text-[9px] text-muted-foreground select-none"
      style="width: {DAY_LABEL_W}px;"
    >
      {#each DAY_LABELS as label}
        <span style="height: {STEP}px; line-height: {STEP}px;">{label}</span>
      {/each}
    </div>

    <!-- Grid: CSS Grid for precise cell placement -->
    <div
      class="overflow-x-auto overflow-y-hidden"
      style="display: grid; grid-template-rows: repeat(7, {CELL}px); grid-template-columns: repeat({grid.weeks}, {CELL}px); gap: {GAP}px;"
    >
      {#each { length: grid.weeks } as _, col}
        {#each { length: 7 } as _, row}
          {@const cell = grid.cells[row][col]}
          {#if cell}
            <div
              class="rounded-[2px] {cell.level === 0
                ? 'bg-muted/30'
                : cell.level === 1
                  ? 'bg-primary/20'
                  : cell.level === 2
                    ? 'bg-primary/40'
                    : cell.level === 3
                      ? 'bg-primary/65'
                      : 'bg-primary'}"
              style="grid-row: {row + 1}; grid-column: {col + 1};"
              title={cellTooltip(cell.date, cell.value)}
            ></div>
          {:else}
            <div style="grid-row: {row + 1}; grid-column: {col + 1};"></div>
          {/if}
        {/each}
      {/each}
    </div>
  </div>

  <!-- Legend -->
  <div class="flex items-center gap-1 text-[9px] text-muted-foreground justify-end select-none">
    <span>{t("usage_heatmapLess")}</span>
    <div class="rounded-[2px] bg-muted/30" style="width: {CELL}px; height: {CELL}px;"></div>
    <div class="rounded-[2px] bg-primary/20" style="width: {CELL}px; height: {CELL}px;"></div>
    <div class="rounded-[2px] bg-primary/40" style="width: {CELL}px; height: {CELL}px;"></div>
    <div class="rounded-[2px] bg-primary/65" style="width: {CELL}px; height: {CELL}px;"></div>
    <div class="rounded-[2px] bg-primary" style="width: {CELL}px; height: {CELL}px;"></div>
    <span>{t("usage_heatmapMore")}</span>
  </div>
</div>

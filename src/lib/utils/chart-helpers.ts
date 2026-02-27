import type { DailyAggregate } from "$lib/types";

/** Compute [p25, p50, p75] from non-zero values. Returns [0,0,0] if empty. */
export function computePercentileThresholds(values: number[]): [number, number, number] {
  const nonZero = values.filter((v) => v > 0).sort((a, b) => a - b);
  if (nonZero.length === 0) return [0, 0, 0];
  const p = (pct: number) => {
    const idx = Math.floor((pct / 100) * (nonZero.length - 1));
    return nonZero[idx];
  };
  return [p(25), p(50), p(75)];
}

/** Map a value to heatmap level 0-4 based on percentile thresholds. */
export function valueToLevel(
  value: number,
  thresholds: [number, number, number],
): 0 | 1 | 2 | 3 | 4 {
  if (value <= 0) return 0;
  if (value <= thresholds[0]) return 1;
  if (value <= thresholds[1]) return 2;
  if (value <= thresholds[2]) return 3;
  return 4;
}

export interface GridCell {
  date: string;
  value: number;
  level: 0 | 1 | 2 | 3 | 4;
}

export interface MonthLabel {
  label: string;
  col: number;
}

export interface GridResult {
  cells: (GridCell | null)[][]; // [row 0..6][col 0..N]
  monthLabels: MonthLabel[];
  weeks: number;
}

/**
 * Build a 7×N week grid from daily data, right-aligned to today.
 * Rows: 0=Mon, 1=Tue, ..., 6=Sun.
 * All date math uses UTC to match backend daily aggregation boundaries.
 */
export function buildWeekGrid(daily: DailyAggregate[], metric: string, weeks = 53): GridResult {
  const MIN_MONTH_LABEL_GAP_COLS = 3;

  const now = new Date();
  const today = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()));

  const todayDay = (today.getUTCDay() + 6) % 7; // 0=Mon
  const startDate = new Date(today);
  startDate.setUTCDate(startDate.getUTCDate() - todayDay - (weeks - 1) * 7);

  const valueMap = new Map<string, number>();
  for (const d of daily) {
    let v = 0;
    if (metric === "cost") v = d.costUsd;
    else if (metric === "tokens") v = d.inputTokens + d.outputTokens;
    else if (metric === "messages") v = d.messageCount ?? 0;
    else if (metric === "sessions") v = d.sessionCount ?? 0;
    else v = d.inputTokens + d.outputTokens;
    valueMap.set(d.date, v);
  }

  const thresholds = computePercentileThresholds([...valueMap.values()]);
  const cells: (GridCell | null)[][] = Array.from({ length: 7 }, () =>
    Array.from({ length: weeks }, () => null),
  );

  const monthLabels: MonthLabel[] = [];
  let lastMonth = -1;

  for (let col = 0; col < weeks; col++) {
    for (let row = 0; row < 7; row++) {
      const d = new Date(startDate);
      d.setUTCDate(startDate.getUTCDate() + col * 7 + row);
      if (d.getTime() > today.getTime()) continue;

      const dateStr = d.toISOString().slice(0, 10);
      const value = valueMap.get(dateStr) ?? 0;
      cells[row][col] = { date: dateStr, value, level: valueToLevel(value, thresholds) };

      if (row === 0) {
        const month = d.getUTCMonth();
        if (month !== lastMonth) {
          const label = d.toLocaleDateString("en-US", { month: "short", timeZone: "UTC" });
          const prev = monthLabels[monthLabels.length - 1];
          if (prev && col - prev.col < MIN_MONTH_LABEL_GAP_COLS) {
            // Too close — replace previous label with current (newer month wins)
            monthLabels[monthLabels.length - 1] = { label, col };
          } else {
            monthLabels.push({ label, col });
          }
          lastMonth = month;
        }
      }
    }
  }

  return { cells, monthLabels, weeks };
}

/** Deterministic model name → color index (0-7). */
export function getModelColorIndex(modelName: string): number {
  let hash = 0;
  for (let i = 0; i < modelName.length; i++) {
    hash = (hash * 31 + modelName.charCodeAt(i)) | 0;
  }
  return ((hash % 8) + 8) % 8;
}

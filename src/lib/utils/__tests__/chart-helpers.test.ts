import { describe, it, expect } from "vitest";
import {
  computePercentileThresholds,
  valueToLevel,
  buildWeekGrid,
  getModelColorIndex,
} from "../chart-helpers";

describe("computePercentileThresholds", () => {
  it("normal distribution", () => {
    const values = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    const [p25, p50, p75] = computePercentileThresholds(values);
    expect(p25).toBeGreaterThan(0);
    expect(p50).toBeGreaterThanOrEqual(p25);
    expect(p75).toBeGreaterThanOrEqual(p50);
  });

  it("all same values", () => {
    const values = [5, 5, 5, 5];
    const [p25, p50, p75] = computePercentileThresholds(values);
    expect(p25).toBe(5);
    expect(p50).toBe(5);
    expect(p75).toBe(5);
  });

  it("empty", () => {
    expect(computePercentileThresholds([])).toEqual([0, 0, 0]);
  });
});

describe("valueToLevel", () => {
  it("boundary values", () => {
    const thresholds: [number, number, number] = [10, 20, 30];
    expect(valueToLevel(0, thresholds)).toBe(0);
    expect(valueToLevel(-1, thresholds)).toBe(0);
    expect(valueToLevel(5, thresholds)).toBe(1);
    expect(valueToLevel(10, thresholds)).toBe(1);
    expect(valueToLevel(15, thresholds)).toBe(2);
    expect(valueToLevel(25, thresholds)).toBe(3);
    expect(valueToLevel(50, thresholds)).toBe(4);
  });
});

describe("buildWeekGrid", () => {
  it("dimensions", () => {
    const result = buildWeekGrid([], "cost");
    expect(result.cells.length).toBe(7);
    expect(result.cells[0].length).toBeLessThanOrEqual(53);
  });

  it("empty data - all cells value 0", () => {
    const result = buildWeekGrid([], "cost");
    for (const row of result.cells) {
      for (const cell of row) {
        if (cell) expect(cell.value).toBe(0);
      }
    }
  });
});

describe("getModelColorIndex", () => {
  it("deterministic", () => {
    const a = getModelColorIndex("claude-opus-4");
    const b = getModelColorIndex("claude-opus-4");
    expect(a).toBe(b);
  });

  it("range 0-7", () => {
    const models = ["claude-opus-4", "claude-sonnet-4", "claude-haiku-3.5", "gpt-4o", "some-model"];
    for (const m of models) {
      const idx = getModelColorIndex(m);
      expect(idx).toBeGreaterThanOrEqual(0);
      expect(idx).toBeLessThanOrEqual(7);
    }
  });
});

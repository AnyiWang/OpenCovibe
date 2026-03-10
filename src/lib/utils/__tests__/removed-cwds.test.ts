import { describe, it, expect, beforeEach } from "vitest";
import { loadRemovedCwds, isRemovedCwd } from "../removed-cwds";

// ── Mock localStorage ──

const store: Record<string, string> = {};

beforeEach(() => {
  for (const key of Object.keys(store)) delete store[key];
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => store[key] ?? null,
      setItem: (key: string, value: string) => {
        store[key] = value;
      },
      removeItem: (key: string) => {
        delete store[key];
      },
    },
    writable: true,
    configurable: true,
  });
});

// ── loadRemovedCwds ──

describe("loadRemovedCwds", () => {
  it("returns empty array when nothing stored", () => {
    expect(loadRemovedCwds()).toEqual([]);
  });

  it("loads from current key", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["/projA", "/projB"]);
    const result = loadRemovedCwds();
    expect(result).toEqual(["/projA", "/projB"]);
  });

  it("merges legacy key and removes it", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["/projA"]);
    store["ocv:hidden-cwds"] = JSON.stringify(["/projB"]);
    const result = loadRemovedCwds();
    expect(result).toContain("/projA");
    expect(result).toContain("/projB");
    expect(result).toHaveLength(2);
    // Legacy key should be removed
    expect(store["ocv:hidden-cwds"]).toBeUndefined();
    // Merged result should be persisted
    expect(JSON.parse(store["ocv:removed-cwds"])).toEqual(result);
  });

  it("deduplicates after normalization", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["/proj", "/proj/", "/proj"]);
    const result = loadRemovedCwds();
    expect(result).toEqual(["/proj"]);
  });

  it("filters empty and root values", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["", "/", "  ", "/real"]);
    const result = loadRemovedCwds();
    expect(result).toEqual(["/real"]);
  });

  it("handles corrupted current key gracefully", () => {
    store["ocv:removed-cwds"] = "not-json";
    store["ocv:hidden-cwds"] = JSON.stringify(["/projB"]);
    const result = loadRemovedCwds();
    expect(result).toContain("/projB");
  });

  it("handles corrupted legacy key gracefully", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["/projA"]);
    store["ocv:hidden-cwds"] = "{bad}";
    const result = loadRemovedCwds();
    expect(result).toEqual(["/projA"]);
  });

  it("normalizes Windows paths", () => {
    store["ocv:removed-cwds"] = JSON.stringify(["c:\\Users\\proj\\"]);
    const result = loadRemovedCwds();
    expect(result).toEqual(["C:/Users/proj"]);
  });
});

// ── isRemovedCwd ──

describe("isRemovedCwd", () => {
  it("returns false for empty cwd (Uncategorized)", () => {
    const set = new Set([""]);
    expect(isRemovedCwd("", set)).toBe(false);
  });

  it("returns true for removed cwd", () => {
    const set = new Set(["/projA"]);
    expect(isRemovedCwd("/projA", set)).toBe(true);
  });

  it("returns false for non-removed cwd", () => {
    const set = new Set(["/projA"]);
    expect(isRemovedCwd("/projB", set)).toBe(false);
  });

  it("normalizes before checking", () => {
    const set = new Set(["C:/Users/proj"]);
    expect(isRemovedCwd("c:\\Users\\proj\\", set)).toBe(true);
  });

  it("returns false for undefined/root", () => {
    const set = new Set(["/proj"]);
    expect(isRemovedCwd("/", set)).toBe(false);
  });
});

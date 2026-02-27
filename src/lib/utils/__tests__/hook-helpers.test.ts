import { describe, it, expect } from "vitest";
import {
  ensureHooksObject,
  normalizeForDisplay,
  addGroup,
  removeGroup,
  patchGroup,
  HOOK_EVENT_TYPES,
} from "../hook-helpers";

describe("HOOK_EVENT_TYPES", () => {
  it("contains expected event types", () => {
    expect(HOOK_EVENT_TYPES).toContain("PreToolUse");
    expect(HOOK_EVENT_TYPES).toContain("PostToolUse");
    expect(HOOK_EVENT_TYPES).toContain("Notification");
    expect(HOOK_EVENT_TYPES).toContain("Stop");
  });
});

describe("ensureHooksObject", () => {
  it("returns {} for null/undefined", () => {
    expect(ensureHooksObject(null)).toEqual({});
    expect(ensureHooksObject(undefined)).toEqual({});
  });
  it("returns {} for array", () => {
    expect(ensureHooksObject([1, 2])).toEqual({});
  });
  it("returns {} for primitive", () => {
    expect(ensureHooksObject("string")).toEqual({});
    expect(ensureHooksObject(42)).toEqual({});
  });
  it("returns same object reference for plain object", () => {
    const obj = { PreToolUse: [] };
    expect(ensureHooksObject(obj)).toBe(obj);
  });
});

describe("normalizeForDisplay", () => {
  it("returns {} for non-object", () => {
    expect(normalizeForDisplay(null)).toEqual({});
    expect(normalizeForDisplay("str")).toEqual({});
    expect(normalizeForDisplay([1])).toEqual({});
  });
  it("keeps only array-valued keys", () => {
    const raw = {
      PreToolUse: [{ hooks: [] }],
      _someString: "ignored",
      PostToolUse: [{ hooks: [] }],
      _someNumber: 42,
    };
    const result = normalizeForDisplay(raw);
    expect(Object.keys(result)).toEqual(["PreToolUse", "PostToolUse"]);
    expect(result.PreToolUse).toHaveLength(1);
    expect(result.PostToolUse).toHaveLength(1);
  });
});

describe("addGroup", () => {
  it("adds group to existing event array", () => {
    const raw = { PreToolUse: [{ matcher: "Bash" }] };
    const result = addGroup(raw, "PreToolUse", { matcher: "Read" });
    expect(result.PreToolUse).toHaveLength(2);
    expect((result.PreToolUse as Array<{ matcher: string }>)[1].matcher).toBe("Read");
  });
  it("creates event array if not present", () => {
    const result = addGroup({}, "Stop", { hooks: [{ type: "command", command: "echo hi" }] });
    expect(result.Stop).toHaveLength(1);
  });
  it("creates event array if current value is not array", () => {
    const result = addGroup({ Stop: "bad" }, "Stop", { hooks: [] });
    expect(Array.isArray(result.Stop)).toBe(true);
    expect(result.Stop).toHaveLength(1);
  });
  it("does not mutate original", () => {
    const raw = { PreToolUse: [{ matcher: "Bash" }] };
    addGroup(raw, "PreToolUse", { matcher: "Read" });
    expect(raw.PreToolUse).toHaveLength(1);
  });
  it("preserves unknown event keys", () => {
    const raw = { CustomEvent: [1, 2], PreToolUse: [] };
    const result = addGroup(raw, "PreToolUse", { hooks: [] });
    expect(result.CustomEvent).toEqual([1, 2]);
  });
});

describe("removeGroup", () => {
  it("removes group at index", () => {
    const raw = { PreToolUse: [{ matcher: "A" }, { matcher: "B" }, { matcher: "C" }] };
    const result = removeGroup(raw, "PreToolUse", 1);
    expect(result.PreToolUse).toHaveLength(2);
    expect((result.PreToolUse as Array<{ matcher: string }>)[0].matcher).toBe("A");
    expect((result.PreToolUse as Array<{ matcher: string }>)[1].matcher).toBe("C");
  });
  it("deletes event key when array becomes empty", () => {
    const raw = { PreToolUse: [{ matcher: "A" }] };
    const result = removeGroup(raw, "PreToolUse", 0);
    expect(result.PreToolUse).toBeUndefined();
    expect(Object.keys(result)).toEqual([]);
  });
  it("returns clone if event key is not an array", () => {
    const raw = { PreToolUse: "not-array" };
    const result = removeGroup(raw, "PreToolUse", 0);
    expect(result.PreToolUse).toBe("not-array");
  });
  it("does not mutate original", () => {
    const raw = { PreToolUse: [{ a: 1 }] };
    removeGroup(raw, "PreToolUse", 0);
    expect(raw.PreToolUse).toHaveLength(1);
  });
});

describe("patchGroup", () => {
  it("merges patch into existing group object", () => {
    const raw = { Stop: [{ hooks: [{ type: "command", command: "echo 1" }], unknownField: true }] };
    const result = patchGroup(raw, "Stop", 0, {
      hooks: [{ type: "command", command: "echo 2" }],
    });
    const group = (result.Stop as Array<Record<string, unknown>>)[0];
    // hooks replaced by patch
    expect(group.hooks).toEqual([{ type: "command", command: "echo 2" }]);
    // unknown field preserved via spread
    expect(group.unknownField).toBe(true);
  });
  it("replaces non-object group entirely", () => {
    const raw = { Stop: ["string-group"] };
    const result = patchGroup(raw, "Stop", 0, { hooks: [] });
    expect((result.Stop as unknown[])[0]).toEqual({ hooks: [] });
  });
  it("replaces array group entirely", () => {
    const raw = { Stop: [[1, 2, 3]] };
    const result = patchGroup(raw, "Stop", 0, { hooks: [] });
    expect((result.Stop as unknown[])[0]).toEqual({ hooks: [] });
  });
  it("returns clone if index out of range", () => {
    const raw = { Stop: [{ a: 1 }] };
    const result = patchGroup(raw, "Stop", 5, { b: 2 });
    expect(result.Stop).toEqual([{ a: 1 }]);
  });
  it("returns clone if event key is not an array", () => {
    const raw = { Stop: "bad" };
    const result = patchGroup(raw, "Stop", 0, { b: 2 });
    expect(result.Stop).toBe("bad");
  });
  it("does not mutate original", () => {
    const raw = { Stop: [{ hooks: [{ type: "command", command: "echo 1" }] }] };
    patchGroup(raw, "Stop", 0, { hooks: [] });
    expect((raw.Stop[0] as Record<string, unknown>).hooks).toHaveLength(1);
  });
});

import { describe, it, expect } from "vitest";
import {
  extractFilesFromTimeline,
  extractFilesFromHooks,
  extractFilesFromPersisted,
  mergeFileEntries,
} from "../file-entries";
import type { TimelineEntry, HookEvent, FileEntry } from "$lib/types";

// ── Helpers ──

function toolEntry(
  name: string,
  input: Record<string, unknown>,
  opts?: { id?: string; sub?: TimelineEntry[] },
): TimelineEntry {
  return {
    kind: "tool",
    id: opts?.id ?? `t-${name}-${Math.random().toString(36).slice(2, 6)}`,
    ts: new Date().toISOString(),
    tool: {
      tool_use_id: opts?.id ?? `tuid-${name}-${Math.random().toString(36).slice(2, 6)}`,
      tool_name: name,
      input,
      status: "success",
    },
    subTimeline: opts?.sub,
  };
}

function hookEvent(name: string, input: Record<string, unknown>): HookEvent {
  return {
    run_id: "r1",
    hook_type: "tool",
    tool_name: name,
    tool_input: input,
    status: "done",
    timestamp: new Date().toISOString(),
  };
}

// ── extractFilesFromTimeline ──

describe("extractFilesFromTimeline", () => {
  it("extracts Read/Write/Edit/NotebookEdit tools", () => {
    const timeline: TimelineEntry[] = [
      toolEntry("Read", { file_path: "src/a.ts" }, { id: "t1" }),
      toolEntry("Write", { file_path: "src/b.ts" }, { id: "t2" }),
      toolEntry("Edit", { file_path: "src/c.ts" }, { id: "t3" }),
      toolEntry("NotebookEdit", { notebook_path: "nb.ipynb" }, { id: "t4" }),
    ];
    const result = extractFilesFromTimeline(timeline);
    expect(result).toHaveLength(4);
    expect(result[0]).toMatchObject({ path: "src/a.ts", action: "read", toolUseId: "t1" });
    expect(result[1]).toMatchObject({ path: "src/b.ts", action: "write", toolUseId: "t2" });
    expect(result[2]).toMatchObject({ path: "src/c.ts", action: "edit", toolUseId: "t3" });
    expect(result[3]).toMatchObject({ path: "nb.ipynb", action: "edit", toolUseId: "t4" });
  });

  it("extracts snake_case aliases (read_file, write_file, edit_file)", () => {
    const timeline: TimelineEntry[] = [
      toolEntry("read_file", { file_path: "a.ts" }, { id: "t1" }),
      toolEntry("write_file", { file_path: "b.ts" }, { id: "t2" }),
      toolEntry("edit_file", { file_path: "c.ts" }, { id: "t3" }),
    ];
    const result = extractFilesFromTimeline(timeline);
    expect(result).toHaveLength(3);
    expect(result[0]).toMatchObject({ action: "read" });
    expect(result[1]).toMatchObject({ action: "write" });
    expect(result[2]).toMatchObject({ action: "edit" });
  });

  it("top-level tools have toolUseId, subTimeline tools do not", () => {
    const sub = toolEntry("Write", { file_path: "inner.ts" }, { id: "sub1" });
    const parent = toolEntry("Task", { description: "test" }, { id: "top1", sub: [sub] });
    const result = extractFilesFromTimeline([parent]);
    expect(result).toHaveLength(1);
    expect(result[0].path).toBe("inner.ts");
    expect(result[0].toolUseId).toBeUndefined();
  });

  it("ignores non-file tools (Bash, Grep, etc.)", () => {
    const timeline: TimelineEntry[] = [
      toolEntry("Bash", { command: "ls" }),
      toolEntry("Grep", { pattern: "foo" }),
      toolEntry("WebSearch", { query: "test" }),
    ];
    const result = extractFilesFromTimeline(timeline);
    expect(result).toHaveLength(0);
  });
});

// ── extractFilesFromHooks ──

describe("extractFilesFromHooks", () => {
  it("extracts file tools from hooks", () => {
    const hooks: HookEvent[] = [
      hookEvent("Read", { file_path: "a.ts" }),
      hookEvent("Write", { file_path: "b.ts" }),
      hookEvent("Edit", { file_path: "c.ts" }),
      hookEvent("read_file", { path: "d.ts" }),
    ];
    const result = extractFilesFromHooks(hooks);
    expect(result).toHaveLength(4);
    expect(result[0]).toMatchObject({ path: "a.ts", action: "read" });
    expect(result[3]).toMatchObject({ path: "d.ts", action: "read" });
  });

  it("ignores non-file hooks", () => {
    const hooks: HookEvent[] = [hookEvent("Bash", { command: "ls" })];
    expect(extractFilesFromHooks(hooks)).toHaveLength(0);
  });
});

// ── extractFilesFromPersisted ──

describe("extractFilesFromPersisted", () => {
  it("handles { filename } shape", () => {
    const result = extractFilesFromPersisted([{ filename: "a.ts" }]);
    expect(result).toEqual([{ path: "a.ts", action: "persisted" }]);
  });

  it("handles { path } shape", () => {
    const result = extractFilesFromPersisted([{ path: "b.ts" }]);
    expect(result).toEqual([{ path: "b.ts", action: "persisted" }]);
  });

  it("handles { file_path } shape", () => {
    const result = extractFilesFromPersisted([{ file_path: "c.ts" }]);
    expect(result).toEqual([{ path: "c.ts", action: "persisted" }]);
  });

  it("handles plain string", () => {
    const result = extractFilesFromPersisted(["d.ts"]);
    expect(result).toEqual([{ path: "d.ts", action: "persisted" }]);
  });

  it("ignores empty string", () => {
    expect(extractFilesFromPersisted([""])).toHaveLength(0);
  });

  it("ignores whitespace-only string", () => {
    expect(extractFilesFromPersisted(["  "])).toHaveLength(0);
  });

  it("ignores null, numbers, empty objects, unrelated objects", () => {
    expect(
      extractFilesFromPersisted([null, 42, {}, { unrelated: true }] as unknown[]),
    ).toHaveLength(0);
  });
});

// ── mergeFileEntries ──

describe("mergeFileEntries", () => {
  it("write > edit > read > persisted priority", () => {
    const entries: FileEntry[] = [
      { path: "a.ts", action: "read" },
      { path: "a.ts", action: "write" },
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result).toHaveLength(1);
    expect(result[0].action).toBe("write");
  });

  it("same priority keeps latest occurrence", () => {
    const entries: FileEntry[] = [
      { path: "a.ts", action: "read", toolUseId: "old" },
      { path: "a.ts", action: "read", toolUseId: "new" },
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result).toHaveLength(1);
    expect(result[0].toolUseId).toBe("new");
  });

  it("action upgrade clears toolUseId (no inherit from old action)", () => {
    const entries: FileEntry[] = [
      { path: "a.ts", action: "read", toolUseId: "read-id" },
      { path: "a.ts", action: "edit" }, // upgrade, no toolUseId
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result).toHaveLength(1);
    expect(result[0].action).toBe("edit");
    expect(result[0].toolUseId).toBeUndefined();
  });

  it("same action inherits toolUseId", () => {
    const entries: FileEntry[] = [
      { path: "a.ts", action: "edit", toolUseId: "first" },
      { path: "a.ts", action: "edit" }, // no toolUseId → inherit
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result[0].toolUseId).toBe("first");
  });

  it("lower priority does not overwrite higher", () => {
    const entries: FileEntry[] = [
      { path: "a.ts", action: "write", toolUseId: "w1" },
      { path: "a.ts", action: "read", toolUseId: "r1" },
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result[0].action).toBe("write");
    expect(result[0].toolUseId).toBe("w1");
  });

  it("normalizes paths: ./a.ts and a.ts are the same file", () => {
    const entries: FileEntry[] = [
      { path: "./a.ts", action: "read" },
      { path: "a.ts", action: "write" },
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result).toHaveLength(1);
    expect(result[0].action).toBe("write");
  });

  it("normalizes paths: a//b.ts → a/b.ts", () => {
    const entries: FileEntry[] = [
      { path: "a//b.ts", action: "read" },
      { path: "a/b.ts", action: "edit" },
    ];
    const result = mergeFileEntries({ entries, hasTemporalOrder: true });
    expect(result).toHaveLength(1);
    expect(result[0].action).toBe("edit");
  });

  it("hasTemporalOrder: false sources sort to bottom", () => {
    const temporal: FileEntry[] = [{ path: "a.ts", action: "read" }];
    const persisted: FileEntry[] = [{ path: "b.ts", action: "persisted" }];
    const result = mergeFileEntries(
      { entries: temporal, hasTemporalOrder: true },
      { entries: persisted, hasTemporalOrder: false },
    );
    expect(result).toHaveLength(2);
    expect(result[0].path).toBe("a.ts");
    expect(result[1].path).toBe("b.ts");
  });

  it("mixed sources: timeline entries by order, persisted-only at bottom", () => {
    const timeline: FileEntry[] = [
      { path: "first.ts", action: "read" },
      { path: "second.ts", action: "write" },
    ];
    const persisted: FileEntry[] = [
      { path: "only-persisted.ts", action: "persisted" },
      { path: "second.ts", action: "persisted" }, // also in timeline → won't sink
    ];
    const result = mergeFileEntries(
      { entries: timeline, hasTemporalOrder: true },
      { entries: persisted, hasTemporalOrder: false },
    );
    // second.ts (write, seq=1), first.ts (read, seq=0), only-persisted.ts (persisted, seq=-1)
    expect(result).toHaveLength(3);
    expect(result[0].path).toBe("second.ts");
    expect(result[0].action).toBe("write");
    expect(result[1].path).toBe("first.ts");
    expect(result[2].path).toBe("only-persisted.ts");
  });
});

import { describe, it, expect } from "vitest";
import {
  extractOutputText,
  extractImageBlocks,
  getLanguageFromPath,
  isImagePath,
  extractTaskToolMeta,
  isToolTerminal,
  isToolActive,
  shouldShowSubTimeline,
  aggregateBatchStatus,
  detectBatchGroups,
} from "../tool-rendering";

// ── extractOutputText ──

describe("extractOutputText", () => {
  it("returns empty string for null/undefined", () => {
    expect(extractOutputText(null)).toBe("");
    expect(extractOutputText(undefined)).toBe("");
  });

  it("returns string output directly", () => {
    expect(extractOutputText("hello world")).toBe("hello world");
  });

  it("extracts .content from object", () => {
    expect(extractOutputText({ content: "file contents here" })).toBe("file contents here");
  });

  it("falls back to .error from object", () => {
    expect(extractOutputText({ error: "not found" })).toBe("not found");
  });

  it("extracts text from content block array", () => {
    const output = {
      content: [
        { type: "text", text: "line 1" },
        { type: "text", text: "line 2" },
      ],
    };
    expect(extractOutputText(output)).toBe("line 1\nline 2");
  });

  it("falls back to JSON.stringify for unknown objects", () => {
    const output = { foo: 42 };
    expect(extractOutputText(output)).toBe('{"foo":42}');
  });
});

// ── getLanguageFromPath ──

describe("getLanguageFromPath", () => {
  it("maps .ts to typescript", () => {
    expect(getLanguageFromPath("src/lib/utils.ts")).toBe("typescript");
  });

  it("maps .py to python", () => {
    expect(getLanguageFromPath("script.py")).toBe("python");
  });

  it("maps .rs to rust", () => {
    expect(getLanguageFromPath("src-tauri/src/main.rs")).toBe("rust");
  });

  it("returns empty string for unknown extension", () => {
    expect(getLanguageFromPath("Makefile.unknown")).toBe("");
  });

  it("returns empty string for no extension", () => {
    expect(getLanguageFromPath("Makefile")).toBe("");
  });
});

// ── isImagePath ──

describe("isImagePath", () => {
  it("returns true for image extensions", () => {
    expect(isImagePath("photo.png")).toBe(true);
    expect(isImagePath("photo.jpg")).toBe(true);
    expect(isImagePath("icon.gif")).toBe(true);
    expect(isImagePath("logo.webp")).toBe(true);
  });

  it("returns false for non-image extensions", () => {
    expect(isImagePath("main.ts")).toBe(false);
    expect(isImagePath("lib.rs")).toBe(false);
  });

  it("returns false for no extension", () => {
    expect(isImagePath("README")).toBe(false);
  });
});

// ── extractImageBlocks ──

describe("extractImageBlocks", () => {
  it("returns empty for non-object input", () => {
    expect(extractImageBlocks(null)).toEqual([]);
    expect(extractImageBlocks("hello")).toEqual([]);
  });

  it("extracts image blocks from content array", () => {
    const output = {
      content: [
        { type: "text", text: "description" },
        { type: "image", source: { type: "base64", media_type: "image/png", data: "abc123" } },
      ],
    };
    const images = extractImageBlocks(output);
    expect(images).toHaveLength(1);
    expect(images[0].source.data).toBe("abc123");
  });

  it("skips text blocks", () => {
    const output = {
      content: [{ type: "text", text: "no images here" }],
    };
    expect(extractImageBlocks(output)).toEqual([]);
  });
});

// ── extractTaskToolMeta ──

describe("extractTaskToolMeta", () => {
  it("extracts all fields from complete input", () => {
    const input = {
      subagent_type: "Explore",
      description: "Find auth files",
      model: "haiku",
      isolation: "worktree",
      prompt: "Search for authentication code",
    };
    const meta = extractTaskToolMeta(input);
    expect(meta).not.toBeNull();
    expect(meta!.subagentType).toBe("Explore");
    expect(meta!.description).toBe("Find auth files");
    expect(meta!.model).toBe("haiku");
    expect(meta!.isolation).toBe("worktree");
    expect(meta!.prompt).toBe("Search for authentication code");
  });

  it("extracts minimal input with only subagent_type", () => {
    const input = { subagent_type: "general-purpose" };
    const meta = extractTaskToolMeta(input);
    expect(meta).not.toBeNull();
    expect(meta!.subagentType).toBe("general-purpose");
    expect(meta!.description).toBeUndefined();
    expect(meta!.model).toBeUndefined();
    expect(meta!.isolation).toBeUndefined();
    expect(meta!.prompt).toBeUndefined();
  });

  it("returns null for null input", () => {
    expect(extractTaskToolMeta(null)).toBeNull();
  });

  it("returns null for non-object input", () => {
    expect(extractTaskToolMeta("hello")).toBeNull();
    expect(extractTaskToolMeta(42)).toBeNull();
  });

  it("returns null when subagent_type is missing", () => {
    expect(extractTaskToolMeta({ description: "no type" })).toBeNull();
    expect(extractTaskToolMeta({})).toBeNull();
  });

  it("truncates long prompts to 200 chars", () => {
    const longPrompt = "x".repeat(300);
    const meta = extractTaskToolMeta({ subagent_type: "Explore", prompt: longPrompt });
    expect(meta!.prompt).toHaveLength(201); // 200 + "…"
    expect(meta!.prompt!.endsWith("…")).toBe(true);
  });

  it("handles camelCase subagentType field name", () => {
    const input = { subagentType: "Plan", description: "Design plan" };
    const meta = extractTaskToolMeta(input);
    expect(meta).not.toBeNull();
    expect(meta!.subagentType).toBe("Plan");
    expect(meta!.description).toBe("Design plan");
  });
});

// ── isToolTerminal ──

describe("isToolTerminal", () => {
  it.each(["success", "error", "denied", "permission_denied"] as const)(
    "returns true for %s",
    (s) => expect(isToolTerminal(s)).toBe(true),
  );
  it.each(["running", "ask_pending", "permission_prompt"] as const)("returns false for %s", (s) =>
    expect(isToolTerminal(s)).toBe(false),
  );
});

// ── isToolActive ──

describe("isToolActive", () => {
  it.each(["running", "ask_pending", "permission_prompt"] as const)("returns true for %s", (s) =>
    expect(isToolActive(s)).toBe(true),
  );
  it.each(["success", "error", "denied", "permission_denied"] as const)(
    "returns false for %s",
    (s) => expect(isToolActive(s)).toBe(false),
  );
});

// ── shouldShowSubTimeline ──

describe("shouldShowSubTimeline", () => {
  it("running → true", () => expect(shouldShowSubTimeline("running", true)).toBe(true));
  it("ask_pending → true", () => expect(shouldShowSubTimeline("ask_pending", true)).toBe(true));
  it("permission_prompt → true", () =>
    expect(shouldShowSubTimeline("permission_prompt", true)).toBe(true));
  it("success → false", () => expect(shouldShowSubTimeline("success", true)).toBe(false));
  it("error → false", () => expect(shouldShowSubTimeline("error", true)).toBe(false));
  it("denied → false", () => expect(shouldShowSubTimeline("denied", true)).toBe(false));
  it("permission_denied → false", () =>
    expect(shouldShowSubTimeline("permission_denied", true)).toBe(false));
  it("no subTimeline → false", () => expect(shouldShowSubTimeline("running", false)).toBe(false));
});

// ── aggregateBatchStatus ──

describe("aggregateBatchStatus", () => {
  const tool = (status: string) =>
    ({ tool_use_id: "", tool_name: "Task", input: {}, status }) as any;

  it("counts all categories correctly", () => {
    const result = aggregateBatchStatus([
      tool("success"),
      tool("success"),
      tool("error"),
      tool("permission_denied"),
      tool("running"),
      tool("ask_pending"),
      tool("permission_prompt"),
    ]);
    expect(result).toEqual({ completed: 2, failed: 2, running: 3, total: 7 });
  });

  it("empty array", () => {
    expect(aggregateBatchStatus([])).toEqual({ completed: 0, failed: 0, running: 0, total: 0 });
  });

  it("all success", () => {
    const result = aggregateBatchStatus([tool("success"), tool("success"), tool("success")]);
    expect(result).toEqual({ completed: 3, failed: 0, running: 0, total: 3 });
  });
});

// ── detectBatchGroups ──

describe("detectBatchGroups", () => {
  const task = (id: string, status = "running") => ({
    kind: "tool" as const,
    tool: { tool_use_id: id, tool_name: "Task", input: {}, status } as any,
  });
  const other = (id: string) => ({
    kind: "tool" as const,
    tool: { tool_use_id: id, tool_name: "Bash", input: {}, status: "success" } as any,
  });
  const user = () => ({ kind: "user" as const });

  it("detects ≥3 consecutive Task tools", () => {
    const tl = [task("1"), task("2"), task("3")];
    const groups = detectBatchGroups(tl);
    expect(groups.size).toBe(1);
    expect(groups.get(0)!.length).toBe(3);
  });

  it("ignores <3 consecutive Task tools", () => {
    const tl = [task("1"), task("2")];
    expect(detectBatchGroups(tl).size).toBe(0);
  });

  it("non-Task entry breaks the group", () => {
    const tl = [task("1"), task("2"), other("x"), task("3"), task("4"), task("5")];
    const groups = detectBatchGroups(tl);
    expect(groups.size).toBe(1);
    expect(groups.has(0)).toBe(false);
    expect(groups.get(3)!.length).toBe(3);
  });

  it("detects multiple groups", () => {
    const tl = [
      task("1"),
      task("2"),
      task("3"),
      user(),
      task("4"),
      task("5"),
      task("6"),
      task("7"),
    ];
    const groups = detectBatchGroups(tl);
    expect(groups.size).toBe(2);
    expect(groups.get(0)!.length).toBe(3);
    expect(groups.get(4)!.length).toBe(4);
  });

  it("empty timeline", () => {
    expect(detectBatchGroups([]).size).toBe(0);
  });
});

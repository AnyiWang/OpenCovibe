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
  planFileSuffix,
  extractPlanContent,
  applyPlanEditsForward,
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

// ── planFileSuffix ──

describe("planFileSuffix", () => {
  it("extracts suffix from absolute path", () => {
    expect(planFileSuffix("/home/user/.claude/plans/foo.md")).toBe("/.claude/plans/foo.md");
  });

  it("extracts suffix from relative path", () => {
    expect(planFileSuffix(".claude/plans/foo.md")).toBe("/.claude/plans/foo.md");
  });

  it("extracts suffix from Windows path", () => {
    expect(planFileSuffix("C:\\Users\\.claude\\plans\\foo.md")).toBe("/.claude/plans/foo.md");
  });

  it("returns null for non-plan file", () => {
    expect(planFileSuffix("src/lib/foo.ts")).toBeNull();
  });

  it("returns null for empty string", () => {
    expect(planFileSuffix("")).toBeNull();
  });

  it("returns null for .claude/plans/ without .md extension", () => {
    expect(planFileSuffix("/home/.claude/plans/foo.txt")).toBeNull();
  });

  it("handles nested project paths", () => {
    expect(planFileSuffix("/Users/dev/project/.claude/plans/my-plan.md")).toBe(
      "/.claude/plans/my-plan.md",
    );
  });
});

// ── extractPlanContent ──

describe("extractPlanContent", () => {
  const planPath = "/home/user/.claude/plans/my-plan.md";

  const write = (content: string, status = "success") =>
    ({
      kind: "tool",
      tool: {
        tool_name: "Write",
        tool_use_id: `w-${Math.random()}`,
        input: { file_path: planPath, content },
        status,
      },
    }) as any;

  const edit = (old_string: string, new_string: string, status = "success", fp = planPath) =>
    ({
      kind: "tool",
      tool: {
        tool_name: "Edit",
        tool_use_id: `e-${Math.random()}`,
        input: { file_path: fp, old_string, new_string },
        status,
      },
    }) as any;

  const exitPlan = (status = "success", tool_use_result?: Record<string, unknown>) =>
    ({
      kind: "tool",
      tool: {
        tool_name: "ExitPlanMode",
        tool_use_id: `ep-${Math.random()}`,
        input: {},
        status,
        ...(tool_use_result ? { tool_use_result } : {}),
      },
    }) as any;

  const other = () => ({ kind: "user" }) as any;

  it("extracts content from a single successful Write", () => {
    const tl = [write("# My Plan\n\nStep 1"), exitPlan("permission_prompt")];
    const result = extractPlanContent(tl, 1);
    expect(result).toEqual({ content: "# My Plan\n\nStep 1", fileName: "my-plan" });
  });

  it("applies successful Edit after Write", () => {
    const tl = [
      write("# Plan\n\nOld step"),
      edit("Old step", "New step"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan\n\nNew step", fileName: "my-plan" });
  });

  it("ignores failed Edit", () => {
    const tl = [
      write("# Plan\n\nStep"),
      edit("Step", "Changed", "error"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan\n\nStep", fileName: "my-plan" });
  });

  it("uses latest Write when overwritten", () => {
    const tl = [write("First"), write("Second"), exitPlan("permission_prompt")];
    const result = extractPlanContent(tl, 2);
    // extractPlanContent finds the latest Write by scanning backwards, then applies forwards
    // The backwards scan finds Write("Second") at index 1 first
    expect(result).toEqual({ content: "Second", fileName: "my-plan" });
  });

  it("does not apply Edit to different plan file", () => {
    const otherPath = "/home/user/.claude/plans/other-plan.md";
    const tl = [
      write("# Plan"),
      edit("Plan", "Changed", "success", otherPath),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan", fileName: "my-plan" });
  });

  it("skips Edit when old_string not found in content", () => {
    const tl = [
      write("# Plan\n\nStep 1"),
      edit("Nonexistent", "Replacement"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan\n\nStep 1", fileName: "my-plan" });
  });

  it("returns null when no plan Write exists", () => {
    const tl = [other(), exitPlan("permission_prompt")];
    expect(extractPlanContent(tl, 1)).toBeNull();
  });

  it("returns null for empty timeline", () => {
    expect(extractPlanContent([], 0)).toBeNull();
  });

  it("does not cross completed ExitPlanMode boundary", () => {
    const tl = [
      write("Old plan"),
      exitPlan("success"), // completed → boundary
      write("New plan"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 3);
    expect(result).toEqual({ content: "New plan", fileName: "my-plan" });
  });

  it("allows crossing denied/error ExitPlanMode (same-round retry)", () => {
    const tl = [
      write("# Plan\n\nOriginal"),
      exitPlan("denied"), // denied → not a boundary
      edit("Original", "Updated"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 3);
    expect(result).toEqual({ content: "# Plan\n\nUpdated", fileName: "my-plan" });
  });

  it("matches relative Edit path against absolute Write path", () => {
    const relativePath = ".claude/plans/my-plan.md";
    const tl = [
      write("# Plan\n\nStep"),
      edit("Step", "Changed", "success", relativePath),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan\n\nChanged", fileName: "my-plan" });
  });

  it("handles multiple Edits in sequence", () => {
    const tl = [
      write("A B C"),
      edit("A", "X"),
      edit("B", "Y"),
      edit("C", "Z"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 4);
    expect(result).toEqual({ content: "X Y Z", fileName: "my-plan" });
  });

  it("ignores non-tool entries between Write and ExitPlanMode", () => {
    const tl = [write("# Plan"), other(), other(), exitPlan("permission_prompt")];
    const result = extractPlanContent(tl, 3);
    expect(result).toEqual({ content: "# Plan", fileName: "my-plan" });
  });

  it("finds Write inside Agent subTimeline", () => {
    const agent = {
      kind: "tool",
      tool: {
        tool_name: "Agent",
        tool_use_id: "a-1",
        input: { prompt: "write plan" },
        status: "success",
      },
      subTimeline: [write("# SubAgent Plan\n\nDone")],
    } as any;
    const tl = [agent, exitPlan("permission_prompt")];
    const result = extractPlanContent(tl, 1);
    expect(result).toEqual({ content: "# SubAgent Plan\n\nDone", fileName: "my-plan" });
  });

  it("applies Edit inside Agent subTimeline after top-level Write", () => {
    const agent = {
      kind: "tool",
      tool: {
        tool_name: "Agent",
        tool_use_id: "a-2",
        input: { prompt: "update plan" },
        status: "success",
      },
      subTimeline: [edit("Old", "New")],
    } as any;
    const tl = [write("# Plan\n\nOld"), agent, exitPlan("permission_prompt")];
    const result = extractPlanContent(tl, 2);
    expect(result).toEqual({ content: "# Plan\n\nNew", fileName: "my-plan" });
  });

  it("uses plan from completed ExitPlanMode when no Write in current round", () => {
    // Round 1: Write → ExitPlanMode(success) with plan content
    // Round 2: Edit → ExitPlanMode(permission_prompt)
    const tl = [
      write("# Plan\n\nOriginal"),
      exitPlan("success", { plan: "# Plan\n\nOriginal", filePath: planPath }),
      edit("Original", "Updated"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 3);
    expect(result).toEqual({ content: "# Plan\n\nUpdated", fileName: "my-plan" });
  });

  it("uses plan from ExitPlanMode even without filePath", () => {
    const tl = [
      write("# Plan\n\nStep 1"),
      exitPlan("success", { plan: "# Plan\n\nStep 1" }), // no filePath
      edit("Step 1", "Step 2"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 3);
    expect(result).toEqual({ content: "# Plan\n\nStep 2", fileName: "plan" });
  });

  it("returns null when ExitPlanMode boundary has no plan content", () => {
    // ExitPlanMode(success) without tool_use_result.plan → no base content
    const tl = [
      write("# Plan"),
      exitPlan("success"), // no plan in tool_use_result
      edit("Plan", "Changed"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 3);
    expect(result).toBeNull();
  });

  it("applies multiple Edits after ExitPlanMode boundary with plan", () => {
    const tl = [
      write("A B C"),
      exitPlan("success", { plan: "A B C", filePath: planPath }),
      edit("A", "X"),
      edit("B", "Y"),
      edit("C", "Z"),
      exitPlan("permission_prompt"),
    ];
    const result = extractPlanContent(tl, 5);
    expect(result).toEqual({ content: "X Y Z", fileName: "my-plan" });
  });
});

// ── applyPlanEditsForward ──

describe("applyPlanEditsForward", () => {
  const planPath = "/home/user/.claude/plans/my-plan.md";

  const edit = (old_string: string, new_string: string, status = "success", fp = planPath) =>
    ({
      kind: "tool",
      tool: {
        tool_name: "Edit",
        tool_use_id: `e-${Math.random()}`,
        input: { file_path: fp, old_string, new_string },
        status,
      },
    }) as any;

  const write = (content: string, status = "success") =>
    ({
      kind: "tool",
      tool: {
        tool_name: "Write",
        tool_use_id: `w-${Math.random()}`,
        input: { file_path: planPath, content },
        status,
      },
    }) as any;

  const exitPlan = (status = "success", tool_use_result?: Record<string, unknown>) =>
    ({
      kind: "tool",
      tool: {
        tool_name: "ExitPlanMode",
        tool_use_id: `ep-${Math.random()}`,
        input: {},
        status,
        ...(tool_use_result ? { tool_use_result } : {}),
      },
    }) as any;

  const read = () =>
    ({
      kind: "tool",
      tool: {
        tool_name: "Read",
        tool_use_id: `r-${Math.random()}`,
        input: { file_path: planPath },
        status: "success",
      },
    }) as any;

  const other = () => ({ kind: "user" }) as any;

  it("returns base plan when no edits follow", () => {
    const tl = [exitPlan("success", { plan: "# Plan", filePath: planPath }), other()];
    const result = applyPlanEditsForward(tl, 0, "# Plan", planPath);
    expect(result).toBe("# Plan");
  });

  it("applies Edits after the ExitPlanMode index", () => {
    const tl = [
      exitPlan("success", { plan: "# Plan\n\nOriginal", filePath: planPath }),
      read(),
      edit("Original", "Updated"),
    ];
    const result = applyPlanEditsForward(tl, 0, "# Plan\n\nOriginal", planPath);
    expect(result).toBe("# Plan\n\nUpdated");
  });

  it("applies multiple sequential Edits", () => {
    const tl = [
      exitPlan("success", { plan: "A B C", filePath: planPath }),
      edit("A", "X"),
      edit("B", "Y"),
      edit("C", "Z"),
    ];
    const result = applyPlanEditsForward(tl, 0, "A B C", planPath);
    expect(result).toBe("X Y Z");
  });

  it("ignores failed Edits", () => {
    const tl = [
      exitPlan("success", { plan: "# Plan", filePath: planPath }),
      edit("Plan", "Changed", "error"),
    ];
    const result = applyPlanEditsForward(tl, 0, "# Plan", planPath);
    expect(result).toBe("# Plan");
  });

  it("ignores Edits to different plan files", () => {
    const otherPath = "/home/user/.claude/plans/other-plan.md";
    const tl = [
      exitPlan("success", { plan: "# Plan", filePath: planPath }),
      edit("Plan", "Changed", "success", otherPath),
    ];
    const result = applyPlanEditsForward(tl, 0, "# Plan", planPath);
    expect(result).toBe("# Plan");
  });

  it("handles Write overwriting the plan after approval", () => {
    const tl = [exitPlan("success", { plan: "# Old", filePath: planPath }), write("# Rewritten")];
    const result = applyPlanEditsForward(tl, 0, "# Old", planPath);
    expect(result).toBe("# Rewritten");
  });
});

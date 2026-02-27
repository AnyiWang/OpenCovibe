import { describe, it, expect } from "vitest";
import {
  filterSlashCommands,
  getSlashKeyAction,
  getCommandInteraction,
  getArgumentHint,
  shouldBackFromSubView,
  isSubViewInputValid,
  mergeWithVirtual,
  isVirtualCommand,
  parseVirtualAction,
  getQuickActions,
  classifyCloseReason,
  getCommandCategory,
  groupSlashCommands,
  VIRTUAL_COMMANDS,
  QUICK_ACTION_NAMES,
  buildHelpText,
  quoteCliArg,
  normalizeDirPath,
  pathsEqual,
} from "../slash-commands";
import type { CliCommand } from "$lib/types";

const MOCK_COMMANDS: CliCommand[] = [
  { name: "compact", description: "Compact context", aliases: ["c"] },
  { name: "config", description: "Open config", aliases: [] },
  { name: "model", description: "Switch model", aliases: ["m"] },
  { name: "allowed-tools", description: "Manage tools", aliases: [] },
  { name: "help", description: "Show help", aliases: ["h", "?"] },
];

// ── filterSlashCommands ──

describe("filterSlashCommands", () => {
  it("returns all commands for empty query", () => {
    expect(filterSlashCommands(MOCK_COMMANDS, "")).toEqual(MOCK_COMMANDS);
  });

  it('filters by name prefix "co"', () => {
    const result = filterSlashCommands(MOCK_COMMANDS, "co");
    expect(result.map((c) => c.name)).toEqual(["compact", "config"]);
  });

  it("returns empty for no match", () => {
    expect(filterSlashCommands(MOCK_COMMANDS, "xyz")).toEqual([]);
  });

  it("matches alias prefix", () => {
    const result = filterSlashCommands(MOCK_COMMANDS, "c");
    // "c" matches compact (alias "c") and config (name "config")
    expect(result.map((c) => c.name)).toEqual(["compact", "config"]);
  });

  it("matches hyphenated command", () => {
    const result = filterSlashCommands(MOCK_COMMANDS, "allowed-");
    expect(result.map((c) => c.name)).toEqual(["allowed-tools"]);
  });

  it("is case insensitive", () => {
    const result = filterSlashCommands(MOCK_COMMANDS, "CO");
    expect(result.map((c) => c.name)).toEqual(["compact", "config"]);
  });
});

// ── getCommandInteraction ──

describe("getCommandInteraction", () => {
  it('returns "immediate" for command without argumentHint', () => {
    const cmd: CliCommand = { name: "compact", description: "Compact", aliases: [] };
    expect(getCommandInteraction(cmd)).toBe("immediate");
  });

  it('returns "immediate" for command with empty argumentHint', () => {
    const cmd: CliCommand = {
      name: "compact",
      description: "Compact",
      aliases: [],
      argumentHint: "",
    };
    expect(getCommandInteraction(cmd)).toBe("immediate");
  });

  it('returns "immediate" for command with whitespace-only argumentHint', () => {
    const cmd: CliCommand = {
      name: "compact",
      description: "Compact",
      aliases: [],
      argumentHint: "  ",
    };
    expect(getCommandInteraction(cmd)).toBe("immediate");
  });

  it('returns "free-text" for command with non-empty argumentHint', () => {
    const cmd: CliCommand = {
      name: "config",
      description: "Config",
      aliases: [],
      argumentHint: "<key> [value]",
    };
    expect(getCommandInteraction(cmd)).toBe("free-text");
  });

  it('returns "enum" for virtual command with _enum', () => {
    const cmd: CliCommand = {
      name: "model",
      description: "Switch model",
      aliases: ["m"],
      _virtual: true,
      _enum: true,
    };
    expect(getCommandInteraction(cmd)).toBe("enum");
  });

  it('returns "enum" for CLI command merged with virtual _enum', () => {
    const cli: CliCommand[] = [{ name: "model", description: "", aliases: ["m"] }];
    const merged = mergeWithVirtual(cli);
    const modelCmd = merged.find((c) => c.name === "model")!;
    expect(getCommandInteraction(modelCmd)).toBe("enum");
  });
});

// ── getArgumentHint ──

describe("getArgumentHint", () => {
  it("returns argumentHint string when present", () => {
    const cmd: CliCommand = {
      name: "config",
      description: "Config",
      aliases: [],
      argumentHint: "<key> [value]",
    };
    expect(getArgumentHint(cmd)).toBe("<key> [value]");
  });

  it("returns empty string when no hint", () => {
    const cmd: CliCommand = { name: "compact", description: "Compact", aliases: [] };
    expect(getArgumentHint(cmd)).toBe("");
  });

  it("returns empty string when hint is non-string", () => {
    const cmd: CliCommand = { name: "test", description: "Test", aliases: [], argumentHint: 42 };
    expect(getArgumentHint(cmd)).toBe("");
  });
});

// ── shouldBackFromSubView ──

describe("shouldBackFromSubView", () => {
  it("returns true when param empty and cursor at end", () => {
    expect(shouldBackFromSubView("/model ", 7, "model")).toBe(true);
  });

  it("returns true when param empty no trailing space and cursor at end", () => {
    expect(shouldBackFromSubView("/model", 6, "model")).toBe(true);
  });

  it("returns false when param has text", () => {
    expect(shouldBackFromSubView("/model opus", 11, "model")).toBe(false);
  });

  it("returns false when cursor not at end", () => {
    expect(shouldBackFromSubView("/model ", 3, "model")).toBe(false);
  });

  it("returns false when activeCmdName is undefined", () => {
    expect(shouldBackFromSubView("/model ", 7, undefined)).toBe(false);
  });

  it("returns false when input doesn't match active command", () => {
    expect(shouldBackFromSubView("/config ", 8, "model")).toBe(false);
  });
});

// ── isSubViewInputValid ──

describe("isSubViewInputValid", () => {
  it("returns true for /model with trailing space", () => {
    expect(isSubViewInputValid("/model ", "model")).toBe(true);
  });

  it("returns true for /model opus", () => {
    expect(isSubViewInputValid("/model opus", "model")).toBe(true);
  });

  it("returns true for /model with no trailing space", () => {
    expect(isSubViewInputValid("/model", "model")).toBe(true);
  });

  it("returns false for /mod (partial command name)", () => {
    expect(isSubViewInputValid("/mod", "model")).toBe(false);
  });

  it("returns false for plain text", () => {
    expect(isSubViewInputValid("hello", "model")).toBe(false);
  });

  it("returns false for different command", () => {
    expect(isSubViewInputValid("/config foo", "model")).toBe(false);
  });
});

// ── getSlashKeyAction ──

describe("getSlashKeyAction", () => {
  it('returns "next" for ArrowDown', () => {
    expect(getSlashKeyAction("ArrowDown", false)).toEqual({ action: "next" });
  });

  it('returns "prev" for ArrowUp', () => {
    expect(getSlashKeyAction("ArrowUp", false)).toEqual({ action: "prev" });
  });

  it('returns "select" for Enter', () => {
    expect(getSlashKeyAction("Enter", false)).toEqual({ action: "select" });
  });

  it('returns "select" for Tab', () => {
    expect(getSlashKeyAction("Tab", false)).toEqual({ action: "select" });
  });

  it('returns "dismiss" for Escape', () => {
    expect(getSlashKeyAction("Escape", false)).toEqual({ action: "dismiss" });
  });

  it("returns null for Enter during IME composition", () => {
    expect(getSlashKeyAction("Enter", true)).toBeNull();
  });

  it("returns null for any key during IME composition", () => {
    expect(getSlashKeyAction("ArrowDown", true)).toBeNull();
  });

  it("returns null for non-intercepted keys", () => {
    expect(getSlashKeyAction("a", false)).toBeNull();
    expect(getSlashKeyAction("Backspace", false)).toBeNull();
  });
});

// ── mergeWithVirtual ──

describe("mergeWithVirtual", () => {
  it("appends virtual commands to CLI commands", () => {
    const cli: CliCommand[] = [{ name: "compact", description: "Compact", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    expect(merged.length).toBe(1 + VIRTUAL_COMMANDS.length);
    // All virtual commands are appended (order matches VIRTUAL_COMMANDS)
    const appended = merged.slice(1);
    expect(appended.map((c) => c.name)).toEqual(VIRTUAL_COMMANDS.map((v) => v.name));
  });

  it("merges virtual metadata onto CLI command with same name", () => {
    const cli: CliCommand[] = [{ name: "model", description: "CLI model", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    // 1 merged (model) + remaining virtuals not in CLI
    const novelVirtuals = VIRTUAL_COMMANDS.filter((v) => v.name !== "model").length;
    expect(merged.length).toBe(1 + novelVirtuals);
    // CLI description takes priority over empty virtual description
    expect(merged[0].description).toBe("CLI model");
    // Virtual metadata is injected
    expect(merged[0]["_virtual"]).toBe(true);
    expect(merged[0]["_enum"]).toBe(true);
  });

  it("uses fallback description when both CLI and virtual descriptions are empty", () => {
    const cli: CliCommand[] = [{ name: "model", description: "", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    const modelCmd = merged.find((c) => c.name === "model")!;
    // Virtual has empty desc, CLI has empty desc → fallback from KNOWN_COMMAND_DESCRIPTIONS
    expect(modelCmd.description).not.toBe("");
  });

  it("preserves CLI aliases when merging", () => {
    const cli: CliCommand[] = [{ name: "model", description: "CLI model", aliases: ["mod"] }];
    const merged = mergeWithVirtual(cli);
    expect(merged[0].aliases).toEqual(["mod"]);
  });

  it("returns only virtuals when CLI list is empty", () => {
    const merged = mergeWithVirtual([]);
    expect(merged.length).toBe(VIRTUAL_COMMANDS.length);
    expect(merged.every((c) => c["_virtual"] === true)).toBe(true);
  });

  it("applies fallback description for known CLI commands with empty description", () => {
    // CLI sends commands as strings → converted to { name, description: "", aliases: [] }
    const cli: CliCommand[] = [
      { name: "review", description: "", aliases: [] },
      { name: "compact", description: "", aliases: [] },
    ];
    const merged = mergeWithVirtual(cli);
    expect(merged[0].description).toBe("Review a pull request");
    expect(merged[1].description).toBe("Clear conversation history but keep a summary in context");
  });

  it("does not override existing CLI description with fallback", () => {
    const cli: CliCommand[] = [{ name: "review", description: "Custom desc", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    expect(merged[0].description).toBe("Custom desc");
  });

  it("leaves unknown commands without description unchanged", () => {
    const cli: CliCommand[] = [{ name: "my-custom-skill", description: "", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    const cmd = merged.find((c) => c.name === "my-custom-skill");
    expect(cmd?.description).toBe("");
  });
});

// ── isVirtualCommand ──

describe("isVirtualCommand", () => {
  it("returns true for virtual command", () => {
    expect(isVirtualCommand(VIRTUAL_COMMANDS[0])).toBe(true);
  });

  it("returns false for CLI command", () => {
    expect(isVirtualCommand(MOCK_COMMANDS[0])).toBe(false);
  });
});

// ── parseVirtualAction ──

describe("parseVirtualAction", () => {
  it("parses /model opus", () => {
    expect(parseVirtualAction("/model opus")).toEqual({ name: "model", args: "opus" });
  });

  it("parses /model with extra whitespace", () => {
    expect(parseVirtualAction("/model   haiku  ")).toEqual({ name: "model", args: "haiku" });
  });

  it("parses alias /m opus", () => {
    expect(parseVirtualAction("/m opus")).toEqual({ name: "model", args: "opus" });
  });

  it("parses /model without args", () => {
    expect(parseVirtualAction("/model")).toEqual({ name: "model", args: "" });
  });

  it("returns null for non-virtual command", () => {
    expect(parseVirtualAction("/compact")).toBeNull();
  });

  it("returns null for plain text", () => {
    expect(parseVirtualAction("hello world")).toBeNull();
  });
});

// ── getQuickActions ──

describe("getQuickActions", () => {
  it("returns commands in QUICK_ACTION_NAMES order", () => {
    const allCmds: CliCommand[] = [
      { name: "clear", description: "Clear", aliases: [] },
      { name: "compact", description: "Compact", aliases: [] },
      { name: "model", description: "Model", aliases: [] },
      { name: "copy", description: "Copy", aliases: [], _virtual: true, _action: "copy-last" },
      { name: "context", description: "Context", aliases: [] },
      { name: "cost", description: "Cost", aliases: [] },
    ];
    const result = getQuickActions(allCmds);
    expect(result.map((c) => c.name)).toEqual([
      "compact",
      "copy",
      "model",
      "context",
      "cost",
      "clear",
    ]);
  });

  it("skips commands not present in allCommands", () => {
    const allCmds: CliCommand[] = [
      { name: "compact", description: "Compact", aliases: [] },
      { name: "cost", description: "Cost", aliases: [] },
    ];
    const result = getQuickActions(allCmds);
    expect(result.map((c) => c.name)).toEqual(["compact", "cost"]);
  });

  it("returns empty array when allCommands is empty", () => {
    expect(getQuickActions([])).toEqual([]);
  });

  it("includes all QUICK_ACTION_NAMES when all are present", () => {
    const allCmds: CliCommand[] = QUICK_ACTION_NAMES.map((name) => ({
      name,
      description: name,
      aliases: [],
    }));
    const result = getQuickActions(allCmds);
    expect(result.length).toBe(QUICK_ACTION_NAMES.length);
  });
});

// ── /status virtual command ──

describe("/status virtual command", () => {
  it("parseVirtualAction recognizes /status", () => {
    expect(parseVirtualAction("/status")).toEqual({ name: "status", args: "" });
  });

  it("parseVirtualAction recognizes /info alias", () => {
    expect(parseVirtualAction("/info")).toEqual({ name: "status", args: "" });
  });

  it("mergeWithVirtual merges /status with CLI status command", () => {
    const cli: CliCommand[] = [
      { name: "status", description: "Show Claude Code status and version info", aliases: [] },
    ];
    const merged = mergeWithVirtual(cli);
    const statusCmd = merged.find((c) => c.name === "status")!;
    expect(statusCmd["_virtual"]).toBe(true);
    expect(statusCmd["_action"]).toBe("show-status");
    // CLI description takes priority
    expect(statusCmd.description).toBe("Show Claude Code status and version info");
  });
});

// ── classifyCloseReason ──

describe("classifyCloseReason", () => {
  it.each([
    ["escape", "restore"],
    ["click-outside", "restore"],
    ["button-toggle", "restore"],
    ["no-match", "restore"],
    ["disabled", "restore"],
    ["mode-open", "restore"],
    ["at-open", "restore"],
    ["sub-invalid-input", "restore"],
  ] as const)('classifies "%s" as "%s"', (reason, expected) => {
    expect(classifyCloseReason(reason)).toBe(expected);
  });

  it.each([
    ["execute", "clear"],
    ["fill", "clear"],
    ["sub-select", "clear"],
  ] as const)('classifies "%s" as "%s"', (reason, expected) => {
    expect(classifyCloseReason(reason)).toBe(expected);
  });
});

// ── getCommandCategory ──

describe("getCommandCategory", () => {
  it('returns "session" for known session commands', () => {
    expect(getCommandCategory("compact")).toBe("session");
    expect(getCommandCategory("clear")).toBe("session");
    expect(getCommandCategory("fork")).toBe("session");
  });

  it('returns "coding" for known coding commands', () => {
    expect(getCommandCategory("model")).toBe("coding");
    expect(getCommandCategory("review")).toBe("coding");
    expect(getCommandCategory("plan")).toBe("coding");
  });

  it('returns "config" for known config commands', () => {
    expect(getCommandCategory("config")).toBe("config");
    expect(getCommandCategory("mcp")).toBe("config");
    expect(getCommandCategory("vim")).toBe("config");
  });

  it('returns "help" for known help commands', () => {
    expect(getCommandCategory("help")).toBe("help");
    expect(getCommandCategory("doctor")).toBe("help");
    expect(getCommandCategory("feedback")).toBe("help");
  });

  it('returns "skills" when name is in skillNames set', () => {
    const skills = new Set(["find-bugs", "review-pr"]);
    expect(getCommandCategory("find-bugs", skills)).toBe("skills");
  });

  it('returns "other" for unknown command without skillNames', () => {
    expect(getCommandCategory("my-custom-cmd")).toBe("other");
  });

  it("is case insensitive", () => {
    expect(getCommandCategory("Model")).toBe("coding");
    expect(getCommandCategory("COMPACT")).toBe("session");
  });

  it("static map takes precedence over skillNames", () => {
    // "help" is in static map — even if it's also in skillNames, static wins
    const skills = new Set(["help"]);
    expect(getCommandCategory("help", skills)).toBe("help");
  });
});

// ── groupSlashCommands ──

const GROUP_COMMANDS: CliCommand[] = [
  { name: "compact", description: "Compact", aliases: [] },
  { name: "clear", description: "Clear", aliases: [] },
  { name: "model", description: "Model", aliases: [] },
  { name: "review", description: "Review", aliases: [] },
  { name: "config", description: "Config", aliases: [] },
  { name: "help", description: "Help", aliases: [] },
  { name: "doctor", description: "Doctor", aliases: [] },
];

describe("groupSlashCommands", () => {
  it("groups commands by category", () => {
    const result = groupSlashCommands(GROUP_COMMANDS);
    // Should have session, coding, config, help (4 non-empty groups)
    expect(result.groups.length).toBe(4);
    expect(result.groups.map((g) => g.category)).toEqual(["session", "coding", "config", "help"]);
  });

  it("only includes non-empty groups", () => {
    const result = groupSlashCommands(GROUP_COMMANDS);
    // "skills" and "other" have no commands → not in groups
    expect(result.groups.every((g) => g.commands.length > 0)).toBe(true);
    expect(result.groups.find((g) => g.category === "skills")).toBeUndefined();
    expect(result.groups.find((g) => g.category === "other")).toBeUndefined();
  });

  it("flatOrder follows SLASH_CATEGORY_ORDER", () => {
    const result = groupSlashCommands(GROUP_COMMANDS);
    // session commands first, then coding, then config, then help
    const names = result.flatOrder.map((c) => c.name);
    expect(names).toEqual(["compact", "clear", "model", "review", "config", "help", "doctor"]);
  });

  it("startIndex + i aligns with flatOrder position", () => {
    const result = groupSlashCommands(GROUP_COMMANDS);
    for (const group of result.groups) {
      for (let i = 0; i < group.commands.length; i++) {
        expect(result.flatOrder[group.startIndex + i]).toBe(group.commands[i]);
      }
    }
  });

  it("flatOrder.length equals commands.length", () => {
    const result = groupSlashCommands(GROUP_COMMANDS);
    expect(result.flatOrder.length).toBe(GROUP_COMMANDS.length);
  });

  it("returns empty groups and flatOrder for empty input", () => {
    const result = groupSlashCommands([]);
    expect(result.groups).toEqual([]);
    expect(result.flatOrder).toEqual([]);
  });

  it("puts skill commands in other group (skills group hidden)", () => {
    const cmds: CliCommand[] = [
      ...GROUP_COMMANDS,
      { name: "find-bugs", description: "Find bugs", aliases: [] },
    ];
    const skills = new Set(["find-bugs"]);
    const result = groupSlashCommands(cmds, skills);
    const skillsGroup = result.groups.find((g) => g.category === "skills");
    expect(skillsGroup).toBeUndefined();
    const otherGroup = result.groups.find((g) => g.category === "other");
    expect(otherGroup).toBeDefined();
    expect(otherGroup!.commands.map((c) => c.name)).toContain("find-bugs");
  });

  it("puts unknown commands in other group", () => {
    const cmds: CliCommand[] = [
      ...GROUP_COMMANDS,
      { name: "my-unknown-cmd", description: "Unknown", aliases: [] },
    ];
    const result = groupSlashCommands(cmds);
    const otherGroup = result.groups.find((g) => g.category === "other");
    expect(otherGroup).toBeDefined();
    expect(otherGroup!.commands.map((c) => c.name)).toEqual(["my-unknown-cmd"]);
  });

  it("handles case-insensitive skill matching (merged into other)", () => {
    const cmds: CliCommand[] = [{ name: "Find-Bugs", description: "Find bugs", aliases: [] }];
    const skills = new Set(["find-bugs"]);
    const result = groupSlashCommands(cmds, skills);
    const skillsGroup = result.groups.find((g) => g.category === "skills");
    expect(skillsGroup).toBeUndefined();
    const otherGroup = result.groups.find((g) => g.category === "other");
    expect(otherGroup).toBeDefined();
    expect(otherGroup!.commands[0].name).toBe("Find-Bugs");
  });
});

// ── Data flow contract tests ──

describe("data flow contract (PromptInput→SlashMenu)", () => {
  const ALL_CMDS: CliCommand[] = [
    { name: "compact", description: "Compact", aliases: ["c"] },
    { name: "clear", description: "Clear", aliases: [] },
    { name: "model", description: "Model", aliases: ["m"] },
    { name: "config", description: "Config", aliases: [] },
    { name: "help", description: "Help", aliases: ["h"] },
    { name: "find-bugs", description: "Find bugs skill", aliases: [] },
    { name: "unknown-cmd", description: "Unknown", aliases: [] },
  ];

  it("empty query → grouped mode: flatOrder covers all commands, no duplicates", () => {
    const filtered = filterSlashCommands(ALL_CMDS, "");
    expect(filtered.length).toBe(ALL_CMDS.length);

    const skills = new Set(["find-bugs"]);
    const groups = groupSlashCommands(filtered, skills);

    // Every command appears exactly once in flatOrder
    expect(groups.flatOrder.length).toBe(ALL_CMDS.length);
    const nameSet = new Set(groups.flatOrder.map((c) => c.name));
    expect(nameSet.size).toBe(ALL_CMDS.length);

    // startIndex + i covers all positions
    const covered = new Set<number>();
    for (const group of groups.groups) {
      for (let i = 0; i < group.commands.length; i++) {
        covered.add(group.startIndex + i);
      }
    }
    expect(covered.size).toBe(groups.flatOrder.length);
  });

  it("non-empty query → flat mode: direct index mapping", () => {
    const filtered = filterSlashCommands(ALL_CMDS, "co");
    // Should match compact and config
    expect(filtered.length).toBe(2);
    expect(filtered.map((c) => c.name)).toEqual(["compact", "config"]);
    // In flat mode, effectiveCommands = filteredCommands, index i maps directly
    for (let i = 0; i < filtered.length; i++) {
      expect(filtered[i]).toBe(filtered[i]);
    }
  });
});

// ── /help virtual command ──

describe("/help virtual command", () => {
  it("parseVirtualAction recognizes /help", () => {
    expect(parseVirtualAction("/help")).toEqual({ name: "help", args: "" });
  });

  it("parseVirtualAction recognizes /h alias", () => {
    expect(parseVirtualAction("/h")).toEqual({ name: "help", args: "" });
  });

  it("parseVirtualAction recognizes /? alias", () => {
    expect(parseVirtualAction("/?")).toEqual({ name: "help", args: "" });
  });

  it("mergeWithVirtual merges /help with CLI help command", () => {
    const cli: CliCommand[] = [
      { name: "help", description: "Show help and available commands", aliases: [] },
    ];
    const merged = mergeWithVirtual(cli);
    const helpCmd = merged.find((c) => c.name === "help")!;
    expect(helpCmd["_virtual"]).toBe(true);
    expect(helpCmd["_action"]).toBe("show-help");
    // CLI description takes priority
    expect(helpCmd.description).toBe("Show help and available commands");
  });
});

// ── /doctor virtual command ──

describe("/doctor virtual command", () => {
  it("parseVirtualAction recognizes /doctor", () => {
    expect(parseVirtualAction("/doctor")).toEqual({ name: "doctor", args: "" });
  });

  it("mergeWithVirtual merges /doctor with CLI doctor command", () => {
    const cli: CliCommand[] = [
      { name: "doctor", description: "Diagnose and verify your installation", aliases: [] },
    ];
    const merged = mergeWithVirtual(cli);
    const doctorCmd = merged.find((c) => c.name === "doctor")!;
    expect(doctorCmd["_virtual"]).toBe(true);
    expect(doctorCmd["_action"]).toBe("run-doctor");
    // CLI description takes priority
    expect(doctorCmd.description).toBe("Diagnose and verify your installation");
  });

  it("isVirtualCommand returns true for /doctor", () => {
    const doctorVirtual = VIRTUAL_COMMANDS.find((c) => c.name === "doctor")!;
    expect(isVirtualCommand(doctorVirtual)).toBe(true);
  });
});

// ── /tasks virtual command ──

describe("/tasks virtual command", () => {
  it("parseVirtualAction recognizes /tasks", () => {
    expect(parseVirtualAction("/tasks")).toEqual({ name: "tasks", args: "" });
  });

  it("parseVirtualAction recognizes /tasks with id arg", () => {
    expect(parseVirtualAction("/tasks abc-123")).toEqual({ name: "tasks", args: "abc-123" });
  });

  it("VIRTUAL_COMMANDS includes tasks", () => {
    const tasksCmd = VIRTUAL_COMMANDS.find((c) => c.name === "tasks");
    expect(tasksCmd).toBeDefined();
    expect(tasksCmd!["_action"]).toBe("list-tasks");
  });

  it("tasks command is categorized as coding", () => {
    expect(getCommandCategory("tasks")).toBe("coding");
  });

  it("mergeWithVirtual appends /tasks when not in CLI", () => {
    const cli: CliCommand[] = [{ name: "compact", description: "Compact", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    const tasksCmd = merged.find((c) => c.name === "tasks");
    expect(tasksCmd).toBeDefined();
    expect(tasksCmd!["_virtual"]).toBe(true);
    expect(tasksCmd!["_action"]).toBe("list-tasks");
  });
});

// ── buildHelpText ──

describe("buildHelpText", () => {
  const HELP_COMMANDS: CliCommand[] = [
    {
      name: "compact",
      description: "Clear conversation history but keep a summary",
      aliases: ["c"],
    },
    { name: "model", description: "Switch the AI model", aliases: ["m"] },
    { name: "config", description: "Open config panel", aliases: [] },
    { name: "help", description: "Show available commands", aliases: ["h", "?"] },
    { name: "doctor", description: "Diagnose installation", aliases: [] },
  ];

  it("returns a string containing markdown headers for each category", () => {
    const text = buildHelpText(HELP_COMMANDS);
    expect(text).toContain("## Session");
    expect(text).toContain("## Coding");
    expect(text).toContain("## Config");
    expect(text).toContain("## Help");
  });

  it("includes command names with / prefix", () => {
    const text = buildHelpText(HELP_COMMANDS);
    expect(text).toContain("/compact");
    expect(text).toContain("/model");
    expect(text).toContain("/config");
    expect(text).toContain("/help");
  });

  it("includes aliases in italics", () => {
    const text = buildHelpText(HELP_COMMANDS);
    expect(text).toContain("*(c)*");
    expect(text).toContain("*(m)*");
  });

  it("does not add alias notation for commands without aliases", () => {
    const text = buildHelpText(HELP_COMMANDS);
    // /config has no aliases — just /config followed by |
    expect(text).toMatch(/\/config \|/);
  });

  it("includes descriptions", () => {
    const text = buildHelpText(HELP_COMMANDS);
    expect(text).toContain("Clear conversation history but keep a summary");
    expect(text).toContain("Switch the AI model");
  });

  it("includes footer hint about slash menu", () => {
    const text = buildHelpText(HELP_COMMANDS);
    expect(text).toContain("Type `/` to open the command menu with fuzzy search.");
  });

  it("omits empty categories", () => {
    const text = buildHelpText(HELP_COMMANDS);
    // No skills or other commands → no Skills/Other headers
    expect(text).not.toContain("## Skills");
    expect(text).not.toContain("## Other");
  });

  it("puts skill commands in Other section (Skills section hidden)", () => {
    const cmds: CliCommand[] = [
      ...HELP_COMMANDS,
      { name: "find-bugs", description: "Find bugs in code", aliases: [] },
    ];
    const skills = new Set(["find-bugs"]);
    const text = buildHelpText(cmds, skills);
    expect(text).not.toContain("## Skills");
    expect(text).toContain("## Other");
    expect(text).toContain("/find-bugs");
  });

  it("returns valid markdown table format", () => {
    const text = buildHelpText(HELP_COMMANDS);
    // Each section should have table header row
    const tableHeaders = text.match(/\| Command \| Description \|/g);
    expect(tableHeaders).not.toBeNull();
    expect(tableHeaders!.length).toBeGreaterThan(0);
    // Each section should have separator row right after header
    const separators = text.match(/\|[-]+\|[-]+\|/g);
    expect(separators).not.toBeNull();
    expect(separators!.length).toBe(tableHeaders!.length);
  });

  it("handles empty command list", () => {
    const text = buildHelpText([]);
    // No categories → just the footer
    expect(text).toContain("Type `/` to open the command menu with fuzzy search.");
    expect(text).not.toContain("## Session");
  });

  it("uses — for commands with empty description", () => {
    const cmds: CliCommand[] = [{ name: "compact", description: "", aliases: [] }];
    const text = buildHelpText(cmds);
    expect(text).toContain("— |");
  });
});

// ── /add-dir virtual command ──

describe("/add-dir virtual command", () => {
  it("appears in mergeWithVirtual when CLI does not provide it", () => {
    const cli: CliCommand[] = [{ name: "compact", description: "Compact", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    const addDirCmd = merged.find((c) => c.name === "add-dir");
    expect(addDirCmd).toBeDefined();
    expect(addDirCmd!["_virtual"]).toBe(true);
    expect(addDirCmd!["_action"]).toBe("add-dir");
  });

  it("preserves _action when CLI also provides add-dir", () => {
    const cli: CliCommand[] = [{ name: "add-dir", description: "CLI add-dir", aliases: [] }];
    const merged = mergeWithVirtual(cli);
    const addDirCmd = merged.find((c) => c.name === "add-dir")!;
    expect(addDirCmd["_action"]).toBe("add-dir");
    expect(addDirCmd["_virtual"]).toBe(true);
  });

  it('getCommandInteraction returns "immediate" for add-dir', () => {
    const cmd = VIRTUAL_COMMANDS.find((c) => c.name === "add-dir")!;
    expect(getCommandInteraction(cmd)).toBe("immediate");
  });

  it("parseVirtualAction recognizes /add-dir with path arg", () => {
    expect(parseVirtualAction("/add-dir /some/path")).toEqual({
      name: "add-dir",
      args: "/some/path",
    });
  });

  it("parseVirtualAction recognizes /add-dir without args", () => {
    expect(parseVirtualAction("/add-dir")).toEqual({ name: "add-dir", args: "" });
  });

  it('getCommandCategory returns "config" for add-dir', () => {
    expect(getCommandCategory("add-dir")).toBe("config");
  });
});

// ── quoteCliArg ──

describe("quoteCliArg", () => {
  it("quotes a normal path", () => {
    expect(quoteCliArg("/path/to/dir")).toBe('"/path/to/dir"');
  });

  it("quotes a path with spaces", () => {
    expect(quoteCliArg("/path/to/my dir")).toBe('"/path/to/my dir"');
  });

  it("escapes double quotes", () => {
    expect(quoteCliArg('/path/to/"dir"')).toBe(
      '/path/to/\\"dir\\"'.replace(/^/, '"').replace(/$/, '"'),
    );
    // More explicit: input has quotes, output wraps in quotes and escapes inner quotes
    expect(quoteCliArg('a"b')).toBe('"a\\"b"');
  });

  it("escapes backslashes", () => {
    expect(quoteCliArg("C:\\Users\\foo")).toBe('"C:\\\\Users\\\\foo"');
  });

  it("returns null for path containing newline", () => {
    expect(quoteCliArg("/path/to\n/dir")).toBeNull();
  });

  it("returns null for path containing carriage return", () => {
    expect(quoteCliArg("/path/to\r/dir")).toBeNull();
  });

  it("handles Windows path with spaces", () => {
    expect(quoteCliArg("C:\\Users\\My Dir")).toBe('"C:\\\\Users\\\\My Dir"');
  });
});

// ── normalizeDirPath ──

describe("normalizeDirPath", () => {
  it("removes trailing slash", () => {
    expect(normalizeDirPath("/path/to/dir/")).toBe("/path/to/dir");
  });

  it("removes trailing backslash", () => {
    expect(normalizeDirPath("C:\\Users\\foo\\")).toBe("C:\\Users\\foo");
  });

  it("removes trailing forward slash on Windows path", () => {
    expect(normalizeDirPath("C:/Users/foo/")).toBe("C:/Users/foo");
  });

  it("preserves Unix root /", () => {
    expect(normalizeDirPath("/")).toBe("/");
  });

  it("preserves Windows root C:\\", () => {
    expect(normalizeDirPath("C:\\")).toBe("C:\\");
  });

  it("preserves Windows root C:/", () => {
    expect(normalizeDirPath("C:/")).toBe("C:/");
  });

  it("does not trim whitespace", () => {
    expect(normalizeDirPath(" /path/to/dir ")).toBe(" /path/to/dir ");
  });

  it("returns path unchanged when no trailing separator", () => {
    expect(normalizeDirPath("/path/to/dir")).toBe("/path/to/dir");
  });
});

// ── pathsEqual ──

describe("pathsEqual", () => {
  it("Unix paths are case sensitive", () => {
    expect(pathsEqual("/Foo", "/foo")).toBe(false);
  });

  it("Windows paths are case insensitive", () => {
    expect(pathsEqual("C:\\Foo", "c:\\foo")).toBe(true);
  });

  it("mixed: any side with drive prefix triggers case-insensitive", () => {
    expect(pathsEqual("C:\\Foo", "c:\\Foo")).toBe(true);
  });

  it("identical Unix paths are equal", () => {
    expect(pathsEqual("/path/to/dir", "/path/to/dir")).toBe(true);
  });

  it("different Unix paths are not equal", () => {
    expect(pathsEqual("/path/to/dir", "/path/to/other")).toBe(false);
  });
});

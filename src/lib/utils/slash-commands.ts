import type { CliCommand } from "$lib/types";
import { dbg } from "$lib/utils/debug";

// ── Fallback descriptions for known CLI commands ──
// CLI system/init only sends command names (strings), not descriptions.
// These are extracted from the CLI source (cli.js) to fill the gap.
const KNOWN_COMMAND_DESCRIPTIONS: Record<string, string> = {
  agents: "Manage agent configurations",
  clear: "Clear conversation history and free up context",
  color: "Set the prompt bar color for this session",
  compact: "Clear conversation history but keep a summary in context",
  config: "Open config panel",
  context: "Visualize current context usage",
  copy: "Copy Claude's last response to clipboard as markdown",
  cost: "Show the total cost and duration of the current session",
  diff: "View uncommitted changes (git diff HEAD)",
  doctor: "Diagnose and verify your installation and settings",
  feedback: "Submit feedback about Claude Code",
  files: "List all files currently in context",
  fork: "Create a fork of the current conversation at this point",
  help: "Show help and available commands",
  hooks: "Manage hook configurations for tool events",
  ide: "Manage IDE integrations and show status",
  init: "Initialize a new CLAUDE.md file with codebase documentation",
  insights: "View AI insights",
  keybindings: "Open or create your keybindings configuration file",
  login: "Sign in to your Anthropic account",
  logout: "Sign out from your Anthropic account",
  mcp: "Manage MCP servers",
  memory: "Edit Claude memory files",
  model: "Switch the AI model for this session",
  plan: "Enable plan mode or view the current session plan",
  "pr-comments": "View pull request comments",
  "release-notes": "View release notes",
  rename: "Rename the current conversation",
  resume: "Resume a previous conversation",
  review: "Review a pull request",
  "security-review": "Review code for security issues",
  skills: "List available skills",
  status: "Show Claude Code status and version info",
  theme: "Change the theme",
  tasks: "List background tasks in this session",
  todos: "List current todo items",
  usage: "Show plan usage limits",
  vim: "Toggle between Vim and Normal editing modes",
  "add-dir": "Add a directory to the workspace",
};

// ── Virtual commands (not returned by CLI initialize) ──

/** App-handled commands injected into the slash menu. Marked with `_virtual: true`. */
export const VIRTUAL_COMMANDS: CliCommand[] = [
  {
    name: "model",
    description: "", // Use CLI's description; virtual only for _enum UI
    aliases: ["m"],
    _virtual: true,
    _enum: true,
    argumentHint: "",
  },
  {
    name: "config",
    description: "Open CLI config settings",
    aliases: [],
    _virtual: true,
    _navigate: "/settings?tab=cli-config",
  },
  {
    name: "stats",
    description: "View usage stats, heatmap, and model breakdown",
    aliases: ["usage"],
    _virtual: true,
    _navigate: "/usage",
  },
  {
    name: "copy",
    description: "Copy Claude's last response to clipboard as markdown",
    aliases: [],
    _virtual: true,
    _action: "copy-last",
  },
  {
    name: "plan",
    description: "Toggle plan mode (read-only exploration, then user approval)",
    aliases: [],
    _virtual: true,
    _action: "toggle-plan",
    argumentHint: "[instructions]",
  },
  {
    name: "rename",
    description: "Rename the current session",
    aliases: [],
    _virtual: true,
    _action: "rename-session",
    argumentHint: "[name]",
  },
  {
    name: "status",
    description: "Show session status overview",
    aliases: ["info"],
    _virtual: true,
    _action: "show-status",
  },
  {
    name: "help",
    description: "Show available commands",
    aliases: ["h", "?"],
    _virtual: true,
    _action: "show-help",
  },
  {
    name: "doctor",
    description: "Diagnose installation, auth, and connectivity",
    aliases: [],
    _virtual: true,
    _action: "run-doctor",
  },
  {
    name: "diff",
    description: "View uncommitted changes (git diff)",
    aliases: [],
    _virtual: true,
    _action: "show-diff",
  },
  {
    name: "todos",
    description: "List current todo items",
    aliases: ["todo"],
    _virtual: true,
    _action: "list-todos",
  },
  {
    name: "tasks",
    description: "List background tasks in this session",
    aliases: [],
    _virtual: true,
    _action: "list-tasks",
    argumentHint: "[task_id]",
  },
  {
    name: "add-dir",
    description: "Add a directory to the workspace",
    aliases: [],
    _virtual: true,
    _action: "add-dir",
  },
];

/**
 * Merge CLI commands with virtual commands and apply fallback descriptions.
 * When a CLI command shares a name with a virtual, merge virtual metadata onto it
 * (CLI fields take priority for name/desc/aliases). Append remaining virtuals.
 * Commands with empty descriptions get a fallback from KNOWN_COMMAND_DESCRIPTIONS.
 */
export function mergeWithVirtual(cliCommands: CliCommand[]): CliCommand[] {
  const cliMap = new Map(cliCommands.map((c) => [c.name, c]));
  const result = cliCommands.map((c) => {
    const virtual = VIRTUAL_COMMANDS.find((v) => v.name === c.name);
    const merged = virtual
      ? { ...virtual, ...c, _virtual: true, _enum: virtual["_enum"] ?? false }
      : c;
    // Apply fallback description if empty (works for both virtual-merged and plain CLI commands)
    if (!merged.description) {
      const fallback = KNOWN_COMMAND_DESCRIPTIONS[merged.name];
      if (fallback) return { ...merged, description: fallback };
    }
    return merged;
  });
  // Append virtuals not present in CLI
  for (const v of VIRTUAL_COMMANDS) {
    if (!cliMap.has(v.name)) result.push(v);
  }
  return result;
}

export function isVirtualCommand(cmd: CliCommand): boolean {
  return cmd["_virtual"] === true;
}

/**
 * Parse a virtual command invocation from send text.
 * Returns `{ name, args }` if the text matches a virtual command, else null.
 */
export function parseVirtualAction(text: string): { name: string; args: string } | null {
  const match = text.match(/^\/(\S+)(?:\s+(.*))?$/);
  if (!match) return null;
  const name = match[1];
  const virtual = VIRTUAL_COMMANDS.find((v) => v.name === name || (v.aliases ?? []).includes(name));
  if (!virtual) return null;
  return { name: virtual.name, args: (match[2] ?? "").trim() };
}

/** Filter CLI commands by name and aliases prefix match. */
export function filterSlashCommands(commands: CliCommand[], query: string): CliCommand[] {
  if (!query) return commands;
  const q = query.toLowerCase();
  return commands.filter(
    (cmd) =>
      cmd.name.toLowerCase().startsWith(q) ||
      (cmd.aliases ?? []).some((a) => a.toLowerCase().startsWith(q)),
  );
}

// ── Command classification (replaces KNOWN_PARAM_COMMANDS + cmdHasParams) ──

export type CommandInteraction = "immediate" | "free-text" | "enum";

/** Classify how a command should be interacted with in the slash menu. */
export function getCommandInteraction(cmd: CliCommand): CommandInteraction {
  if (cmd["_enum"] === true) return "enum";
  // Virtual action commands execute immediately (args are optional)
  if (cmd["_action"]) return "immediate";
  const hint = cmd["argumentHint"];
  if (typeof hint === "string" && hint.trim().length > 0) return "free-text";
  return "immediate";
}

/** Extract the argumentHint string from a command, or empty string if missing. */
export function getArgumentHint(cmd: CliCommand): string {
  const hint = cmd["argumentHint"];
  return typeof hint === "string" ? hint : "";
}

/**
 * Determine which keydown action to take when the slash menu is open.
 * Returns null if the key should not be intercepted.
 */
export type SlashKeyAction =
  | { action: "next" }
  | { action: "prev" }
  | { action: "select" }
  | { action: "dismiss" }
  | null;

export function getSlashKeyAction(key: string, isComposing: boolean): SlashKeyAction {
  if (isComposing) return null;
  switch (key) {
    case "ArrowDown":
      return { action: "next" };
    case "ArrowUp":
      return { action: "prev" };
    case "Enter":
    case "Tab":
      return { action: "select" };
    case "Escape":
      return { action: "dismiss" };
    default:
      return null;
  }
}

/** Whether Backspace should navigate back from sub-view to commands. */
export function shouldBackFromSubView(
  inputText: string,
  cursorPos: number,
  activeCmdName: string | undefined,
): boolean {
  if (!activeCmdName) return false;
  const pattern = new RegExp(`^\\/${activeCmdName}\\s*$`);
  return pattern.test(inputText) && cursorPos === inputText.length;
}

/** Whether sub-view input is still valid for the active command. */
export function isSubViewInputValid(inputText: string, activeCmdName: string): boolean {
  const pattern = new RegExp(`^\\/${activeCmdName}(?:\\s.*)?$`);
  return pattern.test(inputText);
}

// ── Quick action pills (L3) ──

/** Ordered list of command names shown as quick-action pills above the action bar. */
export const QUICK_ACTION_NAMES: readonly string[] = [
  "compact",
  "copy",
  "model",
  "context",
  "cost",
  "clear",
] as const;

/** Return the subset of allCommands that appear in QUICK_ACTION_NAMES, preserving pill order. */
export function getQuickActions(allCommands: CliCommand[]): CliCommand[] {
  const map = new Map(allCommands.map((c) => [c.name, c]));
  return QUICK_ACTION_NAMES.filter((n) => map.has(n)).map((n) => map.get(n)!);
}

// ── Slash command categories (grouped menu) ──

export type SlashCategory = "session" | "coding" | "config" | "help" | "skills" | "other";

export const SLASH_CATEGORY_ORDER: readonly SlashCategory[] = [
  "session",
  "coding",
  "config",
  "help",
  "skills",
  "other",
];

const COMMAND_CATEGORY_MAP: Record<string, SlashCategory> = {
  // Session
  compact: "session",
  clear: "session",
  status: "session",
  rename: "session",
  context: "session",
  cost: "session",
  resume: "session",
  fork: "session",
  copy: "session",
  files: "session",
  // Coding
  model: "coding",
  diff: "coding",
  review: "coding",
  "security-review": "coding",
  plan: "coding",
  init: "coding",
  "pr-comments": "coding",
  edit: "coding",
  run: "coding",
  terminal: "coding",
  todos: "coding",
  tasks: "coding",
  // Config
  config: "config",
  "allowed-tools": "config",
  permissions: "config",
  mcp: "config",
  memory: "config",
  agents: "config",
  vim: "config",
  theme: "config",
  color: "config",
  keybindings: "config",
  hooks: "config",
  ide: "config",
  "add-dir": "config",
  // Help
  help: "help",
  doctor: "help",
  insights: "help",
  stats: "help",
  usage: "help",
  skills: "help",
  bug: "help",
  login: "help",
  logout: "help",
  feedback: "help",
  "release-notes": "help",
};

export interface SlashCommandGroup {
  category: SlashCategory;
  commands: CliCommand[];
  /** Index of this group's first command in the flatOrder array */
  startIndex: number;
}

export interface SlashCommandGroups {
  groups: SlashCommandGroup[];
  flatOrder: CliCommand[];
}

/** Determine the category for a single command. */
export function getCommandCategory(name: string, skillNames?: Set<string>): SlashCategory {
  const lower = name.toLowerCase();
  const mapped = COMMAND_CATEGORY_MAP[lower];
  if (mapped) return mapped;
  if (skillNames && skillNames.has(lower)) return "skills";
  return "other";
}

/** Group commands by category for the slash menu. */
export function groupSlashCommands(
  commands: CliCommand[],
  skillNames?: Set<string>,
): SlashCommandGroups {
  // Normalize skill names once
  const normalizedSkills = skillNames
    ? new Set([...skillNames].map((s) => s.toLowerCase()))
    : undefined;

  // Bucket commands by category
  const buckets = new Map<SlashCategory, CliCommand[]>();
  for (const cat of SLASH_CATEGORY_ORDER) {
    buckets.set(cat, []);
  }
  for (const cmd of commands) {
    const cat = getCommandCategory(cmd.name, normalizedSkills);
    buckets.get(cat)!.push(cmd);
  }

  // Build groups (skip empty) and flat order
  const groups: SlashCommandGroup[] = [];
  const flatOrder: CliCommand[] = [];

  for (const cat of SLASH_CATEGORY_ORDER) {
    if (cat === "skills") continue; // Skills accessed via SkillSelector
    const cmds = buckets.get(cat)!;
    // Merge skills into "other" bucket
    if (cat === "other") {
      cmds.push(...(buckets.get("skills") ?? []));
    }
    if (cmds.length === 0) continue;
    groups.push({ category: cat, commands: cmds, startIndex: flatOrder.length });
    flatOrder.push(...cmds);
  }

  dbg("slash", "grouped", {
    categories: groups.length,
    flat: flatOrder.length,
    skills: normalizedSkills?.size ?? 0,
  });

  return { groups, flatOrder };
}

// ── Help text builder ──

const CATEGORY_LABELS: Record<SlashCategory, string> = {
  session: "Session",
  coding: "Coding",
  config: "Config",
  help: "Help",
  skills: "Skills",
  other: "Other",
};

/**
 * Build Markdown help text listing all commands grouped by category.
 * Used by the /help virtual command to render in-chat output.
 */
export function buildHelpText(commands: CliCommand[], skillNames?: Set<string>): string {
  const { groups } = groupSlashCommands(commands, skillNames);
  const sections: string[] = [];

  for (const group of groups) {
    if (group.category === "skills") continue; // Skills accessed via SkillSelector
    const label = CATEGORY_LABELS[group.category];
    const lines: string[] = [
      `## ${label}`,
      "",
      "| Command | Description |",
      "|---------|-------------|",
    ];
    for (const cmd of group.commands) {
      const aliases = (cmd.aliases ?? []).length > 0 ? ` *(${cmd.aliases!.join(", ")})* ` : " ";
      lines.push(`| /${cmd.name}${aliases}| ${cmd.description || "—"} |`);
    }
    sections.push(lines.join("\n"));
  }

  sections.push("*Type `/` to open the command menu with fuzzy search.*");
  return sections.join("\n\n");
}

// ── Close reason classification (for savedInputForSlash lifecycle) ──

/**
 * Classify a closeSlashMenu reason into whether the saved input should be
 * restored ("restore") or discarded ("clear").
 *
 * - "restore": user dismissed without executing → restore their draft
 * - "clear": user executed a command → draft was consumed or replaced
 */
export function classifyCloseReason(reason: string): "restore" | "clear" {
  switch (reason) {
    case "execute":
    case "fill":
    case "sub-select":
      return "clear";
    default:
      return "restore";
  }
}

// ── Path utilities for /add-dir ──

/**
 * Quote a filesystem path for safe inclusion in a CLI slash command.
 * Escapes backslashes and double-quotes, wraps in double quotes.
 * Returns null if path contains newline characters (injection risk).
 */
export function quoteCliArg(arg: string): string | null {
  if (/[\r\n]/.test(arg)) return null;
  return `"${arg.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * Normalize a directory path for dedup comparison:
 * remove trailing slash/backslash (unless root like "/" or "C:\" or "C:/").
 * Does NOT trim whitespace — directory names with leading/trailing spaces are valid.
 */
export function normalizeDirPath(p: string): string {
  if (p.length > 1 && (p.endsWith("/") || p.endsWith("\\"))) {
    const isUnixRoot = p === "/";
    const isWinRoot = /^[A-Za-z]:[/\\]$/.test(p);
    if (!isUnixRoot && !isWinRoot) {
      return p.slice(0, -1);
    }
  }
  return p;
}

/** Whether a path looks like a Windows path (drive letter prefix). */
function isWindowsPath(p: string): boolean {
  return /^[A-Za-z]:/.test(p);
}

/**
 * Compare two normalized paths for dedup.
 * Case-insensitive on Windows-style paths (drive letter prefix).
 */
export function pathsEqual(a: string, b: string): boolean {
  if (isWindowsPath(a) || isWindowsPath(b)) {
    return a.toLowerCase() === b.toLowerCase();
  }
  return a === b;
}

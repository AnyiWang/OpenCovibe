import type { CliCommand } from "$lib/types";
import { dbg } from "$lib/utils/debug";

// ── Fallback descriptions for known CLI commands ──
// CLI system/init only sends command names (strings), not descriptions.
// These are extracted from the CLI source (cli.js) to fill the gap.
const KNOWN_COMMAND_DESCRIPTIONS: Record<string, string> = {
  agents: "Manage agent configurations",
  clear: "Clear conversation history and free up context",
  "code-review": "Review code quality (optional effort level: low|medium|high)",
  color: "Set the prompt bar color for this session",
  compact: "Clear conversation history but keep a summary in context",
  config: "Open config panel",
  context: "Visualize current context usage",
  copy: "Copy Claude's last response to clipboard as markdown",
  cost: "Show the total cost and duration of the current session",
  diff: "View uncommitted changes (git diff HEAD)",
  fast: "Toggle fast mode on or off",
  doctor: "Diagnose and verify your installation and settings",
  feedback: "Submit feedback about Claude Code",
  files: "List all files currently in context",
  fork: "Create a fork of the current conversation at this point",
  help: "Show help and available commands",
  hooks: "Manage hook configurations for tool events",
  plugin: "Manage plugins, skills, MCP servers, and hooks",
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
  "reload-skills": "Re-scan skill directories without restarting the session",
  rename: "Rename the current conversation",
  resume: "Resume a previous conversation",
  review: "Review a pull request",
  "security-review": "Review code for security issues",
  simplify: "Review code for cleanup (reuse, simplification, efficiency) and apply fixes",
  skills: "List available skills",
  status: "Show Claude Code status and version info",
  theme: "Change the theme",
  tasks: "List background tasks in this session",
  todos: "List current todo items",
  usage: "Show plan usage limits",
  "usage-credits": "Show usage credits balance and history",
  vim: "Toggle between Vim and Normal editing modes",
  "add-dir": "Add a directory to the workspace",
  btw: "Ask a side question without interrupting the current task",
  loop: "Run a prompt or slash command on a recurring interval",
  "team-onboarding": "Help teammates ramp on Claude Code with a guide from your usage",
};

// ── Fallback argumentHints for known CLI commands ──
// Some CLI commands need argumentHint to prevent immediate execution.
// Unlike VIRTUAL_COMMANDS, these only apply when CLI actually returns the command.
const KNOWN_ARGUMENT_HINTS: Record<string, string> = {
  loop: "[interval] <prompt>",
};

/** Marker for context-cleared separators. Used by reducer, renderer, and dimming logic. */
export const CONTEXT_CLEARED_MARKER = "__context_cleared__";

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
    // No hot-switch for Codex — model change takes effect on next turn
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
    // Codex: mode takes effect next turn (--sandbox read-only / default / bypass)
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
    // Codex: saves to settings, takes effect next turn (--add-dir flag)
  },
  {
    name: "fast",
    description: "Toggle fast mode on or off",
    aliases: [],
    _virtual: true,
    _enum: true,
    _action: "toggle-fast",
    _excludeAgents: ["codex"], // Claude-specific
  },
  {
    name: "rewind",
    description: "Rewind files to a previous checkpoint",
    aliases: ["undo"],
    _virtual: true,
    _action: "rewind",
    _excludeAgents: ["codex"], // needs snapshots
  },
  {
    name: "rewind",
    // Codex variant: turn-based history rollback (drops N turns), NOT a file
    // snapshot rewind. Distinct entry so the Claude variant's snapshot copy/
    // exclusion stays intact. Resolved per-agent by parseVirtualAction.
    description: "Rewind conversation to an earlier turn (history only)",
    aliases: ["undo"],
    _virtual: true,
    _action: "codex-rewind",
    _excludeAgents: ["claude"],
  },
  {
    name: "compact",
    // Codex `thread/compact/start`: summarize + clear history, keep going.
    // Claude CLI returns its own /compact (passthrough), so Codex-only here.
    description: "Clear conversation history but keep a summary in context",
    aliases: [],
    _virtual: true,
    _action: "codex-compact",
    _excludeAgents: ["claude"],
  },
  {
    name: "goal",
    // Codex `thread/goal/*`: set an objective + token budget and watch live
    // progress. No Claude equivalent.
    description: "Set or view the session objective and token budget",
    aliases: [],
    _virtual: true,
    _action: "codex-goal",
    _excludeAgents: ["claude"],
  },
  {
    name: "clear",
    // Codex parity: `/new`, `/exit`, `/quit` are aliases — all fall through to the
    // same clear-context action. In a GUI app "exit"/"quit" mean leave the current
    // chat (not quit the app), which matches clear semantics. Gated to Codex so we
    // don't silently intercept these names on Claude, whose CLI owns /exit and /quit
    // (and has no /new) — there they fall through to CLI passthrough.
    // Listed before the agent-neutral entry so Codex's merged menu surfaces the aliases.
    description: "Clear conversation history and free up context",
    aliases: ["new", "exit", "quit"],
    _virtual: true,
    _action: "clear-context",
    _excludeAgents: ["claude"],
  },
  {
    name: "clear",
    description: "Clear conversation history and free up context",
    aliases: [],
    _virtual: true,
    _action: "clear-context",
    // Codex: navigates to fresh chat (new thread on next message)
  },
  {
    name: "permissions",
    description: "Manage tool permission rules (allow/deny)",
    aliases: [],
    _virtual: true,
    _action: "open-permissions",
    _excludeAgents: ["codex"], // Claude permission mode
  },
  {
    name: "plugin",
    description: "Manage plugins, skills, MCP servers, and hooks",
    aliases: ["plugins"],
    _virtual: true,
    _navigate: "/plugins",
  },
  {
    name: "mcp",
    description: "Manage MCP servers",
    aliases: [],
    _virtual: true,
    _navigate: "/plugins?section=mcp&source=configured",
    // Intentional behaviour change: even when Claude CLI passes through /mcp,
    // OpenCovibe intercepts and opens the in-app Extend page. Same pattern as
    // /memory — the app owns this UI rather than delegating to CLI TUI.
  },
  {
    name: "agents",
    description: "Manage agent configurations",
    // Singular `/agent` removed (was wave 2 alias). Codex TUI's `/agent`
    // opens a sub-agent picker — a completely different semantic — so
    // routing /agent to the Extend page was misleading. Codex /agent now
    // has its own informative virtual; /agents (plural) stays here.
    aliases: [],
    _virtual: true,
    _navigate: "/plugins?section=agents",
  },
  {
    name: "hooks",
    description: "Manage hook configurations",
    aliases: [],
    _virtual: true,
    _navigate: "/plugins?section=hooks",
  },
  {
    name: "skills",
    description: "Browse and manage skills",
    aliases: [],
    _virtual: true,
    _navigate: "/plugins?section=skills&source=installed",
  },
  {
    name: "resume",
    // Honest framing: navigates to /history rather than auto-resuming the
    // most recent session (Codex TUI's /resume picks last). Users choose.
    description: "Open conversation history to resume a previous chat",
    aliases: [],
    _virtual: true,
    _navigate: "/history",
  },
  {
    name: "theme",
    description: "Change the theme",
    aliases: [],
    _virtual: true,
    // Theme controls live within General tab (SettingsTab union has no
    // separate `appearance`). Verified at settings/+page.svelte:45,734.
    _navigate: "/settings?tab=general",
  },
  {
    name: "feedback",
    description: "Open the OpenCovibe issue tracker",
    aliases: [],
    _virtual: true,
    _action: "open-feedback",
  },
  {
    name: "agent",
    description: "Switch active sub-agent thread (Codex sub-agent picker)",
    aliases: [],
    _virtual: true,
    _action: "codex-agent-info",
    // Codex TUI /agent opens OpenAgentPicker — a sub-agent picker. OpenCovibe
    // doesn't have that picker UI yet (sub-agents currently nest inline as
    // Agent tool calls in the timeline). This virtual emits an explainer
    // rather than misroute users to the Extend Agents config page.
    // Claude CLI has no singular /agent command; only Codex.
    _excludeAgents: ["claude"],
  },
  {
    name: "review",
    description: "Review uncommitted changes for bugs and improvements",
    aliases: [],
    _virtual: true,
    _action: "codex-review",
    // Claude CLI has its own /review (PR / security review) that we leave to
    // CLI passthrough. Codex exec mode has no slash dispatch, so we inject
    // a review prompt at the app layer.
    _excludeAgents: ["claude"],
  },
  {
    name: "btw",
    // Codex parity: Codex CLI names this `/side`. Gated to Codex so the `/side`
    // alias isn't intercepted on Claude (where it has no native meaning and would
    // otherwise shadow CLI passthrough). The canonical `/btw` stays on both agents.
    // Listed before the agent-neutral entry so Codex's merged menu surfaces the alias.
    description: "Ask a side question without interrupting the current task",
    aliases: ["side"],
    _virtual: true,
    _action: "side-question",
    argumentHint: "<question>",
    _excludeAgents: ["claude"],
  },
  {
    name: "btw",
    description: "Ask a side question without interrupting the current task",
    aliases: [],
    _virtual: true,
    _action: "side-question",
    argumentHint: "<question>",
    // Codex: ephemeral single-shot (no fork, read-only sandbox)
  },
  {
    name: "init",
    description: "Create an AGENTS.md with project instructions",
    aliases: [],
    _virtual: true,
    _action: "init-project",
    // Claude CLI handles /init itself (writes CLAUDE.md). For Codex (exec mode),
    // OpenCovibe replicates Codex TUI's /init: inject upstream init prompt.
    _excludeAgents: ["claude"],
  },
  {
    name: "memory",
    description: "Edit project memory files (CLAUDE.md / AGENTS.md)",
    // Codex parity: `/memories` is an alias (Codex TUI uses the plural).
    aliases: ["memories"],
    _virtual: true,
    _navigate: "/memory",
  },
  {
    name: "login",
    description: "Sign in to Codex",
    aliases: [],
    _virtual: true,
    _action: "codex-login",
    // Claude CLI's /login is handled by Claude itself (interactive TUI).
    // OpenCovibe only intervenes for Codex (exec mode has no slash dispatch).
    _excludeAgents: ["claude"],
  },
  {
    name: "logout",
    description: "Sign out of Codex",
    aliases: [],
    _virtual: true,
    _action: "codex-logout",
    _excludeAgents: ["claude"],
  },
  {
    name: "stickers",
    description: "Get Claude Code stickers",
    aliases: ["sticker"],
    _virtual: true,
    _action: "open-stickers",
  },
  {
    name: "keybindings",
    description: "Open keybindings settings",
    // Codex CLI names this `/keymap`; we accept both.
    aliases: ["keymap"],
    _virtual: true,
    _navigate: "/settings?tab=shortcuts",
  },
  {
    name: "preview",
    description: "Open localhost preview for element picking",
    aliases: [],
    _virtual: true,
    _action: "toggle-preview",
    argumentHint: "[url]",
  },
  {
    name: "ralph",
    description: "Start a Ralph loop (auto-iterate same prompt until done)",
    aliases: ["ralph-loop"],
    _virtual: true,
    _action: "start-ralph-loop",
    argumentHint: "<prompt> [--max-iterations N] [--completion-promise TEXT]",
    // session_actor Ralph binds stream-session; Codex pipe-exec path not yet implemented
    _excludeAgents: ["codex"],
  },
  {
    name: "cancel-ralph",
    description: "Cancel active Ralph loop",
    aliases: ["stop-ralph"],
    _virtual: true,
    _action: "cancel-ralph-loop",
    _excludeAgents: ["codex"],
  },
];

// ── Agent exclusion helpers ──

function isExcludedForAgent(cmd: CliCommand, agent: string): boolean {
  const excluded = cmd["_excludeAgents"];
  return Array.isArray(excluded) && excluded.includes(agent);
}

/**
 * Resolve a virtual command name/alias to the agent-correct variant.
 * Some names (rewind, compact, …) have per-agent variants distinguished only by
 * `_excludeAgents`; this returns the first one NOT excluded for `agent` so callers
 * dispatch the right `_action` (e.g. Codex /rewind → "codex-rewind", not "rewind").
 */
export function resolveVirtualCommand(
  name: string,
  agent: string = "claude",
): CliCommand | undefined {
  const lname = name.toLowerCase();
  const candidates = VIRTUAL_COMMANDS.filter(
    (v) =>
      v.name.toLowerCase() === lname || (v.aliases ?? []).some((a) => a.toLowerCase() === lname),
  );
  return candidates.find((v) => !isExcludedForAgent(v, agent));
}

/** Known virtual command names+aliases for an agent (for isKnownSlashCommand). */
export function getKnownVirtualNames(agent: string): Set<string> {
  const result = new Set<string>();
  for (const v of VIRTUAL_COMMANDS) {
    if (isExcludedForAgent(v, agent)) continue;
    result.add(v.name.toLowerCase());
    for (const a of v.aliases ?? []) result.add(a.toLowerCase());
  }
  return result;
}

/**
 * Parse /loop command arguments.
 * Extracts: prompt, --max-iterations N, --completion-promise TEXT
 */
export function parseRalphArgs(args: string): {
  prompt: string;
  maxIterations: number;
  completionPromise: string | null;
} {
  let maxIterations = 0;
  let completionPromise: string | null = null;
  const parts: string[] = [];

  const tokens = args.split(/\s+/);
  let i = 0;
  while (i < tokens.length) {
    if (tokens[i] === "--max-iterations" && i + 1 < tokens.length) {
      maxIterations = parseInt(tokens[i + 1], 10) || 0;
      i += 2;
    } else if (tokens[i] === "--completion-promise" && i + 1 < tokens.length) {
      // Collect until next flag or end
      i += 1;
      const promiseParts: string[] = [];
      while (i < tokens.length && !tokens[i].startsWith("--")) {
        promiseParts.push(tokens[i]);
        i += 1;
      }
      completionPromise = promiseParts.join(" ") || null;
    } else {
      parts.push(tokens[i]);
      i += 1;
    }
  }

  return { prompt: parts.join(" "), maxIterations, completionPromise };
}

/**
 * Merge global CLI commands with project-level commands.
 * Project commands override global commands with the same name.
 */
export function mergeProjectCommands(
  globalCommands: CliCommand[],
  projectCommands: CliCommand[],
): CliCommand[] {
  if (projectCommands.length === 0) return globalCommands;
  const projectMap = new Map(projectCommands.map((c) => [c.name, c]));
  // Start with global, override with project where names match
  const merged = globalCommands.map((c) => projectMap.get(c.name) ?? c);
  // Append project commands not in global list
  for (const pc of projectCommands) {
    if (!globalCommands.some((g) => g.name === pc.name)) {
      merged.push(pc);
    }
  }
  return merged;
}

/**
 * Merge CLI commands with virtual commands and apply fallback descriptions.
 * When a CLI command shares a name with a virtual, merge virtual metadata onto it
 * (CLI fields take priority for name/desc/aliases). Append remaining virtuals.
 * Commands with empty descriptions get a fallback from KNOWN_COMMAND_DESCRIPTIONS.
 */
export function mergeWithVirtual(
  cliCommands: CliCommand[],
  agent: string = "claude",
): CliCommand[] {
  const applicableVirtuals = VIRTUAL_COMMANDS.filter((v) => !isExcludedForAgent(v, agent));
  const cliMap = new Map(cliCommands.map((c) => [c.name, c]));
  const result = cliCommands.map((c) => {
    const virtual = applicableVirtuals.find((v) => v.name === c.name);
    let merged = virtual
      ? { ...virtual, ...c, _virtual: true, _enum: virtual["_enum"] ?? false }
      : c;
    // Apply fallback description if empty (works for both virtual-merged and plain CLI commands)
    if (!merged.description) {
      const fallback = KNOWN_COMMAND_DESCRIPTIONS[merged.name];
      if (fallback) merged = { ...merged, description: fallback };
    }
    // Apply fallback argumentHint if missing (prevents immediate execution for commands that need args)
    const hintFallback = KNOWN_ARGUMENT_HINTS[merged.name];
    if (hintFallback && !merged["argumentHint"]) {
      merged = { ...merged, argumentHint: hintFallback };
    }
    return merged;
  });
  // Append virtuals not present in CLI. Some names have multiple applicable
  // variants for one agent (e.g. Codex's base `clear` + the alias-carrying
  // `clear`); append only the first so the menu shows no duplicate entries.
  const appended = new Set<string>();
  for (const v of applicableVirtuals) {
    if (!cliMap.has(v.name) && !appended.has(v.name)) {
      result.push(v);
      appended.add(v.name);
    }
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
export function parseVirtualAction(
  text: string,
  agent: string = "claude",
): { name: string; args: string } | null {
  const match = text.match(/^\/(\S+)(?:\s+(.*))?$/);
  if (!match) return null;
  const name = match[1];
  // Some names (rewind, compact) have per-agent variants — resolve to the variant
  // that ISN'T excluded for this agent so e.g. Codex /rewind resolves to the
  // turn-based variant rather than short-circuiting on the Claude snapshot one.
  const virtual = resolveVirtualCommand(name, agent);
  if (!virtual) return null; // unknown, or all variants excluded for this agent
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
  // Action commands with required arguments (e.g. /btw <question>) need free-text input
  const hint = cmd["argumentHint"];
  if (cmd["_action"] && typeof hint === "string" && hint.startsWith("<")) return "free-text";
  // Virtual action commands execute immediately (args are optional)
  if (cmd["_action"]) return "immediate";
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

/**
 * Extract the slash-command query from input text.
 * Supports both ASCII slash (/) and Chinese dun (、) as trigger prefixes.
 * Returns the query (text after trigger), or null if input doesn't start with a trigger.
 */
export function extractSlashQuery(inputText: string): string | null {
  const m = inputText.match(/^([/、])([a-zA-Z0-9_-]*)$/);
  return m ? m[2] : null;
}

// ── Quick action pills (L3) ──

/** Ordered list of command names shown as quick-action pills above the action bar. */
const CLAUDE_QUICK_ACTIONS: readonly string[] = [
  "compact",
  "copy",
  "model",
  "context",
  "cost",
  "clear",
] as const;

const CODEX_QUICK_ACTIONS: readonly string[] = [
  "compact",
  "copy",
  "diff",
  "status",
  "stats",
] as const;

/** @deprecated Use getQuickActions(allCommands, agent) instead. */
export const QUICK_ACTION_NAMES: readonly string[] = CLAUDE_QUICK_ACTIONS;

/** Return the subset of allCommands that appear in quick-action list, preserving pill order. */
export function getQuickActions(allCommands: CliCommand[], agent: string = "claude"): CliCommand[] {
  const names = agent === "codex" ? CODEX_QUICK_ACTIONS : CLAUDE_QUICK_ACTIONS;
  const map = new Map(allCommands.map((c) => [c.name, c]));
  return names.filter((n) => map.has(n)).map((n) => map.get(n)!);
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
  rewind: "session",
  goal: "session",
  btw: "session",
  loop: "session",
  copy: "session",
  fast: "session",
  files: "session",
  // Coding
  model: "coding",
  diff: "coding",
  review: "coding",
  "security-review": "coding",
  "code-review": "coding",
  simplify: "coding",
  plan: "coding",
  init: "coding",
  "pr-comments": "coding",
  edit: "coding",
  run: "coding",
  terminal: "coding",
  todos: "coding",
  preview: "coding",
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
  plugin: "config",
  ide: "config",
  "add-dir": "config",
  "reload-skills": "config",
  // Coding (continued)
  "team-onboarding": "coding",
  // Help
  help: "help",
  doctor: "help",
  insights: "help",
  stats: "help",
  usage: "help",
  "usage-credits": "help",
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

// Re-export path utilities (moved to path-utils.ts)
export { quoteCliArg, normalizeDirPath, pathsEqual } from "./path-utils";

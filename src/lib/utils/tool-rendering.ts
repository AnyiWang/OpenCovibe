/**
 * Utility functions for rendering tool inputs/outputs in the chat UI.
 */

/** Extract plain text from an array of content blocks (Anthropic format). */
export function extractTextFromBlocks(blocks: unknown[]): string {
  if (!Array.isArray(blocks)) return "";
  return blocks
    .filter((b): b is { type: "text"; text: string } => {
      return typeof b === "object" && b !== null && (b as Record<string, unknown>).type === "text";
    })
    .map((b) => b.text)
    .join("\n");
}

/** Extract display text from opaque tool output (handles string/object/array/null). */
export function extractOutputText(output: unknown): string {
  if (output == null) return "";
  if (typeof output === "string") return output;
  if (typeof output !== "object") return String(output);

  const obj = output as Record<string, unknown>;

  // Content blocks array (Anthropic API format)
  if (Array.isArray(obj.content)) {
    const text = extractTextFromBlocks(obj.content);
    if (text) return text;
  }
  // Direct content string
  if (typeof obj.content === "string" && obj.content) return obj.content;
  // Error fallback
  if (typeof obj.error === "string" && obj.error) return obj.error;
  // Array of content blocks at top level
  if (Array.isArray(output)) {
    const text = extractTextFromBlocks(output);
    if (text) return text;
  }
  // Last resort: JSON stringify
  try {
    return JSON.stringify(output);
  } catch {
    return "[unrenderable output]";
  }
}

/** Extract image content blocks (base64) from tool output, if any. */
export function extractImageBlocks(
  output: unknown,
): Array<{ type: "image"; source: { type: string; media_type: string; data: string } }> {
  if (output == null || typeof output !== "object") return [];
  const obj = output as Record<string, unknown>;
  const blocks = Array.isArray(obj.content) ? obj.content : Array.isArray(output) ? output : [];
  return blocks.filter(
    (b): b is { type: "image"; source: { type: string; media_type: string; data: string } } => {
      return typeof b === "object" && b !== null && (b as Record<string, unknown>).type === "image";
    },
  );
}

const EXT_LANG_MAP: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  py: "python",
  rs: "rust",
  go: "go",
  rb: "ruby",
  java: "java",
  kt: "kotlin",
  swift: "swift",
  c: "c",
  cpp: "cpp",
  h: "c",
  hpp: "cpp",
  cs: "csharp",
  css: "css",
  scss: "scss",
  html: "html",
  json: "json",
  yaml: "yaml",
  yml: "yaml",
  md: "markdown",
  sql: "sql",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  toml: "toml",
  xml: "xml",
  svelte: "html",
  vue: "html",
};

/** Map a file path's extension to a highlight.js language name. */
export function getLanguageFromPath(filePath: string): string {
  const dot = filePath.lastIndexOf(".");
  if (dot < 0) return "";
  const ext = filePath.slice(dot + 1).toLowerCase();
  return EXT_LANG_MAP[ext] ?? "";
}

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif"]);

/** Check if a file path refers to an image type. */
export function isImagePath(filePath: string): boolean {
  const dot = filePath.lastIndexOf(".");
  if (dot < 0) return false;
  return IMAGE_EXTS.has(filePath.slice(dot + 1).toLowerCase());
}

/**
 * Extract structured data from tool.output for team tools (TaskList, etc.).
 * Handles string-wrapped JSON, content blocks, and direct arrays/objects.
 */
export function extractStructuredOutput(output: unknown): unknown {
  if (!output) return null;
  if (typeof output === "string") {
    try {
      return JSON.parse(output);
    } catch {
      return output;
    }
  }
  if (Array.isArray(output)) return output;
  const obj = output as Record<string, unknown>;
  if (obj.content != null) {
    if (typeof obj.content === "string") {
      try {
        return JSON.parse(obj.content);
      } catch {
        return obj.content;
      }
    }
    return obj.content;
  }
  return output;
}

const FRIENDLY_TOOL_NAMES: Record<string, string> = {
  Bash: "Run commands",
  Read: "Read files",
  Write: "Write files",
  Edit: "Edit files",
  Glob: "Find files",
  Grep: "Search content",
  WebFetch: "Fetch URLs",
  WebSearch: "Search web",
  Task: "Run sub-agent",
  NotebookEdit: "Edit notebook",
};

/** Map a tool name to a human-readable description. Falls back to the original name. */
export function friendlyToolName(name: string): string {
  return FRIENDLY_TOOL_NAMES[name] ?? name;
}

/**
 * Detect if a file path targets a Claude plan file (~/.claude/plans/*.md).
 * Matches both absolute paths (/.claude/plans/) and relative paths (.claude/plans/).
 */
export function isPlanFilePath(filePath: string): boolean {
  if (!filePath) return false;
  const normalized = filePath.replaceAll("\\", "/");
  return (
    (normalized.includes("/.claude/plans/") || normalized.startsWith(".claude/plans/")) &&
    normalized.endsWith(".md")
  );
}

/** Extract short plan name from a plan file path. Returns null if not a plan file. */
export function planFileName(filePath: string): string | null {
  if (!isPlanFilePath(filePath)) return null;
  const normalized = filePath.replaceAll("\\", "/");
  const name = normalized.split("/").pop()!.replace(/\.md$/, "");
  return name;
}

// ── Task (subagent) tool metadata extraction ──

export interface TaskToolMeta {
  subagentType: string;
  description?: string;
  model?: string;
  isolation?: string;
  prompt?: string;
}

/** Extract agent metadata from a Task tool's input object. Returns null if not a Task tool input. */
export function extractTaskToolMeta(input: unknown): TaskToolMeta | null {
  if (input == null || typeof input !== "object") return null;
  const obj = input as Record<string, unknown>;
  const subagentType = obj.subagent_type ?? obj.subagentType;
  if (typeof subagentType !== "string") return null;
  return {
    subagentType,
    description: typeof obj.description === "string" ? obj.description : undefined,
    model: typeof obj.model === "string" ? obj.model : undefined,
    isolation: typeof obj.isolation === "string" ? obj.isolation : undefined,
    prompt:
      typeof obj.prompt === "string"
        ? obj.prompt.length > 200
          ? obj.prompt.slice(0, 200) + "…"
          : obj.prompt
        : undefined,
  };
}

// ── Batch / subagent status helpers ──

import type { BusToolItem } from "$lib/types";
import { dbg } from "$lib/utils/debug";

/** Tool is in a terminal state — no further status changes expected. */
export function isToolTerminal(status: BusToolItem["status"]): boolean {
  return (
    status === "success" ||
    status === "error" ||
    status === "denied" ||
    status === "permission_denied"
  );
}

/** Tool is actively working or awaiting interaction. */
export function isToolActive(status: BusToolItem["status"]): boolean {
  return status === "running" || status === "ask_pending" || status === "permission_prompt";
}

/** Whether a tool's subTimeline should be visible by default (no user override).
 *  All tools with subTimelines auto-collapse when in terminal state. */
export function shouldShowSubTimeline(
  status: BusToolItem["status"],
  hasSubTimeline: boolean,
): boolean {
  if (!hasSubTimeline) return false;
  return !isToolTerminal(status);
}

/** Aggregate batch tool statuses in a single pass. */
export function aggregateBatchStatus(tools: BusToolItem[]): {
  completed: number;
  failed: number;
  running: number;
  total: number;
} {
  let completed = 0,
    failed = 0,
    running = 0;
  for (const t of tools) {
    if (t.status === "success") completed++;
    else if (isToolTerminal(t.status)) failed++;
    else if (isToolActive(t.status)) running++;
  }
  return { completed, failed, running, total: tools.length };
}

/** Detect consecutive runs of Task tools (≥3) in a timeline for batch progress display.
 *  Returns Map<startIndex, BusToolItem[]>. */
export function detectBatchGroups(
  timeline: Array<{ kind: string; tool?: BusToolItem }>,
): Map<number, BusToolItem[]> {
  const groups = new Map<number, BusToolItem[]>();
  let i = 0;
  while (i < timeline.length) {
    const entry = timeline[i];
    if (entry.kind === "tool" && entry.tool?.tool_name === "Task") {
      const start = i;
      const tools: BusToolItem[] = [];
      while (
        i < timeline.length &&
        timeline[i].kind === "tool" &&
        timeline[i].tool?.tool_name === "Task"
      ) {
        tools.push(timeline[i].tool!);
        i++;
      }
      if (tools.length >= 3) groups.set(start, tools);
    } else {
      i++;
    }
  }
  return groups;
}

// ── Tool Burst Collapse ──

export interface ToolBurst {
  /** Stable key: first tool's tool_use_id (survives timeline index shifts). */
  key: string;
  startIndex: number;
  endIndex: number; // inclusive
  tools: BusToolItem[];
  /** Per-tool_name count summary, ordered by first appearance. */
  summary: Array<{ toolName: string; count: number }>;
  stats: { completed: number; failed: number; running: number; total: number };
}

const BURST_EXCLUDE = new Set(["Task", "AskUserQuestion", "ExitPlanMode", "EnterPlanMode"]);

/**
 * Detect "tool burst" segments: consecutive tool entries (regardless of tool_name)
 * in the timeline, excluding Task (handled by BatchProgressBar) and interactive tools.
 * Returns Map<startIndex, ToolBurst>.
 */
export function detectToolBursts(
  timeline: Array<{ kind: string; tool?: BusToolItem }>,
  minSize = 4,
): Map<number, ToolBurst> {
  const bursts = new Map<number, ToolBurst>();
  let i = 0;
  while (i < timeline.length) {
    const entry = timeline[i];
    if (entry.kind === "tool" && entry.tool && !BURST_EXCLUDE.has(entry.tool.tool_name)) {
      const start = i;
      const tools: BusToolItem[] = [];
      while (
        i < timeline.length &&
        timeline[i].kind === "tool" &&
        timeline[i].tool &&
        !BURST_EXCLUDE.has(timeline[i].tool!.tool_name)
      ) {
        tools.push(timeline[i].tool!);
        i++;
      }
      // Skip burst at index 0 — may be truncated by renderLimit, key would be unstable
      if (tools.length >= minSize && start > 0) {
        const seen = new Map<string, number>();
        for (const t of tools) {
          seen.set(t.tool_name, (seen.get(t.tool_name) ?? 0) + 1);
        }
        const summary = Array.from(seen, ([toolName, count]) => ({ toolName, count }));
        bursts.set(start, {
          key: tools[0].tool_use_id,
          startIndex: start,
          endIndex: start + tools.length - 1,
          tools,
          summary,
          stats: aggregateBatchStatus(tools),
        });
      }
    } else {
      i++;
    }
  }
  return bursts;
}

/** Extract the /.claude/plans/<name>.md suffix from a plan file path.
 *  Returns null if not a plan file. Works for both absolute and relative paths. */
export function planFileSuffix(filePath: string): string | null {
  if (!filePath) return null;
  const normalized = filePath.replaceAll("\\", "/");
  const idx = normalized.lastIndexOf("/.claude/plans/");
  if (idx >= 0 && normalized.endsWith(".md")) return normalized.slice(idx);
  if (normalized.startsWith(".claude/plans/") && normalized.endsWith(".md"))
    return "/" + normalized;
  return null;
}

/** Flatten timeline entries, inlining subTimeline tool entries from Agent/subagent
 *  tools so that Write/Edit operations inside subagents are visible to plan extraction. */
function flattenToolEntries(
  timeline: Array<{
    kind: string;
    tool?: BusToolItem;
    subTimeline?: Array<{
      kind: string;
      tool?: BusToolItem;
      subTimeline?: Array<{ kind: string; tool?: BusToolItem }>;
    }>;
  }>,
  endIndex: number,
): Array<{ kind: string; tool?: BusToolItem }> {
  const result: Array<{ kind: string; tool?: BusToolItem }> = [];
  for (let i = 0; i < endIndex; i++) {
    const entry = timeline[i];
    result.push(entry);
    // Inline subTimeline tool entries (from Agent/subagent tools)
    if (entry.kind === "tool" && entry.subTimeline) {
      for (const sub of entry.subTimeline) {
        if (sub.kind === "tool") result.push(sub);
        // Recurse one more level for nested subagents
        if (sub.kind === "tool" && sub.subTimeline) {
          for (const subsub of sub.subTimeline) {
            if (subsub.kind === "tool") result.push(subsub);
          }
        }
      }
    }
  }
  return result;
}

/** Extract final plan content from timeline entries before a given index.
 *  Finds the latest successful Write to a plan file, then applies
 *  subsequent successful Edits to the same file.
 *  Searches inside Agent/subagent subTimeline as well.
 *  Stops at any prior ExitPlanMode to avoid crossing plan rounds. */
export function extractPlanContent(
  timeline: Array<{
    kind: string;
    tool?: BusToolItem;
    subTimeline?: Array<{
      kind: string;
      tool?: BusToolItem;
      subTimeline?: Array<{ kind: string; tool?: BusToolItem }>;
    }>;
  }>,
  beforeIndex: number,
): { content: string; fileName: string } | null {
  // Flatten timeline: inline subTimeline entries so Write/Edit inside agents are visible
  const flat = flattenToolEntries(timeline, beforeIndex);

  // 1. Search backwards for latest successful plan Write, stop at completed ExitPlanMode
  let writeIndex = -1;
  let baseContent: string | null = null;
  let baseSuffix: string | null = null;
  let baseName: string | null = null;

  for (let i = flat.length - 1; i >= 0; i--) {
    const entry = flat[i];
    if (entry.kind !== "tool" || !entry.tool) continue;

    // Boundary: completed ExitPlanMode (previous round)
    // Use its tool_use_result.plan as base content if available (cross-round editing)
    if (entry.tool.tool_name === "ExitPlanMode" && entry.tool.status === "success") {
      const result = entry.tool.tool_use_result as { plan?: string; filePath?: string } | undefined;
      if (result?.plan && typeof result.plan === "string") {
        const fp = result.filePath ?? "";
        writeIndex = i;
        baseContent = result.plan;
        baseSuffix = isPlanFilePath(fp) ? planFileSuffix(fp) : null;
        baseName = isPlanFilePath(fp) ? planFileName(fp) : "plan";
        dbg("plan", "extractPlanContent: using plan from completed ExitPlanMode", {
          i,
          name: baseName,
        });
      } else {
        dbg("plan", "extractPlanContent: hit completed ExitPlanMode without plan content", { i });
      }
      break;
    }

    if (entry.tool.status !== "success") continue;
    const fp = String(entry.tool.input?.file_path ?? entry.tool.input?.path ?? "");
    if (!isPlanFilePath(fp)) continue;

    if (entry.tool.tool_name === "Write" && typeof entry.tool.input?.content === "string") {
      writeIndex = i;
      baseContent = entry.tool.input.content as string;
      baseSuffix = planFileSuffix(fp);
      baseName = planFileName(fp);
      dbg("plan", "extractPlanContent: found base Write", { i, name: baseName });
      break;
    }
  }

  if (writeIndex < 0 || !baseContent || !baseName) return null;

  // 2. Apply subsequent successful Edits to the same plan file
  let content = baseContent;
  for (let i = writeIndex + 1; i < flat.length; i++) {
    const entry = flat[i];
    if (entry.kind !== "tool" || !entry.tool) continue;
    if (entry.tool.status !== "success") continue;
    const fp = String(entry.tool.input?.file_path ?? entry.tool.input?.path ?? "");
    if (!isPlanFilePath(fp)) continue;
    // Compare suffix path (/.claude/plans/<name>.md) for cross-format compatibility
    // When baseSuffix is null (e.g. from ExitPlanMode without filePath), accept any plan file
    if (baseSuffix && planFileSuffix(fp) !== baseSuffix) continue;

    if (entry.tool.tool_name === "Write" && typeof entry.tool.input?.content === "string") {
      content = entry.tool.input.content as string;
      dbg("plan", "extractPlanContent: overwrite by later Write", { i });
    } else if (
      entry.tool.tool_name === "Edit" &&
      typeof entry.tool.input?.old_string === "string"
    ) {
      const oldStr = entry.tool.input.old_string as string;
      const newStr = (entry.tool.input?.new_string as string) ?? "";
      if (content.includes(oldStr)) {
        content = content.replace(oldStr, newStr);
        dbg("plan", "extractPlanContent: applied Edit", { i });
      } else {
        dbg("plan", "extractPlanContent: Edit old_string not found, skipped", { i });
      }
    }
  }

  return { content, fileName: baseName };
}

/** Apply forward Edits to an approved plan's content.
 *  Starting after the given index, scan forward for successful
 *  Write/Edit to the same plan file and apply them.
 *  This keeps the approved plan card up-to-date when the plan file
 *  is edited after approval in the same session. */
export function applyPlanEditsForward(
  timeline: Array<{
    kind: string;
    tool?: BusToolItem;
    subTimeline?: Array<{
      kind: string;
      tool?: BusToolItem;
      subTimeline?: Array<{ kind: string; tool?: BusToolItem }>;
    }>;
  }>,
  afterIndex: number,
  basePlan: string,
  planFilePath?: string,
): string {
  const baseSuffix =
    planFilePath && isPlanFilePath(planFilePath) ? planFileSuffix(planFilePath) : null;
  let content = basePlan;

  function applyTool(tool: BusToolItem): void {
    if (tool.status !== "success") return;
    const fp = String(tool.input?.file_path ?? tool.input?.path ?? "");
    if (!isPlanFilePath(fp)) return;
    if (baseSuffix && planFileSuffix(fp) !== baseSuffix) return;

    if (tool.tool_name === "Write" && typeof tool.input?.content === "string") {
      content = tool.input.content as string;
      dbg("plan", "applyPlanEditsForward: overwrite by later Write");
    } else if (tool.tool_name === "Edit" && typeof tool.input?.old_string === "string") {
      const oldStr = tool.input.old_string as string;
      const newStr = (tool.input?.new_string as string) ?? "";
      if (content.includes(oldStr)) {
        content = content.replace(oldStr, newStr);
        dbg("plan", "applyPlanEditsForward: applied Edit");
      }
    }
  }

  for (let i = afterIndex + 1; i < timeline.length; i++) {
    const entry = timeline[i];
    if (entry.kind !== "tool" || !entry.tool) continue;
    applyTool(entry.tool);
    // Also check subTimeline (agent/subagent)
    if (entry.subTimeline) {
      for (const sub of entry.subTimeline) {
        if (sub.kind === "tool" && sub.tool) applyTool(sub.tool);
        if (sub.subTimeline) {
          for (const subsub of sub.subTimeline) {
            if (subsub.kind === "tool" && subsub.tool) applyTool(subsub.tool);
          }
        }
      }
    }
  }

  return content;
}

// ── Tool Render Level ──

/** Tools whose output is the primary content (auto-expand, accent border). */
const LEVEL_2_TOOLS = new Set(["Bash", "bash", "Edit", "edit_file", "Write", "write_file"]);

/**
 * Determine the render level for a tool card.
 * Level 1 = one-liner (info tools), Level 2 = inline content (output tools), Level 3 = interactive card.
 */
export function getToolRenderLevel(toolName: string, status: BusToolItem["status"]): 1 | 2 | 3 {
  // AskUserQuestion is always Level 3 (all states: active, done, denied)
  if (toolName === "AskUserQuestion") return 3;
  // Interactive statuses: user must approve/deny
  if (status === "permission_prompt") return 3;
  // ExitPlanMode only needs Level 3 when it's a permission_prompt (plan approval card)
  // Other states (running, success, error) use Level 1 one-liner
  if (toolName === "ExitPlanMode" && status === "permission_prompt") return 3;
  // Output-focused tools (including cross-provider aliases)
  if (LEVEL_2_TOOLS.has(toolName)) return 2;
  // Everything else
  return 1;
}

// ── Permission / tool input utilities ──

import type { PermissionSuggestion } from "$lib/types";

/** Extract a human-readable detail string from tool input (file path, command, pattern, etc.). */
export function getToolDetail(input: Record<string, unknown> | undefined): string {
  if (!input || Object.keys(input).length === 0) return "";
  return (
    (input.file_path as string) ??
    (input.path as string) ??
    (input.command as string) ??
    (input.pattern as string) ??
    (input.query as string) ??
    (input.url as string) ??
    (input.team_name as string) ??
    (input.subject as string) ??
    (input.taskId != null || input.task_id != null
      ? `#${input.taskId ?? input.task_id}`
      : undefined) ??
    (input.skill as string) ??
    (input.recipient as string) ??
    ""
  );
}

/** Format a permission suggestion label for display.
 *  Requires a `t` translation function since this runs outside Svelte component context. */
export function formatSuggestionLabel(
  s: PermissionSuggestion,
  t: (key: string, params?: Record<string, string>) => string,
): string {
  if (s.type === "addRules" && s.rules?.length && s.behavior === "allow") {
    return t("inline_alwaysAllow") + ` ${s.rules[0]}`;
  }
  if (s.type === "setMode" && s.mode) {
    return t("inline_switchToMode", { mode: s.mode });
  }
  if (s.type === "addDirectories" && s.directories?.length) {
    return t("inline_addDirectory", { dir: s.directories[0] });
  }
  if (s.type === "additionalContext") {
    return t("inline_applyHookContext");
  }
  return `Apply: ${s.type}`;
}

/** Copy text to clipboard with legacy fallback for Tauri WebView. */
export async function copyToClipboard(text: string): Promise<void> {
  if (navigator.clipboard) {
    await navigator.clipboard.writeText(text);
  } else {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();
    document.execCommand("copy");
    document.body.removeChild(textarea);
  }
}

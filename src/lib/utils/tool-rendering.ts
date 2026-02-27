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

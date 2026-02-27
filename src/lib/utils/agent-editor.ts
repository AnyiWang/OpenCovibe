import { dbg } from "./debug";

// ── Types ──

export interface AgentFormData {
  name: string;
  description: string;
  model: string; // "inherit" | "sonnet" | "opus" | "haiku"
  tools: string[];
  disallowedTools: string[];
  permissionMode: string; // "default" | "acceptEdits" | "dontAsk" | "bypassPermissions" | "plan"
  maxTurns: number | null;
  memory: string; // "" | memory file path/glob
  background: boolean;
  isolation: string; // "" | "worktree"
  systemPrompt: string;
}

export interface ValidationError {
  field: string;
  message: string;
}

// ── Constants ──

const BUILTIN_AGENT_NAMES = [
  "explore",
  "plan",
  "general-purpose",
  "claude-code-guide",
  "statusline-setup",
];

const VALID_MODELS = ["inherit", "sonnet", "opus", "haiku"];
const VALID_PERMISSION_MODES = ["default", "acceptEdits", "dontAsk", "bypassPermissions", "plan"];

const NAME_REGEX = /^[a-z0-9][a-z0-9-]{0,63}$/;

// ── Serialization (form → .md content, for creating new agents) ──

export function serializeAgentFile(data: AgentFormData): string {
  const lines: string[] = ["---"];

  lines.push(`name: ${data.name}`);
  lines.push(`description: ${yamlString(data.description)}`);

  if (data.model && data.model !== "inherit") {
    lines.push(`model: ${data.model}`);
  }
  if (data.tools.length > 0) {
    lines.push("tools:");
    for (const tool of data.tools) {
      lines.push(`  - ${tool}`);
    }
  }
  if (data.disallowedTools.length > 0) {
    lines.push("disallowedTools:");
    for (const tool of data.disallowedTools) {
      lines.push(`  - ${tool}`);
    }
  }
  if (data.permissionMode && data.permissionMode !== "default") {
    lines.push(`permissionMode: ${data.permissionMode}`);
  }
  if (data.maxTurns != null && data.maxTurns > 0) {
    lines.push(`maxTurns: ${data.maxTurns}`);
  }
  if (data.memory) {
    lines.push(`memory: ${yamlString(data.memory)}`);
  }
  if (data.background) {
    lines.push("background: true");
  }
  if (data.isolation === "worktree") {
    lines.push("isolation: worktree");
  }

  lines.push("---");
  lines.push("");

  if (data.systemPrompt.trim()) {
    lines.push(data.systemPrompt.trim());
    lines.push("");
  }

  return lines.join("\n");
}

function yamlString(s: string): string {
  if (
    s.includes(":") ||
    s.includes("#") ||
    s.includes('"') ||
    s.includes("'") ||
    s.startsWith(" ") ||
    s.endsWith(" ")
  ) {
    return `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  }
  return s;
}

// ── Parsing (raw .md → form data, for form-view only) ──

export function parseAgentFile(content: string): AgentFormData {
  const defaultData: AgentFormData = {
    name: "",
    description: "",
    model: "inherit",
    tools: [],
    disallowedTools: [],
    permissionMode: "default",
    maxTurns: null,
    memory: "",
    background: false,
    isolation: "",
    systemPrompt: "",
  };

  const trimmed = content.trim();
  if (!trimmed.startsWith("---")) {
    return { ...defaultData, systemPrompt: trimmed };
  }

  const afterFirst = trimmed.slice(3);
  const endIdx = afterFirst.indexOf("\n---");
  if (endIdx === -1) {
    return { ...defaultData, systemPrompt: trimmed };
  }

  const yamlStr = afterFirst.slice(0, endIdx);
  const body = afterFirst.slice(endIdx + 4).replace(/^\n+/, "");

  // Simple line-by-line YAML extraction (no full parser needed for known fields)
  const fm = parseSimpleYaml(yamlStr);

  return {
    name: fm.name ?? "",
    description: fm.description ?? "",
    model: fm.model ?? "inherit",
    tools: fm.tools ?? [],
    disallowedTools: fm.disallowedTools ?? [],
    permissionMode: fm.permissionMode ?? "default",
    maxTurns: fm.maxTurns ?? null,
    memory: fm.memory ?? "",
    background: fm.background ?? false,
    isolation: fm.isolation ?? "",
    systemPrompt: body,
  };
}

interface SimpleFm {
  name?: string;
  description?: string;
  model?: string;
  tools?: string[];
  disallowedTools?: string[];
  permissionMode?: string;
  maxTurns?: number | null;
  memory?: string;
  background?: boolean;
  isolation?: string;
}

function parseSimpleYaml(yaml: string): SimpleFm {
  const result: SimpleFm = {};
  const lines = yaml.split("\n");
  let currentArray: string[] | null = null;
  let currentKey = "";

  for (const line of lines) {
    const trimLine = line.trimEnd();

    // Array item: "  - value"
    if (trimLine.match(/^\s+-\s+/) && currentArray) {
      const value = trimLine.replace(/^\s+-\s+/, "").trim();
      if (value) currentArray.push(value);
      continue;
    }

    // Finish previous array
    if (currentArray) {
      setArrayField(result, currentKey, currentArray);
      currentArray = null;
    }

    // Key-value pair: "key: value"
    const kvMatch = trimLine.match(/^(\w+):\s*(.*)/);
    if (kvMatch) {
      const [, key, rawValue] = kvMatch;
      const value = stripYamlQuotes(rawValue.trim());

      switch (key) {
        case "name":
          result.name = value;
          break;
        case "description":
          result.description = value;
          break;
        case "model":
          result.model = value;
          break;
        case "permissionMode":
          result.permissionMode = value;
          break;
        case "maxTurns":
          result.maxTurns = parseInt(value, 10) || null;
          break;
        case "memory":
          result.memory = value;
          break;
        case "background":
          result.background = value === "true";
          break;
        case "isolation":
          result.isolation = value;
          break;
        case "tools":
          if (!value) {
            currentArray = [];
            currentKey = "tools";
          } else {
            result.tools = [value];
          }
          break;
        case "disallowedTools":
          if (!value) {
            currentArray = [];
            currentKey = "disallowedTools";
          } else {
            result.disallowedTools = [value];
          }
          break;
      }
    }
  }

  // Final array
  if (currentArray) {
    setArrayField(result, currentKey, currentArray);
  }

  return result;
}

function setArrayField(fm: SimpleFm, key: string, arr: string[]): void {
  if (key === "tools") fm.tools = arr;
  if (key === "disallowedTools") fm.disallowedTools = arr;
}

function stripYamlQuotes(s: string): string {
  if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
    return s.slice(1, -1);
  }
  return s;
}

// ── Validation ──

export function validateAgentForm(data: AgentFormData): ValidationError[] {
  const errors: ValidationError[] = [];

  if (!data.name) {
    errors.push({ field: "name", message: "Name is required" });
  } else if (!NAME_REGEX.test(data.name)) {
    errors.push({
      field: "name",
      message: "Must match [a-z0-9][a-z0-9-]{0,63}",
    });
  } else if (BUILTIN_AGENT_NAMES.includes(data.name.toLowerCase())) {
    errors.push({ field: "name", message: "Conflicts with a built-in agent" });
  }

  if (!data.description) {
    errors.push({ field: "description", message: "Description is required" });
  } else if (data.description.length > 500) {
    errors.push({
      field: "description",
      message: "Description too long (max 500 characters)",
    });
  }

  if (data.model && !VALID_MODELS.includes(data.model)) {
    errors.push({
      field: "model",
      message: `Invalid model: ${data.model}`,
    });
  }

  if (data.permissionMode && !VALID_PERMISSION_MODES.includes(data.permissionMode)) {
    errors.push({
      field: "permissionMode",
      message: `Invalid permission mode: ${data.permissionMode}`,
    });
  }

  if (data.maxTurns != null) {
    if (!Number.isInteger(data.maxTurns) || data.maxTurns <= 0) {
      errors.push({
        field: "maxTurns",
        message: "Must be a positive integer",
      });
    }
  }

  for (const tool of data.tools) {
    if (!tool.trim()) {
      errors.push({ field: "tools", message: "Tool name cannot be empty" });
      break;
    }
  }

  return errors;
}

// ── Source mode content validation (light check before save) ──

export interface SourceValidationResult {
  valid: boolean;
  warnings: string[];
}

export function validateSourceContent(
  content: string,
  existingAgentNames: string[],
): SourceValidationResult {
  const warnings: string[] = [];
  const trimmed = content.trim();

  if (!trimmed.startsWith("---")) {
    warnings.push("No frontmatter found (missing --- delimiters)");
    return { valid: false, warnings };
  }

  const afterFirst = trimmed.slice(3);
  const endIdx = afterFirst.indexOf("\n---");
  if (endIdx === -1) {
    warnings.push("Unclosed frontmatter (missing closing ---)");
    return { valid: false, warnings };
  }

  const yamlStr = afterFirst.slice(0, endIdx);
  const fm = parseSimpleYaml(yamlStr);

  if (!fm.name) {
    warnings.push("Frontmatter missing required field: name");
  } else {
    if (!NAME_REGEX.test(fm.name)) {
      warnings.push(`Name "${fm.name}" has invalid format (expected lowercase, numbers, hyphens)`);
    }
    if (BUILTIN_AGENT_NAMES.includes(fm.name.toLowerCase())) {
      warnings.push(`Name "${fm.name}" conflicts with a built-in agent`);
    }
    if (existingAgentNames.includes(fm.name)) {
      warnings.push(`Name "${fm.name}" conflicts with another custom agent`);
    }
  }

  if (!fm.description) {
    warnings.push("Frontmatter missing required field: description");
  }

  dbg("agent-editor", "validateSourceContent", { warnings });
  return { valid: warnings.length === 0, warnings };
}

// ── Extract frontmatter name (for display purposes) ──

export function extractFrontmatterName(content: string): string | null {
  const trimmed = content.trim();
  if (!trimmed.startsWith("---")) return null;

  const afterFirst = trimmed.slice(3);
  const endIdx = afterFirst.indexOf("\n---");
  if (endIdx === -1) return null;

  const yamlStr = afterFirst.slice(0, endIdx);
  const fm = parseSimpleYaml(yamlStr);
  return fm.name ?? null;
}

export function defaultFormData(): AgentFormData {
  return {
    name: "",
    description: "",
    model: "inherit",
    tools: [],
    disallowedTools: [],
    permissionMode: "default",
    maxTurns: null,
    memory: "",
    background: false,
    isolation: "",
    systemPrompt: "",
  };
}

import { describe, it, expect } from "vitest";
import {
  serializeAgentFile,
  parseAgentFile,
  validateAgentForm,
  validateSourceContent,
  extractFrontmatterName,
  defaultFormData,
  type AgentFormData,
} from "../agent-editor";

describe("serializeAgentFile", () => {
  it("all fields", () => {
    const data: AgentFormData = {
      name: "code-reviewer",
      description: "Reviews code quality",
      model: "sonnet",
      tools: ["Read", "Grep"],
      disallowedTools: ["Write"],
      permissionMode: "plan",
      maxTurns: 10,
      memory: "MEMORY.md",
      background: true,
      isolation: "worktree",
      systemPrompt: "You are a code reviewer.",
    };
    const result = serializeAgentFile(data);
    expect(result).toContain("name: code-reviewer");
    expect(result).toContain("description: Reviews code quality");
    expect(result).toContain("model: sonnet");
    expect(result).toContain("tools:");
    expect(result).toContain("  - Read");
    expect(result).toContain("  - Grep");
    expect(result).toContain("disallowedTools:");
    expect(result).toContain("  - Write");
    expect(result).toContain("permissionMode: plan");
    expect(result).toContain("maxTurns: 10");
    expect(result).toContain("memory: MEMORY.md");
    expect(result).toContain("background: true");
    expect(result).toContain("isolation: worktree");
    expect(result).toContain("You are a code reviewer.");
  });

  it("minimal — only required fields", () => {
    const data: AgentFormData = {
      ...defaultFormData(),
      name: "simple",
      description: "A simple agent",
    };
    const result = serializeAgentFile(data);
    expect(result).toContain("name: simple");
    expect(result).toContain("description: A simple agent");
    expect(result).not.toContain("model:");
    expect(result).not.toContain("tools:");
    expect(result).not.toContain("disallowedTools:");
    expect(result).not.toContain("permissionMode:");
    expect(result).not.toContain("maxTurns:");
    expect(result).not.toContain("memory:");
    expect(result).not.toContain("background:");
    expect(result).not.toContain("isolation:");
  });

  it("empty arrays are not output", () => {
    const data: AgentFormData = {
      ...defaultFormData(),
      name: "test",
      description: "test",
      tools: [],
      disallowedTools: [],
    };
    const result = serializeAgentFile(data);
    expect(result).not.toContain("tools:");
    expect(result).not.toContain("disallowedTools:");
  });
});

describe("parseAgentFile", () => {
  it("roundtrip — serialize then parse yields same data", () => {
    const original: AgentFormData = {
      name: "test-agent",
      description: "A test agent",
      model: "haiku",
      tools: ["Read", "Grep"],
      disallowedTools: ["Write"],
      permissionMode: "plan",
      maxTurns: 5,
      memory: "MEMORY.md",
      background: true,
      isolation: "worktree",
      systemPrompt: "You are a test agent.",
    };
    const serialized = serializeAgentFile(original);
    const parsed = parseAgentFile(serialized);

    expect(parsed.name).toBe(original.name);
    expect(parsed.description).toBe(original.description);
    expect(parsed.model).toBe(original.model);
    expect(parsed.tools).toEqual(original.tools);
    expect(parsed.disallowedTools).toEqual(original.disallowedTools);
    expect(parsed.permissionMode).toBe(original.permissionMode);
    expect(parsed.maxTurns).toBe(original.maxTurns);
    expect(parsed.memory).toBe(original.memory);
    expect(parsed.background).toBe(original.background);
    expect(parsed.isolation).toBe(original.isolation);
    expect(parsed.systemPrompt).toBe(original.systemPrompt);
  });

  it("no frontmatter — entire content is systemPrompt", () => {
    const content = "Just a plain markdown file.";
    const parsed = parseAgentFile(content);
    expect(parsed.name).toBe("");
    expect(parsed.systemPrompt).toBe("Just a plain markdown file.");
  });
});

describe("validateAgentForm", () => {
  it("valid input — 0 errors", () => {
    const data: AgentFormData = {
      ...defaultFormData(),
      name: "my-agent",
      description: "A valid agent",
    };
    expect(validateAgentForm(data)).toHaveLength(0);
  });

  it("name format — various invalid names", () => {
    const cases = ["", "FOO", "has space", "a@b", "_start", "-start"];
    for (const name of cases) {
      const errors = validateAgentForm({ ...defaultFormData(), name, description: "ok" });
      expect(errors.some((e) => e.field === "name")).toBe(true);
    }
  });

  it("name — built-in conflict", () => {
    const errors = validateAgentForm({
      ...defaultFormData(),
      name: "explore",
      description: "ok",
    });
    expect(errors.some((e) => e.message.includes("built-in"))).toBe(true);
  });

  it("description — too long", () => {
    const errors = validateAgentForm({
      ...defaultFormData(),
      name: "ok",
      description: "x".repeat(501),
    });
    expect(errors.some((e) => e.field === "description")).toBe(true);
  });

  it("maxTurns — negative", () => {
    const errors = validateAgentForm({
      ...defaultFormData(),
      name: "ok",
      description: "ok",
      maxTurns: -1,
    });
    expect(errors.some((e) => e.field === "maxTurns")).toBe(true);
  });

  it("maxTurns — zero", () => {
    const errors = validateAgentForm({
      ...defaultFormData(),
      name: "ok",
      description: "ok",
      maxTurns: 0,
    });
    expect(errors.some((e) => e.field === "maxTurns")).toBe(true);
  });

  it("maxTurns — decimal", () => {
    const errors = validateAgentForm({
      ...defaultFormData(),
      name: "ok",
      description: "ok",
      maxTurns: 3.5,
    });
    expect(errors.some((e) => e.field === "maxTurns")).toBe(true);
  });
});

describe("validateSourceContent", () => {
  it("valid content — 0 warnings", () => {
    const content = "---\nname: my-agent\ndescription: A test\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(true);
    expect(result.warnings).toHaveLength(0);
  });

  it("missing name — warning", () => {
    const content = "---\ndescription: A test\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("name"))).toBe(true);
  });

  it("missing description — warning", () => {
    const content = "---\nname: my-agent\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("description"))).toBe(true);
  });

  it("name format invalid — warning", () => {
    const content = "---\nname: BAD NAME\ndescription: ok\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("invalid format"))).toBe(true);
  });

  it("name conflicts with built-in — warning", () => {
    const content = "---\nname: explore\ndescription: ok\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("built-in"))).toBe(true);
  });

  it("name conflicts with existing agent — warning", () => {
    const content = "---\nname: my-agent\ndescription: ok\n---\nBody.";
    const result = validateSourceContent(content, ["my-agent"]);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("conflicts with another custom agent"))).toBe(
      true,
    );
  });

  it("no frontmatter — warning", () => {
    const content = "Just some text without frontmatter.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    expect(result.warnings.some((w) => w.includes("No frontmatter"))).toBe(true);
  });

  it("force save bypasses validation", () => {
    // This test validates the interface contract: validateSourceContent returns
    // warnings but does not block the save — the caller decides whether to proceed.
    const content = "---\ndescription: no name\n---\nBody.";
    const result = validateSourceContent(content, []);
    expect(result.valid).toBe(false);
    // The caller can ignore warnings and still call updateAgentFile (force save)
    expect(result.warnings.length).toBeGreaterThan(0);
  });
});

describe("extractFrontmatterName", () => {
  it("present — returns name", () => {
    const content = "---\nname: my-agent\ndescription: ok\n---\nBody.";
    expect(extractFrontmatterName(content)).toBe("my-agent");
  });

  it("missing name field — returns null", () => {
    const content = "---\ndescription: ok\n---\nBody.";
    expect(extractFrontmatterName(content)).toBeNull();
  });

  it("no frontmatter — returns null", () => {
    const content = "Just text.";
    expect(extractFrontmatterName(content)).toBeNull();
  });
});

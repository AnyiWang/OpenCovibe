import { describe, it, expect } from "vitest";
import {
  parseContextMarkdown,
  computeContextDelta,
  getColor,
  getIcon,
  CATEGORY_COLORS,
  type ContextData,
} from "../context-parser";

// ── Fixtures ──

const VALID_CONTEXT_MD = `## Context Window

**Model:** claude-sonnet-4-20250514
**Tokens:** 104,251/200,000 (52%)

### Estimated usage by category
| Category | Tokens | % |
|---|---|---|
| System prompt | 12,000 | 6.0% |
| Messages | 45,000 | 22.5% |
| MCP tools | 8,000 | 4.0% |
| Free space | 96,000 | 48.0% |

### MCP Tools
| Tool | Server | Tokens |
|---|---|---|
| context7 | context7 | 2,000 |
| fetch | web-fetch | 1,500 |
`;

const MINIMAL_CONTEXT_MD = `## Context Window

**Model:** claude-haiku-4-5-20251001
**Tokens:** 10k/100k (10%)

### Estimated usage by category
| Category | Tokens | % |
|---|---|---|
| System prompt | 5k | 5.0% |
| Free space | 90k | 90.0% |
`;

describe("parseContextMarkdown", () => {
  it("parses valid context markdown", () => {
    const result = parseContextMarkdown(VALID_CONTEXT_MD);
    expect(result).not.toBeNull();
    expect(result!.model).toBe("claude-sonnet-4-20250514");
    expect(result!.usedTokens).toBe("104,251");
    expect(result!.maxTokens).toBe("200,000");
    expect(result!.percentage).toBe(52);
    expect(result!.categories).toHaveLength(4);
    expect(result!.categories[0]).toEqual({
      name: "System prompt",
      tokens: "12,000",
      percentage: 6.0,
    });
    expect(result!.categories[2]).toEqual({
      name: "MCP tools",
      tokens: "8,000",
      percentage: 4.0,
    });
  });

  it("parses sub-tables", () => {
    const result = parseContextMarkdown(VALID_CONTEXT_MD);
    expect(result).not.toBeNull();
    expect(result!.subTables).toHaveLength(1);
    expect(result!.subTables[0].title).toBe("MCP Tools");
    expect(result!.subTables[0].rows).toHaveLength(2);
    expect(result!.subTables[0].rows[0][0]).toBe("context7");
  });

  it("returns null for missing model", () => {
    expect(parseContextMarkdown("no model here")).toBeNull();
  });

  it("returns null for missing tokens", () => {
    expect(parseContextMarkdown("**Model:** test\nno tokens")).toBeNull();
  });

  it("returns null for empty categories", () => {
    const md = `**Model:** test\n**Tokens:** 1k/10k (10%)`;
    expect(parseContextMarkdown(md)).toBeNull();
  });

  it("parses minimal context", () => {
    const result = parseContextMarkdown(MINIMAL_CONTEXT_MD);
    expect(result).not.toBeNull();
    expect(result!.categories).toHaveLength(2);
    expect(result!.subTables).toHaveLength(0);
  });
});

describe("computeContextDelta", () => {
  const prev: ContextData = {
    model: "claude-sonnet-4-20250514",
    usedTokens: "80k",
    maxTokens: "200k",
    percentage: 40,
    categories: [
      { name: "System prompt", tokens: "10k", percentage: 5.0 },
      { name: "Messages", tokens: "30k", percentage: 15.0 },
      { name: "Free space", tokens: "120k", percentage: 60.0 },
    ],
    subTables: [],
  };

  const curr: ContextData = {
    model: "claude-sonnet-4-20250514",
    usedTokens: "104k",
    maxTokens: "200k",
    percentage: 52,
    categories: [
      { name: "System prompt", tokens: "10k", percentage: 5.0 },
      { name: "Messages", tokens: "45k", percentage: 22.5 },
      { name: "MCP tools", tokens: "8k", percentage: 4.0 },
      { name: "Free space", tokens: "96k", percentage: 48.0 },
    ],
    subTables: [],
  };

  it("computes overall percentage delta", () => {
    const delta = computeContextDelta(prev, curr);
    expect(delta.pctDelta).toBe(12);
  });

  it("computes category deltas for changed categories", () => {
    const delta = computeContextDelta(prev, curr);
    const messagesDelta = delta.categoryDeltas.find((d) => d.name === "Messages");
    expect(messagesDelta).toBeDefined();
    expect(messagesDelta!.pctBefore).toBe(15.0);
    expect(messagesDelta!.pctAfter).toBe(22.5);
    expect(messagesDelta!.pctDelta).toBe(7.5);
  });

  it("includes new categories (present in curr but not prev)", () => {
    const delta = computeContextDelta(prev, curr);
    const mcpDelta = delta.categoryDeltas.find((d) => d.name === "MCP tools");
    expect(mcpDelta).toBeDefined();
    expect(mcpDelta!.pctBefore).toBe(0);
    expect(mcpDelta!.pctAfter).toBe(4.0);
  });

  it("excludes unchanged categories", () => {
    const delta = computeContextDelta(prev, curr);
    const systemDelta = delta.categoryDeltas.find((d) => d.name === "System prompt");
    expect(systemDelta).toBeUndefined();
  });

  it("handles zero change", () => {
    const delta = computeContextDelta(prev, prev);
    expect(delta.pctDelta).toBe(0);
    expect(delta.categoryDeltas).toHaveLength(0);
  });

  it("handles negative delta (context decreased)", () => {
    const delta = computeContextDelta(curr, prev);
    expect(delta.pctDelta).toBe(-12);
    const freeSpaceDelta = delta.categoryDeltas.find((d) => d.name === "Free space");
    expect(freeSpaceDelta).toBeDefined();
    expect(freeSpaceDelta!.pctDelta).toBe(12); // Free space increased
  });
});

describe("getColor / getIcon", () => {
  it("returns correct color for known categories", () => {
    expect(getColor("System prompt")).toBe("#a78bfa");
    expect(getColor("Messages")).toBe("#f472b6");
    expect(getColor("MCP tools")).toBe("#34d399");
  });

  it("returns gray for free space", () => {
    expect(getColor("Free space")).toBe("#6b7280");
  });

  it("returns fallback for unknown categories", () => {
    expect(getColor("Unknown Category")).toBe("#9ca3af");
  });

  it("returns correct icons", () => {
    expect(getIcon("Free space")).toBe("▢");
    expect(getIcon("Autocompact buffer")).toBe("⊠");
    expect(getIcon("System prompt")).toBe("⛁");
  });
});

describe("CATEGORY_COLORS", () => {
  it("has entries for main categories", () => {
    expect(CATEGORY_COLORS["System prompt"]).toBeDefined();
    expect(CATEGORY_COLORS["Messages"]).toBeDefined();
    expect(CATEGORY_COLORS["MCP tools"]).toBeDefined();
    expect(CATEGORY_COLORS["Skills"]).toBeDefined();
  });
});

import { describe, it, expect } from "vitest";
import { getToolDetail, formatSuggestionLabel } from "./tool-rendering";
import type { PermissionSuggestion } from "$lib/types";

describe("getToolDetail", () => {
  it("returns file_path when present", () => {
    expect(getToolDetail({ file_path: "/src/foo.ts" })).toBe("/src/foo.ts");
  });

  it("returns path when file_path is absent", () => {
    expect(getToolDetail({ path: "/tmp/dir" })).toBe("/tmp/dir");
  });

  it("returns command for Bash tools", () => {
    expect(getToolDetail({ command: "npm test" })).toBe("npm test");
  });

  it("returns pattern for Grep/Glob tools", () => {
    expect(getToolDetail({ pattern: "*.ts" })).toBe("*.ts");
  });

  it("returns query for WebSearch tools", () => {
    expect(getToolDetail({ query: "search term" })).toBe("search term");
  });

  it("returns url for WebFetch tools", () => {
    expect(getToolDetail({ url: "https://example.com" })).toBe("https://example.com");
  });

  it("returns taskId with # prefix", () => {
    expect(getToolDetail({ taskId: "42" })).toBe("#42");
  });

  it("returns task_id with # prefix", () => {
    expect(getToolDetail({ task_id: "7" })).toBe("#7");
  });

  it("returns skill name", () => {
    expect(getToolDetail({ skill: "commit" })).toBe("commit");
  });

  it("returns recipient name", () => {
    expect(getToolDetail({ recipient: "researcher" })).toBe("researcher");
  });

  it("returns empty string for empty input", () => {
    expect(getToolDetail({})).toBe("");
  });

  it("returns empty string for undefined input", () => {
    expect(getToolDetail(undefined)).toBe("");
  });

  it("returns empty string for null input", () => {
    expect(getToolDetail(undefined)).toBe("");
  });
});

describe("formatSuggestionLabel", () => {
  const mockT = (key: string, params?: Record<string, string>) => {
    if (key === "inline_alwaysAllow") return "Always Allow";
    if (key === "inline_switchToMode") return `Switch to ${params?.mode} mode`;
    if (key === "inline_addDirectory") return `Add directory: ${params?.dir}`;
    if (key === "inline_applyHookContext") return "Apply hook context";
    return key;
  };

  it("formats addRules suggestion", () => {
    const s: PermissionSuggestion = {
      type: "addRules",
      rules: ["Read"],
      behavior: "allow",
    };
    expect(formatSuggestionLabel(s, mockT)).toBe("Always Allow Read");
  });

  it("formats setMode suggestion", () => {
    const s: PermissionSuggestion = {
      type: "setMode",
      mode: "acceptEdits",
    };
    expect(formatSuggestionLabel(s, mockT)).toBe("Switch to acceptEdits mode");
  });

  it("formats addDirectories suggestion", () => {
    const s: PermissionSuggestion = {
      type: "addDirectories",
      directories: ["/tmp/foo"],
    };
    expect(formatSuggestionLabel(s, mockT)).toBe("Add directory: /tmp/foo");
  });

  it("formats additionalContext suggestion", () => {
    const s: PermissionSuggestion = {
      type: "additionalContext",
    };
    expect(formatSuggestionLabel(s, mockT)).toBe("Apply hook context");
  });

  it("falls back for unknown suggestion type", () => {
    const s: PermissionSuggestion = {
      type: "unknownType",
    };
    expect(formatSuggestionLabel(s, mockT)).toBe("Apply: unknownType");
  });
});

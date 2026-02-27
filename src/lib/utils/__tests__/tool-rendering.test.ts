import { describe, it, expect } from "vitest";
import {
  extractOutputText,
  extractImageBlocks,
  getLanguageFromPath,
  isImagePath,
  extractTaskToolMeta,
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

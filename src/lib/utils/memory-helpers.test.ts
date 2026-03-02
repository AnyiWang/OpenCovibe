import { describe, it, expect } from "vitest";
import { filterVisibleCandidates } from "./memory-helpers";

const FILES = [
  { path: "/project/CLAUDE.md", exists: true },
  { path: "/project/.claude/settings.json", exists: true },
  { path: "/project/.claude/AGENTS.md", exists: false },
  { path: "/project/.claude/commands/foo.md", exists: false },
];

describe("filterVisibleCandidates", () => {
  it("returns only existing files by default", () => {
    const result = filterVisibleCandidates(FILES, false, "");
    expect(result).toEqual([
      { path: "/project/CLAUDE.md", exists: true },
      { path: "/project/.claude/settings.json", exists: true },
    ]);
  });

  it("returns all files when showCreate is true", () => {
    const result = filterVisibleCandidates(FILES, true, "");
    expect(result).toEqual(FILES);
  });

  it("always includes the selected non-existing file", () => {
    const result = filterVisibleCandidates(FILES, false, "/project/.claude/AGENTS.md");
    expect(result).toEqual([
      { path: "/project/CLAUDE.md", exists: true },
      { path: "/project/.claude/settings.json", exists: true },
      { path: "/project/.claude/AGENTS.md", exists: false },
    ]);
  });
});

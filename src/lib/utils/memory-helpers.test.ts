import { describe, it, expect } from "vitest";
import { filterVisibleCandidates } from "./memory-helpers";
import type { MemoryFileCandidate } from "$lib/types";

const FILES: MemoryFileCandidate[] = [
  { path: "/project/CLAUDE.md", label: "CLAUDE.md", scope: "project", exists: true },
  {
    path: "/project/.claude/settings.json",
    label: "settings.json",
    scope: "project",
    exists: true,
  },
  { path: "/project/.claude/AGENTS.md", label: "AGENTS.md", scope: "project", exists: false },
  { path: "/project/.claude/commands/foo.md", label: "foo.md", scope: "project", exists: false },
];

describe("filterVisibleCandidates", () => {
  it("returns only existing files by default", () => {
    const result = filterVisibleCandidates(FILES, false, "");
    expect(result).toEqual([
      { path: "/project/CLAUDE.md", label: "CLAUDE.md", scope: "project", exists: true },
      {
        path: "/project/.claude/settings.json",
        label: "settings.json",
        scope: "project",
        exists: true,
      },
    ]);
  });

  it("returns all files when showCreate is true", () => {
    const result = filterVisibleCandidates(FILES, true, "");
    expect(result).toEqual(FILES);
  });

  it("always includes the selected non-existing file", () => {
    const result = filterVisibleCandidates(FILES, false, "/project/.claude/AGENTS.md");
    expect(result).toEqual([
      { path: "/project/CLAUDE.md", label: "CLAUDE.md", scope: "project", exists: true },
      {
        path: "/project/.claude/settings.json",
        label: "settings.json",
        scope: "project",
        exists: true,
      },
      { path: "/project/.claude/AGENTS.md", label: "AGENTS.md", scope: "project", exists: false },
    ]);
  });
});

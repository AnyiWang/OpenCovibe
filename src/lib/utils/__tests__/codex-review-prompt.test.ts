import { describe, it, expect } from "vitest";
import {
  CODEX_REVIEW_UNCOMMITTED_PROMPT,
  codexReviewBasePrompt,
  codexReviewCommitPrompt,
  codexReviewCustomPrompt,
} from "../codex-review-prompt";

const READONLY = "Do not modify any files";

describe("codex review prompts", () => {
  it("uncommitted prompt keeps the read-only guard", () => {
    expect(CODEX_REVIEW_UNCOMMITTED_PROMPT).toContain(READONLY);
    expect(CODEX_REVIEW_UNCOMMITTED_PROMPT).toContain("git diff");
  });

  it("base prompt diffs against the branch and is read-only", () => {
    const p = codexReviewBasePrompt("main");
    expect(p).toContain("main...HEAD");
    expect(p).toContain(READONLY);
  });

  it("commit prompt targets the sha via git show", () => {
    const p = codexReviewCommitPrompt("abc123");
    expect(p).toContain("git show abc123");
    expect(p).toContain(READONLY);
  });

  it("custom prompt includes the user instructions and the read-only framing", () => {
    const p = codexReviewCustomPrompt("focus on perf");
    expect(p).toContain("focus on perf");
    expect(p).toContain(READONLY);
  });

  it("trims whitespace in inputs", () => {
    expect(codexReviewBasePrompt("  develop  ")).toContain("develop...HEAD");
    expect(codexReviewCommitPrompt("  deadbeef  ")).toContain("git show deadbeef");
  });
});

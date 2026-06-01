// OpenCovibe-authored review prompts — Codex TUI's `/review` uses a picker UI
// rather than a prompt-injection file, so there is no upstream `prompt_for_
// review_command.md` to mirror like CODEX_INIT_PROMPT does. We mirror the four
// Codex presets (uncommitted / base branch / commit / custom) as prompts and let
// the model use its bash + read tools to gather the diff.
//
// Tone matches CODEX_INIT_PROMPT: tell the model what to run and what to report.

// Shared read-only guard + per-change report + severity grouping, appended to
// each preset's "what to inspect" section.
const REVIEW_REPORT_FRAMING = `For each change, surface:
   - A one-line summary of what it does
   - Potential bugs, edge cases, or regressions
   - Code quality / style issues worth fixing
   - Concrete suggested edits where useful

Group findings by severity (critical / important / nit) and keep feedback actionable. If the diff is very large, focus on the highest-impact files and call out that other files were skipped.`;

const REVIEW_READONLY_GUARD = `**Do not modify any files** — this is a read-only review. Only inspect and report findings; do not run apply_patch, edit, or any write command.`;

export const CODEX_REVIEW_UNCOMMITTED_PROMPT = `Review my uncommitted changes in this repository.

${REVIEW_READONLY_GUARD}

1. Enumerate what changed using **all** of:
   - \`git status --short\` for a quick overview
   - \`git diff --stat\` for file-level scope
   - \`git diff\` for the actual line-level changes to tracked files
   - \`git ls-files --others --exclude-standard\` to list untracked files; then read each untracked file directly (git diff does NOT show untracked file contents)
2. ${REVIEW_REPORT_FRAMING}

If the working tree is clean, say so plainly.
`;

/** Review the changes on the current branch against a base branch. */
export function codexReviewBasePrompt(branch: string): string {
  const b = branch.trim();
  return `Review the changes on this branch compared against base branch \`${b}\`.

${REVIEW_READONLY_GUARD}

1. Enumerate what changed using **all** of:
   - \`git diff ${b}...HEAD --stat\` for file-level scope
   - \`git diff ${b}...HEAD\` for the actual line-level changes
   - \`git log ${b}..HEAD --oneline\` for the commits being reviewed
2. ${REVIEW_REPORT_FRAMING}

If there are no differences from \`${b}\`, say so plainly.
`;
}

/** Review the changes introduced by a specific commit. */
export function codexReviewCommitPrompt(sha: string): string {
  const s = sha.trim();
  return `Review the changes introduced by commit \`${s}\`.

${REVIEW_READONLY_GUARD}

1. Enumerate what changed using **all** of:
   - \`git show ${s} --stat\` for file-level scope
   - \`git show ${s}\` for the actual line-level changes
2. ${REVIEW_REPORT_FRAMING}
`;
}

/** Review with custom user-supplied instructions, keeping the read-only framing. */
export function codexReviewCustomPrompt(instructions: string): string {
  return `${instructions.trim()}

${REVIEW_READONLY_GUARD}

Gather the relevant diff with git (\`git status\`, \`git diff\`, \`git show\`, etc.) as needed, then report findings. ${REVIEW_REPORT_FRAMING}
`;
}

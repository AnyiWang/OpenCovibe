// OpenCovibe-authored review prompt — Codex TUI's `/review` uses a picker UI
// rather than a prompt-injection file, so there is no upstream `prompt_for_
// review_command.md` to mirror like CODEX_INIT_PROMPT does.
//
// This is the "review uncommitted changes" preset only; the other three Codex
// TUI presets (review against base branch / review a specific commit / custom
// review prompt) need a picker UI and are out of scope for wave 4a.
//
// Tone matches CODEX_INIT_PROMPT: tell the model what to run and what to
// report, then let the model use its bash + read tools to gather the diff.

export const CODEX_REVIEW_UNCOMMITTED_PROMPT = `Review my uncommitted changes in this repository.

**Do not modify any files** — this is a read-only review. Only inspect and report findings; do not run apply_patch, edit, or any write command.

1. Enumerate what changed using **all** of:
   - \`git status --short\` for a quick overview
   - \`git diff --stat\` for file-level scope
   - \`git diff\` for the actual line-level changes to tracked files
   - \`git ls-files --others --exclude-standard\` to list untracked files; then read each untracked file directly (git diff does NOT show untracked file contents)
2. For each change, surface:
   - A one-line summary of what it does
   - Potential bugs, edge cases, or regressions
   - Code quality / style issues worth fixing
   - Concrete suggested edits where useful
3. Group findings by severity (critical / important / nit) and keep feedback actionable.

If the working tree is clean, say so plainly. If the diff is very large, focus on the highest-impact files and call out that other files were skipped.
`;

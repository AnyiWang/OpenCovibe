// Codex `/init` prompt — mirrors `codex-rs/tui/prompt_for_init_command.md`
// from the OpenAI Codex CLI source tree. Codex's interactive `/init` command
// submits this prompt as a user message; the model then inspects the repo
// (e.g. via `git ls-files`) and writes AGENTS.md using its file tools.
//
// OpenCovibe runs Codex in `exec` mode (non-interactive), so the CLI's own
// `/init` is unreachable — we replicate the behaviour at the app layer.
//
// Sync from upstream when Codex CLI updates this prompt. Last sync:
//   codex-main @ 2026-02 — prompt_for_init_command.md (41 lines)

export const CODEX_INIT_PROMPT = `Generate a file named AGENTS.md that serves as a contributor guide for this repository.
Your goal is to produce a clear, concise, and well-structured document with descriptive headings and actionable explanations for each section.
Follow the outline below, but adapt as needed — add sections if relevant, and omit those that do not apply to this project.

Document Requirements

- Title the document "Repository Guidelines".
- Use Markdown headings (#, ##, etc.) for structure.
- Keep the document concise. 200-400 words is optimal.
- Keep explanations short, direct, and specific to this repository.
- Provide examples where helpful (commands, directory paths, naming patterns).
- Maintain a professional, instructional tone.

Recommended Sections

Project Structure & Module Organization

- Outline the project structure, including where the source code, tests, and assets are located.

Build, Test, and Development Commands

- List key commands for building, testing, and running locally (e.g., npm test, make build).
- Briefly explain what each command does.

Coding Style & Naming Conventions

- Specify indentation rules, language-specific style preferences, and naming patterns.
- Include any formatting or linting tools used.

Testing Guidelines

- Identify testing frameworks and coverage requirements.
- State test naming conventions and how to run tests.

Commit & Pull Request Guidelines

- Summarize commit message conventions found in the project's Git history.
- Outline pull request requirements (descriptions, linked issues, screenshots, etc.).

(Optional) Add other sections if relevant, such as Security & Configuration Tips, Architecture Overview, or Agent-Specific Instructions.
`;

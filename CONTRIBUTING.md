# Contributing to OpenCovibe

Welcome! We're glad you're interested in contributing to OpenCovibe. Whether it's a bug fix, new feature, translation, or documentation improvement — every contribution helps.

## Ways to Contribute

- **Bug reports** — [Open a bug report](https://github.com/AnyiWang/OpenCovibe/issues/new?template=bug_report.yml)
- **Feature requests** — [Open a feature request](https://github.com/AnyiWang/OpenCovibe/issues/new?template=feature_request.yml)
- **Code** — Fix bugs or implement features via pull requests
- **Translations** — Help translate the UI into more languages
- **Documentation** — Improve README, guides, or inline docs

## Development Setup

```bash
git clone https://github.com/AnyiWang/OpenCovibe.git
cd OpenCovibe
npm install
npm run tauri dev
```

See [README.md](README.md) for detailed prerequisites (Rust, Node.js, Tauri CLI).

## Workflow

1. **Fork** the repository
2. **Create a branch** from `master` — use `fix/xxx` for bug fixes, `feat/xxx` for features
3. **Make your changes**
4. **Run checks** — `npm run verify` (lint + format + test + build)
5. **Commit** using [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `chore:`, etc.)
6. **Open a PR** against `master`

## Code Style

- **TypeScript** — strict mode enabled
- **Svelte 5** — use runes (`$state`, `$derived`, `$effect`, `$props()`, `$bindable()`), no legacy stores or `<slot>`
- **Comments** — only comment the *why*, not the *what*. See "Comment Standards" in `CLAUDE.md` for detailed rules
- Run `npm run fix` to auto-format and lint-fix before committing

## Testing

- Run `npm test` (Vitest)
- New features should include tests when applicable

## PR Guidelines

- Fill out the PR template completely
- Keep each PR focused on a single change
- Ensure CI passes (all checks green)

## i18n

When adding new UI text, update both locale files:

- `messages/en.json`
- `messages/zh-CN.json`

## Rust Backend

- Run `cargo clippy --manifest-path src-tauri/Cargo.toml` — zero warnings required
- Run `cargo fmt --manifest-path src-tauri/Cargo.toml` — auto-format
- Use `RUST_LOG=debug cargo tauri dev` for debugging

## Security & Conduct

- **Security vulnerabilities** — please report via [SECURITY.md](SECURITY.md) (do not open public issues)
- **Community conduct** — see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)

## License

By submitting a contribution, you agree that your work will be licensed under the [Apache License 2.0](LICENSE).

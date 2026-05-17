/**
 * Static CodeMirror language resolution.
 *
 * Pre-imports common languages so the editor never depends on dynamic chunk
 * loading for the ~30 most-used file types. Unknown extensions fall through
 * to @codemirror/language-data (async, handled by the caller).
 */

import type { Extension } from "@codemirror/state";
import { StreamLanguage } from "@codemirror/language";
import { dbgWarn } from "$lib/utils/debug";

// ── Modern language packages (tree-shaken, sync) ──
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { go } from "@codemirror/lang-go";
import { java } from "@codemirror/lang-java";
import { cpp } from "@codemirror/lang-cpp";
import { xml } from "@codemirror/lang-xml";
import { yaml } from "@codemirror/lang-yaml";
import { sql } from "@codemirror/lang-sql";
import { markdown } from "@codemirror/lang-markdown";

// ── Legacy stream-parser modes ──
import { shell as shellMode } from "@codemirror/legacy-modes/mode/shell";
import { toml as tomlMode } from "@codemirror/legacy-modes/mode/toml";
import { diff as diffMode } from "@codemirror/legacy-modes/mode/diff";

// ── Extension-based static mapping ──

const EXT_MAP: Record<string, () => Extension[]> = {
  // TypeScript
  ts: () => [javascript({ typescript: true })],
  mts: () => [javascript({ typescript: true })],
  cts: () => [javascript({ typescript: true })],
  tsx: () => [javascript({ typescript: true, jsx: true })],
  // JavaScript
  js: () => [javascript()],
  mjs: () => [javascript()],
  cjs: () => [javascript()],
  jsx: () => [javascript({ jsx: true })],
  // Data / Config
  json: () => [json()],
  jsonc: () => [json()],
  toml: () => [StreamLanguage.define(tomlMode)],
  // Markup
  md: () => [markdown()],
  markdown: () => [markdown()],
  html: () => [html()],
  htm: () => [html()],
  // Svelte / Astro / Marko: HTML-shaped with embedded <script>/<style>;
  // html() does mixed-mode parsing for these blocks. Not perfect (doesn't
  // know Svelte directives like {#if}, Astro frontmatter) but better than
  // plain text. NOTE: vue and liquid intentionally OMITTED — they have
  // real parsers in @codemirror/language-data (lang-vue, lang-liquid)
  // which the dynamic fallback path will load. Adding them here would
  // pre-empt the better parser with our HTML approximation.
  svelte: () => [html()],
  astro: () => [html()],
  marko: () => [html()],
  xml: () => [xml()],
  svg: () => [xml()],
  xsl: () => [xml()],
  // Markdown variants. mdx has embedded JSX (not parsed), qmd/rmd have
  // executable code chunks (not parsed) — all approximated as plain markdown.
  mdx: () => [markdown()],
  qmd: () => [markdown()],
  rmd: () => [markdown()],
  // Styles
  css: () => [css()],
  // Languages
  py: () => [python()],
  rs: () => [rust()],
  go: () => [go()],
  java: () => [java()],
  c: () => [cpp()],
  cpp: () => [cpp()],
  cc: () => [cpp()],
  cxx: () => [cpp()],
  h: () => [cpp()],
  hpp: () => [cpp()],
  // Config
  yaml: () => [yaml()],
  yml: () => [yaml()],
  sql: () => [sql()],
  // Shell
  sh: () => [StreamLanguage.define(shellMode)],
  bash: () => [StreamLanguage.define(shellMode)],
  zsh: () => [StreamLanguage.define(shellMode)],
  ksh: () => [StreamLanguage.define(shellMode)],
  // Diff
  diff: () => [StreamLanguage.define(diffMode)],
  patch: () => [StreamLanguage.define(diffMode)],
  // Misc config extensions (shellMode is approximation — colorizes comments
  // and strings reasonably even if it doesn't know section headers)
  env: () => [StreamLanguage.define(shellMode)],
  conf: () => [StreamLanguage.define(shellMode)],
  cfg: () => [StreamLanguage.define(shellMode)],
  ini: () => [StreamLanguage.define(shellMode)],
  properties: () => [StreamLanguage.define(shellMode)],
  editorconfig: () => [StreamLanguage.define(shellMode)],
};

/**
 * Per-extension metadata. `kind` is a stable canonical language identifier
 * (NOT the file extension), suitable for log aggregation / telemetry / UI
 * labels. `approx` marks fallback approximations.
 *
 * Every key in EXT_MAP MUST have an entry here. The completeness test in
 * __tests__/codemirror-languages.test.ts iterates EXT_MAP keys and asserts
 * each has a META entry — adding an EXT_MAP entry without META will fail CI.
 */
const EXT_META: Record<string, { kind: string; approx: boolean }> = {
  // TypeScript family
  ts: { kind: "typescript", approx: false },
  mts: { kind: "typescript", approx: false },
  cts: { kind: "typescript", approx: false },
  tsx: { kind: "typescript", approx: false },
  // JavaScript family
  js: { kind: "javascript", approx: false },
  mjs: { kind: "javascript", approx: false },
  cjs: { kind: "javascript", approx: false },
  jsx: { kind: "javascript", approx: false },
  // Markup
  html: { kind: "html", approx: false },
  htm: { kind: "html", approx: false },
  // vue / liquid: handled by dynamic language-data fallback (real parsers)
  svelte: { kind: "html", approx: true },
  astro: { kind: "html", approx: true },
  marko: { kind: "html", approx: true },
  xml: { kind: "xml", approx: false },
  svg: { kind: "xml", approx: false },
  xsl: { kind: "xml", approx: false },
  md: { kind: "markdown", approx: false },
  markdown: { kind: "markdown", approx: false },
  mdx: { kind: "markdown", approx: true },
  qmd: { kind: "markdown", approx: true },
  rmd: { kind: "markdown", approx: true },
  // Styles
  css: { kind: "css", approx: false },
  // Languages
  py: { kind: "python", approx: false },
  rs: { kind: "rust", approx: false },
  go: { kind: "go", approx: false },
  java: { kind: "java", approx: false },
  c: { kind: "c", approx: true }, // .c uses cpp() parser as approximation
  cpp: { kind: "cpp", approx: false },
  cc: { kind: "cpp", approx: false },
  cxx: { kind: "cpp", approx: false },
  h: { kind: "cpp", approx: false },
  hpp: { kind: "cpp", approx: false },
  // Data / config
  json: { kind: "json", approx: false },
  jsonc: { kind: "json", approx: false },
  yaml: { kind: "yaml", approx: false },
  yml: { kind: "yaml", approx: false },
  toml: { kind: "toml", approx: false },
  sql: { kind: "sql", approx: false },
  // Shell family
  sh: { kind: "shell", approx: false },
  bash: { kind: "shell", approx: false },
  zsh: { kind: "shell", approx: false },
  ksh: { kind: "shell", approx: false },
  // Diff
  diff: { kind: "diff", approx: false },
  patch: { kind: "diff", approx: false },
  // Misc config (shellMode is approximation)
  env: { kind: "shell", approx: true },
  conf: { kind: "shell", approx: true },
  cfg: { kind: "shell", approx: true },
  ini: { kind: "shell", approx: true },
  properties: { kind: "shell", approx: true },
  editorconfig: { kind: "shell", approx: true },
};

export interface StaticLanguageInfo {
  extensions: Extension[];
  kind: string;
  approx: boolean;
}

/**
 * Resolve static language info from a filename's **extension only**.
 * Does NOT consider bare-filename matches (Dockerfile, Makefile) — for
 * those, callers must first try `resolveStaticFilenameInfo`. CodeEditor
 * wires both in the documented order (filename → extension → dynamic →
 * first-line).
 *
 * Returns null if the extension is unknown OR if EXT_META is missing the
 * entry (intentional — the missing entry is a bug, not a fallback).
 */
export function resolveStaticExtensionInfo(name: string): StaticLanguageInfo | null {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  const factory = EXT_MAP[ext];
  if (!factory) return null;
  const meta = EXT_META[ext];
  if (!meta) {
    dbgWarn("code-editor", "static-meta-missing", { ext });
    return null;
  }
  return { extensions: factory(), kind: meta.kind, approx: meta.approx };
}

/**
 * @deprecated Do not use in new code. Kept for the test bridge against
 * `resolveStaticExtensionInfo` and as a stable name for any older import
 * sites. Semantics narrowed: extension-only — returns null for bare-filename
 * matches like ".gitignore" or "Makefile" that the old composite version
 * used to handle. New callers should use the explicit pair:
 * `resolveStaticFilenameInfo(name) ?? resolveStaticExtensionInfo(name)`.
 */
export function resolveStaticLanguage(name: string): Extension[] | null {
  return resolveStaticExtensionInfo(name)?.extensions ?? null;
}

/**
 * @internal — exported only for the completeness test in
 * __tests__/codemirror-languages.test.ts. Not a public API; do not consume
 * from production code. The naming is deliberately un-API-ish so it's clear
 * at the import site.
 */
export function __getStaticExtensionKeysForTest(): readonly string[] {
  return Object.freeze(Object.keys(EXT_MAP));
}

// ── Filename-based static mapping (dotfiles, Dockerfile, prefix variants) ──

type FilenameSpec = {
  match: (n: string) => boolean;
  load: () => Extension[];
  kind: string;
  approx: boolean;
};

function shellApproxSpec(match: (n: string) => boolean): FilenameSpec {
  return {
    match,
    load: () => [StreamLanguage.define(shellMode)],
    kind: "shell",
    approx: true,
  };
}
function jsonSpec(match: (n: string) => boolean): FilenameSpec {
  return { match, load: () => [json()], kind: "json", approx: false };
}
function tomlSpec(match: (n: string) => boolean): FilenameSpec {
  return {
    match,
    load: () => [StreamLanguage.define(tomlMode)],
    kind: "toml",
    approx: false,
  };
}

// NOTE: "Dockerfile" intentionally OMITTED — language-data has a real
// Dockerfile parser via @codemirror/legacy-modes/mode/dockerfile, which the
// dynamic fallback will load (matched by its `^Dockerfile$` regex). We only
// keep the variants (Dockerfile.dev, etc.) since dynamic doesn't cover those.
// Containerfile/Makefile/Justfile/etc. don't have language-data entries.
const SHELL_EXACT_FILENAMES = [
  "Containerfile",
  "Makefile",
  "GNUmakefile",
  "makefile",
  "Justfile",
  "justfile",
  "Earthfile",
  "Procfile",
];

const SHELL_DOTFILE_NAMES = [
  ".gitignore",
  ".dockerignore",
  ".npmignore",
  ".prettierignore",
  ".eslintignore",
  ".env",
  ".editorconfig",
];

const JSON_DOTFILE_NAMES = [".prettierrc", ".eslintrc", ".babelrc", ".swcrc"];

const FILENAMES: FilenameSpec[] = [
  // Exact bare filenames (build/container)
  ...SHELL_EXACT_FILENAMES.map((name) => shellApproxSpec((n) => n === name)),
  // Existing dotfiles (preserved from old FILENAME_MAP)
  ...SHELL_DOTFILE_NAMES.map((name) => shellApproxSpec((n) => n === name)),
  ...JSON_DOTFILE_NAMES.map((name) => jsonSpec((n) => n === name)),
  // Specific lock files
  tomlSpec((n) => n === "Cargo.lock"),
  // Prefix variants. Regex `^Name\..+$` — prefix is specific enough that we
  // don't need a strict charset; allows Dockerfile.prod+ci, Makefile.cross-arm64.
  shellApproxSpec((n) => /^(Dockerfile|Containerfile)\..+$/.test(n)),
  shellApproxSpec((n) => /^(Makefile|makefile|GNUmakefile)\..+$/.test(n)),
  shellApproxSpec((n) => /^(Justfile|justfile)\..+$/.test(n)),
  // .env.* (.env.local, .env.production)
  shellApproxSpec((n) => /^\.env\..+$/.test(n)),
];

/**
 * Resolve static language info from a bare filename or filename prefix
 * pattern (Containerfile, Makefile, Cargo.lock, .gitignore, .env.local,
 * Dockerfile.dev, ...). Does NOT fall back to extension matching; callers
 * should call `resolveStaticExtensionInfo` if this returns null.
 *
 * Exact "Dockerfile" is intentionally OMITTED — the dynamic loader has a
 * real Dockerfile parser (legacy-modes/mode/dockerfile) that's strictly
 * better than our shellMode approximation.
 */
export function resolveStaticFilenameInfo(name: string): StaticLanguageInfo | null {
  for (const spec of FILENAMES) {
    if (spec.match(name)) {
      return { extensions: spec.load(), kind: spec.kind, approx: spec.approx };
    }
  }
  return null;
}

/** Extensions-only thin wrapper for filename match. */
export function resolveStaticFilename(name: string): Extension[] | null {
  return resolveStaticFilenameInfo(name)?.extensions ?? null;
}

/**
 * Guess language from the first line of file content (shebang, XML
 * declaration, etc.). Used as a last-resort fallback for extensionless files.
 *
 * Returns `Extension[]` on match, `null` otherwise.
 */
export function resolveByFirstLine(firstLine: string): Extension[] | null {
  if (/^#!.*\b(bash|sh|zsh)\b/.test(firstLine)) return [StreamLanguage.define(shellMode)];
  if (/^#!.*\b(python|python3)\b/.test(firstLine)) return [python()];
  if (/^#!.*\bnode\b/.test(firstLine)) return [javascript()];
  if (/^<\?xml\b/.test(firstLine)) return [xml()];
  if (/^<!DOCTYPE\s+html/i.test(firstLine) || /^<html/i.test(firstLine)) return [html()];

  // HTML-like first lines — split into two patterns so the intent is
  // unambiguous and future maintainers don't accidentally widen `svelte:` to
  // arbitrary `<svelte:foo>`. Both patterns require a tag boundary
  // (whitespace, `>`, or end-of-line) to avoid matching `<scripty>` etc.
  if (/^\s*<(script|template|style|astro)(\s|>|$)/i.test(firstLine)) return [html()];
  if (/^\s*<svelte:options(\s|>|$)/i.test(firstLine)) return [html()];

  // ⚠️ Intentionally NOT matching `---`: too broad without filename context.
  //    YAML configs (no extension) often start with `---`; calling them
  //    markdown is worse than plain text. mdx/qmd/rmd already covered by
  //    EXT_MAP.
  if (/^\s*[{[]/.test(firstLine)) return [json()];
  return null;
}

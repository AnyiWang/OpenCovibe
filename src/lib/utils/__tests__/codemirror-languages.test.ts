import { describe, it, expect } from "vitest";
import {
  resolveStaticLanguage,
  resolveStaticExtensionInfo,
  resolveStaticFilename,
  resolveStaticFilenameInfo,
  resolveByFirstLine,
  __getStaticExtensionKeysForTest,
} from "../codemirror-languages";

// ──────────────────────────────────────────────────────────────────────
// Extension-based resolution
// ──────────────────────────────────────────────────────────────────────

describe("resolveStaticExtensionInfo — new framework approximations", () => {
  it.each([
    ["App.svelte", "html"],
    ["page.astro", "html"],
    ["comp.marko", "html"],
  ])("%s → html (approx)", (name, kind) => {
    const info = resolveStaticExtensionInfo(name);
    expect(info).not.toBeNull();
    expect(info!.kind).toBe(kind);
    expect(info!.approx).toBe(true);
    expect(info!.extensions.length).toBeGreaterThan(0);
    // Bridge: thin wrapper returns same-shape extensions array.
    // Can't deep-compare because StreamLanguage.define() returns fresh
    // instances per call; length + non-null is enough to catch divergence.
    expect(resolveStaticLanguage(name)?.length).toBe(info!.extensions.length);
  });

  it.each([["README.mdx"], ["paper.qmd"], ["notebook.rmd"]])(
    "%s → markdown (approx — embedded JSX/R/Python not parsed)",
    (name) => {
      const info = resolveStaticExtensionInfo(name);
      expect(info).not.toBeNull();
      expect(info!.kind).toBe("markdown");
      expect(info!.approx).toBe(true);
      // Bridge: thin wrapper returns same-shape extensions array.
      // Can't deep-compare because StreamLanguage.define() returns fresh
      // instances per call; length + non-null is enough to catch divergence.
      expect(resolveStaticLanguage(name)?.length).toBe(info!.extensions.length);
    },
  );
});

describe("resolveStaticExtensionInfo — existing parsers (regression + canonical kind)", () => {
  // Table-driven smoke test — confirms (a) no existing ext was broken and
  // (b) `kind` uses canonical language names, not extension strings.
  it.each([
    ["app.ts", "typescript", false],
    ["openai.d.ts", "typescript", false],
    ["global.d.mts", "typescript", false],
    ["app.tsx", "typescript", false],
    ["index.js", "javascript", false],
    ["index.jsx", "javascript", false],
    ["main.mjs", "javascript", false],
    ["main.cjs", "javascript", false],
    ["lib.rs", "rust", false],
    ["script.py", "python", false],
    ["main.go", "go", false],
    ["App.java", "java", false],
    ["foo.c", "c", true], // .c uses cpp() parser as approximation
    ["main.cpp", "cpp", false],
    ["util.h", "cpp", false],
    ["util.hpp", "cpp", false],
    ["index.html", "html", false],
    ["page.htm", "html", false],
    ["styles.css", "css", false],
    ["data.json", "json", false],
    ["config.yaml", "yaml", false],
    ["config.yml", "yaml", false],
    ["Cargo.toml", "toml", false],
    ["README.md", "markdown", false],
    ["data.xml", "xml", false],
    ["icon.svg", "xml", false],
    ["schema.sql", "sql", false],
    ["script.sh", "shell", false],
    ["script.bash", "shell", false],
    ["changes.diff", "diff", false],
    ["fix.patch", "diff", false],
    ["my.conf", "shell", true],
    ["settings.ini", "shell", true],
  ])("%s resolves with kind=%s approx=%s", (name, kind, approx) => {
    const info = resolveStaticExtensionInfo(name);
    expect(info, name).not.toBeNull();
    expect(info!.kind, name).toBe(kind);
    expect(info!.approx, name).toBe(approx);
    expect(resolveStaticLanguage(name)?.length, name).toBe(info!.extensions.length);
  });

  it("does NOT map .nix (intentional — shellMode too misleading)", () => {
    expect(resolveStaticExtensionInfo("default.nix")).toBeNull();
    expect(resolveStaticLanguage("default.nix")).toBeNull();
  });

  it("does NOT statically map vue/liquid (let dynamic loader use real parsers)", () => {
    // language-data has lang-vue and lang-liquid; static html() approx would pre-empt them.
    expect(resolveStaticExtensionInfo("Component.vue")).toBeNull();
    expect(resolveStaticExtensionInfo("snippet.liquid")).toBeNull();
  });

  it("returns null for unknown extensions", () => {
    expect(resolveStaticExtensionInfo("unknown.xyz")).toBeNull();
    expect(resolveStaticExtensionInfo("data.parquet")).toBeNull();
  });

  it("returns null for files without an extension (extension-only function)", () => {
    // Bare-filename matches (Dockerfile, README) belong to resolveStaticFilenameInfo.
    expect(resolveStaticExtensionInfo("README")).toBeNull();
  });

  // Exhaustive guardrail — iterates ALL EXT_MAP keys, asserts each has both
  // a working factory AND an EXT_META entry. Catches "added EXT_MAP, forgot
  // EXT_META" at PR time instead of at user-facing static-hit miss.
  it("every EXT_MAP key has working factory + EXT_META entry", () => {
    for (const key of __getStaticExtensionKeysForTest()) {
      const synthetic = `dummy.${key}`;
      const info = resolveStaticExtensionInfo(synthetic);
      expect(info, `missing META or factory for .${key}`).not.toBeNull();
      // kind allows lowercase + digits + dashes (future: vue-html, objective-c).
      expect(info!.kind, `kind for .${key}`).toMatch(/^[a-z][a-z0-9-]*$/);
      expect(info!.extensions.length, `extensions for .${key}`).toBeGreaterThan(0);
    }
  });
});

// ──────────────────────────────────────────────────────────────────────
// Filename-based resolution (Dockerfile, .gitignore, Cargo.lock, ...)
// ──────────────────────────────────────────────────────────────────────

describe("resolveStaticFilenameInfo — exact + prefix", () => {
  it.each([
    "Containerfile",
    "Makefile",
    "GNUmakefile",
    "makefile",
    "Justfile",
    "justfile",
    "Earthfile",
    "Procfile",
    ".gitignore",
    ".dockerignore",
    ".npmignore",
    ".prettierignore",
    ".eslintignore",
    ".env",
    ".editorconfig",
  ])("exact shell: %s → shell approx", (name) => {
    const info = resolveStaticFilenameInfo(name);
    expect(info, name).not.toBeNull();
    expect(info!.kind).toBe("shell");
    expect(info!.approx).toBe(true);
    expect(resolveStaticFilename(name)?.length, name).toBe(info!.extensions.length);
  });

  it.each([".prettierrc", ".eslintrc", ".babelrc", ".swcrc"])("exact json: %s → json", (name) => {
    const info = resolveStaticFilenameInfo(name);
    expect(info, name).not.toBeNull();
    expect(info!.kind).toBe("json");
    expect(info!.approx).toBe(false);
    expect(resolveStaticFilename(name)?.length, name).toBe(info!.extensions.length);
  });

  it("Cargo.lock → toml (specific lock file, not blanket .lock)", () => {
    const info = resolveStaticFilenameInfo("Cargo.lock");
    expect(info).not.toBeNull();
    expect(info!.kind).toBe("toml");
    expect(info!.approx).toBe(false);
  });

  it("package-lock.json resolves via .json extension (not filename)", () => {
    // Deliberate: filename has no entry; ext path handles it identically.
    expect(resolveStaticFilenameInfo("package-lock.json")).toBeNull();
    const info = resolveStaticExtensionInfo("package-lock.json");
    expect(info).not.toBeNull();
    expect(info!.kind).toBe("json");
  });

  it("yarn.lock has no static handler (no good parser → plain text)", () => {
    expect(resolveStaticFilenameInfo("yarn.lock")).toBeNull();
    expect(resolveStaticExtensionInfo("yarn.lock")).toBeNull();
  });

  it("Dockerfile (exact) is NOT statically mapped — dynamic loader has real parser", () => {
    // language-data matches /^Dockerfile$/ to legacy-modes/mode/dockerfile.
    // Static shellMode approx would pre-empt the better parser.
    expect(resolveStaticFilenameInfo("Dockerfile")).toBeNull();
    // But variants (Dockerfile.dev) ARE captured — dynamic doesn't cover them.
    expect(resolveStaticFilenameInfo("Dockerfile.dev")).not.toBeNull();
  });

  it.each([
    "Dockerfile.dev",
    "Dockerfile.prod",
    "Containerfile.alpine",
    "Makefile.linux",
    "Justfile.local",
    "justfile.local",
    ".env.local",
    ".env.production",
    ".env.development.local",
  ])("prefix: %s → shell approx", (name) => {
    const info = resolveStaticFilenameInfo(name);
    expect(info, name).not.toBeNull();
    expect(info!.kind).toBe("shell");
    expect(info!.approx).toBe(true);
    expect(resolveStaticFilename(name)?.length, name).toBe(info!.extensions.length);
  });

  it.each([
    "foo.svelte", // would match EXT, not filename
    "DockerfileXYZ", // no dot separator
    "dockerfile", // lowercase 'd' not in exact list
    "myDockerfile", // doesn't start with Dockerfile
  ])("does NOT match: %s", (name) => {
    expect(resolveStaticFilenameInfo(name), name).toBeNull();
    expect(resolveStaticFilename(name), name).toBeNull();
  });
});

// ──────────────────────────────────────────────────────────────────────
// First-line detection
// ──────────────────────────────────────────────────────────────────────

describe("resolveByFirstLine — shebangs, XML, HTML, JSON", () => {
  it("detects shell shebang", () => {
    expect(resolveByFirstLine("#!/bin/bash")).not.toBeNull();
    expect(resolveByFirstLine("#!/usr/bin/env sh")).not.toBeNull();
    expect(resolveByFirstLine("#!/usr/bin/env zsh")).not.toBeNull();
  });

  it("detects python shebang", () => {
    expect(resolveByFirstLine("#!/usr/bin/env python3")).not.toBeNull();
    expect(resolveByFirstLine("#!/usr/bin/python")).not.toBeNull();
  });

  it("detects node shebang", () => {
    expect(resolveByFirstLine("#!/usr/bin/env node")).not.toBeNull();
  });

  it("detects XML declaration", () => {
    expect(resolveByFirstLine('<?xml version="1.0"?>')).not.toBeNull();
  });

  it("detects HTML doctype", () => {
    expect(resolveByFirstLine("<!DOCTYPE html>")).not.toBeNull();
    expect(resolveByFirstLine("<html lang='en'>")).not.toBeNull();
  });

  it("detects JSON opening brace/bracket", () => {
    expect(resolveByFirstLine("{")).not.toBeNull();
    expect(resolveByFirstLine("[")).not.toBeNull();
    expect(resolveByFirstLine('  {"key": "value"}')).not.toBeNull();
  });

  it("returns null for unrecognized content", () => {
    expect(resolveByFirstLine("hello world")).toBeNull();
    expect(resolveByFirstLine("")).toBeNull();
    expect(resolveByFirstLine("some random text")).toBeNull();
  });
});

describe("resolveByFirstLine — HTML-like (split, strict)", () => {
  it.each([
    '<script lang="ts">',
    "<template>",
    "<style>",
    "<astro>",
    "  <script>", // leading whitespace OK
  ])("matches plain HTML-like: %s", (line) => {
    expect(resolveByFirstLine(line)).not.toBeNull();
  });

  it.each([
    "<svelte:options>",
    "<svelte:options runes={true}>",
    // ⚠️ Open tag with no closing > yet (streaming first line). REGRESSION GUARD:
    //    if someone tightens (\s|>|$) → [\s>] this case will fail and prevent
    //    silently dropping streaming-tag detection.
    "<svelte:options",
  ])("matches svelte:options: %s", (line) => {
    expect(resolveByFirstLine(line)).not.toBeNull();
  });

  it.each([
    "<scripty>", // tag boundary required
    "<scriptaculous",
    "<svelte:component>", // explicitly only svelte:options is whitelisted
    "<svelte:foo>",
  ])("does NOT match: %s", (line) => {
    expect(resolveByFirstLine(line)).toBeNull();
  });

  it("does NOT match `---` (intentional — would mis-classify YAML configs)", () => {
    expect(resolveByFirstLine("---")).toBeNull();
    expect(resolveByFirstLine("--- # yaml comment")).toBeNull();
  });
});

/**
 * Shell command syntax colorizer.
 * Converts a shell command string into HTML with <span> color tags.
 * Pure visual decoration — heuristic tokenizer, not a full shell parser.
 *
 * Graceful degradation: unrecognized syntax keeps default color, never throws.
 */

import { escapeHtml } from "./ansi";
import { dbg } from "./debug";

// Syntax token CSS classes — actual colors live in app.css (.sh-*) so they can
// switch with the light/dark theme instead of being baked into inline styles.
const C = {
  prompt: "sh-prompt", // $ prompt
  command: "sh-command", // command name
  flag: "sh-flag", // --flags, -f
  string: "sh-string", // "quoted" / 'quoted'
  operator: "sh-operator", // |, &&, ||, ;, >, >>, <, 2>&1
  assign: "sh-assign", // FOO=bar env var assignment
  arg: "sh-arg", // other arguments
} as const;

function span(cls: string, text: string): string {
  return `<span class="${cls}">${text}</span>`;
}

// Regex to tokenize shell commands into meaningful pieces
// Order matters: longer/more-specific patterns first
const TOKEN_RE = /"(?:[^"\\]|\\.)*"?|'[^']*'?|2>&1|>>|&&|\|\||[|;<>]|[^\s|;&<>"']+|\s+/g;

const OPERATOR_SET = new Set(["|", "&&", "||", ";", ">", ">>", "<", "2>&1"]);
const ENV_ASSIGN_RE = /^[A-Za-z_][A-Za-z0-9_]*=/;

/**
 * Colorize a shell command string into HTML.
 * Input is escaped for XSS safety before processing.
 */
export function colorizeCommand(command: string): string {
  dbg("shell-colorize", "colorize", { len: command.length });

  if (!command) {
    return span(C.prompt, "$") + " ";
  }

  const tokens: string[] = [];
  let m: RegExpExecArray | null;
  const re = new RegExp(TOKEN_RE.source, TOKEN_RE.flags);
  while ((m = re.exec(command)) !== null) {
    tokens.push(m[0]);
  }

  const parts: string[] = [span(C.prompt, "$"), " "];
  // Track whether next non-whitespace token should be treated as a command name
  let expectCommand = true;

  for (const raw of tokens) {
    const escaped = escapeHtml(raw);

    // Whitespace: preserve as-is
    if (/^\s+$/.test(raw)) {
      parts.push(escaped);
      continue;
    }

    // Operators: colorize and reset expectCommand
    if (OPERATOR_SET.has(raw)) {
      parts.push(span(C.operator, escaped));
      expectCommand = true;
      continue;
    }

    // Quoted strings
    if (raw.startsWith('"') || raw.startsWith("'")) {
      parts.push(span(C.string, escaped));
      if (expectCommand) expectCommand = false;
      continue;
    }

    // Flags: --flag or -f (but not bare -)
    if (/^--?\w/.test(raw)) {
      parts.push(span(C.flag, escaped));
      continue;
    }

    // Environment variable assignment (FOO=bar) — doesn't consume command slot
    if (ENV_ASSIGN_RE.test(raw) && expectCommand) {
      parts.push(span(C.assign, escaped));
      // expectCommand stays true: next non-assign token is the command
      continue;
    }

    // Command name (first non-whitespace, non-operator, non-assign token)
    if (expectCommand) {
      parts.push(span(C.command, escaped));
      expectCommand = false;
      continue;
    }

    // Default: argument
    parts.push(span(C.arg, escaped));
  }

  return parts.join("");
}

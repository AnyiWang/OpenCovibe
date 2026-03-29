// ── Path utilities (extracted from slash-commands.ts) ──

/**
 * Quote a filesystem path for safe inclusion in a CLI slash command.
 * Escapes backslashes and double-quotes, wraps in double quotes.
 * Returns null if path contains newline characters (injection risk).
 */
export function quoteCliArg(arg: string): string | null {
  if (/[\r\n]/.test(arg)) return null;
  return `"${arg.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * Normalize a directory path for dedup comparison:
 * remove trailing slash/backslash (unless root like "/" or "C:\" or "C:/").
 * Does NOT trim whitespace — directory names with leading/trailing spaces are valid.
 */
export function normalizeDirPath(p: string): string {
  if (p.length > 1 && (p.endsWith("/") || p.endsWith("\\"))) {
    const isUnixRoot = p === "/";
    const isWinRoot = /^[A-Za-z]:[/\\]$/.test(p);
    if (!isUnixRoot && !isWinRoot) {
      return p.slice(0, -1);
    }
  }
  return p;
}

/** Whether a path looks like a Windows path (drive letter prefix). */
function isWindowsPath(p: string): boolean {
  return /^[A-Za-z]:/.test(p);
}

/**
 * Compare two normalized paths for dedup.
 * Case-insensitive on Windows-style paths (drive letter prefix).
 */
export function pathsEqual(a: string, b: string): boolean {
  if (isWindowsPath(a) || isWindowsPath(b)) {
    return a.toLowerCase() === b.toLowerCase();
  }
  return a === b;
}

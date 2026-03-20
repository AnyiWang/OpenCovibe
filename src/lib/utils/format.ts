export function formatCost(cost: number): string {
  if (cost >= 1) return `$${cost.toFixed(2)}`;
  if (cost >= 0.01) return `$${cost.toFixed(4)}`;
  return `$${cost.toFixed(6)}`;
}

export function formatTokenCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
}

export { fmtRelative as relativeTime } from "$lib/i18n/format";

export function truncate(text: string, maxLen: number): string {
  if (text.length <= maxLen) return text;
  return text.slice(0, maxLen) + "\u2026";
}

/** Return a snippet of `text` centered on the first occurrence of `query`. */
export function snippetAround(text: string, query: string, maxLen: number): string {
  // Collapse whitespace (newlines, tabs) so the snippet is single-line
  const flat = text.replace(/\s+/g, " ");
  if (flat.length <= maxLen) return flat;
  const lower = flat.toLowerCase();
  const idx = lower.indexOf(query.toLowerCase());
  if (idx < 0) return flat.slice(0, maxLen) + "\u2026";
  const half = Math.floor((maxLen - query.length) / 2);
  let start = Math.max(0, idx - half);
  const end = Math.min(flat.length, start + maxLen);
  if (end - start < maxLen) start = Math.max(0, end - maxLen);
  let result = flat.slice(start, end);
  if (start > 0) result = "\u2026" + result;
  if (end < flat.length) result += "\u2026";
  return result;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Human-readable size label for a pasted text block. */
export function formatPasteSize(lines: number, chars: number): string {
  if (lines >= 1000) return `${(lines / 1000).toFixed(1)}k lines`;
  if (lines > 1) return `${lines} lines`;
  return `${chars} chars`;
}

/** Split a file path by both / and \ separators (cross-platform). */
export function splitPath(p: string): string[] {
  return p.split(/[/\\]/);
}

/** Extract filename from a path (cross-platform). */
export function fileName(p: string): string {
  return splitPath(p).pop() ?? p;
}

/** Check if a path looks like an absolute path (Unix, Windows drive, or UNC). */
export function isAbsolutePath(p: string): boolean {
  return (
    p.startsWith("/") || p.startsWith("~/") || /^[A-Za-z]:[/\\]/.test(p) || p.startsWith("\\\\")
  );
}

/** Short display label for a cwd path — last directory segment. */
export function cwdDisplayLabel(cwd: string): string {
  if (!cwd || cwd === "/") return "/";
  const parts = splitPath(cwd.replace(/[/\\]+$/, "")).filter(Boolean);
  return parts[parts.length - 1] || "/";
}

/** Format an install count with K/M suffix (e.g. 160242 → "160K"). */
export function formatInstallCount(count: number): string {
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1_000) return `${(count / 1_000).toFixed(0)}K`;
  return count.toString();
}

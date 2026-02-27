/**
 * Debug logging utility with ring buffer, tag filtering, and structured log entries.
 *
 * Enable:  localStorage.setItem('ocv:debug', '1')           → all tags
 *          localStorage.setItem('ocv:debug', 'api,bus')      → only api and bus
 *          localStorage.setItem('ocv:debug', '-replay')      → exclude replay
 * Disable: localStorage.removeItem('ocv:debug')
 * URL:     ?debug
 */

const MAX_LOG_ENTRIES = 2000;

export interface LogEntry {
  ts: string; // ISO timestamp
  tag: string;
  level: "debug" | "warn";
  args: string; // serialized args
}

const logBuffer: LogEntry[] = [];

function shouldLog(tag: string): boolean {
  if (typeof window === "undefined") return false;
  const filter = localStorage.getItem("ocv:debug") ?? "";
  if (!filter) {
    // Check URL param
    if (new URL(window.location.href).searchParams.has("debug")) return true;
    return false;
  }
  if (filter === "1") return true;
  const lowerTag = tag.toLowerCase();
  const parts = filter
    .split(",")
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean);
  const excludes = parts.filter((p) => p.startsWith("-")).map((p) => p.slice(1));
  const includes = parts.filter((p) => !p.startsWith("-"));
  if (excludes.includes(lowerTag)) return false;
  if (includes.length > 0) return includes.includes(lowerTag);
  return true;
}

let _enabled: boolean | null = null;
function enabled(): boolean {
  if (_enabled === null) {
    if (typeof window === "undefined") return false;
    const filter = localStorage.getItem("ocv:debug");
    if (filter) {
      _enabled = true; // has a filter value — enabled (shouldLog handles per-tag)
    } else {
      _enabled = new URL(window.location.href).searchParams.has("debug");
    }
  }
  return _enabled;
}

/** Reset cached enabled state (call after toggling) */
export function refreshDebugState(): void {
  _enabled = null;
}

function ts(): string {
  return new Date().toISOString().slice(11, 23); // HH:mm:ss.SSS
}

function serializeArgs(args: unknown[]): string {
  return args.map((a) => (typeof a === "object" ? JSON.stringify(a) : String(a))).join(" ");
}

/** Debug log — only outputs when debug mode is enabled and tag passes filter */
export function dbg(tag: string, ...args: unknown[]): void {
  if (!enabled() || !shouldLog(tag)) return;
  const timestamp = ts();
  const serialized = serializeArgs(args);
  const entry: LogEntry = { ts: timestamp, tag, level: "debug", args: serialized };
  logBuffer.push(entry);
  if (logBuffer.length > MAX_LOG_ENTRIES) logBuffer.shift();
  console.debug(`%c[ocv:${tag}]`, "color:#6cf", ...args);
}

/** Always log, regardless of debug mode (for errors/warnings) */
export function dbgWarn(tag: string, ...args: unknown[]): void {
  const timestamp = ts();
  const serialized = serializeArgs(args);
  const entry: LogEntry = { ts: timestamp, tag, level: "warn", args: serialized };
  logBuffer.push(entry);
  if (logBuffer.length > MAX_LOG_ENTRIES) logBuffer.shift();
  console.warn(`[ocv:${tag}]`, ...args);
}

/** Get all buffered log entries as a single string */
export function getDebugLogs(): string {
  return logBuffer
    .map((e) => `[${e.ts}] [${e.level === "warn" ? "WARN:" : ""}${e.tag}] ${e.args}`)
    .join("\n");
}

/** Get structured log entries as JSON array */
export function getDebugLogsJSON(): LogEntry[] {
  return [...logBuffer];
}

/** Copy debug logs to clipboard, returns success */
export async function copyDebugLogs(): Promise<boolean> {
  try {
    const logs = getDebugLogs();
    if (!logs) return false;
    await navigator.clipboard.writeText(logs);
    return true;
  } catch {
    return false;
  }
}

/** Clear buffered logs */
export function clearDebugLogs(): void {
  logBuffer.length = 0;
}

/** Get buffer size */
export function getDebugLogCount(): number {
  return logBuffer.length;
}

/** Redact sensitive field values in an object for safe logging. */
const SENSITIVE_FIELDS = new Set(["apiKey", "api_key", "anthropic_api_key", "primaryApiKey"]);

export function redactSensitive(obj: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    out[k] = SENSITIVE_FIELDS.has(k) && typeof v === "string" ? "***" : v;
  }
  return out;
}

/** Check if debug mode is currently on */
export function isDebugMode(): boolean {
  return enabled();
}

/** Get current debug filter string */
export function getDebugFilter(): string {
  if (typeof window === "undefined") return "";
  return localStorage.getItem("ocv:debug") ?? "";
}

/** Toggle debug mode on/off, or set a tag filter */
export function setDebugMode(on: boolean | string): void {
  if (typeof on === "string") {
    localStorage.setItem("ocv:debug", on);
  } else if (on) {
    localStorage.setItem("ocv:debug", "1");
  } else {
    localStorage.removeItem("ocv:debug");
  }
  refreshDebugState();
}

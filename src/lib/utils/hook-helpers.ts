/**
 * Hook management helpers.
 *
 * Core principle: **writes go through raw JSON deep-clone, normalize is UI-only**.
 */

type Rec = Record<string, any>;

/** All known Claude Code hook event types. */
export const HOOK_EVENT_TYPES = [
  "PreToolUse",
  "PostToolUse",
  "Notification",
  "Stop",
  "SubagentStop",
  "SubagentTool",
] as const;

export type HookEventType = (typeof HOOK_EVENT_TYPES)[number];

// ── Raw-based write helpers ──

/** Ensure raw hooks is a plain object; fallback to {} for null/array/primitive. */
export function ensureHooksObject(raw: unknown): Rec {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  return raw as Rec;
}

/** Deep-clone raw hooks → push a new group to `event`. */
export function addGroup(raw: unknown, event: string, group: unknown): Rec {
  const h = structuredClone(ensureHooksObject(raw));
  if (!Array.isArray(h[event])) h[event] = [];
  (h[event] as unknown[]).push(group);
  return h;
}

/** Deep-clone raw hooks → splice group at `index` from `event`. Deletes key if empty. */
export function removeGroup(raw: unknown, event: string, index: number): Rec {
  const h = structuredClone(ensureHooksObject(raw));
  if (!Array.isArray(h[event])) return h;
  (h[event] as unknown[]).splice(index, 1);
  if ((h[event] as unknown[]).length === 0) delete h[event];
  return h;
}

/** Deep-clone raw hooks → merge `patch` into group at `index`. Preserves unknown fields via spread. */
export function patchGroup(raw: unknown, event: string, index: number, patch: Rec): Rec {
  const h = structuredClone(ensureHooksObject(raw));
  if (!Array.isArray(h[event])) return h;
  const arr = h[event] as unknown[];
  if (index >= arr.length) return h;
  const original = arr[index];
  // Non-plain-object group → replace entirely
  if (!original || typeof original !== "object" || Array.isArray(original)) {
    arr[index] = patch;
  } else {
    arr[index] = { ...(original as Rec), ...patch };
  }
  return h;
}

// ── Display-only normalize ──

/** Normalise raw hooks for UI rendering: keep only keys whose value is an array. */
export function normalizeForDisplay(raw: unknown): Record<string, unknown[]> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const result: Record<string, unknown[]> = {};
  for (const [key, val] of Object.entries(raw as Rec)) {
    if (Array.isArray(val)) result[key] = val;
  }
  return result;
}

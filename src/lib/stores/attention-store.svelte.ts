/**
 * Attention Store: tracks which runs need user attention (permission prompt, ask pending).
 *
 * Multi-reason bitset per run — each reason is independently marked/cleared.
 * Read by sidebar components to show "waiting" instead of "running".
 */
import { dbg } from "$lib/utils/debug";

interface AttentionFlags {
  permission: boolean;
  ask: boolean;
}

const attention = $state<Record<string, AttentionFlags>>({});

export function markAttention(runId: string, reason: "permission" | "ask"): void {
  if (!attention[runId]) {
    attention[runId] = { permission: false, ask: false };
  }
  attention[runId][reason] = true;
  dbg("attention", "mark", { runId, reason });
}

export function clearAttention(runId: string, reason?: "permission" | "ask"): void {
  const flags = attention[runId];
  if (!flags) return;
  if (reason) {
    flags[reason] = false;
    dbg("attention", "clear", { runId, reason });
    if (!flags.permission && !flags.ask) {
      delete attention[runId];
    }
  } else {
    dbg("attention", "clear-all", { runId });
    delete attention[runId];
  }
}

export function hasAttention(runId: string): boolean {
  const flags = attention[runId];
  return !!flags && (flags.permission || flags.ask);
}

/** @internal test-only */
export function _resetForTest(): void {
  for (const key of Object.keys(attention)) {
    delete attention[key];
  }
}

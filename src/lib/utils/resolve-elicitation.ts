/**
 * Optimistic elicitation cleanup — called by UI immediately after responding.
 * Removes the elicitation from the store and clears attention if no pending items remain.
 */
import type { SessionStore } from "$lib/stores/session-store.svelte";
import { clearAttention } from "$lib/stores/attention-store.svelte";

export function resolveElicitationOptimistic(
  store: SessionStore,
  runId: string,
  requestId: string,
): void {
  store.removeElicitation(requestId);
  // Only clear attention when no remaining elicitations or permissions
  if (!store.hasElicitation && !store.hasPendingPermission) {
    clearAttention(runId, "permission");
  }
}

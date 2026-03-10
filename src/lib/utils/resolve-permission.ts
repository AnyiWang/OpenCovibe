/**
 * Optimistic permission resolve + attention clear helper.
 *
 * Extracted from +page.svelte so it's independently testable.
 */
import { clearAttention } from "$lib/stores/attention-store.svelte";
import { dbg } from "$lib/utils/debug";

interface PermissionResolver {
  resolvePermissionAllow(requestId: string): void;
  resolvePermissionDeny(requestId: string): void;
}

/** Optimistic resolve + clear attention after permission respond IPC. */
export function resolvePermissionOptimistic(
  store: PermissionResolver,
  runId: string,
  requestId: string,
  behavior: "allow" | "deny",
): void {
  if (behavior === "deny") {
    store.resolvePermissionDeny(requestId);
  }
  if (behavior === "allow") {
    store.resolvePermissionAllow(requestId);
  }
  clearAttention(runId, "permission");
  // Deny may also need to clear ask: if an AskUserQuestion permission was denied,
  // tool_end(error) arrives first and marks ask before this optimistic clear.
  if (behavior === "deny") {
    clearAttention(runId, "ask");
  }
  dbg("attention", "optimistic-clear", { runId, requestId, behavior });
}

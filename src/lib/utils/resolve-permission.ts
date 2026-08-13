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
  pendingPermissionModeOverride: string | null;
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

/** Resolve the permission card only after the backend confirms primary stdin delivery. */
export async function resolvePermissionAfterDelivery(
  store: PermissionResolver,
  runId: string,
  requestId: string,
  behavior: "allow" | "deny",
  deliver: () => Promise<void>,
  modeOverride?: string,
): Promise<void> {
  const previousModeOverride = store.pendingPermissionModeOverride;
  if (modeOverride) {
    // Stage before invoking IPC because the resulting tool_end event can race its reply.
    store.pendingPermissionModeOverride = modeOverride;
    dbg("permission", "mode-override-staged", { requestId, modeOverride });
  }
  try {
    await deliver();
  } catch (error) {
    // Do not overwrite a newer response that changed the staged value while IPC was pending.
    if (modeOverride && store.pendingPermissionModeOverride === modeOverride) {
      store.pendingPermissionModeOverride = previousModeOverride;
      dbg("permission", "mode-override-rolled-back", { requestId, modeOverride });
    }
    throw error;
  }
  resolvePermissionOptimistic(store, runId, requestId, behavior);
}

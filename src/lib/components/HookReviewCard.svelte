<script lang="ts">
  import { dbg } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let {
    hookEvent,
    onRespond,
  }: {
    hookEvent: {
      type: string;
      hook_id: string;
      data: unknown;
      request_id?: string;
      status?: string;
    };
    onRespond: (requestId: string, decision: "allow" | "deny") => Promise<void>;
  } = $props();

  let submitting = $state(false);

  // Extract display info from hook data
  let data = $derived(hookEvent.data as Record<string, unknown>);
  let hookName = $derived(
    (data as { hook_name?: string }).hook_name ??
      (data as { tool_name?: string }).tool_name ??
      "Unknown Tool",
  );
  let hookEventType = $derived((data as { hook_event?: string }).hook_event ?? hookEvent.type);

  async function handleRespond(decision: "allow" | "deny") {
    if (!hookEvent.request_id || submitting) return;
    submitting = true;
    dbg("hook-review", "respond", { requestId: hookEvent.request_id, decision });
    try {
      await onRespond(hookEvent.request_id, decision);
    } catch {
      submitting = false;
    }
  }
</script>

{#if hookEvent.status === "hook_pending" && hookEvent.request_id}
  <div class="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 my-2">
    <div class="flex items-center gap-2 mb-2">
      <div class="h-2 w-2 rounded-full bg-amber-500 animate-pulse"></div>
      <span class="text-sm font-medium text-amber-600 dark:text-amber-400"
        >{t("hook_reviewTitle", { type: hookEventType })}</span
      >
    </div>
    <p class="text-xs text-muted-foreground mb-2">
      {t("hook_tool", { name: hookName })}
    </p>
    <div class="flex gap-2">
      <button
        class="rounded-md border border-green-500/30 bg-green-500/10 px-3 py-1.5 text-xs font-medium text-green-600 dark:text-green-400 hover:bg-green-500/20 transition-all disabled:opacity-50"
        disabled={submitting}
        onclick={() => handleRespond("allow")}>{t("common_allow")}</button
      >
      <button
        class="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-1.5 text-xs font-medium text-red-600 dark:text-red-400 hover:bg-red-500/20 transition-all disabled:opacity-50"
        disabled={submitting}
        onclick={() => handleRespond("deny")}>{t("common_deny")}</button
      >
    </div>
  </div>
{:else if hookEvent.status === "responding"}
  <div class="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 my-2 text-xs text-amber-500">
    {t("interaction_responsePending")}
  </div>
{:else if hookEvent.status === "error"}
  <div class="rounded-lg border border-red-500/30 bg-red-500/5 p-3 my-2 text-xs text-red-500">
    {t("interaction_responseFailed")}
  </div>
{/if}

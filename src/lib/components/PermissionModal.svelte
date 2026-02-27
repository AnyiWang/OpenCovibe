<script lang="ts">
  import type { ToolRequest } from "$lib/types";
  import * as api from "$lib/api";
  import { dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let {
    request,
    onResolved,
  }: {
    request: ToolRequest | null;
    onResolved: () => void;
  } = $props();

  let resolving = $state(false);

  async function handleDecision(decision: "allow" | "deny") {
    if (!request || resolving) return;
    resolving = true;
    try {
      await api.resolvePermission(request.request_id, decision);
    } catch (e) {
      dbgWarn("perm", "failed to resolve permission:", e);
    } finally {
      resolving = false;
      onResolved();
    }
  }

  // Tool display info
  let toolLabel = $derived(request?.tool_name ?? "Unknown");
  let isWrite = $derived(
    request?.tool_name === "write_file" ||
      request?.tool_name === "edit_file" ||
      request?.tool_name === "bash",
  );

  let detail = $derived.by(() => {
    if (!request) return "";
    const input = request.input;
    if (request.tool_name === "bash") {
      return ((input as Record<string, unknown>).command as string) ?? "";
    }
    if (
      request.tool_name === "write_file" ||
      request.tool_name === "edit_file" ||
      request.tool_name === "read_file"
    ) {
      return ((input as Record<string, unknown>).path as string) ?? "";
    }
    return JSON.stringify(input, null, 2);
  });
</script>

{#if request}
  <!-- Backdrop -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm animate-fade-in"
  >
    <div
      class="mx-4 w-full max-w-md rounded-xl border border-border bg-background shadow-2xl animate-slide-up"
    >
      <!-- Header -->
      <div class="flex items-center gap-3 border-b border-border px-5 py-4">
        <div
          class="flex h-9 w-9 items-center justify-center rounded-lg {isWrite
            ? 'bg-amber-500/10'
            : 'bg-blue-500/10'}"
        >
          {#if isWrite}
            <svg
              class="h-5 w-5 text-amber-600 dark:text-amber-400"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path
                d="M12 9v4M12 17h.01M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"
              />
            </svg>
          {:else}
            <svg
              class="h-5 w-5 text-blue-500 dark:text-blue-400"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <circle cx="12" cy="12" r="10" /><line x1="12" y1="16" x2="12" y2="12" /><line
                x1="12"
                y1="8"
                x2="12.01"
                y2="8"
              />
            </svg>
          {/if}
        </div>
        <div>
          <h3 class="text-sm font-semibold text-foreground">{t("perm_title")}</h3>
          <p class="text-xs text-muted-foreground">
            {t("perm_agentWantsToUse")} <span class="font-medium text-foreground">{toolLabel}</span>
          </p>
        </div>
      </div>

      <!-- Content -->
      <div class="px-5 py-4">
        {#if detail}
          <div class="rounded-lg bg-muted p-3 max-h-48 overflow-y-auto">
            <pre
              class="text-xs font-mono text-muted-foreground whitespace-pre-wrap break-all">{detail}</pre>
          </div>
        {/if}

        {#if request.tool_name === "bash"}
          <p class="mt-3 text-xs text-amber-600 dark:text-amber-400/80">
            {t("perm_shellWarning")}
          </p>
        {/if}
      </div>

      <!-- Actions -->
      <div class="flex gap-3 border-t border-border px-5 py-4">
        <button
          class="flex-1 rounded-lg border border-border px-4 py-2.5 text-sm font-medium text-foreground hover:bg-accent transition-colors"
          onclick={() => handleDecision("deny")}
          disabled={resolving}
        >
          {t("common_deny")}
        </button>
        <button
          class="flex-1 rounded-lg bg-emerald-600 px-4 py-2.5 text-sm font-medium text-white hover:bg-emerald-500 transition-colors"
          onclick={() => handleDecision("allow")}
          disabled={resolving}
        >
          {t("common_allow")}
        </button>
      </div>
    </div>
  </div>
{/if}

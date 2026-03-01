<script lang="ts">
  import type { McpServerInfo } from "$lib/types";
  import * as api from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import { statusDotClass, statusLabel, parseServersFromResponse } from "$lib/utils/mcp";

  let {
    runId,
    mcpServers,
    sessionAlive = false,
    onClose,
    onServersUpdate,
  }: {
    runId: string;
    mcpServers: McpServerInfo[];
    sessionAlive?: boolean;
    onClose: () => void;
    onServersUpdate?: (servers: McpServerInfo[]) => void;
  } = $props();

  let loading = $state(false);
  let togglingServer = $state<string | null>(null);
  let servers = $state<McpServerInfo[]>([]);
  let error = $state("");
  let successMsg = $state("");

  // Sync from prop when it changes
  $effect(() => {
    servers = [...mcpServers];
  });

  async function refresh() {
    if (!sessionAlive) return;
    loading = true;
    error = "";
    try {
      dbg("mcp", "refresh", { runId });
      const response = await api.getMcpStatus(runId);
      const updated = parseServersFromResponse(response);
      if (updated.length > 0) {
        servers = updated;
        onServersUpdate?.(updated);
      }
    } catch (e) {
      dbgWarn("mcp", "refresh failed", e);
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function reconnect(serverName: string) {
    if (!sessionAlive) return;
    loading = true;
    error = "";
    try {
      dbg("mcp", "reconnect", { runId, serverName });
      await api.reconnectMcpServer(runId, serverName);
      await refresh();
    } catch (e) {
      dbgWarn("mcp", "reconnect failed", e);
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function toggle(serverName: string, currentlyEnabled: boolean) {
    togglingServer = serverName;
    error = "";
    successMsg = "";
    const newEnabled = !currentlyEnabled;

    // Find server scope (default to "user")
    const scope = "user";

    try {
      dbg("mcp", "toggle via config", { serverName, enabled: newEnabled, scope });
      const result = await api.toggleMcpServerConfig(serverName, newEnabled, scope);
      if (result.success) {
        // Update local state immediately
        servers = servers.map((s) =>
          s.name === serverName
            ? { ...s, status: newEnabled ? "pending" : "disabled" }
            : s,
        );
        onServersUpdate?.(servers);
        successMsg = result.message;
        setTimeout(() => (successMsg = ""), 3000);
      } else {
        error = result.message;
      }
    } catch (e) {
      dbgWarn("mcp", "toggle config failed", e);
      error = String(e);
    } finally {
      togglingServer = null;
    }
  }
</script>

<div class="rounded-lg border border-border bg-background shadow-lg w-80 animate-fade-in">
  <!-- Header -->
  <div class="flex items-center justify-between px-3 py-2 border-b border-border">
    <span class="text-xs font-semibold text-foreground">{t("mcp_serversTitle")}</span>
    <div class="flex items-center gap-1">
      <button
        class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors disabled:opacity-50"
        disabled={loading || !sessionAlive}
        onclick={refresh}
        title={t("mcp_refreshStatus")}
      >
        <svg
          class="h-3.5 w-3.5 {loading ? 'animate-spin' : ''}"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
          <path d="M3 3v5h5" />
          <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
          <path d="M16 16h5v5" />
        </svg>
      </button>
      <button
        class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
        onclick={onClose}
        title={t("common_close")}
      >
        <svg
          class="h-3.5 w-3.5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M18 6 6 18" /><path d="m6 6 12 12" />
        </svg>
      </button>
    </div>
  </div>

  <!-- Server list -->
  <div class="max-h-64 overflow-y-auto">
    {#if servers.length === 0}
      <div class="px-3 py-4 text-center text-xs text-muted-foreground">
        {t("mcp_noConfiguredStatus")}
      </div>
    {:else}
      {#each servers as server}
        <div class="flex items-center gap-2 px-3 py-2 border-b border-border/50 last:border-b-0">
          <!-- Status dot -->
          <span class="h-2 w-2 shrink-0 rounded-full {statusDotClass(server.status)}"></span>

          <!-- Name + status -->
          <div class="flex-1 min-w-0">
            <div class="text-xs font-medium text-foreground truncate">{server.name}</div>
            <div class="text-[10px] text-muted-foreground">{statusLabel(server.status)}</div>
            {#if server.error}
              <div class="text-[10px] text-destructive truncate" title={server.error}>
                {server.error}
              </div>
            {/if}
          </div>

          <!-- Actions -->
          <div class="flex items-center gap-1 shrink-0">
            {#if sessionAlive && (server.status === "failed" || server.status === "needs-auth")}
              <button
                class="rounded px-1.5 py-0.5 text-[10px] font-medium text-foreground/70 hover:text-foreground hover:bg-accent border border-border/50 transition-colors disabled:opacity-50"
                disabled={loading}
                onclick={() => reconnect(server.name)}>{t("mcp_reconnect")}</button
              >
            {/if}
            <button
              class="rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors disabled:opacity-50 {server.status ===
              'disabled'
                ? 'text-emerald-600 dark:text-emerald-400 hover:bg-emerald-500/10 border border-emerald-500/30'
                : 'text-foreground/70 hover:text-foreground hover:bg-accent border border-border/50'}"
              disabled={togglingServer === server.name}
              onclick={() => toggle(server.name, server.status !== "disabled")}
            >
              {#if togglingServer === server.name}
                <span class="flex items-center gap-1">
                  <span class="h-2.5 w-2.5 border border-current/30 border-t-current rounded-full animate-spin"></span>
                </span>
              {:else}
                {server.status === "disabled" ? t("mcp_enable") : t("mcp_disable")}
              {/if}
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  <!-- Success message -->
  {#if successMsg}
    <div class="px-3 py-2 border-t border-emerald-500/20 bg-emerald-500/5 text-xs text-emerald-600 dark:text-emerald-400">
      {successMsg}
    </div>
  {/if}

  <!-- Error -->
  {#if error}
    <div class="px-3 py-2 border-t border-destructive/20 bg-destructive/5 text-xs text-destructive">
      {error}
    </div>
  {/if}

  <!-- Footer note -->
  {#if !sessionAlive && servers.length > 0}
    <div class="px-3 py-2 border-t border-border/50 text-[10px] text-muted-foreground">
      {t("mcp_sessionInactive")}
    </div>
  {/if}
</div>

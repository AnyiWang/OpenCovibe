<script lang="ts">
  import { listConfiguredMcpServers, removeMcpServer } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import type { ConfiguredMcpServer } from "$lib/types";

  let {
    projectCwd = "",
    visible = false,
    operationLoading = $bindable<string | null>(null),
    showToast,
    confirmAction = $bindable<{
      title: string;
      message: string;
      onConfirm: () => void;
    } | null>(null),
  }: {
    projectCwd: string;
    visible?: boolean;
    operationLoading: string | null;
    showToast: (message: string, type: "success" | "error") => void;
    confirmAction: {
      title: string;
      message: string;
      onConfirm: () => void;
    } | null;
  } = $props();

  // ── State ──
  let servers = $state<ConfiguredMcpServer[]>([]);
  let loading = $state(true);
  let selectedServer = $state<ConfiguredMcpServer | null>(null);

  // ── Init — reload when tab becomes visible ──

  $effect(() => {
    if (visible) {
      loadServers();
    }
  });

  async function loadServers() {
    loading = true;
    try {
      servers = await listConfiguredMcpServers(projectCwd || undefined);
      dbg("mcp-configured", "loaded", { count: servers.length });
    } catch (e) {
      dbgWarn("mcp-configured", "load error", e);
      servers = [];
    } finally {
      loading = false;
    }
  }

  async function refreshServers() {
    try {
      servers = await listConfiguredMcpServers(projectCwd || undefined);
    } catch (e) {
      dbgWarn("mcp-configured", "refresh error", e);
    }
  }

  function handleRemove(server: ConfiguredMcpServer) {
    confirmAction = {
      title: t("mcp_removeTitle"),
      message: t("mcp_removeConfirm", { name: server.name, scope: server.scope }),
      onConfirm: async () => {
        operationLoading = server.name;
        try {
          const result = await removeMcpServer(server.name, server.scope, projectCwd || undefined);
          showToast(
            result.success ? t("mcp_removedServer", { name: server.name }) : result.message,
            result.success ? "success" : "error",
          );
          if (result.success) {
            if (selectedServer?.name === server.name) {
              selectedServer = null;
            }
            await refreshServers();
          }
          dbg("mcp-configured", "remove result", result);
        } catch (e) {
          showToast(t("mcp_errorGeneric", { error: String(e) }), "error");
        } finally {
          operationLoading = null;
        }
      },
    };
  }

  function typeBadgeColor(serverType: string): string {
    switch (serverType) {
      case "stdio":
        return "bg-blue-500/10 text-blue-600 dark:text-blue-400";
      case "http":
        return "bg-teal-500/10 text-teal-600 dark:text-teal-400";
      case "sse":
        return "bg-purple-500/10 text-purple-600 dark:text-purple-400";
      default:
        return "bg-muted text-muted-foreground";
    }
  }

  function scopeBadgeColor(scope: string): string {
    switch (scope) {
      case "local":
        return "bg-amber-500/10 text-amber-600 dark:text-amber-400";
      case "user":
        return "bg-muted text-muted-foreground";
      case "project":
        return "bg-blue-500/10 text-blue-600 dark:text-blue-400";
      default:
        return "bg-muted text-muted-foreground";
    }
  }
</script>

{#if loading}
  <div class="flex items-center justify-center py-8">
    <div
      class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
    ></div>
    <span class="ml-2 text-xs text-muted-foreground">{t("mcp_loadingConfigured")}</span>
  </div>
{:else if servers.length === 0}
  <div class="flex flex-col items-center justify-center py-12 text-center">
    <div
      class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border border-border bg-muted"
    >
      <svg
        class="h-6 w-6 text-muted-foreground"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><rect width="20" height="8" x="2" y="2" rx="2" ry="2" /><rect
          width="20"
          height="8"
          x="2"
          y="14"
          rx="2"
          ry="2"
        /><line x1="6" x2="6.01" y1="6" y2="6" /><line x1="6" x2="6.01" y1="18" y2="18" /></svg
      >
    </div>
    <h2 class="text-sm font-medium text-foreground mb-1">{t("mcp_noConfigured")}</h2>
    <p class="text-xs text-muted-foreground max-w-sm">
      {t("mcp_useDiscoverTab")}
    </p>
  </div>
{:else}
  <div class="flex gap-3" style="height: calc(100vh - 300px); min-height: 300px;">
    <!-- Left: scrollable server list -->
    <div class="w-[280px] shrink-0 overflow-y-auto space-y-1.5 pr-1">
      {#each servers as server}
        <div
          class="w-full text-left rounded-lg border px-3 py-2 transition-colors cursor-pointer {selectedServer?.name ===
            server.name && selectedServer?.scope === server.scope
            ? 'border-primary/50 bg-primary/5'
            : 'border-border/50 bg-muted/30 hover:bg-muted/50'}"
          onclick={() => (selectedServer = server)}
          onkeydown={(e) => {
            if (e.key === "Enter") selectedServer = server;
          }}
          role="button"
          tabindex="0"
        >
          <div class="flex items-center justify-between gap-2">
            <div class="flex-1 min-w-0">
              <span class="text-sm font-medium text-foreground truncate block">{server.name}</span>
              <div class="flex items-center gap-1.5 mt-0.5">
                <span
                  class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {typeBadgeColor(
                    server.server_type,
                  )}"
                >
                  {server.server_type}
                </span>
                <span
                  class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {scopeBadgeColor(
                    server.scope,
                  )}"
                >
                  {server.scope}
                </span>
              </div>
            </div>
            <button
              class="shrink-0 rounded p-1 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors disabled:opacity-50"
              onclick={(e) => {
                e.stopPropagation();
                handleRemove(server);
              }}
              title={t("mcp_removeServerTooltip")}
              disabled={operationLoading === server.name}
            >
              {#if operationLoading === server.name}
                <div
                  class="h-3.5 w-3.5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                ></div>
              {:else}
                <svg
                  class="h-3.5 w-3.5"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><path d="M3 6h18" /><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" /><path
                    d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"
                  /></svg
                >
              {/if}
            </button>
          </div>
        </div>
      {/each}
    </div>

    <!-- Right: detail panel -->
    <div class="flex-1 min-w-0 overflow-y-auto">
      {#if selectedServer}
        <div class="rounded-lg border border-border/50 bg-muted/20 p-4 space-y-3">
          <!-- Header -->
          <div class="flex items-start justify-between gap-2">
            <div class="flex-1 min-w-0">
              <h3 class="text-sm font-semibold text-foreground">{selectedServer.name}</h3>
              <div class="flex items-center gap-1.5 mt-1">
                <span
                  class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {typeBadgeColor(
                    selectedServer.server_type,
                  )}"
                >
                  {selectedServer.server_type}
                </span>
                <span
                  class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {scopeBadgeColor(
                    selectedServer.scope,
                  )}"
                >
                  {selectedServer.scope}
                </span>
              </div>
            </div>
            <button
              class="shrink-0 text-muted-foreground hover:text-foreground"
              onclick={() => (selectedServer = null)}
              title={t("common_close")}
            >
              <svg
                class="h-3.5 w-3.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
              >
            </button>
          </div>

          <!-- Command + args (stdio) -->
          {#if selectedServer.command}
            <div class="border-t border-border pt-3">
              <div class="text-[11px] font-medium text-muted-foreground mb-1">
                {t("mcp_command")}
              </div>
              <div class="rounded-md bg-muted/40 px-3 py-2 font-mono text-xs text-foreground">
                {selectedServer.command}{#if selectedServer.args?.length > 0}{" " +
                    selectedServer.args.join(" ")}{/if}
              </div>
            </div>
          {/if}

          <!-- URL (http/sse) -->
          {#if selectedServer.url}
            <div class="border-t border-border pt-3">
              <div class="text-[11px] font-medium text-muted-foreground mb-1">{t("mcp_url")}</div>
              <div
                class="rounded-md bg-muted/40 px-3 py-2 font-mono text-xs text-foreground truncate"
              >
                {selectedServer.url}
              </div>
            </div>
          {/if}

          <!-- Env keys -->
          {#if selectedServer.env_keys?.length > 0}
            <div class="border-t border-border pt-3">
              <div class="text-[11px] font-medium text-muted-foreground mb-1">
                {t("mcp_envVars")}
              </div>
              <div class="flex flex-wrap gap-1.5">
                {#each selectedServer.env_keys as key}
                  <span
                    class="rounded-md bg-muted/40 px-2 py-1 font-mono text-[10px] text-foreground"
                    >{key}</span
                  >
                {/each}
              </div>
            </div>
          {/if}

          <!-- Header keys -->
          {#if selectedServer.header_keys?.length > 0}
            <div class="border-t border-border pt-3">
              <div class="text-[11px] font-medium text-muted-foreground mb-1">
                {t("mcp_headers")}
              </div>
              <div class="flex flex-wrap gap-1.5">
                {#each selectedServer.header_keys as key}
                  <span
                    class="rounded-md bg-muted/40 px-2 py-1 font-mono text-[10px] text-foreground"
                    >{key}</span
                  >
                {/each}
              </div>
            </div>
          {/if}

          <!-- Remove button -->
          <div class="border-t border-border pt-3">
            <button
              class="rounded-md border border-destructive/30 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 transition-colors disabled:opacity-50"
              onclick={() => handleRemove(selectedServer!)}
              disabled={operationLoading === selectedServer.name}
            >
              {operationLoading === selectedServer.name ? t("mcp_removing") : t("mcp_removeServer")}
            </button>
          </div>
        </div>
      {:else}
        <div
          class="rounded-lg border border-dashed border-border/50 p-6 flex items-center justify-center h-full"
        >
          <p class="text-xs text-muted-foreground">{t("mcp_selectServerDetails")}</p>
        </div>
      {/if}
    </div>
  </div>
{/if}

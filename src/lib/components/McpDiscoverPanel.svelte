<script lang="ts">
  import {
    checkMcpRegistryHealth,
    searchMcpRegistry,
    addMcpServer,
    listConfiguredMcpServers,
  } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import type {
    ProviderHealth,
    McpRegistryServer,
    PluginOperationResult,
    ConfiguredMcpServer,
  } from "$lib/types";

  let {
    projectCwd = "",
    visible = false,
    operationLoading = $bindable<string | null>(null),
    showToast,
  }: {
    projectCwd: string;
    visible?: boolean;
    operationLoading: string | null;
    showToast: (message: string, type: "success" | "error") => void;
  } = $props();

  // ── State ──
  let registryHealth = $state<ProviderHealth | null>(null);
  let query = $state("");
  let results = $state<McpRegistryServer[]>([]);
  let popularResults = $state<McpRegistryServer[]>([]);
  let searching = $state(false);
  let refreshing = $state(false);
  let detail = $state<McpRegistryServer | null>(null);

  // Installed state — full configured server list for matching
  let installedServers = $state<ConfiguredMcpServer[]>([]);

  // Install form
  let installScope = $state<"local" | "user" | "project">("user");
  let envValues = $state<Record<string, string>>({});
  let headerValues = $state<Record<string, string>>({});

  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;

  let displayResults = $derived(query.trim().length >= 2 ? results : popularResults);

  /** Check if a registry server matches an already-configured server.
   *  Matches by URL (http) or package identifier in args (stdio). */
  function isInstalled(server: McpRegistryServer): boolean {
    // HTTP: match by URL
    if (server.remotes.length > 0) {
      const remoteUrl = server.remotes[0].url;
      return installedServers.some((s) => s.url && s.url === remoteUrl);
    }
    // stdio: match by package identifier in args
    if (server.packages.length > 0) {
      const pkgId = server.packages[0].identifier;
      if (pkgId) {
        return installedServers.some(
          (s) => s.args && s.args.some((a) => a === pkgId || a.includes(pkgId)),
        );
      }
    }
    return false;
  }

  async function refreshInstalledServers() {
    try {
      const servers = await listConfiguredMcpServers(projectCwd || undefined);
      installedServers = servers;
      dbg("mcp-discover", "installed servers", servers.length);
    } catch (e) {
      dbgWarn("mcp-discover", "failed to load installed servers", e);
    }
  }

  // ── Init ──

  $effect(() => {
    // Load on mount (runs once)
    loadInitial();
  });

  // Refresh installed state when tab becomes visible
  $effect(() => {
    if (visible) {
      refreshInstalledServers();
    }
  });

  async function loadInitial() {
    checkMcpRegistryHealth()
      .then((h) => {
        registryHealth = h;
        dbg("mcp-discover", "health", h);
      })
      .catch(() => {});
    searchMcpRegistry("server", 20)
      .then((r) => {
        popularResults = r.servers;
        dbg("mcp-discover", "popular loaded", r.servers.length);
      })
      .catch(() => {});
    refreshInstalledServers();
  }

  async function refreshRegistry() {
    refreshing = true;
    try {
      const [h, r] = await Promise.all([checkMcpRegistryHealth(), searchMcpRegistry("server", 20)]);
      registryHealth = h;
      popularResults = r.servers;
      dbg("mcp-discover", "refreshed", { health: h.available, popular: r.servers.length });
    } catch (e) {
      dbgWarn("mcp-discover", "refresh error", e);
    } finally {
      refreshing = false;
    }
  }

  // ── Search ──

  function handleSearch() {
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    const q = query.trim();
    if (q.length < 2) {
      results = [];
      return;
    }
    searchDebounceTimer = setTimeout(async () => {
      searching = true;
      try {
        const r = await searchMcpRegistry(q, 30);
        results = r.servers;
        dbg("mcp-discover", "search results", { query: q, count: r.servers.length });
      } catch (e) {
        showToast(t("mcp_searchFailed"), "error");
      } finally {
        searching = false;
      }
    }, 300);
  }

  function setFilter(q: string) {
    query = q;
    handleSearch();
  }

  // ── Detail / Install ──

  function selectServer(server: McpRegistryServer) {
    detail = server;
    envValues = {};
    headerValues = {};
    // Pre-fill env var names from first package
    if (server.packages.length > 0) {
      for (const ev of server.packages[0].environmentVariables) {
        envValues[ev.name] = "";
      }
    }
    // Pre-fill header names from first remote
    if (server.remotes.length > 0) {
      for (const h of server.remotes[0].headers) {
        headerValues[h.name] = h.value ?? "";
      }
    }
  }

  function getTransportType(server: McpRegistryServer): "stdio" | "http" {
    if (server.remotes.length > 0) return "http";
    return "stdio";
  }

  function getTransportLabel(server: McpRegistryServer): string {
    if (server.remotes.length > 0) return "HTTP";
    if (server.packages.length > 0) {
      const pkg = server.packages[0];
      return pkg.registryType || "npm";
    }
    return "stdio";
  }

  async function handleInstall(server: McpRegistryServer) {
    operationLoading = server.name;
    const transport = getTransportType(server);

    try {
      let result: PluginOperationResult;

      if (transport === "http" && server.remotes.length > 0) {
        const remote = server.remotes[0];
        // Build non-empty headers
        const hdrs: Record<string, string> = {};
        for (const [k, v] of Object.entries(headerValues)) {
          if (v.trim()) hdrs[k] = v.trim();
        }
        result = await addMcpServer(
          server.name,
          "http",
          installScope,
          projectCwd || undefined,
          undefined,
          remote.url,
          undefined,
          Object.keys(hdrs).length > 0 ? hdrs : undefined,
        );
      } else if (server.packages.length > 0) {
        const pkg = server.packages[0];
        // Build env object (only non-empty values)
        const env: Record<string, string> = {};
        for (const [k, v] of Object.entries(envValues)) {
          if (v.trim()) env[k] = v.trim();
        }
        // Build config JSON for add-json
        const config: Record<string, unknown> = {
          type: "stdio",
          command: pkg.registryType === "pypi" ? "uvx" : "npx",
          args: ["-y", pkg.identifier],
        };
        if (Object.keys(env).length > 0) {
          config.env = env;
        }
        result = await addMcpServer(
          server.name,
          "stdio",
          installScope,
          projectCwd || undefined,
          JSON.stringify(config),
        );
      } else {
        showToast(t("mcp_noPackageFound"), "error");
        return;
      }

      showToast(
        result.success ? t("mcp_addedServer", { name: server.name }) : result.message,
        result.success ? "success" : "error",
      );
      if (result.success) {
        await refreshInstalledServers();
      }
      dbg("mcp-discover", "install result", result);
    } catch (e) {
      showToast(t("mcp_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }
</script>

<!-- Health badge + search + scope -->
<div class="flex items-center gap-3 mb-4">
  <!-- Health indicator + refresh -->
  <div class="flex items-center gap-1 shrink-0">
    <div class="flex items-center gap-1.5" title={registryHealth?.reason ?? ""}>
      <span
        class="inline-block h-2 w-2 rounded-full {registryHealth === null
          ? 'bg-muted-foreground/40'
          : registryHealth.available
            ? 'bg-green-500'
            : 'bg-red-500'}"
      ></span>
      <span class="text-[10px] text-muted-foreground">registry</span>
    </div>
    <button
      class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted transition-colors disabled:opacity-40"
      title={t("mcp_refreshTitle")}
      disabled={refreshing}
      onclick={refreshRegistry}
    >
      <svg
        class="h-3 w-3 {refreshing ? 'animate-spin' : ''}"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /></svg
      >
    </button>
  </div>

  <!-- Search input -->
  <div class="relative flex-1">
    <svg
      class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></svg
    >
    <input
      type="text"
      placeholder={t("mcp_searchPlaceholder")}
      class="w-full rounded-md border border-border bg-background pl-8 pr-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
      bind:value={query}
      oninput={handleSearch}
    />
  </div>

  <!-- Scope selector -->
  <div class="flex rounded-md border border-border p-0.5 shrink-0">
    <button
      class="rounded px-2 py-1 text-[11px] font-medium transition-colors {installScope === 'user'
        ? 'bg-primary text-primary-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => (installScope = "user")}>{t("mcp_scopeUser")}</button
    >
    <button
      class="rounded px-2 py-1 text-[11px] font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed {installScope ===
      'project'
        ? 'bg-primary text-primary-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      disabled={!projectCwd}
      onclick={() => (installScope = "project")}>{t("mcp_scopeProject")}</button
    >
    <button
      class="rounded px-2 py-1 text-[11px] font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed {installScope ===
      'local'
        ? 'bg-primary text-primary-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      disabled={!projectCwd}
      onclick={() => (installScope = "local")}>{t("mcp_scopeLocal")}</button
    >
  </div>
</div>

<!-- Quick filters -->
<div class="flex flex-wrap gap-1.5 mb-4">
  {#each ["filesystem", "database", "github", "api", "web", "docker"] as filter}
    <button
      class="rounded-full border border-border px-2.5 py-0.5 text-[10px] text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-colors {query ===
      filter
        ? 'bg-primary/10 border-primary/30 text-foreground'
        : ''}"
      onclick={() => setFilter(filter)}
    >
      {filter}
    </button>
  {/each}
</div>

<!-- Loading spinner -->
{#if searching}
  <div class="flex items-center justify-center py-4">
    <div
      class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
    ></div>
    <span class="ml-2 text-xs text-muted-foreground">{t("mcp_searching")}</span>
  </div>
{/if}

<!-- Results -->
{#if !searching}
  <div>
    <div class="text-[11px] font-medium text-muted-foreground mb-2">
      {query.trim().length >= 2
        ? t("mcp_resultsCount", { count: String(results.length) })
        : t("mcp_popularServers")}
    </div>

    {#if displayResults.length === 0}
      <div class="flex flex-col items-center justify-center py-12 text-center gap-2">
        <p class="text-xs text-muted-foreground">
          {query.trim().length >= 2
            ? t("mcp_noServersFound")
            : refreshing
              ? t("mcp_loadingPopular")
              : t("mcp_couldNotLoadPopular")}
        </p>
        {#if query.trim().length < 2 && !refreshing}
          <button
            class="rounded-md border border-border px-3 py-1.5 text-xs text-foreground hover:bg-muted transition-colors"
            onclick={refreshRegistry}
          >
            {t("common_retry")}
          </button>
        {/if}
      </div>
    {:else}
      <!-- Side-by-side: list (left) + detail (right) -->
      <div class="flex gap-3" style="height: calc(100vh - 340px); min-height: 300px;">
        <!-- Left: scrollable server list -->
        <div class="w-[280px] shrink-0 overflow-y-auto space-y-1.5 pr-1">
          {#each displayResults as server}
            <div
              class="w-full text-left rounded-lg border px-3 py-2 transition-colors cursor-pointer {detail?.name ===
                server.name && detail?.version === server.version
                ? 'border-primary/50 bg-primary/5'
                : 'border-border/50 bg-muted/30 hover:bg-muted/50'}"
              onclick={() => selectServer(server)}
              onkeydown={(e) => {
                if (e.key === "Enter") selectServer(server);
              }}
              role="button"
              tabindex="0"
            >
              <div class="flex items-center justify-between gap-2">
                <div class="flex-1 min-w-0">
                  <span class="text-sm font-medium text-foreground truncate block"
                    >{server.name}</span
                  >
                  <div class="flex items-center gap-2 mt-0.5">
                    {#if server.version}
                      <span class="text-[10px] text-muted-foreground">v{server.version}</span>
                    {/if}
                    <span
                      class="rounded-full px-1.5 py-0.5 text-[9px] font-medium bg-teal-500/10 text-teal-600 dark:text-teal-400"
                    >
                      {getTransportLabel(server)}
                    </span>
                  </div>
                </div>
                {#if isInstalled(server)}
                  <span
                    class="rounded-md px-2 py-1 text-[10px] font-medium bg-green-500/10 text-green-600 dark:text-green-400 shrink-0"
                    >{t("mcp_installed")}</span
                  >
                {:else}
                  <button
                    class="rounded-md px-2 py-1 text-[10px] font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50 shrink-0"
                    onclick={(e) => {
                      e.stopPropagation();
                      handleInstall(server);
                    }}
                    disabled={operationLoading === server.name}
                  >
                    {operationLoading === server.name ? "..." : t("mcp_addButton")}
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>

        <!-- Right: detail panel -->
        <div class="flex-1 min-w-0 overflow-y-auto">
          {#if detail}
            <div class="rounded-lg border border-border/50 bg-muted/20 p-4 space-y-3">
              <!-- Header -->
              <div class="flex items-start justify-between gap-2">
                <div class="flex-1 min-w-0">
                  <h3 class="text-sm font-semibold text-foreground truncate">
                    {detail.title ?? detail.name}
                  </h3>
                  <div class="flex items-center gap-2 mt-1 flex-wrap">
                    {#if detail.version}
                      <span class="text-[10px] text-muted-foreground">v{detail.version}</span>
                    {/if}
                    <span
                      class="rounded-full px-1.5 py-0.5 text-[9px] font-medium bg-teal-500/10 text-teal-600 dark:text-teal-400"
                    >
                      {getTransportLabel(detail)}
                    </span>
                    {#if detail.repository?.url}
                      <a
                        href={detail.repository.url}
                        target="_blank"
                        rel="noopener noreferrer"
                        class="text-[10px] text-muted-foreground hover:text-foreground underline"
                        >Repository</a
                      >
                    {/if}
                  </div>
                  {#if detail.description}
                    <p class="text-[11px] text-muted-foreground mt-1.5">{detail.description}</p>
                  {/if}
                </div>
                <button
                  class="shrink-0 text-muted-foreground hover:text-foreground"
                  onclick={() => (detail = null)}
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

              <!-- Packages -->
              {#if detail.packages.length > 0}
                <div class="border-t border-border pt-3">
                  <div class="text-[11px] font-medium text-muted-foreground mb-2">
                    {t("mcp_packages")}
                  </div>
                  {#each detail.packages as pkg}
                    <div class="rounded-md bg-muted/40 px-3 py-2 mb-1.5 text-xs">
                      <div class="flex items-center gap-2">
                        <span class="font-mono text-foreground">{pkg.identifier}</span>
                        <span
                          class="rounded-full px-1.5 py-0.5 text-[9px] font-medium bg-blue-500/10 text-blue-600 dark:text-blue-400"
                        >
                          {pkg.registryType}
                        </span>
                        {#if pkg.version}
                          <span class="text-muted-foreground">{pkg.version}</span>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}

              <!-- Remotes -->
              {#if detail.remotes.length > 0}
                <div class="border-t border-border pt-3">
                  <div class="text-[11px] font-medium text-muted-foreground mb-2">
                    {t("mcp_remoteEndpoints")}
                  </div>
                  {#each detail.remotes as remote}
                    <div class="rounded-md bg-muted/40 px-3 py-2 mb-1.5 text-xs">
                      <div class="font-mono text-foreground truncate">{remote.url}</div>
                      <div class="text-muted-foreground mt-0.5">{remote.type}</div>
                    </div>
                  {/each}
                </div>
              {/if}

              <!-- Install form -->
              <div class="border-t border-border pt-3">
                <div class="text-[11px] font-medium text-muted-foreground mb-2">
                  {t("mcp_install")}
                </div>

                <!-- Env vars (stdio servers) -->
                {#if detail.packages.length > 0 && detail.packages[0].environmentVariables.length > 0}
                  <div class="space-y-2 mb-3">
                    <div class="text-[10px] text-muted-foreground">{t("mcp_envVars")}</div>
                    {#each detail.packages[0].environmentVariables as envVar}
                      <div class="flex items-center gap-2">
                        <label
                          class="text-[11px] text-foreground font-mono w-40 truncate shrink-0"
                          title={envVar.description ?? envVar.name}
                        >
                          {envVar.name}
                          {#if envVar.isRequired}
                            <span class="text-destructive">*</span>
                          {/if}
                        </label>
                        <input
                          type={envVar.isSecret ? "password" : "text"}
                          placeholder={envVar.description ?? ""}
                          class="flex-1 rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                          value={envValues[envVar.name] ?? ""}
                          oninput={(e) => {
                            envValues[envVar.name] = (e.target as HTMLInputElement).value;
                          }}
                        />
                      </div>
                    {/each}
                  </div>
                {/if}

                <!-- Headers (http servers) -->
                {#if detail.remotes.length > 0 && detail.remotes[0].headers.length > 0}
                  <div class="space-y-2 mb-3">
                    <div class="text-[10px] text-muted-foreground">{t("mcp_headers")}</div>
                    {#each detail.remotes[0].headers as header}
                      <div class="flex items-center gap-2">
                        <label
                          class="text-[11px] text-foreground font-mono w-40 truncate shrink-0"
                          title={header.description ?? header.name}
                        >
                          {header.name}
                          {#if header.isRequired}
                            <span class="text-destructive">*</span>
                          {/if}
                        </label>
                        <input
                          type={header.isSecret ? "password" : "text"}
                          placeholder={header.description ?? ""}
                          class="flex-1 rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                          value={headerValues[header.name] ?? ""}
                          oninput={(e) => {
                            headerValues[header.name] = (e.target as HTMLInputElement).value;
                          }}
                        />
                      </div>
                    {/each}
                  </div>
                {/if}

                {#if isInstalled(detail)}
                  <div
                    class="rounded-md bg-green-500/10 px-3 py-1.5 text-xs font-medium text-green-600 dark:text-green-400 text-center"
                  >
                    {t("mcp_alreadyInstalled")}
                  </div>
                {:else}
                  <button
                    class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                    onclick={() => handleInstall(detail!)}
                    disabled={operationLoading === detail.name}
                  >
                    {operationLoading === detail.name
                      ? t("mcp_adding")
                      : t("mcp_addToScope", { scope: installScope })}
                  </button>
                {/if}
              </div>
            </div>
          {:else}
            <div
              class="rounded-lg border border-dashed border-border/50 p-6 flex items-center justify-center h-full"
            >
              <p class="text-xs text-muted-foreground">{t("mcp_clickToSeeDetails")}</p>
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}

<script lang="ts">
  import { onMount, getContext } from "svelte";
  import { goto } from "$app/navigation";
  import {
    listMarketplacePlugins,
    listStandaloneSkills,
    listMarketplaces,
    getSkillContent,
    createSkill,
    updateSkill,
    deleteSkill,
    listInstalledPlugins,
    installPlugin,
    uninstallPlugin,
    enablePlugin,
    disablePlugin,
    updatePlugin,
    addMarketplace,
    removeMarketplace,
    updateMarketplace,
    checkCommunityHealth,
    searchCommunitySkills,
    getCommunitySkillDetail,
    installCommunitySkill,
  } from "$lib/api";
  import { formatInstallCount, relativeTime } from "$lib/utils/format";
  import { renderMarkdown } from "$lib/utils/markdown";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import McpDiscoverPanel from "$lib/components/McpDiscoverPanel.svelte";
  import McpConfiguredPanel from "$lib/components/McpConfiguredPanel.svelte";
  import HookManager from "$lib/components/HookManager.svelte";
  import AgentsPanel from "$lib/components/AgentsPanel.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import type {
    MarketplacePlugin,
    StandaloneSkill,
    MarketplaceInfo,
    InstalledPlugin,
    CommunitySkillResult,
    CommunitySkillDetail,
    ProviderHealth,
  } from "$lib/types";

  // Active section driven by layout sidebar context
  const sectionCtx = getContext<{ active: string }>("pluginSection");
  let activeTab = $derived(
    (sectionCtx?.active ?? "skills") as "skills" | "mcp" | "hooks" | "plugins" | "agents",
  );

  // Skills section: Discover vs Installed toggle
  let skillsSource = $state<"discover" | "installed">("discover");

  // Plugins section: Marketplace vs Installed toggle
  let pluginsSource = $state<"marketplace" | "installed">("marketplace");

  // Plugins section: Registries collapsed state
  let registriesOpen = $state(false);

  // MCP section: toggle
  let mcpSource = $state<"discover" | "configured">("discover");

  let plugins = $state<MarketplacePlugin[]>([]);
  let installedPlugins = $state<InstalledPlugin[]>([]);
  let skills = $state<StandaloneSkill[]>([]);
  let marketplaces = $state<MarketplaceInfo[]>([]);
  let loading = $state(true);
  let loadError = $state(false);
  let loadWarnings = $state<string[]>([]);
  let searchQuery = $state("");
  let selectedCategory = $state<string | null>(null);
  let selectedSkillPath = $state<string | null>(null);

  // Skill editor state
  let editorMode = $state<null | "new" | "edit">(null);
  let editorName = $state("");
  let editorDescription = $state("");
  let editorContent = $state("");
  let editorScope = $state<"user" | "project">("user");
  let editorPath = $state("");
  let editorSaving = $state(false);

  // Project CWD for project-scope skills
  let projectCwd = $state("");

  // Operation state
  let operationLoading = $state<string | null>(null);
  let toastMessage = $state<string | null>(null);
  let toastType = $state<"success" | "error">("success");
  let toastTimeout: ReturnType<typeof setTimeout> | null = null;

  // Scope selector for install operations
  let installScope = $state<"user" | "project" | "local">("user");

  // Confirmation dialog
  let confirmAction = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  // Marketplace add input
  let newMarketplaceSource = $state("");

  // Community tab state
  let communityQuery = $state("");
  let communityResults = $state<CommunitySkillResult[]>([]);
  let communityPopular = $state<CommunitySkillResult[]>([]);
  let communitySearching = $state(false);
  let communityScope = $state<"user" | "project">("user");
  let communityHealth = $state<ProviderHealth | null>(null);
  let communityDetail = $state<CommunitySkillDetail | null>(null);
  let communityDetailLoading = $state(false);
  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;

  let communityDisplayResults = $derived(
    communityQuery.trim().length >= 2 ? communityResults : communityPopular,
  );

  const categoryColors: Record<string, string> = {
    development: "bg-blue-500/10 text-blue-600 dark:text-blue-400",
    productivity: "bg-teal-500/10 text-teal-600 dark:text-teal-400",
    security: "bg-red-500/10 text-red-600 dark:text-red-400",
    testing: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
    learning: "bg-purple-500/10 text-purple-600 dark:text-purple-400",
    database: "bg-green-500/10 text-green-600 dark:text-green-400",
    monitoring: "bg-orange-500/10 text-orange-600 dark:text-orange-400",
    deployment: "bg-indigo-500/10 text-indigo-600 dark:text-indigo-400",
    design: "bg-pink-500/10 text-pink-600 dark:text-pink-400",
  };

  const componentBadges: {
    key: keyof MarketplacePlugin["components"];
    label: () => string;
    color: string;
  }[] = [
    {
      key: "skills",
      label: () => t("plugin_badgeSkills"),
      color: "bg-rose-500/10 text-rose-600 dark:text-rose-400",
    },
    {
      key: "commands",
      label: () => t("plugin_badgeCommands"),
      color: "bg-blue-500/10 text-blue-600 dark:text-blue-400",
    },
    {
      key: "agents",
      label: () => t("plugin_badgeAgents"),
      color: "bg-purple-500/10 text-purple-600 dark:text-purple-400",
    },
    {
      key: "hooks",
      label: () => t("plugin_badgeHooks"),
      color: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
    },
    {
      key: "mcp_servers",
      label: () => t("plugin_badgeMcp"),
      color: "bg-teal-500/10 text-teal-600 dark:text-teal-400",
    },
    {
      key: "lsp_servers",
      label: () => t("plugin_badgeLsp"),
      color: "bg-green-500/10 text-green-600 dark:text-green-400",
    },
  ];

  // ── Derived values ──

  let categories = $derived([
    ...new Set(plugins.map((p) => p.category).filter(Boolean)),
  ] as string[]);

  let filteredPlugins = $derived(
    plugins.filter((p) => {
      if (selectedCategory && p.category !== selectedCategory) return false;
      if (!searchQuery) return true;
      const q = searchQuery.toLowerCase();
      return (
        p.name.toLowerCase().includes(q) ||
        p.description.toLowerCase().includes(q) ||
        (p.author?.name ?? "").toLowerCase().includes(q)
      );
    }),
  );

  // Installed skill names (for community install status check)
  let installedSkillNames = $derived(new Set(skills.map((s) => s.name.toLowerCase())));

  // MCP: installed plugins that declare mcp_servers

  // ── Lifecycle ──

  onMount(async () => {
    // Initialize from URL params
    const params = new URL(window.location.href).searchParams;
    const urlSection = params.get("section");
    if (urlSection && ["skills", "mcp", "hooks", "plugins"].includes(urlSection)) {
      if (sectionCtx) sectionCtx.active = urlSection;
    }
    const urlSource = params.get("source");
    if (urlSection === "skills" && (urlSource === "discover" || urlSource === "installed")) {
      skillsSource = urlSource;
    }
    if (urlSection === "plugins" && (urlSource === "marketplace" || urlSource === "installed")) {
      pluginsSource = urlSource;
    }
    if (urlSection === "mcp" && (urlSource === "discover" || urlSource === "configured")) {
      mcpSource = urlSource;
    }

    projectCwd = localStorage.getItem("ocv:project-cwd") ?? "";
    loading = true;
    const warnings: string[] = [];
    try {
      const results = await Promise.allSettled([
        listMarketplacePlugins(),
        listInstalledPlugins(),
        listStandaloneSkills(projectCwd || undefined),
        listMarketplaces(),
      ]);

      if (results[0].status === "fulfilled") {
        plugins = results[0].value;
      } else {
        dbgWarn("plugins", "marketplace load error", results[0].reason);
        warnings.push("marketplace plugins");
      }

      if (results[1].status === "fulfilled") {
        installedPlugins = results[1].value;
      } else {
        dbgWarn("plugins", "installed plugins load error", results[1].reason);
        warnings.push("installed plugins");
      }

      if (results[2].status === "fulfilled") {
        skills = results[2].value;
      } else {
        dbgWarn("plugins", "skills load error", results[2].reason);
        warnings.push("standalone skills");
      }

      if (results[3].status === "fulfilled") {
        marketplaces = results[3].value;
      } else {
        dbgWarn("plugins", "marketplaces load error", results[3].reason);
        warnings.push("marketplaces");
      }

      loadWarnings = warnings;
      loadError = warnings.length === 4;

      dbg("plugins", "loaded", {
        marketplace: plugins.length,
        installed: installedPlugins.length,
        skills: skills.length,
        marketplaces: marketplaces.length,
        warnings: warnings.length,
      });
    } catch (e) {
      dbgWarn("plugins", "load error", e);
      loadError = true;
    } finally {
      loading = false;
    }

    // Load community health + popular list (non-blocking)
    checkCommunityHealth()
      .then((h) => {
        communityHealth = h;
      })
      .catch(() => {});
    searchCommunitySkills("skill", 20)
      .then((r) => {
        communityPopular = r;
      })
      .catch(() => {});
  });

  // ── Helpers ──

  function hasComponent(
    components: MarketplacePlugin["components"],
    key: keyof MarketplacePlugin["components"],
  ): boolean {
    const val = components[key];
    if (typeof val === "boolean") return val;
    if (Array.isArray(val)) return val.length > 0;
    return false;
  }

  function componentCount(
    components: MarketplacePlugin["components"],
    key: keyof MarketplacePlugin["components"],
  ): number {
    const val = components[key];
    if (Array.isArray(val)) return val.length;
    return 0;
  }

  function getCategoryColor(category: string): string {
    return categoryColors[category.toLowerCase()] ?? "bg-muted text-muted-foreground";
  }

  // ── URL sync ──

  function syncUrl() {
    const section = activeTab;
    let url = `/plugins?section=${section}`;
    if (section === "skills") url += `&source=${skillsSource}`;
    else if (section === "plugins") url += `&source=${pluginsSource}`;
    else if (section === "mcp") url += `&source=${mcpSource}`;
    // hooks has no sub-source
    goto(url, { replaceState: true, noScroll: true });
  }

  // ── Skill CRUD ──

  function startNewSkill() {
    editorMode = "new";
    editorName = "";
    editorDescription = "Brief description";
    editorContent = "# New Skill\n\nInstructions for Claude...";
    editorScope = "user";
    editorPath = "";
    selectedSkillPath = null;
  }

  function startEditSkill(skill: StandaloneSkill) {
    editorMode = "edit";
    editorName = skill.name;
    editorDescription = skill.description;
    editorPath = skill.path;
    editorScope = (skill.scope as "user" | "project") ?? "user";
    getSkillContent(skill.path, projectCwd || undefined)
      .then((raw) => {
        editorContent = raw;
        selectedSkillPath = skill.path;
      })
      .catch((e) => {
        editorContent = t("plugin_loadFailedContent");
        dbgWarn("plugins", "edit load error", e);
      });
  }

  function cancelEditor() {
    editorMode = null;
    editorName = "";
    editorDescription = "";
    editorContent = "";
    editorPath = "";
  }

  async function handleCreateSkill() {
    const name = editorName.trim();
    if (!name) {
      showToast(t("plugin_skillNameRequired"), "error");
      return;
    }
    editorSaving = true;
    dbg("plugins", "createSkill", { name, scope: editorScope });
    try {
      const skill = await createSkill(
        name,
        editorDescription.trim(),
        editorContent,
        editorScope,
        projectCwd || undefined,
      );
      showToast(t("plugin_createdSkill", { name: skill.name }), "success");
      cancelEditor();
      await refreshSkills();
    } catch (e) {
      showToast(t("plugin_failedCreateSkill", { error: String(e) }), "error");
    } finally {
      editorSaving = false;
    }
  }

  async function handleSaveSkill() {
    editorSaving = true;
    dbg("plugins", "updateSkill", { path: editorPath });
    try {
      await updateSkill(editorPath, editorContent, projectCwd || undefined);
      showToast(t("plugin_skillSaved"), "success");
      cancelEditor();
      await refreshSkills();
    } catch (e) {
      showToast(t("plugin_failedSaveSkill", { error: String(e) }), "error");
    } finally {
      editorSaving = false;
    }
  }

  function handleDeleteSkill(skill: StandaloneSkill) {
    confirmAction = {
      title: t("plugin_deleteSkillTitle"),
      message: t("plugin_deleteSkillMsg", { name: skill.name }),
      onConfirm: async () => {
        operationLoading = skill.path;
        dbg("plugins", "deleteSkill", { path: skill.path });
        try {
          await deleteSkill(skill.path, projectCwd || undefined);
          showToast(t("plugin_deletedSkill", { name: skill.name }), "success");
          if (selectedSkillPath === skill.path) {
            selectedSkillPath = null;
          }
          cancelEditor();
          await refreshSkills();
        } catch (e) {
          showToast(t("plugin_failedDeleteSkill", { error: String(e) }), "error");
        } finally {
          operationLoading = null;
        }
      },
    };
  }

  async function refreshSkills() {
    try {
      skills = await listStandaloneSkills(projectCwd || undefined);
    } catch (e) {
      dbgWarn("plugins", "refresh skills error", e);
    }
  }

  // ── Community skill handlers ──

  let communityRefreshing = $state(false);

  async function refreshCommunity() {
    communityRefreshing = true;
    try {
      const [h, r] = await Promise.all([
        checkCommunityHealth(),
        searchCommunitySkills("skill", 20),
      ]);
      communityHealth = h;
      communityPopular = r;
      dbg("plugins", "community refreshed", { health: h.available, popular: r.length });
    } catch (e) {
      dbgWarn("plugins", "community refresh error", e);
    } finally {
      communityRefreshing = false;
    }
  }

  function handleCommunitySearch() {
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    const q = communityQuery.trim();
    if (q.length < 2) {
      communityResults = [];
      return;
    }
    searchDebounceTimer = setTimeout(async () => {
      communitySearching = true;
      try {
        communityResults = await searchCommunitySkills(q, 30);
      } catch (e) {
        showToast(t("plugin_searchFailed", { error: String(e) }), "error");
      } finally {
        communitySearching = false;
      }
    }, 300);
  }

  async function handleCommunityDetail(skill: CommunitySkillResult) {
    communityDetailLoading = true;
    communityDetail = null;
    try {
      communityDetail = await getCommunitySkillDetail(skill.source, skill.name);
    } catch (e) {
      showToast(t("plugin_failedLoadDetail", { error: String(e) }), "error");
    } finally {
      communityDetailLoading = false;
    }
  }

  async function handleCommunityInstall(skill: CommunitySkillResult) {
    operationLoading = skill.id;
    try {
      const result = await installCommunitySkill(
        skill.source,
        skill.name,
        communityScope,
        projectCwd || undefined,
      );
      showToast(
        result.success ? t("plugin_installedSkill", { name: skill.name }) : result.message,
        result.success ? "success" : "error",
      );
      if (result.success) {
        await refreshSkills();
        await refreshPluginData();
      }
    } catch (e) {
      showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }

  function setCommunityFilter(q: string) {
    communityQuery = q;
    handleCommunitySearch();
  }

  // ── Toast & refresh helpers ──

  function showToast(message: string, type: "success" | "error") {
    toastMessage = message;
    toastType = type;
    if (toastTimeout) clearTimeout(toastTimeout);
    toastTimeout = setTimeout(() => {
      toastMessage = null;
    }, 4000);
  }

  async function refreshPluginData() {
    const results = await Promise.allSettled([listMarketplacePlugins(), listInstalledPlugins()]);
    if (results[0].status === "fulfilled") plugins = results[0].value;
    else dbgWarn("plugins", "refresh marketplace error", results[0].reason);
    if (results[1].status === "fulfilled") installedPlugins = results[1].value;
    else dbgWarn("plugins", "refresh installed error", results[1].reason);
  }

  // ── Plugin operation handlers ──

  async function handleInstall(pluginName: string) {
    operationLoading = pluginName;
    dbg("plugins", "install", { name: pluginName, scope: installScope });
    try {
      const result = await installPlugin(pluginName, installScope);
      dbg("plugins", "install result", result);
      showToast(
        result.success
          ? t("plugin_installedPlugin", { name: pluginName })
          : t("plugin_failedOp", { error: result.message }),
        result.success ? "success" : "error",
      );
      if (result.success) await refreshPluginData();
    } catch (e) {
      showToast(t("plugin_errorInstalling", { name: pluginName, error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }

  async function handleUninstall(pluginName: string, scope: string) {
    confirmAction = {
      title: t("plugin_uninstallTitle"),
      message: t("plugin_uninstallMsg", { name: pluginName }),
      onConfirm: async () => {
        operationLoading = pluginName;
        dbg("plugins", "uninstall", { name: pluginName, scope });
        try {
          const result = await uninstallPlugin(pluginName, scope);
          dbg("plugins", "uninstall result", result);
          showToast(
            result.success
              ? t("plugin_uninstalledPlugin", { name: pluginName })
              : t("plugin_failedOp", { error: result.message }),
            result.success ? "success" : "error",
          );
          if (result.success) await refreshPluginData();
        } catch (e) {
          showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
        } finally {
          operationLoading = null;
        }
      },
    };
  }

  async function handleToggleEnabled(plugin: InstalledPlugin) {
    const action = plugin.enabled !== false ? "disable" : "enable";
    const scope = (plugin.scope as string) ?? "user";
    operationLoading = plugin.name;
    dbg("plugins", action, { name: plugin.name, scope });
    try {
      const fn = plugin.enabled !== false ? disablePlugin : enablePlugin;
      const result = await fn(plugin.name, scope);
      dbg("plugins", `${action} result`, result);
      showToast(
        result.success
          ? plugin.enabled !== false
            ? t("plugin_disabledPlugin", { name: plugin.name })
            : t("plugin_enabledPlugin", { name: plugin.name })
          : t("plugin_failedOp", { error: result.message }),
        result.success ? "success" : "error",
      );
      if (result.success) await refreshPluginData();
    } catch (e) {
      showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }

  async function handleUpdate(pluginName: string, scope: string) {
    operationLoading = pluginName;
    dbg("plugins", "update", { name: pluginName, scope });
    try {
      const result = await updatePlugin(pluginName, scope);
      dbg("plugins", "update result", result);
      showToast(
        result.success
          ? t("plugin_updatedName", { name: pluginName })
          : t("plugin_failedOp", { error: result.message }),
        result.success ? "success" : "error",
      );
      if (result.success) await refreshPluginData();
    } catch (e) {
      showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }

  // ── Marketplace operation handlers ──

  async function handleAddMarketplace() {
    const source = newMarketplaceSource.trim();
    if (!source) return;
    operationLoading = "__marketplace_add";
    dbg("plugins", "addMarketplace", { source });
    try {
      const result = await addMarketplace(source);
      dbg("plugins", "addMarketplace result", result);
      showToast(
        result.success
          ? t("plugin_addedMarketplace")
          : t("plugin_failedOp", { error: result.message }),
        result.success ? "success" : "error",
      );
      if (result.success) {
        newMarketplaceSource = "";
        [plugins, marketplaces] = await Promise.all([listMarketplacePlugins(), listMarketplaces()]);
      }
    } catch (e) {
      showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }

  async function handleRemoveMarketplace(name: string) {
    confirmAction = {
      title: t("plugin_removeMarketplaceTitle"),
      message: t("plugin_removeMarketplaceMsg", { name }),
      onConfirm: async () => {
        operationLoading = `__mp_${name}`;
        dbg("plugins", "removeMarketplace", { name });
        try {
          const result = await removeMarketplace(name);
          dbg("plugins", "removeMarketplace result", result);
          showToast(
            result.success
              ? t("plugin_removedMarketplace", { name })
              : t("plugin_failedOp", { error: result.message }),
            result.success ? "success" : "error",
          );
          if (result.success) {
            [plugins, marketplaces] = await Promise.all([
              listMarketplacePlugins(),
              listMarketplaces(),
            ]);
          }
        } catch (e) {
          showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
        } finally {
          operationLoading = null;
        }
      },
    };
  }

  async function handleUpdateMarketplace(name: string) {
    operationLoading = `__mp_${name}`;
    dbg("plugins", "updateMarketplace", { name });
    try {
      const result = await updateMarketplace(name);
      dbg("plugins", "updateMarketplace result", result);
      showToast(
        result.success
          ? t("plugin_updatedName", { name })
          : t("plugin_failedOp", { error: result.message }),
        result.success ? "success" : "error",
      );
      if (result.success) {
        [plugins, marketplaces] = await Promise.all([listMarketplacePlugins(), listMarketplaces()]);
      }
    } catch (e) {
      showToast(t("plugin_errorGeneric", { error: String(e) }), "error");
    } finally {
      operationLoading = null;
    }
  }
</script>

<!-- Toast notification -->
{#if toastMessage}
  <div
    class="fixed top-4 right-4 z-50 rounded-lg border px-4 py-2 text-sm shadow-lg transition-opacity {toastType ===
    'success'
      ? 'border-green-500/30 bg-green-500/10 text-green-600 dark:text-green-400'
      : 'border-destructive/30 bg-destructive/10 text-destructive'}"
  >
    {toastMessage}
  </div>
{/if}

<!-- Confirmation dialog -->
{#if confirmAction}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    onclick={() => (confirmAction = null)}
  >
    <div
      class="rounded-lg border border-border bg-background p-6 shadow-xl max-w-sm"
      onclick={(e) => e.stopPropagation()}
    >
      <h3 class="text-sm font-semibold text-foreground mb-2">{confirmAction.title}</h3>
      <p class="text-xs text-muted-foreground mb-4">{confirmAction.message}</p>
      <div class="flex justify-end gap-2">
        <button
          class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          onclick={() => (confirmAction = null)}>{t("common_cancel")}</button
        >
        <button
          class="rounded-md bg-destructive px-3 py-1.5 text-xs text-destructive-foreground hover:bg-destructive/90"
          onclick={() => {
            confirmAction?.onConfirm();
            confirmAction = null;
          }}>{t("plugin_confirm")}</button
        >
      </div>
    </div>
  </div>
{/if}

<div class="px-6 py-5 h-full overflow-y-auto">
  {#if loading}
    <div class="flex items-center justify-center py-16">
      <div
        class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
    </div>
  {:else if loadError}
    <div class="flex flex-col items-center justify-center py-16 text-center">
      <p class="text-sm text-destructive">
        {t("plugin_loadFailed")}
      </p>
    </div>
  {:else}
    <!-- Partial load warning -->
    {#if loadWarnings.length > 0}
      <div
        class="rounded-md border border-amber-500/30 bg-amber-500/10 px-4 py-2 text-xs text-amber-600 dark:text-amber-400 mb-4"
      >
        {t("plugin_couldNotLoad", { items: loadWarnings.join(", ") })}
      </div>
    {/if}

    <!-- ═══════════════════════════════════════════════════════ -->
    <!-- Skills Section                                         -->
    <!-- ═══════════════════════════════════════════════════════ -->
    <div class="space-y-4" class:hidden={activeTab !== "skills"}>
      <div>
        <h2 class="text-sm font-semibold text-foreground">{t("plugin_title")}</h2>
        <p class="text-[11px] text-muted-foreground">
          {t("plugin_desc")}
        </p>
      </div>

      <!-- Source toggle + Create Skill -->
      <div class="flex items-center gap-3">
        <div class="flex gap-1 rounded-lg border border-border p-0.5 w-fit">
          <button
            class="rounded-md px-3 py-1 text-xs font-medium transition-colors {skillsSource ===
            'discover'
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => {
              skillsSource = "discover";
              syncUrl();
            }}>{t("plugin_discover")}</button
          >
          <button
            class="rounded-md px-3 py-1 text-xs font-medium transition-colors {skillsSource ===
            'installed'
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => {
              skillsSource = "installed";
              syncUrl();
            }}>{t("plugin_installed")}</button
          >
        </div>
        <button
          class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
          onclick={startNewSkill}
        >
          {t("plugin_createSkill")}
        </button>
      </div>

      <!-- Create Skill editor (shown inline, hides sub-views) -->
      {#if editorMode === "new"}
        <div class="rounded-lg border border-border/50 bg-muted/20 px-4 py-4 space-y-3">
          <div class="flex items-center justify-between">
            <h3 class="text-sm font-medium text-foreground">{t("plugin_createSkill")}</h3>
            <button
              class="text-xs text-muted-foreground hover:text-foreground"
              onclick={cancelEditor}>{t("common_cancel")}</button
            >
          </div>

          <div>
            <label class="block text-[11px] font-medium text-muted-foreground mb-1"
              >{t("plugin_editorName")}</label
            >
            <input
              type="text"
              placeholder="my-skill-name"
              class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={editorName}
            />
          </div>

          <div>
            <label class="block text-[11px] font-medium text-muted-foreground mb-1"
              >{t("plugin_editorDescription")}</label
            >
            <input
              type="text"
              placeholder="Brief description of what this skill does"
              class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={editorDescription}
            />
          </div>

          <div>
            <label class="block text-[11px] font-medium text-muted-foreground mb-1"
              >{t("plugin_editorScope")}</label
            >
            <select
              class="rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={editorScope}
            >
              <option value="user">{t("plugin_editorScopeUser")}</option>
              <option value="project" disabled={!projectCwd}>
                {t("plugin_editorScopeProject")}
                {projectCwd ? "" : t("plugin_editorNoProject")}
              </option>
            </select>
          </div>

          <div>
            <label class="block text-[11px] font-medium text-muted-foreground mb-1"
              >{t("plugin_editorContent")}</label
            >
            <textarea
              class="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring resize-y"
              rows="16"
              placeholder="# Skill Title&#10;&#10;Instructions for Claude..."
              bind:value={editorContent}
            ></textarea>
          </div>

          <div class="flex justify-end gap-2">
            <button
              class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
              onclick={cancelEditor}>{t("common_cancel")}</button
            >
            <button
              class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
              onclick={handleCreateSkill}
              disabled={editorSaving || !editorName.trim()}
            >
              {editorSaving ? t("plugin_saving") : t("plugin_createSkill")}
            </button>
          </div>
        </div>
      {/if}

      <!-- Discover sub-view (community skills) -->
      <div class:hidden={skillsSource !== "discover" || editorMode === "new"}>
        <!-- Health badge + search + scope -->
        <div class="flex items-center gap-3 mb-4">
          <!-- Health indicator + refresh -->
          <div class="flex items-center gap-1 shrink-0">
            <div class="flex items-center gap-1.5" title={communityHealth?.reason ?? ""}>
              <span
                class="inline-block h-2 w-2 rounded-full {communityHealth === null
                  ? 'bg-muted-foreground/40'
                  : communityHealth.available
                    ? 'bg-green-500'
                    : 'bg-red-500'}"
              ></span>
              <span class="text-[10px] text-muted-foreground">skills.sh</span>
            </div>
            <button
              class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted transition-colors disabled:opacity-40"
              title="Refresh"
              disabled={communityRefreshing}
              onclick={refreshCommunity}
            >
              <svg
                class="h-3 w-3 {communityRefreshing ? 'animate-spin' : ''}"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                ><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" /><path
                  d="M21 3v5h-5"
                /></svg
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
              stroke-linejoin="round"
              ><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></svg
            >
            <input
              type="text"
              placeholder={t("plugin_searchCommunity")}
              class="w-full rounded-md border border-border bg-background pl-8 pr-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={communityQuery}
              oninput={handleCommunitySearch}
            />
          </div>

          <!-- Scope selector -->
          <div class="flex rounded-md border border-border p-0.5 shrink-0">
            <button
              class="rounded px-2 py-1 text-[11px] font-medium transition-colors {communityScope ===
              'user'
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (communityScope = "user")}>{t("plugin_scopeUser")}</button
            >
            <button
              class="rounded px-2 py-1 text-[11px] font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed {communityScope ===
              'project'
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              disabled={!projectCwd}
              onclick={() => (communityScope = "project")}>{t("plugin_scopeProject")}</button
            >
          </div>
        </div>

        <!-- Quick filters -->
        <div class="flex flex-wrap gap-1.5 mb-4">
          {#each ["react", "python", "security", "testing", "devops", "best practices"] as filter}
            <button
              class="rounded-full border border-border px-2.5 py-0.5 text-[10px] text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-colors {communityQuery ===
              filter
                ? 'bg-primary/10 border-primary/30 text-foreground'
                : ''}"
              onclick={() => setCommunityFilter(filter)}
            >
              {filter}
            </button>
          {/each}
        </div>

        <!-- Loading spinner for search -->
        {#if communitySearching}
          <div class="flex items-center justify-center py-4">
            <div
              class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
            ></div>
            <span class="ml-2 text-xs text-muted-foreground">{t("plugin_searching")}</span>
          </div>
        {/if}

        <!-- Results / Popular -->
        {#if !communitySearching}
          <div>
            <div class="text-[11px] font-medium text-muted-foreground mb-2">
              {communityQuery.trim().length >= 2
                ? t("plugin_nResults", { count: String(communityResults.length) })
                : t("plugin_popularSkills")}
            </div>

            {#if communityDisplayResults.length === 0}
              <div class="flex flex-col items-center justify-center py-12 text-center gap-2">
                <p class="text-xs text-muted-foreground">
                  {communityQuery.trim().length >= 2
                    ? t("plugin_noSkillsFound")
                    : communityRefreshing
                      ? t("plugin_loadingPopular")
                      : t("plugin_couldNotLoadPopular")}
                </p>
                {#if communityQuery.trim().length < 2 && !communityRefreshing}
                  <button
                    class="rounded-md border border-border px-3 py-1.5 text-xs text-foreground hover:bg-muted transition-colors"
                    onclick={refreshCommunity}
                  >
                    {t("common_retry")}
                  </button>
                {/if}
              </div>
            {:else}
              <!-- Side-by-side: skill list (left) + preview (right) -->
              <div class="flex gap-3" style="height: calc(100vh - 320px); min-height: 300px;">
                <!-- Left: scrollable skill list -->
                <div class="w-[280px] shrink-0 overflow-y-auto space-y-1.5 pr-1">
                  {#each communityDisplayResults as skill}
                    {@const isInstalled = installedSkillNames.has(skill.name.toLowerCase())}
                    <div
                      class="w-full text-left rounded-lg border px-3 py-2 transition-colors cursor-pointer {communityDetail?.id ===
                      skill.id
                        ? 'border-primary/50 bg-primary/5'
                        : 'border-border/50 bg-muted/30 hover:bg-muted/50'}"
                      onclick={() => handleCommunityDetail(skill)}
                      onkeydown={(e) => {
                        if (e.key === "Enter") handleCommunityDetail(skill);
                      }}
                      role="button"
                      tabindex="0"
                    >
                      <div class="flex items-center justify-between gap-2">
                        <div class="flex-1 min-w-0">
                          <span class="text-sm font-medium text-foreground truncate block"
                            >{skill.name}</span
                          >
                          <div class="flex items-center gap-2 mt-0.5">
                            {#if skill.installs > 0}
                              <span class="text-[10px] text-muted-foreground"
                                >{formatInstallCount(skill.installs)}</span
                              >
                            {/if}
                            <span class="text-[10px] text-muted-foreground/50 truncate"
                              >{skill.source}</span
                            >
                          </div>
                        </div>
                        <button
                          class="rounded-md px-2 py-1 text-[10px] font-medium transition-colors disabled:opacity-50 shrink-0 {isInstalled
                            ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 cursor-default'
                            : 'bg-primary text-primary-foreground hover:bg-primary/90'}"
                          onclick={(e) => {
                            e.stopPropagation();
                            if (!isInstalled) handleCommunityInstall(skill);
                          }}
                          disabled={isInstalled || operationLoading === skill.id}
                        >
                          {#if operationLoading === skill.id}
                            ...
                          {:else if isInstalled}
                            {t("plugin_installed")}
                          {:else}
                            {t("plugin_install")}
                          {/if}
                        </button>
                      </div>
                    </div>
                  {/each}
                </div>

                <!-- Right: preview panel (sticky) -->
                <div class="flex-1 min-w-0 overflow-y-auto">
                  {#if communityDetailLoading}
                    <div
                      class="rounded-lg border border-border/50 bg-muted/20 p-6 flex items-center justify-center h-full"
                    >
                      <div
                        class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                      ></div>
                      <span class="ml-2 text-xs text-muted-foreground"
                        >{t("plugin_loadingPreview")}</span
                      >
                    </div>
                  {:else if communityDetail}
                    <div class="rounded-lg border border-border/50 bg-muted/20 p-4 space-y-3">
                      <!-- Header -->
                      <div class="flex items-start justify-between gap-2">
                        <div class="flex-1 min-w-0">
                          <h3 class="text-sm font-semibold text-foreground truncate">
                            {communityDetail.name}
                          </h3>
                          <div class="flex items-center gap-2 mt-1 flex-wrap">
                            {#if communityDetail.installs > 0}
                              <span class="text-[10px] text-muted-foreground"
                                >{formatInstallCount(communityDetail.installs)}
                                {t("plugin_installs")}</span
                              >
                            {/if}
                            <span class="text-[10px] text-muted-foreground/60"
                              >{communityDetail.source}</span
                            >
                            {#if communityDetail.github_url}
                              <a
                                href={communityDetail.github_url}
                                target="_blank"
                                rel="noopener noreferrer"
                                class="text-[10px] text-muted-foreground hover:text-foreground underline"
                                >GitHub</a
                              >
                            {/if}
                            {#if communityDetail.skills_sh_url}
                              <a
                                href={communityDetail.skills_sh_url}
                                target="_blank"
                                rel="noopener noreferrer"
                                class="text-[10px] text-muted-foreground hover:text-foreground underline"
                                >skills.sh</a
                              >
                            {/if}
                          </div>
                          {#if communityDetail.description}
                            <p class="text-[11px] text-muted-foreground mt-1.5">
                              {communityDetail.description}
                            </p>
                          {/if}
                        </div>
                        <button
                          class="shrink-0 text-muted-foreground hover:text-foreground"
                          onclick={() => (communityDetail = null)}
                          title={t("plugin_closePreview")}
                        >
                          <svg
                            class="h-3.5 w-3.5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            ><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
                          >
                        </button>
                      </div>

                      <!-- SKILL.md content -->
                      {#if communityDetail.content}
                        <div class="border-t border-border pt-3">
                          <div class="prose prose-sm dark:prose-invert max-w-none">
                            {@html renderMarkdown(communityDetail.content)}
                          </div>
                        </div>
                      {:else}
                        <div class="border-t border-border pt-3">
                          <p class="text-xs text-muted-foreground italic">
                            {t("plugin_noContentPreview")}
                          </p>
                        </div>
                      {/if}
                    </div>
                  {:else}
                    <div
                      class="rounded-lg border border-dashed border-border/50 p-6 flex items-center justify-center h-full"
                    >
                      <p class="text-xs text-muted-foreground">{t("plugin_clickToPreview")}</p>
                    </div>
                  {/if}
                </div>
              </div>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Installed sub-view (standalone skills) -->
      <div class:hidden={skillsSource !== "installed" || editorMode === "new"}>
        <div class="mb-4">
          <h3 class="text-xs font-medium text-muted-foreground">
            {t("plugin_standaloneSkills")}
          </h3>
        </div>

        {#if skills.length === 0 && !editorMode}
          <div class="flex flex-col items-center justify-center py-16 text-center">
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
                ><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20" /></svg
              >
            </div>
            <h2 class="text-sm font-medium text-foreground mb-1">
              {t("plugin_noStandaloneSkills")}
            </h2>
            <p class="text-xs text-muted-foreground max-w-sm mb-3">
              {t("plugin_skillsEmptyDesc")}
            </p>
            <button
              class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
              onclick={startNewSkill}
            >
              {t("plugin_createFirstSkill")}
            </button>
          </div>
        {:else}
          <div class="flex gap-3" style="height: calc(100vh - 320px); min-height: 300px;">
            <!-- Left: scrollable skill list -->
            <div class="w-[280px] shrink-0 overflow-y-auto space-y-1.5 pr-1">
              {#each skills as skill}
                <div
                  class="w-full text-left rounded-lg border px-3 py-2 transition-colors cursor-pointer {selectedSkillPath ===
                    skill.path && !editorMode
                    ? 'border-primary/50 bg-primary/5'
                    : 'border-border/50 bg-muted/30 hover:bg-muted/50'}"
                  onclick={() => startEditSkill(skill)}
                  onkeydown={(e) => {
                    if (e.key === "Enter") startEditSkill(skill);
                  }}
                  role="button"
                  tabindex="0"
                >
                  <div class="flex items-center justify-between gap-2">
                    <div class="flex-1 min-w-0">
                      <span class="text-sm font-medium text-foreground truncate block"
                        >{skill.name}</span
                      >
                      <div class="flex items-center gap-2 mt-0.5">
                        {#if skill.scope}
                          <span
                            class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {skill.scope ===
                            'project'
                              ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                              : 'bg-muted text-muted-foreground'}">{skill.scope}</span
                          >
                        {/if}
                        <span class="text-[10px] text-muted-foreground truncate"
                          >{skill.description}</span
                        >
                      </div>
                    </div>
                    <button
                      class="shrink-0 rounded p-1 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                      onclick={(e) => {
                        e.stopPropagation();
                        handleDeleteSkill(skill);
                      }}
                      title={t("plugin_deleteSkillTooltip")}
                      disabled={operationLoading === skill.path}
                    >
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
                    </button>
                  </div>
                </div>
              {/each}
            </div>

            <!-- Right: Edit editor or placeholder -->
            <div class="flex-1 min-w-0 overflow-y-auto">
              {#if editorMode === "edit"}
                <!-- Skill edit editor -->
                <div class="rounded-lg border border-border/50 bg-muted/20 px-4 py-4 space-y-3">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-medium text-foreground">
                      {t("plugin_editSkillHeader", { name: editorName })}
                    </h3>
                    <button
                      class="text-xs text-muted-foreground hover:text-foreground"
                      onclick={cancelEditor}>{t("common_cancel")}</button
                    >
                  </div>

                  <div>
                    <label class="block text-[11px] font-medium text-muted-foreground mb-1"
                      >{t("plugin_skillMdContent")}</label
                    >
                    <textarea
                      class="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring resize-y"
                      rows="16"
                      placeholder="# Skill Title&#10;&#10;Instructions for Claude..."
                      bind:value={editorContent}
                    ></textarea>
                  </div>

                  <div class="flex justify-end gap-2">
                    <button
                      class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
                      onclick={cancelEditor}>{t("common_cancel")}</button
                    >
                    <button
                      class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                      onclick={handleSaveSkill}
                      disabled={editorSaving}
                    >
                      {editorSaving ? t("plugin_saving") : t("plugin_saveChanges")}
                    </button>
                  </div>
                </div>
              {:else}
                <div
                  class="rounded-lg border border-dashed border-border/50 p-6 flex items-center justify-center h-full"
                >
                  <p class="text-xs text-muted-foreground">{t("plugin_selectSkillToEdit")}</p>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>
    </div>

    <!-- ═══════════════════════════════════════════════════════ -->
    <!-- MCP Servers Section                                    -->
    <!-- ═══════════════════════════════════════════════════════ -->
    <div class="space-y-4" class:hidden={activeTab !== "mcp"}>
      <div>
        <h2 class="text-sm font-semibold text-foreground">{t("plugin_mcpTitle")}</h2>
        <p class="text-[11px] text-muted-foreground">
          {t("plugin_mcpDesc")}
        </p>
      </div>

      <!-- Source toggle -->
      <div class="flex gap-1 rounded-lg border border-border p-0.5 w-fit">
        <button
          class="rounded-md px-3 py-1 text-xs font-medium transition-colors {mcpSource ===
          'discover'
            ? 'bg-primary text-primary-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => {
            mcpSource = "discover";
            syncUrl();
          }}>{t("plugin_discover")}</button
        >
        <button
          class="rounded-md px-3 py-1 text-xs font-medium transition-colors {mcpSource ===
          'configured'
            ? 'bg-primary text-primary-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => {
            mcpSource = "configured";
            syncUrl();
          }}>{t("plugin_configured")}</button
        >
      </div>

      <!-- Discover sub-view -->
      <div class:hidden={mcpSource !== "discover"}>
        <McpDiscoverPanel
          {projectCwd}
          visible={mcpSource === "discover"}
          bind:operationLoading
          {showToast}
        />
      </div>

      <!-- Configured sub-view -->
      <div class:hidden={mcpSource !== "configured"}>
        <McpConfiguredPanel
          {projectCwd}
          visible={mcpSource === "configured"}
          bind:operationLoading
          {showToast}
          bind:confirmAction
        />
      </div>
    </div>

    <!-- ═══════════════════════════════════════════════════════ -->
    <!-- Hooks Section                                           -->
    <!-- ═══════════════════════════════════════════════════════ -->
    <div class="space-y-4" class:hidden={activeTab !== "hooks"}>
      <HookManager />
    </div>

    <!-- ═══════════════════════════════════════════════════════ -->
    <!-- Plugins Section                                        -->
    <!-- ═══════════════════════════════════════════════════════ -->
    <div class="space-y-4" class:hidden={activeTab !== "plugins"}>
      <div>
        <h2 class="text-sm font-semibold text-foreground">{t("plugin_pluginsTitle")}</h2>
        <p class="text-[11px] text-muted-foreground">{t("plugin_pluginsDesc")}</p>
      </div>

      <!-- Source toggle -->
      <div class="flex gap-1 rounded-lg border border-border p-0.5 w-fit">
        <button
          class="rounded-md px-3 py-1 text-xs font-medium transition-colors {pluginsSource ===
          'marketplace'
            ? 'bg-primary text-primary-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => {
            pluginsSource = "marketplace";
            syncUrl();
          }}>{t("plugin_marketplaceCount", { count: String(plugins.length) })}</button
        >
        <button
          class="rounded-md px-3 py-1 text-xs font-medium transition-colors {pluginsSource ===
          'installed'
            ? 'bg-primary text-primary-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => {
            pluginsSource = "installed";
            syncUrl();
          }}>{t("plugin_installedCount", { count: String(installedPlugins.length) })}</button
        >
      </div>

      <!-- Marketplace sub-view -->
      <div class:hidden={pluginsSource !== "marketplace"}>
        <!-- Search + Filter + Scope -->
        <div class="flex items-center gap-3 mb-4">
          <div class="relative flex-1">
            <svg
              class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              ><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></svg
            >
            <input
              type="text"
              placeholder={t("plugin_searchPlugins")}
              class="w-full rounded-md border border-border bg-background pl-8 pr-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={searchQuery}
            />
          </div>
          <select
            class="rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground focus:outline-none focus:ring-1 focus:ring-ring"
            value={selectedCategory ?? ""}
            onchange={(e) => {
              const val = (e.target as HTMLSelectElement).value;
              selectedCategory = val || null;
            }}
          >
            <option value="">{t("plugin_allCategories")}</option>
            {#each categories as cat}
              <option value={cat}>{cat}</option>
            {/each}
          </select>
          <div class="flex rounded-md border border-border p-0.5 shrink-0">
            <button
              class="rounded px-2 py-1 text-[11px] font-medium transition-colors {installScope ===
              'user'
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (installScope = "user")}>{t("plugin_scopeUser")}</button
            >
            <button
              class="rounded px-2 py-1 text-[11px] font-medium transition-colors {installScope ===
              'project'
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (installScope = "project")}>{t("plugin_scopeProject")}</button
            >
            <button
              class="rounded px-2 py-1 text-[11px] font-medium transition-colors {installScope ===
              'local'
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (installScope = "local")}>{t("plugin_scopeLocal")}</button
            >
          </div>
        </div>

        <!-- Plugin cards -->
        {#if filteredPlugins.length === 0}
          <div class="flex flex-col items-center justify-center py-16 text-center">
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
                ><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" /></svg
              >
            </div>
            <h2 class="text-sm font-medium text-foreground mb-1">{t("plugin_noPluginsFound")}</h2>
            <p class="text-xs text-muted-foreground max-w-sm">
              {#if searchQuery || selectedCategory}
                {t("plugin_noPluginsMatch")}
              {:else}
                {t("plugin_addMarketplaceHint")}
              {/if}
            </p>
          </div>
        {:else}
          <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
            {#each filteredPlugins as plugin}
              <div class="rounded-lg border border-border/50 bg-muted/30 px-4 py-3 space-y-2">
                <!-- Name + version + homepage -->
                <div class="flex items-start gap-2">
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="text-sm font-medium text-foreground truncate">{plugin.name}</span
                      >
                      {#if plugin.version}
                        <span class="text-[10px] text-muted-foreground shrink-0"
                          >v{plugin.version}</span
                        >
                      {/if}
                    </div>
                    {#if plugin.author}
                      <div class="text-[11px] text-muted-foreground/70">{plugin.author.name}</div>
                    {/if}
                  </div>
                  <div class="flex items-center gap-2 shrink-0">
                    <!-- Install button -->
                    <button
                      class="rounded-md bg-primary px-2 py-1 text-[10px] font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                      onclick={() => handleInstall(plugin.name)}
                      disabled={operationLoading === plugin.name}
                    >
                      {operationLoading === plugin.name
                        ? t("plugin_installing")
                        : t("plugin_install")}
                    </button>
                    {#if plugin.install_count != null && plugin.install_count > 0}
                      <span class="text-[10px] text-muted-foreground"
                        >{formatInstallCount(plugin.install_count)} {t("plugin_installs")}</span
                      >
                    {/if}
                    {#if plugin.homepage}
                      <a
                        href={plugin.homepage}
                        target="_blank"
                        rel="noopener noreferrer"
                        class="text-muted-foreground hover:text-foreground transition-colors"
                        title={t("plugin_homepage")}
                      >
                        <svg
                          class="h-3.5 w-3.5"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                          ><path
                            d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"
                          /><polyline points="15 3 21 3 21 9" /><line
                            x1="10"
                            x2="21"
                            y1="14"
                            y2="3"
                          /></svg
                        >
                      </a>
                    {/if}
                  </div>
                </div>

                <!-- Description -->
                <p class="text-xs text-muted-foreground line-clamp-2">{plugin.description}</p>

                <!-- Badges row: category + components -->
                <div class="flex flex-wrap items-center gap-1.5">
                  {#if plugin.category}
                    <span
                      class="rounded-full px-2 py-0.5 text-[10px] font-medium {getCategoryColor(
                        plugin.category,
                      )}">{plugin.category}</span
                    >
                  {/if}
                  {#each componentBadges as badge}
                    {#if hasComponent(plugin.components, badge.key)}
                      <span class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {badge.color}">
                        {badge.label()}{#if componentCount(plugin.components, badge.key) > 0}
                          ({componentCount(plugin.components, badge.key)}){/if}
                      </span>
                    {/if}
                  {/each}
                </div>

                <!-- Tags -->
                {#if plugin.tags.length > 0}
                  <div class="flex flex-wrap gap-1">
                    {#each plugin.tags as tag}
                      <span class="rounded bg-muted px-1.5 py-0.5 text-[9px] text-muted-foreground"
                        >{tag}</span
                      >
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Installed sub-view -->
      <div class:hidden={pluginsSource !== "installed"}>
        {#if installedPlugins.length === 0}
          <div class="flex flex-col items-center justify-center py-16 text-center">
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
                ><path
                  d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"
                /></svg
              >
            </div>
            <h2 class="text-sm font-medium text-foreground mb-1">
              {t("plugin_noInstalledPlugins")}
            </h2>
            <p class="text-xs text-muted-foreground max-w-sm">
              {t("plugin_installFromMarketplace")}
            </p>
          </div>
        {:else}
          <div class="space-y-2">
            {#each installedPlugins as plugin}
              <div
                class="rounded-lg border border-border/50 bg-muted/30 px-4 py-3 flex items-center justify-between gap-4"
              >
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="text-sm font-medium text-foreground">{plugin.name}</span>
                    {#if plugin.version}
                      <span class="text-[10px] text-muted-foreground">v{plugin.version}</span>
                    {/if}
                    {#if plugin.scope}
                      <span
                        class="rounded-full px-1.5 py-0.5 text-[9px] font-medium bg-muted text-muted-foreground"
                        >{plugin.scope}</span
                      >
                    {/if}
                  </div>
                  {#if plugin.description}
                    <p class="text-xs text-muted-foreground mt-0.5 line-clamp-1">
                      {plugin.description}
                    </p>
                  {/if}
                  <!-- Component badges for installed plugins -->
                  {#if plugins.find((p) => p.name === plugin.name)}
                    {@const mpMatch = plugins.find((p) => p.name === plugin.name)}
                    {#if mpMatch}
                      <div class="flex flex-wrap gap-1 mt-1">
                        {#each componentBadges as badge}
                          {#if hasComponent(mpMatch.components, badge.key)}
                            <span
                              class="rounded-full px-1.5 py-0.5 text-[9px] font-medium {badge.color}"
                            >
                              {badge.label()}
                            </span>
                          {/if}
                        {/each}
                      </div>
                    {/if}
                  {/if}
                </div>
                <div class="flex items-center gap-2 shrink-0">
                  <!-- Enable/Disable toggle -->
                  <button
                    class="rounded-md border border-border px-2 py-1 text-[10px] {plugin.enabled !==
                    false
                      ? 'text-green-600 dark:text-green-400 border-green-500/30'
                      : 'text-muted-foreground'} hover:bg-muted transition-colors disabled:opacity-50"
                    onclick={() => handleToggleEnabled(plugin)}
                    disabled={operationLoading === plugin.name}
                  >
                    {plugin.enabled !== false ? t("plugin_enabled") : t("plugin_disabled")}
                  </button>
                  <!-- Update button -->
                  <button
                    class="rounded-md border border-border px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground hover:bg-muted transition-colors disabled:opacity-50"
                    onclick={() => handleUpdate(plugin.name, (plugin.scope as string) ?? "user")}
                    disabled={operationLoading === plugin.name}
                    title="Update plugin"
                  >
                    {t("plugin_update")}
                  </button>
                  <!-- Uninstall button -->
                  <button
                    class="rounded-md border border-destructive/30 px-2 py-1 text-[10px] text-destructive hover:bg-destructive/10 transition-colors disabled:opacity-50"
                    onclick={() => handleUninstall(plugin.name, (plugin.scope as string) ?? "user")}
                    disabled={operationLoading === plugin.name}
                  >
                    {t("plugin_uninstall")}
                  </button>
                  <!-- Loading spinner -->
                  {#if operationLoading === plugin.name}
                    <div
                      class="h-3.5 w-3.5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                    ></div>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Registries (collapsible, always visible at bottom) -->
      <div class="mt-6 border-t border-border pt-4">
        <button
          class="flex items-center gap-2 text-xs font-medium text-muted-foreground hover:text-foreground transition-colors"
          onclick={() => (registriesOpen = !registriesOpen)}
        >
          <svg
            class="h-3 w-3 transition-transform {registriesOpen ? 'rotate-180' : ''}"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
          >
          {t("plugin_registries", { count: String(marketplaces.length) })}
        </button>
        {#if registriesOpen}
          <div class="mt-3 space-y-3">
            <!-- Add marketplace input -->
            <div class="flex gap-2">
              <input
                type="text"
                placeholder={t("plugin_marketplacePlaceholder")}
                class="flex-1 rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                bind:value={newMarketplaceSource}
              />
              <button
                class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                onclick={() => handleAddMarketplace()}
                disabled={!newMarketplaceSource.trim() || operationLoading === "__marketplace_add"}
              >
                {operationLoading === "__marketplace_add" ? t("plugin_adding") : t("plugin_add")}
              </button>
            </div>

            {#if marketplaces.length === 0}
              <div class="flex flex-col items-center justify-center py-8 text-center">
                <p class="text-xs text-muted-foreground">
                  {t("plugin_noMarketplaces")}
                </p>
              </div>
            {:else}
              <div class="space-y-2">
                {#each marketplaces as mp}
                  <div class="rounded-lg border border-border/50 bg-muted/30 px-4 py-3">
                    <div class="flex items-center justify-between">
                      <div>
                        <span class="text-sm font-medium text-foreground">{mp.name}</span>
                        <div
                          class="flex items-center gap-3 mt-0.5 text-[11px] text-muted-foreground"
                        >
                          <span>{t("plugin_pluginCount", { count: String(mp.plugin_count) })}</span>
                          {#if mp.last_updated}
                            <span
                              >{t("plugin_updatedTime", {
                                time: relativeTime(mp.last_updated),
                              })}</span
                            >
                          {/if}
                        </div>
                      </div>
                      <div class="flex items-center gap-2">
                        <button
                          class="rounded-md border border-border px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground hover:bg-muted transition-colors disabled:opacity-50"
                          onclick={() => handleUpdateMarketplace(mp.name)}
                          disabled={operationLoading === `__mp_${mp.name}`}
                        >
                          {operationLoading === `__mp_${mp.name}`
                            ? t("plugin_updating")
                            : t("plugin_update")}
                        </button>
                        <button
                          class="rounded-md border border-destructive/30 px-2 py-1 text-[10px] text-destructive hover:bg-destructive/10 transition-colors disabled:opacity-50"
                          onclick={() => handleRemoveMarketplace(mp.name)}
                          disabled={operationLoading === `__mp_${mp.name}`}
                        >
                          {t("plugin_remove")}
                        </button>
                        {#if operationLoading === `__mp_${mp.name}`}
                          <div
                            class="h-3.5 w-3.5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                          ></div>
                        {/if}
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
    <!-- ═══════════════════════════════════════════════════════ -->
    <!-- Agents Section                                        -->
    <!-- ═══════════════════════════════════════════════════════ -->
    <div class="space-y-4" class:hidden={activeTab !== "agents"}>
      <AgentsPanel {projectCwd} {showToast} />
    </div>
  {/if}
</div>

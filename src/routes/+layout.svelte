<script lang="ts">
  import "../app.css";
  import {
    listRuns,
    getUserSettings,
    updateUserSettings,
    listDirectory,
    getGitSummary,
    listPromptFavorites,
    searchPrompts,
  } from "$lib/api";
  import RunListItem from "$lib/components/RunListItem.svelte";
  import ProjectSelector from "$lib/components/ProjectSelector.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";
  import SetupWizard from "$lib/components/SetupWizard.svelte";
  import AboutModal from "$lib/components/AboutModal.svelte";
  import CliSessionBrowser from "$lib/components/CliSessionBrowser.svelte";
  import UpdateBanner from "$lib/components/UpdateBanner.svelte";
  import type {
    TaskRun,
    UserSettings,
    DirEntry,
    GitSummary,
    PromptFavorite,
    PromptSearchResult,
  } from "$lib/types";
  import { cwdDisplayLabel, truncate, relativeTime } from "$lib/utils/format";
  import { page } from "$app/stores";
  import { goto, afterNavigate } from "$app/navigation";
  import { onMount, setContext } from "svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { PLATFORM_PRESETS } from "$lib/utils/platform-presets";
  import type { PlatformCredential } from "$lib/types";
  import { TeamStore } from "$lib/stores/team-store.svelte";
  import { KeybindingStore } from "$lib/stores/keybindings.svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import {
    t,
    LOCALE_REGISTRY,
    getEntry,
    initLocale,
    switchLocale,
    currentLocale,
  } from "$lib/i18n/index.svelte";

  // Wire reactive locale before any t() usage
  initLocale();

  let localePopupOpen = $state(false);

  function handleLocaleSelect(code: string) {
    switchLocale(code);
    localePopupOpen = false;
  }

  let commandPaletteOpen = $state(false);
  let showSetupWizard = $state(false);
  let showAbout = $state(false);
  let showCliBrowser = $state(false);

  // Team store (shared via context with /teams page)
  const teamStore = new TeamStore();
  setContext("teamStore", teamStore);

  // Keybinding store (shared via context with all pages)
  const keybindingStore = new KeybindingStore();
  setContext("keybindings", keybindingStore);

  let { children } = $props();

  let runs = $state<TaskRun[]>([]);
  let sidebarFavorites = $state<PromptFavorite[]>([]);
  let favoriteRunIds = $derived(new Set(sidebarFavorites.map((f) => f.runId)));
  let settings = $state<UserSettings | null>(null);
  let sidebarOpen = $state(true);
  let projectCwd = $state("");
  type ThemeMode = "light" | "dark" | "system";

  function getInitialTheme(): ThemeMode {
    if (typeof window === "undefined") return "dark";
    const saved = localStorage.getItem("ocv:theme");
    if (saved === "light" || saved === "dark" || saved === "system") return saved;
    return "dark";
  }

  let themeMode = $state<ThemeMode>(getInitialTheme());
  let systemDark = $state(
    typeof window !== "undefined"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : true,
  );
  let effectiveDark = $derived(themeMode === "system" ? systemDark : themeMode === "dark");
  let pinnedCwds = $state<string[]>([]);

  let panelTab = $state<"chats" | "teams">("chats");
  let runSearchQuery = $state("");

  // ── Deep search (backend full-text) ──
  let searchResults = $state<PromptSearchResult[]>([]);
  let searching = $state(false);
  let searchRequestId = $state(0);
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  // ── File tree state (shown in sidebar when on /explorer) ──
  interface TreeNode {
    name: string;
    fullPath: string;
    is_dir: boolean;
    size: number;
    expanded: boolean;
    loaded: boolean;
    children: TreeNode[];
    depth: number;
  }

  let fileTree = $state<TreeNode[]>([]);
  let treeLoading = $state(false);
  let explorerSelectedFile = $state("");
  let explorerTab = $state<"files" | "git">("files");

  // ── Git state (shown in sidebar Git tab when on /explorer) ──
  let gitSummary = $state<GitSummary | null>(null);
  let gitLoading = $state(false);

  const GIT_STATUS_COLORS: Record<string, string> = {
    M: "text-blue-400",
    A: "text-green-400",
    D: "text-red-400",
    R: "text-purple-400",
    "?": "text-muted-foreground",
  };

  function entriesToNodes(entries: DirEntry[], parentPath: string, depth: number): TreeNode[] {
    return entries.map((e) => ({
      name: e.name,
      fullPath: `${parentPath}/${e.name}`,
      is_dir: e.is_dir,
      size: e.size,
      expanded: false,
      loaded: false,
      children: [],
      depth,
    }));
  }

  async function loadRootTree() {
    if (!projectCwd) {
      fileTree = [];
      return;
    }
    treeLoading = true;
    try {
      const listing = await listDirectory(projectCwd, true);
      fileTree = entriesToNodes(listing.entries, projectCwd, 0);
      dbg("layout", "file tree loaded", { count: fileTree.length });
    } catch (e) {
      dbgWarn("layout", "file tree load error", e);
      fileTree = [];
    } finally {
      treeLoading = false;
    }
  }

  async function toggleFolder(node: TreeNode) {
    if (!node.loaded) {
      try {
        const listing = await listDirectory(node.fullPath, true);
        node.children = entriesToNodes(listing.entries, node.fullPath, node.depth + 1);
        node.loaded = true;
        dbg("layout", "folder loaded", { path: node.fullPath, count: node.children.length });
      } catch (e) {
        dbgWarn("layout", "folder load error", e);
        node.children = [];
        node.loaded = true;
      }
    }
    node.expanded = !node.expanded;
  }

  function selectFile(node: TreeNode) {
    explorerSelectedFile = node.fullPath;
    // Notify explorer page via custom event
    window.dispatchEvent(new CustomEvent("ocv:explorer-file", { detail: { path: node.fullPath } }));
  }

  async function loadGitSummary() {
    if (!projectCwd) {
      gitSummary = null;
      return;
    }
    gitLoading = true;
    try {
      gitSummary = await getGitSummary(projectCwd);
      dbg("layout", "git summary loaded", {
        branch: gitSummary.branch,
        files: gitSummary.total_files,
      });
    } catch {
      gitSummary = null;
    } finally {
      gitLoading = false;
    }
  }

  function selectDiffFile(filePath: string) {
    // Notify explorer page to show diff
    window.dispatchEvent(new CustomEvent("ocv:explorer-diff", { detail: { path: filePath } }));
  }

  // Load tree + git when switching to explorer page or changing project
  $effect(() => {
    const _path = currentPath;
    const _cwd = projectCwd;
    if (_path?.startsWith("/explorer") && _cwd) {
      loadRootTree();
      loadGitSummary();
    }
  });

  // Navigation items (declared before pageName derivation)
  const navItems = [
    { path: "/chat", label: () => t("nav_chat"), icon: "message" },
    { path: "/explorer", label: () => t("nav_explorer"), icon: "folder" },
    { path: "/plugins", label: () => t("nav_extend"), icon: "zap" },
    { path: "/memory", label: () => t("nav_memory"), icon: "book" },
    { path: "/usage", label: () => t("nav_usage"), icon: "chart" },
    { path: "/settings", label: () => t("nav_settings"), icon: "settings" },
  ];

  // Load initial data
  async function loadRuns() {
    try {
      runs = await listRuns();
    } catch {
      // Silently fail
    }
  }

  async function loadSidebarFavorites() {
    try {
      sidebarFavorites = await listPromptFavorites();
    } catch {
      // Silently fail
    }
  }

  // ── Deep search ──

  function onDeepQueryInput() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => doDeepSearch(), 300);
  }

  async function doDeepSearch() {
    const q = runSearchQuery.trim();
    if (!q) {
      searchResults = [];
      searching = false;
      return;
    }
    searching = true;
    const reqId = ++searchRequestId;
    try {
      const results = await searchPrompts(q);
      if (reqId !== searchRequestId) return;
      searchResults = results;
      dbg("layout", "search results", { count: results.length });
    } catch (e) {
      if (reqId !== searchRequestId) return;
      dbg("layout", "search error", e);
      searchResults = [];
    } finally {
      if (reqId === searchRequestId) searching = false;
    }
  }

  function highlightMatch(text: string, query: string): string {
    if (!query.trim()) return escapeHtml(text);
    const escaped = escapeHtml(text);
    const q = escapeHtml(query.trim());
    const re = new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi");
    return escaped.replace(re, "<mark>$1</mark>");
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  async function loadSettings() {
    try {
      settings = await getUserSettings();
      if (settings.working_directory) {
        localStorage.setItem("ocv:settings-cwd", settings.working_directory);
        if (!projectCwd) projectCwd = settings.working_directory;
      }
      // Show setup wizard if onboarding not completed
      if (!settings.onboarding_completed) {
        showSetupWizard = true;
      }
      // One-time migration: if platform_credentials is empty but api_key exists,
      // create an initial credential from current settings
      await migrateCredentialsIfNeeded(settings);
    } catch {
      // Silently fail
    }
  }

  /** Migrate existing api_key into platform_credentials (one-time). */
  async function migrateCredentialsIfNeeded(s: UserSettings) {
    if (s.platform_credentials && s.platform_credentials.length > 0) return;
    if (!s.anthropic_api_key) return;

    // Detect platform from base_url
    let platformId = "anthropic";
    if (s.anthropic_base_url) {
      const match = PLATFORM_PRESETS.find((p) => p.base_url && s.anthropic_base_url === p.base_url);
      platformId = match?.id ?? "custom-migrated";
    }

    const cred: PlatformCredential = {
      platform_id: platformId,
      api_key: s.anthropic_api_key,
      base_url: s.anthropic_base_url || undefined,
      auth_env_var: s.auth_env_var || undefined,
      ...(platformId === "custom-migrated" ? { name: "Migrated" } : {}),
    };

    try {
      await updateUserSettings({
        platform_credentials: [cred],
        active_platform_id: platformId,
      } as Partial<UserSettings>);
      dbg("layout", "migrated credentials", { platformId });
    } catch (e) {
      dbgWarn("layout", "credential migration failed:", e);
    }
  }

  function handleSetupComplete() {
    showSetupWizard = false;
    loadSettings();
  }

  // Use onMount for initialization (not $effect - avoids accidental reactive tracking)
  onMount(() => {
    // Remove splash screen
    const splash = document.getElementById("app-splash");
    if (splash) {
      splash.style.opacity = "0";
      setTimeout(() => splash.remove(), 300);
    }

    loadRuns();
    loadSettings();
    loadSidebarFavorites();

    // Load saved CWD and pinned folders from localStorage
    const saved = localStorage.getItem("ocv:project-cwd");
    if (saved) projectCwd = saved;
    try {
      const pinned = localStorage.getItem("ocv:pinned-cwds");
      if (pinned) pinnedCwds = JSON.parse(pinned);
    } catch {
      /* ignore parse errors */
    }

    // Poll for runs every 60s (fallback only — primary updates via ocv:runs-changed event)
    const interval = setInterval(loadRuns, 60000);

    // Team store: initial load + poll fallback (60s)
    teamStore.loadTeams();
    const teamPollInterval = setInterval(() => teamStore.loadTeams(), 60000);

    // Team/task event listeners — app-level lifecycle, independent of chat page
    type TeamUpdatePayload = { team_name: string; change: string };
    type TaskUpdatePayload = { team_name: string; task_id: string; change: string };

    let destroyed = false;
    let unlistenTeam: UnlistenFn | undefined;
    let unlistenTask: UnlistenFn | undefined;
    const retryTimers: ReturnType<typeof setTimeout>[] = [];

    // 首次+重试成功后都补偿同步（debounce 300ms）
    let resyncTimer: ReturnType<typeof setTimeout> | undefined;
    function scheduleResync() {
      if (resyncTimer) clearTimeout(resyncTimer);
      resyncTimer = setTimeout(() => {
        if (!destroyed) teamStore.forceRefresh();
      }, 300);
    }

    function registerTeamListener<T>(
      name: string,
      handler: (event: { payload: T }) => void,
      assign: (fn: UnlistenFn) => void,
    ) {
      function tryListen(attempt: number) {
        listen<T>(name, handler)
          .then((fn) => {
            if (destroyed) {
              fn();
              return;
            }
            assign(fn);
            scheduleResync();
          })
          .catch((e) => {
            if (destroyed) return;
            if (attempt < 2) {
              const delay = (attempt + 1) * 2000; // 2s, 4s
              dbgWarn(
                "layout",
                `${name} listen failed (attempt ${attempt + 1}/3), retry in ${delay}ms`,
                e,
              );
              const t = setTimeout(() => tryListen(attempt + 1), delay);
              retryTimers.push(t);
            } else {
              dbgWarn("layout", `${name} listen failed after 3 attempts, falling back to poll`, e);
            }
          });
      }
      tryListen(0);
    }

    registerTeamListener<TeamUpdatePayload>(
      "team-update",
      (event) => {
        dbg("layout", "team-update", event.payload);
        teamStore.handleTeamUpdate(event.payload);
      },
      (fn) => (unlistenTeam = fn),
    );

    registerTeamListener<TaskUpdatePayload>(
      "task-update",
      (event) => {
        dbg("layout", "task-update", event.payload);
        teamStore.handleTaskUpdate(event.payload);
      },
      (fn) => (unlistenTask = fn),
    );

    // Keybinding store: load overrides + CLI bindings, register app-level callbacks
    keybindingStore.loadOverrides();
    keybindingStore.loadCliBindings();
    keybindingStore.registerCallback("app:toggleSidebar", toggleSidebar);
    keybindingStore.registerCallback("app:commandPalette", () => {
      commandPaletteOpen = !commandPaletteOpen;
    });
    keybindingStore.registerCallback("app:newChat", newChat);

    // Immediate refresh when chat page signals a status change
    function onRunsChanged() {
      loadRuns();
      // Also refresh git if on explorer
      if (currentPath?.startsWith("/explorer")) loadGitSummary();
    }
    window.addEventListener("ocv:runs-changed", onRunsChanged);

    // Refresh sidebar favorites when /runs page changes them
    function onFavoritesChanged() {
      loadSidebarFavorites();
    }
    window.addEventListener("ocv:favorites-changed", onFavoritesChanged);

    // Listen for Settings page requesting wizard re-open
    function onShowWizard() {
      showSetupWizard = true;
    }
    window.addEventListener("ocv:show-wizard", onShowWizard);

    // Sync projectCwd when chat page picks a folder via dialog
    function handleCwdChanged() {
      const newCwd = localStorage.getItem("ocv:project-cwd");
      if (newCwd && newCwd !== projectCwd) {
        projectCwd = newCwd;
      }
    }
    window.addEventListener("ocv:cwd-changed", handleCwdChanged);

    return () => {
      clearInterval(interval);
      clearInterval(teamPollInterval);
      if (debounceTimer) clearTimeout(debounceTimer);
      destroyed = true;
      unlistenTeam?.();
      unlistenTask?.();
      retryTimers.forEach(clearTimeout);
      if (resyncTimer) clearTimeout(resyncTimer);
      keybindingStore.unregisterCallback("app:toggleSidebar");
      keybindingStore.unregisterCallback("app:commandPalette");
      keybindingStore.unregisterCallback("app:newChat");
      window.removeEventListener("ocv:runs-changed", onRunsChanged);
      window.removeEventListener("ocv:favorites-changed", onFavoritesChanged);
      window.removeEventListener("ocv:show-wizard", onShowWizard);
      window.removeEventListener("ocv:cwd-changed", handleCwdChanged);
    };
  });

  // Save CWD to localStorage when changed (clear key for "All Projects")
  // Also pin manually-selected folders so they persist in the project list
  $effect(() => {
    if (typeof window !== "undefined") {
      if (projectCwd) {
        localStorage.setItem("ocv:project-cwd", projectCwd);
        // Pin this cwd so it stays in the dropdown after switching away
        if (projectCwd !== "/" && !pinnedCwds.includes(projectCwd)) {
          pinnedCwds = [...pinnedCwds, projectCwd];
          localStorage.setItem("ocv:pinned-cwds", JSON.stringify(pinnedCwds));
        }
      } else {
        localStorage.removeItem("ocv:project-cwd");
      }
      // Notify child pages (e.g. Memory) that project cwd changed
      window.dispatchEvent(new CustomEvent("ocv:project-changed", { detail: { cwd: projectCwd } }));
    }
  });

  afterNavigate(({ to }) => {
    dbg("layout", "navigated to:", to?.url.pathname);
    // Auto-switch sidebar tab when navigating to /teams
    if (to?.url.pathname === "/teams") panelTab = "teams";
    // Sync plugin section from URL when navigating to /plugins
    if (to?.url.pathname.startsWith("/plugins")) {
      const section = to.url.searchParams.get("section");
      if (section && pluginSections.some((s) => s.id === section)) {
        pluginActiveSection = section;
      }
    }
  });

  // Catch unhandled errors that could break the router
  onMount(() => {
    function onError(e: ErrorEvent) {
      dbgWarn("layout", "global error", e.message, e.filename, e.lineno);
    }
    function onRejection(e: PromiseRejectionEvent) {
      dbgWarn("layout", "unhandled rejection", e.reason);
      // Don't call e.preventDefault() — let rejections surface in devtools
    }
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  });

  // Get selected run from URL
  let selectedRunId = $derived.by(() => {
    const url = $page.url;
    return url.searchParams.get("run") ?? "";
  });

  // Derive project list from runs + pinned folders
  // Runs with empty or "/" cwd are "unscoped" — folded into "All Projects"
  // Pinned folders (from "Open folder..." or previous selections) always appear
  let projects = $derived.by(() => {
    const cwdMap = new Map<string, { count: number; latestAt: string }>();
    for (const run of runs) {
      const cwd = run.cwd || "";
      if (!cwd || cwd === "/") continue;
      const existing = cwdMap.get(cwd);
      if (!existing || run.started_at > existing.latestAt) {
        cwdMap.set(cwd, { count: (existing?.count ?? 0) + 1, latestAt: run.started_at });
      } else {
        cwdMap.set(cwd, { ...existing, count: existing.count + 1 });
      }
    }
    // Include pinned cwds not already present from runs
    for (const cwd of pinnedCwds) {
      if (cwd && cwd !== "/" && !cwdMap.has(cwd)) {
        cwdMap.set(cwd, { count: 0, latestAt: "" });
      }
    }
    return Array.from(cwdMap.entries())
      .sort((a, b) => b[1].latestAt.localeCompare(a[1].latestAt))
      .map(([cwd, info]) => ({
        cwd,
        label: cwdDisplayLabel(cwd),
        runCount: info.count,
      }));
  });

  // Filter runs by selected project cwd + text search
  let filteredRuns = $derived.by(() => {
    let result = projectCwd ? runs.filter((r) => r.cwd === projectCwd) : runs;
    if (runSearchQuery.trim()) {
      const q = runSearchQuery.trim().toLowerCase();
      result = result.filter(
        (r) =>
          r.prompt.toLowerCase().includes(q) ||
          (r.name && r.name.toLowerCase().includes(q)) ||
          r.agent.toLowerCase().includes(q) ||
          r.status.toLowerCase().includes(q),
      );
    }
    return result;
  });

  // Split sidebar runs into pinned (favorited) and normal
  let pinnedRuns = $derived(filteredRuns.filter((r) => favoriteRunIds.has(r.id)));
  let normalRuns = $derived(filteredRuns.filter((r) => !favoriteRunIds.has(r.id)));

  // Current page detection
  let currentPath = $derived($page.url.pathname);
  let isChatPage = $derived(currentPath === "/chat" || currentPath === "/");
  let isPluginsPage = $derived(currentPath.startsWith("/plugins"));
  let isExplorerPage = $derived(currentPath.startsWith("/explorer"));

  // Plugin sidebar navigation (shown when on /plugins route)
  const pluginSections = [
    { id: "skills", label: () => t("sidebar_skills"), icon: "sparkles" },
    { id: "mcp", label: () => t("sidebar_mcpServers"), icon: "server" },
    { id: "hooks", label: () => t("sidebar_hooks"), icon: "webhook" },
    { id: "plugins", label: () => t("sidebar_plugins"), icon: "package" },
    { id: "agents", label: () => t("sidebar_agents"), icon: "agents" },
  ];

  let pluginActiveSection = $state<string>("skills");
  setContext("pluginSection", {
    get active() {
      return pluginActiveSection;
    },
    set active(v: string) {
      pluginActiveSection = v;
    },
  });

  // Breadcrumb for non-chat pages
  let pageName = $derived(
    navItems.find((n) => currentPath.startsWith(n.path))?.label() ?? t("layout_appName"),
  );

  function newChat() {
    goto("/chat");
  }

  function handleProjectChange(_cwd: string) {
    runSearchQuery = "";
    // Navigate to clean chat when switching projects (clears current run)
    if (isChatPage) goto("/chat");
  }

  async function pickFolder() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, title: t("layout_selectProjectFolder") });
      if (selected) projectCwd = selected as string;
    } catch (e) {
      dbgWarn("layout", "failed to open folder dialog:", e);
    }
  }

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen;
  }

  setContext("toggleSidebar", toggleSidebar);

  function cycleTheme() {
    const order: ThemeMode[] = ["dark", "light", "system"];
    const idx = order.indexOf(themeMode);
    themeMode = order[(idx + 1) % order.length];
    dbg("layout", "theme cycled", { themeMode, effectiveDark });
  }

  // Persist theme + apply class
  $effect(() => {
    localStorage.setItem("ocv:theme", themeMode);
    document.documentElement.classList.toggle("dark", effectiveDark);
  });

  // Note: <html lang> is set by initLocale() and switchLocale() directly.

  // Listen for system preference changes
  onMount(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    function onSystemChange(e: MediaQueryListEvent) {
      systemDark = e.matches;
    }
    mq.addEventListener("change", onSystemChange);
    // Apply initial theme
    document.documentElement.classList.toggle("dark", effectiveDark);
    return () => mq.removeEventListener("change", onSystemChange);
  });

  function handleKeydown(e: KeyboardEvent) {
    keybindingStore.dispatch(e);
  }
</script>

{#snippet treeNodes(nodes: TreeNode[])}
  {#each nodes as node}
    <button
      class="flex w-full items-center gap-1 py-0.5 text-[13px] transition-colors
        text-sidebar-foreground hover:bg-sidebar-accent/50
        {explorerSelectedFile === node.fullPath ? 'bg-sidebar-accent/70' : ''}"
      style="padding-left: {8 + node.depth * 12}px"
      onclick={() => (node.is_dir ? toggleFolder(node) : selectFile(node))}
    >
      {#if node.is_dir}
        <svg
          class="h-3 w-3 shrink-0 transition-transform duration-150 {node.expanded
            ? 'rotate-90'
            : ''}"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
        >
        <svg
          class="h-3.5 w-3.5 shrink-0 text-blue-400/70"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><path
            d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
          /></svg
        >
      {:else}
        <span class="w-3 shrink-0"></span>
        <svg
          class="h-3.5 w-3.5 shrink-0 opacity-40"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path
            d="M14 2v4a2 2 0 0 0 2 2h4"
          /></svg
        >
      {/if}
      <span class="min-w-0 truncate">{node.name}</span>
    </button>
    {#if node.is_dir && node.expanded}
      {@render treeNodes(node.children)}
    {/if}
  {/each}
{/snippet}

<svelte:window onkeydown={handleKeydown} />

<div class="flex h-screen overflow-hidden">
  <!-- Sidebar: Icon Rail + Content Panel -->
  {#if sidebarOpen}
    <aside class="flex shrink-0 bg-sidebar text-sidebar-foreground transition-all duration-200">
      <!-- A. Icon Rail -->
      <div
        class="flex w-[52px] flex-col items-center border-r border-sidebar-border bg-black/[0.03] dark:bg-black/20"
      >
        <!-- Rail logo (OC) -->
        <div class="flex h-14 w-full items-center justify-center border-b border-sidebar-border">
          <img src="/logo.png?v=2" alt="OC" class="h-8 w-8 rounded-lg" />
        </div>

        <!-- Rail nav icons -->
        <nav class="flex flex-1 flex-col items-center gap-1 py-2">
          {#each navItems as item}
            {@const isActive = currentPath.startsWith(item.path)}
            <a
              href={item.path}
              class="relative flex h-auto w-11 flex-col items-center justify-center gap-0.5 rounded-md py-1 transition-colors duration-150 no-underline
                {isActive
                ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                : 'hover:bg-sidebar-accent/50 text-sidebar-foreground'}"
              title={item.label()}
            >
              <!-- Active indicator bar -->
              {#if isActive}
                <span class="absolute left-0 top-1.5 h-5 w-[3px] rounded-r-full bg-primary"></span>
              {/if}
              {#if item.icon === "message"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"><path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z" /></svg
                >
              {:else if item.icon === "folder"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><path
                    d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
                  /></svg
                >
              {:else if item.icon === "zap"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" /></svg
                >
              {:else if item.icon === "book"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20" /></svg
                >
              {:else if item.icon === "chart"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"><path d="M3 3v18h18" /><path d="m19 9-5 5-4-4-3 3" /></svg
                >
              {:else if item.icon === "settings"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><path
                    d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
                  /><circle cx="12" cy="12" r="3" /></svg
                >
              {/if}
              <span class="text-[9px] leading-tight text-center truncate w-full px-0.5"
                >{item.label()}</span
              >
            </a>
          {/each}
        </nav>

        <!-- Rail version + locale + dark mode toggle -->
        <div class="border-t border-sidebar-border py-2">
          <div class="flex items-center justify-center pb-1">
            <button
              class="text-[8px] text-muted-foreground/50 hover:text-muted-foreground transition-colors cursor-pointer"
              onclick={() => (showAbout = true)}
              title="About OpenCovibe">v0.1</button
            >
          </div>
          <div class="relative mx-auto mb-0.5">
            <button
              class="flex h-9 w-9 items-center justify-center rounded-md text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors duration-150"
              onclick={() => (localePopupOpen = !localePopupOpen)}
              title={currentLocale()}
            >
              <span class="text-xs font-medium"
                >{getEntry(currentLocale())?.shortLabel ?? currentLocale()}</span
              >
            </button>
            {#if localePopupOpen}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="fixed inset-0 z-40"
                onclick={() => (localePopupOpen = false)}
                onkeydown={(e) => e.key === "Escape" && (localePopupOpen = false)}
              ></div>
              <div
                class="absolute bottom-0 left-full ml-1 z-50 min-w-[140px] rounded-md border border-sidebar-border bg-popover py-1 shadow-lg"
              >
                {#each LOCALE_REGISTRY as entry}
                  <button
                    class="flex w-full items-center gap-2 px-3 py-1.5 text-xs transition-colors
                      {currentLocale() === entry.code
                      ? 'bg-accent text-accent-foreground'
                      : 'text-popover-foreground hover:bg-accent/50'}"
                    onclick={() => handleLocaleSelect(entry.code)}
                  >
                    <span class="w-5 text-center font-medium">{entry.shortLabel}</span>
                    <span>{entry.nativeName}</span>
                    {#if entry.status === "beta"}
                      <span
                        class="ml-auto text-[10px] text-muted-foreground/60 border border-muted-foreground/20 rounded px-1"
                        >Beta</span
                      >
                    {/if}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
          <button
            class="flex h-9 w-9 items-center justify-center rounded-md text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors duration-150"
            onclick={cycleTheme}
            title={themeMode === "dark"
              ? t("layout_themeTitle_dark")
              : themeMode === "light"
                ? t("layout_themeTitle_light")
                : t("layout_themeTitle_system")}
          >
            {#if themeMode === "dark"}
              <!-- Moon icon (dark mode active) -->
              <svg
                class="h-[18px] w-[18px]"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" /></svg
              >
            {:else if themeMode === "light"}
              <!-- Sun icon (light mode active) -->
              <svg
                class="h-[18px] w-[18px]"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                ><circle cx="12" cy="12" r="4" /><path
                  d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"
                /></svg
              >
            {:else}
              <!-- Monitor icon (system mode active) -->
              <svg
                class="h-[18px] w-[18px]"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                ><rect width="20" height="14" x="2" y="3" rx="2" /><line
                  x1="8"
                  x2="16"
                  y1="21"
                  y2="21"
                /><line x1="12" x2="12" y1="17" y2="21" /></svg
              >
            {/if}
          </button>
        </div>
      </div>

      <!-- B. Content Panel -->
      <div class="flex w-[280px] flex-col overflow-hidden border-r border-sidebar-border">
        <!-- Panel header: Project selector + new chat -->
        <div class="flex h-14 items-center gap-1.5 border-b border-sidebar-border px-3">
          <div class="flex-1 min-w-0">
            <ProjectSelector
              {projects}
              bind:value={projectCwd}
              onOpenFolder={pickFolder}
              onchange={handleProjectChange}
            />
          </div>
          <button
            class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors duration-150"
            onclick={() => (showCliBrowser = true)}
            title={t("cliSync_title")}
          >
            <svg
              class="h-4 w-4"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              ><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline
                points="7 10 12 15 17 10"
              /><line x1="12" x2="12" y1="15" y2="3" /></svg
            >
          </button>
          <button
            class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors duration-150"
            onclick={newChat}
            title={t("layout_newConversation")}
          >
            <svg
              class="h-4 w-4"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d="M12 5v14" /><path d="M5 12h14" /></svg
            >
          </button>
        </div>

        {#if isPluginsPage}
          <!-- Plugin section navigation (replaces Chats/Files when on /plugins) -->
          <div class="flex-1 overflow-y-auto py-2">
            {#each pluginSections as section}
              {@const isActive = pluginActiveSection === section.id}
              <button
                class="flex w-full items-center gap-2 py-2 px-3 text-[11px] font-medium transition-colors
                  {isActive
                  ? 'bg-sidebar-accent text-sidebar-foreground'
                  : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
                onclick={() => {
                  pluginActiveSection = section.id;
                  goto(`/plugins?section=${section.id}`, { replaceState: true, noScroll: true });
                }}
              >
                {#if section.icon === "sparkles"}
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><path
                      d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"
                    /></svg
                  >
                {:else if section.icon === "server"}
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><rect width="20" height="8" x="2" y="2" rx="2" ry="2" /><rect
                      width="20"
                      height="8"
                      x="2"
                      y="14"
                      rx="2"
                      ry="2"
                    /><line x1="6" x2="6.01" y1="6" y2="6" /><line
                      x1="6"
                      x2="6.01"
                      y1="18"
                      y2="18"
                    /></svg
                  >
                {:else if section.icon === "webhook"}
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><path
                      d="M18 16.98h-5.99c-1.1 0-1.95.94-2.48 1.9A4 4 0 0 1 2 17c.01-.7.2-1.4.57-2"
                    /><path d="m6 17 3.13-5.78c.53-.97.1-2.18-.5-3.1a4 4 0 1 1 6.89-4.06" /><path
                      d="m12 6 3.13 5.73C15.66 12.7 16.9 13 18 13a4 4 0 0 1 0 8H12"
                    /></svg
                  >
                {:else if section.icon === "package"}
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><path d="m7.5 4.27 9 5.15" /><path
                      d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"
                    /><path d="m3.3 7 8.7 5 8.7-5" /><path d="M12 22V12" /></svg
                  >
                {:else if section.icon === "agents"}
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><path d="M12 8V4H8" /><rect width="16" height="12" x="4" y="8" rx="2" /><path
                      d="M2 14h2"
                    /><path d="M20 14h2" /><path d="M15 13v2" /><path d="M9 13v2" /></svg
                  >
                {/if}
                <span class="min-w-0 truncate">{section.label()}</span>
              </button>
            {/each}
          </div>
        {:else if isExplorerPage}
          <!-- Explorer tab bar: Files / Git -->
          <div class="flex shrink-0 border-b border-sidebar-border">
            <button
              class="flex-1 py-1.5 text-[11px] font-medium text-center transition-colors
              {explorerTab === 'files'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (explorerTab = "files")}>{t("sidebar_files")}</button
            >
            <button
              class="relative flex-1 py-1.5 text-[11px] font-medium text-center transition-colors
              {explorerTab === 'git'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (explorerTab = "git")}
              >{t("sidebar_git")}
              {#if gitSummary && gitSummary.total_files > 0}
                <span
                  class="ml-0.5 inline-flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-blue-500/80 px-1 text-[8px] font-bold text-white"
                  >{gitSummary.total_files}</span
                >
              {/if}
            </button>
          </div>

          <!-- Explorer tab content -->
          {#if explorerTab === "files"}
            <div class="flex-1 overflow-y-auto px-1 py-1">
              {#if !projectCwd}
                <div class="flex items-center justify-center px-3 py-12">
                  <p class="text-xs text-muted-foreground text-center">
                    {t("sidebar_selectProjectBrowse")}
                  </p>
                </div>
              {:else if treeLoading}
                <div class="flex items-center justify-center py-12">
                  <div
                    class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                  ></div>
                </div>
              {:else if fileTree.length === 0}
                <p class="px-2 py-8 text-xs text-muted-foreground text-center">
                  {t("sidebar_emptyDirectory")}
                </p>
              {:else}
                {@render treeNodes(fileTree)}
              {/if}
            </div>
          {:else}
            <!-- Git tab -->
            {#if !projectCwd}
              <div class="flex-1 flex items-center justify-center px-3">
                <p class="text-xs text-muted-foreground text-center">
                  {t("sidebar_selectProjectGit")}
                </p>
              </div>
            {:else if gitLoading}
              <div class="flex-1 flex items-center justify-center">
                <div
                  class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                ></div>
              </div>
            {:else if !gitSummary}
              <div class="flex-1 flex items-center justify-center px-3">
                <p class="text-xs text-muted-foreground text-center">{t("sidebar_notGitRepo")}</p>
              </div>
            {:else}
              <!-- Branch info -->
              <div
                class="flex items-center gap-1.5 px-3 py-2 border-b border-sidebar-border shrink-0"
              >
                <svg
                  class="h-3 w-3 shrink-0 text-muted-foreground"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><circle cx="12" cy="12" r="3" /><line x1="3" x2="9" y1="12" y2="12" /><line
                    x1="15"
                    x2="21"
                    y1="12"
                    y2="12"
                  /></svg
                >
                <span class="text-[12px] font-medium text-sidebar-foreground min-w-0 truncate"
                  >{gitSummary.branch || t("sidebar_detached")}</span
                >
                <button
                  class="ml-auto flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
                  onclick={loadGitSummary}
                  title={t("sidebar_refresh")}
                >
                  <svg
                    class="h-3 w-3"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path
                      d="M3 3v5h5"
                    /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path
                      d="M16 16h5v5"
                    /></svg
                  >
                </button>
              </div>
              <!-- Summary -->
              {#if gitSummary.total_files > 0}
                <div
                  class="flex items-center gap-2 px-3 py-1.5 text-[11px] text-muted-foreground border-b border-sidebar-border shrink-0"
                >
                  <span class="tabular-nums"
                    >{gitSummary.total_files !== 1
                      ? t("sidebar_changedFiles", { count: String(gitSummary.total_files) })
                      : t("sidebar_changedFile", { count: String(gitSummary.total_files) })}</span
                  >
                  {#if gitSummary.total_insertions > 0}
                    <span class="text-green-500 tabular-nums">+{gitSummary.total_insertions}</span>
                  {/if}
                  {#if gitSummary.total_deletions > 0}
                    <span class="text-red-400 tabular-nums">-{gitSummary.total_deletions}</span>
                  {/if}
                </div>
                <!-- Changed files list -->
                <div class="flex-1 overflow-y-auto">
                  {#each gitSummary.files as file}
                    <button
                      class="flex w-full items-center gap-1.5 px-3 py-1 text-[12px] hover:bg-sidebar-accent/50 transition-colors"
                      onclick={() => selectDiffFile(file.path)}
                    >
                      <span
                        class="w-3 shrink-0 text-center font-mono text-[10px] font-bold {GIT_STATUS_COLORS[
                          file.status
                        ] ?? 'text-muted-foreground'}">{file.status}</span
                      >
                      <span class="flex-1 min-w-0 truncate text-sidebar-foreground text-left"
                        >{file.path}</span
                      >
                      {#if file.insertions > 0}
                        <span class="text-[10px] text-green-500">+{file.insertions}</span>
                      {/if}
                      {#if file.deletions > 0}
                        <span class="text-[10px] text-red-400">-{file.deletions}</span>
                      {/if}
                    </button>
                  {/each}
                </div>
              {:else}
                <div class="flex-1 flex items-center justify-center px-3">
                  <div class="flex flex-col items-center gap-1.5 text-center">
                    <svg
                      class="h-6 w-6 text-muted-foreground/30"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"><path d="M20 6 9 17l-5-5" /></svg
                    >
                    <p class="text-xs text-muted-foreground">{t("sidebar_workingTreeClean")}</p>
                  </div>
                </div>
              {/if}
            {/if}
          {/if}
        {:else}
          <!-- Tab bar -->
          <div class="flex shrink-0 border-b border-sidebar-border">
            <button
              class="flex-1 py-1.5 text-[11px] font-medium text-center transition-colors
              {panelTab === 'chats'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (panelTab = "chats")}>{t("sidebar_chats")}</button
            >
            <button
              class="relative flex-1 py-1.5 text-[11px] font-medium text-center transition-colors
              {panelTab === 'teams'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (panelTab = "teams")}
              >{t("sidebar_teams")}
              {#if teamStore.teams.length > 0}
                <span
                  class="ml-0.5 inline-flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-teal-500/80 px-1 text-[8px] font-bold text-white"
                  >{teamStore.teams.length}</span
                >
              {/if}
            </button>
          </div>

          <!-- Tab content -->
          {#if panelTab === "chats"}
            <div class="px-2 pt-2 pb-1 shrink-0">
              <input
                type="text"
                bind:value={runSearchQuery}
                oninput={onDeepQueryInput}
                placeholder={t("sidebar_searchChats")}
                class="w-full rounded-md border border-sidebar-border bg-sidebar px-2 py-1 text-xs text-sidebar-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:border-ring/50"
              />
              {#if runSearchQuery.trim()}
                {#if searching}
                  <p class="text-[10px] text-muted-foreground px-1 pt-0.5">
                    {t("runs_searching")}
                  </p>
                {:else if searchResults.length > 0}
                  <p class="text-[10px] text-muted-foreground px-1 pt-0.5">
                    {t("runs_resultsCount", { count: String(searchResults.length) })}
                  </p>
                {/if}
              {/if}
            </div>

            {#if runSearchQuery.trim()}
              <!-- Search results -->
              <div class="flex-1 overflow-y-auto">
                {#if searching && searchResults.length === 0}
                  <div class="flex items-center justify-center py-10">
                    <div
                      class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                    ></div>
                  </div>
                {:else if !searching && searchResults.length === 0}
                  <div class="flex items-center justify-center px-3 py-10 text-center">
                    <p class="text-xs text-muted-foreground">{t("runs_noMatching")}</p>
                  </div>
                {:else}
                  {#each searchResults as result}
                    <a
                      href="/chat?run={result.runId}"
                      class="flex flex-col gap-0.5 px-3 py-2 hover:bg-sidebar-accent/50 transition-colors no-underline text-sidebar-foreground"
                    >
                      <p class="text-[12px] min-w-0 truncate">
                        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                        {@html highlightMatch(truncate(result.matchedText, 60), runSearchQuery)}
                      </p>
                      <div
                        class="flex items-center gap-1 text-[10px] text-muted-foreground min-w-0"
                      >
                        <span class="flex-1 min-w-0 truncate"
                          >{result.runName || truncate(result.runPrompt, 30)}</span
                        >
                        <span class="ml-auto shrink-0">{relativeTime(result.matchedTs)}</span>
                      </div>
                    </a>
                  {/each}
                {/if}
              </div>
            {:else}
              <!-- Run list -->
              <div class="flex-1 overflow-y-auto px-2 py-1">
                {#if pinnedRuns.length > 0}
                  <div
                    class="text-[10px] font-medium text-muted-foreground/60 uppercase tracking-wider px-3 pt-1 pb-0.5"
                  >
                    {t("sidebar_favorites")}
                  </div>
                  {#each pinnedRuns as run}
                    <RunListItem
                      {run}
                      selected={run.id === selectedRunId}
                      isFork={!!run.parent_run_id}
                      favorite={true}
                      onclick={() => goto(`/chat?run=${run.id}`)}
                      onresume={(runId, mode) => goto(`/chat?run=${runId}&resume=${mode}`)}
                    />
                  {/each}
                {/if}
                {#each normalRuns as run}
                  <RunListItem
                    {run}
                    selected={run.id === selectedRunId}
                    isFork={!!run.parent_run_id}
                    onclick={() => goto(`/chat?run=${run.id}`)}
                    onresume={(runId, mode) => goto(`/chat?run=${runId}&resume=${mode}`)}
                  />
                {/each}
                {#if filteredRuns.length === 0}
                  <div class="flex flex-col items-center gap-2 px-3 py-6 text-center">
                    <p class="text-xs text-muted-foreground">
                      {#if projectCwd && runs.length > 0}
                        {t("sidebar_noConversationsProject")}
                      {:else}
                        {t("sidebar_noConversationsYet")}<br />{t("sidebar_startNewChat")}
                      {/if}
                    </p>
                  </div>
                {/if}
              </div>
            {/if}
          {:else if panelTab === "teams"}
            <!-- Teams list in sidebar -->
            <div class="flex-1 overflow-y-auto px-2 py-1">
              {#if teamStore.loading}
                <div class="flex items-center justify-center py-6">
                  <div
                    class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                  ></div>
                </div>
              {:else if teamStore.teams.length === 0}
                <div class="flex flex-col items-center gap-1 px-3 py-6 text-center">
                  <p class="text-xs text-muted-foreground">{t("sidebar_noActiveTeams")}</p>
                  <p class="text-[10px] text-muted-foreground/60">{t("sidebar_startTeamHint")}</p>
                </div>
              {:else}
                {#each teamStore.teams as team}
                  <button
                    class="flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left transition-colors mb-0.5
                        {teamStore.selectedTeam === team.name
                      ? 'bg-sidebar-accent text-sidebar-foreground'
                      : 'hover:bg-sidebar-accent/50 text-sidebar-foreground'}"
                    onclick={() => {
                      teamStore.selectTeam(team.name);
                      goto("/teams");
                    }}
                  >
                    <div class="flex items-center gap-1.5">
                      <span class="h-2 w-2 rounded-full bg-teal-500 shrink-0"></span>
                      <span class="text-[13px] font-medium min-w-0 truncate">{team.name}</span>
                    </div>
                    {#if team.description}
                      <p class="text-[10px] text-muted-foreground truncate pl-3.5">
                        {team.description}
                      </p>
                    {/if}
                    <div
                      class="flex items-center gap-2 pl-3.5 text-[10px] text-muted-foreground/60"
                    >
                      <span>{t("sidebar_members", { count: String(team.member_count) })}</span>
                      <span>{t("sidebar_tasks", { count: String(team.task_count) })}</span>
                    </div>
                  </button>
                {/each}
              {/if}
            </div>
          {/if}
        {/if}
      </div>
    </aside>
  {/if}

  <!-- Main content -->
  <div class="flex flex-1 flex-col overflow-hidden">
    <UpdateBanner />
    <!-- Top bar (non-chat pages only — chat uses SessionStatusBar) -->
    {#if !isChatPage}
      <header class="flex h-14 items-center gap-3 border-b px-4">
        <button
          class="rounded-md p-1.5 hover:bg-accent transition-all duration-150"
          onclick={toggleSidebar}
          title={t("layout_toggleSidebar")}
        >
          <svg
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><rect width="18" height="18" x="3" y="3" rx="2" /><path d="M9 3v18" /></svg
          >
        </button>

        <div class="flex items-center gap-2 text-sm">
          <span class="text-muted-foreground">{t("layout_appName")}</span>
          <svg
            class="h-3.5 w-3.5 text-muted-foreground/50"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="m9 18 6-6-6-6" /></svg
          >
          <span class="font-medium">{pageName}</span>
        </div>
      </header>
    {/if}

    <!-- Page content -->
    <main class="flex-1 overflow-y-auto">
      {@render children()}
    </main>
  </div>
</div>

<CommandPalette
  bind:open={commandPaletteOpen}
  cwd={projectCwd || "/"}
  onOpenFolderBrowser={pickFolder}
/>

{#if showSetupWizard}
  <SetupWizard onComplete={handleSetupComplete} />
{/if}

<AboutModal bind:open={showAbout} />

{#if showCliBrowser}
  <CliSessionBrowser
    cwd={projectCwd || "/"}
    onclose={() => (showCliBrowser = false)}
    onimported={(runId) => {
      showCliBrowser = false;
      loadRuns();
      goto(`/chat?run=${runId}`);
    }}
  />
{/if}

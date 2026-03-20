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
    listMemoryFiles,
    softDeleteRuns,
  } from "$lib/api";
  import ProjectFolderItem from "$lib/components/ProjectFolderItem.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";
  import SetupWizard from "$lib/components/SetupWizard.svelte";
  import AboutModal from "$lib/components/AboutModal.svelte";
  import PermissionsModal from "$lib/components/PermissionsModal.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import CliSessionBrowser from "$lib/components/CliSessionBrowser.svelte";
  import UpdateBanner from "$lib/components/UpdateBanner.svelte";
  import type {
    TaskRun,
    UserSettings,
    DirEntry,
    GitSummary,
    PromptFavorite,
    PromptSearchResult,
    MemoryFileCandidate,
  } from "$lib/types";
  import { cwdDisplayLabel, truncate, snippetAround, relativeTime } from "$lib/utils/format";
  import { filterVisibleCandidates } from "$lib/utils/memory-helpers";
  import {
    buildProjectFolders,
    autoExpandForRun,
    expandForProjectChange,
    normalizeCwd,
    type ConversationGroup,
  } from "$lib/utils/sidebar-groups";
  import { loadRemovedCwds } from "$lib/utils/removed-cwds";
  import { page } from "$app/stores";
  import { goto, afterNavigate } from "$app/navigation";
  import { onMount, setContext, untrack } from "svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { PLATFORM_PRESETS } from "$lib/utils/platform-presets";
  import type { PlatformCredential } from "$lib/types";
  import { TeamStore } from "$lib/stores/team-store.svelte";
  import { KeybindingStore } from "$lib/stores/keybindings.svelte";
  import { getTransport } from "$lib/transport";
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
  let permissionsModalOpen = $state(false);

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
  type ColorScheme = "warm" | "neutral";

  function getInitialTheme(): ThemeMode {
    if (typeof window === "undefined") return "dark";
    const saved = localStorage.getItem("ocv:theme");
    if (saved === "light" || saved === "dark" || saved === "system") return saved;
    return "dark";
  }

  function getInitialScheme(): ColorScheme {
    if (typeof window === "undefined") return "warm";
    const saved = localStorage.getItem("ocv:colorScheme");
    return saved === "neutral" ? "neutral" : "warm";
  }

  let themeMode = $state<ThemeMode>(getInitialTheme());
  let colorScheme = $state<ColorScheme>(getInitialScheme());
  let systemDark = $state(
    typeof window !== "undefined"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : true,
  );
  let effectiveDark = $derived(themeMode === "system" ? systemDark : themeMode === "dark");
  let pinnedCwds = $state<string[]>([]);
  let removedCwds = $state<string[]>([]);

  let panelTab = $state<"chats" | "teams">("chats");
  let runSearchQuery = $state("");

  // ── Folder tree state ──
  let expandedProjects = $state<Set<string>>(new Set());
  let runsLoadSucceededOnce = $state(false);

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
  let explorerProjectOpen = $state(false);

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

  let _treeSeq = 0;
  async function loadRootTree() {
    if (!projectCwd) {
      fileTree = [];
      return;
    }
    const seq = ++_treeSeq;
    treeLoading = true;
    try {
      const listing = await listDirectory(projectCwd, true);
      if (seq !== _treeSeq) return; // stale response, discard
      fileTree = entriesToNodes(listing.entries, projectCwd, 0);
      dbg("layout", "file tree loaded", { count: fileTree.length });
    } catch (e) {
      if (seq !== _treeSeq) return;
      dbgWarn("layout", "file tree load error", e);
      fileTree = [];
    } finally {
      if (seq === _treeSeq) treeLoading = false;
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

  let _gitSeq = 0;
  let _gitLoadedCwd = "";
  async function loadGitSummary() {
    if (!projectCwd) {
      gitSummary = null;
      _gitLoadedCwd = "";
      return;
    }
    const requestedCwd = projectCwd;
    const seq = ++_gitSeq;
    gitLoading = true;
    try {
      const result = await getGitSummary(requestedCwd);
      if (seq !== _gitSeq) return; // stale response, discard
      gitSummary = result;
      _gitLoadedCwd = requestedCwd;
      dbg("layout", "git summary loaded", {
        branch: result.branch,
        files: result.total_files,
      });
    } catch (e) {
      if (seq !== _gitSeq) return;
      dbgWarn("layout", "git summary load error", e);
      gitSummary = null;
      _gitLoadedCwd = "";
    } finally {
      if (seq === _gitSeq) gitLoading = false;
    }
  }

  function selectDiffFile(filePath: string) {
    // Notify explorer page to show diff
    window.dispatchEvent(new CustomEvent("ocv:explorer-diff", { detail: { path: filePath } }));
  }

  // ── Memory sidebar state (shown when on /memory) ──
  let memoryCandidates = $state<MemoryFileCandidate[]>([]);
  let memorySelectedFile = $state("");
  let memoryLoading = $state(false);
  let memoryScopeExpanded = $state<Record<string, boolean>>({
    global: false,
  });

  let memoryScopeProject = $derived(memoryCandidates.filter((c) => c.scope === "project"));
  let memoryScopeGlobal = $derived(memoryCandidates.filter((c) => c.scope === "global"));
  let memoryScopeMemory = $derived(memoryCandidates.filter((c) => c.scope === "memory"));
  // Merged project + auto memory for flat folder view
  let memoryScopeFolder = $derived([...memoryScopeProject, ...memoryScopeMemory]);

  let memoryCandidateSeq = 0;

  async function loadMemoryCandidates(opts?: { soft?: boolean }) {
    const seq = ++memoryCandidateSeq;
    if (!opts?.soft) memoryLoading = true;
    try {
      const result = await listMemoryFiles(projectCwd || undefined);
      if (seq !== memoryCandidateSeq) return; // stale — discard
      memoryCandidates = result;
      dbg("layout", "memory candidates loaded", {
        count: result.length,
        existing: result.filter((f) => f.exists).length,
      });
    } catch (e) {
      if (seq !== memoryCandidateSeq) return;
      if (opts?.soft) {
        dbgWarn("layout", "soft memory refresh failed, keeping old data", e);
      } else {
        dbgWarn("layout", "memory candidates load error", e);
        memoryCandidates = [];
      }
    } finally {
      if (seq === memoryCandidateSeq) memoryLoading = false;
    }
  }

  function selectMemoryFile(file: MemoryFileCandidate) {
    // Don't set highlight immediately — page will confirm dirty state first.
    // If confirmed, page sends ocv:memory-file-selected to ack the switch.
    window.dispatchEvent(
      new CustomEvent("ocv:memory-select", { detail: { path: file.path, exists: file.exists } }),
    );
  }

  function toggleMemoryScope(scope: string) {
    memoryScopeExpanded = { ...memoryScopeExpanded, [scope]: !memoryScopeExpanded[scope] };
  }

  // Load tree when switching to explorer page or changing project
  // Git summary is lazy-loaded when user clicks the Git tab (see below)
  let _prevExplorerCwd: string | undefined;
  $effect(() => {
    const _path = currentPath;
    const _cwd = projectCwd;
    if (_path?.startsWith("/explorer")) {
      if (_cwd) {
        loadRootTree();
        // Invalidate git cache when cwd changes so Git tab reloads on next switch
        if (_prevExplorerCwd !== undefined && _prevExplorerCwd !== _cwd) {
          ++_gitSeq; // cancel in-flight request so it can't backfill _gitLoadedCwd
          gitLoading = false;
          _gitLoadedCwd = "";
        }
        _prevExplorerCwd = _cwd;
      } else {
        // Increment seq to invalidate any in-flight requests
        ++_treeSeq;
        ++_gitSeq;
        // Clear state
        fileTree = [];
        gitSummary = null;
        gitLoading = false;
        treeLoading = false;
        _gitLoadedCwd = "";
        _prevExplorerCwd = _cwd;
      }
    }
  });

  // Lazy-load git summary when user switches to Git tab (only on Explorer page)
  $effect(() => {
    if (
      currentPath?.startsWith("/explorer") &&
      explorerTab === "git" &&
      projectCwd &&
      _gitLoadedCwd !== projectCwd
    ) {
      loadGitSummary();
    }
  });

  // Load memory candidates when switching to memory page or changing project
  let _prevMemoryCwd: string | undefined;
  $effect(() => {
    const _path = currentPath;
    const _cwd = projectCwd;
    if (_path?.startsWith("/memory")) {
      const cwdChanged = _cwd !== _prevMemoryCwd;
      _prevMemoryCwd = _cwd;
      if (cwdChanged) {
        // Only clear project scope, keep Global/Memory to avoid visual jitter
        // Use untrack to read memoryCandidates without adding it as a dependency
        memoryCandidates = untrack(() => memoryCandidates).filter((c) => c.scope !== "project");
      }
      loadMemoryCandidates();
    }
  });

  // Navigation items (declared before pageName derivation)
  const navItems = [
    { path: "/chat", label: () => t("nav_chat"), icon: "message" },
    { path: "/explorer", label: () => t("nav_explorer"), icon: "folder" },
    { path: "/plugins", label: () => t("nav_extend"), icon: "zap" },
    { path: "/memory", label: () => t("nav_memory"), icon: "book" },
    { path: "/usage", label: () => t("nav_usage"), icon: "chart" },
    { path: "/history", label: () => t("nav_history"), icon: "clock" },
    { path: "/settings", label: () => t("nav_settings"), icon: "settings" },
  ];

  // Load initial data
  async function loadRuns() {
    try {
      runs = await listRuns();
      runsLoadSucceededOnce = true;
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
      const normalizedWd = normalizeCwd(settings.working_directory);
      if (normalizedWd) {
        localStorage.setItem("ocv:settings-cwd", normalizedWd);
        if (!projectCwd) projectCwd = normalizedWd;
      } else {
        localStorage.removeItem("ocv:settings-cwd");
      }
      // Show setup wizard if onboarding not completed
      if (!settings.onboarding_completed) {
        showSetupWizard = true;
      }
      // One-time migration: if platform_credentials is empty but api_key exists,
      // create an initial credential from current settings
      await migrateCredentialsIfNeeded(settings);
      applyZoom(settings.ui_zoom);
    } catch {
      // Silently fail
    }
  }

  function applyZoom(zoom?: number) {
    const factor = Math.min(1.5, Math.max(0.75, zoom ?? 1.0));
    import("@tauri-apps/api/webviewWindow")
      .then(({ getCurrentWebviewWindow }) => {
        getCurrentWebviewWindow()
          .setZoom(factor)
          .then(() => dbg("layout", "applyZoom success", { factor }))
          .catch((e) => dbgWarn("layout", "setZoom failed", e));
      })
      .catch((e) => dbgWarn("layout", "import webviewWindow failed", e));
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
    if (saved) projectCwd = normalizeCwd(saved) || "";

    // Load expanded projects from localStorage (defensive parse)
    try {
      const rawExpanded = localStorage.getItem("ocv:expanded-projects");
      if (rawExpanded) {
        const parsed = JSON.parse(rawExpanded);
        if (Array.isArray(parsed) && parsed.every((v: unknown) => typeof v === "string")) {
          expandedProjects = new Set(parsed as string[]);
        }
      }
    } catch {
      /* ignore corrupted data, keep empty Set */
    }
    try {
      const pinned = localStorage.getItem("ocv:pinned-cwds");
      if (pinned) pinnedCwds = JSON.parse(pinned);
    } catch {
      /* ignore parse errors */
    }
    removedCwds = loadRemovedCwds();

    // Poll for runs every 60s (fallback only — primary updates via ocv:runs-changed event)
    const interval = setInterval(loadRuns, 60000);

    // Team store: initial load + poll fallback (60s)
    teamStore.loadTeams();
    const teamPollInterval = setInterval(() => teamStore.loadTeams(), 60000);

    // Team/task event listeners — app-level lifecycle, independent of chat page
    type TeamUpdatePayload = { team_name: string; change: string };
    type TaskUpdatePayload = { team_name: string; task_id: string; change: string };

    let destroyed = false;
    let unlistenTeam: (() => void) | undefined;
    let unlistenTask: (() => void) | undefined;
    const retryTimers: ReturnType<typeof setTimeout>[] = [];

    // 首次+重试成功后都补偿同步（debounce 300ms）
    let resyncTimer: ReturnType<typeof setTimeout> | undefined;
    function scheduleResync() {
      if (resyncTimer) clearTimeout(resyncTimer);
      resyncTimer = setTimeout(() => {
        if (!destroyed) teamStore.forceRefresh();
      }, 300);
    }

    const transport = getTransport();

    function registerTeamListener<T>(
      name: string,
      handler: (payload: T) => void,
      assign: (fn: () => void) => void,
    ) {
      function tryListen(attempt: number) {
        transport
          .listen<T>(name, handler)
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
      (payload) => {
        dbg("layout", "team-update", payload);
        teamStore.handleTeamUpdate(payload);
      },
      (fn) => (unlistenTeam = fn),
    );

    registerTeamListener<TaskUpdatePayload>(
      "task-update",
      (payload) => {
        dbg("layout", "task-update", payload);
        teamStore.handleTaskUpdate(payload);
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
      // Invalidate git cache unconditionally; if currently viewing Git tab on Explorer,
      // reload immediately — otherwise lazy $effect picks it up on next visit.
      ++_gitSeq; // cancel in-flight request so it can't backfill _gitLoadedCwd
      gitLoading = false;
      _gitLoadedCwd = "";
      if (currentPath?.startsWith("/explorer") && explorerTab === "git") {
        loadGitSummary();
      }
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

    // Memory page signals which file it selected (for sidebar highlight sync)
    function onMemoryFileSelected(e: Event) {
      const path = (e as CustomEvent).detail?.path ?? "";
      if (path) memorySelectedFile = path;
    }
    window.addEventListener("ocv:memory-file-selected", onMemoryFileSelected);

    // Memory page signals a file was saved (refresh candidates to update exists status)
    function onMemoryFileSaved() {
      if (currentPath?.startsWith("/memory")) loadMemoryCandidates({ soft: true });
    }
    window.addEventListener("ocv:memory-file-saved", onMemoryFileSaved);

    // Sync projectCwd when chat page picks a folder via dialog
    function handleCwdChanged() {
      const newCwd = normalizeCwd(localStorage.getItem("ocv:project-cwd") ?? "") || "";
      if (newCwd !== projectCwd) {
        projectCwd = newCwd;
      }
    }
    window.addEventListener("ocv:cwd-changed", handleCwdChanged);

    // Open permissions modal from any entry point (Command Palette, PromptInput button)
    function onOpenPermissions() {
      permissionsModalOpen = true;
    }
    window.addEventListener("ocv:open-permissions", onOpenPermissions);

    // ── External link interceptor ──
    // Prevent webview from navigating away to external URLs.
    // Opens them in the system browser instead.
    function handleExternalLink(e: MouseEvent) {
      // Only intercept plain left-click (no modifier keys)
      if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;

      const anchor = (e.target as HTMLElement)?.closest?.("a");
      if (!anchor) return;

      const href = anchor.getAttribute("href");
      if (!href) return;

      // Parse URL — handles protocol-relative (//example.com), case-insensitive schemes
      let url: URL;
      try {
        url = new URL(href, window.location.origin);
      } catch {
        return;
      }

      // Only intercept http/https
      if (url.protocol !== "http:" && url.protocol !== "https:") return;
      // Skip internal SvelteKit routes (same origin)
      if (url.origin === window.location.origin) return;

      // Prevent webview navigation, don't stopPropagation (let other listeners see it)
      e.preventDefault();

      dbg("layout", "external-link: opening in system browser", { href });
      import("@tauri-apps/plugin-shell")
        .then(({ open }) => open(href))
        .catch((err) => {
          dbgWarn("layout", "external-link: plugin-shell failed, fallback to window.open", err);
          window.open(href, "_blank");
        });
    }
    document.addEventListener("click", handleExternalLink, true);
    dbg("layout", "external-link interceptor mounted");

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
      window.removeEventListener("ocv:memory-file-selected", onMemoryFileSelected);
      window.removeEventListener("ocv:memory-file-saved", onMemoryFileSaved);
      window.removeEventListener("ocv:open-permissions", onOpenPermissions);
      document.removeEventListener("click", handleExternalLink, true);
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

  // ── Delete conversation confirm flow ──
  let deleteConfirmOpen = $state(false);
  let deleteTarget: ConversationGroup | null = $state(null);

  function requestDeleteConversation(conv: ConversationGroup) {
    deleteTarget = conv;
    deleteConfirmOpen = true;
  }

  async function confirmDeleteConversation() {
    const conv = deleteTarget;
    deleteConfirmOpen = false;
    deleteTarget = null;
    if (!conv) return;
    try {
      const ids = conv.runs.map((r) => r.id);
      await softDeleteRuns(ids);
      dbg("layout", "deleteConversation success", { ids });
      window.dispatchEvent(new Event("ocv:runs-changed"));
      if (conv.runs.some((r) => r.id === selectedRunId)) {
        goto("/chat");
      }
    } catch (e) {
      dbgWarn("layout", "deleteConversation failed", e);
    }
  }

  function cancelDeleteConversation() {
    deleteConfirmOpen = false;
    deleteTarget = null;
  }

  // ── Remove project folder confirm flow ──
  let removeProjectConfirmOpen = $state(false);
  let removeProjectTarget = $state("");

  function persistRemovedCwds() {
    localStorage.setItem("ocv:removed-cwds", JSON.stringify(removedCwds));
  }

  function requestRemoveProject(cwd: string) {
    removeProjectTarget = normalizeCwd(cwd);
    removeProjectConfirmOpen = true;
  }

  function confirmRemoveProject() {
    const normalized = removeProjectTarget;
    removeProjectConfirmOpen = false;
    removeProjectTarget = "";
    if (!normalized) return;

    // Add to removedCwds
    if (!removedCwds.includes(normalized)) {
      removedCwds = [...removedCwds, normalized];
      persistRemovedCwds();
    }

    // Remove from pinnedCwds (compare normalized)
    const newPinned = pinnedCwds.filter((c) => normalizeCwd(c) !== normalized);
    if (newPinned.length !== pinnedCwds.length) {
      pinnedCwds = newPinned;
      localStorage.setItem("ocv:pinned-cwds", JSON.stringify(pinnedCwds));
    }

    // If currently viewing this project, switch to All Projects
    if (normalizeCwd(projectCwd) === normalized) {
      projectCwd = "";
    }

    dbg("layout", "removeProject", { cwd: normalized });
  }

  function cancelRemoveProject() {
    removeProjectConfirmOpen = false;
    removeProjectTarget = "";
  }

  // Build project folder tree for chats tab
  let projectFolders = $derived.by(() =>
    buildProjectFolders(runs, favoriteRunIds, pinnedCwds, removedCwds),
  );

  // Selectable folders: real project folders (exclude Uncategorized)
  const selectableFolders = $derived(projectFolders.filter((f) => !f.isUncategorized));

  // Removed cwd set for O(1) lookup in search filtering
  let removedCwdSet = $derived(new Set(removedCwds.map(normalizeCwd)));

  // Filter search results to exclude removed project cwds
  let visibleSearchResults = $derived.by(() => {
    if (removedCwdSet.size === 0) return searchResults;
    // Build runId→cwd mapping from runs
    const runCwdMap = new Map<string, string>();
    for (const run of runs) {
      runCwdMap.set(run.id, normalizeCwd(run.cwd));
    }
    return searchResults.filter((result) => {
      const cwd = runCwdMap.get(result.runId);
      // Unknown runId (not in runs yet) → show by default (avoid async timing issues)
      if (cwd === undefined) return true;
      // "" = Uncategorized → always show
      if (!cwd) return true;
      return !removedCwdSet.has(cwd);
    });
  });

  // Debug log when folder tree rebuilds
  $effect(() => {
    dbg("layout", "folders rebuilt", {
      count: projectFolders.length,
      total: projectFolders.reduce((s, f) => s + f.conversationCount, 0),
    });
  });

  // Defensive fallback: reset projectCwd if it's no longer in selectable folders
  $effect(() => {
    if (!projectCwd) return; // "" is always valid (All Projects)
    const validCwds = new Set(selectableFolders.map((f) => f.cwd));
    if (!validCwds.has(projectCwd)) {
      dbg("layout", "projectCwd not in selectable folders, resetting", { projectCwd });
      projectCwd = "";
    }
  });

  // Current page detection
  let currentPath = $derived($page.url.pathname);
  let isChatPage = $derived(currentPath === "/chat" || currentPath === "/");
  let isPluginsPage = $derived(currentPath.startsWith("/plugins"));
  let isExplorerPage = $derived(currentPath.startsWith("/explorer"));
  let isMemoryPage = $derived(currentPath.startsWith("/memory"));

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
  let pageName = $derived.by(() => {
    const nav = navItems.find((n) => currentPath.startsWith(n.path));
    if (nav) return nav.label();
    if (currentPath.startsWith("/release-notes")) return t("release_cliChangelog");
    return t("layout_appName");
  });

  function newChat() {
    goto("/chat");
  }

  function newChatInFolder(cwd: string) {
    projectCwd = cwd;
    goto(`/chat?folder=${encodeURIComponent(cwd)}`);
  }

  function toggleProject(folderKey: string) {
    const next = new Set(expandedProjects);
    if (next.has(folderKey)) next.delete(folderKey);
    else next.add(folderKey);
    expandedProjects = next;
  }

  async function pickFolder() {
    if (getTransport().isDesktop()) {
      try {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const selected = await open({ directory: true, title: t("layout_selectProjectFolder") });
        if (selected) {
          const normalized = normalizeCwd(selected as string) || "";
          if (normalized && removedCwds.includes(normalized)) {
            removedCwds = removedCwds.filter((c) => c !== normalized);
            persistRemovedCwds();
            dbg("layout", "pickFolder: un-removed cwd", { cwd: normalized });
          }
          projectCwd = normalized;
        }
      } catch (e) {
        dbgWarn("layout", "failed to open folder dialog:", e);
      }
    } else {
      // Browser mode: no native file picker, prompt for path
      const input = window.prompt(
        t("layout_selectProjectFolder"),
        projectCwd || settings?.working_directory || "/",
      );
      dbg("layout", "pickFolder (browser)", { input });
      if (input) {
        const normalized = normalizeCwd(input.trim()) || "";
        if (normalized && removedCwds.includes(normalized)) {
          removedCwds = removedCwds.filter((c) => c !== normalized);
          persistRemovedCwds();
          dbg("layout", "pickFolder: un-removed cwd", { cwd: normalized });
        }
        projectCwd = normalized;
      }
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

  function cycleScheme() {
    colorScheme = colorScheme === "warm" ? "neutral" : "warm";
    dbg("layout", "color scheme cycled", { colorScheme });
  }

  // Persist theme + apply class
  $effect(() => {
    localStorage.setItem("ocv:theme", themeMode);
    document.documentElement.classList.toggle("dark", effectiveDark);
  });

  // Persist color scheme + apply class
  $effect(() => {
    localStorage.setItem("ocv:colorScheme", colorScheme);
    document.documentElement.classList.toggle("scheme-neutral", colorScheme === "neutral");
  });

  // Auto-expand folder containing selected run (chats tab only)
  // Track runId + runs.length as change signals. runs.length is the most
  // reliable: it changes on any new run (including resume into existing
  // session where conversationCount stays the same).
  // Don't track expandedProjects itself (otherwise collapsing re-expands).
  let _prevAutoExpandRunId = "";
  let _prevAutoExpandRunsLen = 0;
  $effect(() => {
    if (!isChatPage || panelTab !== "chats") return;
    const runId = selectedRunId;
    const runsLen = runs.length;
    const runChanged = runId !== _prevAutoExpandRunId;
    const runsChanged = runsLen !== _prevAutoExpandRunsLen;
    if (!runChanged && !runsChanged) return; // early-return avoids tracking expandedProjects
    _prevAutoExpandRunId = runId;
    _prevAutoExpandRunsLen = runsLen;
    if (!runId) return;
    const next = autoExpandForRun(runId, projectFolders, expandedProjects);
    if (next) {
      dbg("layout", "auto-expand for run", { selectedRunId: runId });
      expandedProjects = next;
    }
  });

  // Auto-expand folder matching projectCwd (cross-tab sync)
  let _prevAutoExpandCwd = "";
  $effect(() => {
    const cwd = projectCwd;
    if (cwd === _prevAutoExpandCwd) return;
    _prevAutoExpandCwd = cwd;
    if (!cwd) return;
    const folderKey = `cwd:${cwd}`;
    const next = expandForProjectChange(folderKey, expandedProjects);
    if (next) {
      dbg("layout", "auto-expand for cwd change", { cwd });
      expandedProjects = next;
    }
  });

  // Persist expandedProjects + prune stale keys (only after first successful load)
  $effect(() => {
    if (!runsLoadSucceededOnce) return;
    const validKeys = new Set(projectFolders.map((f) => f.folderKey));
    const pruned = [...expandedProjects].filter((k) => validKeys.has(k));
    if (pruned.length !== expandedProjects.size) {
      expandedProjects = new Set(pruned);
    }
    localStorage.setItem("ocv:expanded-projects", JSON.stringify(pruned));
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
        class="flex w-[44px] flex-col items-center border-r border-sidebar-border bg-black/[0.03] dark:bg-black/20"
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
              class="relative flex h-9 w-9 items-center justify-center rounded-md transition-colors duration-150 no-underline
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
              {:else if item.icon === "clock"}
                <svg
                  class="h-[18px] w-[18px]"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><circle cx="12" cy="12" r="10" /><polyline points="12 6 12 12 16 14" /></svg
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
              <span class="sr-only">{item.label()}</span>
            </a>
          {/each}
        </nav>

        <!-- Rail version + locale + dark mode toggle -->
        <div class="border-t border-sidebar-border py-2">
          <div class="flex items-center justify-center pb-1">
            <button
              class="text-xs text-muted-foreground hover:text-muted-foreground transition-colors cursor-pointer"
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
          <button
            class="flex h-9 w-9 items-center justify-center rounded-md text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors duration-150"
            onclick={cycleScheme}
            title={colorScheme === "warm"
              ? t("layout_schemeTitle_warm")
              : t("layout_schemeTitle_neutral")}
          >
            <!-- Palette icon -->
            <svg
              class="h-[18px] w-[18px]"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              ><circle cx="13.5" cy="6.5" r=".5" fill="currentColor" /><circle
                cx="17.5"
                cy="10.5"
                r=".5"
                fill="currentColor"
              /><circle cx="8.5" cy="7.5" r=".5" fill="currentColor" /><circle
                cx="6.5"
                cy="12"
                r=".5"
                fill="currentColor"
              /><path
                d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"
              /></svg
            >
          </button>
        </div>
      </div>

      <!-- B. Content Panel -->
      <div class="flex w-[280px] flex-col overflow-hidden border-r border-sidebar-border">
        <!-- Panel header: Project selector + new chat -->
        <div class="flex h-14 items-center gap-1.5 border-b border-sidebar-border px-3">
          <span class="flex-1 min-w-0 truncate text-sm font-medium text-sidebar-foreground"
            >{pageName}</span
          >
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
                class="flex w-full items-center gap-2 py-2 px-3 text-xs font-medium transition-colors
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
              class="flex-1 py-1.5 text-xs font-medium text-center transition-colors
              {explorerTab === 'files'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (explorerTab = "files")}>{t("sidebar_files")}</button
            >
            <button
              class="relative flex-1 py-1.5 text-xs font-medium text-center transition-colors
              {explorerTab === 'git'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (explorerTab = "git")}
              >{t("sidebar_git")}
              {#if gitSummary && gitSummary.total_files > 0}
                <span
                  class="ml-0.5 inline-flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-blue-500/80 px-1 text-[10px] font-bold text-white"
                  >{gitSummary.total_files}</span
                >
              {/if}
            </button>
          </div>

          <!-- Compact project picker (below tabs) -->
          <div class="relative shrink-0 border-b border-sidebar-border">
            <button
              class="flex w-full items-center gap-1.5 px-2.5 py-1.5 text-xs transition-colors hover:bg-sidebar-accent/50"
              onclick={() => (explorerProjectOpen = !explorerProjectOpen)}
            >
              <svg
                class="h-3.5 w-3.5 shrink-0 text-muted-foreground/70"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                ><path
                  d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"
                /></svg
              >
              <span class="min-w-0 truncate text-sidebar-foreground"
                >{projectCwd ? cwdDisplayLabel(projectCwd) : t("sidebar_selectProjectBrowse")}</span
              >
              <svg
                class="ml-auto h-3 w-3 shrink-0 text-muted-foreground/50 transition-transform {explorerProjectOpen
                  ? 'rotate-180'
                  : ''}"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg
              >
            </button>
            {#if explorerProjectOpen}
              <div class="border-b border-sidebar-border bg-sidebar">
                {#each selectableFolders as folder (folder.folderKey)}
                  <button
                    class="flex w-full items-center gap-1.5 px-2.5 py-1.5 text-xs transition-colors
                      {folder.cwd === projectCwd
                      ? 'bg-sidebar-accent text-sidebar-foreground'
                      : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
                    onclick={() => {
                      projectCwd = folder.cwd;
                      explorerProjectOpen = false;
                    }}
                  >
                    <svg
                      class="h-3 w-3 shrink-0 text-muted-foreground/70"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      ><path
                        d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"
                      /></svg
                    >
                    <span class="min-w-0 truncate">{cwdDisplayLabel(folder.cwd)}</span>
                  </button>
                {/each}
                <button
                  class="flex w-full items-center gap-1.5 px-2.5 py-1.5 text-xs text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
                  onclick={() => {
                    pickFolder();
                    explorerProjectOpen = false;
                  }}
                >
                  <svg
                    class="h-3 w-3 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"><path d="M12 5v14" /><path d="M5 12h14" /></svg
                  >
                  <span>{t("project_openFolder")}</span>
                </button>
              </div>
            {/if}
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
                  class="flex items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground border-b border-sidebar-border shrink-0"
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
        {:else if isMemoryPage}
          <!-- Memory file tree -->
          <div class="flex-1 overflow-y-auto py-1">
            <!-- Project folders (accordion: only one expanded at a time) -->
            {#each selectableFolders as folder (folder.folderKey)}
              <ProjectFolderItem
                {folder}
                label={cwdDisplayLabel(folder.cwd)}
                expanded={folder.cwd === projectCwd}
                showCount={false}
                onToggle={() => {
                  projectCwd = projectCwd === folder.cwd ? "" : folder.cwd;
                }}
              >
                {#if memoryLoading}
                  <div class="flex items-center justify-center py-6">
                    <div
                      class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                    ></div>
                  </div>
                {:else if memoryScopeFolder.length > 0}
                  {#each filterVisibleCandidates(memoryScopeFolder, true, memorySelectedFile) as file}
                    <button
                      class="flex w-full items-center gap-1.5 py-1 pl-4 pr-3 text-xs transition-colors
                        {memorySelectedFile === file.path
                        ? 'bg-sidebar-accent text-sidebar-foreground'
                        : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
                      onclick={() => selectMemoryFile(file)}
                      title={file.path}
                    >
                      <svg
                        class="h-3 w-3 shrink-0 {file.scope === 'memory'
                          ? 'text-amber-400'
                          : file.exists
                            ? 'text-blue-400'
                            : 'text-muted-foreground/40'}"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><path
                          d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"
                        /><path d="M14 2v4a2 2 0 0 0 2 2h4" /></svg
                      >
                      <span class="min-w-0 truncate">{file.label}</span>
                      {#if !file.exists}
                        <span class="ml-auto text-[10px] text-muted-foreground shrink-0"
                          >{t("memory_new")}</span
                        >
                      {/if}
                    </button>
                  {/each}
                {:else}
                  <p class="px-2 py-3 text-xs text-muted-foreground">
                    {t("memory_noProjectFiles")}
                  </p>
                {/if}
              </ProjectFolderItem>
            {/each}
            <!-- Global scope (same style as project folders, globe icon) -->
            {#if memoryScopeGlobal.length > 0}
              <div class="mb-0.5">
                <button
                  class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
                  onclick={() => toggleMemoryScope("global")}
                >
                  <svg
                    class="h-3 w-3 shrink-0 text-muted-foreground/60 transition-transform duration-150 {memoryScopeExpanded[
                      'global'
                    ]
                      ? 'rotate-90'
                      : ''}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"><path d="M9 18l6-6-6-6" /></svg
                  >
                  <!-- Globe icon -->
                  <svg
                    class="h-3.5 w-3.5 shrink-0 text-muted-foreground/70"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><circle cx="12" cy="12" r="10" /><path d="M2 12h20" /><path
                      d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"
                    /></svg
                  >
                  <span class="truncate">{t("memory_tabGlobal")}</span>
                </button>
                {#if memoryScopeExpanded["global"]}
                  <div class="pl-3">
                    {#each filterVisibleCandidates(memoryScopeGlobal, true, memorySelectedFile) as file}
                      <button
                        class="flex w-full items-center gap-1.5 py-1 pl-4 pr-3 text-xs transition-colors
                          {memorySelectedFile === file.path
                          ? 'bg-sidebar-accent text-sidebar-foreground'
                          : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
                        onclick={() => selectMemoryFile(file)}
                        title={file.path}
                      >
                        <svg
                          class="h-3 w-3 shrink-0 {file.exists
                            ? 'text-blue-400'
                            : 'text-muted-foreground/40'}"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                          ><path
                            d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"
                          /><path d="M14 2v4a2 2 0 0 0 2 2h4" /></svg
                        >
                        <span class="min-w-0 truncate">{file.label}</span>
                        {#if !file.exists}
                          <span class="ml-auto text-[10px] text-muted-foreground shrink-0"
                            >{t("memory_new")}</span
                          >
                        {/if}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}

            <!-- Open folder button -->
            <button
              class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
              onclick={pickFolder}
            >
              <svg
                class="h-3.5 w-3.5 shrink-0"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="M12 5v14" /><path d="M5 12h14" /></svg
              >
              <span>+ {t("project_openFolder")}</span>
            </button>
          </div>
        {:else}
          <!-- Tab bar -->
          <div class="flex shrink-0 border-b border-sidebar-border">
            <button
              class="flex-1 py-1.5 text-xs font-medium text-center transition-colors
              {panelTab === 'chats'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (panelTab = "chats")}>{t("sidebar_chats")}</button
            >
            <button
              class="relative flex-1 py-1.5 text-xs font-medium text-center transition-colors
              {panelTab === 'teams'
                ? 'text-sidebar-foreground border-b-2 border-primary'
                : 'text-muted-foreground hover:text-sidebar-foreground'}"
              onclick={() => (panelTab = "teams")}
              >{t("sidebar_teams")}
              {#if teamStore.teams.length > 0}
                <span
                  class="ml-0.5 inline-flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-teal-500/80 px-1 text-[10px] font-bold text-white"
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
                  <p class="text-xs text-muted-foreground px-1 pt-0.5">
                    {t("runs_searching")}
                  </p>
                {:else if visibleSearchResults.length > 0}
                  <p
                    class="flex items-center justify-between text-xs text-muted-foreground px-1 pt-0.5"
                  >
                    <span
                      >{t("runs_resultsCount", {
                        count: String(visibleSearchResults.length),
                      })}</span
                    >
                    <a
                      href="/history?q={encodeURIComponent(runSearchQuery)}"
                      class="text-primary/70 hover:text-primary transition-colors"
                      >{t("history_advancedSearch")}</a
                    >
                  </p>
                {/if}
              {/if}
            </div>

            {#if runSearchQuery.trim()}
              <!-- Search results -->
              <div class="flex-1 overflow-y-auto">
                {#if searching && visibleSearchResults.length === 0}
                  <div class="flex items-center justify-center py-10">
                    <div
                      class="h-4 w-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                    ></div>
                  </div>
                {:else if !searching && visibleSearchResults.length === 0}
                  <div class="flex items-center justify-center px-3 py-10 text-center">
                    <p class="text-xs text-muted-foreground">{t("runs_noMatching")}</p>
                  </div>
                {:else}
                  {#each visibleSearchResults as result}
                    <button
                      class="w-full text-left flex flex-col gap-0.5 px-3 py-2 hover:bg-sidebar-accent/50 transition-colors text-sidebar-foreground"
                      onclick={() => {
                        runSearchQuery = "";
                        searchResults = [];
                        goto(
                          `/chat?run=${result.runId}&scrollTo=${encodeURIComponent(result.matchedEventId || result.matchedTs)}`,
                        );
                      }}
                    >
                      <p class="text-[12px] min-w-0 line-clamp-2 break-all">
                        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                        {@html highlightMatch(
                          snippetAround(result.matchedText, runSearchQuery, 80),
                          runSearchQuery,
                        )}
                      </p>
                      <div class="flex items-center gap-1 text-xs text-muted-foreground min-w-0">
                        <span class="flex-1 min-w-0 truncate"
                          >{result.runName || truncate(result.runPrompt, 30)}</span
                        >
                        <span class="ml-auto shrink-0">{relativeTime(result.matchedTs)}</span>
                      </div>
                    </button>
                  {/each}
                {/if}
              </div>
            {:else}
              <!-- Project folder tree -->
              <div class="flex-1 overflow-y-auto px-2 py-1">
                {#each projectFolders as folder (folder.folderKey)}
                  <ProjectFolderItem
                    {folder}
                    label={folder.isUncategorized
                      ? t("sidebar_uncategorized")
                      : cwdDisplayLabel(folder.cwd)}
                    expanded={expandedProjects.has(folder.folderKey)}
                    {selectedRunId}
                    onToggle={() => toggleProject(folder.folderKey)}
                    onSelectConversation={(runId) => goto(`/chat?run=${runId}`)}
                    onResume={(runId, mode) => goto(`/chat?run=${runId}&resume=${mode}`)}
                    onDelete={requestDeleteConversation}
                    onRemove={folder.isUncategorized
                      ? undefined
                      : () => requestRemoveProject(folder.cwd)}
                    onNewChat={folder.isUncategorized
                      ? undefined
                      : () => newChatInFolder(folder.cwd)}
                  />
                {/each}
                <!-- Open folder... -->
                <button
                  class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
                  onclick={pickFolder}
                >
                  <svg
                    class="h-3.5 w-3.5 shrink-0"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"><path d="M12 5v14" /><path d="M5 12h14" /></svg
                  >
                  <span>{t("project_openFolder")}</span>
                </button>

                {#if projectFolders.length === 0}
                  <div class="flex flex-col items-center gap-2 px-3 py-6 text-center">
                    <p class="text-xs text-muted-foreground">
                      {t("sidebar_noConversationsYet")}<br />{t("sidebar_startNewChat")}
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
                      <p class="text-xs text-muted-foreground truncate pl-3.5">
                        {team.description}
                      </p>
                    {/if}
                    <div class="flex items-center gap-2 pl-3.5 text-xs text-muted-foreground">
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

<PermissionsModal bind:open={permissionsModalOpen} cwd={projectCwd} />

{#if showCliBrowser}
  <CliSessionBrowser
    cwd="/"
    onclose={() => (showCliBrowser = false)}
    onimported={(runId) => {
      showCliBrowser = false;
      loadRuns();
      goto(`/chat?run=${runId}`);
    }}
  />
{/if}

<Modal bind:open={deleteConfirmOpen} title={t("sidebar_deleteConfirm")}>
  <p class="text-sm text-muted-foreground mb-4">{t("sidebar_deleteDesc")}</p>
  <div class="flex justify-end gap-2">
    <button
      class="px-3 py-1.5 text-sm rounded-md border border-border hover:bg-accent transition-colors"
      onclick={cancelDeleteConversation}
    >
      {t("sidebar_deleteCancel")}
    </button>
    <button
      class="px-3 py-1.5 text-sm rounded-md bg-destructive text-destructive-foreground hover:bg-destructive/90 transition-colors"
      onclick={confirmDeleteConversation}
    >
      {t("sidebar_deleteOk")}
    </button>
  </div>
</Modal>

<Modal bind:open={removeProjectConfirmOpen} title={t("sidebar_removeProjectConfirm")}>
  <p class="text-sm text-muted-foreground mb-4">{t("sidebar_removeProjectDesc")}</p>
  <div class="flex justify-end gap-2">
    <button
      class="px-3 py-1.5 text-sm rounded-md border border-border hover:bg-accent transition-colors"
      onclick={cancelRemoveProject}
    >
      {t("sidebar_deleteCancel")}
    </button>
    <button
      class="px-3 py-1.5 text-sm rounded-md bg-destructive text-destructive-foreground hover:bg-destructive/90 transition-colors"
      onclick={confirmRemoveProject}
    >
      {t("sidebar_deleteOk")}
    </button>
  </div>
</Modal>

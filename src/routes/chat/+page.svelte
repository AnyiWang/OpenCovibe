<script lang="ts">
  import { page } from "$app/stores";
  import { goto, replaceState } from "$app/navigation";
  import { tick, onMount, untrack, getContext } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import * as api from "$lib/api";
  import {
    SessionStore,
    KeybindingStore,
    getEventMiddleware,
    loadCliInfo,
    getCliCurrentModel,
    getCliCommands,
    getCliModels,
    canResumeRun,
    getResumeWarning,
    loadCliVersionInfo,
    getCliVersionInfo_cached,
  } from "$lib/stores";
  import type {
    Attachment,
    UserSettings,
    AgentSettings,
    SessionMode,
    CliModelInfo,
    ScreenshotPayload,
    SessionInfoData,
    TimelineEntry,
  } from "$lib/types";
  import { PLATFORM_PRESETS, findCredential } from "$lib/utils/platform-presets";
  import XTerminal from "$lib/components/XTerminal.svelte";
  import ChatMessage from "$lib/components/ChatMessage.svelte";
  import InlineToolCard from "$lib/components/InlineToolCard.svelte";
  import SessionStatusBar from "$lib/components/SessionStatusBar.svelte";
  import McpStatusPanel from "$lib/components/McpStatusPanel.svelte";
  import PromptInput from "$lib/components/PromptInput.svelte";
  import AuthSourceBadge from "$lib/components/AuthSourceBadge.svelte";

  import ToolActivity from "$lib/components/ToolActivity.svelte";
  import BackgroundTaskPanel from "$lib/components/BackgroundTaskPanel.svelte";
  import ShortcutHelpPanel from "$lib/components/ShortcutHelpPanel.svelte";
  import type { PromptInputSnapshot } from "$lib/types";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import HookReviewCard from "$lib/components/HookReviewCard.svelte";
  import CliSessionBrowser from "$lib/components/CliSessionBrowser.svelte";
  import ContextUsageGrid from "$lib/components/ContextUsageGrid.svelte";
  import CostSummaryView from "$lib/components/CostSummaryView.svelte";
  import { parseContextMarkdown } from "$lib/utils/context-parser";
  import type { ContextSnapshot } from "$lib/types";
  import ReleaseNotesCard from "$lib/components/ReleaseNotesCard.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { getToolColor } from "$lib/utils/tool-colors";
  import { ansiToHtml, hasAnsiCodes } from "$lib/utils/ansi";
  import { randomSpinnerVerb } from "$lib/utils/spinner-verbs";
  import { type TurnUsage, classifyError } from "$lib/stores/types";
  import { mergeWithVirtual, buildHelpText } from "$lib/utils/slash-commands";
  import { executeAddDir } from "$lib/utils/add-dir";
  import { buildDoctorReport } from "$lib/utils/doctor";

  // ── Helpers ──

  // WebKit (Tauri WKWebView on macOS) has unreliable content-visibility:auto re-layout.
  // Disable the optimization on all WebKit engines to ensure correct rendering.
  const isWebKit =
    typeof navigator !== "undefined" &&
    /AppleWebKit/.test(navigator.userAgent) &&
    !/(Chrome|Chromium|Edg|OPR)/.test(navigator.userAgent);

  // ── Layout context ──
  const toggleLayoutSidebar = getContext<() => void>("toggleSidebar");
  const keybindingStore = getContext<KeybindingStore>("keybindings");

  // ── Store + Middleware ──
  const store = new SessionStore();
  const middleware = getEventMiddleware();

  // ── UI-only state (not in store) ──
  let middlewareReady = $state(false);
  let settings = $state<UserSettings | null>(null);
  let xtermRef: XTerminal | undefined = $state();
  let promptRef: PromptInput | undefined = $state();
  let xtermReady = $state(false);
  let pendingMessage = $state<{ text: string; attachments: Attachment[] } | null>(null);
  let sidebarCollapsed = $state(false);
  let chatAreaRef: HTMLDivElement | undefined = $state();
  let agentSettings = $state<AgentSettings | null>(null);
  let resuming = $state(false);
  /** Suppress "Session ended" flash during tool approval restart cycle. */
  let approving = $state(false);
  // (pendingResumeText removed — auto-resume uses atomic resume+send via initialMessage)
  /** Most recent run with a session_id — for "Continue last session" on welcome screen. */
  let lastContinuableRun = $state<import("$lib/types").TaskRun | null>(null);
  /** Available remote hosts from settings. */
  let remoteHosts = $state<import("$lib/types").RemoteHost[]>([]);
  /** Target host dropdown in hero meta. */
  let targetDropdownOpen = $state(false);
  /** Auth overview for AuthSourceBadge. */
  let authOverview = $state<import("$lib/types").AuthOverview | null>(null);
  /** Preloaded skill details from filesystem (has descriptions). */
  let preloadedSkills = $state<import("$lib/types").StandaloneSkill[]>([]);

  // ── Model contamination helpers ──

  /** Cache of last confirmed-clean Anthropic model, used as final fallback. */
  let lastKnownGoodAnthropicModel: string | undefined;

  /** Detect if default_model was contaminated by a third-party platform model.
   *  Returns:
   *  - true  = confirmed contaminated (in third-party models, not in CLI models)
   *  - false = confirmed clean (in CLI known models)
   *  - null  = unknown (CLI not loaded, or model not found in any list)
   */
  function isContaminatedDefaultModel(dm: string): boolean | null {
    const cliModels = getCliModels();
    if (!cliModels.length) return null; // CLI models not loaded yet
    if (cliModels.some((m) => m.value === dm)) return false; // in CLI model list = clean

    const inThirdParty =
      PLATFORM_PRESETS.some(
        (p) => p.id !== "anthropic" && p.id !== "custom" && p.models?.includes(dm),
      ) ||
      (settings?.platform_credentials ?? []).some(
        (c) => c.platform_id !== "anthropic" && c.models?.includes(dm),
      );
    return inThirdParty ? true : null; // not in CLI + not in third-party = unknown
  }

  // ── Project init detection ──
  let projectInitStatus = $state<import("$lib/types").ProjectInitStatus | null>(null);
  let initCheckSeq = 0;

  // ── Task notification banner ──
  let notificationVisible = $state(false);
  let latestNotification = $state<{ task_id: string; status: string } | null>(null);

  // ── Shortcut help panel ──
  let shortcutHelpOpen = $state(false);
  let statusBarRef: SessionStatusBar | undefined = $state();
  let stashedInput: PromptInputSnapshot | null = $state(null);
  let taskPanelCollapsed = $state(false);
  let sidebarRequestedTab = $state<"tools" | "context" | "files" | "info" | null>(null);

  // ── Verbose state (chat page level) ──
  let verboseEnabled = $state(false);
  let verboseSeq = 0;
  let lastSyncedRunId = "__unset__"; // sentinel ≠ "__no_run__", ensures first-screen trigger
  let verboseRetryTick = $state(0);
  let verboseRetryCount = 0;
  let verboseRetryTimer: ReturnType<typeof setTimeout> | null = null;
  const VERBOSE_MAX_RETRIES = 3;

  // ── Tool result lazy-load cache (Phase 2) ──
  let toolResultCache = new Map<string, Record<string, unknown>>();
  let toolResultInflight = new Map<string, Promise<Record<string, unknown> | null>>();
  // Clear cache on run switch
  $effect(() => {
    const _ = store.run?.id;
    toolResultCache = new Map();
    toolResultInflight = new Map();
  });

  async function fetchToolResult(
    runId: string,
    toolUseId: string,
  ): Promise<Record<string, unknown> | null> {
    const key = `${runId}:${toolUseId}`;
    const cached = toolResultCache.get(key);
    if (cached) return cached;
    let pending = toolResultInflight.get(key);
    if (!pending) {
      pending = api.getToolResult(runId, toolUseId);
      toolResultInflight.set(key, pending);
    }
    try {
      const result = await pending;
      // Run-gen check: don't write stale results into a different run's cache
      if (result && store.run?.id === runId) {
        toolResultCache.set(key, result);
      }
      return result;
    } finally {
      toolResultInflight.delete(key);
    }
  }

  // ── Timeline rendering ──
  let renderLimit = $state(Infinity);
  let progressiveGen = 0; // generation counter for stale-callback protection

  async function syncVerboseState(runId: string | undefined) {
    const key = runId ?? "__no_run__";
    if (key === lastSyncedRunId) return; // same run — skip
    const seq = ++verboseSeq;
    // New run resets retry counter
    verboseRetryCount = 0;
    try {
      const cfg = await api.getCliConfig();
      if (seq !== verboseSeq) return; // stale response
      lastSyncedRunId = key; // mark synced on success only
      verboseEnabled = cfg.verbose === true;
      dbg("chat", "verbose state synced", { verbose: verboseEnabled, runId, seq });
    } catch {
      // Don't mark synced — retry via tick++ after 3s (up to max)
      if (seq === verboseSeq && verboseRetryCount < VERBOSE_MAX_RETRIES) {
        verboseRetryCount++;
        verboseRetryTimer = setTimeout(() => {
          verboseRetryTimer = null;
          verboseRetryTick++;
        }, 3000);
      }
    }
  }

  // ── MCP panel ──
  let mcpPanelOpen = $state(false);

  // ── CLI session browser ──
  let showCliBrowser = $state(false);

  // Track status bar expansion for MCP panel offset
  let statusBarExpanded = $state(
    typeof window !== "undefined"
      ? localStorage.getItem("ocv:statusbar-expanded") !== "false"
      : true,
  );

  // ── Tool filtering ──
  let toolFilter = $state<string | null>(null);

  // ── Input history (most recent first) ──
  let userHistory = $derived.by(() =>
    store.timeline
      .filter((e): e is Extract<TimelineEntry, { kind: "user" }> => e.kind === "user")
      .map((e) => e.content)
      .reverse(),
  );

  let toolNamesInTimeline = $derived.by(() => {
    const names = new Set<string>();
    for (const entry of store.timeline) {
      if (entry.kind === "tool") names.add(entry.tool.tool_name);
    }
    return [...names].sort();
  });

  let filteredTimeline = $derived.by(() => {
    if (!toolFilter) return store.timeline;
    return store.timeline.filter((e) => e.kind !== "tool" || e.tool.tool_name === toolFilter);
  });

  let visibleTimeline = $derived.by(() => {
    const ft = filteredTimeline;
    if (renderLimit >= ft.length) return ft;
    return ft.slice(ft.length - renderLimit);
  });

  // ── Auto-context tracking ──
  // Map<runId, snapshots> — persists across run switches within the session
  let contextHistoryMap = $state<Map<string, ContextSnapshot[]>>(new Map());
  let contextHistory = $derived(contextHistoryMap.get(store.run?.id ?? "") ?? []);

  // ── Cumulative session token totals (from modelUsage, which is session-cumulative) ──
  // status bar shows session totals; per-turn values are in the turn separator annotations.
  let cumulativeTokens = $derived.by(() => {
    const mu = store.usage.modelUsage;
    if (!mu || Object.keys(mu).length === 0) {
      // No modelUsage yet — fall back to per-turn values (better than zero)
      return {
        input: store.usage.inputTokens,
        output: store.usage.outputTokens,
        cacheRead: store.usage.cacheReadTokens,
        cacheWrite: store.usage.cacheWriteTokens,
      };
    }
    let input = 0,
      output = 0,
      cacheRead = 0,
      cacheWrite = 0;
    for (const entry of Object.values(mu)) {
      input += entry.input_tokens;
      output += entry.output_tokens;
      cacheRead += entry.cache_read_tokens;
      cacheWrite += entry.cache_write_tokens;
    }
    return { input, output, cacheRead, cacheWrite };
  });

  // ── Session info for InfoPanel ──
  let currentSessionInfo: SessionInfoData | null = $derived.by(() => {
    if (!store.run) return null;
    return {
      sessionId: store.run.session_id,
      runId: store.run.id,
      runName: store.run.name,
      cwd: store.sessionCwd || store.run.cwd,
      numTurns: store.numTurns,
      status: store.run.status ?? "pending",
      startedAt: store.run.started_at ?? null,
      endedAt: store.run.ended_at ?? null,
      lastTurnDurationMs: store.durationMs,
      tokensEstimated: !store.usage.modelUsage || Object.keys(store.usage.modelUsage).length === 0,
      model: store.run.model ?? store.model,
      agent: store.run.agent ?? store.agent,
      cliVersion: store.cliVersion,
      permissionMode: store.permissionMode,
      fastModeState: store.fastModeState,
      cost: store.usage.cost,
      inputTokens: cumulativeTokens.input,
      outputTokens: cumulativeTokens.output,
      cacheReadTokens: cumulativeTokens.cacheRead,
      cacheWriteTokens: cumulativeTokens.cacheWrite,
      contextWindow: store.contextWindow,
      contextUtilization: store.contextUtilization,
      compactCount: store.compactCount,
      microcompactCount: store.microcompactCount,
      mcpServers: store.mcpServers,
      remoteHostName: store.remoteHostName,
      platformId: store.platformId,
      cliUsageIncomplete: store.run.cli_usage_incomplete ?? false,
      runSource: store.run.source,
      authSourceLabel: store.authSourceLabel || undefined,
      platformName: platformDisplayName || undefined,
      cliUpdateAvailable:
        store.cliVersion && channelLatest && channelLatest !== store.cliVersion
          ? channelLatest
          : undefined,
    };
  });

  // ── Sidebar data availability (matches sidebar render condition) ──
  let hasSidebarData = $derived(
    !!(store.run || store.tools.length > 0 || store.timeline.some((e) => e.kind === "tool")),
  );

  // ── CLI version info (reactive — ensures heroMetaFooter re-renders after async load) ──
  let cliVersionInfo = $derived(getCliVersionInfo_cached());

  // ── CLI update channel ──
  let channelLatest = $derived.by(() => {
    if (!cliVersionInfo?.installed) return undefined;
    return cliVersionInfo.channel === "stable" ? cliVersionInfo.stable : cliVersionInfo.latest;
  });

  // ── Platform display name ──
  let platformDisplayName = $derived.by(() => {
    const pid = store.platformId;
    if (!pid) return undefined;
    const preset = PLATFORM_PRESETS.find((p) => p.id === pid);
    return preset?.name ?? authOverview?.app_platform_name ?? pid;
  });

  // ── Provider-aware model list ──
  // When a third-party platform is active and has a models list, use that instead of CLI models.
  // Priority: credential.models (user-configured) > preset.models (static defaults)
  let platformModels = $derived.by((): CliModelInfo[] => {
    const pid = store.platformId;
    if (!pid || pid === "anthropic") return [];
    const cred = findCredential(settings?.platform_credentials ?? [], pid);
    const preset = PLATFORM_PRESETS.find((p) => p.id === pid);
    const models = cred?.models?.length ? cred.models : preset?.models;
    if (!models?.length) return [];
    return models.map((m, i) => ({
      value: m,
      displayName: m,
      description: i === 0 ? "Default" : "",
    }));
  });

  let effectiveModels = $derived(platformModels.length > 0 ? platformModels : getCliModels());

  // Reset filter on run change & auto-focus input
  $effect(() => {
    const _ = store.run?.id;
    toolFilter = null;
    // Auto-focus the prompt input when entering a session
    requestAnimationFrame(() => promptRef?.focus());
  });

  // Sync verbose state from CLI config when run changes (or on retry)
  $effect(() => {
    const _tick = verboseRetryTick; // extra dep: drives retry on failure
    syncVerboseState(store.run?.id);
  });

  // ── Progressive timeline rendering ── helpers

  function cancelProgressive() {
    progressiveGen++;
    renderLimit = Infinity;
  }

  /**
   * Load a run and render its full timeline in one frame.
   * content-visibility:auto on entries lets the browser skip layout/paint
   * for off-screen items, keeping scroll performance smooth.
   */
  async function loadRunProgressive(
    id: string,
    xtermRef?: { clear(): void; writeText(s: string): void },
  ) {
    toolFilter = null;
    cancelProgressive();
    const gen = ++progressiveGen;

    await store.loadRun(id, xtermRef);

    if (gen !== progressiveGen) return;
    renderLimit = Infinity;
    dbg("chat", "loadRun complete", { timeline: filteredTimeline.length, gen });

    // Scroll to bottom after DOM update — ensures content-visibility triggers re-layout
    await tick();
    requestAnimationFrame(() => {
      if (chatAreaRef) chatAreaRef.scrollTop = chatAreaRef.scrollHeight;
    });
  }

  let isExpandingTimeline = $derived(false);

  let welcomeVisible = $derived(store.timeline.length === 0 && !store.streamingText && !store.run);

  /** Skill info for SkillSelector: merge preloaded details with session skill names. */
  let skillItems = $derived.by(() => {
    const detailMap = new Map(preloadedSkills.map((s) => [s.name, s]));
    const names = store.availableSkills;
    if (names.length > 0) {
      return names.map((name) => ({
        name,
        description: detailMap.get(name)?.description ?? "",
      }));
    }
    return preloadedSkills.map((s) => ({ name: s.name, description: s.description }));
  });

  // ── Per-turn usage annotations in timeline ──

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
    if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
    return String(n);
  }

  let usageByTurn = $derived(new Map(store.turnUsages.map((tu) => [tu.turnIndex, tu])));

  /** Prefix-sum of user message count across filteredTimeline (for progressive rendering offset). */
  let userCountPrefix = $derived.by(() => {
    const ft = filteredTimeline;
    const arr = new Int32Array(ft.length + 1);
    for (let i = 0; i < ft.length; i++) {
      arr[i + 1] = arr[i] + (ft[i].kind === "user" ? 1 : 0);
    }
    return arr;
  });

  /** Map of visibleTimeline index → TurnUsage to show BEFORE this entry (turn boundary). */
  let usageAnnotations = $derived.by(() => {
    const map = new Map<number, TurnUsage>();
    if (usageByTurn.size === 0) return map;
    const vt = visibleTimeline;
    const hidden = filteredTimeline.length - vt.length;
    let userCount = userCountPrefix[hidden];
    for (let i = 0; i < vt.length; i++) {
      if (vt[i].kind === "user") {
        if (userCount > 0) {
          const tu = usageByTurn.get(userCount);
          if (tu) map.set(i, tu);
        }
        userCount++;
      }
    }
    return map;
  });

  /** Usage for the last (current/latest) turn — shown after all entries. */
  let lastTurnUsage = $derived.by(() => {
    const userCount = filteredTimeline.filter((e) => e.kind === "user").length;
    if (userCount === 0) return null;
    return usageByTurn.get(userCount) ?? null;
  });

  // ── Fork overlay ──
  let forkOverlay = $state<{
    active: boolean;
    sourceRunId: string;
    startedAt: number;
    error: string | null;
  } | null>(null);
  let forkElapsed = $state(0);

  // ── Thinking timer + panel ──
  let thinkingStartedAt = $state<number | null>(null);
  let thinkingElapsed = $state(0);
  let thinkingExpanded = $state(true);
  let spinnerVerb = $state(randomSpinnerVerb());
  /** Debounced visibility — prevents spinner flash on fast CLI commands (/context, /cost). */
  let thinkingVisible = $state(false);

  /** Slash command processing indicator — shown before thinkingVisible kicks in. */
  let processingSlashCmd = $state<string | null>(null);
  let slashCmdSeenRunning = $state(false);

  $effect(() => {
    if (!processingSlashCmd) return;
    // Track: phase was "running" at some point since flag was set
    if (store.isRunning) slashCmdSeenRunning = true;
    // Clear when content arrives, error set, or turn completed (idle after running)
    if (
      store.streamingText ||
      store.thinkingText ||
      store.error ||
      store.phase === "failed" ||
      store.phase === "completed" ||
      store.phase === "stopped" ||
      (slashCmdSeenRunning && store.phase === "idle")
    ) {
      processingSlashCmd = null;
      slashCmdSeenRunning = false;
    }
  });

  $effect(() => {
    if (store.isThinking) {
      if (!thinkingStartedAt) {
        thinkingStartedAt = Date.now();
        spinnerVerb = randomSpinnerVerb();
      }
      // Debounce: only show spinner after 300ms to avoid flash on fast commands
      const showTimer = setTimeout(() => {
        thinkingVisible = true;
      }, 300);
      const interval = setInterval(() => {
        thinkingElapsed = Math.floor((Date.now() - thinkingStartedAt!) / 1000);
      }, 1000);
      return () => {
        clearTimeout(showTimer);
        clearInterval(interval);
      };
    } else {
      thinkingStartedAt = null;
      thinkingElapsed = 0;
      thinkingVisible = false;
    }
  });

  // Fork overlay timer: tick elapsed seconds while active
  $effect(() => {
    if (forkOverlay?.active && !forkOverlay.error) {
      const interval = setInterval(() => {
        forkElapsed = Math.floor((Date.now() - forkOverlay!.startedAt) / 1000);
      }, 1000);
      return () => clearInterval(interval);
    } else {
      forkElapsed = 0;
    }
  });

  // Fork overlay phase watcher: show error on failure during step 1 (fork_oneshot).
  // Overlay is dismissed explicitly by handleResume after step 1 succeeds.
  // Guard `!forkOverlay.error`: only set error once to prevent infinite $effect loop —
  // writing `forkOverlay = { ...spread }` creates a new object ref that re-triggers the effect.
  $effect(() => {
    if (!forkOverlay?.active) return;
    const phase = store.phase;
    if ((phase === "failed" || phase === "stopped") && !forkOverlay.error) {
      forkOverlay = { ...forkOverlay, error: store.error || t("chat_forkFailedFallback") };
    }
  });

  // Task notification: auto-show and dismiss after 5s
  $effect(() => {
    const notifications = store.taskNotifications;
    if (notifications.size === 0) return;
    const latest = Array.from(notifications.values()).pop();
    if (!latest) return;
    latestNotification = { task_id: latest.task_id, status: latest.status };
    notificationVisible = true;
    const timer = setTimeout(() => {
      notificationVisible = false;
    }, 5000);
    return () => clearTimeout(timer);
  });

  function formatElapsed(seconds: number): string {
    if (seconds < 60) return `${seconds}s`;
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  // ── URL-derived ──
  let runId = $derived($page.url.searchParams.get("run") ?? "");

  // ── Computed (thin wrappers for template convenience) ──
  let sending = $derived(store.phase === "spawning");

  // Example prompts for empty state
  const examplePrompts = [
    () => t("chat_examplePrompt1"),
    () => t("chat_examplePrompt2"),
    () => t("chat_examplePrompt3"),
  ];

  // ── Lifecycle ──

  // Load settings
  onMount(async () => {
    try {
      settings = await api.getUserSettings();
      store.authMode = settings.auth_mode ?? "cli";
      remoteHosts = settings.remote_hosts ?? [];
      // Restore last target selection
      if (!store.run && remoteHosts.length > 0) {
        try {
          const lastTarget = localStorage.getItem("ocv:last-target");
          if (lastTarget && remoteHosts.some((h) => h.name === lastTarget)) {
            store.remoteHostName = lastTarget;
          }
        } catch {
          // localStorage access may fail in restricted contexts
        }
      }
      // Initialize per-session platform from global active
      if (!store.platformId) {
        store.platformId = settings.active_platform_id ?? "anthropic";
      }
      // Initialize model: for third-party platforms, use credential > preset default model
      // Only for new sessions — if runId is set, loadRun will handle model restoration.
      if (!store.model && !runId && store.phase !== "loading") {
        const initCred = findCredential(settings.platform_credentials ?? [], store.platformId);
        const initPreset = PLATFORM_PRESETS.find((p) => p.id === store.platformId);
        const initModels = initCred?.models?.length ? initCred.models : initPreset?.models;
        if (store.platformId !== "anthropic" && initModels?.[0]) {
          store.model = initModels[0];
        } else if (store.platformId === "anthropic" && settings.default_model) {
          // default_model is global — only valid for Anthropic native platform.
          // Third-party platforms without a models list leave model unset.
          store.model = settings.default_model;
        }
      }
      // Load auth overview for AuthSourceBadge
      api
        .getAuthOverview()
        .then((ov) => (authOverview = ov))
        .catch(() => {});
    } catch (e) {
      dbgWarn("chat", "failed to load settings:", e);
    }
    try {
      agentSettings = await api.getAgentSettings("claude");
    } catch (e) {
      dbgWarn("chat", "failed to load agent settings:", e);
    }
    // Initialize permission mode from saved settings (before session_init arrives)
    // Agent plan_mode=true overrides user permission_mode (legacy compat)
    if (agentSettings?.plan_mode) {
      store.permissionMode = "plan";
    } else if (settings?.permission_mode) {
      const cliName = APP_TO_CLI_MODE[settings.permission_mode] ?? settings.permission_mode;
      store.permissionMode = cliName;
    }
    // Find most recent run with session_id for "Continue last session"
    try {
      const runs = await api.listRuns();
      lastContinuableRun = runs.find((r) => r.session_id && r.status !== "running") ?? null;
    } catch (e) {
      dbgWarn("chat", "failed to load runs for continue:", e);
    }
    let selfHealDone = false;
    let selfHealInFlight = false;
    loadCliInfo().then(() => {
      // Self-heal: detect and fix contaminated default_model
      if (settings?.default_model && !selfHealDone && !selfHealInFlight) {
        const dm = settings.default_model;
        const contaminated = isContaminatedDefaultModel(dm);
        if (contaminated === true) {
          const healModel = getCliCurrentModel();
          if (healModel) {
            selfHealInFlight = true;
            dbg("chat", "self-heal: default_model contaminated, persisting fix", {
              old: dm,
              new: healModel,
            });
            api
              .updateUserSettings({ default_model: healModel })
              .then(() => {
                settings!.default_model = healModel;
                lastKnownGoodAnthropicModel = healModel;
                selfHealDone = true;
                dbg("chat", "self-heal: persist succeeded");
              })
              .catch((e) => {
                dbgWarn("chat", "self-heal persist failed, will retry next loadCliInfo", e);
              })
              .finally(() => {
                selfHealInFlight = false;
              });
          } else {
            dbg("chat", "self-heal: contaminated but CLI model unavailable, deferring", { dm });
          }
        } else if (contaminated === false) {
          selfHealDone = true;
        }
      }

      const cliModel = getCliCurrentModel();
      const isThirdParty = store.platformId && store.platformId !== "anthropic";
      // Update lastKnownGoodAnthropicModel when CLI model is available
      if (cliModel && !isThirdParty) {
        lastKnownGoodAnthropicModel = cliModel;
      }
      // Only for genuinely new chats: no run loaded/loading, no URL run param
      if (cliModel && !store.run && !runId && store.phase !== "loading" && !isThirdParty) {
        dbg("chat", "set model from CLI after loadCliInfo", { cliModel, prev: store.model });
        store.model = cliModel;
      }
    });
    loadCliVersionInfo();
    checkProjectInit();
    // Preload skills from filesystem (no session needed)
    {
      const cwd = localStorage.getItem("ocv:project-cwd") || "";
      api
        .listStandaloneSkills(cwd)
        .then((skills) => {
          preloadedSkills = skills;
          if (skills.length > 0 && store.availableSkills.length === 0) {
            store.availableSkills = skills.map((s) => s.name);
          }
          dbg("chat", "preloaded skills from filesystem", { count: skills.length });
        })
        .catch((e) => dbgWarn("chat", "failed to preload skills", e));
    }
  });

  // Listen for project folder changes to re-check project init
  onMount(() => {
    const handler = () => checkProjectInit();
    window.addEventListener("ocv:project-changed", handler);
    return () => window.removeEventListener("ocv:project-changed", handler);
  });

  // Check for pending plan from ExitPlanMode "clear context"
  onMount(() => {
    const pendingPlan = sessionStorage.getItem("ocv:pending-plan-prompt");
    const pendingCwd = sessionStorage.getItem("ocv:pending-plan-cwd");
    if (pendingPlan && !store.run) {
      sessionStorage.removeItem("ocv:pending-plan-prompt");
      sessionStorage.removeItem("ocv:pending-plan-cwd");
      const cwd = pendingCwd || localStorage.getItem("ocv:project-cwd") || "";
      dbg("chat", "auto-sending pending plan from ExitPlanMode clear context");
      // Use tick to ensure mount is complete
      tick().then(async () => {
        try {
          const newRunId = await store.startSession(pendingPlan, cwd, []);
          goto(`/chat?run=${newRunId}`);
          // Set permission mode to acceptEdits in new session
          // Wait for session to initialize first
          setTimeout(async () => {
            if (store.run) {
              await api.setPermissionMode(store.run.id, "acceptEdits");
              store.permissionMode = "acceptEdits";
              dbg("chat", "new session permission mode set to acceptEdits");
            }
          }, 2000);
        } catch (e) {
          dbgWarn("chat", "auto-send pending plan failed:", e);
        }
      });
    }
  });

  // Start middleware + register handlers
  onMount(() => {
    let destroyed = false;
    (async () => {
      try {
        await middleware.start();
      } catch (e) {
        console.error("[chat] middleware.start() failed:", e);
        store.error = t("chat_eventSystemFailed");
      }
      if (!destroyed) middlewareReady = true;
    })();

    // PTY handler: write binary data to xterm
    middleware.setPtyHandler({
      onOutput(payload) {
        if (store.run && payload.run_id === store.run.id && xtermRef) {
          try {
            const binary = atob(payload.data);
            const bytes = new Uint8Array(binary.length);
            for (let i = 0; i < binary.length; i++) {
              bytes[i] = binary.charCodeAt(i);
            }
            xtermRef.writeData(bytes);
          } catch {
            // ignore decode errors
          }
        }
      },
      onExit(payload) {
        if (store.run && payload.run_id === store.run.id) {
          store.handlePtyExit();
        }
      },
    });

    // Pipe handler: chat-delta / chat-done (Codex pipe mode)
    middleware.setPipeHandler({
      onDelta(delta) {
        store.handleChatDelta(delta.text, xtermRef);
      },
      onDone(done) {
        store.handleChatDone(done);
      },
    });

    // Run event handler: stderr for Codex pipe mode
    middleware.setRunEventHandler({
      onRunEvent(event) {
        if (
          store.run?.agent === "codex" &&
          store.run &&
          event.run_id === store.run.id &&
          xtermRef
        ) {
          if (event.type === "stderr") {
            xtermRef.writeText(`\x1b[31m${event.text}\x1b[0m\r\n`);
          }
        }
      },
    });

    return () => {
      destroyed = true;
      // Kill fork run process on unmount (but not the source run)
      if (forkOverlay?.active && store.run && store.run.id !== forkOverlay.sourceRunId) {
        api.stopSession(store.run.id).catch(() => {});
      }
      store.unmountGuards();
      middleware.destroy();
    };
  });

  // Watch runId changes → load run + subscribe middleware
  // Gated on middlewareReady to ensure listeners are registered before subscribing
  $effect(() => {
    if (!middlewareReady) return;
    const id = runId;
    const hasResume = $page.url.searchParams.has("resume");
    untrack(() => {
      middleware.subscribeCurrent(id, store);

      // Strongest guard: resume operation in progress — don't interfere.
      // Check both store guard (set inside resumeSession) and local flag
      // (set at handleResume entry, before store guard is acquired).
      if (store.resumeInFlight || resuming) {
        dbg("effect", "skip loadRun — resume in progress");
        return;
      }
      // Resume $effect will handle this case
      if (hasResume) return;

      if (!id) {
        store.loadRun("", xtermRef);
        cancelProgressive(); // empty run — no progressive needed
        return;
      }

      // If store already holds an active session for this run, skip redundant loadRun
      if (store.run?.id === id && store.sessionAlive) {
        dbg("effect", "skip loadRun — session already alive for", id);
        return;
      }

      loadRunProgressive(id, xtermRef);
    });
  });

  // Consume ?resume= URL param for session resume via sidebar button
  $effect(() => {
    const url = $page.url;
    const paramRunId = url.searchParams.get("run");
    const resumeMode = url.searchParams.get("resume") as SessionMode | null;

    if (paramRunId && resumeMode) {
      // Clean URL immediately to prevent re-trigger on refresh
      const clean = new URL(url);
      clean.searchParams.delete("resume");
      replaceState(clean, {});

      untrack(() => {
        handleResume(resumeMode, paramRunId);
      });
    }
  });

  // Auto-focus prompt input on mount + listen for status bar toggle + register chat keybindings
  onMount(() => {
    requestAnimationFrame(() => promptRef?.focus());
    function onStatusBarToggle(e: Event) {
      statusBarExpanded = (e as CustomEvent).detail.expanded;
    }
    window.addEventListener("ocv:statusbar-toggle", onStatusBarToggle);

    // Register chat-context keybinding callbacks
    keybindingStore.registerCallback("chat:interrupt", () => {
      if (shortcutHelpOpen) {
        shortcutHelpOpen = false;
        return;
      }
      if (store.isRunning) {
        store.interrupt();
      }
    });
    keybindingStore.registerCallback("chat:sendGlobal", () => {
      if (!store.isRunning) {
        promptRef?.triggerSend();
      }
    });
    keybindingStore.registerCallback("app:shortcutHelp", () => {
      shortcutHelpOpen = !shortcutHelpOpen;
    });
    keybindingStore.registerCallback("app:modelPicker", () => {
      statusBarRef?.openModelDropdown();
    });
    keybindingStore.registerCallback("chat:cyclePermission", () => {
      // Guard: if focus is on a focusable interactive control, don't cycle (preserve Shift+Tab navigation)
      const active = document.activeElement;
      if (active && active !== document.body) {
        const el = active as HTMLElement;
        const isFocusable =
          el.tagName === "BUTTON" ||
          el.tagName === "SELECT" ||
          el.tagName === "A" ||
          (el.hasAttribute("tabindex") && el.getAttribute("tabindex") !== "-1") ||
          el.closest("[role='menu']") ||
          el.closest("[role='listbox']") ||
          el.closest("[role='dialog']") ||
          (el.hasAttribute("role") &&
            ["button", "link", "menuitem", "option", "tab"].includes(
              el.getAttribute("role") ?? "",
            ));
        if (isFocusable) return;
      }
      const modes = ["default", "acceptEdits", "bypassPermissions", "plan", "delegate", "dontAsk"];
      const idx = modes.indexOf(store.permissionMode);
      const next = modes[(idx + 1) % modes.length];
      handlePermissionModeChange(next);
    });
    keybindingStore.registerCallback("chat:stashPrompt", () => {
      if (stashedInput) {
        promptRef?.restoreSnapshot(stashedInput);
        stashedInput = null;
        showChatToast(t("toast_stashRestored"));
      } else {
        const snapshot = promptRef?.getInputSnapshot();
        if (
          snapshot &&
          (snapshot.text.trim() || snapshot.attachments.length || snapshot.pastedBlocks.length)
        ) {
          stashedInput = snapshot;
          promptRef?.clearAll();
          showChatToast(t("toast_stashSaved"));
        }
      }
    });
    keybindingStore.registerCallback("app:toggleFastMode", () => {
      toggleCliConfigBool("fastMode");
    });
    keybindingStore.registerCallback("chat:toggleVerbose", () => {
      toggleCliConfigBool("verbose");
    });
    keybindingStore.registerCallback("chat:toggleTasks", () => {
      if (store.hasBackgroundTasks) {
        taskPanelCollapsed = !taskPanelCollapsed;
      }
    });
    keybindingStore.registerCallback("chat:undoLastTurn", () => {
      handleRewind();
    });

    // Screenshot event listener (global hotkey → attachment injection)
    const screenshotUnlisten = listen<ScreenshotPayload>("screenshot-taken", (event) => {
      dbg("chat", "screenshot-taken", { filename: event.payload.filename });
      const { contentBase64, mediaType, filename } = event.payload;
      const bytes = Uint8Array.from(atob(contentBase64), (c) => c.charCodeAt(0));
      const file = new File([bytes], filename, { type: mediaType });
      promptRef?.addFiles([file]);
    });

    return () => {
      window.removeEventListener("ocv:statusbar-toggle", onStatusBarToggle);
      keybindingStore.unregisterCallback("chat:interrupt");
      keybindingStore.unregisterCallback("chat:sendGlobal");
      keybindingStore.unregisterCallback("app:shortcutHelp");
      keybindingStore.unregisterCallback("app:modelPicker");
      keybindingStore.unregisterCallback("chat:cyclePermission");
      keybindingStore.unregisterCallback("chat:stashPrompt");
      keybindingStore.unregisterCallback("app:toggleFastMode");
      keybindingStore.unregisterCallback("chat:toggleVerbose");
      keybindingStore.unregisterCallback("chat:toggleTasks");
      keybindingStore.unregisterCallback("chat:undoLastTurn");
      screenshotUnlisten.then((fn) => fn());
      // Clean up verbose retry timer
      if (verboseRetryTimer) clearTimeout(verboseRetryTimer);
      // Clean up progressive rendering timer
      cancelProgressive();
    };
  });

  // Listen for auto-context snapshots from Rust backend
  onMount(() => {
    const unlisten = listen<{ runId: string; content: string; turnIndex: number; ts: string }>(
      "context-snapshot",
      (event) => {
        const { runId, content, turnIndex, ts } = event.payload;
        dbg("chat", "context-snapshot-recv", { runId, turnIndex, len: content.length });
        if (runId !== store.run?.id) return;
        const data = parseContextMarkdown(content);
        if (!data) {
          dbgWarn("chat", "context-parse-failed", {
            runId,
            turnIndex,
            head: content.slice(0, 200),
          });
          return;
        }
        // Upsert by turnIndex: same turn overwrites (not appends)
        const prev = contextHistoryMap.get(runId) ?? [];
        const existingIdx = prev.findIndex((s) => s.turnIndex === turnIndex);
        const replaced = existingIdx >= 0;
        const updated = replaced
          ? prev.map((s, i) => (i === existingIdx ? { runId, turnIndex, ts, data } : s))
          : [...prev, { runId, turnIndex, ts, data }];
        contextHistoryMap.set(runId, updated);
        contextHistoryMap = new Map(contextHistoryMap); // trigger reactivity
        dbg("chat", "context-snapshot", { turn: turnIndex, pct: data.percentage, replaced });
      },
    );
    return () => {
      unlisten.then((f) => f());
    };
  });

  // Auto-scroll chat
  $effect(() => {
    if (store.useStreamSession && chatAreaRef) {
      const _ = store.timeline.length + store.streamingText.length + (store.run?.id ?? "");
      if (isExpandingTimeline) return; // progressive expansion handles its own scrolling
      requestAnimationFrame(() => {
        if (chatAreaRef) {
          chatAreaRef.scrollTop = chatAreaRef.scrollHeight;
        }
      });
    }
  });

  // Restore model when store.model is empty (e.g. after reset/loadRun):
  // For third-party platforms, use the platform's default model.
  // For Anthropic, prefer CC's current active model, fall back to our saved default_model
  // (only if confirmed clean via three-state contamination check).
  $effect(() => {
    if (!store.model) {
      // Don't overwrite model during loadRun async gap — loadRun will set it
      if (store.phase === "loading") return;

      const isThirdParty = store.platformId && store.platformId !== "anthropic";
      if (isThirdParty) {
        const restoreCred = findCredential(settings?.platform_credentials ?? [], store.platformId);
        const restorePreset = PLATFORM_PRESETS.find((p) => p.id === store.platformId);
        const restoreModels = restoreCred?.models?.length
          ? restoreCred.models
          : restorePreset?.models;
        if (restoreModels?.[0]) {
          dbg("chat", "restore model from credential/preset", {
            platform: store.platformId,
            model: restoreModels[0],
          });
          store.model = restoreModels[0];
          return;
        }
      }
      // Only fall back to default_model for Anthropic platform — otherwise
      // default_model may belong to a different platform (cross-pollution).
      const cliModel = getCliCurrentModel();
      const isAnthropicPlatform = !store.platformId || store.platformId === "anthropic";
      const rawFallback = isAnthropicPlatform ? settings?.default_model : undefined;
      const contaminated = rawFallback ? isContaminatedDefaultModel(rawFallback) : null;
      // Only use default_model when confirmed clean (false). true/null → skip.
      const fallback = contaminated === false ? rawFallback : undefined;
      // Last resort: cached last-known-good Anthropic model (only for Anthropic platform)
      const model =
        cliModel || fallback || (isAnthropicPlatform ? lastKnownGoodAnthropicModel : undefined);
      if (model) {
        // Update cache when we have a trusted source
        if (isAnthropicPlatform && (cliModel || contaminated === false)) {
          lastKnownGoodAnthropicModel = model;
        }
        dbg("chat", "restore model", {
          cliModel,
          rawFallback,
          contaminated,
          lastKnownGood: lastKnownGoodAnthropicModel,
          using: model,
        });
        store.model = model;
      }
    }
  });

  // ── PTY helpers ──

  async function handleTermReady(cols: number, rows: number) {
    xtermReady = true;
    if (
      store.run &&
      !store.ptySpawned &&
      pendingMessage &&
      store.agent === "claude" &&
      !store.useStreamSession
    ) {
      await doSpawnPty(cols, rows);
    }
  }

  async function doSpawnPty(cols: number, rows: number) {
    if (!store.run || store.ptySpawned) return;
    try {
      await api.spawnPty(store.run.id, rows, cols);
      store.ptySpawned = true;
      store.phase = "running";
      pendingMessage = null;
      requestAnimationFrame(() => promptRef?.focus());
    } catch (e) {
      store.error = String(e);
    }
  }

  function handleTermResize(cols: number, rows: number) {
    if (!store.run || !store.ptySpawned) return;
    api.resizePty(store.run.id, rows, cols).catch((e) => dbgWarn("chat", "resizePty failed:", e));
  }

  function handleTermData(data: string) {
    if (!store.run || !store.ptySpawned) return;
    const bytes = new TextEncoder().encode(data);
    const b64 = btoa(String.fromCharCode(...bytes));
    api.writePty(store.run.id, b64).catch((e) => dbgWarn("chat", "writePty failed:", e));
  }

  // ── Send message ──

  async function sendMessage(text: string, attachments: Attachment[]) {
    if (!text.trim()) return;

    store.error = "";

    // Detect slash command (same check as store timeout skip)
    const isSlash = store.isKnownSlashCommand(text);
    const slashCmd = isSlash ? (text.match(/^\/\S+/)?.[0] ?? null) : null;

    try {
      if (!store.run) {
        // First message: create run
        let cwd =
          typeof window !== "undefined"
            ? localStorage.getItem("ocv:project-cwd") ||
              localStorage.getItem("ocv:settings-cwd") ||
              ""
            : "";

        if (!cwd || cwd === "/") {
          const { open } = await import("@tauri-apps/plugin-dialog");
          const selected = await open({
            directory: true,
            title: t("layout_selectProjectFolder"),
          });
          if (!selected) return; // user cancelled → don't send
          cwd = selected as string;
          localStorage.setItem("ocv:project-cwd", cwd);
          window.dispatchEvent(new Event("ocv:cwd-changed"));
        }

        // Set indicator AFTER all early-return points
        if (slashCmd) {
          processingSlashCmd = slashCmd;
          slashCmdSeenRunning = false;
        }

        const runId = await store.startSession(text, cwd, attachments);
        goto(`/chat?run=${runId}`, { replaceState: true });
        window.dispatchEvent(new Event("ocv:runs-changed"));

        // CLI PTY mode: queue message and spawn PTY
        if (!store.useStreamSession && store.agent === "claude") {
          pendingMessage = { text, attachments };
          if (xtermReady && xtermRef) {
            await doSpawnPty(80, 24);
          }
        }
      } else if (store.useStreamSession && !store.sessionAlive && store.run.session_id) {
        // Stopped stream session: atomic resume + send (message written to CLI stdin at spawn)
        dbg("chat", "auto-resume on send", {
          runId: store.run.id,
          sessionId: store.run.session_id,
        });
        if (slashCmd) {
          processingSlashCmd = slashCmd;
          slashCmdSeenRunning = false;
        }
        await handleResume("resume", undefined, text, attachments);
      } else {
        // Subsequent message
        if (slashCmd) {
          processingSlashCmd = slashCmd;
          slashCmdSeenRunning = false;
        }
        await store.sendMessage(text, attachments);
        requestAnimationFrame(() => promptRef?.focus());
      }
    } catch (e) {
      store.error = String(e);
      processingSlashCmd = null;
    }
  }

  function fillPrompt(text: string) {
    promptRef?.setValue(text);
  }

  // ── Project init detection ──
  let showInitHint = $derived(
    projectInitStatus !== null && !projectInitStatus.has_claude_md && !store.run,
  );

  async function checkProjectInit() {
    const cwd = localStorage.getItem("ocv:project-cwd") || "";
    if (!cwd || cwd === "/") {
      projectInitStatus = null;
      dbg("chat", "checkProjectInit: skip (no cwd)");
      return;
    }
    const seq = ++initCheckSeq;
    try {
      const status = await api.checkProjectInit(cwd);
      dbg("chat", "checkProjectInit result", {
        cwd,
        status,
        seq,
        currentSeq: initCheckSeq,
        hasRun: !!store.run,
        isApiMode: store.isApiMode,
      });
      if (seq !== initCheckSeq) return;
      const dismissKey = `ocv:init-dismissed:${status.cwd}`;
      const dismissed = localStorage.getItem(dismissKey);
      if (dismissed) {
        projectInitStatus = null;
        dbg("chat", "checkProjectInit: dismissed", dismissKey);
        return;
      }
      projectInitStatus = status;
    } catch (e) {
      dbgWarn("chat", "checkProjectInit failed", e);
      if (seq === initCheckSeq) projectInitStatus = null;
    }
  }

  function dismissInitHint() {
    if (projectInitStatus?.cwd) {
      localStorage.setItem(`ocv:init-dismissed:${projectInitStatus.cwd}`, "1");
    }
    projectInitStatus = null;
    dbg("chat", "init hint dismissed");
  }

  // ── Permission mode name translation ──
  // Store/dropdown use CLI names; UserSettings uses app names; adapter.rs maps app→CLI.
  const CLI_TO_APP_MODE: Record<string, string> = {
    default: "ask",
    acceptEdits: "auto_read",
    bypassPermissions: "auto_all",
    plan: "plan",
    delegate: "delegate",
    dontAsk: "dont_ask",
  };
  const APP_TO_CLI_MODE: Record<string, string> = {
    ask: "default",
    auto_read: "acceptEdits",
    auto_all: "bypassPermissions",
    plan: "plan",
    delegate: "delegate",
    dont_ask: "dontAsk",
  };

  function getPermModeLabel(mode: string): string {
    const map: Record<string, () => string> = {
      default: () => t("prompt_permAskShort"),
      acceptEdits: () => t("prompt_permAutoReadShort"),
      bypassPermissions: () => t("prompt_permAutoAllShort"),
      plan: () => t("prompt_permPlanShort"),
      delegate: () => t("prompt_permDelegateShort"),
      dontAsk: () => t("prompt_permDontAskShort"),
    };
    return map[mode]?.() ?? mode;
  }

  async function handlePermissionModeChange(
    newMode: string,
    opts?: { toast?: boolean },
  ): Promise<boolean> {
    const oldMode = store.permissionMode;
    dbg("chat", "permission mode change", { from: oldMode, to: newMode });

    // Optimistic UI update
    store.permissionMode = newMode;

    if (store.sessionAlive && store.run) {
      // Active session: hot-switch via control protocol (CLI expects CLI names)
      try {
        await api.setPermissionMode(store.run.id, newMode);
        dbg("chat", "permission mode changed via control protocol", { newMode });
      } catch (e) {
        // Revert on failure
        store.permissionMode = oldMode;
        dbgWarn("chat", "permission mode change failed:", e);
        store.error = t("chat_permModeFailed", { mode: newMode, error: String(e) });
        if (opts?.toast !== false) {
          showChatToast(t("toast_permissionFailed"));
        }
        return false;
      }
    }

    if (opts?.toast !== false) {
      showChatToast(t("toast_permissionMode", { mode: getPermModeLabel(newMode) }));
    }

    // Persist to user settings (uses app names for adapter.rs compatibility)
    const appName = CLI_TO_APP_MODE[newMode] ?? newMode;
    try {
      await api.updateUserSettings({ permission_mode: appName });
    } catch (e) {
      dbgWarn("chat", "permission mode persist failed:", e);
    }

    // Sync legacy plan_mode boolean for backward compat
    try {
      await api.updateAgentSettings("claude", { plan_mode: newMode === "plan" });
    } catch (e) {
      dbgWarn("chat", "plan_mode sync failed:", e);
    }

    return true;
  }

  async function handleModelChange(newModel: string) {
    dbg("chat", "model change", { from: store.model, to: newModel });
    store.model = newModel;

    const isThirdParty = store.platformId && store.platformId !== "anthropic";

    // Hot-switch model if session is alive (only for Anthropic — third-party models
    // are set via ANTHROPIC_MODEL env var at spawn time, not via control protocol)
    if (!isThirdParty && store.sessionAlive && store.run) {
      try {
        await api.sendSessionControl(store.run.id, "set_model", { model: newModel });
        dbg("chat", "model hot-switched via control protocol");
      } catch (e) {
        dbgWarn("chat", "model hot-switch failed, will use new model on next session", e);
      }
    }

    // Persist model to run meta (per-run model memory)
    if (store.run) {
      api.updateRunModel(store.run.id, newModel).catch((e) => {
        dbgWarn("chat", "failed to persist run model", e);
      });
    }

    // Only persist default_model for Anthropic — third-party models managed per-credential
    if (!isThirdParty) {
      lastKnownGoodAnthropicModel = newModel;
      try {
        await api.updateUserSettings({ default_model: newModel });
      } catch (e) {
        dbgWarn("chat", "failed to persist model change", e);
      }
    }
  }

  async function handleAuthModeChange(mode: string) {
    dbg("chat", "auth mode change", { from: store.authMode, to: mode });
    store.authMode = mode;
    try {
      await api.updateUserSettings({ auth_mode: mode } as Partial<UserSettings>);
      // Refresh auth overview after mode change
      authOverview = await api.getAuthOverview();
    } catch (e) {
      dbgWarn("chat", "failed to persist auth mode change", e);
    }
  }

  async function handlePlatformChange(platformId: string) {
    dbg("chat", "platform change", { from: store.platformId, to: platformId });
    store.platformId = platformId;

    // Auto-switch model to provider's default when switching to a third-party platform
    // Priority: credential.models (user-configured) > preset.models (static defaults)
    const cred = findCredential(settings?.platform_credentials ?? [], platformId);
    const preset = PLATFORM_PRESETS.find((p) => p.id === platformId);
    const models = cred?.models?.length ? cred.models : preset?.models;
    if (models?.length) {
      const defaultModel = models[0];
      dbg("chat", "auto-switch model for platform", { platformId, model: defaultModel });
      store.model = defaultModel;
    } else if (platformId === "anthropic") {
      // Switching back to Anthropic: always overwrite — don't keep third-party model;
      // don't fallback to settings.default_model which might be contaminated.
      const cliModel = getCliCurrentModel();
      store.model = cliModel || "";
      dbg("chat", "restore model on switch to anthropic", { cliModel, using: store.model });
    } else {
      // Custom/unknown platform without preset models: clear model
      // (let CLI use whatever default it has, or the user can set manually)
      store.model = "";
    }

    // Only persist default_model when switching to Anthropic with a validated CLI model.
    // Don't persist empty or potentially-stale model values.
    const persistUpdate: Partial<UserSettings> = { active_platform_id: platformId };
    if (platformId === "anthropic") {
      const validated = getCliCurrentModel();
      if (validated) persistUpdate.default_model = validated;
    }
    try {
      await api.updateUserSettings(persistUpdate);
    } catch (e) {
      dbgWarn("chat", "failed to persist platform change", e);
    }
  }

  function appendCommandOutput(text: string) {
    store.timeline = [
      ...store.timeline,
      {
        kind: "command_output",
        id: crypto.randomUUID(),
        content: text,
        ts: new Date().toISOString(),
      },
    ];
  }

  async function handleRename(name: string) {
    if (!store.run) return;
    try {
      await api.renameRun(store.run.id, name);
      store.run = { ...store.run, name };
      window.dispatchEvent(new Event("ocv:runs-changed"));
      dbg("chat", "renamed run", { id: store.run.id, name });
    } catch (e) {
      dbgWarn("chat", "rename failed", e);
    }
  }

  // Auto-name: on first idle, generate title from prompt
  $effect(() => {
    if (store.phase === "idle" && store.run && !store.run.name && store.run.prompt) {
      const firstLine = store.run.prompt.split("\n")[0].trim();
      const autoName = firstLine.length > 40 ? firstLine.slice(0, 40) + "…" : firstLine;
      if (autoName) {
        handleRename(autoName);
      }
    }
  });

  async function handleVirtualCommand(action: string, args: string) {
    dbg("chat", "virtualCommand", { action, args });
    if (action === "copy-last") {
      const lastAssistant = [...store.timeline].reverse().find((e) => e.kind === "assistant");
      if (lastAssistant && lastAssistant.kind === "assistant" && lastAssistant.content) {
        try {
          await navigator.clipboard.writeText(lastAssistant.content);
          const chars = lastAssistant.content.length;
          const lines = lastAssistant.content.split("\n").length;
          appendCommandOutput(
            t("chat_copiedClipboard", { chars: String(chars), lines: String(lines) }),
          );
          dbg("chat", "copied last response", { chars, lines });
        } catch (e) {
          dbgWarn("chat", "copy failed", e);
          appendCommandOutput(t("chat_copyFailed"));
        }
      } else {
        appendCommandOutput(t("chat_noResponseToCopy"));
      }
    } else if (action === "rename-session") {
      if (!store.run) {
        appendCommandOutput(t("chat_noSessionToRename"));
        return;
      }
      if (args) {
        // With args: rename locally
        await handleRename(args);
        appendCommandOutput(t("chat_sessionRenamed", { name: args }));
      } else if (store.sessionAlive) {
        // No args + session alive: send /rename to CLI (AI-generated name)
        await sendMessage("/rename", []);
      } else {
        appendCommandOutput("Usage: /rename <name>");
      }
    } else if (action === "toggle-plan") {
      const entering = store.permissionMode !== "plan";
      const newMode = entering ? "plan" : "default";
      const ok = await handlePermissionModeChange(newMode, { toast: false });
      if (ok) {
        appendCommandOutput(entering ? "Plan mode enabled" : "Plan mode disabled");
        // If instructions provided, send them as a message
        if (args && entering) {
          await sendMessage(args, []);
        }
      }
      // On failure, handlePermissionModeChange already sets store.error
    } else if (action === "show-help") {
      const allCmds = mergeWithVirtual(
        store.sessionInitReceived && store.sessionCommands.length > 0
          ? store.sessionCommands
          : getCliCommands(),
      );
      const skillSet = new Set(store.availableSkills);
      appendCommandOutput(buildHelpText(allCmds, skillSet));
    } else if (action === "run-doctor") {
      try {
        dbg("doctor", "run-doctor triggered", { cwd: store.effectiveCwd });
        const cwd = store.effectiveCwd || localStorage.getItem("ocv:project-cwd") || "";
        const mcpSvrs = store.sessionAlive ? store.mcpServers : undefined;
        const report = await buildDoctorReport(cwd, mcpSvrs);
        appendCommandOutput(report);
      } catch (err) {
        dbgWarn("doctor", "run_diagnostics failed", err);
        appendCommandOutput(
          `❌ ${t("doctor_failed")}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    } else if (action === "show-status") {
      if (!hasSidebarData) {
        appendCommandOutput(t("statusPanel_noSession"));
        return;
      }
      if (sidebarCollapsed) sidebarCollapsed = false;
      sidebarRequestedTab = "info";
    } else if (action === "list-todos") {
      // Escape markdown special chars in todo content
      const esc = (s: string) => s.replace(/([\\*_~`[\]#>|])/g, "\\$1");

      // Find the last TodoWrite tool_end in timeline with newTodos
      const lastTodo = [...store.timeline]
        .reverse()
        .find(
          (e): e is Extract<TimelineEntry, { kind: "tool" }> =>
            e.kind === "tool" &&
            e.tool.tool_name === "TodoWrite" &&
            e.tool.status === "success" &&
            e.tool.tool_use_result != null &&
            typeof e.tool.tool_use_result === "object" &&
            "newTodos" in e.tool.tool_use_result &&
            Array.isArray(e.tool.tool_use_result.newTodos),
        );

      if (lastTodo) {
        const todos = lastTodo.tool.tool_use_result!.newTodos as Array<{
          content: string;
          status: "pending" | "in_progress" | "completed";
        }>;
        if (todos.length === 0) {
          appendCommandOutput(t("todos_empty"));
        } else {
          const lines = todos.map((td) => {
            const text = esc(td.content);
            if (td.status === "completed") return `- [x] ~~${text}~~`;
            if (td.status === "in_progress") return `- [ ] **⏳ ${text}**`;
            return `- [ ] ${text}`;
          });
          appendCommandOutput(lines.join("\n"));
        }
      } else {
        // No TodoWrite in timeline — show local prompt
        // CLI /todos is an internal command that doesn't produce timeline events,
        // so fallback to sendMessage would just create an empty turn.
        appendCommandOutput(t("todos_empty"));
      }
    } else if (action === "show-diff") {
      const cwd = store.effectiveCwd || localStorage.getItem("ocv:project-cwd") || "";
      if (!cwd) {
        appendCommandOutput(t("diff_noCwd"));
        return;
      }
      try {
        dbg("chat", "show-diff", { cwd });
        const [unstaged, staged] = await Promise.all([
          api.getGitDiff(cwd, false),
          api.getGitDiff(cwd, true),
        ]);
        if (!unstaged.trim() && !staged.trim()) {
          appendCommandOutput(t("diff_noChanges"));
          return;
        }
        // Add source-file line numbers parsed from @@ hunk headers
        function addLineNumbers(raw: string): string {
          const lines = raw.split("\n");
          const out: string[] = [];
          let oldLn = 0,
            newLn = 0;
          for (const line of lines) {
            if (line.startsWith("@@")) {
              const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
              if (m) {
                oldLn = parseInt(m[1]);
                newLn = parseInt(m[2]);
              }
              out.push(line);
            } else if (
              line.startsWith("diff ") ||
              line.startsWith("index ") ||
              line.startsWith("--- ") ||
              line.startsWith("+++ ")
            ) {
              out.push(line);
            } else if (line.startsWith("+")) {
              out.push(`+${String(newLn).padStart(4)} │ ${line.slice(1)}`);
              newLn++;
            } else if (line.startsWith("-")) {
              out.push(`-${String(oldLn).padStart(4)} │ ${line.slice(1)}`);
              oldLn++;
            } else if (line.length > 0 && line[0] === " ") {
              out.push(` ${String(newLn).padStart(4)} │ ${line.slice(1)}`);
              oldLn++;
              newLn++;
            } else {
              out.push(line);
            }
          }
          return out.join("\n");
        }
        const parts: string[] = [];
        if (unstaged.trim()) {
          parts.push(
            `### ${t("diff_unstaged")}\n\n\`\`\`diff\n${addLineNumbers(unstaged.trimEnd())}\n\`\`\``,
          );
        }
        if (staged.trim()) {
          parts.push(
            `### ${t("diff_staged")}\n\n\`\`\`diff\n${addLineNumbers(staged.trimEnd())}\n\`\`\``,
          );
        }
        appendCommandOutput(parts.join("\n\n"));
      } catch (err) {
        dbgWarn("chat", "show-diff failed", err);
        appendCommandOutput(
          `${t("diff_failed")}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    } else if (action === "list-tasks") {
      const tasks = [...store.taskNotifications.values()];

      if (!args) {
        // /tasks (no args) — list all tasks as a table
        dbg("chat", "list-tasks", { count: store.taskNotifications.size });
        if (tasks.length === 0) {
          appendCommandOutput(t("slashTasks_empty"));
          return;
        }
        // Sort: active first, then by most recent
        const sorted = tasks.sort((a, b) => {
          const aActive =
            a.status !== "completed" && a.status !== "failed" && a.status !== "error" ? 1 : 0;
          const bActive =
            b.status !== "completed" && b.status !== "failed" && b.status !== "error" ? 1 : 0;
          if (aActive !== bActive) return bActive - aActive;
          return b.startedAt - a.startedAt;
        });
        const now = Date.now();
        const elapsed = (ms: number) => {
          const sec = Math.floor((now - ms) / 1000);
          if (sec < 60) return `${sec}s`;
          const min = Math.floor(sec / 60);
          if (min < 60) return `${min}m`;
          return `${Math.floor(min / 60)}h${min % 60}m`;
        };
        const lines: string[] = [
          "| ID | Type | Status | Description | Elapsed |",
          "|-----|------|--------|-------------|---------|",
        ];
        for (const task of sorted) {
          const shortId = task.task_id.length > 12 ? task.task_id.slice(0, 12) + "…" : task.task_id;
          const taskType = task.task_type || "—";
          const desc =
            (task.summary || task.message || "").length > 50
              ? (task.summary || task.message || "").slice(0, 50) + "…"
              : task.summary || task.message || "—";
          lines.push(
            `| \`${shortId}\` | ${taskType} | ${task.status} | ${desc} | ${elapsed(task.startedAt)} |`,
          );
        }
        lines.push("");
        lines.push(t("slashTasks_hint"));
        appendCommandOutput(lines.join("\n"));
      } else {
        // /tasks <id> — show detail for a specific task
        dbg("chat", "list-tasks:detail", { id: args });

        // Exact match first, then prefix match
        let matches = tasks.filter((t) => t.task_id === args);
        if (matches.length === 0) {
          matches = tasks.filter((t) => t.task_id.startsWith(args));
        }

        if (matches.length === 0) {
          dbg("chat", "list-tasks:detail", { id: args, found: false });
          appendCommandOutput(t("slashTasks_notFound", { id: args }));
        } else if (matches.length === 1) {
          const task = matches[0];
          const hasOutput = !!task.output_file;
          dbg("chat", "list-tasks:detail", { id: args, found: true, hasOutput });
          const now = Date.now();
          const sec = Math.floor((now - task.startedAt) / 1000);
          const meta = [
            `| Field | Value |`,
            `|-------|-------|`,
            `| ID | \`${task.task_id}\` |`,
            `| Status | ${task.status} |`,
            `| Type | ${task.task_type || "—"} |`,
            `| Description | ${task.message || "—"} |`,
            task.summary ? `| Summary | ${task.summary} |` : null,
            `| Elapsed | ${sec}s |`,
            task.output_file ? `| Output file | \`${task.output_file}\` |` : null,
          ]
            .filter(Boolean)
            .join("\n");

          if (task.output_file) {
            try {
              const raw = await api.readTaskOutput(task.output_file);
              dbg("chat", "readTaskOutput", { path: task.output_file, ok: true });
              // Frontend truncation: last 200 lines
              const allLines = raw.split("\n");
              const trimmed =
                allLines.length > 200
                  ? `... (${allLines.length - 200} lines truncated)\n${allLines.slice(-200).join("\n")}`
                  : raw;
              appendCommandOutput(`${meta}\n\n**Output:**\n\`\`\`\n${trimmed}\n\`\`\``);
            } catch (err) {
              dbgWarn("chat", "readTaskOutput failed", err);
              appendCommandOutput(
                `${meta}\n\n${t("slashTasks_outputError", { error: err instanceof Error ? err.message : String(err) })}`,
              );
            }
          } else {
            appendCommandOutput(meta);
          }
        } else {
          // Multiple matches — ambiguous
          const list = matches.map((m) => `- \`${m.task_id}\` (${m.status})`).join("\n");
          appendCommandOutput(`${t("slashTasks_ambiguous", { id: args })}\n${list}`);
        }
      }
    } else if (action === "add-dir") {
      try {
        await executeAddDir(
          { agent: store.agent, sessionAlive: store.sessionAlive, args },
          {
            openDirectoryDialog: async (title) => {
              const { open } = await import("@tauri-apps/plugin-dialog");
              const result = await open({ directory: true, title });
              return typeof result === "string" ? result : null;
            },
            sendMessage: (text) => sendMessage(text, []),
            getAgentSettings: api.getAgentSettings,
            updateAgentSettings: api.updateAgentSettings,
            appendOutput: appendCommandOutput,
            t,
          },
        );
      } catch (err) {
        dbgWarn("chat", "add-dir failed", err);
        appendCommandOutput(
          t("chat_addDirFailed", {
            error: err instanceof Error ? err.message : String(err),
          }),
        );
      }
    }
  }

  async function handleStop() {
    await store.stop();
    window.dispatchEvent(new Event("ocv:runs-changed"));
  }

  async function handleResume(
    mode: SessionMode,
    overrideRunId?: string,
    initialMessage?: string,
    initialAttachments?: Attachment[],
  ) {
    const targetRunId = overrideRunId ?? store.run?.id;
    if (!targetRunId || resuming) return;
    resuming = true;

    // Per-session platform: resume automatically uses run's saved platform_id
    // via backend resolve_auth_env_for_platform() — no mismatch dialog needed.

    // Fork: activate overlay immediately for progress feedback
    if (mode === "fork") {
      forkOverlay = { active: true, sourceRunId: targetRunId, startedAt: Date.now(), error: null };
    }

    try {
      // Fork: don't subscribe to source — backend emits RunState(stopped)
      // for the source which would interfere with the fork state machine.
      if (mode !== "fork") {
        middleware.subscribeCurrent(targetRunId, store);
      }
      const resultId = await store.resumeSession(
        targetRunId,
        mode,
        initialMessage,
        initialAttachments,
      );
      if (resultId) {
        middleware.subscribeCurrent(resultId, store);
        if (mode === "fork") {
          // Check if user cancelled during fork_oneshot
          if (!forkOverlay) {
            dbg("chat", "fork: cancelled during fork_oneshot, skipping step 2");
          } else {
            // Step 1 complete — dismiss overlay, use normal session startup UI for step 2
            forkOverlay = null;
            goto(`/chat?run=${resultId}`, { replaceState: true });
            // Step 2: establish stream-json connection (shows "Starting session..." spinner)
            try {
              await store.connectSession(resultId);
            } catch (e) {
              store.error = String(e);
            }
          }
        } else {
          goto(`/chat?run=${resultId}`, { replaceState: true });
        }
      } else if (mode === "fork") {
        // Fork failed — don't clear overlay or navigate away.
        // The phase watcher $effect will show the error in the overlay.
        // User can Retry or Cancel from there.
        dbg("chat", "fork failed, keeping overlay for retry/cancel");
      } else {
        // Non-fork resume failed — stay on the target run's view instead of
        // navigating to blank new-session page (the run's history is still useful).
        lastContinuableRun = null;
        goto(`/chat?run=${targetRunId}`, { replaceState: true });
      }
      window.dispatchEvent(new Event("ocv:runs-changed"));
    } catch (e) {
      // Fork sync failure → show error in overlay instead of error bar
      if (mode === "fork" && forkOverlay) {
        forkOverlay = { ...forkOverlay, error: String(e) };
      }
    } finally {
      resuming = false;
    }
  }

  /** Stop the fork run's process (if it exists and isn't the source run). */
  async function stopForkProcess(sourceRunId: string) {
    if (store.run && store.run.id !== sourceRunId) {
      try {
        await api.stopSession(store.run.id);
      } catch {
        /* best-effort */
      }
    }
  }

  async function handleForkCancel() {
    if (!forkOverlay) return;
    const sourceRunId = forkOverlay.sourceRunId;
    await stopForkProcess(sourceRunId);
    forkOverlay = null;
    store.error = "";
    goto(`/chat?run=${sourceRunId}`, { replaceState: true });
    // Explicit reload — URL may not change if we're returning to the same run
    await loadRunProgressive(sourceRunId);
    window.dispatchEvent(new Event("ocv:runs-changed"));
  }

  async function handleForkRetry() {
    if (!forkOverlay || resuming) return;
    const sourceRunId = forkOverlay.sourceRunId;
    await stopForkProcess(sourceRunId);
    forkOverlay = { active: true, sourceRunId, startedAt: Date.now(), error: null };
    store.error = "";
    await handleResume("fork", sourceRunId);
  }

  // ── Chat-level toast (same pattern as PromptInput's showFileToast) ──
  let chatToast = $state<string | null>(null);
  let chatToastTimeout: ReturnType<typeof setTimeout> | null = null;
  function showChatToast(msg: string) {
    chatToast = msg;
    if (chatToastTimeout) clearTimeout(chatToastTimeout);
    chatToastTimeout = setTimeout(() => {
      chatToast = null;
    }, 2500);
  }

  async function toggleCliConfigBool(key: string) {
    try {
      const config = await api.getCliConfig();
      const current = config[key] === true;
      await api.updateCliConfig({ [key]: !current });
      dbg("chat", `toggled ${key}`, { from: current, to: !current });
      // Immediately mirror UI state
      if (key === "fastMode") {
        store.fastModeState = !current ? "on" : "";
        dbg("chat", "fastMode UI mirrored", { state: store.fastModeState });
      } else if (key === "verbose") {
        verboseEnabled = !current;
        dbg("chat", "verbose UI mirrored", { verbose: verboseEnabled });
      }
      const label =
        key === "fastMode"
          ? !current
            ? "toast_fastModeOn"
            : "toast_fastModeOff"
          : !current
            ? "toast_verboseOn"
            : "toast_verboseOff";
      showChatToast(t(label as Parameters<typeof t>[0]));
    } catch (e) {
      dbgWarn("chat", `toggle ${key} failed:`, e);
    }
  }

  // Chat keybinding callbacks — registered/unregistered via keybindingStore in onMount below

  // ── Page-level drag-drop (forward to PromptInput) ──
  let pageDragCounter = $state(0);
  let pageDragActive = $derived(pageDragCounter > 0);

  function handlePageDragEnter(e: DragEvent) {
    e.preventDefault();
    pageDragCounter++;
  }
  function handlePageDragLeave(e: DragEvent) {
    e.preventDefault();
    pageDragCounter--;
  }
  function handlePageDragOver(e: DragEvent) {
    e.preventDefault();
  }
  function handlePageDrop(e: DragEvent) {
    e.preventDefault();
    pageDragCounter = 0;
    const files = e.dataTransfer?.files;
    if (files && promptRef) {
      promptRef.addFiles(files);
    }
  }

  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
  }

  async function scrollToTool(toolUseId: string) {
    // Cancel progressive rendering so full timeline is available
    if (renderLimit !== Infinity) {
      cancelProgressive();
      await tick();
    }
    // Clear filter first (target tool may be filtered out)
    if (toolFilter) {
      toolFilter = null;
      await tick();
    }
    const el = document.getElementById("tool-" + toolUseId);
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
      el.classList.add("ring-2", "ring-primary/50");
      setTimeout(() => el.classList.remove("ring-2", "ring-primary/50"), 2000);
    }
  }

  async function handleToolAnswer(toolUseId: string, answer: string) {
    await store.answerToolQuestion(toolUseId, answer);
  }

  async function handleRewind() {
    if (!store.run || !store.sessionAlive) return;
    try {
      await api.rewindFiles(store.run.id);
      dbg("chat", "rewind triggered");
      showChatToast(t("toast_rewindSuccess"));
    } catch (e) {
      dbgWarn("chat", "rewind failed:", e);
      store.error = String(e);
    }
  }

  async function handleToolApprove(toolName: string) {
    if (!store.run) return;
    approving = true;
    dbg("chat", "approving tool", { runId: store.run.id, toolName });
    try {
      await api.approveSessionTool(store.run.id, toolName);
    } catch (e) {
      dbgWarn("chat", "approve failed:", e);
      store.error = String(e);
    } finally {
      // approving resets when new RunState events arrive (spawning/running)
      setTimeout(() => {
        approving = false;
      }, 3000);
    }
  }

  async function handlePermissionRespond(
    requestId: string,
    behavior: "allow" | "deny",
    updatedPermissions?: import("$lib/types").PermissionSuggestion[],
    updatedInput?: Record<string, unknown>,
    denyMessage?: string,
    interrupt?: boolean,
  ) {
    if (!store.run) return;
    dbg("chat", "inline permission respond", {
      runId: store.run.id,
      requestId,
      behavior,
      updatedPermissions,
      updatedInput,
      denyMessage,
      interrupt,
    });
    try {
      // Set pending mode override BEFORE responding (so reducer picks it up)
      if (behavior === "allow" && updatedPermissions) {
        const modePerm = updatedPermissions.find((p) => p.type === "setMode");
        if (modePerm && modePerm.mode) {
          store.pendingPermissionModeOverride = modePerm.mode;
          dbg("chat", "set pendingPermissionModeOverride", { mode: modePerm.mode });
        }
      }

      await api.respondPermission(
        store.run.id,
        requestId,
        behavior,
        updatedPermissions,
        updatedInput,
        denyMessage,
        interrupt,
      );
      // Optimistic local update: CLI doesn't emit a separate event for deny
      if (behavior === "deny") {
        store.resolvePermissionDeny(requestId);
      }
    } catch (e) {
      dbgWarn("chat", "permission respond failed:", e);
      // If the CLI rejected the response (e.g. session already idle after interrupt),
      // still resolve the card locally so buttons are removed.
      if (behavior === "deny") {
        store.resolvePermissionDeny(requestId);
      }
      store.error = String(e);
    }
  }

  async function handleExitPlanClearContext() {
    if (!store.run) return;
    const runId = store.run.id;
    const cwd = localStorage.getItem("ocv:project-cwd") || "";
    dbg("chat", "ExitPlanMode: clear context + auto-accept");

    // Find the ExitPlanMode tool's permission request ID from timeline
    const exitPlanEntry = store.timeline.find(
      (e) =>
        e.kind === "tool" &&
        e.tool.tool_name === "ExitPlanMode" &&
        e.tool.status === "permission_prompt" &&
        e.tool.permission_request_id,
    );
    if (!exitPlanEntry || exitPlanEntry.kind !== "tool") return;
    const requestId = exitPlanEntry.tool.permission_request_id!;

    try {
      // 1. Set flags BEFORE responding
      store.pendingPermissionModeOverride = "acceptEdits";
      store.pendingClearContextPlan = "__pending__"; // marker: waiting for tool_end

      // 2. Allow ExitPlanMode (with setMode) — satisfies the control_response requirement
      await api.respondPermission(
        runId,
        requestId,
        "allow",
        [{ type: "setMode", mode: "acceptEdits", destination: "session" }],
        exitPlanEntry.tool.input,
      );

      // 3. Wait for tool_end to deliver plan content (via pendingClearContextPlan)
      //    Poll briefly — tool_end should arrive within a few hundred ms
      let planContent: string | null = null;
      for (let i = 0; i < 20; i++) {
        await new Promise((r) => setTimeout(r, 200));
        if (store.pendingClearContextPlan && store.pendingClearContextPlan !== "__pending__") {
          planContent = store.pendingClearContextPlan;
          break;
        }
      }
      store.pendingClearContextPlan = null;

      if (!planContent) {
        dbgWarn("chat", "ExitPlanMode: timed out waiting for plan content");
        // Fallback: continue in current session (ExitPlanMode already allowed)
        return;
      }

      // 4. Interrupt + stop current session
      await api.interruptSession(runId).catch(() => {});
      await api.stopSession(runId);
      dbg("chat", "ExitPlanMode: session stopped");

      // 5. Navigate to fresh chat and schedule plan sending
      const planPrompt = `Implement the following plan:\n\n${planContent}`;
      sessionStorage.setItem("ocv:pending-plan-prompt", planPrompt);
      sessionStorage.setItem("ocv:pending-plan-cwd", cwd);
      goto("/chat");
      // The fresh chat page mount will detect sessionStorage items and auto-send
    } catch (e) {
      dbgWarn("chat", "ExitPlanMode clear context failed:", e);
      store.pendingClearContextPlan = null;
      store.error = String(e);
    }
  }

  async function handleHookCallbackRespond(requestId: string, decision: "allow" | "deny") {
    if (!store.run) return;
    dbg("chat", "hook callback respond", { runId: store.run.id, requestId, decision });
    try {
      await api.respondHookCallback(store.run.id, requestId, decision);
      // Update hook event status in store
      store.hookEvents = store.hookEvents.map((h) =>
        h.request_id === requestId
          ? { ...h, status: decision === "allow" ? ("allowed" as const) : ("denied" as const) }
          : h,
      );
    } catch (e) {
      dbgWarn("chat", "hook callback respond failed:", e);
      store.error = String(e);
    }
  }
</script>

{#snippet initHintCard()}
  {#if showInitHint}
    <div class="mt-3 flex items-center gap-2 text-[11px] text-amber-400/80">
      <svg
        class="h-3.5 w-3.5 shrink-0"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M12 9v4" /><path d="M12 17h.01" />
        <path d="M3.6 15.4 10.2 4a2 2 0 0 1 3.6 0l6.6 11.4a2 2 0 0 1-1.8 3H5.4a2 2 0 0 1-1.8-3Z" />
      </svg>
      <span>
        Run
        <button
          class="font-mono text-amber-300 hover:text-amber-200 underline underline-offset-2 transition-colors"
          onclick={() => sendMessage("/init", [])}>{t("chat_initHintAction")}</button
        >
        to create CLAUDE.md
      </span>
      <button
        class="ml-auto text-muted-foreground/40 hover:text-muted-foreground/70 transition-colors"
        onclick={dismissInitHint}
        title={t("chat_initHintDismiss")}
      >
        <svg
          class="h-3 w-3"
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
  {/if}
{/snippet}

{#snippet heroMetaItems()}
  {@const hasUpdate = !!(
    cliVersionInfo?.installed &&
    channelLatest &&
    cliVersionInfo.installed !== channelLatest
  )}
  {#if cliVersionInfo?.installed}
    <button
      class="tabular-nums hover:text-muted-foreground transition-colors"
      onclick={() => goto("/release-notes")}
    >
      {t("chat_cliVersion").replace("{version}", cliVersionInfo.installed)}
    </button>
    {#if hasUpdate}
      <span class="text-primary/70">·</span>
      <button
        class="text-primary/70 hover:text-primary transition-colors"
        onclick={() => goto("/release-notes")}
        title={t("chat_cliUpdateAvailable").replace("{version}", channelLatest!)}
      >
        {t("chat_cliUpdateAvailable").replace("{version}", channelLatest!)}
      </button>
    {/if}
  {/if}
  {#if remoteHosts.length > 0}
    {#if cliVersionInfo?.installed}
      <span class="text-muted-foreground/30">·</span>
    {/if}
    <div class="relative inline-flex items-center">
      {#if targetDropdownOpen}
        <!-- Invisible backdrop to close dropdown on outside click -->
        <div class="fixed inset-0 z-40" onclick={() => (targetDropdownOpen = false)}></div>
      {/if}
      <button
        class="inline-flex items-center gap-1 cursor-pointer text-[11px] {store.remoteHostName
          ? 'text-blue-400/70 hover:text-blue-400'
          : 'text-muted-foreground/60 hover:text-muted-foreground'} transition-colors"
        onclick={() => (targetDropdownOpen = !targetDropdownOpen)}
      >
        <svg
          class="h-3 w-3 shrink-0"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <rect width="20" height="14" x="2" y="3" rx="2" /><line
            x1="8"
            y1="21"
            x2="16"
            y2="21"
          /><line x1="12" y1="17" x2="12" y2="21" />
        </svg>
        <span>{store.remoteHostName || t("chat_local")}</span>
        <svg
          class="h-2.5 w-2.5 opacity-60"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"><path d="m6 9 6 6 6-6" /></svg
        >
      </button>
      {#if targetDropdownOpen}
        <div
          class="absolute bottom-full left-1/2 -translate-x-1/2 mb-1.5 whitespace-nowrap rounded-md border border-border bg-popover py-1 shadow-md z-50"
        >
          <button
            class="flex w-full items-center gap-1.5 px-3 py-1.5 text-[11px] {!store.remoteHostName
              ? 'text-foreground font-medium'
              : 'text-foreground/70 hover:bg-accent'} transition-colors"
            onclick={() => {
              store.remoteHostName = null;
              try {
                localStorage.setItem("ocv:last-target", "");
              } catch {
                // localStorage may fail in restricted contexts
              }
              dbg("chat", "target changed", "local");
              targetDropdownOpen = false;
            }}
          >
            {t("chat_local")}
          </button>
          {#each remoteHosts as host (host.name)}
            <button
              class="flex w-full items-center gap-1.5 px-3 py-1.5 text-[11px] {store.remoteHostName ===
              host.name
                ? 'text-foreground font-medium'
                : 'text-foreground/70 hover:bg-accent'} transition-colors"
              onclick={() => {
                store.remoteHostName = host.name;
                try {
                  localStorage.setItem("ocv:last-target", host.name);
                } catch {
                  // localStorage may fail in restricted contexts
                }
                dbg("chat", "target changed", host.name);
                targetDropdownOpen = false;
              }}
            >
              {host.name} ({host.user}@{host.host})
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet heroMetaFooter()}
  <div class="mt-4 flex items-center justify-center gap-1.5 text-[11px] text-muted-foreground/60">
    {@render heroMetaItems()}
  </div>
{/snippet}

<div
  class="flex h-full overflow-hidden bg-background relative"
  ondragenter={handlePageDragEnter}
  ondragleave={handlePageDragLeave}
  ondragover={handlePageDragOver}
  ondrop={handlePageDrop}
>
  <!-- Page-level drag overlay -->
  {#if pageDragActive}
    <div
      class="absolute inset-0 z-50 flex items-center justify-center bg-background/60 backdrop-blur-[2px]"
    >
      <div
        class="flex flex-col items-center gap-2 rounded-xl border-2 border-dashed border-primary/50 bg-primary/5 px-12 py-8"
      >
        <svg
          class="h-8 w-8 text-primary/60"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="17 8 12 3 7 8" />
          <line x1="12" x2="12" y1="3" y2="15" />
        </svg>
        <span class="text-sm font-medium text-primary/70">{t("prompt_dropFiles")}</span>
      </div>
    </div>
  {/if}

  <!-- Main content area -->
  <div class="flex flex-1 flex-col min-w-0 relative">
    <!-- Status bar -->
    <SessionStatusBar
      bind:this={statusBarRef}
      running={store.sessionAlive}
      run={store.run}
      agent={store.run?.agent ?? store.agent}
      model={store.model}
      cost={store.usage.cost}
      inputTokens={cumulativeTokens.input}
      outputTokens={cumulativeTokens.output}
      cacheReadTokens={cumulativeTokens.cacheRead}
      cacheWriteTokens={cumulativeTokens.cacheWrite}
      parentRunId={store.run?.parent_run_id}
      onEndSession={handleStop}
      onFork={forkOverlay ? undefined : () => handleResume("fork")}
      onModelChange={handleModelChange}
      onNavigateParent={store.run?.parent_run_id
        ? () => goto(`/chat?run=${store.run!.parent_run_id}`)
        : undefined}
      cwd={store.effectiveCwd}
      onToggleSidebar={toggleLayoutSidebar}
      mcpServers={store.mcpServers}
      onMcpToggle={() => (mcpPanelOpen = !mcpPanelOpen)}
      cliVersion={store.cliVersion}
      permissionMode={store.permissionMode}
      {platformModels}
      fastModeState={store.fastModeState}
      verbose={verboseEnabled}
      numTurns={store.numTurns}
      durationMs={store.durationMs}
      persistedFiles={store.persistedFiles}
      onRewind={store.sessionAlive && !store.isRunning ? handleRewind : undefined}
      contextUtilization={store.contextUtilization}
      contextWarningLevel={store.contextWarningLevel}
      contextWindow={store.contextWindow}
      lastCompactedAt={store.lastCompactedAt}
      compactCount={store.compactCount}
      microcompactCount={store.microcompactCount}
      turnUsages={store.turnUsages}
      activeTaskCount={store.activeBackgroundTasks.length}
      mode={store.run ? (store.useStreamSession ? "Stream" : "CLI") : ""}
      toolsCount={sidebarCollapsed
        ? store.timeline.some((e) => e.kind === "tool")
          ? store.timeline.filter((e) => e.kind === "tool").length
          : store.tools.filter((e) => e.tool_name).length
        : 0}
      onToolsClick={sidebarCollapsed ? toggleSidebar : undefined}
      remoteHostName={store.remoteHostName}
      onRename={store.run ? handleRename : undefined}
      authSourceLabel={store.authSourceLabel}
      authSourceCategory={store.authSourceCategory}
      apiKeySource={store.apiKeySource}
      onStatusClick={() => {
        if (!hasSidebarData) return;
        if (sidebarCollapsed) sidebarCollapsed = false;
        sidebarRequestedTab = "info";
      }}
    />

    {#if store.hasBackgroundTasks}
      <BackgroundTaskPanel
        tasks={store.taskNotifications}
        activeTasks={store.activeBackgroundTasks}
        bind:collapsed={taskPanelCollapsed}
      />
    {/if}

    <!-- MCP panel (floating below status bar) -->
    {#if mcpPanelOpen && store.mcpServers.length > 0}
      <div class="absolute {statusBarExpanded ? 'top-16' : 'top-9'} right-3 z-30">
        <McpStatusPanel
          runId={store.run?.id ?? ""}
          mcpServers={store.mcpServers}
          sessionAlive={store.sessionAlive}
          onClose={() => (mcpPanelOpen = false)}
          onServersUpdate={(servers) => {
            store.mcpServers = servers;
          }}
        />
      </div>
    {/if}

    <!-- Main area -->
    <div class="flex-1 overflow-hidden relative">
      {#if store.useStreamSession}
        <!-- API mode: chat messages -->
        <div class="h-full overflow-y-auto" style="overflow-anchor:auto" bind:this={chatAreaRef}>
          {#if welcomeVisible}
            <!-- Welcome state -->
            <div class="flex h-full items-center justify-center">
              <div class="flex flex-col items-center max-w-sm">
                <div class="text-center animate-slide-up">
                  <img src="/logo.png?v=2" alt="OC" class="mx-auto mb-4 h-12 w-12 rounded-2xl" />
                  <h2 class="text-lg font-semibold text-primary mb-4">{t("layout_appName")}</h2>
                  {#if lastContinuableRun}
                    <button
                      class="w-full flex items-center justify-center gap-2 rounded-lg border border-border bg-muted/50 px-4 py-3 text-sm text-foreground hover:bg-accent hover:border-ring/30 transition-all duration-150"
                      onclick={() => goto(`/chat?run=${lastContinuableRun!.id}&resume=continue`)}
                    >
                      <svg
                        class="h-4 w-4 text-muted-foreground"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"><path d="m5 12 7-7 7 7" /><path d="M12 19V5" /></svg
                      >
                      {t("chat_continueLastSession")}
                    </button>
                    <p class="mt-2 text-[11px] text-muted-foreground/50">
                      {t("chat_orTypeToStart")}
                    </p>
                  {:else}
                    <p class="text-sm text-muted-foreground">{t("chat_typeToStart")}</p>
                  {/if}
                  {@render initHintCard()}
                </div>
                <!-- Footer outside animate-slide-up: AuthSourceBadge needs transform-free ancestor for fixed dropdown -->
                <div
                  class="mt-4 flex items-center justify-center gap-1.5 text-[11px] text-muted-foreground/60"
                >
                  <AuthSourceBadge
                    {authOverview}
                    authSourceLabel={store.authSourceLabel}
                    authSourceCategory={store.authSourceCategory}
                    apiKeySource={store.apiKeySource}
                    hasRun={false}
                    authMode={store.authMode}
                    platformCredentials={settings?.platform_credentials ?? []}
                    platformId={store.platformId ?? "anthropic"}
                    onAuthModeChange={handleAuthModeChange}
                    onPlatformChange={handlePlatformChange}
                    variant="hero"
                  />
                  <span class="text-muted-foreground/30">·</span>
                  {@render heroMetaItems()}
                </div>
              </div>
            </div>
          {:else}
            <!-- Timeline: chat messages + inline tool cards -->
            <div>
              {#if store.run?.parent_run_id}
                <div class="mx-auto max-w-5xl px-8 py-2">
                  <div
                    class="flex items-center gap-2 rounded-md border border-blue-500/20 bg-blue-500/5 px-3 py-2 text-xs text-blue-400"
                  >
                    <svg
                      class="h-3.5 w-3.5 shrink-0"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <circle cx="12" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><circle
                        cx="18"
                        cy="6"
                        r="3"
                      />
                      <path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9" /><path d="M12 12v3" />
                    </svg>
                    <span class="text-foreground/60">{t("chat_forkedBanner")}</span>
                    <button
                      class="ml-auto shrink-0 text-blue-400 hover:text-blue-300 underline underline-offset-2"
                      onclick={() => goto(`/chat?run=${store.run!.parent_run_id}`)}
                      >{t("chat_viewParent")}</button
                    >
                  </div>
                </div>
              {/if}
              {#if notificationVisible && latestNotification}
                <div class="mx-auto max-w-5xl px-8 py-1">
                  <div
                    class="flex items-center gap-2 text-xs text-muted-foreground bg-teal-500/5 border border-teal-500/20 rounded px-3 py-1.5 animate-fade-in"
                  >
                    <span class="h-1.5 w-1.5 rounded-full bg-teal-500 animate-pulse"></span>
                    Task #{latestNotification.task_id}: {latestNotification.status}
                  </div>
                </div>
              {/if}
              {#if toolNamesInTimeline.length >= 2}
                <div class="mx-auto max-w-5xl px-8 py-2">
                  <div class="flex flex-wrap items-center gap-1.5">
                    <button
                      class="rounded-full px-2.5 py-0.5 text-[11px] font-medium transition-colors {!toolFilter
                        ? 'bg-foreground/10 text-foreground'
                        : 'text-muted-foreground hover:text-foreground hover:bg-muted'}"
                      onclick={() => (toolFilter = null)}>{t("chat_filterAll")}</button
                    >
                    {#each toolNamesInTimeline as name}
                      {@const style = getToolColor(name)}
                      <button
                        class="flex items-center gap-1 rounded-full px-2.5 py-0.5 text-[11px] font-medium transition-colors {toolFilter ===
                        name
                          ? style.bg + ' ' + style.text
                          : 'text-muted-foreground hover:text-foreground hover:bg-muted'}"
                        onclick={() => (toolFilter = toolFilter === name ? null : name)}
                      >
                        <svg
                          class="h-2.5 w-2.5"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path d={style.icon} />
                        </svg>
                        {name}
                      </button>
                    {/each}
                  </div>
                </div>
              {/if}
              {#each visibleTimeline as entry, i (entry.id)}
                <div
                  style={isWebKit
                    ? ""
                    : "content-visibility:auto;contain-intrinsic-size:auto 100px"}
                >
                  {#if usageAnnotations.has(i)}
                    {@const tu = usageAnnotations.get(i)}
                    {#if tu}
                      <div class="w-full py-1.5">
                        <div class="mx-auto max-w-5xl px-8">
                          <div class="flex items-center gap-3">
                            <div class="h-px flex-1 bg-border/40"></div>
                            <span class="text-[10px] tabular-nums text-muted-foreground/50">
                              {formatTokens(tu.inputTokens)}
                              {t("chat_usageIn")} · {formatTokens(tu.outputTokens)}
                              {t("chat_usageOut")}
                              {#if tu.cacheReadTokens > 0 || tu.cacheWriteTokens > 0}
                                · {t("chat_usageCache", {
                                  read: formatTokens(tu.cacheReadTokens),
                                  write: formatTokens(tu.cacheWriteTokens),
                                })}
                              {/if}
                            </span>
                            <div class="h-px flex-1 bg-border/40"></div>
                          </div>
                        </div>
                      </div>
                    {/if}
                  {/if}
                  {#if entry.kind === "user"}
                    <ChatMessage
                      message={{
                        id: entry.id,
                        role: "user",
                        content: entry.content,
                        timestamp: entry.ts,
                      }}
                      attachments={entry.attachments}
                    />
                  {:else if entry.kind === "assistant"}
                    <ChatMessage
                      message={{
                        id: entry.id,
                        role: "assistant",
                        content: entry.content,
                        timestamp: entry.ts,
                      }}
                    />
                  {:else if entry.kind === "tool"}
                    <div class="w-full py-1" id="tool-{entry.tool.tool_use_id}">
                      <div class="mx-auto max-w-5xl px-8 pl-11">
                        <InlineToolCard
                          tool={entry.tool}
                          subTimeline={entry.subTimeline}
                          runId={store.run?.id ?? ""}
                          {fetchToolResult}
                          onAnswer={entry.tool.tool_name === "AskUserQuestion" &&
                          (entry.tool.status === "running" || entry.tool.status === "ask_pending")
                            ? (answer) => handleToolAnswer(entry.tool.tool_use_id, answer)
                            : undefined}
                          onApprove={handleToolApprove}
                          onPermissionRespond={handlePermissionRespond}
                          onExitPlanClearContext={handleExitPlanClearContext}
                          taskNotifications={store.taskNotifications}
                        />
                      </div>
                    </div>
                    <!-- ExitPlanMode: render approved plan as standalone markdown in chat stream -->
                    {#if entry.tool.tool_name === "ExitPlanMode" && entry.tool.status === "success" && entry.tool.tool_use_result?.plan}
                      <div class="w-full py-2">
                        <div class="mx-auto max-w-5xl px-8 pl-11">
                          <div
                            class="rounded-lg border border-indigo-500/20 bg-indigo-500/5 px-5 py-4 prose-chat"
                          >
                            <MarkdownContent text={String(entry.tool.tool_use_result.plan)} />
                          </div>
                        </div>
                      </div>
                    {/if}
                  {:else if entry.kind === "command_output"}
                    <div class="w-full py-2">
                      <div class="mx-auto max-w-5xl px-8 pl-11">
                        <div
                          class="command-output rounded-lg border border-border/40 bg-[#1a1b26] px-4 py-3 text-sm overflow-x-auto"
                        >
                          {#if entry.content.includes("## Context Usage")}
                            <ContextUsageGrid text={entry.content} />
                          {:else if entry.content.includes("Total cost:") && entry.content.includes("Total duration")}
                            <CostSummaryView text={entry.content} />
                          {:else if entry.content
                            .trimStart()
                            .startsWith("Version ") && entry.content.includes("•")}
                            <ReleaseNotesCard text={entry.content} />
                          {:else if hasAnsiCodes(entry.content)}
                            <pre
                              class="whitespace-pre font-mono text-xs leading-relaxed text-[#c0caf5] m-0">{@html ansiToHtml(
                                entry.content,
                              )}</pre>
                          {:else}
                            <MarkdownContent text={entry.content} />
                          {/if}
                        </div>
                      </div>
                    </div>
                  {:else if entry.kind === "separator"}
                    <div class="w-full py-3">
                      <div class="mx-auto max-w-5xl px-8">
                        <div class="flex items-center gap-3">
                          <div class="h-px flex-1 bg-amber-500/20"></div>
                          <span class="text-xs text-amber-500/70 font-medium whitespace-nowrap">
                            {entry.content}
                          </span>
                          <div class="h-px flex-1 bg-amber-500/20"></div>
                        </div>
                      </div>
                    </div>
                  {/if}
                </div>
              {/each}

              <!-- Last turn usage annotation (after all entries) -->
              {#if lastTurnUsage && !store.isRunning}
                <div class="w-full py-1.5">
                  <div class="mx-auto max-w-5xl px-8">
                    <div class="flex items-center gap-3">
                      <div class="h-px flex-1 bg-border/40"></div>
                      <span class="text-[10px] tabular-nums text-muted-foreground/50">
                        {formatTokens(lastTurnUsage.inputTokens)}
                        {t("chat_usageIn")} · {formatTokens(lastTurnUsage.outputTokens)}
                        {t("chat_usageOut")}
                        {#if lastTurnUsage.cacheReadTokens > 0 || lastTurnUsage.cacheWriteTokens > 0}
                          · {t("chat_usageCache", {
                            read: formatTokens(lastTurnUsage.cacheReadTokens),
                            write: formatTokens(lastTurnUsage.cacheWriteTokens),
                          })}
                        {/if}
                      </span>
                      <div class="h-px flex-1 bg-border/40"></div>
                    </div>
                  </div>
                </div>
              {/if}

              <!-- Pending hook callbacks -->
              {#each store.hookEvents.filter((h) => h.status === "hook_pending") as hookEvent (hookEvent.request_id)}
                <div class="mx-auto max-w-5xl px-8 pl-11">
                  <HookReviewCard {hookEvent} onRespond={handleHookCallbackRespond} />
                </div>
              {/each}

              <!-- Thinking panel (extended thinking) -->
              {#if store.thinkingText}
                <div class="w-full animate-fade-in">
                  <div class="mx-auto max-w-5xl px-8 py-2">
                    <button
                      class="w-full text-left rounded-lg border border-violet-500/20 bg-violet-500/5 px-3 py-2 transition-colors group"
                      onclick={() => (thinkingExpanded = !thinkingExpanded)}
                    >
                      <div class="flex items-center gap-2">
                        <div
                          class="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-violet-500/10"
                        >
                          <svg
                            class="h-3 w-3 text-violet-400"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                          >
                            <path
                              d="M12 2a8 8 0 0 0-8 8c0 3.4 2.1 6.3 5 7.4V19a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1v-1.6c2.9-1.1 5-4 5-7.4a8 8 0 0 0-8-8z"
                            />
                            <path d="M10 22h4" />
                          </svg>
                        </div>
                        <span class="text-xs font-medium text-violet-400">{t("chat_thinking")}</span
                        >
                        {#if store.isRunning && !store.streamingText}
                          <div
                            class="h-2.5 w-2.5 rounded-full border-2 border-violet-500/30 border-t-violet-400 animate-spin"
                          ></div>
                        {/if}
                        <svg
                          class="h-3 w-3 text-muted-foreground/40 shrink-0 transition-transform ml-auto {thinkingExpanded
                            ? 'rotate-180'
                            : ''}"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path d="m6 9 6 6 6-6" />
                        </svg>
                      </div>
                      {#if thinkingExpanded}
                        <div class="mt-2 pl-7 max-h-60 overflow-y-auto">
                          <pre
                            class="text-xs font-mono whitespace-pre-wrap break-words text-violet-300/70 leading-relaxed">{store.thinkingText}</pre>
                        </div>
                      {/if}
                    </button>
                  </div>
                </div>
              {/if}

              <!-- Streaming text -->
              {#if store.streamingText}
                <div class="w-full animate-fade-in">
                  <div class="mx-auto max-w-5xl px-8 py-4">
                    <div class="mb-1.5 flex items-center gap-2">
                      <div
                        class="flex h-5 w-5 items-center justify-center rounded-sm bg-orange-500/10 text-orange-500"
                      >
                        <svg
                          class="h-3 w-3"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path
                            d="M12 3l1.912 5.813a2 2 0 0 0 1.275 1.275L21 12l-5.813 1.912a2 2 0 0 0-1.275 1.275L12 21l-1.912-5.813a2 2 0 0 0-1.275-1.275L3 12l5.813-1.912a2 2 0 0 0 1.275-1.275L12 3z"
                          />
                        </svg>
                      </div>
                      <span class="text-sm font-semibold text-foreground">{t("chat_claude")}</span>
                    </div>
                    <div class="pl-7 prose-chat">
                      <MarkdownContent text={store.streamingText} streaming={true} />
                    </div>
                  </div>
                </div>
              {/if}

              <!-- Slash command processing indicator (before thinking kicks in) -->
              {#if processingSlashCmd && !thinkingVisible && !store.streamingText && !store.thinkingText}
                <div class="w-full animate-fade-in">
                  <div class="mx-auto max-w-5xl px-8 py-2">
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                      <div
                        class="h-3.5 w-3.5 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                      ></div>
                      <span>{t("chat_processingCommand", { command: processingSlashCmd })}</span>
                    </div>
                  </div>
                </div>
              {/if}

              <!-- Thinking indicator (debounced 300ms to avoid flash on fast CLI commands) -->
              {#if thinkingVisible && !store.thinkingText}
                <div class="w-full animate-fade-in">
                  <div class="mx-auto max-w-5xl px-8 py-4">
                    <div class="mb-1.5 flex items-center gap-2">
                      <div
                        class="flex h-5 w-5 items-center justify-center rounded-sm bg-orange-500/10 text-orange-500"
                      >
                        <svg
                          class="h-3 w-3"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path
                            d="M12 3l1.912 5.813a2 2 0 0 0 1.275 1.275L21 12l-5.813 1.912a2 2 0 0 0-1.275 1.275L12 21l-1.912-5.813a2 2 0 0 0-1.275-1.275L3 12l5.813-1.912a2 2 0 0 0 1.275-1.275L12 3z"
                          />
                        </svg>
                      </div>
                      <span class="text-sm font-semibold text-foreground">{t("chat_claude")}</span>
                      {#if thinkingElapsed > 0}
                        <span class="ml-auto text-[10px] tabular-nums text-muted-foreground"
                          >{formatElapsed(thinkingElapsed)}</span
                        >
                      {/if}
                    </div>
                    <div class="pl-7">
                      {#if store.activeToolName}
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                          <div
                            class="h-3.5 w-3.5 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                          ></div>
                          <span
                            >{t("chat_usingTool")}
                            <span class="text-foreground font-medium">{store.activeToolName}</span
                            ></span
                          >
                          {#if store.thinkingEndMs && store.thinkingDurationSec > 0}
                            <span class="text-xs tabular-nums"
                              >· thought for {store.thinkingDurationSec}s</span
                            >
                          {/if}
                        </div>
                      {:else if approving}
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                          <div
                            class="h-3.5 w-3.5 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                          ></div>
                          <span>{t("chat_restartingApproved")}</span>
                        </div>
                      {:else if sending}
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                          <div
                            class="h-3.5 w-3.5 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                          ></div>
                          <span>{t("chat_startingSession")}</span>
                        </div>
                      {:else}
                        <div class="flex items-center gap-2 text-sm">
                          <span class="spinner-star">✦</span>
                          <span class="spinner-shimmer">{spinnerVerb}…</span>
                          {#if store.thinkingEndMs && store.thinkingDurationSec > 0}
                            <span class="text-muted-foreground text-xs tabular-nums"
                              >· thought for {store.thinkingDurationSec}s</span
                            >
                          {/if}
                        </div>
                      {/if}
                    </div>
                  </div>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {:else if store.ptySpawned || pendingMessage || (store.run && store.run.status !== "pending")}
        <!-- CLI mode: terminal -->
        <XTerminal
          bind:this={xtermRef}
          onResize={handleTermResize}
          onReady={handleTermReady}
          onData={handleTermData}
          class="h-full"
        />
      {:else}
        <!-- CLI mode: welcome state -->
        <div class="flex h-full items-center justify-center">
          <div class="text-center max-w-md animate-slide-up">
            <img src="/logo.png?v=2" alt="OC" class="mx-auto mb-4 h-12 w-12 rounded-2xl" />
            <h2 class="text-lg font-semibold text-primary mb-2">{t("layout_appName")}</h2>
            <p class="text-sm text-muted-foreground mb-4">
              {store.run ? t("chat_typeToStartSession") : t("chat_startSessionHint")}
            </p>
            {#if !store.run}
              <div class="flex flex-col gap-2">
                {#each examplePrompts as prompt}
                  <button
                    class="rounded-lg border border-white/10 bg-white/5 px-4 py-3 text-left text-sm text-neutral-300 hover:bg-white/10 hover:border-white/20 transition-all duration-150 group"
                    onclick={() => fillPrompt(prompt())}
                  >
                    <span
                      class="text-muted-foreground/50 mr-2 group-hover:text-muted-foreground transition-colors"
                      >&rarr;</span
                    >
                    {prompt()}
                  </button>
                {/each}
              </div>
            {/if}
            {@render initHintCard()}
            {@render heroMetaFooter()}
          </div>
        </div>
      {/if}

      <!-- Fork overlay -->
      {#if forkOverlay}
        <div
          class="absolute inset-0 z-20 flex items-center justify-center bg-background/80 backdrop-blur-sm animate-fade-in"
        >
          {#if forkOverlay.error}
            <!-- Error state -->
            <div class="flex flex-col items-center gap-4 max-w-sm text-center animate-slide-up">
              <div
                class="flex h-12 w-12 items-center justify-center rounded-full bg-destructive/10"
              >
                <svg
                  class="h-6 w-6 text-destructive"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <circle cx="12" cy="12" r="10" /><path d="m15 9-6 6" /><path d="m9 9 6 6" />
                </svg>
              </div>
              <div>
                <h3 class="text-sm font-semibold text-foreground mb-1">{t("chat_forkFailed")}</h3>
                <p class="text-xs text-muted-foreground">{forkOverlay.error}</p>
              </div>
              <div class="flex items-center gap-2">
                <button
                  class="rounded-lg border border-border bg-muted px-4 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                  onclick={handleForkCancel}>{t("common_cancel")}</button
                >
                <button
                  class="rounded-lg bg-primary px-4 py-2 text-sm text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                  disabled={resuming}
                  onclick={handleForkRetry}>{t("common_retry")}</button
                >
              </div>
            </div>
          {:else}
            <!-- In-progress state -->
            <div class="flex flex-col items-center gap-4 max-w-sm text-center animate-slide-up">
              <div class="flex h-12 w-12 items-center justify-center rounded-full bg-blue-500/10">
                <svg
                  class="h-6 w-6 text-blue-400 animate-spin"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M21 12a9 9 0 1 1-6.219-8.56" />
                </svg>
              </div>
              <div>
                <h3 class="text-sm font-semibold text-foreground mb-1">
                  {t("chat_forkingSession")}
                </h3>
                <p class="text-xs text-muted-foreground">
                  {t("chat_forkingDesc")}
                </p>
              </div>
              {#if forkElapsed > 0}
                <span class="text-xs tabular-nums text-muted-foreground"
                  >{formatElapsed(forkElapsed)}</span
                >
              {/if}
              <button
                class="rounded-lg border border-border bg-muted px-4 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                onclick={handleForkCancel}>{t("common_cancel")}</button
              >
            </div>
          {/if}
        </div>
      {/if}

      <!-- Classified error card -->
      {#if store.error && !forkOverlay}
        {@const classified = classifyError(store.run?.result_subtype, store.error)}
        {@const catIcon =
          classified.category === "context_limit"
            ? "⚠"
            : classified.category === "auth_issue"
              ? "🔑"
              : classified.category === "budget_limit"
                ? "💰"
                : classified.category === "server_issue"
                  ? "☁"
                  : classified.category === "tool_issue"
                    ? "🔧"
                    : "❌"}
        <div class="absolute bottom-14 left-3 right-3 z-10">
          <div
            class="rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm backdrop-blur-sm animate-fade-in"
          >
            <div class="flex items-start gap-2">
              <span class="shrink-0 text-base leading-none mt-0.5">{catIcon}</span>
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2 mb-1">
                  <span class="text-[10px] font-medium uppercase tracking-wider text-destructive/70"
                    >{t(`error_category_${classified.category}`)}</span
                  >
                </div>
                <p class="text-destructive text-xs leading-relaxed break-words">{store.error}</p>
                <p class="text-destructive/60 text-[10px] mt-1">
                  {t(`error_guidance_${classified.category}`)}
                </p>
              </div>
              <button
                class="shrink-0 text-destructive/50 hover:text-destructive text-xs"
                onclick={() => (store.error = "")}>{t("common_dismiss")}</button
              >
            </div>
            <div class="flex items-center gap-2 mt-2 pl-6">
              {#if classified.canRetry && store.phase === "failed" && store.run?.session_id}
                <button
                  class="rounded px-2.5 py-1 text-xs bg-destructive/20 hover:bg-destructive/30 text-destructive transition-colors"
                  onclick={() => handleResume("continue")}>{t("common_retry")}</button
                >
              {/if}
              {#if classified.canFork && store.run?.session_id}
                <button
                  class="rounded px-2.5 py-1 text-xs bg-blue-500/20 hover:bg-blue-500/30 text-blue-400 transition-colors"
                  onclick={() => handleResume("fork")}>{t("statusbar_fork")}</button
                >
              {/if}
              {#if classified.settingsLink}
                <button
                  class="rounded px-2.5 py-1 text-xs bg-destructive/20 hover:bg-destructive/30 text-destructive transition-colors"
                  onclick={() => goto(classified.settingsLink)}>{t("chat_checkSettings")}</button
                >
              {/if}
            </div>
          </div>
        </div>
      {/if}
    </div>

    <!-- Resume warning (if applicable) -->
    {#if canResumeRun(store.run, store.phase, agentSettings?.no_session_persistence ?? false) && getResumeWarning(store.run)}
      <div
        class="mx-3 mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-2 text-xs text-amber-400"
      >
        {getResumeWarning(store.run)}
      </div>
    {/if}

    <!-- Input bar -->
    {#if !store.ptySpawned || store.sessionAlive}
      <PromptInput
        bind:this={promptRef}
        agent={store.agent}
        running={store.isRunning}
        hasRun={!!store.run}
        sessionAlive={store.sessionAlive}
        canResume={!store.sessionAlive && !!store.run?.session_id && store.useStreamSession}
        useStreamSession={store.useStreamSession}
        isRemote={store.isRemote}
        cliCommands={store.sessionInitReceived && store.sessionCommands.length > 0
          ? store.sessionCommands
          : getCliCommands()}
        models={effectiveModels}
        currentModel={store.model}
        permissionMode={store.permissionMode}
        cwd={store.effectiveCwd || localStorage.getItem("ocv:project-cwd") || ""}
        authMode={store.authMode}
        platformId={store.platformId ?? "anthropic"}
        platformCredentials={settings?.platform_credentials ?? []}
        onSend={sendMessage}
        onAgentChange={(a) => (store.agent = a)}
        onInterrupt={() => store.interrupt()}
        onModelSwitch={handleModelChange}
        onPermissionModeChange={store.agent === "claude" ? handlePermissionModeChange : undefined}
        onVirtualCommand={handleVirtualCommand}
        onPlatformChange={handlePlatformChange}
        {authOverview}
        authSourceLabel={store.authSourceLabel}
        authSourceCategory={store.authSourceCategory}
        apiKeySource={store.apiKeySource}
        onAuthModeChange={handleAuthModeChange}
        showAuthBadge={!welcomeVisible}
        onShortcutHelp={() => (shortcutHelpOpen = !shortcutHelpOpen)}
        availableSkills={store.availableSkills}
        {skillItems}
        hasStash={!!stashedInput}
        {userHistory}
        runId={store.run?.id ?? ""}
        onRestoreStash={() => {
          if (stashedInput) {
            promptRef?.restoreSnapshot(stashedInput);
            stashedInput = null;
            showChatToast(t("toast_stashRestored"));
            dbg("chat", "stash restored via badge click");
          }
        }}
      />
    {/if}
  </div>

  <!-- Tool Activity sidebar -->
  {#if hasSidebarData}
    <ToolActivity
      timeline={store.timeline}
      tools={store.tools}
      turnUsages={store.turnUsages}
      {contextHistory}
      persistedFiles={store.persistedFiles}
      sessionInfo={currentSessionInfo}
      collapsed={sidebarCollapsed}
      onToggle={toggleSidebar}
      onScrollToTool={scrollToTool}
      bind:requestedTab={sidebarRequestedTab}
    />
  {/if}

  <!-- CLI session browser modal -->
  {#if showCliBrowser}
    <CliSessionBrowser
      cwd={localStorage.getItem("ocv:project-cwd") || "/"}
      onclose={() => (showCliBrowser = false)}
      onimported={(runId) => {
        showCliBrowser = false;
        goto(`/chat?run=${runId}`);
      }}
    />
  {/if}

  <ShortcutHelpPanel bind:open={shortcutHelpOpen} />

  <!-- Chat toast (fixed bottom-center, auto-dismiss) -->
  {#if chatToast}
    <div
      class="fixed bottom-20 left-1/2 -translate-x-1/2 z-50
      rounded-lg border bg-background/95 px-4 py-2 text-sm shadow-lg backdrop-blur-sm
      animate-in fade-in slide-in-from-bottom-2 duration-200"
    >
      {chatToast}
    </div>
  {/if}
</div>

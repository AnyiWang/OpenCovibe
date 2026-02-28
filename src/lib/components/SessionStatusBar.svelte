<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import type { TaskRun, McpServerInfo, CliModelInfo } from "$lib/types";
  import type { TurnUsage } from "$lib/stores/types";
  import { dbg } from "$lib/utils/debug";
  import { getCliModels } from "$lib/stores/cli-info.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { fmtNumber } from "$lib/i18n/format";

  let {
    run = null,
    agent = "claude",
    model = "",
    cost = 0,
    inputTokens = 0,
    outputTokens = 0,
    cacheReadTokens = 0,
    cacheWriteTokens = 0,
    running = false,
    parentRunId,
    onEndSession,
    onFork,
    onModelChange,
    onNavigateParent,
    onToggleSidebar,
    mcpServers,
    onMcpToggle,
    cliVersion,
    permissionMode,
    fastModeState,
    numTurns,
    durationMs,
    persistedFiles,
    onRewind,
    contextUtilization,
    contextWarningLevel,
    contextWindow,
    cwd = "",
    lastCompactedAt = 0,
    compactCount = 0,
    microcompactCount = 0,
    turnUsages = [],
    activeTaskCount = 0,
    mode = "",
    toolsCount = 0,
    onToolsClick,
    remoteHostName,
    onRename,
    platformModels = [],
    authSourceLabel,
    authSourceCategory,
    verbose = false,
    apiKeySource,
    onStatusClick,
  }: {
    run?: TaskRun | null;
    agent?: string;
    model?: string;
    cost?: number;
    inputTokens?: number;
    outputTokens?: number;
    cacheReadTokens?: number;
    cacheWriteTokens?: number;
    running?: boolean;
    parentRunId?: string;
    onEndSession?: () => void;
    onFork?: () => void;
    onModelChange?: (model: string) => void;
    onNavigateParent?: () => void;
    onToggleSidebar?: () => void;
    mcpServers?: McpServerInfo[];
    onMcpToggle?: () => void;
    cliVersion?: string;
    permissionMode?: string;
    fastModeState?: string;
    numTurns?: number;
    durationMs?: number;
    persistedFiles?: unknown[];
    onRewind?: () => void;
    contextUtilization?: number;
    contextWarningLevel?: string;
    cwd?: string;
    contextWindow?: number;
    lastCompactedAt?: number;
    compactCount?: number;
    microcompactCount?: number;
    turnUsages?: TurnUsage[];
    activeTaskCount?: number;
    mode?: string;
    toolsCount?: number;
    onToolsClick?: () => void;
    remoteHostName?: string | null;
    onRename?: (name: string) => void;
    platformModels?: CliModelInfo[];
    authSourceLabel?: string;
    authSourceCategory?: string;
    verbose?: boolean;
    apiKeySource?: string;
    onStatusClick?: () => void;
  } = $props();

  $effect(() => {
    dbg("status", "state", { agent, model, running, runId: run?.id });
  });

  // ── Compact indicator (fades after 8s) ──
  let compactVisible = $state(false);
  let compactTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (lastCompactedAt && lastCompactedAt > 0) {
      compactVisible = true;
      clearTimeout(compactTimer);
      compactTimer = setTimeout(() => {
        compactVisible = false;
      }, 8000);
    }
  });

  // ── Expansion state (persisted) ──
  let expanded = $state(
    typeof window !== "undefined"
      ? localStorage.getItem("ocv:statusbar-expanded") !== "false"
      : true,
  );

  $effect(() => {
    localStorage.setItem("ocv:statusbar-expanded", String(expanded));
    if (typeof window !== "undefined") {
      window.dispatchEvent(new CustomEvent("ocv:statusbar-toggle", { detail: { expanded } }));
    }
  });

  let cwdShort = $derived.by(() => {
    const val = cwd || run?.cwd || "";
    if (!val || val === "/") return "";
    const home = val
      .replace(/^\/Users\/[^/]+/, "~")
      .replace(/^\/home\/[^/]+/, "~")
      .replace(/^[A-Za-z]:[/\\](?:Users|users)[/\\][^/\\]+/, "~");
    return home.length > 30 ? "..." + home.slice(-27) : home;
  });

  let sessionIdShort = $derived(run?.session_id ? run.session_id.slice(0, 8) : "");
  let sidCopied = $state(false);

  async function copySessionId() {
    if (!run?.session_id) return;
    try {
      await navigator.clipboard.writeText(run.session_id);
      sidCopied = true;
      setTimeout(() => (sidCopied = false), 1500);
    } catch {
      /* ignore */
    }
  }

  // ── Title inline editing ──
  let titleEditing = $state(false);
  let titleEditValue = $state("");
  let titleInputEl: HTMLInputElement | undefined = $state();

  function truncate(s: string, max: number): string {
    return s.length > max ? s.slice(0, max) + "…" : s;
  }

  function startTitleEdit() {
    if (!onRename || !run) return;
    titleEditValue = run.name || run.prompt;
    titleEditing = true;
    requestAnimationFrame(() => titleInputEl?.select());
  }

  function commitTitleEdit() {
    titleEditing = false;
    const trimmed = titleEditValue.trim();
    if (trimmed && run && trimmed !== (run.name || run.prompt)) {
      onRename?.(trimmed);
    }
  }

  function cancelTitleEdit() {
    titleEditing = false;
  }

  function formatCost(c: number): string {
    if (c === 0) return "$0.00";
    if (c < 0.01) return "<$0.01";
    return "$" + c.toFixed(2);
  }

  function formatTokens(t: number): string {
    if (t === 0) return "0";
    if (t >= 1000) return (t / 1000).toFixed(1) + "k";
    return t.toString();
  }

  function formatDuration(ms: number): string {
    if (ms <= 0) return "";
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  let permissionBadge = $derived.by(() => {
    if (!permissionMode || permissionMode === "default") return null;
    const map: Record<string, { label: string; cls: string }> = {
      bypassPermissions: { label: "auto-all", cls: "bg-amber-500/15 text-amber-500" },
      acceptEdits: { label: "auto-read", cls: "bg-blue-500/15 text-blue-400" },
      plan: { label: "plan", cls: "bg-purple-500/15 text-purple-400" },
      dontAsk: { label: "no-ask", cls: "bg-red-500/15 text-red-400" },
      delegate: { label: "delegate", cls: "bg-teal-500/15 text-teal-400" },
    };
    return (
      map[permissionMode] ?? { label: permissionMode, cls: "bg-foreground/10 text-foreground/60" }
    );
  });

  // ── Model selector dropdown ──
  // Use platform-specific models when a third-party provider is active
  let models = $derived(platformModels.length > 0 ? platformModels : getCliModels());
  let dropdownOpen = $state(false);
  let focusedModelIdx = $state(-1);
  let modelBtnEl: HTMLButtonElement | undefined = $state();
  let dropdownEl: HTMLDivElement | undefined = $state();
  let dropdownStyle = $state("");

  function toggleModelDropdown() {
    dropdownOpen = !dropdownOpen;
    if (dropdownOpen && modelBtnEl) {
      const rect = modelBtnEl.getBoundingClientRect();
      const spaceBelow = window.innerHeight - rect.bottom;
      if (spaceBelow < 200) {
        dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
      } else {
        dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
      }
      focusedModelIdx = models.findIndex((m) => m.value === model);
      if (focusedModelIdx < 0) focusedModelIdx = 0;
      requestAnimationFrame(() => dropdownEl?.focus());
    }
  }

  export function openModelDropdown() {
    dropdownOpen = true;
    if (modelBtnEl) {
      const rect = modelBtnEl.getBoundingClientRect();
      const spaceBelow = window.innerHeight - rect.bottom;
      if (spaceBelow < 200) {
        dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
      } else {
        dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
      }
    }
    focusedModelIdx = models.findIndex((m) => m.value === model);
    if (focusedModelIdx < 0) focusedModelIdx = 0;
    requestAnimationFrame(() => dropdownEl?.focus());
  }

  function selectModel(val: string) {
    dropdownOpen = false;
    onModelChange?.(val);
  }

  function handleDropdownKeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      focusedModelIdx = Math.min(focusedModelIdx + 1, models.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      focusedModelIdx = Math.max(focusedModelIdx - 1, 0);
    } else if (e.key === "Enter" && focusedModelIdx >= 0 && focusedModelIdx < models.length) {
      e.preventDefault();
      dbg("statusbar", "model selected via keyboard", { model: models[focusedModelIdx].value });
      selectModel(models[focusedModelIdx].value);
    } else if (e.key === "Escape") {
      e.preventDefault();
      dropdownOpen = false;
    }
    // Tab: allow focus to leave dropdown; all other keys: stop propagation to prevent global shortcuts
    if (e.key !== "Tab") {
      e.stopPropagation();
    }
  }

  onMount(() => {
    function onDocClick(e: MouseEvent) {
      if (
        dropdownOpen &&
        modelBtnEl &&
        !modelBtnEl.contains(e.target as Node) &&
        dropdownEl &&
        !dropdownEl.contains(e.target as Node)
      ) {
        dropdownOpen = false;
      }
    }
    function onDocKeydown(e: KeyboardEvent) {
      if (dropdownOpen && e.key === "Escape") {
        dropdownOpen = false;
        e.preventDefault();
        e.stopPropagation(); // Prevent bubble to window → keybindingStore.dispatch → chat:interrupt
      }
    }
    document.addEventListener("mousedown", onDocClick, true);
    document.addEventListener("keydown", onDocKeydown);
    return () => {
      document.removeEventListener("mousedown", onDocClick, true);
      document.removeEventListener("keydown", onDocKeydown);
    };
  });

  // ── End Session confirmation ──
  let confirmingEnd = $state(false);
  let confirmTimer: ReturnType<typeof setTimeout> | undefined;

  function requestEnd() {
    confirmingEnd = true;
    confirmTimer = setTimeout(() => {
      confirmingEnd = false;
    }, 3000);
  }

  function confirmEnd() {
    clearTimeout(confirmTimer);
    confirmingEnd = false;
    onEndSession?.();
  }

  function cancelEnd() {
    clearTimeout(confirmTimer);
    confirmingEnd = false;
  }

  let mcpAggregateStatus = $derived.by(() => {
    if (!mcpServers || mcpServers.length === 0) return "none";
    const hasFailure = mcpServers.some((s) => s.status === "failed" || s.status === "needs-auth");
    const hasPending = mcpServers.some((s) => s.status === "pending");
    const allDisabled = mcpServers.every((s) => s.status === "disabled");
    if (hasFailure) return "error";
    if (hasPending) return "pending";
    if (allDisabled) return "disabled";
    return "ok";
  });

  let mcpDotClass = $derived(
    mcpAggregateStatus === "error"
      ? "bg-destructive"
      : mcpAggregateStatus === "pending"
        ? "bg-amber-500"
        : mcpAggregateStatus === "disabled"
          ? "bg-muted-foreground/30"
          : "bg-emerald-500",
  );

  let modelLabel = $derived.by(() => {
    // Check platform models first, then CLI models
    const all = [...(platformModels ?? []), ...getCliModels()];
    const found = all.find((m) => m.value === model);
    if (found) return found.displayName;
    const fuzzy = all.find((m) => model.includes(m.value) && m.value !== "default");
    if (fuzzy) return fuzzy.displayName;
    return model;
  });
</script>

<div class="border-b border-border bg-muted/50 font-mono text-xs text-foreground/70">
  <!-- Tier 1: Always visible (h-9) -->
  <div class="flex h-9 items-center justify-between px-3">
    <!-- Left: core info -->
    <div class="flex items-center gap-1.5 min-w-0 flex-1">
      {#if onToggleSidebar}
        <button
          class="rounded p-1 -ml-1 mr-0.5 hover:bg-accent transition-colors"
          onclick={onToggleSidebar}
          title={t("statusbar_toggleSidebar")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><rect width="18" height="18" x="3" y="3" rx="2" /><path d="M9 3v18" /></svg
          >
        </button>
      {/if}

      <!-- Pulse indicator + agent name (clickable for status) -->
      {#if onStatusClick}
        <button
          class="inline-flex items-center gap-1.5 rounded px-1 -mx-1 hover:bg-accent/50 transition-colors"
          onclick={onStatusClick}
          title={t("toolActivity_tabInfo")}
        >
          <span
            class="inline-block h-2 w-2 rounded-full {running
              ? 'bg-green-500 animate-pulse'
              : 'bg-foreground/20'}"
          ></span>
          <span class="text-foreground font-medium">{agent}</span>
        </button>
      {:else}
        <span
          class="inline-block h-2 w-2 rounded-full {running
            ? 'bg-green-500 animate-pulse'
            : 'bg-foreground/20'}"
        ></span>
      {/if}

      <!-- Session title (inline editable) -->
      {#if run && onRename}
        {#if titleEditing}
          <input
            bind:this={titleInputEl}
            bind:value={titleEditValue}
            class="w-32 bg-transparent border-b border-primary outline-none text-foreground font-medium px-0.5"
            onkeydown={(e) => {
              if (e.key === "Enter") commitTitleEdit();
              else if (e.key === "Escape") cancelTitleEdit();
            }}
            onblur={commitTitleEdit}
          />
        {:else}
          <button
            class="max-w-[200px] truncate text-foreground/80 hover:text-foreground transition-colors {run.name
              ? 'font-medium'
              : 'italic text-foreground/40'}"
            onclick={startTitleEdit}
            title={run.name || run.prompt || t("statusbar_sessionTitle")}
          >
            {truncate(run.name || run.prompt, 30)}
          </button>
        {/if}
        <span class="text-foreground/30">&middot;</span>
      {/if}

      {#if !onStatusClick}
        <span class="text-foreground font-medium">{agent}</span>
      {/if}

      {#if mode}
        <span
          class="text-[10px] font-medium uppercase tracking-wider px-1.5 py-0.5 rounded {mode ===
          'API'
            ? 'bg-violet-500/10 text-violet-400'
            : mode === 'Stream'
              ? 'bg-blue-500/10 text-blue-400'
              : 'bg-emerald-500/10 text-emerald-400'}"
        >
          {mode}
        </span>
      {/if}

      {#if model}
        <span class="text-foreground/30">&middot;</span>
        {#if onModelChange}
          <button
            bind:this={modelBtnEl}
            class="flex items-center gap-1 shrink-0 rounded border border-transparent px-1.5 py-0.5 -my-0.5 text-foreground/80 hover:text-foreground hover:bg-accent hover:border-border transition-colors"
            onclick={toggleModelDropdown}
          >
            {modelLabel}
            <svg
              class="h-3 w-3 text-foreground/40"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
            >
          </button>
        {:else}
          <span class="truncate text-foreground/80">{model}</span>
        {/if}
      {/if}

      <!-- Context bar (Tier 1 — always visible when available) -->
      {#if contextWindow && contextWindow > 0 && contextUtilization != null}
        {@const pct = Math.round(contextUtilization * 100)}
        {@const barColor =
          contextWarningLevel === "critical"
            ? "bg-orange-500"
            : contextWarningLevel === "high"
              ? "bg-orange-500"
              : contextWarningLevel === "moderate"
                ? "bg-amber-500"
                : "bg-emerald-500"}
        {@const textColor =
          contextWarningLevel === "critical"
            ? "text-orange-500"
            : contextWarningLevel === "high"
              ? "text-orange-500"
              : contextWarningLevel === "moderate"
                ? "text-amber-500"
                : "text-foreground/60"}
        <span class="text-foreground/30">&middot;</span>
        <span
          class="flex items-center gap-1.5 {textColor}"
          title={t("statusbar_contextTitle", {
            pct: String(pct),
            tokens: contextWindow ? fmtNumber(contextWindow) : "",
          })}
        >
          <span class="inline-flex h-1.5 w-12 rounded-full bg-foreground/10 overflow-hidden">
            <span
              class="h-full rounded-full transition-all duration-700 ease-out {barColor}"
              style="width: {pct}%"
            ></span>
          </span>
          <span>{t("statusbar_ctx", { pct: String(pct) })}</span>
          {#if compactVisible}
            <span
              class="text-[10px] text-blue-500 font-medium animate-pulse"
              title={t("statusbar_compactDetail", {
                full: String(compactCount),
                micro: String(microcompactCount),
              })}>{t("statusbar_compacted")}</span
            >
          {/if}
        </span>
      {/if}

      {#if activeTaskCount && activeTaskCount > 0}
        <span class="text-foreground/30">&middot;</span>
        <span
          class="flex items-center gap-1 text-blue-400"
          title={t("bgTask_activeTitle", { count: String(activeTaskCount) })}
        >
          <span class="inline-block h-1.5 w-1.5 rounded-full bg-blue-400 animate-pulse"></span>
          <span class="text-[10px]">{t("bgTask_active", { count: String(activeTaskCount) })}</span>
        </span>
      {/if}
    </div>

    <!-- Right: actions + chevron -->
    <div class="flex items-center gap-2">
      {#if !running && onRewind && persistedFiles && persistedFiles.length > 0}
        <button
          class="flex items-center gap-1 rounded px-2 py-0.5 text-foreground/50 hover:text-foreground hover:bg-accent transition-colors"
          onclick={onRewind}
          title={t("statusbar_rewindTitle")}
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path
              d="M3 3v5h5"
            /></svg
          >
          {t("statusbar_rewind")}
        </button>
      {/if}
      {#if onFork && run?.session_id}
        <button
          class="flex items-center gap-1 rounded px-2 py-0.5 text-foreground/50 hover:text-foreground hover:bg-accent transition-colors"
          onclick={onFork}
          title={t("statusbar_forkTitle")}
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><circle cx="12" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><circle
              cx="18"
              cy="6"
              r="3"
            /><path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9" /><path d="M12 12v3" /></svg
          >
          {t("statusbar_fork")}
        </button>
      {/if}
      {#if running && onEndSession}
        {#if confirmingEnd}
          <div class="flex items-center gap-1">
            <span class="text-xs text-amber-500">{t("statusbar_endConfirm")}</span>
            <button
              class="rounded px-1.5 py-0.5 text-xs bg-destructive/20 text-destructive hover:bg-destructive/30 transition-colors"
              onclick={confirmEnd}>{t("statusbar_yes")}</button
            >
            <button
              class="rounded px-1.5 py-0.5 text-xs text-foreground/50 hover:text-foreground hover:bg-accent transition-colors"
              onclick={cancelEnd}>{t("statusbar_no")}</button
            >
          </div>
        {:else}
          <button
            class="flex items-center gap-1 rounded px-2 py-0.5 text-foreground/40 hover:text-foreground/60 transition-colors"
            onclick={requestEnd}
            title={t("statusbar_endTitle")}
          >
            {t("statusbar_endSession")}
          </button>
        {/if}
      {/if}

      {#if toolsCount && toolsCount > 0 && onToolsClick}
        <button
          class="text-[10px] text-muted-foreground hover:text-foreground transition-colors"
          onclick={onToolsClick}
          title={t("statusbar_showTools")}
        >
          {t("statusbar_tools", { count: String(toolsCount) })}
        </button>
      {/if}

      <!-- Expand/collapse chevron -->
      <button
        class="rounded p-0.5 text-foreground/30 hover:text-foreground/60 hover:bg-accent transition-colors"
        onclick={() => (expanded = !expanded)}
        title={expanded ? t("statusbar_collapse") : t("statusbar_expand")}
      >
        <svg
          class="h-3.5 w-3.5 transition-transform {expanded ? '' : 'rotate-180'}"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
    </div>
  </div>

  <!-- Tier 2: Collapsible details (h-7) -->
  {#if expanded}
    <div class="flex h-7 items-center justify-between px-3 border-t border-border/50">
      <!-- Left: details -->
      <div class="flex items-center gap-1.5 min-w-0 flex-1">
        {#if cwdShort}
          <span class="truncate" title={cwd || run?.cwd || ""}>{cwdShort}</span>
        {/if}

        {#if sessionIdShort}
          <span class="text-foreground/30">&middot;</span>
          <button
            class="text-foreground/40 hover:text-foreground/70 transition-colors"
            title="{t('statusbar_sessionLabel', {
              id: run?.session_id ?? '',
            })}\n{t('statusbar_clickToCopy')}"
            onclick={copySessionId}
          >
            {sidCopied ? t("statusbar_copied") : sessionIdShort}
          </button>
        {/if}

        {#if parentRunId && onNavigateParent}
          <span class="text-foreground/30">&middot;</span>
          <button
            class="flex items-center gap-1 text-blue-400/70 hover:text-blue-400 transition-colors"
            onclick={onNavigateParent}
            title={t("statusbar_viewParent")}
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
              <circle cx="12" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><circle
                cx="18"
                cy="6"
                r="3"
              />
              <path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9" /><path d="M12 12v3" />
            </svg>
            <span>{t("statusbar_forked")}</span>
          </button>
        {/if}

        {#if cost > 0}
          <span class="text-foreground/30">&middot;</span>
          <span>{formatCost(cost)}</span>
        {/if}

        {#if inputTokens > 0 || outputTokens > 0}
          <span class="text-foreground/30">&middot;</span>
          <span
            title={`${t("statusbar_inputLabel")}: ${fmtNumber(inputTokens)} / ${t("statusbar_outputLabel")}: ${fmtNumber(outputTokens)}${cacheReadTokens ? `\n${t("statusbar_cacheReadLabel")}: ${fmtNumber(cacheReadTokens)}` : ""}${cacheWriteTokens ? `\n${t("statusbar_cacheWriteLabel")}: ${fmtNumber(cacheWriteTokens)}` : ""}`}
            >{formatTokens(inputTokens)} / {formatTokens(outputTokens)} {t("statusbar_tok")}</span
          >
          {#if cacheReadTokens > 0 || cacheWriteTokens > 0}
            <span class="text-foreground/30 text-[10px]"
              >{t("statusbar_cacheRW", {
                read: formatTokens(cacheReadTokens),
                write: formatTokens(cacheWriteTokens),
              })}</span
            >
          {/if}
        {/if}

        {#if mcpServers && mcpServers.length > 0 && onMcpToggle}
          <span class="text-foreground/30">&middot;</span>
          <button
            class="flex items-center gap-1 shrink-0 rounded border border-transparent px-1.5 py-0.5 -my-0.5 text-foreground/70 hover:text-foreground hover:bg-accent hover:border-border transition-colors"
            onclick={onMcpToggle}
            title={t("statusbar_mcpTitle", { count: String(mcpServers.length) })}
          >
            <span class="inline-block h-1.5 w-1.5 rounded-full {mcpDotClass}"></span>
            <span>{t("statusbar_mcpLabel", { count: String(mcpServers.length) })}</span>
          </button>
        {/if}

        {#if numTurns && numTurns > 0}
          <span class="text-foreground/30">&middot;</span>
          <span title={t("statusbar_turnsTitle")}
            >{t("statusbar_turns", { count: String(numTurns) })}</span
          >
        {/if}

        {#if durationMs && durationMs > 0}
          {@const turnDetail = turnUsages
            .filter((tu) => tu.durationMs && tu.durationMs > 0)
            .map((tu) => `T${tu.turnIndex}: ${formatDuration(tu.durationMs!)}`)
            .join(", ")}
          <span class="text-foreground/30">&middot;</span>
          <span
            title={t("statusbar_durationTitle") +
              (turnDetail ? `\n${t("statusbar_durationPerTurn")}: ${turnDetail}` : "")}
            >{formatDuration(durationMs)}</span
          >
        {/if}
      </div>

      <!-- Right: secondary controls -->
      <div class="flex items-center gap-1.5">
        {#if permissionBadge}
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium {permissionBadge.cls}"
            title={t("statusbar_permissionMode", { mode: permissionMode ?? "" })}
            >{permissionBadge.label}</span
          >
        {/if}

        {#if fastModeState === "on"}
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium bg-amber-500/15 text-amber-500"
            title={t("statusbar_fastModeTitle")}>{t("statusbar_fastMode")}</span
          >
        {/if}

        {#if verbose}
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium bg-sky-500/15 text-sky-400"
            title={t("statusbar_verboseTitle")}>{t("statusbar_verbose")}</span
          >
        {/if}

        {#if authSourceLabel}
          {@const authBadgeColor =
            authSourceCategory === "login"
              ? "bg-emerald-500/15 text-emerald-500"
              : authSourceCategory === "env_key"
                ? "bg-blue-500/15 text-blue-400"
                : authSourceCategory === "none"
                  ? "bg-amber-500/15 text-amber-500"
                  : "bg-foreground/10 text-foreground/60"}
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium {authBadgeColor}"
            title={t("statusbar_authTitle", { source: apiKeySource ?? "" })}>{authSourceLabel}</span
          >
        {/if}

        {#if remoteHostName}
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium bg-blue-500/15 text-blue-500"
            title={t("statusbar_sshTitle", { name: remoteHostName ?? "" })}
            >{t("statusbar_sshLabel", { name: remoteHostName ?? "" })}</span
          >
        {/if}

        {#if cliVersion}
          <button
            class="text-foreground/30 hover:text-foreground/60 transition-colors"
            title={t("statusbar_cliVersionTitle", { version: cliVersion ?? "" })}
            onclick={() => goto("/release-notes")}>v{cliVersion}</button
          >
        {/if}
      </div>
    </div>
  {/if}
</div>

{#if dropdownOpen}
  <div
    bind:this={dropdownEl}
    tabindex="-1"
    role="listbox"
    class="min-w-[560px] w-max rounded-md border bg-background shadow-lg animate-fade-in outline-none"
    style={dropdownStyle}
    onkeydown={handleDropdownKeydown}
  >
    <div class="p-1">
      {#each models as m, i}
        <button
          class="flex w-full items-center gap-2 rounded-sm px-3 py-2 text-xs hover:bg-accent transition-colors {model ===
          m.value
            ? 'bg-accent font-medium'
            : ''} {i === focusedModelIdx ? 'ring-1 ring-primary/50' : ''}"
          onclick={() => selectModel(m.value)}
        >
          {#if model === m.value}
            <svg
              class="h-3 w-3 text-primary shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"><path d="M20 6 9 17l-5-5" /></svg
            >
          {:else}
            <span class="w-3 shrink-0"></span>
          {/if}
          <span class="shrink-0 text-foreground">{m.displayName}</span>
          <span class="text-[10px] text-foreground/50 truncate">{m.description}</span>
        </button>
      {/each}
    </div>
  </div>
{/if}

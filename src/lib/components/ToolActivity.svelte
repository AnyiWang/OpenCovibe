<script lang="ts">
  import type { HookEvent, ContextSnapshot, SessionInfoData, FileEntry } from "$lib/types";
  import type { TimelineEntry, BusToolItem, TurnUsage } from "$lib/stores/types";
  import { getToolColor } from "$lib/utils/tool-colors";
  import { truncate, formatTokenCount, formatDuration } from "$lib/utils/format";
  import { getToolDetail as getToolDetailRaw } from "$lib/utils/tool-rendering";
  import { dbg } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import ContextHistoryPanel from "$lib/components/ContextHistoryPanel.svelte";
  import FilesPanel from "$lib/components/FilesPanel.svelte";
  import FilePreviewPane from "$lib/components/FilePreviewPane.svelte";
  import { onMount } from "svelte";
  import SessionInfoPanel from "$lib/components/SessionInfoPanel.svelte";
  import StatusIcon from "$lib/components/StatusIcon.svelte";
  import {
    extractFilesFromTimeline,
    extractFilesFromHooks,
    extractFilesFromPersisted,
    mergeFileEntries,
  } from "$lib/utils/file-entries";
  import { extractTaskToolMeta, type TaskToolMeta } from "$lib/utils/tool-rendering";
  import type { TaskNotificationItem } from "$lib/stores/session-store.svelte";

  let {
    timeline = [],
    tools = [],
    turnUsages = [],
    contextHistory = [],
    persistedFiles = [],
    sessionInfo = null,
    collapsed = false,
    onToggle,
    onScrollToTool,
    onScrollToTurn,
    requestedTab = $bindable(null as "tools" | "context" | "files" | "info" | "tasks" | null),
    backgroundTasks = new Map(),
    activeBackgroundTasks = [],
    cwd = "",
    runId = "",
    isRemote = false,
    requestedPreviewPath = $bindable(null as string | null),
  }: {
    timeline: TimelineEntry[];
    tools: HookEvent[];
    turnUsages?: TurnUsage[];
    contextHistory?: ContextSnapshot[];
    persistedFiles?: unknown[];
    sessionInfo?: SessionInfoData | null;
    collapsed: boolean;
    onToggle: () => void;
    onScrollToTool?: (toolUseId: string) => void;
    onScrollToTurn?: (anchorId: string) => void;
    requestedTab?: "tools" | "context" | "files" | "info" | "tasks" | null;
    backgroundTasks?: Map<string, TaskNotificationItem>;
    activeBackgroundTasks?: TaskNotificationItem[];
    /** Working directory for file preview (typically store.effectiveCwd). */
    cwd?: string;
    /** Run id — when it changes the preview is cleared. */
    runId?: string;
    /** Remote run flag — disables file preview (file APIs are local-only). */
    isRemote?: boolean;
    /** External request to open preview for a path (auto-switches to files tab). */
    requestedPreviewPath?: string | null;
  } = $props();

  // ── Tab state ──
  type SidebarPanel = "tools" | "context" | "files" | "info" | "tasks";
  let activeTab: SidebarPanel = $state("tools");

  // ── External tab request ──
  $effect(() => {
    if (requestedTab) {
      activeTab = requestedTab;
      requestedTab = null;
    }
  });

  // ── Preview state ──
  let previewPath = $state<string | null>(null);

  // External preview request → set path + switch tab; consume by setting $bindable to null
  $effect(() => {
    if (requestedPreviewPath) {
      previewPath = requestedPreviewPath;
      activeTab = "files";
      requestedPreviewPath = null;
    }
  });

  // Clear preview when run changes (different session — paths from previous run no longer relevant)
  $effect(() => {
    void runId;
    previewPath = null;
  });

  // ── Width state (browser-safe initialization) ──
  const WIDTH_MIN = 280;
  const WIDTH_MAX = 720;
  const WIDTH_DEFAULT = 320;
  const WIDTH_AUTO_PREVIEW = 560;

  function clampWidth(v: number): number {
    return Math.max(WIDTH_MIN, Math.min(WIDTH_MAX, v));
  }

  let savedWidth = $state(WIDTH_DEFAULT);
  let viewportWidth = $state(1200);
  /** True once the user has explicitly resized (or loaded a persisted resize). Disables auto-floor. */
  let userHasResized = $state(false);

  onMount(() => {
    if (typeof window === "undefined") return;
    const stored = window.localStorage.getItem("ocv:toolactivity-width");
    if (stored) {
      const n = parseInt(stored, 10);
      if (Number.isFinite(n)) {
        savedWidth = clampWidth(n);
        userHasResized = true;
      }
    }
    viewportWidth = window.innerWidth;
    const onResize = () => {
      viewportWidth = window.innerWidth;
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  });

  /**
   * Effective width displayed:
   * - If user has explicitly resized (or has a persisted preference) → always respect savedWidth.
   * - Else if a preview is open → temporarily widen to accommodate code, but DO NOT persist.
   * - Else → savedWidth (compact default).
   *
   * This keeps the auto-widening transient: closing the preview returns to savedWidth, and
   * once the user drags the handle their preference is honored regardless of preview state.
   */
  let effectiveWidth = $derived.by(() => {
    if (userHasResized) return clampWidth(savedWidth);
    if (previewPath) {
      const autoFloor = Math.min(WIDTH_AUTO_PREVIEW, 0.45 * viewportWidth);
      return clampWidth(Math.max(savedWidth, autoFloor));
    }
    return clampWidth(savedWidth);
  });

  // ── Resize handle (VS Code-style: ghost line during drag, single commit on release) ──
  // Why this approach: in-place live resize forces chat-main reflow on every pointermove,
  // which is too expensive with thousands of chat DOM nodes. Instead, during drag we DON'T
  // move any panel — only render a fixed-position vertical line at the cursor that previews
  // the new boundary. On release we commit savedWidth ONCE → single reflow.
  let resizing = $state(false);
  let ghostX = $state(0);
  let resizeStartX = 0;
  let resizeStartWidth = 0;
  let pendingWidth: number | null = null;
  let rafId: number | null = null;
  let asideEl: HTMLElement | undefined = $state();

  function onResizeStart(e: PointerEvent) {
    resizing = true;
    resizeStartX = e.clientX;
    resizeStartWidth = effectiveWidth;
    pendingWidth = resizeStartWidth;
    ghostX = e.clientX;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    e.preventDefault();
  }

  function flushGhostFrame() {
    rafId = null;
    // ghostX state already updated; this is just a frame-aligned re-render gate.
  }

  function onResizeMove(e: PointerEvent) {
    if (!resizing) return;
    const delta = resizeStartX - e.clientX; // dragging left grows the panel
    pendingWidth = clampWidth(resizeStartWidth + delta);
    // Snap ghost line to the new panel boundary (clamped). Aside is on the right side of
    // the viewport, so its left edge after commit = window.innerWidth - pendingWidth.
    const wantedX = typeof window !== "undefined" ? window.innerWidth - pendingWidth : e.clientX;
    if (rafId === null && typeof window !== "undefined") {
      rafId = window.requestAnimationFrame(flushGhostFrame);
    }
    ghostX = wantedX;
  }

  function onResizeEnd(e: PointerEvent) {
    if (!resizing) return;
    resizing = false;
    if (rafId !== null && typeof window !== "undefined") {
      window.cancelAnimationFrame(rafId);
      rafId = null;
    }
    (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);

    if (pendingWidth !== null && pendingWidth !== savedWidth) {
      savedWidth = pendingWidth;
      userHasResized = true;
      if (typeof window !== "undefined") {
        window.localStorage.setItem("ocv:toolactivity-width", String(savedWidth));
      }
    }
    pendingWidth = null;
  }

  // ── Helpers ──

  function getToolDetail(tool: BusToolItem): string {
    return truncate(getToolDetailRaw(tool.input as Record<string, unknown>), 50);
  }

  function getHookDetail(event: HookEvent): string {
    return truncate(getToolDetailRaw(event.tool_input as Record<string, unknown>), 50);
  }

  type StatusCategory = "done" | "running" | "error" | "other";

  function categorizeBusStatus(status: string): StatusCategory {
    switch (status) {
      case "success":
        return "done";
      case "running":
        return "running";
      case "error":
      case "denied":
      case "permission_denied":
        return "error";
      case "ask_pending":
      case "permission_prompt":
        return "other";
      default:
        return "other";
    }
  }

  function categorizeHookStatus(status: string | undefined): StatusCategory {
    if (!status) return "other";
    switch (status) {
      case "done":
      case "success":
        return "done";
      case "running":
      case "pending":
        return "running";
      case "error":
      case "denied":
        return "error";
      default:
        return "other";
    }
  }

  // ── Tree structure for hierarchical tool display ──

  interface ToolNode {
    tool: BusToolItem;
    children: ToolNode[];
  }

  /** Build a tree from TimelineEntries, preserving parent→child hierarchy. */
  function buildToolTree(entries: TimelineEntry[], seen: Set<string>): ToolNode[] {
    const result: ToolNode[] = [];
    for (const entry of entries) {
      if (entry.kind === "tool" && !seen.has(entry.tool.tool_use_id)) {
        seen.add(entry.tool.tool_use_id);
        result.push({
          tool: entry.tool,
          children: entry.subTimeline ? buildToolTree(entry.subTimeline, seen) : [],
        });
      }
    }
    return result;
  }

  /** Flatten tree nodes for counting/statistics. */
  function flattenNodes(nodes: ToolNode[]): BusToolItem[] {
    const result: BusToolItem[] = [];
    for (const node of nodes) {
      result.push(node.tool);
      if (node.children.length > 0) result.push(...flattenNodes(node.children));
    }
    return result;
  }

  /** Recursively count all nodes in a tree. */
  function countToolNodes(nodes: ToolNode[]): number {
    let count = 0;
    for (const node of nodes) count += 1 + countToolNodes(node.children);
    return count;
  }

  // ── Dual-source strategy ──

  // ── Background tasks (sorted: active first, then by recency) ──

  let sortedBgTasks = $derived.by(() => {
    const items = [...backgroundTasks.values()];
    return items.sort((a, b) => {
      const aActive =
        a.status !== "completed" && a.status !== "failed" && a.status !== "error" ? 0 : 1;
      const bActive =
        b.status !== "completed" && b.status !== "failed" && b.status !== "error" ? 0 : 1;
      if (aActive !== bActive) return aActive - bActive;
      return b.startedAt - a.startedAt;
    });
  });

  function bgElapsed(startedAt: number): string {
    const ms = Date.now() - startedAt;
    if (ms < 1000) return "<1s";
    return `${Math.floor(ms / 1000)}s`;
  }

  let useTimeline = $derived(timeline.some((e) => e.kind === "tool"));

  // ── Turn grouping (timeline mode) ──

  interface ToolTurn {
    turnIndex: number;
    userPreview: string;
    tools: ToolNode[];
    anchorId?: string;
  }

  let turns = $derived.by(() => {
    if (!useTimeline) return [];
    const result: ToolTurn[] = [];
    let currentTools: ToolNode[] = [];
    let currentPreview = "";
    let currentAnchorId: string | undefined;
    let turnIdx = 0;
    // Defensive dedup: CLI can emit events with missing parent_tool_use_id,
    // causing the same tool_use_id to appear in both main timeline and a subTimeline.
    // Track seen IDs to prevent each_key_duplicate crashes in {#each} blocks.
    const seen = new Set<string>();

    for (const entry of timeline) {
      if (entry.kind === "separator") continue;
      if (entry.kind === "user") {
        // Flush previous turn (guard: don't flush initial empty state)
        if (currentTools.length > 0 || currentPreview || currentAnchorId) {
          result.push({
            turnIndex: turnIdx,
            userPreview: currentPreview,
            tools: currentTools,
            anchorId: currentAnchorId,
          });
        }
        turnIdx++;
        currentPreview = entry.content.slice(0, 40);
        currentAnchorId = entry.anchorId;
        currentTools = [];
      } else if (entry.kind === "tool") {
        if (!seen.has(entry.tool.tool_use_id)) {
          seen.add(entry.tool.tool_use_id);
          currentTools.push({
            tool: entry.tool,
            children: entry.subTimeline ? buildToolTree(entry.subTimeline, seen) : [],
          });
        }
      }
    }
    // Flush last turn
    if (currentTools.length > 0 || currentPreview || currentAnchorId) {
      result.push({
        turnIndex: turnIdx,
        userPreview: currentPreview,
        tools: currentTools,
        anchorId: currentAnchorId,
      });
    }
    return result;
  });

  // ── HookEvent fallback (pipe/PTY mode) ──

  let hookToolEvents = $derived(tools.filter((e) => e.tool_name));

  // ── File entries (dual-source + persisted merge) ──

  let fileEntries: FileEntry[] = $derived.by(() => {
    const timelineFiles = useTimeline
      ? extractFilesFromTimeline(timeline)
      : extractFilesFromHooks(hookToolEvents);
    const persistedEntries = extractFilesFromPersisted(persistedFiles ?? []);
    return mergeFileEntries(
      { entries: timelineFiles, hasTemporalOrder: true },
      { entries: persistedEntries, hasTemporalOrder: false },
    );
  });

  // ── Subagent extraction (for info tab) ──

  interface SubagentInfo {
    toolUseId: string;
    meta: TaskToolMeta;
    status: string;
    durationMs?: number;
    toolCount: number;
  }

  let subagents: SubagentInfo[] = $derived.by(() => {
    if (!useTimeline) return [];
    const result: SubagentInfo[] = [];
    for (const turn of turns) {
      for (const node of flattenNodes(turn.tools)) {
        if (node.tool_name === "Task") {
          const meta = extractTaskToolMeta(node.input);
          if (!meta) continue;
          // Count nested tools from the result
          let toolCount = 0;
          let durationMs: number | undefined;
          const tur = node.tool_use_result as Record<string, unknown> | undefined;
          if (tur && typeof tur === "object") {
            if ("totalToolUseCount" in tur) toolCount = tur.totalToolUseCount as number;
            if ("totalDurationMs" in tur) durationMs = tur.totalDurationMs as number;
          }
          result.push({
            toolUseId: node.tool_use_id,
            meta,
            status: node.status,
            durationMs,
            toolCount,
          });
        }
      }
    }
    return result;
  });

  // ── Summary + status counts (single-pass) ──

  let toolStats = $derived.by(() => {
    const counts: Record<string, number> = {};
    let done = 0,
      running = 0,
      errors = 0,
      total = 0;
    if (useTimeline) {
      for (const turn of turns) {
        for (const t of flattenNodes(turn.tools)) {
          counts[t.tool_name] = (counts[t.tool_name] ?? 0) + 1;
          total++;
          const cat = categorizeBusStatus(t.status);
          if (cat === "done") done++;
          else if (cat === "running") running++;
          else if (cat === "error") errors++;
        }
      }
    } else {
      for (const ev of hookToolEvents) {
        const name = ev.tool_name ?? "other";
        counts[name] = (counts[name] ?? 0) + 1;
        total++;
        const cat = categorizeHookStatus(ev.status);
        if (cat === "done") done++;
        else if (cat === "running") running++;
        else if (cat === "error") errors++;
      }
    }
    return {
      summary: Object.entries(counts).sort((a, b) => b[1] - a[1]),
      doneCount: done,
      runningCount: running,
      errorCount: errors,
      totalToolCount: total,
    };
  });
  // ── Per-turn usage lookup ──

  let usageByTurn = $derived(new Map(turnUsages.map((tu) => [tu.turnIndex, tu])));

  // ── Collapsible turn state ──
  // Default: collapse all turns except the latest to reduce initial DOM count

  let collapsedTurns = $state(new Set<number>());

  // Auto-collapse older turns when turn count changes (session load / new turn)
  let prevTurnCount = 0;
  $effect(() => {
    const count = turns.length;
    if (count !== prevTurnCount && count > 1) {
      const collapsed = new Set<number>();
      for (const turn of turns) {
        // Collapse all except the last turn
        if (turn !== turns[turns.length - 1]) {
          collapsed.add(turn.turnIndex);
        }
      }
      collapsedTurns = collapsed;
    }
    prevTurnCount = count;
  });

  function toggleTurn(turnIndex: number) {
    if (collapsedTurns.has(turnIndex)) {
      collapsedTurns.delete(turnIndex);
    } else {
      collapsedTurns.add(turnIndex);
    }
    collapsedTurns = new Set(collapsedTurns);
  }

  $effect(() => {
    dbg("tools", "sidebar updated", {
      useTimeline,
      turns: turns.length,
      hookTools: hookToolEvents.length,
      total: toolStats.totalToolCount,
      files: fileEntries.length,
    });
  });
</script>

{#snippet statusIcon(category: StatusCategory)}
  <StatusIcon status={category} size="sm" />
{/snippet}

{#snippet toolNodeView(node: ToolNode)}
  {@const style = getToolColor(node.tool.tool_name)}
  {@const detail = getToolDetail(node.tool)}
  {@const cat = categorizeBusStatus(node.tool.status)}
  <button
    class="w-full text-left px-2.5 py-1 hover:bg-accent/50 rounded-sm transition-colors group"
    onclick={() => onScrollToTool?.(node.tool.tool_use_id)}
    title={t("toolActivity_scrollToTool")}
  >
    <div class="flex items-center gap-1.5">
      {@render statusIcon(cat)}
      <div class="flex h-4 w-4 shrink-0 items-center justify-center rounded {style.bg}">
        <svg
          class="h-2.5 w-2.5 {style.text}"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d={style.icon} />
        </svg>
      </div>
      <span class="text-[11px] font-medium text-foreground shrink-0">{node.tool.tool_name}</span>
      {#if detail}
        <span
          class="text-[10px] text-muted-foreground truncate min-w-0 opacity-70 group-hover:opacity-100"
          >{detail}</span
        >
      {/if}
    </div>
  </button>
  {#if node.children.length > 0}
    <div class="ml-5 border-l-2 border-cyan-500/25">
      {#each node.children as child (child.tool.tool_use_id)}
        {@render toolNodeView(child)}
      {/each}
    </div>
  {/if}
{/snippet}

<!--
  Outer wrapper is ALWAYS mounted: width animates between 32px (collapsed) and effectiveWidth.
  The expanded panel inside stays in the DOM at full width but is visually clipped + hidden when
  collapsed, so CodeMirror (FilePreviewPane) is never torn down on collapse toggle. This is the
  fix for "files tab + click expand briefly hangs".
-->
{#if resizing}
  <!-- Ghost line during drag: zero-cost preview, no layout reflow elsewhere -->
  <div
    class="fixed top-0 bottom-0 w-0.5 bg-primary/60 z-[9999] pointer-events-none"
    style="left: {ghostX}px;"
  ></div>
{/if}
<aside
  bind:this={asideEl}
  class="relative h-full border-l border-border bg-muted/30 overflow-hidden"
  style="width: {collapsed ? '32px' : effectiveWidth + 'px'}; contain: layout style;"
>
  <!-- Always-mounted expanded panel (visibility-hidden when collapsed) -->
  <div
    class="absolute top-0 left-0 h-full flex flex-col"
    style="width: {effectiveWidth}px; visibility: {collapsed
      ? 'hidden'
      : 'visible'}; pointer-events: {collapsed ? 'none' : 'auto'};"
    aria-hidden={collapsed}
  >
    <!-- Resize handle on the left edge -->
    <div
      role="separator"
      aria-orientation="vertical"
      tabindex="-1"
      class="absolute left-0 top-0 h-full w-1 cursor-col-resize hover:bg-primary/30 active:bg-primary/50 z-20 {resizing
        ? 'bg-primary/50'
        : ''}"
      onpointerdown={onResizeStart}
      onpointermove={onResizeMove}
      onpointerup={onResizeEnd}
      onpointercancel={onResizeEnd}
    ></div>
    <!-- Header: 4 icon tabs -->
    <div class="px-2 py-1.5 border-b border-border">
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-0.5">
          <!-- Tools icon -->
          <button
            class="p-1.5 rounded transition-colors {activeTab === 'tools'
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
            onclick={() => (activeTab = "tools")}
            title={t("toolActivity_tabTools")}
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
              <path
                d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"
              />
            </svg>
          </button>
          <!-- Context icon -->
          <button
            class="p-1.5 rounded transition-colors relative {activeTab === 'context'
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
            onclick={() => (activeTab = "context")}
            title={t("toolActivity_tabContext")}
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
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
            </svg>
            {#if contextHistory.length > 0}
              <span class="absolute top-0.5 right-0.5 h-1.5 w-1.5 rounded-full bg-emerald-500"
              ></span>
            {/if}
          </button>
          <!-- Files icon -->
          <button
            class="p-1.5 rounded transition-colors relative {activeTab === 'files'
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
            onclick={() => (activeTab = "files")}
            title={t("toolActivity_tabFiles")}
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
              <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
              <polyline points="14 2 14 8 20 8" />
            </svg>
            {#if fileEntries.length > 0}
              <span class="absolute top-0.5 right-0.5 h-1.5 w-1.5 rounded-full bg-amber-400"></span>
            {/if}
          </button>
          <!-- Info icon -->
          <button
            class="p-1.5 rounded transition-colors {activeTab === 'info'
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
            onclick={() => (activeTab = "info")}
            title={t("toolActivity_tabInfo")}
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
              <circle cx="12" cy="12" r="10" />
              <line x1="12" y1="16" x2="12" y2="12" />
              <line x1="12" y1="8" x2="12.01" y2="8" />
            </svg>
          </button>
          <!-- Tasks icon -->
          <button
            class="p-1.5 rounded transition-colors relative {activeTab === 'tasks'
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
            onclick={() => (activeTab = "tasks")}
            title={t("toolActivity_tabTasks")}
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
              <rect x="3" y="3" width="18" height="18" rx="2" />
              <path d="M9 12l2 2 4-4" />
            </svg>
            {#if activeBackgroundTasks.length > 0}
              <span
                class="absolute top-0.5 right-0.5 h-1.5 w-1.5 rounded-full bg-blue-400 animate-pulse"
              ></span>
            {/if}
          </button>
        </div>
        <button
          class="text-muted-foreground hover:text-foreground transition-colors p-0.5 rounded hover:bg-accent"
          onclick={onToggle}
          title={t("toolActivity_collapse")}
        >
          <svg
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <polyline points="15 18 9 12 15 6" />
          </svg>
        </button>
      </div>
    </div>

    {#if activeTab === "tasks"}
      <!-- Background tasks panel -->
      <div class="flex-1 overflow-y-auto">
        {#if backgroundTasks.size === 0}
          <div class="flex items-center justify-center h-32 text-xs text-muted-foreground/50">
            {t("bgTask_empty")}
          </div>
        {:else}
          <div class="py-1 space-y-0.5">
            {#each sortedBgTasks as item (item.task_id)}
              {@const isDone = item.status === "completed"}
              {@const isFailed = item.status === "failed" || item.status === "error"}
              {@const isActive = !isDone && !isFailed}
              {@const rawData = (item.data as Record<string, unknown> | undefined)?.data as
                | Record<string, unknown>
                | undefined}
              {@const usage = rawData?.usage as
                | { duration_ms?: number; tool_uses?: number; total_tokens?: number }
                | undefined}
              {@const toolUseId = item.tool_use_id}
              <button
                class="w-full text-left mx-1.5 rounded px-2 py-1.5 transition-colors {isDone
                  ? 'text-foreground/40 hover:bg-accent/30'
                  : isFailed
                    ? 'bg-destructive/5 text-foreground/50 hover:bg-destructive/10'
                    : 'bg-blue-500/5 text-foreground/70 hover:bg-blue-500/10'}"
                onclick={() => {
                  if (toolUseId) onScrollToTool?.(toolUseId);
                }}
                title={toolUseId ? t("toolActivity_scrollToTool") : ""}
              >
                <div class="flex items-center gap-2">
                  <StatusIcon status={isActive ? "running" : isDone ? "done" : "error"} size="sm" />
                  <span class="flex-1 min-w-0 truncate text-[11px]"
                    >{item.summary || item.message}</span
                  >
                  {#if isActive}
                    <span class="shrink-0 text-[10px] text-foreground/30 tabular-nums"
                      >{bgElapsed(item.startedAt)}</span
                    >
                  {/if}
                </div>
                {#if usage && (usage.tool_uses || usage.total_tokens || usage.duration_ms)}
                  <div class="mt-0.5 text-[10px] text-muted-foreground/60 pl-5">
                    {#if usage.tool_uses}{usage.tool_uses} tools{/if}
                    {#if usage.tool_uses && usage.duration_ms}
                      ·
                    {/if}
                    {#if usage.duration_ms}{formatDuration(usage.duration_ms)}{/if}
                    {#if (usage.tool_uses || usage.duration_ms) && usage.total_tokens}
                      ·
                    {/if}
                    {#if usage.total_tokens}{formatTokenCount(usage.total_tokens)} tok{/if}
                  </div>
                {/if}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {:else if activeTab === "context"}
      <ContextHistoryPanel history={contextHistory} {turnUsages} {sessionInfo} />
    {:else if activeTab === "files"}
      <div class="flex flex-1 flex-col min-h-0">
        <div class="flex-shrink-0 max-h-[40vh] overflow-y-auto border-b border-border/50">
          <FilesPanel
            {fileEntries}
            {onScrollToTool}
            onPreview={(p) => (previewPath = p)}
            selectedPath={previewPath ?? undefined}
          />
        </div>
        <div class="flex-1 min-h-0 overflow-hidden">
          <FilePreviewPane
            {cwd}
            path={previewPath ?? ""}
            mode="preview"
            editable={false}
            {isRemote}
            scopeKey={runId}
          />
        </div>
      </div>
    {:else if activeTab === "info"}
      <!-- Subagents section (shown above session info when Task tools exist) -->
      {#if subagents.length > 0}
        <div class="px-3 py-2 border-b border-border/50">
          <div
            class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5"
          >
            {t("tool_subagents", { count: String(subagents.length) })}
          </div>
          <div class="space-y-1.5">
            {#each subagents as sa (sa.toolUseId)}
              {@const isDone = sa.status === "success"}
              {@const isError = sa.status === "error" || sa.status === "denied"}
              {@const isRunning = !isDone && !isError}
              <button
                class="w-full text-left rounded-md border border-border/50 bg-background/50 px-2.5 py-1.5 hover:bg-accent/30 transition-colors"
                onclick={() => onScrollToTool?.(sa.toolUseId)}
                title="Scroll to tool"
              >
                <div class="flex items-center gap-1.5">
                  <span class="text-[11px] font-medium text-foreground">{sa.meta.subagentType}</span
                  >
                  {#if sa.meta.model}
                    <span
                      class="text-[10px] px-1 py-0.5 rounded bg-cyan-500/15 text-cyan-600 dark:text-cyan-400 font-medium"
                      >{sa.meta.model}</span
                    >
                  {/if}
                  <span class="ml-auto">
                    {#if isDone}
                      <StatusIcon status="done" size="sm" />
                    {:else if isError}
                      <StatusIcon status="error" size="sm" />
                    {:else if isRunning}
                      <StatusIcon status="running" size="sm" />
                    {/if}
                  </span>
                </div>
                {#if sa.meta.description}
                  <div class="text-[10px] text-muted-foreground truncate mt-0.5">
                    {sa.meta.description}
                  </div>
                {/if}
                {#if sa.toolCount > 0 || sa.durationMs != null}
                  <div class="text-[10px] text-muted-foreground/60 mt-0.5">
                    {#if sa.toolCount > 0}{sa.toolCount} tools{/if}
                    {#if sa.toolCount > 0 && sa.durationMs != null}
                      ·
                    {/if}
                    {#if sa.durationMs != null}{formatDuration(sa.durationMs)}{/if}
                  </div>
                {/if}
              </button>
            {/each}
          </div>
        </div>
      {/if}
      <SessionInfoPanel info={sessionInfo} {activeTab} />
    {:else}
      <!-- Tools panel -->
      <!-- Summary chips -->
      {#if toolStats.summary.length > 1}
        <div class="flex flex-wrap gap-1 px-2.5 py-1.5 border-b border-border/50">
          {#each toolStats.summary as [name, count]}
            {@const style = getToolColor(name)}
            <span
              class="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded {style.bg} {style.text} font-medium"
            >
              {name}
              <span class="opacity-70">{count}</span>
            </span>
          {/each}
        </div>
      {/if}

      <!-- Tool list -->
      <div class="flex-1 overflow-y-auto py-0.5">
        {#if toolStats.totalToolCount === 0}
          <div class="flex items-center justify-center h-32 text-xs text-muted-foreground/50">
            {t("toolActivity_noToolCalls")}
          </div>
        {:else if useTimeline}
          <!-- Timeline mode: grouped by turn -->
          {#each turns as turn (turn.turnIndex)}
            {@const isCollapsed = collapsedTurns.has(turn.turnIndex)}
            {@const tu = usageByTurn.get(turn.turnIndex)}
            {@const hasTools = turn.tools.length > 0}
            <!-- Turn header: div with two sibling buttons (no nesting) -->
            <div
              class="flex items-center w-full px-2.5 py-1.5 hover:bg-accent/50 transition-colors border-b border-border/30"
            >
              <button
                class="flex-1 flex items-center gap-1.5 text-left min-w-0"
                onclick={() => {
                  if (hasTools) {
                    toggleTurn(turn.turnIndex);
                  } else if (turn.anchorId) {
                    dbg("tool-activity", "scroll to turn (no tools)", {
                      turnIndex: turn.turnIndex,
                      anchorId: turn.anchorId,
                    });
                    onScrollToTurn?.(turn.anchorId);
                  }
                }}
              >
                {#if hasTools}
                  <svg
                    class="h-3 w-3 text-muted-foreground/50 shrink-0 transition-transform {isCollapsed
                      ? ''
                      : 'rotate-90'}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <polyline points="9 18 15 12 9 6" />
                  </svg>
                {/if}
                <span class="text-[11px] font-medium text-muted-foreground truncate">
                  {#if turn.userPreview}
                    {t("toolActivity_turn", { index: String(turn.turnIndex) })}
                    <span class="text-foreground/70">{truncate(turn.userPreview, 25)}</span>
                  {:else}
                    <span class="text-muted-foreground/60">{t("toolActivity_systemResume")}</span>
                  {/if}
                </span>
                <span class="ml-auto flex items-center gap-1.5 shrink-0">
                  {#if tu}
                    <span class="text-[10px] text-muted-foreground"
                      >{formatTokenCount(tu.inputTokens + tu.outputTokens)}</span
                    >
                  {/if}
                  {#if hasTools}
                    <span
                      class="text-[10px] px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground font-medium"
                      >{countToolNodes(turn.tools)}</span
                    >
                  {/if}
                </span>
              </button>
              {#if turn.anchorId}
                <button
                  class="shrink-0 ml-1 p-0.5 rounded text-muted-foreground/40 hover:text-foreground hover:bg-muted transition-colors"
                  onclick={() => {
                    dbg("tool-activity", "scroll to turn", {
                      turnIndex: turn.turnIndex,
                      anchorId: turn.anchorId,
                    });
                    onScrollToTurn?.(turn.anchorId!);
                  }}
                  title={t("toolActivity_scrollToTurn")}
                >
                  <svg
                    class="h-3 w-3"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    ><circle cx="12" cy="12" r="3" /><path d="M12 2v4m0 12v4M2 12h4m12 0h4" /></svg
                  >
                </button>
              {/if}
            </div>

            <!-- Tools in this turn (only render if turn has tools) -->
            {#if hasTools && !isCollapsed}
              <div class="py-0.5">
                {#each turn.tools as node (node.tool.tool_use_id)}
                  {@render toolNodeView(node)}
                {/each}
              </div>
            {/if}
          {/each}
        {:else}
          <!-- HookEvent fallback mode (pipe/PTY) -->
          {#each hookToolEvents as event, ei (ei)}
            {@const style = getToolColor(event.tool_name ?? "")}
            {@const detail = getHookDetail(event)}
            {@const cat = categorizeHookStatus(event.status)}
            <div class="px-2.5 py-1">
              <div class="flex items-center gap-1.5">
                {@render statusIcon(cat)}
                <div class="flex h-4 w-4 shrink-0 items-center justify-center rounded {style.bg}">
                  <svg
                    class="h-2.5 w-2.5 {style.text}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <path d={style.icon} />
                  </svg>
                </div>
                <span class="text-[11px] font-medium text-foreground shrink-0"
                  >{event.tool_name ?? event.hook_type}</span
                >
                {#if detail}
                  <span class="text-[10px] text-muted-foreground truncate min-w-0">{detail}</span>
                {/if}
              </div>
            </div>
          {/each}
        {/if}
      </div>

      <!-- Stats footer (status counts only, tools tab only) -->
      {#if toolStats.totalToolCount > 0}
        <div class="border-t border-border px-3 py-1.5">
          <div class="flex items-center gap-3 text-[11px]">
            {#if toolStats.doneCount > 0}
              <span class="flex items-center gap-1 text-emerald-500 dark:text-emerald-400">
                <StatusIcon status="done" size="sm" />
                {toolStats.doneCount}
              </span>
            {/if}
            {#if toolStats.runningCount > 0}
              <span class="flex items-center gap-1 text-muted-foreground">
                <StatusIcon status="running" size="sm" />
                {toolStats.runningCount}
              </span>
            {/if}
            {#if toolStats.errorCount > 0}
              <span class="flex items-center gap-1 text-destructive">
                <StatusIcon status="error" size="sm" />
                {toolStats.errorCount}
              </span>
            {/if}
          </div>
        </div>
      {/if}
    {/if}
  </div>
  {#if collapsed}
    <!-- Collapsed: thin icon rail overlay (absolute, only mounted when collapsed) -->
    <div
      class="absolute top-0 left-0 h-full flex flex-col items-center py-2 px-1 gap-1"
      style="width: 32px;"
    >
      <button
        class="text-muted-foreground hover:text-foreground transition-colors p-1 rounded hover:bg-accent"
        onclick={onToggle}
        title={t("toolActivity_show")}
      >
        <svg
          class="h-4 w-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <polyline points="9 18 15 12 9 6" />
        </svg>
      </button>
      <!-- Collapsed icon buttons -->
      <button
        class="p-1 rounded transition-colors {activeTab === 'tools'
          ? 'text-foreground bg-accent'
          : 'text-muted-foreground/50 hover:text-muted-foreground'}"
        onclick={() => {
          activeTab = "tools";
          onToggle();
        }}
        title={t("toolActivity_tabTools")}
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
          <path
            d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"
          />
        </svg>
      </button>
      <button
        class="p-1 rounded transition-colors {activeTab === 'context'
          ? 'text-foreground bg-accent'
          : 'text-muted-foreground/50 hover:text-muted-foreground'}"
        onclick={() => {
          activeTab = "context";
          onToggle();
        }}
        title={t("toolActivity_tabContext")}
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
          <circle cx="12" cy="12" r="10" />
          <polyline points="12 6 12 12 16 14" />
        </svg>
      </button>
      <button
        class="p-1 rounded transition-colors {activeTab === 'files'
          ? 'text-foreground bg-accent'
          : 'text-muted-foreground/50 hover:text-muted-foreground'}"
        onclick={() => {
          activeTab = "files";
          onToggle();
        }}
        title={t("toolActivity_tabFiles")}
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
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <polyline points="14 2 14 8 20 8" />
        </svg>
      </button>
      <button
        class="p-1 rounded transition-colors {activeTab === 'info'
          ? 'text-foreground bg-accent'
          : 'text-muted-foreground/50 hover:text-muted-foreground'}"
        onclick={() => {
          activeTab = "info";
          onToggle();
        }}
        title={t("toolActivity_tabInfo")}
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
          <circle cx="12" cy="12" r="10" />
          <line x1="12" y1="16" x2="12" y2="12" />
          <line x1="12" y1="8" x2="12.01" y2="8" />
        </svg>
      </button>
      <button
        class="p-1 rounded transition-colors relative {activeTab === 'tasks'
          ? 'text-foreground bg-accent'
          : 'text-muted-foreground/50 hover:text-muted-foreground'}"
        onclick={() => {
          activeTab = "tasks";
          onToggle();
        }}
        title={t("toolActivity_tabTasks")}
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
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M9 12l2 2 4-4" />
        </svg>
        {#if activeBackgroundTasks.length > 0}
          <span class="absolute top-0 right-0 h-1.5 w-1.5 rounded-full bg-blue-400 animate-pulse"
          ></span>
        {/if}
      </button>
      {#if toolStats.totalToolCount > 0}
        <span class="mt-1 text-[10px] text-muted-foreground" style="writing-mode: vertical-rl;"
          >{toolStats.totalToolCount}</span
        >
      {/if}
    </div>
  {/if}
</aside>

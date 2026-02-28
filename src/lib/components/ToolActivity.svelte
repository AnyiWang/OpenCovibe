<script lang="ts">
  import type { HookEvent, ContextSnapshot, SessionInfoData, FileEntry } from "$lib/types";
  import type { TimelineEntry, BusToolItem, TurnUsage } from "$lib/stores/types";
  import { getToolColor } from "$lib/utils/tool-colors";
  import { splitPath } from "$lib/utils/format";
  import { dbg } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import ContextHistoryPanel from "$lib/components/ContextHistoryPanel.svelte";
  import FilesPanel from "$lib/components/FilesPanel.svelte";
  import SessionInfoPanel from "$lib/components/SessionInfoPanel.svelte";
  import {
    extractFilesFromTimeline,
    extractFilesFromHooks,
    extractFilesFromPersisted,
    mergeFileEntries,
  } from "$lib/utils/file-entries";
  import { extractTaskToolMeta, type TaskToolMeta } from "$lib/utils/tool-rendering";

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
    requestedTab = $bindable(null as "tools" | "context" | "files" | "info" | null),
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
    requestedTab?: "tools" | "context" | "files" | "info" | null;
  } = $props();

  // ── Tab state ──
  type SidebarPanel = "tools" | "context" | "files" | "info";
  let activeTab: SidebarPanel = $state("tools");

  // ── External tab request ──
  $effect(() => {
    if (requestedTab) {
      activeTab = requestedTab;
      requestedTab = null;
    }
  });

  // ── Helpers ──

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
    if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
    return String(n);
  }

  function shortPath(v: unknown): string {
    if (!v || typeof v !== "string") return "";
    const parts = splitPath(v);
    return parts.length > 2 ? "\u2026/" + parts.slice(-2).join("/") : v;
  }

  function truncate(s: string, max: number): string {
    return s.length > max ? s.slice(0, max) + "\u2026" : s;
  }

  function getToolDetail(tool: BusToolItem): string {
    const inp = tool.input;
    if (!inp || typeof inp !== "object") return "";
    switch (tool.tool_name) {
      case "Read":
      case "Write":
      case "Edit":
      case "NotebookEdit":
        return shortPath(inp.file_path ?? inp.notebook_path);
      case "Bash":
        return truncate(String(inp.command ?? ""), 50);
      case "Grep":
      case "Glob":
        return truncate(String(inp.pattern ?? ""), 40);
      case "WebFetch":
        return truncate(String(inp.url ?? ""), 50);
      case "WebSearch":
        return truncate(String(inp.query ?? ""), 50);
      case "Task":
        return truncate(String(inp.description ?? inp.prompt ?? ""), 50);
      default: {
        // First string value
        for (const v of Object.values(inp)) {
          if (typeof v === "string" && v.length > 0) return truncate(v, 50);
        }
        return "";
      }
    }
  }

  function getHookDetail(event: HookEvent): string {
    const input = event.tool_input;
    if (!input || typeof input !== "object") return "";
    const inp = input as Record<string, unknown>;
    const name = event.tool_name ?? "";
    switch (name) {
      case "Read":
      case "Write":
      case "Edit":
      case "NotebookEdit":
        return shortPath(inp.file_path ?? inp.path ?? inp.notebook_path);
      case "Bash":
        return truncate(String(inp.command ?? ""), 50);
      case "Grep":
      case "Glob":
        return truncate(String(inp.pattern ?? ""), 40);
      case "WebFetch":
        return truncate(String(inp.url ?? ""), 50);
      case "WebSearch":
        return truncate(String(inp.query ?? ""), 50);
      case "Task":
        return truncate(String(inp.description ?? inp.prompt ?? ""), 50);
      default:
        return (
          truncate(String(inp.path ?? inp.command ?? inp.pattern ?? inp.query ?? ""), 50) || ""
        );
    }
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

  let hasTimelineTools = $derived(timeline.some((e) => e.kind === "tool"));
  let useTimeline = $derived(hasTimelineTools);

  // ── Turn grouping (timeline mode) ──

  interface ToolTurn {
    turnIndex: number;
    userPreview: string;
    tools: ToolNode[];
  }

  let turns = $derived.by(() => {
    if (!useTimeline) return [];
    const result: ToolTurn[] = [];
    let currentTools: ToolNode[] = [];
    let currentPreview = "";
    let turnIdx = 0;
    // Defensive dedup: CLI can emit events with missing parent_tool_use_id,
    // causing the same tool_use_id to appear in both main timeline and a subTimeline.
    // Track seen IDs to prevent each_key_duplicate crashes in {#each} blocks.
    const seen = new Set<string>();

    for (const entry of timeline) {
      if (entry.kind === "separator") continue;
      if (entry.kind === "user") {
        // Flush previous turn
        if (currentTools.length > 0) {
          result.push({ turnIndex: turnIdx, userPreview: currentPreview, tools: currentTools });
        }
        turnIdx++;
        currentPreview = entry.content.slice(0, 40);
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
    if (currentTools.length > 0) {
      result.push({ turnIndex: turnIdx, userPreview: currentPreview, tools: currentTools });
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
  // Template-compatible aliases
  let toolSummary = $derived(toolStats.summary);
  let doneCount = $derived(toolStats.doneCount);
  let runningCount = $derived(toolStats.runningCount);
  let errorCount = $derived(toolStats.errorCount);
  let totalToolCount = $derived(toolStats.totalToolCount);

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
      total: totalToolCount,
      files: fileEntries.length,
    });
  });
</script>

{#snippet statusIcon(category: StatusCategory)}
  {#if category === "done"}
    <svg
      class="h-3 w-3 text-emerald-500 dark:text-emerald-400 shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
      stroke-linecap="round"
      stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
    >
  {:else if category === "error"}
    <svg
      class="h-3 w-3 text-destructive shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg
    >
  {:else if category === "running"}
    <div
      class="h-3 w-3 rounded-full border-2 border-border border-t-muted-foreground animate-spin shrink-0"
    ></div>
  {:else}
    <div class="h-3 w-3 rounded-full bg-muted-foreground/30 shrink-0"></div>
  {/if}
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

{#if !collapsed}
  <div class="flex h-full flex-col border-l border-border bg-muted/30" style="width: 280px;">
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
              <span
                class="absolute -top-0.5 -right-0.5 text-[8px] font-bold leading-none min-w-[14px] h-[14px] flex items-center justify-center rounded-full bg-muted-foreground/20 text-muted-foreground"
              >
                {fileEntries.length > 99 ? "99+" : fileEntries.length}
              </span>
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

    {#if activeTab === "context"}
      <ContextHistoryPanel history={contextHistory} {turnUsages} {sessionInfo} />
    {:else if activeTab === "files"}
      <FilesPanel {fileEntries} {onScrollToTool} />
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
                      class="text-[9px] px-1 py-0.5 rounded bg-cyan-500/15 text-cyan-600 dark:text-cyan-400 font-medium"
                      >{sa.meta.model}</span
                    >
                  {/if}
                  <span class="ml-auto">
                    {#if isDone}
                      <svg
                        class="h-3 w-3 text-emerald-500"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                      >
                    {:else if isError}
                      <svg
                        class="h-3 w-3 text-destructive"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><line x1="18" y1="6" x2="6" y2="18" /><line
                          x1="6"
                          y1="6"
                          x2="18"
                          y2="18"
                        /></svg
                      >
                    {:else if isRunning}
                      <div
                        class="h-3 w-3 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                      ></div>
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
                    {#if sa.durationMs != null}{(sa.durationMs / 1000).toFixed(1)}s{/if}
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
      {#if toolSummary.length > 1}
        <div class="flex flex-wrap gap-1 px-2.5 py-1.5 border-b border-border/50">
          {#each toolSummary as [name, count]}
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
        {#if totalToolCount === 0}
          <div class="flex items-center justify-center h-32 text-xs text-muted-foreground/50">
            {t("toolActivity_noToolCalls")}
          </div>
        {:else if useTimeline}
          <!-- Timeline mode: grouped by turn -->
          {#each turns as turn (turn.turnIndex)}
            {@const isCollapsed = collapsedTurns.has(turn.turnIndex)}
            {@const tu = usageByTurn.get(turn.turnIndex)}
            <!-- Turn header -->
            <button
              class="w-full text-left px-2.5 py-1.5 hover:bg-accent/50 transition-colors border-b border-border/30"
              onclick={() => toggleTurn(turn.turnIndex)}
            >
              <div class="flex items-center gap-1.5">
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
                <span class="text-[11px] font-medium text-muted-foreground">
                  {#if turn.userPreview}
                    {t("toolActivity_turn", { index: String(turn.turnIndex) })}
                    <span class="text-foreground/70">{truncate(turn.userPreview, 25)}</span>
                  {:else}
                    <span class="text-muted-foreground/60">{t("toolActivity_systemResume")}</span>
                  {/if}
                </span>
                <span class="ml-auto flex items-center gap-1.5">
                  {#if tu}
                    <span class="text-[10px] text-muted-foreground/50"
                      >{formatTokens(tu.inputTokens + tu.outputTokens)}</span
                    >
                  {/if}
                  <span
                    class="text-[10px] px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground font-medium"
                    >{countToolNodes(turn.tools)}</span
                  >
                </span>
              </div>
            </button>

            <!-- Tools in this turn -->
            {#if !isCollapsed}
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
      {#if totalToolCount > 0}
        <div class="border-t border-border px-3 py-1.5">
          <div class="flex items-center gap-3 text-[11px]">
            {#if doneCount > 0}
              <span class="flex items-center gap-1 text-emerald-500 dark:text-emerald-400">
                <svg
                  class="h-3 w-3"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                >
                {doneCount}
              </span>
            {/if}
            {#if runningCount > 0}
              <span class="flex items-center gap-1 text-muted-foreground">
                <div
                  class="h-3 w-3 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
                ></div>
                {runningCount}
              </span>
            {/if}
            {#if errorCount > 0}
              <span class="flex items-center gap-1 text-destructive">
                <svg
                  class="h-3 w-3"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  ><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg
                >
                {errorCount}
              </span>
            {/if}
          </div>
        </div>
      {/if}
    {/if}
  </div>
{:else}
  <!-- Collapsed: thin bar with 4 icon buttons vertically -->
  <div
    class="flex flex-col items-center border-l border-border bg-muted/30 py-2 px-1 gap-1"
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
    {#if totalToolCount > 0}
      <span class="mt-1 text-[10px] text-muted-foreground/50" style="writing-mode: vertical-rl;"
        >{totalToolCount}</span
      >
    {/if}
  </div>
{/if}

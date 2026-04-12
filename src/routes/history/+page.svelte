<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { searchRuns } from "$lib/api";
  import type { RunSearchFilters, RunSearchResponse } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { formatCostDisplay } from "$lib/utils/format";

  let filters = $state<RunSearchFilters>({});
  let response = $state<RunSearchResponse | null>(null);
  let loading = $state(true);
  let error = $state("");
  let showAdvancedFilters = $state(false);
  let requestId = 0;
  let searchInput = $state("");
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  // Active status filter (quick pills)
  let activeStatusFilter = $state<string>("all");
  let stableTools = $state<{ value: string; count: number }[]>([]);

  const PAGE_SIZE = 50;

  // Derived display values
  let totalCostDisplay = $derived.by(() => {
    if (!response) return "$0.00";
    return `$${response.facets.totalCost.toFixed(2)}`;
  });

  function projectDisplayName(cwd: string): string {
    const parts = cwd.replace(/\\/g, "/").split("/");
    return parts[parts.length - 1] || cwd;
  }

  function formatRelativeTime(iso: string): string {
    const now = Date.now();
    const then = new Date(iso).getTime();
    const diff = now - then;
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    if (days < 30) return `${days}d ago`;
    return new Date(iso).toLocaleDateString();
  }

  const formatCost = formatCostDisplay;

  function statusColor(status: string): string {
    switch (status) {
      case "completed":
        return "bg-green-500";
      case "failed":
        return "bg-red-500";
      case "stopped":
        return "bg-yellow-500";
      case "running":
        return "bg-blue-500";
      case "idle":
        return "bg-emerald-500";
      default:
        return "bg-gray-400";
    }
  }

  async function loadData(append = false) {
    const id = ++requestId;
    if (!append) loading = true;
    error = "";

    try {
      // Build API request from user-intent filters — never write back to `filters`
      const requestFilters: RunSearchFilters = {
        ...filters,
        limit: PAGE_SIZE,
        offset: append && response ? response.results.length : 0,
        // Explicitly set or clear statuses based on pill selection
        statuses:
          activeStatusFilter !== "all"
            ? [
                activeStatusFilter as
                  | "completed"
                  | "failed"
                  | "stopped"
                  | "running"
                  | "pending"
                  | "idle",
              ]
            : undefined,
      };

      dbg("history", "loadData", requestFilters);
      const res = await searchRuns(requestFilters);

      if (id !== requestId) return; // stale

      if (append && response) {
        response = {
          ...res,
          results: [...response.results, ...res.results],
        };
      } else {
        response = res;
      }

      // Capture stable tool order on first load (facets are computed from ALL entries)
      if (stableTools.length === 0 && res.facets?.tools?.length) {
        stableTools = res.facets.tools;
      }
    } catch (e) {
      if (id !== requestId) return;
      error = String(e);
      dbgWarn("history", "loadData error", e);
    } finally {
      if (id === requestId) loading = false;
    }
  }

  function onSearchInput() {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      filters = { ...filters, query: searchInput || undefined };
      loadData();
    }, 300);
  }

  function onStatusFilter(status: string) {
    activeStatusFilter = status;
    loadData();
  }

  function onSortChange(field: "date" | "cost" | "tokens" | "turns") {
    if (filters.sortBy === field) {
      filters = { ...filters, sortAsc: !(filters.sortAsc ?? false) };
    } else {
      filters = { ...filters, sortBy: field, sortAsc: false };
    }
    loadData();
  }

  function onProjectFilter(project: string | undefined) {
    filters = { ...filters, projects: project ? [project] : undefined };
    loadData();
  }

  function onAgentFilter(agent: string | undefined) {
    filters = { ...filters, agents: agent ? [agent] : undefined };
    loadData();
  }

  function onToolToggle(tool: string) {
    const current = filters.tools ?? [];
    const next = current.includes(tool) ? current.filter((t) => t !== tool) : [...current, tool];
    filters = { ...filters, tools: next.length > 0 ? next : undefined };
    loadData();
  }

  let activeDateRange = $state<string>("all");

  function onDateRange(range: string) {
    activeDateRange = range;
    if (range === "all") {
      filters = { ...filters, dateFrom: undefined, dateTo: undefined };
    } else {
      const now = new Date();
      const to = now.toISOString().slice(0, 10);
      let from: string;
      if (range === "today") {
        from = to;
      } else if (range === "7d") {
        const d = new Date(now);
        d.setDate(d.getDate() - 6);
        from = d.toISOString().slice(0, 10);
      } else if (range === "30d") {
        const d = new Date(now);
        d.setDate(d.getDate() - 29);
        from = d.toISOString().slice(0, 10);
      } else {
        // 90d
        const d = new Date(now);
        d.setDate(d.getDate() - 89);
        from = d.toISOString().slice(0, 10);
      }
      filters = { ...filters, dateFrom: from, dateTo: to };
    }
    loadData();
  }

  function onCostChange(field: "costMin" | "costMax", value: string) {
    const num = value ? parseFloat(value) : undefined;
    filters = { ...filters, [field]: num };
    loadData();
  }

  function clearFilters() {
    filters = {};
    searchInput = "";
    activeStatusFilter = "all";
    activeDateRange = "all";
    stableTools = [];
    showAdvancedFilters = false;
    loadData();
  }

  function goToRun(runId: string) {
    goto(`/chat?run=${runId}`);
  }

  onMount(() => {
    // Read initial query from URL
    const q = $page.url.searchParams.get("q");
    if (q) {
      searchInput = q;
      filters = { query: q };
    }
    loadData();
  });
</script>

<div class="flex h-full flex-col overflow-hidden">
  <!-- Header -->
  <div class="shrink-0 border-b border-border px-6 py-4">
    <h1 class="text-xl font-semibold text-foreground">{t("history_title")}</h1>
    <p class="mt-1 text-sm text-muted-foreground">{t("history_subtitle")}</p>
  </div>

  <div class="flex-1 overflow-y-auto px-6 py-4">
    <!-- Search + Filter toggle -->
    <div class="mb-4 flex items-center gap-3">
      <div class="relative flex-1">
        <svg
          class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" />
        </svg>
        <input
          type="text"
          bind:value={searchInput}
          oninput={onSearchInput}
          placeholder={t("history_searchPlaceholder")}
          class="w-full rounded-lg border border-border bg-background py-2 pl-10 pr-4 text-sm text-foreground placeholder:text-muted-foreground focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary"
        />
      </div>
      <button
        onclick={() => (showAdvancedFilters = !showAdvancedFilters)}
        class="flex items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-sm transition-colors {showAdvancedFilters
          ? 'bg-primary/10 text-primary'
          : 'text-muted-foreground hover:bg-muted/50'}"
      >
        <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M22 3H2l8 9.46V19l4 2v-8.54L22 3z" />
        </svg>
        {showAdvancedFilters ? t("history_filtersHide") : t("history_filters")}
      </button>
    </div>

    <!-- Status pills -->
    <div class="mb-4 flex flex-wrap gap-2">
      {#each [{ key: "all", label: t("history_allStatuses") }, { key: "completed", label: t("history_statusCompleted") }, { key: "failed", label: t("history_statusFailed") }, { key: "stopped", label: t("history_statusStopped") }, { key: "running", label: t("history_statusRunning") }, { key: "idle", label: t("history_statusDone") }] as pill}
        <button
          onclick={() => onStatusFilter(pill.key)}
          class="rounded-full px-3 py-1 text-xs font-medium transition-colors {activeStatusFilter ===
          pill.key
            ? 'bg-primary text-primary-foreground'
            : 'bg-muted text-muted-foreground hover:bg-muted/80'}"
        >
          {pill.label}
        </button>
      {/each}
    </div>

    <!-- Advanced filters (collapsible) -->
    {#if showAdvancedFilters}
      <div class="mb-4 rounded-lg border border-border bg-muted/20 p-4">
        <!-- Row 1: Dropdowns + Date range -->
        <div class="grid grid-cols-4 gap-3">
          <!-- Project -->
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted-foreground"
              >{t("history_project")}</label
            >
            <div class="relative">
              <select
                onchange={(e) => onProjectFilter(e.currentTarget.value || undefined)}
                class="h-8 w-full appearance-none rounded-md border border-border bg-background px-2.5 pr-7 text-[13px] text-foreground transition-colors hover:border-muted-foreground/40 focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
              >
                <option value="">{t("history_allProjects")}</option>
                {#if response?.facets}
                  {#each response.facets.projects as p}
                    <option value={p.value}>{projectDisplayName(p.value)} ({p.count})</option>
                  {/each}
                {/if}
              </select>
              <svg
                class="pointer-events-none absolute right-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
              >
            </div>
          </div>

          <!-- Agent -->
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted-foreground"
              >{t("history_agent")}</label
            >
            <div class="relative">
              <select
                onchange={(e) => onAgentFilter(e.currentTarget.value || undefined)}
                class="h-8 w-full appearance-none rounded-md border border-border bg-background px-2.5 pr-7 text-[13px] text-foreground transition-colors hover:border-muted-foreground/40 focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
              >
                <option value="">{t("history_allAgents")}</option>
                {#if response?.facets}
                  {#each response.facets.agents as a}
                    <option value={a.value}>{a.value} ({a.count})</option>
                  {/each}
                {/if}
              </select>
              <svg
                class="pointer-events-none absolute right-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
              >
            </div>
          </div>

          <!-- Date range presets -->
          <div class="col-span-2">
            <label class="mb-1.5 block text-xs font-medium text-muted-foreground"
              >{t("history_dateRange")}</label
            >
            <div class="flex gap-1">
              {#each [{ key: "all", label: t("history_dateAll") }, { key: "today", label: t("history_dateToday") }, { key: "7d", label: t("history_date7d") }, { key: "30d", label: t("history_date30d") }, { key: "90d", label: t("history_date90d") }] as opt}
                <button
                  onclick={() => onDateRange(opt.key)}
                  class="h-8 rounded-md px-3 text-[13px] transition-colors {activeDateRange ===
                  opt.key
                    ? 'bg-foreground/10 text-foreground font-medium'
                    : 'text-muted-foreground hover:bg-muted hover:text-foreground'}"
                >
                  {opt.label}
                </button>
              {/each}
            </div>
          </div>
        </div>

        <!-- Row 2: Cost range -->
        <div class="mt-3 grid grid-cols-4 gap-3">
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted-foreground"
              >{t("history_costMin")}</label
            >
            <div class="relative">
              <span
                class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-[13px] text-muted-foreground"
                >$</span
              >
              <input
                type="number"
                step="0.01"
                min="0"
                placeholder="0.00"
                onchange={(e) => onCostChange("costMin", e.currentTarget.value)}
                class="h-8 w-full rounded-md border border-border bg-background pl-6 pr-2.5 text-[13px] text-foreground transition-colors hover:border-muted-foreground/40 focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary/30 [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
              />
            </div>
          </div>
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted-foreground"
              >{t("history_costMax")}</label
            >
            <div class="relative">
              <span
                class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-[13px] text-muted-foreground"
                >$</span
              >
              <input
                type="number"
                step="0.01"
                min="0"
                placeholder="∞"
                onchange={(e) => onCostChange("costMax", e.currentTarget.value)}
                class="h-8 w-full rounded-md border border-border bg-background pl-6 pr-2.5 text-[13px] text-foreground transition-colors hover:border-muted-foreground/40 focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary/30 [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
              />
            </div>
          </div>
          <!-- Clear filters button -->
          <div class="col-span-2 flex items-end justify-end">
            <button
              onclick={clearFilters}
              class="rounded-md px-3 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            >
              {t("history_clearFilters")}
            </button>
          </div>
        </div>

        <!-- Row 3: Tool chips -->
        {#if stableTools.length}
          <div class="mt-3 border-t border-border/50 pt-3">
            <label class="mb-2 block text-xs font-medium text-muted-foreground"
              >{t("history_tools")}</label
            >
            <div class="flex flex-wrap gap-1.5">
              {#each stableTools as tool (tool.value)}
                <button
                  onclick={() => onToolToggle(tool.value)}
                  class="rounded-md px-2 py-1 text-xs transition-colors {filters.tools?.includes(
                    tool.value,
                  )
                    ? 'bg-primary/15 text-primary border border-primary/30 font-medium'
                    : 'bg-muted/50 text-muted-foreground border border-transparent hover:bg-muted hover:text-foreground'}"
                >
                  {tool.value}
                  <span class="ml-0.5 text-muted-foreground/70">{tool.count}</span>
                </button>
              {/each}
            </div>
          </div>
        {/if}
      </div>
    {/if}

    <!-- Summary bar (always visible once we have data; subtle opacity during reload) -->
    {#if response}
      <div
        class="mb-3 flex items-center justify-between text-sm text-muted-foreground transition-opacity"
        class:opacity-50={loading}
      >
        <span>
          {t("history_runsMatching", { count: String(response.totalMatching) })} · {totalCostDisplay}
          {t("history_totalCost")}
        </span>

        <!-- Sort buttons -->
        <div class="flex items-center gap-1">
          {#each [{ key: "date", label: t("history_sortDate") }, { key: "cost", label: t("history_sortCost") }, { key: "tokens", label: t("history_sortTokens") }, { key: "turns", label: t("history_sortTurns") }] as sortOpt}
            <button
              onclick={() => onSortChange(sortOpt.key as "date" | "cost" | "tokens" | "turns")}
              class="rounded px-2 py-0.5 text-xs transition-colors {filters.sortBy ===
                sortOpt.key ||
              (!filters.sortBy && sortOpt.key === 'date')
                ? 'bg-muted text-foreground'
                : 'hover:bg-muted/50'}"
            >
              {sortOpt.label}
              {#if filters.sortBy === sortOpt.key || (!filters.sortBy && sortOpt.key === "date")}
                <span class="ml-0.5">{filters.sortAsc ? "\u2191" : "\u2193"}</span>
              {/if}
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Initial loading spinner (only when no data yet) -->
    {#if loading && !response}
      <div class="flex items-center justify-center py-20">
        <div
          class="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent"
        ></div>
      </div>
    {:else if error}
      <div class="rounded-lg border border-red-500/20 bg-red-500/10 p-4 text-sm text-red-400">
        {error}
      </div>
    {:else if response && response.results.length === 0 && !loading}
      <div class="flex flex-col items-center justify-center py-20 text-muted-foreground">
        <svg
          class="mb-3 h-12 w-12 opacity-30"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
        >
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" />
        </svg>
        <p class="text-sm">{t("history_noResults")}</p>
      </div>
    {:else if response}
      <!-- Run cards (subtle opacity during reload to avoid layout jump) -->
      <div class="space-y-2 transition-opacity" class:opacity-50={loading}>
        {#each response.results as run}
          <button
            onclick={() => goToRun(run.runId)}
            class="w-full rounded-lg border border-border bg-card p-4 text-left transition-colors hover:bg-muted/30"
          >
            <div class="flex items-start justify-between gap-3">
              <!-- Left side -->
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <span class="h-2 w-2 shrink-0 rounded-full {statusColor(run.status)}"></span>
                  <span class="truncate text-sm font-medium text-foreground">
                    {run.name || run.promptPreview || "Untitled"}
                  </span>
                  {#if run.hasErrors}
                    <span
                      class="rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] font-medium text-red-400"
                    >
                      {t("history_errors")}
                    </span>
                  {/if}
                </div>
                <div class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                  <span>{projectDisplayName(run.cwd)}</span>
                  <span>·</span>
                  <span>{formatRelativeTime(run.startedAt)}</span>
                  {#if run.model}
                    <span>·</span>
                    <span>{run.model}</span>
                  {/if}
                </div>
                <!-- Tool chips -->
                {#if run.toolsUsed.length > 0}
                  <div class="mt-2 flex flex-wrap gap-1">
                    {#each run.toolsUsed.slice(0, 6) as tool}
                      <span
                        class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                      >
                        {tool}
                      </span>
                    {/each}
                    {#if run.toolsUsed.length > 6}
                      <span
                        class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                      >
                        +{run.toolsUsed.length - 6}
                      </span>
                    {/if}
                  </div>
                {/if}
              </div>

              <!-- Right side -->
              <div class="shrink-0 text-right">
                <div class="text-sm font-medium text-foreground">
                  {formatCost(run.totalCostUsd)}
                </div>
                <div class="mt-0.5 text-xs text-muted-foreground">
                  {t("history_turns", { count: String(run.numTurns) })}
                </div>
                {#if run.filesTouchedCount > 0}
                  <div class="text-xs text-muted-foreground">
                    {t("history_files", { count: String(run.filesTouchedCount) })}
                  </div>
                {/if}
              </div>
            </div>
          </button>
        {/each}
      </div>

      <!-- Load more -->
      {#if response.results.length < response.totalMatching}
        <div class="mt-4 flex justify-center">
          <button
            onclick={() => loadData(true)}
            class="rounded-lg border border-border px-4 py-2 text-sm text-muted-foreground hover:bg-muted/50 transition-colors"
            disabled={loading}
          >
            {#if loading}
              <span
                class="inline-block h-4 w-4 animate-spin rounded-full border-2 border-primary border-t-transparent"
              ></span>
            {:else}
              {t("history_loadMore")}
            {/if}
          </button>
        </div>
      {/if}
    {/if}
  </div>
</div>

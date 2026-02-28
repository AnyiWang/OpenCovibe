<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { fmtRelative } from "$lib/i18n/format";
  import { cwdDisplayLabel } from "$lib/utils/format";
  import type { CliSessionSummary, ImportResult, SyncResult } from "$lib/types";

  let {
    cwd,
    onclose,
    onimported,
  }: {
    cwd: string;
    onclose: () => void;
    onimported: (runId: string) => void;
  } = $props();

  let sessions: CliSessionSummary[] = $state([]);
  let loading = $state(true);
  let searchQuery = $state("");
  let importingId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let warning = $state<string | null>(null);
  let importingAll = $state(false);

  // ── Project filter ──
  const isShowAll = $derived(!cwd || cwd === "/");
  let selectedProject = $state<string | null>(null); // null = all

  const projects = $derived.by(() => {
    const cwdMap = new Map<string, number>();
    for (const s of sessions) {
      if (s.cwd) {
        cwdMap.set(s.cwd, (cwdMap.get(s.cwd) ?? 0) + 1);
      }
    }
    return Array.from(cwdMap.entries())
      .sort((a, b) => b[1] - a[1])
      .map(([path, count]) => ({ path, label: cwdDisplayLabel(path), count }));
  });

  const filtered = $derived.by(() => {
    let list = sessions;
    // Project filter
    if (selectedProject) {
      list = list.filter((s) => s.cwd === selectedProject);
    }
    // Search filter
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      list = list.filter(
        (s) =>
          s.firstPrompt.toLowerCase().includes(q) ||
          (s.model ?? "").toLowerCase().includes(q) ||
          (s.cwd ?? "").toLowerCase().includes(q),
      );
    }
    return list;
  });

  const newCount = $derived(filtered.filter((s) => !s.alreadyImported).length);

  /** Effective cwd for import — use the session's own cwd */
  function importCwd(session: CliSessionSummary): string {
    return session.cwd || cwd;
  }

  // ── Load sessions on mount ──

  $effect(() => {
    discoverSessions();
  });

  async function discoverSessions() {
    loading = true;
    error = null;
    dbg("cli-browser", "discovering sessions", { cwd });
    try {
      sessions = await invoke<CliSessionSummary[]>("discover_cli_sessions", { cwd });
      dbg("cli-browser", "discovered", { count: sessions.length });
    } catch (e) {
      const msg = String(e);
      dbgWarn("cli-browser", "discover failed", msg);
      error = msg;
    } finally {
      loading = false;
    }
  }

  async function importSession(session: CliSessionSummary) {
    if (importingId) return;
    importingId = session.sessionId;
    error = null;
    warning = null;
    const sessionCwd = importCwd(session);
    dbg("cli-browser", "importing session", { sessionId: session.sessionId, cwd: sessionCwd });
    try {
      const result = await invoke<ImportResult>("import_cli_session", {
        sessionId: session.sessionId,
        cwd: sessionCwd,
      });
      dbg("cli-browser", "import success", { runId: result.runId, events: result.eventsImported });
      if (result.usageIncomplete) {
        warning = t("cliSync_usageIncomplete");
      }
      await discoverSessions();
      onimported(result.runId);
    } catch (e) {
      const msg = String(e);
      dbgWarn("cli-browser", "import failed", msg);
      error = msg;
    } finally {
      importingId = null;
    }
  }

  async function syncSession(runId: string) {
    importingId = runId;
    error = null;
    warning = null;
    dbg("cli-browser", "syncing session", { runId });
    try {
      const result = await invoke<SyncResult>("sync_cli_session", { runId });
      dbg("cli-browser", "sync success", { newEvents: result.newEvents });
      if (result.usageIncomplete) {
        warning = t("cliSync_usageIncomplete");
      }
      await discoverSessions();
    } catch (e) {
      const msg = String(e);
      dbgWarn("cli-browser", "sync failed", msg);
      error = msg;
    } finally {
      importingId = null;
    }
  }

  async function importAllNew() {
    const newSessions = filtered.filter((s) => !s.alreadyImported);
    if (newSessions.length === 0) return;
    importingAll = true;
    error = null;
    warning = null;
    dbg("cli-browser", "importing all new", { count: newSessions.length });
    let lastRunId: string | null = null;
    let importedCount = 0;
    try {
      for (const s of newSessions) {
        importingId = s.sessionId;
        const sessionCwd = importCwd(s);
        const result = await invoke<ImportResult>("import_cli_session", {
          sessionId: s.sessionId,
          cwd: sessionCwd,
        });
        dbg("cli-browser", "imported", { sessionId: s.sessionId, runId: result.runId });
        lastRunId = result.runId;
        importedCount++;
        if (result.usageIncomplete) {
          warning = t("cliSync_usageIncomplete");
        }
      }
      await discoverSessions();
      if (lastRunId) {
        dbg("cli-browser", "import-all done, navigating", { importedCount, lastRunId });
        onimported(lastRunId);
      }
    } catch (e) {
      const msg = String(e);
      dbgWarn("cli-browser", "import-all failed", msg);
      error = msg;
      await discoverSessions().catch(() => {});
    } finally {
      importingId = null;
      importingAll = false;
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onclose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm animate-fade-in"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={handleKeydown}
>
  <div
    class="relative flex max-h-[80vh] w-full max-w-2xl flex-col rounded-xl border border-border bg-background shadow-2xl animate-slide-up"
  >
    <!-- Header -->
    <div class="border-b border-border px-6 py-4">
      <div class="flex items-center justify-between">
        <div>
          <h2 class="text-base font-semibold text-foreground">{t("cliSync_title")}</h2>
          <p class="mt-0.5 text-xs text-muted-foreground">
            {#if isShowAll}
              {t("cliSync_allProjects")} &middot; {t("cliSync_found", {
                count: String(sessions.length),
              })}
            {:else}
              {cwd} &middot; {t("cliSync_found", { count: String(sessions.length) })}
            {/if}
          </p>
        </div>
        <button
          class="rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          onclick={onclose}
          aria-label="Close"
        >
          <svg
            class="h-5 w-5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="M18 6 6 18M6 6l12 12" /></svg
          >
        </button>
      </div>

      <!-- Project filter (inline in header) -->
      {#if isShowAll && projects.length > 1}
        <div class="mt-3 flex items-center gap-1.5 overflow-x-auto">
          <button
            class="shrink-0 rounded-md px-2.5 py-1 text-xs font-medium transition-colors
              {selectedProject === null
              ? 'bg-accent text-accent-foreground'
              : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'}"
            onclick={() => (selectedProject = null)}
          >
            {t("cliSync_filterAll")} ({sessions.length})
          </button>
          {#each projects as proj (proj.path)}
            <button
              class="shrink-0 rounded-md px-2.5 py-1 text-xs font-medium transition-colors
                {selectedProject === proj.path
                ? 'bg-accent text-accent-foreground'
                : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'}"
              onclick={() => (selectedProject = proj.path)}
              title={proj.path}
            >
              {proj.label} ({proj.count})
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Search -->
    <div class="border-b border-border px-6 py-3">
      <div class="relative">
        <svg
          class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" />
        </svg>
        <input
          type="text"
          bind:value={searchQuery}
          placeholder={t("cliSync_searchPlaceholder")}
          class="w-full rounded-lg border border-border bg-muted/50 py-2 pl-10 pr-3 text-sm text-foreground placeholder:text-muted-foreground focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary"
        />
      </div>
    </div>

    <!-- Session list -->
    <div class="flex-1 overflow-y-auto px-6 py-3">
      {#if loading}
        <div class="flex items-center justify-center py-12">
          <div
            class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
          ></div>
        </div>
      {:else if error && sessions.length === 0}
        <div class="flex flex-col items-center gap-2 py-12 text-center">
          <svg
            class="h-8 w-8 text-destructive/60"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
          >
            <circle cx="12" cy="12" r="10" />
            <line x1="12" y1="8" x2="12" y2="12" />
            <line x1="12" y1="16" x2="12.01" y2="16" />
          </svg>
          <p class="text-sm text-destructive">{error}</p>
        </div>
      {:else if filtered.length === 0}
        <div class="flex flex-col items-center gap-2 py-12 text-center">
          <svg
            class="h-8 w-8 text-muted-foreground/40"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
          >
            <path
              d="M3 7v10a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-6l-2-2H5a2 2 0 0 0-2 2z"
            />
          </svg>
          <p class="text-sm text-muted-foreground">{t("cliSync_noSessions")}</p>
        </div>
      {:else}
        <div class="space-y-2">
          {#each filtered as session (session.sessionId)}
            {@const isImporting = importingId === session.sessionId}
            {@const isImported = session.alreadyImported}
            <div
              class="group rounded-lg border border-border p-3 transition-colors hover:bg-muted/30"
            >
              <div class="flex items-start justify-between gap-3">
                <!-- Left: status dot + time + prompt -->
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <span
                      class="inline-block h-2 w-2 shrink-0 rounded-full {isImported
                        ? 'bg-emerald-500'
                        : 'bg-blue-500'}"
                    ></span>
                    <span class="text-xs text-muted-foreground shrink-0">
                      {fmtRelative(session.lastActivityAt)}
                    </span>
                    {#if isShowAll && !selectedProject && session.cwd}
                      <span
                        class="shrink-0 truncate max-w-[140px] rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
                        title={session.cwd}
                      >
                        {cwdDisplayLabel(session.cwd)}
                      </span>
                    {/if}
                    {#if session.model}
                      <span
                        class="ml-auto shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
                      >
                        {session.model}
                      </span>
                    {/if}
                  </div>
                  <p class="mt-1 truncate text-sm font-medium text-foreground">
                    {session.firstPrompt || "\u2014"}
                  </p>
                  <div class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                    <span>{t("cliSync_messages", { count: String(session.messageCount) })}</span>
                    <span>&middot;</span>
                    <span>{formatSize(session.fileSize)}</span>
                    {#if session.hasSubagents}
                      <span>&middot;</span>
                      <span>{t("cliSync_subagents")}</span>
                    {/if}
                    {#if isImported && session.existingRunId}
                      <span>&middot;</span>
                      <span class="text-emerald-600 dark:text-emerald-400">
                        {t("cliSync_alreadyImported")}
                      </span>
                    {/if}
                  </div>
                </div>

                <!-- Right: action buttons -->
                <div class="flex items-center gap-1.5 shrink-0 pt-0.5">
                  {#if isImported && session.existingRunId}
                    <button
                      class="rounded-md border border-border px-2.5 py-1 text-xs font-medium text-foreground hover:bg-accent transition-colors disabled:opacity-50"
                      onclick={() => syncSession(session.existingRunId!)}
                      disabled={!!importingId}
                    >
                      {#if importingId === session.existingRunId}
                        <span
                          class="inline-block h-3 w-3 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
                        ></span>
                      {:else}
                        {t("cliSync_sync")}
                      {/if}
                    </button>
                    <button
                      class="rounded-md border border-border px-2.5 py-1 text-xs font-medium text-foreground hover:bg-accent transition-colors"
                      onclick={() => onimported(session.existingRunId!)}
                    >
                      {t("cliSync_open")}
                    </button>
                  {:else}
                    <button
                      class="rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
                      onclick={() => importSession(session)}
                      disabled={!!importingId}
                    >
                      {#if isImporting}
                        <span
                          class="inline-block h-3 w-3 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"
                        ></span>
                      {:else}
                        {t("cliSync_import")}
                      {/if}
                    </button>
                  {/if}
                </div>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Footer -->
    {#if !loading && filtered.length > 0}
      <div class="flex items-center justify-between border-t border-border px-6 py-3">
        {#if error}
          <p class="text-xs text-destructive truncate max-w-[60%]">{error}</p>
        {:else if warning}
          <p class="text-xs text-yellow-600 dark:text-yellow-400 truncate max-w-[60%]">{warning}</p>
        {:else}
          <div></div>
        {/if}
        {#if newCount > 0}
          <button
            class="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
            onclick={importAllNew}
            disabled={!!importingId || importingAll}
          >
            {#if importingAll}
              <span class="flex items-center gap-2">
                <span
                  class="inline-block h-3.5 w-3.5 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"
                ></span>
                {t("cliSync_importing")}
              </span>
            {:else}
              {t("cliSync_importAll", { count: String(newCount) })}
            {/if}
          </button>
        {/if}
      </div>
    {/if}
  </div>
</div>

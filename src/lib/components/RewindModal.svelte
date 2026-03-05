<script lang="ts">
  import { untrack } from "svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { fmtTime } from "$lib/i18n/format";
  import * as api from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { truncate } from "$lib/utils/format";
  import {
    type RewindCandidate,
    type RewindDryRunResult,
    parseDryRunResult,
    parseExecuteResult,
    isDryRunUnsupported,
    isFilesParamUnsupported,
  } from "$lib/utils/rewind";
  import Modal from "./Modal.svelte";

  let {
    open = $bindable(false),
    runId = "",
    candidates = [] as RewindCandidate[],
    initialCandidate = null as RewindCandidate | null,
    onSuccess,
  }: {
    open: boolean;
    runId: string;
    candidates: RewindCandidate[];
    initialCandidate?: RewindCandidate | null;
    onSuccess?: (info: {
      runId: string;
      targetContent: string;
      targetUuid: string;
      filesReverted: string[];
      degraded: boolean;
    }) => void;
  } = $props();

  // ── Internal state ──
  let phase = $state<"select" | "preview" | "executing">("select");
  let selected = $state<RewindCandidate | null>(null);
  let dryRunLoading = $state(false);
  let dryRunResult = $state<RewindDryRunResult | null>(null);
  let dryRunSkipped = $state(false); // true = CLI doesn't support dry_run, allow execute without preview
  let executeError = $state<string | null>(null);
  let requestSeq = $state(0); // race-condition guard: incrementing sequence number
  let selectedFiles = $state<Set<string>>(new Set());
  let lastAutoSelectKey = ""; // one-shot guard: prevent initialCandidate prop bounce
  let hasFiles = $derived((dryRunResult?.filesChanged?.length ?? 0) > 0);

  // ── Reset on close ──
  $effect(() => {
    if (!open) {
      requestSeq = untrack(() => requestSeq) + 1; // invalidate in-flight requests (untrack to avoid re-triggering this effect)
      phase = "select";
      selected = null;
      dryRunLoading = false;
      dryRunResult = null;
      dryRunSkipped = false;
      executeError = null;
      selectedFiles = new Set();
      lastAutoSelectKey = "";
    }
  });

  // ── Auto-select from initialCandidate (one-shot) ──
  $effect(() => {
    if (open && initialCandidate) {
      const key = initialCandidate.cliUuid;
      if (key !== lastAutoSelectKey) {
        lastAutoSelectKey = key;
        selectCheckpoint(initialCandidate);
      }
    }
  });

  // ── Select a checkpoint → dryRun preview ──
  async function selectCheckpoint(c: RewindCandidate) {
    const seq = ++requestSeq;
    selected = c;
    phase = "preview";
    dryRunLoading = true;
    dryRunResult = null;
    dryRunSkipped = false;
    executeError = null;
    selectedFiles = new Set();
    dbg("rewind-modal", "selectCheckpoint", { uuid: c.cliUuid, seq });

    try {
      const raw = await api.rewindFiles(runId, { userMessageId: c.cliUuid, dryRun: true });
      if (seq !== requestSeq || !open) return; // stale or modal closed
      dbg("rewind-modal", "dryRun response", { raw });

      const result = parseDryRunResult(raw);
      // Check if CLI doesn't support dry_run (resolve path — returns subtype:"error" without throwing)
      if (!result.canRewind && result.error && isDryRunUnsupported(result.error)) {
        dbg("rewind-modal", "dryRun unsupported (resolve path), allowing skip", {
          error: result.error,
        });
        dryRunSkipped = true;
      } else {
        dryRunResult = result;
        // Initialize selectedFiles with all files checked
        if (result.canRewind && result.filesChanged) {
          selectedFiles = new Set(result.filesChanged);
        }
      }
    } catch (e) {
      if (seq !== requestSeq || !open) return; // stale or modal closed
      // Distinguish "CLI doesn't support dry_run" (exception path) vs hard failure
      if (isDryRunUnsupported(e)) {
        dbg("rewind-modal", "dryRun unsupported (exception path), allowing skip");
        dryRunSkipped = true;
      } else {
        dbgWarn("rewind-modal", "dryRun hard failure", e);
        dryRunResult = { canRewind: false, error: String(e) };
      }
    } finally {
      if (seq === requestSeq) dryRunLoading = false;
    }
  }

  // ── Execute rewind ──
  async function executeRewind() {
    if (!selected) return;
    const seq = ++requestSeq;
    // Freeze current values to prevent async prop drift
    const runIdAtExec = runId;
    const selectedAtExec = selected;
    phase = "executing";
    executeError = null;
    let degradedToFull = false;

    const allFiles = dryRunResult?.filesChanged;
    const isSelective = allFiles && selectedFiles.size < allFiles.length && selectedFiles.size > 0;
    let filesToRewind = isSelective ? [...selectedFiles] : undefined;

    dbg("rewind-modal", "executeRewind", {
      uuid: selectedAtExec.cliUuid,
      runId: runIdAtExec,
      selective: isSelective,
      fileCount: filesToRewind?.length,
    });

    try {
      let raw = await api.rewindFiles(runIdAtExec, {
        userMessageId: selectedAtExec.cliUuid,
        files: filesToRewind,
      });
      if (seq !== requestSeq || !open) return;

      let result = parseExecuteResult(raw);

      // Degrade: if files param failed with "files unsupported", retry without files
      if (
        !result.canRewind &&
        filesToRewind &&
        result.error &&
        isFilesParamUnsupported(result.error)
      ) {
        dbg("rewind-modal", "files param unsupported, degrading to full rewind");
        raw = await api.rewindFiles(runIdAtExec, {
          userMessageId: selectedAtExec.cliUuid,
        });
        if (seq !== requestSeq || !open) return;
        result = parseExecuteResult(raw);
        filesToRewind = undefined;
        degradedToFull = true;
      }

      if (result.canRewind) {
        // Authoritative file list: prefer execute response's filesChanged
        const actualFiles = result.filesChanged ?? dryRunResult?.filesChanged ?? [];
        // Silent full-rewind detection: user selected subset but CLI reverted all
        if (filesToRewind && actualFiles.length > 0) {
          const selectedSet = new Set(filesToRewind);
          const allSelected =
            actualFiles.every((f: string) => selectedSet.has(f)) &&
            actualFiles.length <= filesToRewind.length;
          if (!allSelected) {
            degradedToFull = true;
            dbg("rewind-modal", "silent full rewind detected", {
              selected: filesToRewind.length,
              actual: actualFiles.length,
            });
          }
        }
        dbg("rewind-modal", "execute success", { degraded: degradedToFull });
        onSuccess?.({
          runId: runIdAtExec,
          targetContent: selectedAtExec.content,
          targetUuid: selectedAtExec.cliUuid,
          filesReverted: actualFiles,
          degraded: degradedToFull,
        });
        open = false;
      } else {
        dbgWarn("rewind-modal", "execute failed", { error: result.error });
        executeError = result.error ?? t("rewind_checkpointUnavailable");
        phase = "preview";
      }
    } catch (e) {
      if (seq !== requestSeq || !open) return;
      // Degrade: exception path
      if (filesToRewind && isFilesParamUnsupported(e)) {
        dbg("rewind-modal", "files param unsupported (exception), degrading to full rewind");
        try {
          const raw = await api.rewindFiles(runIdAtExec, {
            userMessageId: selectedAtExec.cliUuid,
          });
          if (seq !== requestSeq || !open) return;
          const result = parseExecuteResult(raw);
          if (result.canRewind) {
            degradedToFull = true;
            onSuccess?.({
              runId: runIdAtExec,
              targetContent: selectedAtExec.content,
              targetUuid: selectedAtExec.cliUuid,
              filesReverted: result.filesChanged ?? dryRunResult?.filesChanged ?? [],
              degraded: true,
            });
            open = false;
            return;
          }
          executeError = result.error ?? t("rewind_checkpointUnavailable");
        } catch (e2) {
          executeError = String(e2);
        }
      } else {
        executeError = String(e);
      }
      phase = "preview";
    }
  }

  // ── Go back to selection ──
  function goBack() {
    phase = "select";
    dryRunResult = null;
    dryRunSkipped = false;
    executeError = null;
    selectedFiles = new Set();
  }
</script>

{#snippet candidateList()}
  <div class="{phase === 'select' ? 'max-h-[50vh]' : 'max-h-[30vh] mb-3'} overflow-y-auto">
    {#each candidates as c, i (c.cliUuid)}
      <button
        type="button"
        class="w-full rounded-md border px-3 py-2 text-left transition-colors
          {selected?.cliUuid === c.cliUuid
          ? 'border-primary bg-primary/5'
          : 'border-transparent hover:border-border hover:bg-muted/50'}"
        onclick={() => selectCheckpoint(c)}
        disabled={dryRunLoading && selected?.cliUuid === c.cliUuid}
      >
        <div class="flex items-baseline justify-between gap-2">
          <span class="shrink-0 text-xs font-mono text-muted-foreground/60"
            >#{candidates.length - i}</span
          >
          <span class="min-w-0 flex-1 truncate text-sm">{truncate(c.content, 80)}</span>
          {#if c.ts}
            <span class="shrink-0 text-xs text-muted-foreground">{fmtTime(c.ts)}</span>
          {/if}
        </div>
      </button>
    {/each}
  </div>
{/snippet}

<Modal bind:open title={t("rewind_modalTitle")} closeable={phase !== "executing"}>
  <!-- Phase: select -->
  {#if phase === "select"}
    {#if candidates.length === 0}
      <div class="flex flex-col items-center gap-2 py-8 text-center text-muted-foreground">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-10 w-10 opacity-40"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          stroke-width="1.5"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"
          />
        </svg>
        <p class="text-sm font-medium">{t("rewind_noCheckpoints")}</p>
        <p class="text-xs opacity-70">{t("rewind_noCheckpointsHint")}</p>
      </div>
    {:else}
      <p class="mb-3 text-sm text-muted-foreground">{t("rewind_selectCheckpoint")}</p>
      {@render candidateList()}
    {/if}

    <!-- Phase: preview -->
  {:else if phase === "preview"}
    {#if dryRunLoading}
      <!-- Loading spinner -->
      <div class="flex items-center justify-center py-12">
        <div
          class="h-6 w-6 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent"
        ></div>
      </div>
    {:else if dryRunResult && dryRunResult.canRewind}
      <!-- Successful dryRun preview -->
      {@render candidateList()}

      {#if executeError}
        <div class="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {executeError}
        </div>
      {/if}

      {#if hasFiles}
        <div class="mb-2 flex items-center justify-between">
          <p class="text-sm text-muted-foreground">{t("rewind_previewDesc")}</p>
          <button
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground transition-colors"
            onclick={() => {
              if (selectedFiles.size === dryRunResult!.filesChanged!.length)
                selectedFiles = new Set();
              else selectedFiles = new Set(dryRunResult!.filesChanged!);
            }}
          >
            {selectedFiles.size === dryRunResult!.filesChanged!.length
              ? t("rewind_deselectAll")
              : t("rewind_selectAll")}
          </button>
        </div>
        <div class="mb-4 max-h-[30vh] overflow-y-auto rounded-md border bg-muted/30 p-2">
          {#each dryRunResult.filesChanged as file}
            <label
              class="flex items-center gap-2 rounded px-1 py-0.5 hover:bg-muted/50 cursor-pointer"
            >
              <input
                type="checkbox"
                checked={selectedFiles.has(file)}
                onchange={() => {
                  const next = new Set(selectedFiles);
                  if (next.has(file)) next.delete(file);
                  else next.add(file);
                  selectedFiles = next;
                }}
                class="rounded border-border"
              />
              <span class="truncate font-mono text-xs">{file}</span>
            </label>
          {/each}
        </div>
      {:else}
        <p class="mb-4 text-sm text-muted-foreground">{t("rewind_noFilesChanged")}</p>
      {/if}

      <div class="flex items-center justify-between">
        <span class="text-xs text-muted-foreground/60">
          {#if hasFiles}
            {t("rewind_selectedCount", {
              selected: String(selectedFiles.size),
              total: String(dryRunResult.filesChanged.length),
            })}
          {/if}
        </span>
        <div class="flex gap-2">
          <button
            type="button"
            class="rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-muted"
            onclick={goBack}
          >
            {t("rewind_back")}
          </button>
          <button
            type="button"
            class="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground transition-colors hover:bg-primary/90
              disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={executeRewind}
            disabled={hasFiles && selectedFiles.size === 0}
          >
            {t("rewind_confirm")}
          </button>
        </div>
      </div>
    {:else if dryRunSkipped}
      <!-- CLI doesn't support dry_run — allow execute without preview -->
      {@render candidateList()}

      {#if executeError}
        <div class="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {executeError}
        </div>
      {/if}

      <p class="mb-4 text-sm text-muted-foreground">{t("rewind_previewUnavailable")}</p>

      <div class="flex justify-end gap-2">
        <button
          type="button"
          class="rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-muted"
          onclick={goBack}
        >
          {t("rewind_back")}
        </button>
        <button
          type="button"
          class="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground transition-colors hover:bg-primary/90"
          onclick={executeRewind}
        >
          {t("rewind_confirm")}
        </button>
      </div>
    {:else}
      <!-- dryRun failed (hard error or canRewind: false) -->
      {@render candidateList()}

      <div class="my-4 rounded-md bg-destructive/10 px-3 py-2 text-center text-sm text-destructive">
        {dryRunResult?.error ?? t("rewind_checkpointUnavailable")}
      </div>

      <div class="flex justify-end">
        <button
          type="button"
          class="rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-muted"
          onclick={goBack}
        >
          {t("rewind_back")}
        </button>
      </div>
    {/if}

    <!-- Phase: executing -->
  {:else if phase === "executing"}
    <div class="flex flex-col items-center gap-3 py-12">
      <div
        class="h-6 w-6 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent"
      ></div>
      <p class="text-sm text-muted-foreground">{t("rewind_executing")}</p>
    </div>
  {/if}
</Modal>

<script lang="ts">
  import * as api from "$lib/api";
  import type { HistoryContent } from "$lib/types";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { appendHistoryChunkWindow } from "$lib/utils/history-content-window";
  import { isCurrentContentChunk } from "$lib/utils/history-request-guard";
  import { t } from "$lib/i18n/index.svelte";

  let {
    runId,
    generationId,
    content,
    label,
  }: { runId: string; generationId: string; content: HistoryContent; label?: string } = $props();

  let chunks = $state<string[]>([]);
  let chunkStarts = $state<number[]>([]);
  let nextOffset = $state(0);
  let eof = $state(false);
  let loading = $state(false);
  let error = $state("");
  let windowStart = $state(0);
  let decoder = new TextDecoder();
  let requestGeneration = 0;

  const decoded = $derived(chunks.join(""));

  $effect(() => {
    const identity = `${runId}:${generationId}:${content.contentId ?? "inline"}`;
    void identity;
    requestGeneration++;
    chunks = [];
    chunkStarts = [];
    nextOffset = 0;
    eof = false;
    loading = false;
    error = "";
    windowStart = 0;
    decoder = new TextDecoder();
  });

  async function loadNext() {
    if (!content.contentId || loading || eof) return;
    const request = {
      token: ++requestGeneration,
      runId,
      generationId,
      contentId: content.contentId,
      offset: nextOffset,
    };
    loading = true;
    error = "";
    try {
      const chunk = await api.getHistoryContentChunk(
        request.runId,
        request.generationId,
        request.contentId,
        request.offset,
      );
      const current = { runId, generationId, contentId: content.contentId ?? "" };
      if (!isCurrentContentChunk(request, requestGeneration, chunk, current)) {
        return;
      }
      const bytes = Uint8Array.from(atob(chunk.dataBase64), (char) => char.charCodeAt(0));
      const decodedChunk = decoder.decode(bytes, { stream: !chunk.eof });
      const window = appendHistoryChunkWindow(chunks, chunkStarts, decodedChunk, chunk.offset);
      chunks = window.chunks;
      chunkStarts = window.chunkStarts;
      windowStart = window.windowStart;
      nextOffset = chunk.nextOffset;
      eof = chunk.eof;
      dbg("history", "content chunk loaded", {
        contentId: content.contentId,
        offset: chunk.offset,
        nextOffset,
        eof,
      });
    } catch (e) {
      if (
        request.token !== requestGeneration ||
        request.runId !== runId ||
        request.generationId !== generationId ||
        request.contentId !== content.contentId
      )
        return;
      error = String(e);
      dbgWarn("history", "content chunk failed", e);
    } finally {
      if (
        request.token === requestGeneration &&
        request.runId === runId &&
        request.generationId === generationId &&
        request.contentId === content.contentId
      )
        loading = false;
    }
  }
</script>

<div class="mt-2 rounded border border-border/50 bg-muted/30 p-2 text-xs" data-export-exclude>
  {#if label}<div class="mb-1 font-medium text-muted-foreground">{label}</div>{/if}
  {#if decoded}
    {#if windowStart > 0}
      <div class="mb-1 text-muted-foreground">
        {t("historyContent_window", { offset: windowStart.toLocaleString() })}
      </div>
    {/if}
    <pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono">{decoded}</pre>
  {:else}
    <div class="text-muted-foreground">
      {t("historyContent_preview", { bytes: content.byteLength.toLocaleString() })}
    </div>
  {/if}
  <button
    class="mt-2 rounded bg-muted px-2 py-1 text-foreground hover:bg-muted/80 disabled:opacity-50"
    disabled={loading || eof}
    onclick={loadNext}
  >
    {loading
      ? t("historyContent_loading")
      : eof
        ? t("historyContent_loaded")
        : error
          ? t("historyContent_retry")
          : t("historyContent_loadNext")}
  </button>
  {#if error}<span class="ml-2 text-destructive">{error}</span>{/if}
</div>

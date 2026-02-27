<script lang="ts">
  import { onMount } from "svelte";
  import { getChangelog } from "$lib/api";
  import { getCliVersionInfo_cached } from "$lib/stores";
  import type { ChangelogEntry } from "$lib/types";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let entries = $state<ChangelogEntry[]>([]);
  let loading = $state(true);
  let error = $state("");
  let searchQuery = $state("");

  let filteredEntries = $derived.by(() => {
    if (!searchQuery.trim()) return entries;
    const q = searchQuery.trim().toLowerCase();
    return entries.filter(
      (e) => e.version.includes(q) || e.changes.some((c) => c.toLowerCase().includes(q)),
    );
  });

  // Current CLI version: prefer cli-info cache (updated by session), fall back to localStorage
  let currentVersion = $derived(
    getCliVersionInfo_cached()?.installed ??
      (() => {
        try {
          return localStorage.getItem("ocv:cli-version") ?? "";
        } catch {
          return "";
        }
      })(),
  );

  onMount(async () => {
    try {
      dbg("release-notes", "loading changelog");
      entries = await getChangelog();
      dbg("release-notes", "loaded", { count: entries.length });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      dbgWarn("release-notes", "failed to load", msg);
      error = msg;
    } finally {
      loading = false;
    }
  });
</script>

<div class="flex h-full flex-col bg-background">
  <!-- Header -->
  <div class="flex h-14 shrink-0 items-center gap-3 border-b border-border px-6">
    <button
      class="rounded-md p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
      onclick={() => history.back()}
      title={t("release_goBack")}
    >
      <svg
        class="h-4 w-4"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><path d="m12 19-7-7 7-7" /><path d="M19 12H5" /></svg
      >
    </button>

    <div class="flex items-center gap-2">
      <svg
        class="h-4 w-4 text-muted-foreground"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path
          d="M14 2v4a2 2 0 0 0 2 2h4"
        /><path d="M10 9H8" /><path d="M16 13H8" /><path d="M16 17H8" /></svg
      >
      <h1 class="text-sm font-medium">{t("release_title")}</h1>
      {#if currentVersion}
        <span
          class="rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-mono font-medium text-primary"
        >
          v{currentVersion}
        </span>
      {/if}
    </div>

    <div class="flex-1"></div>

    <!-- Search -->
    <div class="relative">
      <svg
        class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></svg
      >
      <input
        type="text"
        bind:value={searchQuery}
        placeholder={t("release_searchPlaceholder")}
        class="h-8 w-56 rounded-md border border-border bg-background pl-8 pr-3 text-xs
          placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
      />
    </div>

    <a
      href="https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md"
      target="_blank"
      rel="noopener noreferrer"
      class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
    >
      <svg
        class="h-3.5 w-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" /><polyline
          points="15 3 21 3 21 9"
        /><line x1="10" x2="21" y1="14" y2="3" /></svg
      >
      {t("release_github")}
    </a>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <div class="flex items-center justify-center py-20">
        <div
          class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
        ></div>
      </div>
    {:else if error}
      <div class="flex flex-col items-center gap-3 px-6 py-20 text-center">
        <svg
          class="h-8 w-8 text-destructive/50"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><circle cx="12" cy="12" r="10" /><line x1="12" x2="12" y1="8" y2="12" /><line
            x1="12"
            x2="12.01"
            y1="16"
            y2="16"
          /></svg
        >
        <p class="text-sm text-muted-foreground">{t("release_loadFailed")}</p>
        <p class="text-xs text-muted-foreground/60">{error}</p>
        <button
          class="mt-2 rounded-md border border-border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
          onclick={async () => {
            loading = true;
            error = "";
            try {
              entries = await getChangelog();
            } catch (e) {
              error = e instanceof Error ? e.message : String(e);
            } finally {
              loading = false;
            }
          }}>{t("common_retry")}</button
        >
      </div>
    {:else if filteredEntries.length === 0}
      <div class="flex flex-col items-center gap-2 py-20 text-center">
        <p class="text-sm text-muted-foreground">{t("release_noMatching")}</p>
        {#if searchQuery}
          <button
            class="text-xs text-primary/70 hover:text-primary transition-colors"
            onclick={() => (searchQuery = "")}
          >
            {t("release_clearSearch")}
          </button>
        {/if}
      </div>
    {:else}
      <div class="mx-auto max-w-3xl px-6 py-6 space-y-1">
        {#each filteredEntries as entry}
          {@const isCurrent = currentVersion && entry.version === currentVersion}
          <div
            class="group relative rounded-lg border px-5 py-4 transition-colors
              {isCurrent
              ? 'border-primary/30 bg-primary/5'
              : 'border-border/50 hover:border-border'}"
          >
            <!-- Version header -->
            <div class="flex items-center gap-2.5 mb-2.5">
              <span
                class="inline-flex items-center rounded px-2 py-0.5 text-xs font-mono font-semibold
                  {isCurrent ? 'bg-primary/15 text-primary' : 'bg-foreground/8 text-foreground/70'}"
              >
                v{entry.version}
              </span>
              {#if isCurrent}
                <span
                  class="rounded-full bg-primary/10 px-2 py-0.5 text-[10px] font-medium text-primary"
                  >{t("release_current")}</span
                >
              {/if}
              {#if entry.date}
                <span class="text-[11px] text-muted-foreground">{entry.date}</span>
              {/if}
            </div>

            <!-- Changes -->
            <ul class="space-y-1 pl-0.5">
              {#each entry.changes as change}
                <li class="flex items-start gap-2 text-[13px] leading-relaxed text-foreground/80">
                  <span class="mt-2 h-1 w-1 shrink-0 rounded-full bg-foreground/25"></span>
                  <span>{change}</span>
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

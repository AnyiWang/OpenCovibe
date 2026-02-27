<script lang="ts">
  import { goto } from "$app/navigation";
  import { t } from "$lib/i18n/index.svelte";

  let { text }: { text: string } = $props();

  const MAX_VISIBLE = 3;

  interface VersionEntry {
    version: string;
    changes: string[];
  }

  /**
   * Parse CLI /release-notes output format:
   * "Version X.Y.Z:\n• Change 1\n• Change 2\n\nVersion X.Y.Z:\n..."
   */
  function parseReleaseNotes(raw: string): VersionEntry[] {
    const entries: VersionEntry[] = [];
    let current: VersionEntry | null = null;

    for (const line of raw.split("\n")) {
      const trimmed = line.trim();
      if (trimmed.startsWith("Version ") && trimmed.endsWith(":")) {
        if (current && current.changes.length > 0) entries.push(current);
        current = { version: trimmed.slice(8, -1).trim(), changes: [] };
      } else if (trimmed.startsWith("•") && current) {
        const change = trimmed.slice(1).trim();
        if (change) current.changes.push(change);
      }
    }
    if (current && current.changes.length > 0) entries.push(current);
    return entries;
  }

  let allEntries = $derived(parseReleaseNotes(text));
  let visibleEntries = $derived(allEntries.slice(0, MAX_VISIBLE));
  let hiddenCount = $derived(Math.max(0, allEntries.length - MAX_VISIBLE));
</script>

<div class="space-y-3">
  <div class="flex items-center gap-2 text-xs text-foreground/50">
    <svg
      class="h-3.5 w-3.5"
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
    <span class="font-medium">{t("notes_title")}</span>
  </div>

  {#each visibleEntries as entry, i}
    <div class="space-y-1">
      <div class="flex items-center gap-2">
        <span
          class="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono font-medium
            {i === 0 ? 'bg-primary/15 text-primary' : 'bg-foreground/10 text-foreground/60'}"
        >
          v{entry.version}
        </span>
        {#if i === 0}
          <span class="text-[10px] text-primary/60">{t("notes_latest")}</span>
        {/if}
      </div>
      <ul class="space-y-0.5 pl-0.5">
        {#each entry.changes as change}
          <li class="flex items-start gap-1.5 text-xs text-foreground/70">
            <span class="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-foreground/30"></span>
            <span>{change}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/each}

  {#if hiddenCount > 0}
    <button
      class="flex items-center gap-1 text-xs text-primary/70 hover:text-primary transition-colors"
      onclick={() => goto("/release-notes")}
    >
      {t("notes_viewAll", { count: String(allEntries.length) })}
      <svg
        class="h-3 w-3"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
      >
    </button>
  {/if}
</div>

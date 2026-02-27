<script lang="ts">
  import { formatBytes } from "$lib/utils/format";
  import { isPdf } from "$lib/utils/file-types";
  import { t } from "$lib/i18n/index.svelte";

  let {
    name,
    size,
    mimeType = "",
    onremove,
  }: {
    name: string;
    size: number;
    mimeType?: string;
    onremove?: () => void;
  } = $props();

  let isDoc = $derived(isPdf(mimeType));
</script>

<div class="flex items-center gap-2 rounded-md border bg-muted/50 px-2 py-1 text-xs">
  {#if isDoc}
    <!-- Document icon for PDF -->
    <svg
      class="h-3.5 w-3.5 text-red-400 shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
      <path d="M14 2v4a2 2 0 0 0 2 2h4" />
      <path d="M10 9H8" /><path d="M16 13H8" /><path d="M16 17H8" />
    </svg>
  {:else}
    <!-- Paperclip icon for images/other -->
    <svg
      class="h-3.5 w-3.5 text-muted-foreground shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"
      />
    </svg>
  {/if}
  <span class="truncate max-w-[120px]">{name}</span>
  <span class="text-muted-foreground">{formatBytes(size)}</span>
  {#if onremove}
    <button
      class="ml-auto text-muted-foreground hover:text-foreground"
      onclick={onremove}
      aria-label={t("common_removeAttachment")}
    >
      <svg class="h-3 w-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M18 6 6 18M6 6l12 12" />
      </svg>
    </button>
  {/if}
</div>

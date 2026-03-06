<script lang="ts">
  import { formatBytes } from "$lib/utils/format";
  import { isPdf } from "$lib/utils/file-types";
  import { t } from "$lib/i18n/index.svelte";

  let {
    name,
    size,
    mimeType = "",
    isPathRef = false,
    onremove,
  }: {
    name: string;
    size: number;
    mimeType?: string;
    isPathRef?: boolean;
    onremove?: () => void;
  } = $props();

  let isDoc = $derived(isPdf(mimeType));
  let isImage = $derived(mimeType.startsWith("image/"));
  let isDir = $derived(mimeType === "inode/directory");

  // Color scheme per file type
  let colorClasses = $derived.by(() => {
    if (isDir) {
      return {
        border: isPathRef
          ? "border-dashed border-amber-400/50 dark:border-amber-500/40"
          : "border-amber-200 dark:border-amber-800",
        bg: "bg-amber-50 dark:bg-amber-950/40",
        icon: "text-amber-600 dark:text-amber-400",
        size: "text-amber-400 dark:text-amber-500",
      };
    }
    if (isImage) {
      return {
        border: isPathRef
          ? "border-dashed border-sky-400/50 dark:border-sky-500/40"
          : "border-sky-200 dark:border-sky-800",
        bg: "bg-sky-50 dark:bg-sky-950/40",
        icon: "text-sky-600 dark:text-sky-400",
        size: "text-sky-400 dark:text-sky-500",
      };
    }
    if (isDoc) {
      return {
        border: isPathRef
          ? "border-dashed border-red-400/50 dark:border-red-500/40"
          : "border-red-200 dark:border-red-800",
        bg: "bg-red-50 dark:bg-red-950/40",
        icon: "text-red-600 dark:text-red-400",
        size: "text-red-400 dark:text-red-500",
      };
    }
    // Default (other files)
    return {
      border: isPathRef ? "border-dashed border-muted-foreground/40" : "border-border",
      bg: "bg-muted/50",
      icon: "text-muted-foreground",
      size: "text-muted-foreground",
    };
  });
</script>

<div
  class="flex items-center gap-2 rounded-md border {colorClasses.border} {colorClasses.bg} px-2 py-1 text-xs"
>
  {#if isDir}
    <!-- Folder icon -->
    <svg
      class="h-3.5 w-3.5 {colorClasses.icon} shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"
      />
    </svg>
  {:else if isImage}
    <!-- Image icon -->
    <svg
      class="h-3.5 w-3.5 {colorClasses.icon} shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
      <circle cx="9" cy="9" r="2" />
      <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
    </svg>
  {:else if isDoc}
    <!-- Document icon for PDF -->
    <svg
      class="h-3.5 w-3.5 {colorClasses.icon} shrink-0"
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
    <!-- Paperclip icon for other -->
    <svg
      class="h-3.5 w-3.5 {colorClasses.icon} shrink-0"
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
  {#if size > 0}
    <span class={colorClasses.size}>{formatBytes(size)}</span>
  {/if}
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

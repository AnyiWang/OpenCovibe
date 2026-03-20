<script lang="ts">
  import type { Snippet } from "svelte";
  import type { ProjectFolder, ConversationGroup } from "$lib/utils/sidebar-groups";
  import ConversationItem from "./ConversationItem.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { dbgWarn } from "$lib/utils/debug";

  const PAGE_SIZE = 5;

  type BaseProps = {
    folder: ProjectFolder;
    label: string;
    expanded?: boolean;
    onToggle: () => void;
    showCount?: boolean;
    onRemove?: () => void;
  };

  type ChatProps = BaseProps & {
    children?: never;
    selectedRunId?: string;
    onSelectConversation: (runId: string) => void;
    onResume: (runId: string, mode: "resume") => void;
    onDelete?: (conversation: ConversationGroup) => void;
    onNewChat?: () => void;
  };

  type CustomProps = BaseProps & {
    children: Snippet;
    selectedRunId?: never;
    onSelectConversation?: never;
    onResume?: never;
    onDelete?: never;
  };

  let {
    folder,
    label,
    expanded = false,
    onToggle,
    showCount = true,
    onRemove,
    children,
    selectedRunId = "",
    onSelectConversation,
    onResume,
    onDelete,
    onNewChat,
  }: ChatProps | CustomProps = $props();

  let visibleCount = $state(PAGE_SIZE);

  // Reset visible count when folder collapses
  $effect(() => {
    if (!expanded) visibleCount = PAGE_SIZE;
  });

  // Auto-expand visible count if selected run is beyond current page
  $effect(() => {
    if (!expanded || !selectedRunId || children) return;
    const idx = folder.conversations.findIndex((conv) =>
      conv.runs.some((r) => r.id === selectedRunId),
    );
    if (idx >= 0 && idx >= visibleCount) {
      visibleCount = idx + 1;
    }
  });

  // Skip conversation-related derivations when using children snippet
  const visibleConversations = $derived(
    children ? [] : folder.conversations.slice(0, visibleCount),
  );
  const hiddenCount = $derived(children ? 0 : folder.conversationCount - visibleCount);
  const hasMore = $derived(hiddenCount > 0);

  function showMore() {
    visibleCount = Math.min(visibleCount + PAGE_SIZE, folder.conversationCount);
  }

  function isConvSelected(conv: { runs: { id: string }[] }): boolean {
    return conv.runs.some((r) => r.id === selectedRunId);
  }

  // Warn once if conversation-mode callbacks are missing
  let warnedMissingCallbacks = false;
  $effect(() => {
    if (children) {
      // children mode switched back to conversation mode — reset latch
      warnedMissingCallbacks = false;
      return;
    }
    if (!warnedMissingCallbacks && (!onSelectConversation || !onResume)) {
      warnedMissingCallbacks = true;
      if (!onSelectConversation)
        dbgWarn("ProjectFolderItem", "onSelectConversation missing in conversation mode");
      if (!onResume) dbgWarn("ProjectFolderItem", "onResume missing in conversation mode");
    }
  });
</script>

<div class="group/folder mb-0.5">
  <!-- Folder header -->
  <div
    class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors cursor-pointer"
    role="button"
    tabindex="0"
    onclick={onToggle}
    onkeydown={(e) => {
      if (e.target !== e.currentTarget) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onToggle();
      }
    }}
    title={folder.isUncategorized ? label : folder.cwd}
    aria-expanded={expanded}
    aria-label={label}
  >
    <!-- Chevron -->
    <svg
      class="h-3 w-3 shrink-0 text-muted-foreground/60 transition-transform duration-150 {expanded
        ? 'rotate-90'
        : ''}"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M9 18l6-6-6-6" />
    </svg>
    <!-- Icon -->
    {#if folder.isUncategorized}
      <!-- Inbox icon -->
      <svg
        class="h-3.5 w-3.5 shrink-0 text-muted-foreground/70"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <polyline points="22 12 16 12 14 15 10 15 8 12 2 12" />
        <path
          d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"
        />
      </svg>
    {:else}
      <!-- Folder icon -->
      <svg
        class="h-3.5 w-3.5 shrink-0 text-muted-foreground/70"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
      </svg>
    {/if}
    <!-- Label -->
    <span class="truncate">{label}</span>
    <!-- Count badge -->
    {#if showCount && folder.conversationCount > 0}
      <span
        class="shrink-0 inline-flex h-3.5 min-w-[14px] items-center justify-center rounded-full bg-muted px-1 text-[10px] font-medium text-muted-foreground {onRemove
          ? ''
          : 'ml-auto'}"
      >
        {folder.conversationCount}
      </span>
    {/if}
    <!-- Remove button (×) -->
    {#if onRemove}
      <button
        class="ml-auto shrink-0 flex h-4 w-4 items-center justify-center rounded opacity-0 text-muted-foreground hover:text-destructive hover:opacity-100 focus-visible:opacity-100 group-hover/folder:opacity-100 transition-opacity"
        aria-label={t("sidebar_removeProject")}
        onclick={(e) => {
          e.stopPropagation();
          onRemove?.();
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.stopPropagation();
            e.preventDefault();
            onRemove?.();
          }
        }}
      >
        <svg
          class="h-3 w-3"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
        >
      </button>
    {/if}
  </div>

  <!-- Expanded children -->
  {#if expanded}
    <div class="pl-3">
      {#if children}
        {@render children()}
      {:else}
        {#if onNewChat}
          <button
            class="flex w-full items-center gap-1.5 px-3 py-1 text-xs text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 rounded-md transition-colors"
            onclick={(e) => {
              e.stopPropagation();
              onNewChat?.();
            }}
          >
            <svg
              class="h-3 w-3 shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d="M12 5v14" /><path d="M5 12h14" /></svg
            >
            <span>{t("sidebar_newChatInFolder")}</span>
          </button>
        {/if}
        {#each visibleConversations as conv (conv.groupKey)}
          <ConversationItem
            conversation={conv}
            selected={isConvSelected(conv)}
            onclick={() => onSelectConversation?.(conv.latestRun.id)}
            onresume={onResume}
            ondelete={onDelete}
          />
        {/each}
        {#if hasMore}
          <button
            class="w-full px-3 py-1.5 text-xs text-muted-foreground hover:text-sidebar-foreground hover:bg-sidebar-accent/50 rounded-md transition-colors"
            onclick={showMore}
          >
            {t("sidebar_showMore", { count: String(Math.min(PAGE_SIZE, hiddenCount)) })}
          </button>
        {/if}
      {/if}
    </div>
  {/if}
</div>

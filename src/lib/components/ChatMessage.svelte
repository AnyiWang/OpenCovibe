<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { fmtTime, fmtDateTime } from "$lib/i18n/format";
  import MarkdownContent from "./MarkdownContent.svelte";
  import FileAttachment from "./FileAttachment.svelte";
  import { IMAGE_TYPES } from "$lib/utils/file-types";
  import type { ChatMessage, Attachment } from "$lib/types";

  let {
    message,
    attachments,
  }: {
    message: ChatMessage;
    attachments?: Attachment[];
  } = $props();

  function isImage(att: Attachment): boolean {
    return (IMAGE_TYPES as readonly string[]).includes(att.type);
  }

  const isUser = $derived(message.role === "user");

  let hovered = $state(false);
  let copied = $state(false);
  let collapsed = $state(true);

  const lineCount = $derived(message.content.split("\n").length);
  const isLong = $derived(isUser && lineCount > 10);

  function formatTime(ts: string): string {
    const d = new Date(ts);
    if (isNaN(d.getTime())) return "";
    const now = new Date();
    const isToday =
      d.getFullYear() === now.getFullYear() &&
      d.getMonth() === now.getMonth() &&
      d.getDate() === now.getDate();
    return isToday ? fmtTime(d) : fmtDateTime(d);
  }

  function formatFullTime(ts: string): string {
    return fmtDateTime(ts);
  }

  async function copyContent() {
    try {
      await navigator.clipboard.writeText(message.content);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // Silently fail
    }
  }
</script>

<div
  class="w-full {isUser ? 'bg-muted/50' : ''}"
  role="group"
  onmouseenter={() => (hovered = true)}
  onmouseleave={() => (hovered = false)}
>
  <div class="mx-auto max-w-5xl px-8 py-4">
    <!-- Header: icon + name + copy button + timestamp -->
    <div class="mb-1.5 flex items-center gap-2">
      {#if isUser}
        <div class="flex h-5 w-5 items-center justify-center rounded-sm bg-primary/10 text-primary">
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
            <circle cx="12" cy="7" r="4" />
          </svg>
        </div>
        <span class="text-sm font-semibold text-foreground">{t("chat_roleYou")}</span>
      {:else}
        <div
          class="flex h-5 w-5 items-center justify-center rounded-sm bg-orange-500/10 text-orange-500"
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path
              d="M12 3l1.912 5.813a2 2 0 0 0 1.275 1.275L21 12l-5.813 1.912a2 2 0 0 0-1.275 1.275L12 21l-1.912-5.813a2 2 0 0 0-1.275-1.275L3 12l5.813-1.912a2 2 0 0 0 1.275-1.275L12 3z"
            />
          </svg>
        </div>
        <span class="text-sm font-semibold text-foreground">{t("chat_roleClaude")}</span>
      {/if}
      <button
        class="ml-auto p-1 rounded-md text-muted-foreground/50 hover:bg-muted hover:text-foreground transition-all duration-150 {hovered ||
        copied
          ? 'opacity-100'
          : 'opacity-0'}"
        onclick={copyContent}
        title={t("chat_copyMessage")}
      >
        {#if copied}
          <svg
            class="h-3.5 w-3.5 text-emerald-500"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
        {:else}
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><rect width="14" height="14" x="8" y="8" rx="2" /><path
              d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"
            /></svg
          >
        {/if}
      </button>
      <span class="text-[10px] text-muted-foreground" title={formatFullTime(message.timestamp)}>
        {formatTime(message.timestamp)}
      </span>
    </div>
    <!-- Content: indented to align with text after icon -->
    <div class="pl-7 text-sm text-foreground leading-relaxed">
      {#if isUser}
        {#if attachments && attachments.length > 0}
          <div class="flex flex-wrap gap-2 mb-2">
            {#each attachments as att}
              {#if isImage(att) && att.contentBase64}
                <img
                  src="data:{att.type};base64,{att.contentBase64}"
                  alt={att.name}
                  class="max-h-48 max-w-xs rounded-md border border-border object-contain"
                />
              {:else}
                <FileAttachment name={att.name} size={att.size} mimeType={att.type} />
              {/if}
            {/each}
          </div>
        {/if}
        {#if isLong}
          <p
            class="whitespace-pre-wrap {collapsed ? 'max-h-24 overflow-hidden' : ''}"
            style={collapsed
              ? "mask-image: linear-gradient(to bottom, black 70%, transparent);"
              : ""}
          >
            {message.content}
          </p>
          <button
            class="mt-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
            onclick={() => (collapsed = !collapsed)}
          >
            {collapsed
              ? t("common_showAllLines", { count: String(lineCount) })
              : t("common_collapse")}
          </button>
        {:else}
          <p class="whitespace-pre-wrap">{message.content}</p>
        {/if}
      {:else}
        <div class="prose-chat">
          <MarkdownContent text={message.content} />
        </div>
      {/if}
    </div>
  </div>
</div>

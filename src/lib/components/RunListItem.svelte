<script lang="ts">
  import type { TaskRun } from "$lib/types";
  import { TERMINAL_PHASES } from "$lib/stores";
  import StatusBadge from "./StatusBadge.svelte";
  import { relativeTime, truncate } from "$lib/utils/format";
  import { PLATFORM_PRESETS } from "$lib/utils/platform-presets";
  import { t } from "$lib/i18n/index.svelte";

  function platformLabel(id: string): string {
    return PLATFORM_PRESETS.find((p) => p.id === id)?.name ?? id;
  }

  let {
    run,
    selected = false,
    isFork = false,
    favorite = false,
    onclick,
    onresume,
  }: {
    run: TaskRun;
    selected?: boolean;
    isFork?: boolean;
    favorite?: boolean;
    onclick?: () => void;
    onresume?: (runId: string, mode: "resume") => void;
  } = $props();

  const label = $derived(truncate(run.name || run.prompt, 28));
  const time = $derived(relativeTime(run.last_activity_at ?? run.started_at));
  const canResume = $derived(!!run.session_id && TERMINAL_PHASES.includes(run.status as any));

  let editing = $state(false);
  let editValue = $state("");
  let editInputEl: HTMLInputElement | undefined = $state();

  function startRename() {
    editValue = run.name || run.prompt;
    editing = true;
    requestAnimationFrame(() => {
      editInputEl?.select();
    });
  }

  async function commitRename() {
    editing = false;
    const trimmed = editValue.trim();
    if (trimmed && trimmed !== (run.name || run.prompt)) {
      try {
        const { renameRun } = await import("$lib/api");
        await renameRun(run.id, trimmed);
        window.dispatchEvent(new Event("ocv:runs-changed"));
      } catch {
        // runs will refresh on next poll
      }
    }
  }

  function cancelRename() {
    editing = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onclick?.();
    }
  }
</script>

<div
  class="group w-full text-left px-3 py-2 rounded-md transition-colors text-xs cursor-pointer
    {selected
    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
    : 'hover:bg-sidebar-accent/50 text-sidebar-foreground'}"
  role="button"
  tabindex="0"
  {onclick}
  onkeydown={handleKeydown}
>
  <div class="flex items-center justify-between gap-2">
    <div class="flex items-center gap-1.5 min-w-0">
      {#if favorite}
        <svg
          class="h-3 w-3 shrink-0 text-yellow-500"
          viewBox="0 0 24 24"
          fill="currentColor"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <polygon
            points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"
          />
        </svg>
      {/if}
      {#if isFork}
        <svg
          class="h-3 w-3 shrink-0 text-muted-foreground/60"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <circle cx="12" cy="18" r="3" /><circle cx="6" cy="6" r="3" /><circle
            cx="18"
            cy="6"
            r="3"
          />
          <path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9" /><path d="M12 12v3" />
        </svg>
      {/if}
      {#if editing}
        <input
          bind:this={editInputEl}
          bind:value={editValue}
          class="min-w-0 flex-1 bg-transparent text-xs outline-none border-b border-primary"
          onblur={commitRename}
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitRename();
            }
            if (e.key === "Escape") {
              e.preventDefault();
              cancelRename();
            }
            e.stopPropagation();
          }}
          onclick={(e) => e.stopPropagation()}
        />
      {:else}
        <span
          class="truncate"
          ondblclick={(e) => {
            e.stopPropagation();
            startRename();
          }}>{label}</span
        >
      {/if}
    </div>
    <div class="flex items-center gap-1 shrink-0">
      {#if canResume && onresume}
        <button
          class="opacity-0 group-hover:opacity-100 p-1 rounded hover:bg-accent transition-opacity"
          onclick={(e) => {
            e.stopPropagation();
            onresume(run.id, "resume");
          }}
          title={t("runItem_resumeTitle")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" /><path
              d="M21 3v5h-5"
            /></svg
          >
        </button>
      {/if}
      <StatusBadge status={run.status} class="shrink-0" />
    </div>
  </div>
  <div class="mt-0.5 flex items-center gap-2 text-xs text-muted-foreground">
    <div class="flex items-center gap-1.5 min-w-0">
      <span class="shrink-0">{run.agent}</span>
      {#if run.remote_host_name}
        <!-- Globe icon for remote session -->
        <svg
          class="h-3 w-3 shrink-0 text-blue-400"
          title={t("statusbar_sshTitle", { name: run.remote_host_name })}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><circle cx="12" cy="12" r="10" /><path
            d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"
          /><path d="M2 12h20" /></svg
        >
      {/if}
      {#if run.platform_id && run.platform_id !== "anthropic"}
        <span class="shrink-0">&middot;</span>
        <span class="truncate">{platformLabel(run.platform_id)}</span>
      {/if}
    </div>
    <span class="ml-auto shrink-0">{time}</span>
  </div>
</div>

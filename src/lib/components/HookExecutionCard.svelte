<script lang="ts">
  import { formatDuration } from "$lib/utils/format";
  import { dbg } from "$lib/utils/debug";

  let {
    eventName,
    status,
    statusMessage,
    durationMs,
  }: {
    eventName: string;
    status: string;
    statusMessage?: string;
    durationMs?: number;
  } = $props();

  // camelCase HookEventName → human label. Falls back to a spaced/capitalized form
  // for any future event name the backend adds before this map is updated.
  const EVENT_LABELS: Record<string, string> = {
    preToolUse: "Before tool use",
    permissionRequest: "Permission request",
    postToolUse: "After tool use",
    preCompact: "Before compact",
    postCompact: "After compact",
    sessionStart: "Session start",
    userPromptSubmit: "Prompt submitted",
    subagentStart: "Subagent start",
    subagentStop: "Subagent stop",
    stop: "Stop",
  };

  const label = $derived(
    EVENT_LABELS[eventName] ??
      eventName
        .replace(/([A-Z])/g, " $1")
        .replace(/^./, (c) => c.toUpperCase())
        .trim(),
  );

  const isRunning = $derived(status === "running");

  // Map terminal status → badge color classes. Slash-opacity tokens cannot be used as
  // `class:` directives, so resolve to a full class string instead.
  const badgeClass = $derived.by(() => {
    switch (status) {
      case "completed":
        return "bg-green-500/15 text-green-400";
      case "failed":
      case "blocked":
        return "bg-red-500/15 text-red-400";
      case "stopped":
        return "bg-amber-500/15 text-amber-400";
      default:
        return "bg-muted-foreground/15 text-muted-foreground";
    }
  });

  $effect(() => {
    dbg("hook-card", "render", { eventName, status, durationMs });
  });

  // Single-word badge text — keep it short (micro tier). Title-case the raw status.
  const badgeText = $derived(status.charAt(0).toUpperCase() + status.slice(1));
</script>

<div class="w-full py-1">
  <div class="chat-content-width pl-7">
    <div
      class="flex items-center gap-2 rounded-md border border-border/40 bg-card/40 px-3 py-1.5 text-xs"
    >
      <span class="text-muted-foreground">Hook</span>
      <span class="font-medium text-foreground">{label}</span>

      <!-- Status badge: running spins, terminal states color by severity -->
      {#if isRunning}
        <span
          class="ml-auto inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium bg-muted-foreground/15 text-muted-foreground"
        >
          <span
            class="h-2 w-2 rounded-full border border-muted-foreground/60 border-t-transparent animate-spin"
          ></span>
          Running
        </span>
      {:else}
        <span class="ml-auto rounded px-1.5 py-0.5 text-[10px] font-medium {badgeClass}">
          {badgeText}
        </span>
      {/if}

      {#if durationMs != null && durationMs > 0}
        <span class="text-[10px] text-muted-foreground tabular-nums"
          >{formatDuration(durationMs)}</span
        >
      {/if}
    </div>

    {#if statusMessage}
      <p class="mt-1 pl-1 text-xs text-muted-foreground leading-snug">{statusMessage}</p>
    {/if}
  </div>
</div>

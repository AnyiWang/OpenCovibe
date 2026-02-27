<script lang="ts">
  import type { CliCommand, CliModelInfo } from "$lib/types";
  import { getCommandInteraction, getArgumentHint } from "$lib/utils/slash-commands";
  import type { SlashCategory, SlashCommandGroups } from "$lib/utils/slash-commands";
  import { onMount } from "svelte";
  import { t } from "$lib/i18n/index.svelte";
  import type { MessageKey } from "$lib/i18n/types";
  import { dbg } from "$lib/utils/debug";

  const CATEGORY_I18N: Record<SlashCategory, MessageKey> = {
    session: "slashMenu_catSession",
    coding: "slashMenu_catCoding",
    config: "slashMenu_catConfig",
    help: "slashMenu_catHelp",
    skills: "slashMenu_catSkills",
    other: "slashMenu_catOther",
  };

  let {
    commands,
    slashGroups = null,
    selectedIndex,
    anchorEl,
    triggerEl,
    phase,
    models,
    currentModel,
    subSelectedIndex,
    hintText,
    inputDisplay,
    onSelect,
    onHover,
    onSubHover,
    onSubSelect,
    onBack,
    onDismiss,
  }: {
    commands: CliCommand[];
    slashGroups?: SlashCommandGroups | null;
    selectedIndex: number;
    anchorEl: HTMLElement | undefined;
    triggerEl?: HTMLElement;
    phase: "commands" | "sub-model";
    models: CliModelInfo[];
    currentModel: string;
    subSelectedIndex: number;
    hintText: string;
    inputDisplay: string;
    onSelect: (cmd: CliCommand) => void;
    onHover: (index: number) => void;
    onSubHover: (index: number) => void;
    onSubSelect: (model: CliModelInfo) => void;
    onBack: () => void;
    onDismiss: () => void;
  } = $props();

  let menuEl: HTMLDivElement | undefined = $state();
  let bottom = $state(0);
  let left = $state(0);
  let width = $state(0);

  function updatePosition() {
    if (!anchorEl) return;
    const rect = anchorEl.getBoundingClientRect();
    bottom = window.innerHeight - rect.top + 4;
    left = rect.left;
    width = rect.width;
  }

  // Scroll selected item into view (commands phase)
  $effect(() => {
    if (phase !== "commands") return;
    const idx = selectedIndex;
    if (menuEl) {
      const item = menuEl.querySelector(`[data-index="${idx}"]`);
      item?.scrollIntoView({ block: "nearest" });
    }
  });

  // Scroll selected item into view (sub-model phase)
  $effect(() => {
    if (phase !== "sub-model") return;
    const idx = subSelectedIndex;
    if (menuEl) {
      const item = menuEl.querySelector(`[data-sub-index="${idx}"]`);
      item?.scrollIntoView({ block: "nearest" });
    }
  });

  onMount(() => {
    updatePosition();

    // Reposition on scroll (capture phase) and resize
    window.addEventListener("scroll", updatePosition, true);
    window.addEventListener("resize", updatePosition);

    // Click-outside: dismiss if click is outside menu AND anchor AND trigger button
    function handleMousedown(e: MouseEvent) {
      const target = e.target as Node;
      if (
        menuEl &&
        !menuEl.contains(target) &&
        !anchorEl?.contains(target) &&
        !triggerEl?.contains(target)
      ) {
        onDismiss();
      }
    }
    document.addEventListener("mousedown", handleMousedown, true);

    return () => {
      window.removeEventListener("scroll", updatePosition, true);
      window.removeEventListener("resize", updatePosition);
      document.removeEventListener("mousedown", handleMousedown, true);
    };
  });
</script>

<div
  bind:this={menuEl}
  class="fixed z-50 rounded-lg border border-border bg-background shadow-lg animate-fade-in"
  style="bottom: {bottom}px; left: {left}px; width: {width}px;"
>
  {#if phase === "commands"}
    {#if hintText}
      <div class="px-3 py-1.5">
        <span class="text-xs italic text-muted-foreground/50">{hintText}</span>
      </div>
      <div class="border-t border-border"></div>
    {/if}

    {#if slashGroups}
      <!-- Grouped mode (empty query) -->
      <div class="max-h-[320px] overflow-y-auto">
        {#each slashGroups.groups as group}
          <p
            class="px-3 pt-2 pb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/50"
          >
            {t(CATEGORY_I18N[group.category])}
          </p>
          {#each group.commands as cmd, i}
            {@const globalIndex = group.startIndex + i}
            {@const interaction = getCommandInteraction(cmd)}
            {@const hint = getArgumentHint(cmd)}
            <button
              data-index={globalIndex}
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors {globalIndex ===
              selectedIndex
                ? 'bg-accent text-foreground'
                : 'text-muted-foreground hover:bg-accent/50'}"
              onmouseenter={() => onHover(globalIndex)}
              onclick={() => {
                dbg("slash", "select", { index: globalIndex, name: cmd.name });
                onSelect(cmd);
              }}
            >
              <span class="shrink-0 font-mono text-xs text-foreground">/{cmd.name}</span>
              <span class="flex-1 min-w-0 truncate text-xs opacity-70">
                {cmd.description}
                {#if hint}
                  <span class="italic opacity-50"> · {hint}</span>
                {/if}
              </span>
              {#if interaction === "enum"}
                <span class="shrink-0 text-xs text-muted-foreground/40">&rarr;</span>
              {/if}
              {#if cmd.aliases?.length > 0}
                <span class="flex shrink-0 gap-1">
                  {#each cmd.aliases ?? [] as alias}
                    <span
                      class="rounded bg-muted px-1 py-0.5 text-[10px] font-mono text-muted-foreground"
                      >{alias}</span
                    >
                  {/each}
                </span>
              {/if}
            </button>
          {/each}
        {/each}
      </div>
    {:else if commands.length > 0}
      <!-- Flat mode (has query) -->
      <div class="max-h-[240px] overflow-y-auto">
        {#each commands as cmd, i (i)}
          {@const interaction = getCommandInteraction(cmd)}
          {@const hint = getArgumentHint(cmd)}
          <button
            data-index={i}
            class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors {i ===
            selectedIndex
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onmouseenter={() => onHover(i)}
            onclick={() => onSelect(cmd)}
          >
            <span class="shrink-0 font-mono text-xs text-foreground">/{cmd.name}</span>
            <span class="flex-1 min-w-0 truncate text-xs opacity-70">
              {cmd.description}
              {#if hint}
                <span class="italic opacity-50"> · {hint}</span>
              {/if}
            </span>
            {#if interaction === "enum"}
              <span class="shrink-0 text-xs text-muted-foreground/40">&rarr;</span>
            {/if}
            {#if cmd.aliases?.length > 0}
              <span class="flex shrink-0 gap-1">
                {#each cmd.aliases ?? [] as alias}
                  <span
                    class="rounded bg-muted px-1 py-0.5 text-[10px] font-mono text-muted-foreground"
                    >{alias}</span
                  >
                {/each}
              </span>
            {/if}
          </button>
        {/each}
      </div>
    {:else}
      <div class="flex items-center justify-center py-6">
        <span class="text-xs text-muted-foreground/50">No matching commands</span>
      </div>
    {/if}
  {:else if phase === "sub-model"}
    <!-- Sub-view header -->
    <div class="flex items-center gap-2 px-3 py-2 border-b border-border">
      <button
        class="flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
        onclick={onBack}
        title={t("slashMenu_backToCommands")}
      >
        <svg
          class="h-3.5 w-3.5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="m15 18-6-6 6-6" />
        </svg>
      </button>
      <span class="text-xs font-mono text-muted-foreground">{inputDisplay}</span>
    </div>

    {#if models.length > 0}
      <div class="max-h-[240px] overflow-y-auto">
        {#each models as model, i (model.value)}
          <button
            data-sub-index={i}
            class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors {i ===
            subSelectedIndex
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onmouseenter={() => onSubHover(i)}
            onclick={() => onSubSelect(model)}
          >
            <span
              class="w-4 shrink-0 text-xs {model.value === currentModel
                ? 'text-primary'
                : 'text-transparent'}"
            >
              {model.value === currentModel ? "✓" : ""}
            </span>
            <span class="font-medium text-xs text-foreground">{model.displayName}</span>
            <span class="flex-1 min-w-0 truncate text-xs opacity-70">{model.description}</span>
          </button>
        {/each}
      </div>
    {:else}
      <div class="flex items-center justify-center py-6">
        <span class="text-xs text-muted-foreground/50">No models available</span>
      </div>
    {/if}
  {/if}
</div>

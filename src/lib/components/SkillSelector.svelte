<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg } from "$lib/utils/debug";

  interface SkillItem {
    name: string;
    description: string;
  }

  let {
    skills = [],
    agents = [],
    disabled = false,
    onSelect,
  }: {
    skills?: SkillItem[];
    agents?: SkillItem[];
    disabled?: boolean;
    onSelect?: (name: string) => void;
  } = $props();

  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  let isEmpty = $derived(skills.length === 0 && agents.length === 0);

  function toggleDropdown() {
    if (disabled) return;
    dropdownOpen = !dropdownOpen;
    dbg("skill-selector", "toggle", { open: dropdownOpen });
    if (dropdownOpen && buttonEl) updateDropdownPosition();
  }

  function updateDropdownPosition() {
    if (!buttonEl) return;
    const rect = buttonEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    if (spaceBelow < 300) {
      dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
    } else {
      dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
    }
  }

  function selectSkill(name: string) {
    dbg("skill-selector", "select", { skillName: name });
    dropdownOpen = false;
    onSelect?.(name);
  }

  onMount(() => {
    function onDocClick(e: MouseEvent) {
      if (dropdownOpen && wrapperEl && !wrapperEl.contains(e.target as Node)) {
        dropdownOpen = false;
      }
    }
    function onDocKeydown(e: KeyboardEvent) {
      if (dropdownOpen && e.key === "Escape") {
        dropdownOpen = false;
      }
    }
    document.addEventListener("mousedown", onDocClick, true);
    document.addEventListener("keydown", onDocKeydown);
    return () => {
      document.removeEventListener("mousedown", onDocClick, true);
      document.removeEventListener("keydown", onDocKeydown);
    };
  });
</script>

<div bind:this={wrapperEl} class="inline-flex items-center">
  <button
    bind:this={buttonEl}
    class="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs font-medium transition-colors
      {disabled
      ? 'text-muted-foreground/40 cursor-default'
      : 'text-muted-foreground hover:text-foreground hover:bg-accent cursor-pointer'}"
    onclick={toggleDropdown}
    {disabled}
    title={t("skillSelector_label")}
  >
    <!-- Sparkles icon -->
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
        d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"
      />
      <path d="M20 3v4" />
      <path d="M22 5h-4" />
      <path d="M4 17v2" />
      <path d="M5 18H3" />
    </svg>
    {t("skillSelector_label")}
    <svg
      class="h-2.5 w-2.5 text-foreground/30"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
    >
  </button>

  {#if dropdownOpen}
    <div
      class="w-80 max-h-96 overflow-y-auto rounded-lg border bg-background shadow-lg animate-fade-in"
      style={dropdownStyle}
    >
      {#if isEmpty}
        <p class="px-3 py-4 text-xs text-muted-foreground text-center">
          {t("skillSelector_empty")}
        </p>
      {:else}
        <div class="p-1">
          <!-- Skills group -->
          {#if skills.length > 0}
            {#each skills as skill}
              <button
                class="group flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left hover:bg-accent transition-colors"
                onclick={() => selectSkill(skill.name)}
              >
                <span
                  class="shrink-0 text-[11px] font-mono text-muted-foreground group-hover:text-primary transition-colors"
                  >/</span
                >
                <span class="shrink-0 text-xs font-medium text-foreground">{skill.name}</span>
                {#if skill.description}
                  <span class="min-w-0 truncate text-xs text-muted-foreground">
                    {skill.description}
                  </span>
                {/if}
              </button>
            {/each}
          {/if}

          <!-- Agents group (read-only) -->
          {#if agents.length > 0}
            {#if skills.length > 0}
              <div class="mx-2 my-1 border-t border-border/50"></div>
            {/if}
            <div
              class="px-2.5 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60"
            >
              Agents
            </div>
            {#each agents as agent}
              <div class="flex items-center gap-2 rounded-md px-2.5 py-1.5 opacity-70">
                <!-- Bot icon -->
                <svg
                  class="h-3 w-3 shrink-0 text-muted-foreground"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M12 8V4H8" />
                  <rect width="16" height="12" x="4" y="8" rx="2" />
                  <path d="M2 14h2" />
                  <path d="M20 14h2" />
                  <path d="M15 13v2" />
                  <path d="M9 13v2" />
                </svg>
                <span class="shrink-0 text-xs font-medium text-foreground/70">{agent.name}</span>
                {#if agent.description}
                  <span class="min-w-0 truncate text-xs text-muted-foreground">
                    {agent.description}
                  </span>
                {/if}
              </div>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

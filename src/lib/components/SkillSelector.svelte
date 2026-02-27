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
    disabled = false,
    onSelect,
  }: {
    skills?: SkillItem[];
    disabled?: boolean;
    onSelect?: (name: string) => void;
  } = $props();

  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

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
    class="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] font-medium transition-colors
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
      {#if skills.length === 0}
        <p class="px-3 py-4 text-xs text-muted-foreground text-center">
          {t("skillSelector_empty")}
        </p>
      {:else}
        <div class="p-1">
          {#each skills as skill}
            <button
              class="group flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left hover:bg-accent transition-colors"
              onclick={() => selectSkill(skill.name)}
            >
              <span
                class="shrink-0 text-[11px] font-mono text-muted-foreground/50 group-hover:text-primary transition-colors"
                >/</span
              >
              <span class="shrink-0 text-xs font-medium text-foreground">{skill.name}</span>
              {#if skill.description}
                <span class="min-w-0 truncate text-[11px] text-muted-foreground">
                  {skill.description}
                </span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

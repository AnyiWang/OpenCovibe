<script lang="ts">
  import { onMount } from "svelte";
  import type { PlatformCredential } from "$lib/types";
  import {
    buildPlatformList,
    findCredential,
    PRESET_CATEGORIES,
  } from "$lib/utils/platform-presets";
  import { t } from "$lib/i18n/index.svelte";

  let {
    value = $bindable("anthropic"),
    credentials = [],
    disabled = false,
    onchange,
  }: {
    value: string;
    credentials?: PlatformCredential[];
    disabled?: boolean;
    onchange?: (platformId: string) => void;
  } = $props();

  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  let platforms = $derived(buildPlatformList(credentials));
  let displayName = $derived(platforms.find((p) => p.id === value)?.name ?? value);
  let hasKey = $derived(!!findCredential(credentials, value)?.api_key);
  let selectedCategory = $derived(platforms.find((p) => p.id === value)?.category);
  // Show "no API key" warning for non-local providers that have no key configured
  // (anthropic uses CLI auth so it's excluded)
  let showKeyWarning = $derived(!hasKey && value !== "anthropic" && selectedCategory !== "local");

  /** Group platforms by category for the dropdown. */
  let grouped = $derived.by(() => {
    const groups: { label: string; items: typeof platforms }[] = [];
    for (const cat of PRESET_CATEGORIES) {
      const items = platforms.filter((p) => p.category === cat.id);
      if (items.length > 0) groups.push({ label: cat.label, items });
    }
    return groups;
  });

  function toggleDropdown() {
    if (disabled) return;
    dropdownOpen = !dropdownOpen;
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

  function selectPlatform(id: string) {
    value = id;
    dropdownOpen = false;
    onchange?.(id);
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

<div bind:this={wrapperEl} class="inline-flex items-center gap-1">
  <button
    bind:this={buttonEl}
    class="flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs font-medium transition-colors
      {disabled ? 'opacity-60 cursor-default' : 'hover:bg-accent cursor-pointer'}"
    onclick={toggleDropdown}
    {disabled}
    title={t("prompt_platformTitle")}
  >
    <svg
      class="h-3 w-3 text-muted-foreground"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="M2 20a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8l-7 5V8l-7 5V4a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2Z"
      />
    </svg>
    {displayName}
    {#if !disabled}
      <svg
        class="h-2.5 w-2.5 text-muted-foreground"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
      >
    {/if}
  </button>

  {#if showKeyWarning}
    <a
      href="/settings"
      class="text-[10px] text-amber-500 hover:text-amber-400 hover:underline whitespace-nowrap"
    >
      {t("prompt_noPlatformKey")}
    </a>
  {/if}

  {#if dropdownOpen}
    <div
      class="w-64 rounded-md border bg-background shadow-lg animate-fade-in max-h-80 overflow-y-auto"
      style={dropdownStyle}
    >
      <div class="p-1">
        {#each grouped as group}
          <div
            class="px-2 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60"
          >
            {group.label}
          </div>
          {#each group.items as platform}
            {@const saved = findCredential(credentials, platform.id)}
            {@const platformHasKey = !!saved?.api_key}
            <button
              class="flex w-full items-center gap-2 rounded-sm px-3 py-1.5 text-sm hover:bg-accent transition-colors
                {value === platform.id ? 'bg-accent font-medium' : ''}"
              onclick={() => selectPlatform(platform.id)}
            >
              <span
                class="h-1.5 w-1.5 rounded-full shrink-0 {platformHasKey
                  ? 'bg-green-500'
                  : 'bg-muted-foreground/30'}"
              ></span>
              <span class="flex-1 min-w-0 truncate">{platform.name}</span>
              {#if value === platform.id}
                <svg
                  class="h-3 w-3 ml-auto text-primary shrink-0"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"><path d="M20 6 9 17l-5-5" /></svg
                >
              {/if}
            </button>
          {/each}
        {/each}
      </div>
    </div>
  {/if}
</div>

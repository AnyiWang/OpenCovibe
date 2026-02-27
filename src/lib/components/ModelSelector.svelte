<script lang="ts">
  import { onMount } from "svelte";
  import { getCliModels } from "$lib/stores/cli-info.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let {
    value = $bindable(""),
    agent = "claude",
    onchange,
  }: {
    value: string;
    agent?: string;
    onchange?: (model: string) => void;
  } = $props();

  let showCustom = $state(false);
  let customModel = $state("");

  let models = $derived(getCliModels());

  let displayValue = $derived.by(() => {
    const found = getCliModels().find((mdl) => mdl.value === value);
    return found?.displayName ?? (value || "Default");
  });

  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  function toggleDropdown() {
    dropdownOpen = !dropdownOpen;
    if (dropdownOpen && buttonEl) {
      updateDropdownPosition();
    }
  }

  function updateDropdownPosition() {
    if (!buttonEl) return;
    const rect = buttonEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    if (spaceBelow < 260) {
      dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
    } else {
      dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
    }
  }

  function selectModel(val: string) {
    value = val;
    dropdownOpen = false;
    showCustom = false;
    onchange?.(val);
  }

  function applyCustom() {
    if (customModel.trim()) {
      value = customModel.trim();
      onchange?.(value);
    }
    showCustom = false;
    dropdownOpen = false;
  }

  // Use onMount + explicit addEventListener for click-outside detection
  // This avoids <svelte:window> which can interfere with SvelteKit navigation
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

<div bind:this={wrapperEl}>
  <button
    bind:this={buttonEl}
    class="flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-xs font-medium hover:bg-accent transition-colors"
    onclick={toggleDropdown}
  >
    <svg
      class="h-3.5 w-3.5 text-muted-foreground"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><path d="M12 8V4H8" /><rect width="16" height="12" x="4" y="8" rx="2" /><path
        d="M2 14h2"
      /><path d="M20 14h2" /><path d="M15 13v2" /><path d="M9 13v2" /></svg
    >
    {displayValue}
    <svg
      class="h-3 w-3 text-muted-foreground"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
    >
  </button>

  {#if dropdownOpen}
    <div
      class="w-80 rounded-md border bg-background shadow-lg animate-fade-in"
      style={dropdownStyle}
    >
      <div class="p-1">
        {#each models as mdl}
          <button
            class="flex w-full items-center gap-2 rounded-sm px-3 py-2 text-sm hover:bg-accent transition-colors {value ===
            mdl.value
              ? 'bg-accent font-medium'
              : ''}"
            onclick={() => selectModel(mdl.value)}
          >
            {#if value === mdl.value}
              <svg
                class="h-3.5 w-3.5 text-primary"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"><path d="M20 6 9 17l-5-5" /></svg
              >
            {:else}
              <span class="w-3.5"></span>
            {/if}
            <span>{mdl.displayName}</span>
            <span class="text-xs text-muted-foreground/60">{mdl.description}</span>
            <span class="ml-auto text-xs text-muted-foreground">{mdl.value}</span>
          </button>
        {/each}

        <div class="my-1 border-t"></div>

        {#if showCustom}
          <div class="flex items-center gap-1 px-2 py-1">
            <input
              class="flex-1 rounded-sm border bg-background px-2 py-1 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
              bind:value={customModel}
              placeholder={t("model_placeholder")}
              onkeydown={(e) => e.key === "Enter" && applyCustom()}
            />
            <button
              class="rounded-sm bg-primary px-2 py-1 text-xs text-primary-foreground"
              onclick={applyCustom}
            >
              {t("model_set")}
            </button>
          </div>
        {:else}
          <button
            class="flex w-full items-center gap-2 rounded-sm px-3 py-2 text-sm hover:bg-accent transition-colors text-muted-foreground"
            onclick={() => (showCustom = true)}
          >
            <svg
              class="h-3.5 w-3.5"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"><path d="M12 5v14M5 12h14" /></svg
            >
            {t("model_customModel")}
          </button>
        {/if}
      </div>
    </div>
  {/if}
</div>

<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";

  let { value = $bindable([]) }: { value: string[] } = $props();

  let customTool = $state("");

  const commonTools = [
    { id: "Read", desc: () => t("toolSelector_readDesc") },
    { id: "Write", desc: () => t("toolSelector_writeDesc") },
    { id: "Edit", desc: () => t("toolSelector_editDesc") },
    { id: "Bash", desc: () => t("toolSelector_bashDesc") },
    { id: "Glob", desc: () => t("toolSelector_globDesc") },
    { id: "Grep", desc: () => t("toolSelector_grepDesc") },
    { id: "WebFetch", desc: () => t("toolSelector_webFetchDesc") },
    { id: "WebSearch", desc: () => t("toolSelector_webSearchDesc") },
  ];

  function toggle(tool: string) {
    if (value.includes(tool)) {
      value = value.filter((t) => t !== tool);
    } else {
      value = [...value, tool];
    }
  }

  function addCustom() {
    const t = customTool.trim();
    if (t && !value.includes(t)) {
      value = [...value, t];
    }
    customTool = "";
  }

  function removeCustom(tool: string) {
    value = value.filter((t) => t !== tool);
  }

  let customTools = $derived(value.filter((t) => !commonTools.some((c) => c.id === t)));
</script>

<div class="space-y-3">
  <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
    {#each commonTools as tool}
      <button
        class="flex items-center gap-2 rounded-md border px-3 py-2 text-sm transition-all duration-150
          {value.includes(tool.id)
          ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
          : 'hover:bg-accent hover:border-ring/30'}"
        onclick={() => toggle(tool.id)}
      >
        <div
          class="flex h-4 w-4 items-center justify-center rounded border {value.includes(tool.id)
            ? 'bg-primary border-primary'
            : 'border-muted-foreground/30'}"
        >
          {#if value.includes(tool.id)}
            <svg
              class="h-3 w-3 text-primary-foreground"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="3"><path d="M20 6 9 17l-5-5" /></svg
            >
          {/if}
        </div>
        <div class="text-left">
          <div class="font-medium">{tool.id}</div>
          <div class="text-xs text-muted-foreground">{tool.desc()}</div>
        </div>
      </button>
    {/each}
  </div>

  {#if customTools.length > 0}
    <div class="flex flex-wrap gap-1.5">
      {#each customTools as tool}
        <span class="inline-flex items-center gap-1 rounded-full bg-secondary px-2.5 py-1 text-xs">
          {tool}
          <button
            class="hover:text-destructive transition-colors"
            onclick={() => removeCustom(tool)}
          >
            <svg
              class="h-3 w-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
            >
          </button>
        </span>
      {/each}
    </div>
  {/if}

  <div class="flex items-center gap-2">
    <input
      class="flex-1 rounded-md border bg-background px-3 py-1.5 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
      bind:value={customTool}
      placeholder={t("toolSelector_addCustomPlaceholder")}
      onkeydown={(e) => e.key === "Enter" && addCustom()}
    />
    <button
      class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent transition-colors disabled:opacity-50"
      onclick={addCustom}
      disabled={!customTool.trim()}
    >
      {t("toolSelector_add")}
    </button>
  </div>
</div>

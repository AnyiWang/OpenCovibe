<script lang="ts">
  let {
    value = $bindable("claude"),
    class: className = "",
    onchange,
  }: {
    value?: string;
    class?: string;
    onchange?: (agent: string) => void;
  } = $props();

  let open = $state(false);

  const agents = [
    { id: "claude", label: "Claude" },
    // { id: "codex", label: "Codex" }, // Codex disabled
  ];

  let currentLabel = $derived(agents.find((a) => a.id === value)?.label ?? "Claude");

  function select(id: string) {
    value = id;
    open = false;
    onchange?.(id);
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest("[data-agent-selector]")) {
      open = false;
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div class="relative {className}" data-agent-selector>
  <button
    class="flex items-center gap-1 rounded-lg border border-border bg-muted px-2.5 py-1 text-xs font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
    onclick={() => (open = !open)}
  >
    {currentLabel}
    <svg
      class="h-3 w-3 opacity-50"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg
    >
  </button>

  {#if open}
    <div
      class="absolute bottom-full left-0 mb-1 min-w-[120px] rounded-xl border border-border bg-background py-1 shadow-lg animate-fade-in z-50"
    >
      {#each agents as agent}
        <button
          class="flex w-full items-center gap-2 px-3 py-1.5 text-xs transition-colors
            {value === agent.id
            ? 'text-foreground bg-accent'
            : 'text-muted-foreground hover:text-foreground hover:bg-accent/50'}"
          onclick={() => select(agent.id)}
        >
          {agent.label}
          {#if value === agent.id}
            <svg
              class="ml-auto h-3 w-3 text-muted-foreground"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
            >
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

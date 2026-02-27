<script lang="ts">
  import { cwdDisplayLabel } from "$lib/utils/format";
  import { t } from "$lib/i18n/index.svelte";

  let {
    projects,
    value = $bindable(""),
    onOpenFolder,
    onchange,
  }: {
    projects: { cwd: string; label: string; runCount: number }[];
    value?: string;
    onOpenFolder?: () => void;
    onchange?: (cwd: string) => void;
  } = $props();

  let open = $state(false);
  let triggerEl = $state<HTMLButtonElement | null>(null);
  let dropdownStyle = $state("");

  let displayLabel = $derived(value ? cwdDisplayLabel(value) : t("project_allProjects"));

  function toggle() {
    if (!open && triggerEl) {
      const rect = triggerEl.getBoundingClientRect();
      dropdownStyle = `position:fixed;top:${rect.bottom + 4}px;left:${rect.left}px;width:${rect.width}px;`;
    }
    open = !open;
  }

  function select(cwd: string) {
    value = cwd;
    open = false;
    onchange?.(cwd);
  }

  function handleOpenFolder() {
    open = false;
    onOpenFolder?.();
  }
</script>

<div>
  <!-- Trigger -->
  <button
    bind:this={triggerEl}
    class="flex w-full items-center gap-2 rounded-md px-2 py-1.5
      transition-colors duration-150 hover:bg-sidebar-accent/50
      {open ? 'bg-sidebar-accent/40' : ''}"
    onclick={toggle}
    title={value || t("project_allProjects")}
  >
    <svg
      class="h-4 w-4 shrink-0 text-primary/70"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><path
        d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
      /></svg
    >
    <span class="flex-1 min-w-0 truncate text-sm font-medium text-sidebar-foreground"
      >{displayLabel}</span
    >
    <svg
      class="ml-auto h-3.5 w-3.5 shrink-0 text-muted-foreground/50 transition-transform duration-200"
      class:rotate-180={open}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg
    >
  </button>

  <!-- Fixed-position dropdown (escapes overflow-hidden ancestors) -->
  {#if open}
    <!-- Backdrop -->
    <div class="fixed inset-0 z-40" onclick={() => (open = false)}></div>

    <div
      class="z-50 max-h-[60vh] overflow-y-auto rounded-md border border-sidebar-border bg-sidebar py-1 shadow-lg"
      style={dropdownStyle}
    >
      <!-- All Projects -->
      <button
        class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-sm transition-colors duration-100
          {value === ''
          ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
          : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
        onclick={() => select("")}
      >
        <svg
          class="h-3.5 w-3.5 shrink-0 opacity-60"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><circle cx="12" cy="12" r="10" /><path d="M2 12h20" /><path
            d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"
          /></svg
        >
        <span class="flex-1 min-w-0 truncate">{t("project_allProjects")}</span>
        {#if value === ""}
          <svg
            class="ml-auto h-3 w-3 shrink-0 text-primary"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
        {/if}
      </button>

      <!-- Project list -->
      {#each projects as project}
        <button
          class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-sm transition-colors duration-100
            {value === project.cwd
            ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
            : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'}"
          onclick={() => select(project.cwd)}
          title={project.cwd}
        >
          <svg
            class="h-3.5 w-3.5 shrink-0 opacity-60"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path
              d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
            /></svg
          >
          <span class="flex-1 min-w-0 truncate">{project.label}</span>
          {#if value === project.cwd}
            <svg
              class="ml-auto h-3 w-3 shrink-0 text-primary"
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

      <!-- Divider -->
      <div class="my-1 border-t border-sidebar-border"></div>

      <!-- Open folder -->
      <button
        class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-sm text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground transition-colors duration-100"
        onclick={handleOpenFolder}
      >
        <svg
          class="h-3.5 w-3.5 shrink-0 opacity-60"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><path
            d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"
          /><path d="M12 10v6" /><path d="m9 13 3-3 3 3" /></svg
        >
        <span class="flex-1 min-w-0 truncate">{t("project_openFolder")}</span>
      </button>
    </div>
  {/if}
</div>

<script lang="ts">
  import Modal from "./Modal.svelte";
  import * as api from "$lib/api";
  import { t } from "$lib/i18n/index.svelte";

  let {
    open = $bindable(false),
    title = "Git Diff",
  }: {
    open: boolean;
    title?: string;
  } = $props();

  let tab = $state<"unstaged" | "staged">("unstaged");
  let diff = $state("");
  let loading = $state(false);

  async function loadDiff(staged: boolean) {
    const cwd =
      typeof window !== "undefined" ? localStorage.getItem("ocv:project-cwd") || "/" : "/";
    loading = true;
    try {
      diff = await api.getGitDiff(cwd, staged);
    } catch (e) {
      diff = String(e);
    } finally {
      loading = false;
    }
  }

  // Load diff when opened or tab changes
  $effect(() => {
    if (open) {
      loadDiff(tab === "staged");
    }
  });
</script>

<Modal bind:open {title}>
  <div class="space-y-3">
    <div class="flex gap-1 border-b">
      <button
        class="px-3 py-1.5 text-sm transition-colors {tab === 'unstaged'
          ? 'border-b-2 border-primary font-medium'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (tab = "unstaged")}
      >
        {t("diff_unstaged")}
      </button>
      <button
        class="px-3 py-1.5 text-sm transition-colors {tab === 'staged'
          ? 'border-b-2 border-primary font-medium'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (tab = "staged")}
      >
        {t("diff_staged")}
      </button>
    </div>

    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div
          class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
        ></div>
      </div>
    {:else if diff.trim()}
      <pre
        class="max-h-[60vh] overflow-auto rounded-lg bg-muted/50 p-4 text-xs font-mono leading-relaxed">{#each diff.split("\n") as line}{#if line.startsWith("+") && !line.startsWith("+++")}<span
              class="text-green-600 dark:text-green-400">{line}</span
            >
          {:else if line.startsWith("-") && !line.startsWith("---")}<span
              class="text-red-500 dark:text-red-400">{line}</span
            >
          {:else if line.startsWith("@@")}<span class="text-blue-500 dark:text-blue-400"
              >{line}</span
            >
          {:else if line.startsWith("diff ")}<span class="font-bold">{line}</span>
          {:else}{line}
          {/if}{/each}</pre>
    {:else}
      <div class="flex flex-col items-center gap-2 py-8 text-center">
        <svg
          class="h-8 w-8 text-muted-foreground/40"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"><path d="M20 6 9 17l-5-5" /></svg
        >
        <p class="text-sm text-muted-foreground">{t("diff_noChanges")}</p>
      </div>
    {/if}
  </div>
</Modal>

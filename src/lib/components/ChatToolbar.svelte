<script lang="ts">
  import ModelSelector from "./ModelSelector.svelte";
  import DiffModal from "./DiffModal.svelte";
  import * as api from "$lib/api";
  import { dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  import { PLATFORM_PRESETS } from "$lib/utils/platform-presets";

  let {
    agent = "claude",
    runId = "",
    planMode = $bindable(false),
    onSendPrompt,
    onOpenPalette,
    platformId = "",
    authMode = "cli",
  }: {
    agent?: string;
    runId?: string;
    planMode?: boolean;
    onSendPrompt?: (prompt: string) => void;
    onOpenPalette?: () => void;
    platformId?: string;
    authMode?: string;
  } = $props();

  let platformName = $derived(
    platformId ? (PLATFORM_PRESETS.find((p) => p.id === platformId)?.name ?? platformId) : "",
  );

  let model = $state("");
  let diffOpen = $state(false);
  let costDisplay = $state("");
  let exporting = $state(false);

  // Load agent settings on mount and when agent changes
  $effect(() => {
    const a = agent; // track this reactive prop
    api
      .getAgentSettings(a)
      .then((s) => {
        model = s.model ?? "";
        planMode = s.plan_mode ?? false;
      })
      .catch((e) => dbgWarn("toolbar", "failed to load agent settings:", e));
  });

  // Load cost info when runId changes
  $effect(() => {
    const rid = runId;
    if (rid) {
      api
        .getRunArtifacts(rid)
        .then((a) => {
          if (a.cost_estimate != null) {
            costDisplay = `$${a.cost_estimate.toFixed(4)}`;
          } else {
            costDisplay = "";
          }
        })
        .catch(() => {
          costDisplay = "";
        });
    } else {
      costDisplay = "";
    }
  });

  async function handleModelChange(newModel: string) {
    model = newModel;
    try {
      await api.updateAgentSettings(agent, { model: newModel || undefined });
    } catch (e) {
      dbgWarn("toolbar", "failed to update model:", e);
    }
  }

  async function togglePlan() {
    planMode = !planMode;
    try {
      await api.updateAgentSettings(agent, { plan_mode: planMode });
    } catch (e) {
      dbgWarn("toolbar", "failed to toggle plan mode:", e);
      planMode = !planMode;
    }
  }

  async function handleCompact() {
    onSendPrompt?.("/compact");
  }

  async function handleReview() {
    onSendPrompt?.(
      "Review my recent changes. Look at the git diff and provide feedback on code quality, potential bugs, and improvements.",
    );
  }

  async function handleExport() {
    if (!runId) return;
    exporting = true;
    try {
      const md = await api.exportConversation(runId);
      // Use Tauri dialog to save
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: `conversation-${runId.slice(0, 8)}.md`,
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (path) {
        await api.writeTextFile(path, md);
      }
    } catch (e) {
      dbgWarn("toolbar", "export failed:", e);
    } finally {
      exporting = false;
    }
  }

  let isClaude = $derived(agent === "claude");
</script>

<div class="flex flex-wrap items-center gap-1.5 px-6 py-2 border-b bg-muted/20">
  <!-- Model selector -->
  <ModelSelector bind:value={model} {agent} onchange={handleModelChange} />

  <!-- Platform label (API mode, read-only) -->
  {#if authMode === "api" && platformId && platformId !== "anthropic"}
    <span class="text-xs text-muted-foreground px-1 truncate max-w-[80px]">{platformName}</span>
  {/if}

  <!-- Plan mode (Claude only) -->
  {#if isClaude}
    <button
      class="flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors
        {planMode
        ? 'bg-amber-500/10 border-amber-500/30 text-amber-700 dark:text-amber-400'
        : 'hover:bg-accent'}"
      onclick={togglePlan}
      title={t("toolbar_planModeTitle")}
    >
      <svg
        class="h-3.5 w-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" /><path
          d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"
        /></svg
      >
      {planMode ? t("toolbar_planOn") : t("toolbar_planOff")}
    </button>
  {/if}

  <div class="h-4 w-px bg-border mx-0.5"></div>

  <!-- Compact (Claude only) -->
  {#if isClaude}
    <button
      class="flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent transition-colors"
      onclick={handleCompact}
      title={t("toolbar_compactTitle")}
    >
      <svg
        class="h-3.5 w-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><path d="m7 20 5-5 5 5" /><path d="m7 4 5 5 5-5" /></svg
      >
      {t("toolbar_compact")}
    </button>
  {/if}

  <!-- Review (Claude only) -->
  {#if isClaude}
    <button
      class="flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent transition-colors"
      onclick={handleReview}
      title={t("toolbar_reviewTitle")}
    >
      <svg
        class="h-3.5 w-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z" /><path
          d="M14 2v6h6"
        /><path d="m9 15 2 2 4-4" /></svg
      >
      {t("toolbar_review")}
    </button>
  {/if}

  <!-- Diff -->
  <button
    class="flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent transition-colors"
    onclick={() => (diffOpen = true)}
    title={t("toolbar_diffTitle")}
  >
    <svg
      class="h-3.5 w-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><path d="M16 3h5v5" /><path d="M8 3H3v5" /><path
        d="M12 22v-8.3a4 4 0 0 0-1.172-2.872L3 3"
      /><path d="m15 9 6-6" /></svg
    >
    {t("toolbar_diff")}
  </button>

  <!-- Export -->
  <button
    class="flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent transition-colors disabled:opacity-50"
    onclick={handleExport}
    disabled={!runId || exporting}
    title={t("toolbar_exportTitle")}
  >
    <svg
      class="h-3.5 w-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline
        points="7 10 12 15 17 10"
      /><line x1="12" x2="12" y1="15" y2="3" /></svg
    >
    {t("toolbar_export")}
  </button>

  <div class="flex-1"></div>

  <!-- Cost display -->
  {#if costDisplay}
    <span class="text-xs text-muted-foreground font-mono">{costDisplay}</span>
  {/if}

  <!-- Command palette trigger -->
  <button
    class="flex items-center gap-1 rounded-md border px-2 py-1.5 text-xs font-medium hover:bg-accent transition-colors text-muted-foreground"
    onclick={onOpenPalette}
    title={t("toolbar_paletteTitle")}
  >
    <svg
      class="h-3.5 w-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      ><polyline points="4 17 10 11 4 5" /><line x1="12" x2="20" y1="19" y2="19" /></svg
    >
    <kbd class="text-[10px] opacity-60">&#8984;K</kbd>
  </button>
</div>

<DiffModal bind:open={diffOpen} />

<script lang="ts">
  import * as api from "$lib/api";
  import type { CodexFeature } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let {
    runId,
    sessionAlive = false,
    onClose,
  }: {
    runId: string;
    sessionAlive?: boolean;
    onClose: () => void;
  } = $props();

  let loading = $state(false);
  let features = $state<CodexFeature[]>([]);
  let togglingName = $state<string | null>(null);
  let error = $state("");
  let loaded = $state(false);

  // Stage policy: which lifecycle stages a user is allowed to toggle via config.
  // - beta / stable: real user-facing flags → render with a toggle (primary).
  // - underDevelopment / deprecated: shown read-only (visible for transparency,
  //   but writing `[features].<name>` for a half-built or sunsetting flag would
  //   set an option the CLI isn't ready to honor) — no toggle.
  // - removed: the flag no longer does anything → hidden entirely.
  function isToggleable(stage: string): boolean {
    return stage === "beta" || stage === "stable";
  }

  // Toggleable (beta first, then stable) followed by read-only dev/deprecated.
  // `removed` is dropped. Stable sort within a group keeps Codex's order.
  const STAGE_ORDER: Record<string, number> = {
    beta: 0,
    stable: 1,
    underDevelopment: 2,
    deprecated: 3,
  };
  const visibleFeatures = $derived(
    features
      .filter((f) => f.stage !== "removed")
      .slice()
      .sort((a, b) => (STAGE_ORDER[a.stage] ?? 9) - (STAGE_ORDER[b.stage] ?? 9)),
  );

  function stageBadgeClass(stage: string): string {
    switch (stage) {
      case "beta":
        return "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/30";
      case "stable":
        return "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/30";
      case "underDevelopment":
        return "bg-sky-500/10 text-sky-600 dark:text-sky-400 border-sky-500/30";
      case "deprecated":
        return "bg-muted text-muted-foreground border-border";
      default:
        return "bg-muted text-muted-foreground border-border";
    }
  }

  function stageLabel(stage: string): string {
    switch (stage) {
      case "beta":
        return t("features_stageBeta");
      case "stable":
        return t("features_stageStable");
      case "underDevelopment":
        return t("features_stageDev");
      case "deprecated":
        return t("features_stageDeprecated");
      default:
        return stage;
    }
  }

  async function refresh() {
    if (!sessionAlive) return;
    loading = true;
    error = "";
    try {
      dbg("features", "list", { runId });
      const res = await api.listCodexFeatures(runId);
      features = res.data ?? [];
      loaded = true;
      dbg("features", "listed", { count: features.length });
    } catch (e) {
      dbgWarn("features", "list failed", e);
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function toggle(feature: CodexFeature) {
    if (!isToggleable(feature.stage)) return;
    const newEnabled = !feature.enabled;
    togglingName = feature.name;
    error = "";
    try {
      dbg("features", "toggle", { name: feature.name, enabled: newEnabled });
      await api.setCodexFeature(feature.name, newEnabled);
      // Optimistic — durable write takes effect next session, not live.
      features = features.map((f) => (f.name === feature.name ? { ...f, enabled: newEnabled } : f));
    } catch (e) {
      dbgWarn("features", "toggle failed", e);
      error = String(e);
    } finally {
      togglingName = null;
    }
  }

  // Load once when the panel mounts with a live session.
  $effect(() => {
    if (sessionAlive && !loaded && !loading) {
      void refresh();
    }
  });
</script>

<div class="rounded-lg border border-border bg-background shadow-lg w-96 animate-fade-in">
  <!-- Header -->
  <div class="flex items-center justify-between px-3 py-2 border-b border-border">
    <span class="text-xs font-semibold text-foreground">{t("features_title")}</span>
    <div class="flex items-center gap-1">
      <button
        class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors disabled:opacity-50"
        disabled={loading || !sessionAlive}
        onclick={refresh}
        title={t("features_refresh")}
      >
        <svg
          class="h-3.5 w-3.5 {loading ? 'animate-spin' : ''}"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
          <path d="M3 3v5h5" />
          <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
          <path d="M16 16h5v5" />
        </svg>
      </button>
      <button
        class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
        onclick={onClose}
        title={t("common_close")}
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
          <path d="M18 6 6 18" /><path d="m6 6 12 12" />
        </svg>
      </button>
    </div>
  </div>

  <!-- "Next session" hint -->
  <div class="px-3 py-1.5 border-b border-border/50 text-[10px] text-muted-foreground">
    {t("features_nextSessionHint")}
  </div>

  <!-- Feature list -->
  <div class="max-h-80 overflow-y-auto">
    {#if !sessionAlive}
      <div class="px-3 py-4 text-center text-xs text-muted-foreground">
        {t("features_noSession")}
      </div>
    {:else if loading && !loaded}
      <div class="px-3 py-4 text-center text-xs text-muted-foreground">
        {t("features_loading")}
      </div>
    {:else if visibleFeatures.length === 0}
      <div class="px-3 py-4 text-center text-xs text-muted-foreground">
        {t("features_none")}
      </div>
    {:else}
      {#each visibleFeatures as feature (feature.name)}
        {@const toggleable = isToggleable(feature.stage)}
        {@const overridden = feature.enabled !== feature.defaultEnabled}
        <div class="flex items-start gap-2 px-3 py-2 border-b border-border/50 last:border-b-0">
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-1.5 flex-wrap">
              <span class="text-xs font-medium text-foreground">
                {feature.displayName || feature.name}
              </span>
              <span
                class="rounded-full border px-1.5 py-px text-[9px] font-medium uppercase tracking-wide {stageBadgeClass(
                  feature.stage,
                )}"
              >
                {stageLabel(feature.stage)}
              </span>
              {#if overridden}
                <span
                  class="rounded-full border border-border bg-muted px-1.5 py-px text-[9px] font-medium text-muted-foreground"
                  title={t("features_overriddenTitle")}
                >
                  {t("features_overridden")}
                </span>
              {/if}
            </div>
            {#if feature.description}
              <div class="mt-0.5 text-[10px] leading-snug text-muted-foreground">
                {feature.description}
              </div>
            {/if}
          </div>

          <!-- Toggle (beta/stable) or read-only state pill (dev/deprecated) -->
          <div class="shrink-0 pt-0.5">
            {#if toggleable}
              <button
                type="button"
                role="switch"
                aria-checked={feature.enabled}
                disabled={togglingName === feature.name}
                onclick={() => toggle(feature)}
                title={feature.enabled ? t("features_disable") : t("features_enable")}
                class="relative inline-flex h-4 w-7 items-center rounded-full transition-colors disabled:opacity-50 {feature.enabled
                  ? 'bg-emerald-500'
                  : 'bg-muted-foreground/30'}"
              >
                <span
                  class="inline-block h-3 w-3 transform rounded-full bg-white shadow transition-transform {feature.enabled
                    ? 'translate-x-3.5'
                    : 'translate-x-0.5'}"
                ></span>
              </button>
            {:else}
              <span
                class="text-[10px] font-medium {feature.enabled
                  ? 'text-emerald-600 dark:text-emerald-400'
                  : 'text-muted-foreground'}"
                title={t("features_readonlyTitle")}
              >
                {feature.enabled ? t("features_on") : t("features_off")}
              </span>
            {/if}
          </div>
        </div>
      {/each}
    {/if}
  </div>

  <!-- Error -->
  {#if error}
    <div class="px-3 py-2 border-t border-destructive/20 bg-destructive/5 text-xs text-destructive">
      {error}
    </div>
  {/if}
</div>

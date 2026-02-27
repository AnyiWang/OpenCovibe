<script lang="ts">
  /**
   * Renders /cost output with styled terminal-like layout.
   * Parses the plain-text output from CLI's non-interactive /cost handler.
   */
  import { goto } from "$app/navigation";
  import { dbg } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let { text }: { text: string } = $props();

  // ── Parse cost output ──

  interface CostData {
    totalCost: string;
    durationApi: string;
    durationWall: string;
    linesAdded: string;
    linesRemoved: string;
    models: Array<{
      name: string;
      input: string;
      output: string;
      cacheRead: string;
      cacheWrite: string;
      webSearch?: string;
      cost: string;
    }>;
  }

  function parseCostText(raw: string): CostData | null {
    try {
      // Strip ANSI escape codes for parsing
      // eslint-disable-next-line no-control-regex
      const t = raw.replace(/\x1b\[[0-9;]*m/g, "").trim();

      const costMatch = t.match(/Total cost:\s*(\$[\d.]+)/);
      const apiMatch = t.match(/Total duration \(API\):\s*(.+)/);
      const wallMatch = t.match(/Total duration \(wall\):\s*(.+)/);
      const changesMatch = t.match(
        /Total code changes:\s*(\d+)\s*lines?\s*added,\s*(\d+)\s*lines?\s*removed/,
      );

      if (!costMatch) return null;

      const models: CostData["models"] = [];
      // Per-model lines: "  claude-opus-4-6:  1.2k input, 450k output, 0 cache read, 0 cache write ($0.28)"
      const modelRegex =
        /^\s*(\S+):\s+([\d.]+\w*)\s+input,\s+([\d.]+\w*)\s+output,\s+([\d.]+\w*)\s+cache read,\s+([\d.]+\w*)\s+cache write(?:,\s+([\d.]+\w*)\s+web search)?\s+\((\$[\d.]+)\)/gm;
      let mr;
      while ((mr = modelRegex.exec(t)) !== null) {
        models.push({
          name: mr[1],
          input: mr[2],
          output: mr[3],
          cacheRead: mr[4],
          cacheWrite: mr[5],
          webSearch: mr[6] || undefined,
          cost: mr[7],
        });
      }

      dbg("cost-view", "parsed", { cost: costMatch[1], models: models.length });

      return {
        totalCost: costMatch[1],
        durationApi: apiMatch?.[1]?.trim() ?? "—",
        durationWall: wallMatch?.[1]?.trim() ?? "—",
        linesAdded: changesMatch?.[1] ?? "0",
        linesRemoved: changesMatch?.[2] ?? "0",
        models,
      };
    } catch {
      return null;
    }
  }

  // Model color palette
  const MODEL_COLORS = ["#a78bfa", "#60a5fa", "#34d399", "#fbbf24", "#f87171", "#f472b6"];
  function modelColor(idx: number): string {
    return MODEL_COLORS[idx % MODEL_COLORS.length];
  }

  let parsed = $derived(parseCostText(text));
</script>

{#if parsed}
  <div class="font-mono text-xs leading-relaxed">
    <!-- Header -->
    <div class="mb-3 text-sm font-bold text-foreground">{t("cost_sessionCost")}</div>

    <!-- Key-value pairs -->
    <div class="grid gap-y-1" style="grid-template-columns: auto 1fr;">
      <span class="text-muted-foreground pr-4">{t("cost_totalCost")}</span>
      <span class="text-emerald-400 font-bold">{parsed.totalCost}</span>

      <span class="text-muted-foreground pr-4">{t("cost_durationApi")}</span>
      <span class="text-foreground">{parsed.durationApi}</span>

      <span class="text-muted-foreground pr-4">{t("cost_durationWall")}</span>
      <span class="text-foreground">{parsed.durationWall}</span>

      <span class="text-muted-foreground pr-4">{t("cost_codeChanges")}</span>
      <span class="text-foreground">
        <span class="text-green-400">+{parsed.linesAdded}</span>
        <span class="text-muted-foreground">/</span>
        <span class="text-red-400">-{parsed.linesRemoved}</span>
        <span class="text-muted-foreground ml-1">{t("cost_lines")}</span>
      </span>
    </div>

    <!-- Per-model breakdown -->
    {#if parsed.models.length > 0}
      <div class="mt-3 pt-2 border-t border-border/20">
        <div class="mb-1.5 text-muted-foreground italic">{t("cost_usageByModel")}</div>
        <div class="flex flex-col gap-1.5">
          {#each parsed.models as model, idx}
            <div class="flex flex-col gap-0.5">
              <div class="flex items-center gap-1.5">
                <span
                  class="inline-block w-2 h-2 rounded-full flex-shrink-0"
                  style="background-color: {modelColor(idx)}"
                ></span>
                <span class="text-foreground font-medium">{model.name}</span>
                <span class="text-emerald-400 ml-auto">{model.cost}</span>
              </div>
              <div class="ml-3.5 text-muted-foreground">
                {model.input} in · {model.output} out · {model.cacheRead} cache r · {model.cacheWrite}
                cache w{#if model.webSearch}
                  · {model.webSearch} {t("cost_web")}{/if}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Link to usage page -->
    <div class="mt-3 pt-2 border-t border-border/20">
      <button
        class="flex items-center gap-1 text-xs text-primary/70 hover:text-primary transition-colors"
        onclick={() => goto("/usage")}
      >
        {t("cost_viewDetailed")}
        <svg
          class="h-3 w-3"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
        >
      </button>
    </div>
  </div>
{/if}

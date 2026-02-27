<script lang="ts">
  /**
   * Renders a colored context-usage grid matching the Claude CLI's /context TUI.
   * Input: raw markdown text from CLI's non-interactive /context handler.
   */
  import { t } from "$lib/i18n/index.svelte";
  import {
    parseContextMarkdown,
    getColor,
    getIcon,
    type ContextData,
  } from "$lib/utils/context-parser";

  let { text }: { text: string } = $props();

  // ── Build grid ──

  const COLS = 10;

  interface Cell {
    icon: string;
    color: string;
    category: string;
  }

  function buildGrid(data: ContextData): Cell[] {
    const cells: Cell[] = [];
    let allocated = 0;

    for (let i = 0; i < data.categories.length; i++) {
      const cat = data.categories[i];
      let count: number;
      if (i === data.categories.length - 1) {
        count = 100 - allocated;
      } else {
        count = Math.round((cat.percentage / 100) * 100);
        if (count === 0 && cat.percentage > 0) count = 1;
      }
      allocated += count;

      for (let j = 0; j < count; j++) {
        cells.push({ icon: getIcon(cat.name), color: getColor(cat.name), category: cat.name });
      }
    }
    return cells;
  }

  // ── Reactive ──

  let parsed = $derived(parseContextMarkdown(text));
  let cells = $derived(parsed ? buildGrid(parsed) : []);
  let rows = $derived.by(() => {
    const r: Cell[][] = [];
    const total = Math.ceil(cells.length / COLS);
    for (let i = 0; i < total; i++) r.push(cells.slice(i * COLS, (i + 1) * COLS));
    return r;
  });

  // Track which sub-tables are expanded
  let expandedSections = $state<Set<string>>(new Set());

  function toggleSection(title: string) {
    const next = new Set(expandedSections);
    if (next.has(title)) next.delete(title);
    else next.add(title);
    expandedSections = next;
  }
</script>

{#if parsed}
  <div class="font-mono text-xs leading-relaxed">
    <!-- Title -->
    <div class="mb-2 text-sm font-bold text-foreground">{t("context_title")}</div>

    <!-- Grid (left) + Info (right) side by side -->
    <div class="flex gap-6">
      <!-- Left: icon grid -->
      <div class="flex flex-col flex-shrink-0">
        {#each rows as row}
          <div class="flex gap-px">
            {#each row as cell}
              <span
                class="inline-block w-[1.3em] text-center"
                style="color: {cell.color}"
                title={cell.category}>{cell.icon}</span
              >
            {/each}
          </div>
        {/each}
      </div>

      <!-- Right: model info + category legend -->
      <div class="flex flex-col gap-0.5 pt-0">
        <!-- Model summary (aligned with first row) -->
        <div class="text-muted-foreground">
          {parsed.model} · {parsed.usedTokens}/{parsed.maxTokens} tokens ({parsed.percentage}%)
        </div>
        <!-- Blank line -->
        <div class="h-[1.2em]"></div>
        <!-- Category subtitle -->
        <div class="text-muted-foreground italic">{t("context_estimatedUsage")}</div>
        <!-- Per-category breakdown -->
        {#each parsed.categories as cat}
          <div class="flex items-center gap-1.5">
            <span style="color: {getColor(cat.name)}">{getIcon(cat.name)}</span>
            <span class="text-muted-foreground">
              {cat.name}: {cat.tokens} tokens ({cat.percentage}%)
            </span>
          </div>
        {/each}
      </div>
    </div>

    <!-- Sub-tables (MCP Tools, Memory Files, Skills, etc.) -->
    {#if parsed.subTables.length > 0}
      <div class="mt-3 flex flex-col gap-1">
        {#each parsed.subTables as table}
          <div>
            <button
              class="flex items-center gap-1 text-[11px] text-muted-foreground/70 hover:text-muted-foreground transition-colors"
              onclick={() => toggleSection(table.title)}
            >
              <svg
                class="h-3 w-3 transition-transform {expandedSections.has(table.title)
                  ? 'rotate-90'
                  : ''}"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
              >
              {table.title}
              <span class="text-muted-foreground/40">({table.rows.length})</span>
            </button>
            {#if expandedSections.has(table.title)}
              <div class="ml-4 mt-0.5">
                <table class="text-[11px] text-muted-foreground/70">
                  <thead>
                    <tr>
                      {#each table.headers as header}
                        <th class="pr-4 pb-0.5 text-left font-medium text-muted-foreground/50"
                          >{header}</th
                        >
                      {/each}
                    </tr>
                  </thead>
                  <tbody>
                    {#each table.rows as row}
                      <tr>
                        {#each row as cell, i}
                          <td class="pr-4 py-0 {i === 0 ? 'max-w-[200px] truncate' : ''}">{cell}</td
                          >
                        {/each}
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

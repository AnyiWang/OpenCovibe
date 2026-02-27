<script lang="ts">
  import { getContext } from "svelte";
  import { KeybindingStore, formatKeyDisplay } from "$lib/stores/keybindings.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  const keybindingStore = getContext<KeybindingStore>("keybindings");

  let panelEl: HTMLDivElement | undefined = $state();

  // Window capture-phase keydown: intercepts ALL keys before layout dispatch
  $effect(() => {
    if (!open) return;
    requestAnimationFrame(() => panelEl?.focus());

    function captureKeydown(e: KeyboardEvent) {
      e.stopPropagation();
      e.preventDefault();
      if (e.key === "Escape") {
        open = false;
      }
    }
    window.addEventListener("keydown", captureKeydown, true);
    return () => window.removeEventListener("keydown", captureKeydown, true);
  });

  let globalBindings = $derived(
    keybindingStore.resolved.filter((b) => b.context === "global" && b.source === "app"),
  );
  let chatBindings = $derived(
    keybindingStore.resolved.filter((b) => b.context === "chat" && b.source === "app"),
  );
  let promptBindings = $derived(
    keybindingStore.resolved.filter((b) => b.context === "prompt" && b.source === "app"),
  );
  let cliBindings = $derived(keybindingStore.resolved.filter((b) => b.source === "cli"));

  let cliExpanded = $state(false);
</script>

{#if open}
  <!-- Backdrop -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm animate-fade-in"
    onclick={() => (open = false)}
  >
    <!-- Panel -->
    <div
      bind:this={panelEl}
      tabindex="-1"
      role="dialog"
      aria-modal="true"
      class="w-full max-w-md rounded-xl border border-border bg-background shadow-2xl outline-none animate-slide-up"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Header -->
      <div class="flex items-center justify-between border-b border-border px-5 py-3">
        <h2 class="text-sm font-semibold text-foreground">{t("shortcutHelp_title")}</h2>
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={() => (open = false)}
        >
          <svg
            class="h-4 w-4"
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

      <!-- Body -->
      <div class="max-h-[60vh] overflow-y-auto px-5 py-4 space-y-5">
        <!-- Global -->
        {#if globalBindings.length > 0}
          <section>
            <h3
              class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-2"
            >
              {t("shortcutHelp_global")}
            </h3>
            <div class="space-y-1">
              {#each globalBindings as b (b.command)}
                <div class="flex items-center justify-between py-0.5">
                  <span class="text-xs text-foreground/80">{b.label}</span>
                  <kbd
                    class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                    >{formatKeyDisplay(b.key)}</kbd
                  >
                </div>
              {/each}
            </div>
          </section>
        {/if}

        <!-- Chat -->
        {#if chatBindings.length > 0}
          <section>
            <h3
              class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-2"
            >
              {t("shortcutHelp_chat")}
            </h3>
            <div class="space-y-1">
              {#each chatBindings as b (b.command)}
                <div class="flex items-center justify-between py-0.5">
                  <span class="text-xs text-foreground/80">{b.label}</span>
                  <kbd
                    class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                    >{formatKeyDisplay(b.key)}</kbd
                  >
                </div>
              {/each}
            </div>
          </section>
        {/if}

        <!-- Input -->
        <section>
          <h3 class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-2">
            {t("shortcutHelp_input")}
          </h3>
          <div class="space-y-1">
            {#each promptBindings as b (b.command)}
              <div class="flex items-center justify-between py-0.5">
                <span class="text-xs text-foreground/80">{b.label}</span>
                <kbd
                  class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                  >{formatKeyDisplay(b.key)}</kbd
                >
              </div>
            {/each}
            <!-- Custom hints -->
            <div class="flex items-center justify-between py-0.5">
              <span class="text-xs text-foreground/80">{t("shortcutHelp_hintSlash")}</span>
              <kbd
                class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                >/</kbd
              >
            </div>
            <div class="flex items-center justify-between py-0.5">
              <span class="text-xs text-foreground/80">{t("shortcutHelp_hintAt")}</span>
              <kbd
                class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                >@</kbd
              >
            </div>
            <div class="flex items-center justify-between py-0.5">
              <span class="text-xs text-foreground/80">{t("shortcutHelp_hintDoubleEsc")}</span>
              <kbd
                class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                >⎋ ⎋</kbd
              >
            </div>
            <div class="flex items-center justify-between py-0.5">
              <span class="text-xs text-foreground/80">{t("shortcutHelp_hintNewline")}</span>
              <kbd
                class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/70"
                >⇧↵</kbd
              >
            </div>
          </div>
        </section>

        <!-- CLI Reference (collapsible) -->
        {#if cliBindings.length > 0}
          <section>
            <button
              class="flex w-full items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground transition-colors"
              onclick={() => (cliExpanded = !cliExpanded)}
            >
              <svg
                class="h-3 w-3 transition-transform {cliExpanded ? '' : '-rotate-90'}"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="m6 9 6 6 6-6" />
              </svg>
              {t("shortcutHelp_cliRef")}
            </button>
            {#if cliExpanded}
              <div class="mt-2 space-y-1">
                {#each cliBindings as b (b.command)}
                  <div class="flex items-center justify-between py-0.5">
                    <span class="text-xs text-foreground/50">{b.label}</span>
                    <kbd
                      class="inline-flex items-center rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground/50"
                      >{formatKeyDisplay(b.key)}</kbd
                    >
                  </div>
                {/each}
              </div>
            {/if}
          </section>
        {/if}
      </div>

      <!-- Footer -->
      <div class="border-t border-border px-5 py-2.5">
        <p class="text-[10px] text-muted-foreground">
          {t("shortcutHelp_customize")}
        </p>
      </div>
    </div>
  </div>
{/if}

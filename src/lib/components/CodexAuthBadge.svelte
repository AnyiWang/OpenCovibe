<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import type { CodexAuthResult, CodexProviderCredential, UserSettings } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import * as api from "$lib/api";
  import { CODEX_PROVIDER_PRESETS } from "$lib/utils/codex-provider-presets";
  import { dbg, dbgWarn } from "$lib/utils/debug";

  // Codex-specific auth/provider badge for the new-chat hero. Mirrors the Settings page
  // Codex model (codex login "cli" vs third-party "app" provider) instead of Claude's
  // multi-provider/version. Detailed config (API keys, custom endpoints) lives in Settings —
  // this badge selects + links out, matching how AuthSourceBadge defers provider detail.
  let {
    codexProvider = null,
    hasRun = false,
    variant = "default",
    onChanged,
  }: {
    codexProvider?: CodexProviderCredential | null;
    hasRun?: boolean;
    variant?: "default" | "hero";
    onChanged?: () => void;
  } = $props();

  let codexStatus = $state<CodexAuthResult | null>(null);
  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  // app mode = a third-party provider is configured; otherwise codex login (cli).
  let authMode = $derived<"cli" | "app">(codexProvider ? "app" : "cli");
  // Version is shown by the hero's shared meta row (heroMetaItems), not inline here —
  // keeps the Codex badge single-line and visually consistent with AuthSourceBadge.

  let triggerLabel = $derived.by(() => {
    if (authMode === "app") return codexProvider?.name ?? t("codexAuth_appLabel");
    return t("codexAuth_cliLabel");
  });

  let dotColor = $derived.by(() => {
    if (authMode === "app") {
      const preset = CODEX_PROVIDER_PRESETS.find((p) => p.id === codexProvider?.id);
      const ok = preset?.keyless || !!codexProvider?.api_key;
      return ok ? "bg-emerald-500" : "bg-amber-500";
    }
    return codexStatus?.logged_in ? "bg-emerald-500" : "bg-amber-500";
  });

  async function loadStatus() {
    try {
      codexStatus = await api.checkCodexAuth();
    } catch (e) {
      dbgWarn("codex-auth-badge", "checkCodexAuth failed", e);
      codexStatus = null;
    }
  }

  function toggleDropdown() {
    if (hasRun) return;
    dropdownOpen = !dropdownOpen;
    if (dropdownOpen && buttonEl) updateDropdownPosition();
  }

  function updateDropdownPosition() {
    if (!buttonEl) return;
    const rect = buttonEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    if (spaceBelow < 320) {
      dropdownStyle = `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
    } else {
      dropdownStyle = `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
    }
  }

  async function applyChange(patch: Partial<UserSettings>) {
    try {
      await api.updateUserSettings(patch);
      window.dispatchEvent(new Event("ocv:codex-auth-changed"));
      onChanged?.();
    } catch (e) {
      dbgWarn("codex-auth-badge", "updateUserSettings failed", e);
    }
  }

  // Switch to codex login (clear any third-party provider) — mirrors Settings setCodexAuthMode("cli").
  async function selectCliAuth() {
    dropdownOpen = false;
    if (!codexProvider) return;
    await applyChange({ codex_provider: null } as Partial<UserSettings>);
  }

  // Select a third-party provider preset. Custom endpoints need inputs → defer to Settings.
  // Detailed key entry also lives in Settings; here we persist the preset and reuse any
  // existing key for the same provider (mirrors Settings saveCodexProvider).
  async function selectProvider(presetId: string) {
    const preset = CODEX_PROVIDER_PRESETS.find((p) => p.id === presetId);
    if (!preset) return;
    if (preset.custom) {
      dropdownOpen = false;
      goto("/settings");
      return;
    }
    dbg("codex-auth-badge", "selectProvider", { id: preset.id });
    dropdownOpen = false;
    const reuseKey = codexProvider?.id === preset.id ? codexProvider?.api_key : undefined;
    const cred: CodexProviderCredential = {
      id: preset.id,
      name: preset.name,
      base_url: preset.base_url,
      env_key: preset.env_key,
      wire_api: "responses",
      model: preset.model,
      api_key: preset.keyless ? undefined : reuseKey || undefined,
    };
    await applyChange({ codex_provider: cred } as Partial<UserSettings>);
  }

  function providerHasKey(presetId: string): boolean {
    const preset = CODEX_PROVIDER_PRESETS.find((p) => p.id === presetId);
    if (preset?.keyless) return true;
    return codexProvider?.id === presetId && !!codexProvider?.api_key;
  }

  onMount(() => {
    void loadStatus();
    function onDocClick(e: MouseEvent) {
      if (dropdownOpen && wrapperEl && !wrapperEl.contains(e.target as Node)) dropdownOpen = false;
    }
    function onDocKeydown(e: KeyboardEvent) {
      if (dropdownOpen && e.key === "Escape") dropdownOpen = false;
    }
    function onAuthChanged() {
      void loadStatus();
    }
    document.addEventListener("mousedown", onDocClick, true);
    document.addEventListener("keydown", onDocKeydown);
    window.addEventListener("ocv:codex-auth-changed", onAuthChanged);
    return () => {
      document.removeEventListener("mousedown", onDocClick, true);
      document.removeEventListener("keydown", onDocKeydown);
      window.removeEventListener("ocv:codex-auth-changed", onAuthChanged);
    };
  });
</script>

{#if !hasRun}
  <div bind:this={wrapperEl} class="inline-flex items-center">
    <button
      bind:this={buttonEl}
      class="flex items-center gap-1.5 rounded-md transition-colors cursor-pointer
        {variant === 'hero'
        ? 'px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground'
        : 'border px-2 py-1 text-xs font-medium hover:bg-accent'}"
      onclick={toggleDropdown}
      title={t("codexAuth_title")}
    >
      <span class="inline-block h-1.5 w-1.5 rounded-full {dotColor}"></span>
      {triggerLabel}
      <svg
        class="h-2.5 w-2.5 text-muted-foreground"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
      >
    </button>

    {#if dropdownOpen}
      <div
        class="w-72 max-h-80 overflow-y-auto rounded-md border bg-background shadow-lg animate-fade-in"
        style={dropdownStyle}
      >
        <div class="p-2 space-y-1">
          <p
            class="px-2 pt-1 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60"
          >
            {t("settings_auth_modeLabel")}
          </p>

          <!-- Codex login (cli) -->
          <button
            class="flex w-full items-start gap-2.5 rounded-sm px-2.5 py-2 text-sm hover:bg-accent transition-colors
              {authMode === 'cli' ? 'bg-accent' : ''}"
            onclick={selectCliAuth}
          >
            <span class="mt-0.5 inline-block h-3.5 w-3.5 shrink-0">
              {#if authMode === "cli"}
                <svg
                  class="h-3.5 w-3.5 text-primary"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" /><circle
                    cx="12"
                    cy="12"
                    r="4"
                    fill="currentColor"
                  />
                </svg>
              {:else}
                <svg
                  class="h-3.5 w-3.5 text-muted-foreground/50"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <circle cx="12" cy="12" r="10" />
                </svg>
              {/if}
            </span>
            <div class="flex-1 text-left">
              <p class="font-medium text-xs">{t("codexAuth_cliLabel")}</p>
              {#if codexStatus?.logged_in}
                <p class="text-[10px] text-emerald-500">
                  <span class="inline-block h-1 w-1 rounded-full bg-emerald-500 mr-0.5 align-middle"
                  ></span>
                  {codexStatus.status_text ?? t("auth_loggedIn")}
                </p>
              {:else}
                <p class="text-[10px] text-muted-foreground">
                  <span
                    class="inline-block h-1 w-1 rounded-full bg-muted-foreground/40 mr-0.5 align-middle"
                  ></span>
                  {t("auth_notLoggedIn")}
                </p>
              {/if}
            </div>
          </button>

          <!-- Third-party provider (app) -->
          <div class="rounded-sm {authMode === 'app' ? 'bg-accent' : ''}">
            <div class="flex w-full items-start gap-2.5 px-2.5 py-2 text-sm">
              <span class="mt-0.5 inline-block h-3.5 w-3.5 shrink-0">
                {#if authMode === "app"}
                  <svg
                    class="h-3.5 w-3.5 text-primary"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                  >
                    <circle cx="12" cy="12" r="10" /><circle
                      cx="12"
                      cy="12"
                      r="4"
                      fill="currentColor"
                    />
                  </svg>
                {:else}
                  <svg
                    class="h-3.5 w-3.5 text-muted-foreground/50"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                  >
                    <circle cx="12" cy="12" r="10" />
                  </svg>
                {/if}
              </span>
              <p class="flex-1 text-left font-medium text-xs">{t("codexAuth_appLabel")}</p>
            </div>

            <!-- Provider list -->
            <div class="pl-8 pb-2 space-y-0.5">
              {#each CODEX_PROVIDER_PRESETS as preset}
                {@const isSelected = codexProvider?.id === preset.id}
                {@const hasKey = providerHasKey(preset.id)}
                <button
                  class="flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-xs hover:bg-accent/70 transition-colors"
                  onclick={() => selectProvider(preset.id)}
                >
                  <span
                    class="inline-block h-1 w-1 rounded-full shrink-0 {isSelected && !hasKey
                      ? 'bg-amber-500'
                      : hasKey
                        ? 'bg-emerald-500'
                        : 'bg-muted-foreground/30'}"
                  ></span>
                  <span
                    class="flex-1 min-w-0 text-left truncate {isSelected
                      ? 'font-medium text-foreground'
                      : 'text-foreground/80'}">{preset.name}</span
                  >
                  {#if isSelected && !hasKey && !preset.custom}
                    <span class="text-[9px] text-amber-500 shrink-0">{t("codexAuth_needsKey")}</span
                    >
                  {/if}
                  {#if isSelected}
                    <svg
                      class="h-3 w-3 text-primary shrink-0"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"><path d="M20 6 9 17l-5-5" /></svg
                    >
                  {/if}
                </button>
              {/each}
            </div>
          </div>

          <!-- Configure link -->
          <button
            class="flex w-full items-center gap-1.5 rounded-sm px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            onclick={() => {
              dropdownOpen = false;
              goto("/settings");
            }}
          >
            <svg
              class="h-3 w-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path
                d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
              />
              <circle cx="12" cy="12" r="3" />
            </svg>
            {t("auth_configureInSettings")}
          </button>
        </div>
      </div>
    {/if}
  </div>
{/if}

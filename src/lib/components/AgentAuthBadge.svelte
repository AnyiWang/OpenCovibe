<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import type { AuthOverview, CodexAuthResult, CodexProviderCredential } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import * as api from "$lib/api";
  import { CODEX_PROVIDER_PRESETS } from "$lib/utils/codex-provider-presets";
  import { computeDropdownStyle, attachDismissHandlers } from "$lib/utils/dropdown";
  import { dbgWarn } from "$lib/utils/debug";

  // Unified hero auth badge for both agents. Auth is a simple two-way choice — OAuth (the
  // CLI's own login) vs API Key — symmetric across Claude and Codex. Third-party providers
  // (Anthropic-grid / Codex providers) and key entry live in Settings, not here.
  //   Claude: OAuth = `claude` login (auth_mode "cli")  · API Key = key (auth_mode "api")
  //   Codex:  OAuth = `codex` login (no provider)        · API Key = a configured provider
  let {
    agent = "claude",
    // Claude inputs
    authOverview = null,
    onAuthModeChange,
    // Codex inputs
    codexProvider = null,
    onChanged,
    hasRun = false,
    variant = "default",
  }: {
    agent?: string;
    authOverview?: AuthOverview | null;
    onAuthModeChange?: (mode: string) => void;
    codexProvider?: CodexProviderCredential | null;
    onChanged?: () => void;
    hasRun?: boolean;
    variant?: "default" | "hero";
  } = $props();

  let isCodex = $derived(agent === "codex");
  let codexStatus = $state<CodexAuthResult | null>(null);
  let codexLoaded = $state(false);
  let dropdownOpen = $state(false);
  let wrapperEl: HTMLDivElement | undefined = $state();
  let buttonEl: HTMLButtonElement | undefined = $state();
  let dropdownStyle = $state("");

  // Active mode: "oauth" | "api".
  let mode = $derived.by<"oauth" | "api">(() => {
    if (isCodex) return codexProvider ? "api" : "oauth";
    return authOverview?.auth_mode === "api" ? "api" : "oauth";
  });

  // Load Codex auth state when the agent IS Codex — covers switching Claude→Codex after
  // mount (onMount's one-shot load would have early-returned while the agent was Claude,
  // leaving the OAuth dot stuck amber even though `codex login` is active).
  $effect(() => {
    if (isCodex && !codexLoaded) {
      codexLoaded = true;
      void loadCodexStatus();
    }
  });

  let oauthOk = $derived(isCodex ? !!codexStatus?.logged_in : !!authOverview?.cli_login_available);
  let apiKeyOk = $derived.by(() => {
    if (isCodex) {
      const preset = CODEX_PROVIDER_PRESETS.find((p) => p.id === codexProvider?.id);
      return !!preset?.keyless || !!codexProvider?.api_key;
    }
    return !!authOverview?.app_has_credentials;
  });

  let triggerLabel = $derived(mode === "oauth" ? t("auth_oauth") : t("auth_apiKey"));
  let dotColor = $derived(
    (mode === "oauth" ? oauthOk : apiKeyOk) ? "bg-emerald-500" : "bg-amber-500",
  );

  // OAuth sub-status line.
  let oauthStatus = $derived.by(() => {
    if (isCodex) {
      if (codexStatus?.logged_in) return codexStatus.status_text ?? t("auth_loggedIn");
      return t("auth_notLoggedIn");
    }
    if (authOverview?.cli_login_available) {
      return authOverview.cli_login_account
        ? `${t("auth_loggedIn")}: ${authOverview.cli_login_account}`
        : t("auth_loggedIn");
    }
    return t("auth_notLoggedIn");
  });

  // API-Key sub-status line.
  let apiKeyStatus = $derived.by(() => {
    if (apiKeyOk) {
      if (!isCodex && authOverview?.app_platform_name)
        return `${t("auth_loggedIn")} · ${authOverview.app_platform_name}`;
      if (isCodex && codexProvider?.name) return `${t("auth_loggedIn")} · ${codexProvider.name}`;
      return t("auth_loggedIn");
    }
    return t("auth_apiKeyHint");
  });

  async function loadCodexStatus() {
    if (!isCodex) return;
    try {
      codexStatus = await api.checkCodexAuth();
    } catch (e) {
      dbgWarn("agent-auth-badge", "checkCodexAuth failed", e);
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
    dropdownStyle = computeDropdownStyle(buttonEl, 240);
  }

  async function selectOAuth() {
    dropdownOpen = false;
    if (isCodex) {
      // Codex login = no app-managed provider. Clear it if set.
      if (codexProvider) {
        try {
          await api.updateUserSettings({ codex_provider: null } as never);
          window.dispatchEvent(new Event("ocv:codex-auth-changed"));
          onChanged?.();
        } catch (e) {
          dbgWarn("agent-auth-badge", "clear codex provider failed", e);
        }
      }
      return;
    }
    onAuthModeChange?.("cli");
  }

  function selectApiKey() {
    dropdownOpen = false;
    if (isCodex) {
      // Codex "API Key" = a third-party provider, configured in Settings (endpoint + key).
      goto("/settings");
      return;
    }
    onAuthModeChange?.("api");
  }

  onMount(() => {
    // Codex status is loaded by the $effect above (reactive to isCodex); here we only
    // wire the dropdown's outside-click/Escape handlers + the auth-changed refresh.
    const detach = attachDismissHandlers({
      getWrapper: () => wrapperEl,
      isOpen: () => dropdownOpen,
      close: () => (dropdownOpen = false),
    });
    function onCodexAuthChanged() {
      void loadCodexStatus();
    }
    window.addEventListener("ocv:codex-auth-changed", onCodexAuthChanged);
    return () => {
      detach();
      window.removeEventListener("ocv:codex-auth-changed", onCodexAuthChanged);
    };
  });
</script>

{#snippet radioRow(
  selected: boolean,
  label: string,
  status: string,
  statusOk: boolean,
  onClick: () => void,
)}
  <button
    class="flex w-full items-start gap-2.5 rounded-sm px-2.5 py-2 text-sm hover:bg-accent transition-colors
      {selected ? 'bg-accent' : ''}"
    onclick={onClick}
  >
    <span class="mt-0.5 inline-block h-3.5 w-3.5 shrink-0">
      {#if selected}
        <svg
          class="h-3.5 w-3.5 text-primary"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <circle cx="12" cy="12" r="10" /><circle cx="12" cy="12" r="4" fill="currentColor" />
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
      <p class="font-medium text-xs">{label}</p>
      <p class="text-[10px] {statusOk ? 'text-emerald-500' : 'text-muted-foreground'}">
        <span
          class="inline-block h-1 w-1 rounded-full {statusOk
            ? 'bg-emerald-500'
            : 'bg-muted-foreground/40'} mr-0.5 align-middle"
        ></span>
        {status}
      </p>
    </div>
  </button>
{/snippet}

{#if !hasRun}
  <div bind:this={wrapperEl} class="inline-flex items-center">
    <button
      bind:this={buttonEl}
      class="flex items-center gap-1.5 rounded-md transition-colors cursor-pointer
        {variant === 'hero'
        ? 'px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground'
        : 'border px-2 py-1 text-xs font-medium hover:bg-accent'}"
      onclick={toggleDropdown}
      title={t("settings_auth_modeLabel")}
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
        class="w-72 rounded-md border bg-background shadow-lg animate-fade-in"
        style={dropdownStyle}
      >
        <div class="p-2 space-y-1">
          <p
            class="px-2 pt-1 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60"
          >
            {t("settings_auth_modeLabel")}
          </p>
          {@render radioRow(mode === "oauth", t("auth_oauth"), oauthStatus, oauthOk, selectOAuth)}
          {@render radioRow(mode === "api", t("auth_apiKey"), apiKeyStatus, apiKeyOk, selectApiKey)}
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

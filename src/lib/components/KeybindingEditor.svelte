<script lang="ts">
  import type { KeyBinding } from "$lib/types";
  import {
    normalizeKeyEvent,
    formatKeyDisplay,
    RESERVED_KEYS,
  } from "$lib/stores/keybindings.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let {
    binding,
    isOverridden = false,
    conflictWarning = "",
    onSave,
    onReset,
  }: {
    binding: KeyBinding;
    isOverridden?: boolean;
    conflictWarning?: string;
    onSave: (key: string) => void;
    onReset?: () => void;
  } = $props();

  let recording = $state(false);
  let pendingKey = $state("");
  let reservedWarning = $state("");
  let buttonRef = $state<HTMLButtonElement | undefined>();

  function startRecording() {
    recording = true;
    pendingKey = "";
    reservedWarning = "";
    // Focus the button to capture key events
    requestAnimationFrame(() => buttonRef?.focus());
  }

  function cancelRecording() {
    recording = false;
    pendingKey = "";
    reservedWarning = "";
  }

  function confirmKey() {
    if (pendingKey && !reservedWarning) {
      onSave(pendingKey);
      recording = false;
      pendingKey = "";
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      cancelRecording();
      return;
    }

    const normalized = normalizeKeyEvent(e);
    if (!normalized) return; // modifier-only

    // Check reserved keys
    if (RESERVED_KEYS.has(normalized)) {
      pendingKey = normalized;
      reservedWarning = t("keybinding_reservedBySystem");
      return;
    }

    pendingKey = normalized;
    reservedWarning = "";
  }
</script>

<div class="flex items-center gap-3 py-1.5 group">
  <!-- Label -->
  <span class="text-sm text-foreground min-w-[140px]">{binding.label}</span>

  <!-- Key display / recording -->
  {#if recording}
    <button
      bind:this={buttonRef}
      class="flex items-center gap-1.5 rounded-md border-2 border-blue-500 bg-blue-500/5 px-3 py-1 text-sm font-mono min-w-[120px] animate-pulse focus:outline-none"
      onkeydown={handleKeydown}
    >
      {#if pendingKey}
        <span class={reservedWarning ? "text-destructive" : "text-foreground"}>
          {formatKeyDisplay(pendingKey)}
        </span>
      {:else}
        <span class="text-muted-foreground text-xs">{t("keybinding_pressCombo")}</span>
      {/if}
    </button>
  {:else}
    <span
      class="inline-flex items-center rounded-md border bg-muted/50 px-2.5 py-1 text-xs font-mono text-foreground min-w-[60px] justify-center {isOverridden
        ? 'border-primary/30 bg-primary/5'
        : ''}"
    >
      {formatKeyDisplay(binding.key) || "—"}
    </span>
  {/if}

  <!-- Actions -->
  <div class="flex items-center gap-1.5 ml-auto">
    {#if recording}
      {#if reservedWarning}
        <span class="text-[11px] text-destructive">{reservedWarning}</span>
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={cancelRecording}
          title={t("common_cancel")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
          >
        </button>
      {:else if conflictWarning && pendingKey}
        <span class="text-[11px] text-amber-500 max-w-[160px] truncate" title={conflictWarning}
          >{conflictWarning}</span
        >
        <button
          class="rounded p-1 text-emerald-500 hover:text-emerald-400 hover:bg-emerald-500/10 transition-colors"
          onclick={confirmKey}
          title={t("keybinding_useAnyway")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
        </button>
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={cancelRecording}
          title={t("common_cancel")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
          >
        </button>
      {:else if pendingKey}
        <button
          class="rounded p-1 text-emerald-500 hover:text-emerald-400 hover:bg-emerald-500/10 transition-colors"
          onclick={confirmKey}
          title={t("keybinding_confirm")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
        </button>
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={cancelRecording}
          title={t("common_cancel")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
          >
        </button>
      {:else}
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={cancelRecording}
          title={t("common_cancel")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
          >
        </button>
      {/if}
    {:else if binding.editable}
      {#if isOverridden && onReset}
        <button
          class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors opacity-0 group-hover:opacity-100"
          onclick={onReset}
          title={t("keybinding_resetDefault")}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path
              d="M3 3v5h5"
            /></svg
          >
        </button>
      {/if}
      <button
        class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors opacity-0 group-hover:opacity-100"
        onclick={startRecording}
        title={t("keybinding_editShortcut")}
      >
        <svg
          class="h-3.5 w-3.5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><path
            d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"
          /></svg
        >
      </button>
    {/if}
  </div>
</div>

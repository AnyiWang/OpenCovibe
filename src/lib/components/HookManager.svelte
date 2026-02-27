<script lang="ts">
  import { onMount } from "svelte";
  import { getCliConfig, updateCliConfig } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import {
    HOOK_EVENT_TYPES,
    normalizeForDisplay,
    addGroup,
    removeGroup,
    patchGroup,
  } from "$lib/utils/hook-helpers";
  import type { HookHandler } from "$lib/types";

  type Rec = Record<string, any>;

  // ── State ──
  let cliConfig = $state<Record<string, unknown> | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let saving = $state(false);

  // Toast
  let toastMessage = $state<string | null>(null);
  let toastType = $state<"success" | "error">("success");
  let toastTimeout: ReturnType<typeof setTimeout> | null = null;

  // Editor
  let editorMode = $state<null | "new" | "edit">(null);
  let editorEvent = $state<string>(HOOK_EVENT_TYPES[0]);
  let editorMatcher = $state("");
  let editorHandlers = $state<HookHandler[]>([]);
  let editorGroupIndex = $state(0);

  // Confirm dialog
  let confirmAction = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  // ── Derived ──
  let rawHooks = $derived(cliConfig ? cliConfig.hooks : null);
  let displayHooks = $derived(normalizeForDisplay(rawHooks));
  let displayEntries = $derived(Object.entries(displayHooks));
  let totalGroups = $derived(displayEntries.reduce((sum, [, groups]) => sum + groups.length, 0));

  // ── Lifecycle ──
  onMount(() => {
    loadConfig();
  });

  async function loadConfig() {
    loading = true;
    loadError = null;
    try {
      cliConfig = await getCliConfig();
      dbg("hooks", "loaded config", { hasHooks: !!cliConfig?.hooks });
    } catch (e) {
      loadError = String(e);
      dbgWarn("hooks", "load error", e);
    } finally {
      loading = false;
    }
  }

  // ── Save helpers ──
  async function saveHooks(newHooks: Rec) {
    saving = true;
    try {
      const updated = await updateCliConfig({ hooks: newHooks });
      cliConfig = updated;
      showToast(t("hooks_saved"), "success");
      dbg("hooks", "saved", { events: Object.keys(newHooks) });
    } catch (e) {
      showToast(t("hooks_saveFailed", { error: String(e) }), "error");
      dbgWarn("hooks", "save error", e);
    } finally {
      saving = false;
    }
  }

  function showToast(message: string, type: "success" | "error") {
    toastMessage = message;
    toastType = type;
    if (toastTimeout) clearTimeout(toastTimeout);
    toastTimeout = setTimeout(() => {
      toastMessage = null;
    }, 4000);
  }

  // ── Editor ──
  function startAddGroup() {
    editorMode = "new";
    editorEvent = HOOK_EVENT_TYPES[0];
    editorMatcher = "";
    editorHandlers = [{ type: "command", command: "" }];
    editorGroupIndex = 0;
  }

  function startEditGroup(event: string, index: number, group: unknown) {
    editorMode = "edit";
    editorEvent = event;
    editorGroupIndex = index;

    if (group && typeof group === "object" && !Array.isArray(group)) {
      const g = group as Rec;
      editorMatcher = typeof g.matcher === "string" ? g.matcher : "";
      if (Array.isArray(g.hooks)) {
        editorHandlers = g.hooks.map((h: unknown) => {
          if (h && typeof h === "object" && !Array.isArray(h)) {
            const hObj = h as Rec;
            return {
              type: hObj.type === "prompt" ? "prompt" : "command",
              command: typeof hObj.command === "string" ? hObj.command : undefined,
              prompt: typeof hObj.prompt === "string" ? hObj.prompt : undefined,
              timeout: typeof hObj.timeout === "number" ? hObj.timeout : undefined,
              async: typeof hObj.async === "boolean" ? hObj.async : undefined,
              statusMessage:
                typeof hObj.statusMessage === "string" ? hObj.statusMessage : undefined,
              model: typeof hObj.model === "string" ? hObj.model : undefined,
              once: typeof hObj.once === "boolean" ? hObj.once : undefined,
            } as HookHandler;
          }
          return { type: "command" as const, command: "" };
        });
      } else {
        editorHandlers = [{ type: "command", command: "" }];
      }
    } else {
      editorMatcher = "";
      editorHandlers = [{ type: "command", command: "" }];
    }
  }

  function cancelEditor() {
    editorMode = null;
  }

  function addHandler() {
    editorHandlers = [...editorHandlers, { type: "command", command: "" }];
  }

  function removeHandler(idx: number) {
    editorHandlers = editorHandlers.filter((_, i) => i !== idx);
  }

  function buildGroupFromEditor(): Rec {
    const group: Rec = {};
    if (editorMatcher.trim()) group.matcher = editorMatcher.trim();
    group.hooks = editorHandlers.map((h) => {
      const handler: Rec = { type: h.type };
      if (h.type === "command" && h.command) handler.command = h.command;
      if (h.type === "prompt" && h.prompt) handler.prompt = h.prompt;
      if (h.timeout != null && h.timeout > 0) handler.timeout = h.timeout;
      if (h.async === true) handler.async = true;
      if (h.once === true) handler.once = true;
      if (h.statusMessage) handler.statusMessage = h.statusMessage;
      if (h.type === "prompt" && h.model) handler.model = h.model;
      return handler;
    });
    return group;
  }

  async function handleSaveEditor() {
    const group = buildGroupFromEditor();
    let newHooks: Rec;
    if (editorMode === "new") {
      newHooks = addGroup(rawHooks, editorEvent, group);
    } else {
      newHooks = patchGroup(rawHooks, editorEvent, editorGroupIndex, group);
    }
    await saveHooks(newHooks);
    editorMode = null;
  }

  function handleDeleteGroup(event: string, index: number) {
    confirmAction = {
      title: t("hooks_deleteGroup"),
      message: t("hooks_deleteGroupMsg"),
      onConfirm: async () => {
        const newHooks = removeGroup(rawHooks, event, index);
        await saveHooks(newHooks);
      },
    };
  }

  /** Format group summary for compact display. */
  function groupSummary(group: unknown): string {
    if (!group || typeof group !== "object" || Array.isArray(group)) return "—";
    const g = group as Rec;
    const parts: string[] = [];
    if (typeof g.matcher === "string" && g.matcher) parts.push(g.matcher);
    if (Array.isArray(g.hooks)) {
      const count = g.hooks.length;
      parts.push(`${count} handler${count !== 1 ? "s" : ""}`);
    }
    return parts.join(" · ") || "—";
  }
</script>

<!-- Toast -->
{#if toastMessage}
  <div
    class="fixed top-4 right-4 z-50 rounded-lg border px-4 py-2 text-sm shadow-lg transition-opacity {toastType ===
    'success'
      ? 'border-green-500/30 bg-green-500/10 text-green-600 dark:text-green-400'
      : 'border-destructive/30 bg-destructive/10 text-destructive'}"
  >
    {toastMessage}
  </div>
{/if}

<!-- Confirm dialog -->
{#if confirmAction}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    onclick={() => (confirmAction = null)}
  >
    <div
      class="rounded-lg border border-border bg-background p-6 shadow-xl max-w-sm"
      onclick={(e) => e.stopPropagation()}
    >
      <h3 class="text-sm font-semibold text-foreground mb-2">{confirmAction.title}</h3>
      <p class="text-xs text-muted-foreground mb-4">{confirmAction.message}</p>
      <div class="flex justify-end gap-2">
        <button
          class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          onclick={() => (confirmAction = null)}>{t("common_cancel")}</button
        >
        <button
          class="rounded-md bg-destructive px-3 py-1.5 text-xs text-destructive-foreground hover:bg-destructive/90"
          onclick={() => {
            confirmAction?.onConfirm();
            confirmAction = null;
          }}>{t("plugin_confirm")}</button
        >
      </div>
    </div>
  </div>
{/if}

<div class="space-y-4">
  <div class="flex items-center justify-between">
    <div>
      <h2 class="text-sm font-semibold text-foreground">{t("hooks_title")}</h2>
      <p class="text-[11px] text-muted-foreground">{t("hooks_desc")}</p>
    </div>
    <button
      class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted transition-colors"
      title={t("hooks_refresh")}
      onclick={loadConfig}
    >
      <svg
        class="h-3.5 w-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        ><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /></svg
      >
    </button>
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-16">
      <div
        class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
      <span class="ml-2 text-xs text-muted-foreground">{t("hooks_loading")}</span>
    </div>
  {:else if loadError}
    <div
      class="rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-xs text-destructive"
    >
      {t("hooks_loadFailed", { error: loadError })}
    </div>
  {:else}
    <!-- Add button -->
    <div class="flex items-center gap-3">
      <button
        class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
        onclick={startAddGroup}
      >
        {t("hooks_addGroup")}
      </button>
      {#if totalGroups > 0}
        <span class="text-[11px] text-muted-foreground"
          >{t("hooks_groupCount", { count: String(totalGroups) })}</span
        >
      {/if}
    </div>

    <!-- Editor (inline) -->
    {#if editorMode}
      <div class="rounded-lg border border-border/50 bg-muted/20 px-4 py-4 space-y-3">
        <div class="flex items-center justify-between">
          <h3 class="text-sm font-medium text-foreground">
            {editorMode === "new" ? t("hooks_addGroup") : t("hooks_editGroup")}
          </h3>
          <button class="text-xs text-muted-foreground hover:text-foreground" onclick={cancelEditor}
            >{t("common_cancel")}</button
          >
        </div>

        <!-- Edit warning -->
        {#if editorMode === "edit"}
          <div
            class="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-1.5 text-[11px] text-amber-600 dark:text-amber-400"
          >
            {t("hooks_editWarning")}
          </div>
        {/if}

        <!-- Event type (pill selector) -->
        <div>
          <label class="block text-[11px] font-medium text-muted-foreground mb-1.5"
            >{t("hooks_event")}</label
          >
          <div class="flex flex-wrap gap-1">
            {#each HOOK_EVENT_TYPES as ev}
              <button
                class="rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors
                  {editorEvent === ev
                  ? 'bg-primary text-primary-foreground'
                  : 'border border-border text-muted-foreground hover:text-foreground hover:bg-muted'}
                  {editorMode === 'edit' && editorEvent !== ev
                  ? 'opacity-30 pointer-events-none'
                  : ''}"
                onclick={() => {
                  if (editorMode !== "edit") editorEvent = ev;
                }}
              >
                {ev}
              </button>
            {/each}
          </div>
        </div>

        <!-- Matcher -->
        {#if editorEvent === "PreToolUse" || editorEvent === "PostToolUse" || editorEvent === "SubagentTool"}
          <div>
            <label class="block text-[11px] font-medium text-muted-foreground mb-1"
              >{t("hooks_matcher")}</label
            >
            <input
              type="text"
              class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              placeholder={t("hooks_matcherPlaceholder")}
              bind:value={editorMatcher}
            />
            <p class="text-[10px] text-muted-foreground mt-0.5">{t("hooks_matcherHelp")}</p>
          </div>
        {/if}

        <!-- Handlers -->
        <div>
          <div class="flex items-center justify-between mb-2">
            <label class="text-[11px] font-medium text-muted-foreground"
              >{t("hooks_handlers")}</label
            >
            <button
              class="rounded-md border border-border px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground hover:bg-muted transition-colors"
              onclick={addHandler}
            >
              + {t("hooks_addHandler")}
            </button>
          </div>

          <div class="space-y-3">
            {#each editorHandlers as handler, hi}
              <div class="rounded-md border border-border/50 bg-background px-3 py-2.5 space-y-2">
                <div class="flex items-center justify-between gap-2">
                  <!-- Type toggle -->
                  <div class="flex gap-1 rounded-md border border-border p-0.5">
                    <button
                      class="rounded px-2 py-0.5 text-[10px] font-medium transition-colors {handler.type ===
                      'command'
                        ? 'bg-primary text-primary-foreground'
                        : 'text-muted-foreground hover:text-foreground'}"
                      onclick={() => {
                        editorHandlers[hi] = { ...handler, type: "command" };
                      }}>{t("hooks_handlerCommand")}</button
                    >
                    <button
                      class="rounded px-2 py-0.5 text-[10px] font-medium transition-colors {handler.type ===
                      'prompt'
                        ? 'bg-primary text-primary-foreground'
                        : 'text-muted-foreground hover:text-foreground'}"
                      onclick={() => {
                        editorHandlers[hi] = { ...handler, type: "prompt" };
                      }}>{t("hooks_handlerPrompt")}</button
                    >
                  </div>
                  {#if editorHandlers.length > 1}
                    <button
                      class="rounded p-1 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                      onclick={() => removeHandler(hi)}
                      title={t("hooks_removeHandler")}
                    >
                      <svg
                        class="h-3 w-3"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
                      >
                    </button>
                  {/if}
                </div>

                <!-- Command / Prompt input -->
                {#if handler.type === "command"}
                  <input
                    type="text"
                    class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm font-mono text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                    placeholder={t("hooks_commandPlaceholder")}
                    value={handler.command ?? ""}
                    oninput={(e) => {
                      editorHandlers[hi] = {
                        ...handler,
                        command: (e.target as HTMLInputElement).value,
                      };
                    }}
                  />
                {:else}
                  <textarea
                    class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring resize-y"
                    rows="3"
                    placeholder={t("hooks_promptPlaceholder")}
                    value={handler.prompt ?? ""}
                    oninput={(e) => {
                      editorHandlers[hi] = {
                        ...handler,
                        prompt: (e.target as HTMLTextAreaElement).value,
                      };
                    }}
                  ></textarea>
                  <!-- Model for prompt handlers -->
                  <div>
                    <label class="block text-[10px] text-muted-foreground mb-0.5"
                      >{t("hooks_model")}</label
                    >
                    <input
                      type="text"
                      class="w-full rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                      placeholder={t("hooks_modelPlaceholder")}
                      value={handler.model ?? ""}
                      oninput={(e) => {
                        editorHandlers[hi] = {
                          ...handler,
                          model: (e.target as HTMLInputElement).value || undefined,
                        };
                      }}
                    />
                  </div>
                {/if}

                <!-- Options row -->
                <div class="flex items-center gap-5 flex-wrap">
                  <!-- Timeout -->
                  <div class="flex items-center gap-1.5">
                    <span class="text-[10px] text-muted-foreground">{t("hooks_timeout")}</span>
                    <input
                      type="number"
                      class="w-20 rounded-md border border-border bg-background px-2 py-0.5 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                      value={handler.timeout ?? ""}
                      oninput={(e) => {
                        const v = parseInt((e.target as HTMLInputElement).value);
                        editorHandlers[hi] = {
                          ...handler,
                          timeout: isNaN(v) ? undefined : v,
                        };
                      }}
                    />
                  </div>
                  <!-- Async toggle -->
                  <button
                    class="flex items-center gap-1.5"
                    onclick={() => {
                      editorHandlers[hi] = {
                        ...handler,
                        async: handler.async ? undefined : true,
                      };
                    }}
                  >
                    <div
                      class="relative inline-flex h-4 w-7 shrink-0 items-center rounded-full transition-colors duration-200 {handler.async
                        ? 'bg-primary'
                        : 'bg-muted-foreground/25'}"
                    >
                      <div
                        class="inline-block h-3 w-3 transform rounded-full bg-white transition-transform duration-200 {handler.async
                          ? 'translate-x-3.5'
                          : 'translate-x-0.5'}"
                      ></div>
                    </div>
                    <span class="text-[10px] text-muted-foreground">{t("hooks_async")}</span>
                  </button>
                  <!-- Once toggle -->
                  <button
                    class="flex items-center gap-1.5"
                    onclick={() => {
                      editorHandlers[hi] = {
                        ...handler,
                        once: handler.once ? undefined : true,
                      };
                    }}
                  >
                    <div
                      class="relative inline-flex h-4 w-7 shrink-0 items-center rounded-full transition-colors duration-200 {handler.once
                        ? 'bg-primary'
                        : 'bg-muted-foreground/25'}"
                    >
                      <div
                        class="inline-block h-3 w-3 transform rounded-full bg-white transition-transform duration-200 {handler.once
                          ? 'translate-x-3.5'
                          : 'translate-x-0.5'}"
                      ></div>
                    </div>
                    <span class="text-[10px] text-muted-foreground">{t("hooks_once")}</span>
                  </button>
                </div>

                <!-- Status message -->
                {#if handler.type === "command"}
                  <div>
                    <label class="block text-[10px] text-muted-foreground mb-0.5"
                      >{t("hooks_statusMessage")}</label
                    >
                    <input
                      type="text"
                      class="w-full rounded-md border border-border bg-background px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
                      placeholder={t("hooks_statusMessagePlaceholder")}
                      value={handler.statusMessage ?? ""}
                      oninput={(e) => {
                        editorHandlers[hi] = {
                          ...handler,
                          statusMessage: (e.target as HTMLInputElement).value || undefined,
                        };
                      }}
                    />
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        </div>

        <!-- Save / Cancel -->
        <div class="flex justify-end gap-2 pt-1">
          <button
            class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
            onclick={cancelEditor}>{t("common_cancel")}</button
          >
          <button
            class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
            onclick={handleSaveEditor}
            disabled={saving}
          >
            {saving ? t("hooks_saving") : t("hooks_save")}
          </button>
        </div>
      </div>
    {/if}

    <!-- Hook groups list -->
    {#if totalGroups === 0 && !editorMode}
      <div class="flex flex-col items-center justify-center py-16 text-center">
        <div
          class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border border-border bg-muted"
        >
          <svg
            class="h-6 w-6 text-muted-foreground"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path
              d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"
            /></svg
          >
        </div>
        <h2 class="text-sm font-medium text-foreground mb-1">{t("hooks_noHooks")}</h2>
        <p class="text-xs text-muted-foreground max-w-sm mb-3">
          {t("hooks_noHooksDesc")}
        </p>
        <button
          class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
          onclick={startAddGroup}
        >
          {t("hooks_addFirstHook")}
        </button>
      </div>
    {:else if !editorMode}
      <div class="space-y-3">
        {#each displayEntries as [event, groups]}
          <div>
            <!-- Event header -->
            <div class="flex items-center gap-2 mb-1.5">
              <span
                class="rounded-full px-2 py-0.5 text-[10px] font-semibold bg-primary/10 text-primary"
                >{event}</span
              >
              <span class="text-[10px] text-muted-foreground"
                >{t("hooks_groupCount", { count: String(groups.length) })}</span
              >
            </div>
            <!-- Groups -->
            <div class="space-y-1.5">
              {#each groups as group, gi}
                <div
                  class="rounded-lg border border-border/50 bg-muted/30 px-3 py-2 flex items-center justify-between gap-3"
                >
                  <div class="flex-1 min-w-0">
                    <span class="text-xs text-foreground">{groupSummary(group)}</span>
                  </div>
                  <div class="flex items-center gap-1 shrink-0">
                    <button
                      class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted transition-colors"
                      onclick={() => startEditGroup(event, gi, group)}
                      title={t("hooks_editGroup")}
                    >
                      <svg
                        class="h-3.5 w-3.5"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" /><path
                          d="m15 5 4 4"
                        /></svg
                      >
                    </button>
                    <button
                      class="rounded p-1 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                      onclick={() => handleDeleteGroup(event, gi)}
                      title={t("hooks_deleteGroup")}
                    >
                      <svg
                        class="h-3.5 w-3.5"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><path d="M3 6h18" /><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" /><path
                          d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"
                        /></svg
                      >
                    </button>
                  </div>
                </div>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<script lang="ts">
  import type { ElicitationState } from "$lib/stores/session-store.svelte";
  import type { ElicitationFieldSchema } from "$lib/types";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";

  let {
    elicitations,
    onRespond,
  }: {
    elicitations: Map<string, ElicitationState>;
    onRespond: (
      requestId: string,
      action: "accept" | "decline" | "cancel",
      content?: Record<string, unknown>,
    ) => void | Promise<void>;
  } = $props();

  let submitting = $state(false);

  // Queue strategy: show the first pending elicitation
  let current = $derived.by(() => {
    const iter = elicitations.values();
    const first = iter.next();
    return first.done ? null : first.value;
  });

  // Form state for schema fields
  let formValues = $state<Record<string, unknown>>({});

  // Reset form when current elicitation changes
  $effect(() => {
    if (current) {
      const defaults: Record<string, unknown> = {};
      const props = current.requestedSchema?.properties;
      if (props) {
        for (const [key, field] of Object.entries(props)) {
          if (field.default !== undefined) {
            defaults[key] = field.default;
          } else if (field.type === "boolean") {
            defaults[key] = false;
          } else if (field.type === "number") {
            defaults[key] = 0;
          } else {
            defaults[key] = "";
          }
        }
      }
      formValues = defaults;
    }
  });

  function updateField(key: string, value: unknown) {
    formValues = { ...formValues, [key]: value };
  }

  async function handleAccept() {
    if (!current || submitting) return;
    submitting = true;
    dbg("ElicitationDialog", "accept", { requestId: current.requestId });
    try {
      await onRespond(current.requestId, "accept", formValues);
    } catch (e) {
      dbgWarn("ElicitationDialog", "accept error", e);
    } finally {
      submitting = false;
    }
  }

  async function handleDecline() {
    if (!current || submitting) return;
    submitting = true;
    dbg("ElicitationDialog", "decline", { requestId: current.requestId });
    try {
      await onRespond(current.requestId, "decline");
    } catch (e) {
      dbgWarn("ElicitationDialog", "decline error", e);
    } finally {
      submitting = false;
    }
  }

  async function openElicitationUrl(href: string) {
    // Protocol whitelist — block file://, javascript://, etc.
    try {
      const url = new URL(href);
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        dbgWarn("ElicitationDialog", "blocked non-http(s) URL", { href });
        return;
      }
    } catch {
      return;
    }
    // Reuse plugin-shell + window.open pattern (same as +layout.svelte)
    try {
      const { open } = await import("@tauri-apps/plugin-shell");
      await open(href);
    } catch {
      window.open(href, "_blank");
    }
  }

  function renderFieldType(field: ElicitationFieldSchema): string {
    if (field.enum && field.enum.length > 0) return "enum";
    return field.type ?? "string";
  }
</script>

{#if current}
  <div
    class="mx-4 mb-2 rounded-lg border border-blue-500/30 bg-blue-500/5 p-4 shadow-lg"
    role="dialog"
    aria-label={t("elicitation_title")}
  >
    <!-- Header -->
    <div class="mb-3 flex items-center gap-2">
      <div
        class="flex h-6 w-6 items-center justify-center rounded-full bg-blue-500/20 text-xs text-blue-400"
      >
        ?
      </div>
      <div class="flex-1">
        <div class="text-sm font-medium text-neutral-200">
          {t("elicitation_title")}
        </div>
        <div class="text-xs text-neutral-400">
          {current.mcpServerName}
          {#if elicitations.size > 1}
            <span class="ml-1 text-neutral-500">
              ({t("elicitation_pending", { count: String(elicitations.size) })})
            </span>
          {/if}
        </div>
      </div>
    </div>

    <!-- Message -->
    {#if current.message}
      <p class="mb-3 text-sm text-neutral-300">{current.message}</p>
    {/if}

    <!-- URL mode -->
    {#if current.url}
      <div class="mb-3">
        <button
          class="text-sm text-blue-400 underline hover:text-blue-300"
          onclick={() => current?.url && openElicitationUrl(current.url)}
        >
          {t("elicitation_open_url")}
        </button>
      </div>
    {/if}

    <!-- Form mode: render schema fields -->
    {#if current.requestedSchema?.properties}
      <div class="mb-3 space-y-3">
        {#each Object.entries(current.requestedSchema.properties) as [key, field]}
          {@const fieldType = renderFieldType(field)}
          {@const isRequired = current.requestedSchema?.required?.includes(key) ?? field.required}
          <div>
            <label class="mb-1 block text-xs font-medium text-neutral-300" for="elic-{key}">
              {field.title ?? key}
              {#if isRequired}
                <span class="text-red-400">*</span>
              {/if}
            </label>
            {#if field.description}
              <p class="mb-1 text-xs text-neutral-500">{field.description}</p>
            {/if}

            {#if fieldType === "boolean"}
              <label class="flex items-center gap-2">
                <input
                  id="elic-{key}"
                  type="checkbox"
                  checked={!!formValues[key]}
                  onchange={(e) => updateField(key, (e.target as HTMLInputElement).checked)}
                  class="rounded border-neutral-600 bg-neutral-700"
                />
                <span class="text-xs text-neutral-400">{field.title ?? key}</span>
              </label>
            {:else if fieldType === "enum" && field.enum}
              <select
                id="elic-{key}"
                value={String(formValues[key] ?? "")}
                onchange={(e) => updateField(key, (e.target as HTMLSelectElement).value)}
                class="w-full rounded border border-neutral-600 bg-neutral-800 px-2 py-1 text-sm text-neutral-200"
              >
                <option value="">--</option>
                {#each field.enum as opt}
                  <option value={opt}>{opt}</option>
                {/each}
              </select>
            {:else if fieldType === "number"}
              <input
                id="elic-{key}"
                type="number"
                value={formValues[key] as number}
                oninput={(e) => updateField(key, Number((e.target as HTMLInputElement).value))}
                class="w-full rounded border border-neutral-600 bg-neutral-800 px-2 py-1 text-sm text-neutral-200"
              />
            {:else}
              <input
                id="elic-{key}"
                type="text"
                value={String(formValues[key] ?? "")}
                oninput={(e) => updateField(key, (e.target as HTMLInputElement).value)}
                class="w-full rounded border border-neutral-600 bg-neutral-800 px-2 py-1 text-sm text-neutral-200"
                placeholder={field.title ?? key}
              />
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <!-- Actions -->
    <div class="flex items-center justify-end gap-2">
      <button
        class="rounded px-3 py-1.5 text-xs text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200 disabled:opacity-50"
        disabled={submitting}
        onclick={handleDecline}
      >
        {t("elicitation_decline")}
      </button>
      <button
        class="rounded bg-blue-600 px-3 py-1.5 text-xs text-white hover:bg-blue-500 disabled:opacity-50"
        disabled={submitting}
        onclick={handleAccept}
      >
        {submitting ? "..." : t("elicitation_accept")}
      </button>
    </div>
  </div>
{/if}

<script lang="ts">
  import { untrack } from "svelte";
  import { t } from "$lib/i18n/index.svelte";
  import * as api from "$lib/api";
  import type { DirEntry, RemoteHost } from "$lib/types";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { getStoredRemoteCwd, setStoredRemoteCwd } from "$lib/utils/remote-cwd";
  import { getTransport } from "$lib/transport";
  import Modal from "./Modal.svelte";
  import Button from "./Button.svelte";

  type PickResult = { hostName: string | null; path: string };

  let {
    open = $bindable(false),
    initialHost = null,
    initialPath = "",
    hideTargetSelector = false,
    onConfirm,
    onCancel,
  }: {
    open: boolean;
    initialHost?: string | null;
    initialPath?: string;
    hideTargetSelector?: boolean;
    onConfirm: (result: PickResult) => void;
    onCancel?: () => void;
  } = $props();

  let remoteHosts = $state<RemoteHost[]>([]);
  let hostsLoaded = $state(false);

  let hostName = $state<string | null>(null);
  let currentPath = $state<string>("");
  let pathInput = $state<string>("");
  let entries = $state<DirEntry[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let showHidden = $state(false);

  let _seq = 0;
  let _initSent = false;

  function parentPath(p: string): string {
    if (!p || p === "/") return "/";
    const trimmed = p.replace(/\/+$/, "");
    const idx = trimmed.lastIndexOf("/");
    if (idx <= 0) return "/";
    return trimmed.slice(0, idx);
  }

  function joinPath(base: string, name: string): string {
    if (!base || base === "/") return "/" + name;
    return base.replace(/\/+$/, "") + "/" + name;
  }

  async function loadRemoteHosts() {
    try {
      const settings = await api.getUserSettings();
      remoteHosts = settings.remote_hosts ?? [];
    } catch (e) {
      dbgWarn("folder-picker", "failed to load remote hosts:", e);
      remoteHosts = [];
    } finally {
      hostsLoaded = true;
    }
  }

  // ── Reset when closed ──
  $effect(() => {
    if (!open) {
      _seq++;
      _initSent = false;
      loading = false;
      error = null;
      entries = [];
    }
  });

  // ── Initialize when opened ──
  $effect(() => {
    if (!open || _initSent) return;
    _initSent = true;
    untrack(() => {
      hostName = initialHost ?? null;
      currentPath = initialPath ?? "";
      pathInput = initialPath ?? "";
      showHidden = false;
      entries = [];
      error = null;
      void loadRemoteHosts();
      if (hostName) {
        void initRemotePath();
      } else {
        // Local: nothing to fetch — user clicks native dialog
        currentPath = initialPath || "";
        pathInput = currentPath;
      }
    });
  });

  async function initRemotePath() {
    if (!hostName) return;
    let path = initialPath;
    if (!path) {
      path = getStoredRemoteCwd(hostName);
    }
    if (!path) {
      const host = remoteHosts.find((h) => h.name === hostName);
      path = host?.remote_cwd ?? "";
    }
    if (!path) {
      try {
        path = await api.resolveRemoteHome(hostName);
      } catch (e) {
        dbgWarn("folder-picker", "resolveRemoteHome failed:", e);
        path = "/";
      }
    }
    await navigate(path);
  }

  async function navigate(path: string) {
    if (!hostName) return;
    const seq = ++_seq;
    loading = true;
    error = null;
    try {
      const result = await api.listRemoteDirectory(hostName, path, showHidden);
      if (seq !== _seq) return;
      currentPath = result.path;
      pathInput = result.path;
      entries = result.entries.filter((e) => e.is_dir);
    } catch (e) {
      if (seq !== _seq) return;
      const msg = String((e as Error)?.message ?? e);
      error = msg;
      dbgWarn("folder-picker", "listRemoteDirectory failed:", msg);
    } finally {
      if (seq === _seq) loading = false;
    }
  }

  async function onTargetChange(name: string | null) {
    if (hostName === name) return;
    hostName = name;
    error = null;
    entries = [];
    if (name) {
      currentPath = "";
      pathInput = "";
      await initRemotePath();
    } else {
      currentPath = "";
      pathInput = "";
    }
  }

  async function onShowHiddenChange() {
    if (!hostName) return;
    await navigate(currentPath || "/");
  }

  async function onPathInputSubmit() {
    if (!hostName) {
      currentPath = pathInput.trim();
      return;
    }
    const target = pathInput.trim();
    if (!target) return;
    await navigate(target);
  }

  function handlePathKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      void onPathInputSubmit();
    }
  }

  async function browseLocal() {
    try {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const selected = await openDialog({
        directory: true,
        title: t("layout_selectProjectFolder"),
      });
      if (selected) {
        currentPath = selected as string;
        pathInput = currentPath;
      }
    } catch (e) {
      dbgWarn("folder-picker", "native dialog failed:", e);
    }
  }

  function confirm() {
    const path = (pathInput || currentPath).trim();
    if (!path) return;
    if (hostName) {
      setStoredRemoteCwd(hostName, path);
    }
    open = false;
    dbg("folder-picker", "confirm", { hostName, path });
    onConfirm({ hostName, path });
  }

  function cancel() {
    open = false;
    onCancel?.();
  }

  let canConfirm = $derived(((pathInput || currentPath) ?? "").trim().length > 0);
  let isLocalDesktop = $derived(!hostName && getTransport().isDesktop());
</script>

<Modal bind:open title={t("picker_title")}>
  <div class="flex flex-col gap-3 -mt-1 max-w-2xl">
    <!-- Target host selector -->
    {#if !hideTargetSelector}
      <div class="flex items-center gap-2 text-sm">
        <span class="text-muted-foreground shrink-0">{t("picker_target")}:</span>
        <select
          class="h-8 rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          value={hostName ?? ""}
          onchange={(e) => {
            const v = (e.currentTarget as HTMLSelectElement).value;
            void onTargetChange(v ? v : null);
          }}
        >
          <option value="">{t("picker_local")}</option>
          {#each remoteHosts as h (h.name)}
            <option value={h.name}>{h.name} ({h.user}@{h.host})</option>
          {/each}
        </select>
        {#if hostsLoaded && remoteHosts.length === 0}
          <span class="text-xs text-muted-foreground">{t("picker_noTargets")}</span>
        {/if}
      </div>
    {/if}

    <!-- Current path / input — Enter navigates (remote) -->
    <div class="flex items-center gap-2">
      <input
        type="text"
        bind:value={pathInput}
        onkeydown={handlePathKey}
        placeholder={t("picker_pathInput")}
        class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 font-mono text-xs shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />
      {#if hostName}
        <Button
          variant="outline"
          size="sm"
          onclick={() => navigate(parentPath(currentPath || "/"))}
          disabled={loading || !currentPath || currentPath === "/"}
        >
          {t("picker_upToParent")}
        </Button>
      {/if}
    </div>

    {#if hostName}
      <!-- Remote: show directory list -->
      <div class="flex items-center justify-between text-xs text-muted-foreground">
        <label class="inline-flex items-center gap-1.5 cursor-pointer">
          <input
            type="checkbox"
            checked={showHidden}
            onchange={(e) => {
              showHidden = (e.currentTarget as HTMLInputElement).checked;
              void onShowHiddenChange();
            }}
            class="h-3.5 w-3.5"
          />
          <span>{t("picker_showHidden")}</span>
        </label>
        {#if loading}
          <span>{t("picker_loading")}</span>
        {/if}
      </div>

      <div class="h-64 overflow-y-auto rounded-md border border-border bg-muted/30 p-1 text-sm">
        {#if error}
          <div class="flex flex-col gap-2 p-3 text-destructive">
            <div class="font-medium">{t("picker_remoteListError")}</div>
            <div class="text-xs whitespace-pre-wrap break-all">{error}</div>
            <div>
              <Button variant="outline" size="sm" onclick={() => navigate(currentPath || "/")}>
                {t("picker_retry")}
              </Button>
            </div>
          </div>
        {:else if loading && entries.length === 0}
          <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
            {t("picker_loading")}
          </div>
        {:else if entries.length === 0}
          <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
            {t("picker_emptyDir")}
          </div>
        {:else}
          {#each entries as entry (entry.name)}
            <button
              type="button"
              class="flex w-full items-center gap-2 rounded px-2 py-1 text-left hover:bg-accent hover:text-accent-foreground"
              onclick={() => navigate(joinPath(currentPath, entry.name))}
              ondblclick={() => navigate(joinPath(currentPath, entry.name))}
            >
              <svg
                class="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path
                  d="M20 19V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 5.93 3H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2Z"
                />
              </svg>
              <span class="truncate">{entry.name}</span>
            </button>
          {/each}
        {/if}
      </div>
    {:else}
      <!-- Local target -->
      <div class="flex flex-col gap-2 rounded-md border border-border bg-muted/30 p-4 text-sm">
        {#if isLocalDesktop}
          <Button variant="outline" onclick={browseLocal}>{t("picker_browseLocal")}</Button>
          <p class="text-xs text-muted-foreground">
            {t("picker_currentPath")}:
            <span class="font-mono">{currentPath || "—"}</span>
          </p>
        {:else}
          <p class="text-xs text-muted-foreground">
            {t("picker_currentPath")}:
            <span class="font-mono">{pathInput || currentPath || "—"}</span>
          </p>
          <p class="text-xs text-muted-foreground">
            {t("picker_pathInput")}
          </p>
        {/if}
      </div>
    {/if}

    <!-- Footer -->
    <div class="flex items-center justify-end gap-2 pt-2">
      <Button variant="ghost" onclick={cancel}>{t("picker_cancel")}</Button>
      <Button onclick={confirm} disabled={!canConfirm || loading}>
        {t("picker_useThisFolder")}
      </Button>
    </div>
  </div>
</Modal>

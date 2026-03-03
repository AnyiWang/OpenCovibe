<script lang="ts">
  import { onMount } from "svelte";
  import { beforeNavigate } from "$app/navigation";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import Button from "$lib/components/Button.svelte";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { dbgWarn } from "$lib/utils/debug";

  let viewMode = $state<"edit" | "preview">("edit");
  let content = $state("");
  let savedContent = $state("");
  let loading = $state(true);
  let saving = $state(false);
  let toastVisible = $state(false);
  let toastFading = $state(false);
  let error = $state("");

  // The cwd that was active when the current content was loaded.
  // Used by save() so that switching projects before saving doesn't break permissions.
  let saveCwd = $state("");

  // Custom file from ?file= query param (overrides sidebar selection)
  let customFile = $derived($page.url.searchParams.get("file") ?? "");

  // Selected file path — set by sidebar click or initial auto-select
  let selectedFile = $state("");

  let projectCwd = $state(
    typeof window !== "undefined" ? (localStorage.getItem("ocv:project-cwd") ?? "") : "",
  );

  let currentPath = $derived(customFile || selectedFile);

  // Page title: show filename for custom file, otherwise file label
  let pageTitle = $derived.by(() => {
    if (customFile) return customFile.split(/[/\\]/).pop() ?? "File";
    if (selectedFile) return selectedFile.split(/[/\\]/).pop() ?? "File";
    return "Memory";
  });

  // Only show preview toggle for markdown files
  let isMarkdown = $derived(currentPath.endsWith(".md"));

  // Dirty state: content differs from last saved/loaded version
  let isDirty = $derived(content !== savedContent);

  // Notify layout sidebar of dirty state
  $effect(() => {
    window.dispatchEvent(
      new CustomEvent("ocv:file-dirty", {
        detail: { path: currentPath, dirty: isDirty },
      }),
    );
  });

  // --- Sequence guards for race condition protection ---
  let loadSeq = 0;
  let projectChangeSeq = 0;
  let autoSelectSeq = 0;

  /** Load file content. Pass explicit path to avoid $derived timing issues in event callbacks. */
  async function loadContentForPath(explicitPath: string) {
    const seq = ++loadSeq;
    if (!explicitPath) {
      content = "";
      savedContent = "";
      loading = false;
      return;
    }
    loading = true;
    error = "";
    try {
      // Use saveCwd if already established (e.g. Reload after cancelled project switch),
      // fall back to projectCwd for first load or after confirmed project switch.
      const cwdSnapshot = saveCwd || projectCwd;
      const text = await api.readTextFile(explicitPath, cwdSnapshot || undefined);
      if (seq !== loadSeq) return; // stale — discard
      content = text;
      savedContent = text;
      saveCwd = cwdSnapshot;
    } catch (e) {
      if (seq !== loadSeq) return;
      const msg = String(e);
      if (msg.includes("No such file") || msg.includes("not found")) {
        content = "";
        savedContent = "";
        saveCwd = projectCwd;
      } else {
        content = "";
        savedContent = "";
        saveCwd = projectCwd;
        error = msg;
      }
    } finally {
      if (seq === loadSeq) loading = false;
    }
  }

  /** Convenience wrapper: load content for the current path. */
  function loadContent() {
    loadContentForPath(currentPath);
  }

  /** Auto-select first existing file from candidates (initial load). */
  async function autoSelectFirst() {
    const seq = ++autoSelectSeq;
    try {
      const candidates = await api.listMemoryFiles(projectCwd || undefined);
      if (seq !== autoSelectSeq) return; // stale — discard
      // Prefer first existing project file
      const existing = candidates.find((f) => f.exists && f.scope === "project");
      const fallback = candidates.find((f) => f.exists) ?? candidates[0];
      const pick = existing ?? fallback;
      if (pick) {
        selectedFile = pick.path;
        // Sync sidebar highlight — but only when not in customFile mode,
        // otherwise the sidebar would highlight a file the editor isn't showing.
        if (!customFile) {
          window.dispatchEvent(
            new CustomEvent("ocv:memory-file-selected", { detail: { path: pick.path } }),
          );
        }
      }
    } catch (e) {
      if (seq !== autoSelectSeq) return;
      dbgWarn("memory", "autoSelectFirst failed", e);
    }
  }

  /** Guard a file switch: confirm dirty state before switching.
   *  When `exists` is false the file hasn't been created yet — skip the API
   *  round-trip so the editor doesn't flash a loading spinner. */
  function guardedFileSwitch(newPath: string, exists = true) {
    if (newPath === selectedFile) return; // same file — no-op
    if (isDirty && !confirm(t("memory_discardConfirm"))) return;
    saveCwd = ""; // reset so next load uses current projectCwd
    selectedFile = newPath;
    // Ack sidebar: highlight now confirmed (layout waits for this before updating)
    window.dispatchEvent(
      new CustomEvent("ocv:memory-file-selected", { detail: { path: newPath } }),
    );
    if (exists) {
      loadContentForPath(newPath);
    } else {
      // New file — set empty content directly, no loading flash
      ++loadSeq; // cancel any in-flight load
      content = "";
      savedContent = "";
      loading = false;
      saveCwd = projectCwd;
    }
  }

  /** Async variant for project change: refresh candidates -> auto-select -> load. */
  async function guardedProjectChange(newCwd: string) {
    // Always sync projectCwd with layout (layout already committed the switch).
    // This prevents page vs sidebar project mismatch on cancel.
    projectCwd = newCwd;
    if (isDirty && !confirm(t("memory_discardConfirm"))) return;
    // Confirmed — reset saveCwd so loadContentForPath picks up the new projectCwd
    saveCwd = "";
    const seq = ++projectChangeSeq;
    await autoSelectFirst();
    if (seq !== projectChangeSeq) return;
    await loadContent();
  }

  // customFile (query param) changes are SvelteKit navigations —
  // already guarded by beforeNavigate's dirty confirm.
  // Use a non-reactive tracker to avoid state_referenced_locally warning.
  let _customFileInit = false;
  let _prevCustomFile: string | undefined;
  $effect(() => {
    const f = customFile;
    if (!_customFileInit) {
      // First run — record initial value, let onMount handle initial load
      _customFileInit = true;
      _prevCustomFile = f;
      return;
    }
    if (f === _prevCustomFile) return;
    _prevCustomFile = f;
    // Cancel any in-flight chains so they don't overwrite
    ++projectChangeSeq;
    ++loadSeq;
    ++autoSelectSeq;
    if (f) {
      // Entering customFile mode — load the custom file directly
      loadContentForPath(f);
    } else {
      // Exiting customFile mode — selectedFile may be stale (old project).
      // Re-select best file for current project, then load its content.
      autoSelectFirst().then(() => {
        loadContentForPath(selectedFile);
      });
    }
  });

  // Initial load
  onMount(async () => {
    if (!customFile) {
      await autoSelectFirst();
    }
    await loadContent();
  });

  // Listen for sidebar file selection
  onMount(() => {
    function onMemorySelect(e: Event) {
      const detail = (e as CustomEvent).detail;
      const path = detail?.path ?? "";
      if (path) {
        guardedFileSwitch(path, detail?.exists ?? true);
      }
    }
    window.addEventListener("ocv:memory-select", onMemorySelect);
    return () => window.removeEventListener("ocv:memory-select", onMemorySelect);
  });

  // Sync projectCwd when layout changes it
  onMount(() => {
    function onProjectChanged(e: Event) {
      const cwd = (e as CustomEvent).detail?.cwd ?? "";
      if (cwd === projectCwd) return;
      if (customFile) {
        projectCwd = cwd;
        // Fire-and-forget: refresh selectedFile for the new project so it's
        // correct when user later exits ?file= mode (no dirty check needed
        // since currentPath uses customFile, not selectedFile).
        autoSelectFirst();
        return;
      }
      guardedProjectChange(cwd);
    }
    window.addEventListener("ocv:project-changed", onProjectChanged);
    return () => window.removeEventListener("ocv:project-changed", onProjectChanged);
  });

  // Warn before navigating away with unsaved changes
  beforeNavigate(({ cancel }) => {
    if (isDirty && !confirm(t("memory_discardConfirm"))) {
      cancel();
    }
  });

  onMount(() => {
    function onBeforeUnload(e: BeforeUnloadEvent) {
      if (content !== savedContent) {
        e.preventDefault();
      }
    }
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  });

  async function save() {
    const path = currentPath;
    if (!path) return;
    saving = true;
    error = "";
    try {
      await api.writeTextFile(path, content, saveCwd || undefined);
      savedContent = content;
      // Notify layout to refresh candidates (updates exists status in sidebar)
      window.dispatchEvent(new Event("ocv:memory-file-saved"));
      toastFading = false;
      toastVisible = true;
      setTimeout(() => {
        toastFading = true;
        setTimeout(() => (toastVisible = false), 250);
      }, 2500);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }
</script>

<!-- Toast notification -->
{#if toastVisible}
  <div
    class="fixed top-4 left-1/2 -translate-x-1/2 z-50 {toastFading
      ? 'animate-toast-out'
      : 'animate-toast-in'}"
  >
    <div
      class="flex items-center gap-2 rounded-lg bg-emerald-600 px-4 py-2.5 text-sm font-medium text-white shadow-lg"
    >
      <svg
        class="h-4 w-4"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
      >
      {t("memory_saved")}
    </div>
  </div>
{/if}

<div class="flex h-full flex-col">
  <!-- Header bar: filename + dirty dot + path + edit/preview toggle -->
  <div class="flex items-center justify-between border-b px-4 py-2 shrink-0">
    <div class="flex items-center gap-3 min-w-0">
      <span class="text-sm font-medium truncate">{pageTitle}</span>
      {#if isDirty}
        <span class="h-2 w-2 rounded-full bg-primary shrink-0" title={t("memory_unsavedChanges")}
        ></span>
      {/if}
      {#if currentPath}
        <span
          class="text-[11px] text-muted-foreground truncate hidden sm:inline"
          title={currentPath}>{currentPath}</span
        >
      {/if}
    </div>
    <div class="flex items-center gap-2 shrink-0">
      {#if isMarkdown}
        <div class="flex rounded-md border bg-background p-0.5">
          <button
            class="flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium transition-colors
              {viewMode === 'edit'
              ? 'bg-muted text-foreground'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (viewMode = "edit")}
          >
            <svg
              class="h-3 w-3"
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
            {t("common_edit")}
          </button>
          <button
            class="flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium transition-colors
              {viewMode === 'preview'
              ? 'bg-muted text-foreground'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (viewMode = "preview")}
          >
            <svg
              class="h-3 w-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              ><path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" /><circle
                cx="12"
                cy="12"
                r="3"
              /></svg
            >
            {t("common_preview")}
          </button>
        </div>
      {/if}
    </div>
  </div>

  <!-- Content area -->
  {#if !currentPath}
    <div class="flex flex-1 flex-col items-center justify-center gap-3">
      <svg
        class="h-10 w-10 text-muted-foreground/30"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        ><path
          d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
        /></svg
      >
      <p class="text-sm text-muted-foreground">{t("memory_setProjectFirst")}</p>
    </div>
  {:else if loading}
    <div class="flex flex-1 items-center justify-center">
      <div
        class="h-6 w-6 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
    </div>
  {:else if viewMode === "preview" && isMarkdown}
    <div class="flex-1 overflow-y-auto p-4">
      {#if content}
        <MarkdownContent text={content} />
      {:else}
        <p class="text-sm text-muted-foreground italic">{t("memory_noContent")}</p>
      {/if}
    </div>
  {:else}
    <CodeEditor bind:content filePath={currentPath} onsave={save} class="flex-1" />
  {/if}

  <!-- Error -->
  {#if error}
    <div
      class="shrink-0 border-t border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive"
    >
      {error}
    </div>
  {/if}

  <!-- Bottom action bar -->
  {#if currentPath && !loading}
    <div class="flex items-center gap-3 border-t px-4 py-2 shrink-0">
      <Button onclick={save} loading={saving}>
        {#snippet children()}
          {t("common_save")}
        {/snippet}
      </Button>
      <Button variant="outline" onclick={loadContent}>
        {#snippet children()}
          {t("memory_reload")}
        {/snippet}
      </Button>
    </div>
  {/if}
</div>

<script lang="ts">
  import { onMount } from "svelte";
  import { beforeNavigate } from "$app/navigation";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import Button from "$lib/components/Button.svelte";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let tab = $state<"project" | "global">("project");
  let viewMode = $state<"edit" | "preview">("edit");
  let content = $state("");
  let savedContent = $state("");
  let loading = $state(true);
  let saving = $state(false);
  let toastVisible = $state(false);
  let toastFading = $state(false);
  let error = $state("");

  // Custom file from ?file= query param (overrides tab logic)
  let customFile = $derived($page.url.searchParams.get("file") ?? "");

  // Paths
  let globalPath = $state("");

  let projectCwd = $state(
    typeof window !== "undefined" ? (localStorage.getItem("ocv:project-cwd") ?? "") : "",
  );

  let currentPath = $derived.by(() => {
    if (customFile) return customFile;
    if (tab === "global") return globalPath;
    return projectCwd ? `${projectCwd}/CLAUDE.md` : "";
  });

  // Page title: show filename for custom file, otherwise "Memory"
  let pageTitle = $derived(customFile ? (customFile.split("/").pop() ?? "File") : "Memory");

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

  async function loadContent() {
    const path = currentPath;
    if (!path) {
      content = "";
      savedContent = "";
      loading = false;
      return;
    }
    loading = true;
    error = "";
    try {
      const text = await api.readTextFile(path, projectCwd || undefined);
      content = text;
      savedContent = text;
    } catch (e) {
      const msg = String(e);
      // File doesn't exist yet — show empty editor (user can create it by saving)
      if (msg.includes("No such file") || msg.includes("not found")) {
        content = "";
        savedContent = "";
      } else {
        content = "";
        savedContent = "";
        error = msg;
      }
    } finally {
      loading = false;
    }
  }

  // Reload when tab changes
  $effect(() => {
    // Access currentPath to establish dependency
    const _path = currentPath;
    loadContent();
  });

  // Resolve globalPath dynamically from user home directory
  onMount(async () => {
    try {
      const { homeDir, join } = await import("@tauri-apps/api/path");
      const home = await homeDir();
      globalPath = await join(home, ".claude", "CLAUDE.md");
    } catch {
      // Fallback: leave empty, global tab will show empty state
    }
  });

  // Sync projectCwd when layout changes it
  onMount(() => {
    function onProjectChanged(e: Event) {
      const cwd = (e as CustomEvent).detail?.cwd ?? "";
      if (cwd !== projectCwd) {
        projectCwd = cwd;
      }
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
      await api.writeTextFile(path, content, projectCwd || undefined);
      savedContent = content;
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
  <!-- Header bar -->
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

  <!-- Tabs (hidden when viewing a custom file) -->
  {#if !customFile}
    <div class="flex gap-1 border-b px-4 shrink-0">
      <button
        class="px-4 py-2 text-sm transition-colors {tab === 'project'
          ? 'border-b-2 border-primary font-medium'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (tab = "project")}
      >
        {t("memory_tabProject")}
      </button>
      <button
        class="px-4 py-2 text-sm transition-colors {tab === 'global'
          ? 'border-b-2 border-primary font-medium'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (tab = "global")}
      >
        {t("memory_tabGlobal")}
      </button>
    </div>
  {/if}

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

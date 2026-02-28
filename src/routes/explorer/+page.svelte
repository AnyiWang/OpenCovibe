<script lang="ts">
  import { getGitDiff, readTextFile, writeTextFile } from "$lib/api";
  import { dbg } from "$lib/utils/debug";
  import { fileName as pathFileName } from "$lib/utils/format";
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from "svelte";
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";

  // ── State ──

  let selectedFilePath = $state("");
  let diffViewFile = $state<string | null>(null);
  let diffViewContent = $state("");
  let diffViewLoading = $state(false);

  let fileContent = $state("");
  let fileLoading = $state(false);
  let fileSaving = $state(false);
  let fileDirty = $state(false);
  let fileError = $state("");
  let activeView = $state<"preview" | "diff">("preview");
  let editorMode = $state<"edit" | "rendered">("edit");

  const PREVIEWABLE_EXTENSIONS = new Set(["md", "markdown"]);
  let isPreviewable = $derived(
    PREVIEWABLE_EXTENSIONS.has(selectedFilePath.split(".").pop()?.toLowerCase() ?? ""),
  );

  let projectCwd = $state(
    typeof window !== "undefined" ? (localStorage.getItem("ocv:project-cwd") ?? "") : "",
  );

  // Track original content to detect dirty state
  let originalContent = "";

  // ── Diff parsing ──

  interface DiffLine {
    text: string;
    type: "add" | "del" | "context" | "hunk" | "header";
    oldNum: number | null;
    newNum: number | null;
  }

  function parseDiffLines(raw: string): DiffLine[] {
    const result: DiffLine[] = [];
    let oldLine = 0;
    let newLine = 0;
    for (const text of raw.split("\n")) {
      if (text.startsWith("@@")) {
        const match = text.match(/@@ -(\d+)(?:,\d+)? \+(\d+)/);
        if (match) {
          oldLine = parseInt(match[1], 10);
          newLine = parseInt(match[2], 10);
        }
        result.push({ text, type: "hunk", oldNum: null, newNum: null });
      } else if (
        text.startsWith("diff ") ||
        text.startsWith("index ") ||
        text.startsWith("---") ||
        text.startsWith("+++")
      ) {
        result.push({ text, type: "header", oldNum: null, newNum: null });
      } else if (text.startsWith("+")) {
        result.push({ text, type: "add", oldNum: null, newNum: newLine });
        newLine++;
      } else if (text.startsWith("-")) {
        result.push({ text, type: "del", oldNum: oldLine, newNum: null });
        oldLine++;
      } else {
        result.push({ text, type: "context", oldNum: oldLine, newNum: newLine });
        oldLine++;
        newLine++;
      }
    }
    return result;
  }

  // ── File preview ──

  async function loadFilePreview(path: string) {
    if (fileDirty && !confirm(t("explorer_discardConfirm"))) return;
    selectedFilePath = path;
    activeView = "preview";
    fileError = "";
    const ext = path.split(".").pop()?.toLowerCase() ?? "";
    editorMode = PREVIEWABLE_EXTENSIONS.has(ext) ? "rendered" : "edit";
    fileLoading = true;
    fileDirty = false;
    try {
      fileContent = await readTextFile(path, projectCwd);
      originalContent = fileContent;
      dbg("explorer", "file loaded", { path, size: fileContent.length });
    } catch (e) {
      fileContent = "";
      originalContent = "";
      fileError = String(e);
    } finally {
      fileLoading = false;
    }
  }

  async function saveFile() {
    if (!selectedFilePath || fileSaving || !fileDirty) return;
    fileSaving = true;
    try {
      await writeTextFile(selectedFilePath, fileContent, projectCwd);
      originalContent = fileContent;
      fileDirty = false;
      dbg("explorer", "file saved", { path: selectedFilePath });
    } catch (e) {
      dbg("explorer", "save error", e);
    } finally {
      fileSaving = false;
    }
  }

  // Track dirty state when CodeEditor updates content
  $effect(() => {
    if (!fileLoading) {
      fileDirty = fileContent !== originalContent;
    }
  });

  async function openFileDiff(filePath: string) {
    diffViewFile = filePath;
    activeView = "diff";
    diffViewLoading = true;
    diffViewContent = "";
    try {
      let content = await getGitDiff(projectCwd, false, filePath);
      if (!content.trim()) {
        content = await getGitDiff(projectCwd, true, filePath);
      }
      diffViewContent = content;
    } catch (e) {
      diffViewContent = String(e);
    } finally {
      diffViewLoading = false;
    }
  }

  function closeDiffView() {
    diffViewFile = null;
    diffViewContent = "";
    activeView = "preview";
  }

  function fileName(path: string): string {
    return pathFileName(path);
  }

  // ── Lifecycle ──

  onMount(() => {
    // Listen for file selection from sidebar (layout)
    function onExplorerFile(e: Event) {
      const path = (e as CustomEvent).detail?.path;
      if (path) loadFilePreview(path);
    }
    window.addEventListener("ocv:explorer-file", onExplorerFile);

    // Listen for diff selection from sidebar Git tab (layout)
    function onExplorerDiff(e: Event) {
      const path = (e as CustomEvent).detail?.path;
      if (path) openFileDiff(path);
    }
    window.addEventListener("ocv:explorer-diff", onExplorerDiff);

    // Listen for project cwd changes from layout
    function onProjectChanged(e: Event) {
      const cwd = (e as CustomEvent).detail?.cwd ?? "";
      if (cwd !== projectCwd) {
        if (fileDirty && !confirm(t("explorer_discardConfirm"))) return;
        projectCwd = cwd;
        selectedFilePath = "";
        fileContent = "";
        originalContent = "";
        fileDirty = false;
        fileError = "";
        diffViewFile = null;
        diffViewContent = "";
      }
    }
    window.addEventListener("ocv:project-changed", onProjectChanged);

    return () => {
      window.removeEventListener("ocv:explorer-file", onExplorerFile);
      window.removeEventListener("ocv:explorer-diff", onExplorerDiff);
      window.removeEventListener("ocv:project-changed", onProjectChanged);
    };
  });
</script>

<div class="flex h-full flex-col overflow-hidden">
  <!-- Preview / Diff area -->
  <div class="flex flex-1 flex-col overflow-hidden min-h-0">
    {#if activeView === "diff" && diffViewFile}
      <!-- Diff view header -->
      <div class="flex items-center gap-2 border-b px-4 py-2 shrink-0">
        <button
          class="flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={closeDiffView}
          title={t("explorer_closeDiff")}
        >
          <svg
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="m15 18-6-6 6-6" /></svg
          >
        </button>
        <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate"
          >{diffViewFile}</span
        >
      </div>
      <!-- Diff content -->
      <div class="flex-1 overflow-auto">
        {#if diffViewLoading}
          <div class="flex items-center justify-center py-12">
            <div
              class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
            ></div>
          </div>
        {:else if diffViewContent.trim()}
          {@const diffLines = parseDiffLines(diffViewContent)}
          <table class="w-full text-xs font-mono border-collapse">
            {#each diffLines as dl}
              <tr
                class={dl.type === "add"
                  ? "bg-green-500/10"
                  : dl.type === "del"
                    ? "bg-red-500/10"
                    : dl.type === "hunk"
                      ? "bg-blue-500/5"
                      : ""}
              >
                <td
                  class="select-none text-right pr-1 pl-2 text-muted-foreground/40 w-[1%] whitespace-nowrap {dl.type ===
                    'hunk' || dl.type === 'header'
                    ? 'border-y border-border/30'
                    : ''}">{dl.oldNum ?? ""}</td
                >
                <td
                  class="select-none text-right pr-2 text-muted-foreground/40 w-[1%] whitespace-nowrap {dl.type ===
                    'hunk' || dl.type === 'header'
                    ? 'border-y border-border/30'
                    : ''}">{dl.newNum ?? ""}</td
                >
                <td
                  class="whitespace-pre pr-4 {dl.type === 'add'
                    ? 'text-green-600 dark:text-green-400'
                    : dl.type === 'del'
                      ? 'text-red-500 dark:text-red-400'
                      : dl.type === 'hunk'
                        ? 'text-blue-500 dark:text-blue-400'
                        : dl.type === 'header'
                          ? 'font-bold text-foreground'
                          : ''} {dl.type === 'hunk' || dl.type === 'header'
                    ? 'border-y border-border/30 py-1'
                    : ''}">{dl.text}</td
                >
              </tr>
            {/each}
          </table>
        {:else}
          <div class="flex flex-col items-center gap-2 py-12 text-center">
            <svg
              class="h-8 w-8 text-muted-foreground/40"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"><path d="M20 6 9 17l-5-5" /></svg
            >
            <p class="text-sm text-muted-foreground">{t("explorer_noChanges")}</p>
          </div>
        {/if}
      </div>
    {:else if activeView === "preview" && selectedFilePath}
      <!-- File editor header -->
      <div class="flex items-center gap-2 border-b px-4 py-2 shrink-0">
        <svg
          class="h-3.5 w-3.5 shrink-0 opacity-40"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path
            d="M14 2v4a2 2 0 0 0 2 2h4"
          /></svg
        >
        <span class="text-sm font-medium text-foreground min-w-0 truncate"
          >{fileName(selectedFilePath)}</span
        >
        {#if fileDirty}
          <span class="h-2 w-2 rounded-full bg-amber-400 shrink-0" title={t("explorer_modified")}
          ></span>
        {/if}
        <span class="text-xs text-muted-foreground truncate flex-1 min-w-0">{selectedFilePath}</span
        >
        {#if isPreviewable}
          <div class="flex rounded-md border bg-background p-0.5 shrink-0">
            <button
              class="flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium transition-colors
                {editorMode === 'edit'
                ? 'bg-muted text-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (editorMode = "edit")}
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
                {editorMode === 'rendered'
                ? 'bg-muted text-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              onclick={() => (editorMode = "rendered")}
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
        <button
          class="rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors shrink-0 disabled:opacity-40 {fileDirty
            ? 'bg-primary text-primary-foreground hover:bg-primary/90'
            : 'bg-muted text-muted-foreground cursor-default'}"
          disabled={!fileDirty || fileSaving || editorMode === "rendered"}
          title={editorMode === "rendered" ? t("explorer_saveDisabledInPreview") : ""}
          onclick={saveFile}
        >
          {fileSaving ? t("explorer_saving") : t("explorer_save")}
        </button>
      </div>
      <!-- File content -->
      <div class="flex-1 overflow-hidden min-h-0">
        {#if fileLoading}
          <div class="flex items-center justify-center py-12">
            <div
              class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
            ></div>
          </div>
        {:else if fileError}
          <div class="flex flex-1 items-center justify-center p-4">
            <p class="text-sm text-destructive">{fileError}</p>
          </div>
        {:else if editorMode === "rendered" && isPreviewable}
          <div class="flex-1 overflow-y-auto p-4 h-full">
            {#if fileContent}
              <MarkdownContent text={fileContent} />
            {:else}
              <p class="text-sm text-muted-foreground italic">{t("explorer_emptyFile")}</p>
            {/if}
          </div>
        {:else}
          <CodeEditor
            bind:content={fileContent}
            filePath={selectedFilePath}
            onsave={saveFile}
            class="h-full"
          />
        {/if}
      </div>
    {:else}
      <!-- Empty state -->
      <div class="flex flex-1 items-center justify-center">
        <div class="flex flex-col items-center gap-2 text-center">
          <svg
            class="h-10 w-10 text-muted-foreground/20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" /><path
              d="M14 2v4a2 2 0 0 0 2 2h4"
            /></svg
          >
          <p class="text-sm text-muted-foreground">{t("explorer_selectFile")}</p>
        </div>
      </div>
    {/if}
  </div>
</div>

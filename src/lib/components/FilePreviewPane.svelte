<script lang="ts">
  import { getGitDiff, readTextFile, readFileBase64, writeTextFile } from "$lib/api";
  import { dbg } from "$lib/utils/debug";
  import { fileName as pathFileName } from "$lib/utils/format";
  import { t } from "$lib/i18n/index.svelte";
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import { classifyPath, getExtension, isImage, isPreviewable } from "$lib/utils/preview-ext";

  // ── Props ──
  let {
    cwd,
    path,
    mode = "preview",
    editable = false,
    isRemote = false,
    scopeKey = "",
    onLoaded,
    onLoadFailed,
    onCloseDiff,
    onDirtyChange,
  }: {
    cwd: string;
    path: string;
    mode?: "preview" | "diff";
    editable?: boolean;
    isRemote?: boolean;
    scopeKey?: string;
    onLoaded?: (path: string) => void;
    onLoadFailed?: (path: string, err: string) => void;
    onCloseDiff?: () => void;
    /** Fires whenever fileDirty transitions; parents can use this for navigation guards. */
    onDirtyChange?: (dirty: boolean) => void;
  } = $props();

  // ── State ──
  let fileContent = $state("");
  let imageDataUrl = $state("");
  let originalContent = "";
  let fileLoading = $state(false);
  let fileError = $state("");
  let fileDirty = $state(false);
  let fileSaving = $state(false);
  let editorMode = $state<"edit" | "rendered">("edit");

  let diffContent = $state("");
  let diffLoading = $state(false);

  let loadSeq = 0;

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

  // ── Loaders ──
  async function loadPreview(p: string, c: string): Promise<void> {
    const seq = ++loadSeq;
    fileError = "";
    const ext = getExtension(p);
    editorMode = isPreviewable(ext) ? "rendered" : "edit";
    fileLoading = true;
    fileDirty = false;
    imageDataUrl = "";

    try {
      if (isImage(ext)) {
        const [base64, mime] = await readFileBase64(p, c);
        if (seq !== loadSeq) return;
        imageDataUrl = `data:${mime};base64,${base64}`;
        fileContent = "";
        originalContent = "";
      } else {
        const content = await readTextFile(p, c);
        if (seq !== loadSeq) return;
        fileContent = content;
        originalContent = content;
      }
      dbg("preview-pane", "file loaded", { path: p, size: fileContent.length });
      onLoaded?.(p);
    } catch (e) {
      if (seq !== loadSeq) return;
      fileContent = "";
      originalContent = "";
      imageDataUrl = "";
      fileError = String(e);
      onLoadFailed?.(p, String(e));
    } finally {
      if (seq === loadSeq) fileLoading = false;
    }
  }

  async function loadDiff(p: string, c: string): Promise<void> {
    const seq = ++loadSeq;
    diffLoading = true;
    diffContent = "";
    try {
      let content = await getGitDiff(c, false, p);
      if (!content.trim()) {
        content = await getGitDiff(c, true, p);
      }
      if (seq !== loadSeq) return;
      diffContent = content;
    } catch (e) {
      if (seq !== loadSeq) return;
      diffContent = String(e);
    } finally {
      if (seq === loadSeq) diffLoading = false;
    }
  }

  async function saveFile(): Promise<void> {
    if (!path || fileSaving || !fileDirty || !editable || isRemote) return;
    // Snapshot what we're writing — user keystrokes during await must not be silently
    // marked as saved.
    const contentToSave = fileContent;
    fileSaving = true;
    try {
      await writeTextFile(path, contentToSave, cwd);
      originalContent = contentToSave;
      // Re-evaluate dirty against the latest content; if user typed during the write,
      // they remain dirty against the just-persisted snapshot.
      fileDirty = fileContent !== contentToSave;
      dbg("preview-pane", "file saved", { path, dirtyAfterSave: fileDirty });
    } catch (e) {
      dbg("preview-pane", "save error", e);
    } finally {
      fileSaving = false;
    }
  }

  // ── Reactive load ──
  // Svelte 5: read all reactive props inside the effect to register dependency tracking.
  $effect(() => {
    // Establish dependencies: cwd, path, mode, scopeKey, isRemote
    void scopeKey;
    const _cwd = cwd;
    const _path = path;
    const _mode = mode;
    const _isRemote = isRemote;

    // Reset on remote or empty path
    if (_isRemote || !_path) {
      ++loadSeq;
      fileLoading = false;
      diffLoading = false;
      fileContent = "";
      originalContent = "";
      imageDataUrl = "";
      diffContent = "";
      fileError = "";
      fileDirty = false;
      return;
    }

    if (_mode === "diff") {
      // Diff mode has no editable buffer — clear any lingering preview dirty state so
      // navigation guards (parent's onDirtyChange mirror) don't keep prompting after the
      // user already confirmed discard to enter diff.
      fileDirty = false;
      originalContent = fileContent;
      loadDiff(_path, _cwd);
    } else {
      loadPreview(_path, _cwd);
    }
  });

  // Track dirty state when CodeEditor updates content
  $effect(() => {
    if (!fileLoading) {
      fileDirty = fileContent !== originalContent;
    }
  });

  // Notify parent of dirty transitions (for navigation guards in editable contexts)
  let _lastDirty = false;
  $effect(() => {
    const d = fileDirty;
    if (d !== _lastDirty) {
      _lastDirty = d;
      onDirtyChange?.(d);
    }
  });

  // ── Derived ──
  let kind = $derived(classifyPath(path));
  let displayName = $derived(path ? pathFileName(path) : "");
  let canSave = $derived(editable && !isRemote && !fileSaving);
</script>

<div class="flex h-full flex-col overflow-hidden">
  {#if isRemote}
    <!-- Remote unsupported -->
    <div class="flex flex-1 items-center justify-center p-4">
      <div class="flex flex-col items-center gap-2 text-center">
        <svg
          class="h-8 w-8 text-muted-foreground/40"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          ><circle cx="12" cy="12" r="10" /><path d="M2 12h20" /><path
            d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"
          /></svg
        >
        <p class="text-sm text-muted-foreground">{t("preview_remoteUnsupported")}</p>
      </div>
    </div>
  {:else if !path}
    <!-- Empty state -->
    <div class="flex flex-1 items-center justify-center p-4">
      <div class="flex flex-col items-center gap-2 text-center">
        <svg
          class="h-8 w-8 text-muted-foreground/30"
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
        <p class="text-sm text-muted-foreground">{t("filesPanel_noPreviewSelected")}</p>
      </div>
    </div>
  {:else if mode === "diff"}
    <!-- Diff header -->
    <div class="flex items-center gap-2 border-b px-3 py-1.5 shrink-0">
      {#if onCloseDiff}
        <button
          class="flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          onclick={() => onCloseDiff?.()}
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
      {/if}
      <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">{path}</span>
    </div>
    <!-- Diff content -->
    <div class="flex-1 overflow-auto">
      {#if diffLoading}
        <div class="flex items-center justify-center py-12">
          <div
            class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
          ></div>
        </div>
      {:else if diffContent.trim()}
        {@const diffLines = parseDiffLines(diffContent)}
        <table class="w-full text-xs font-mono border-collapse">
          <tbody>
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
          </tbody>
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
  {:else}
    <!-- Preview header -->
    <div class="flex items-center gap-2 border-b px-3 py-1.5 shrink-0">
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
      <span class="text-sm font-medium text-foreground min-w-0 truncate">{displayName}</span>
      {#if fileDirty}
        <span class="h-2 w-2 rounded-full bg-amber-400 shrink-0" title={t("explorer_modified")}
        ></span>
      {/if}
      <span class="text-[11px] text-muted-foreground truncate flex-1 min-w-0">{path}</span>
      {#if kind === "markdown"}
        <div class="flex rounded-md border bg-background p-0.5 shrink-0">
          <button
            class="flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium transition-colors
              {editorMode === 'edit'
              ? 'bg-muted text-foreground'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => (editorMode = "edit")}
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
            {t("common_preview")}
          </button>
        </div>
      {/if}
      {#if editable && kind !== "image"}
        <button
          class="rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors shrink-0 disabled:opacity-40 {fileDirty
            ? 'bg-primary text-primary-foreground hover:bg-primary/90'
            : 'bg-muted text-muted-foreground cursor-default'}"
          disabled={!fileDirty || !canSave || editorMode === "rendered"}
          title={editorMode === "rendered" ? t("explorer_saveDisabledInPreview") : ""}
          onclick={saveFile}
        >
          {fileSaving ? t("explorer_saving") : t("explorer_save")}
        </button>
      {/if}
    </div>
    <!-- Preview content -->
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
      {:else if kind === "image" && imageDataUrl}
        <div
          class="flex items-center justify-center h-full overflow-auto p-4 bg-black/5 dark:bg-white/5"
        >
          <img
            src={imageDataUrl}
            alt={displayName}
            class="max-w-full max-h-full object-contain rounded"
          />
        </div>
      {:else if editorMode === "rendered" && kind === "markdown"}
        <div class="flex-1 overflow-y-auto p-4 h-full">
          {#if fileContent}
            <MarkdownContent text={fileContent} basePath={path.replace(/[/\\][^/\\]*$/, "")} />
          {:else}
            <p class="text-sm text-muted-foreground italic">{t("explorer_emptyFile")}</p>
          {/if}
        </div>
      {:else if editable}
        <CodeEditor bind:content={fileContent} filePath={path} onsave={saveFile} class="h-full" />
      {:else}
        <CodeEditor bind:content={fileContent} filePath={path} readonly class="h-full" />
      {/if}
    </div>
  {/if}
</div>

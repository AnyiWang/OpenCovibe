<script lang="ts">
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { onMount } from "svelte";
  import FilePreviewPane from "$lib/components/FilePreviewPane.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { getCachedFile, setCachedFile, clearCachedFile } from "$lib/utils/explorer-state";

  // ── State ──

  let selectedFilePath = $state("");
  let diffViewFile = $state<string | null>(null);
  let activeView = $state<"preview" | "diff">("preview");

  let projectCwd = $state(
    typeof window !== "undefined" ? (localStorage.getItem("ocv:project-cwd") ?? "") : "",
  );

  /** True while we're restoring from cache — onLoadFailed should clear that cache entry. */
  let restoringFromCache = false;

  /** Mirror of FilePreviewPane.fileDirty for navigation guards. */
  let paneDirty = $state(false);

  /** Returns true if it's safe to navigate away (no dirty changes, or user confirmed discard). */
  function canDiscardEdits(): boolean {
    if (!paneDirty) return true;
    return confirm(t("explorer_discardConfirm"));
  }

  // ── Pane control ──

  function selectFile(path: string) {
    if (path === selectedFilePath && activeView === "preview") return;
    if (!canDiscardEdits()) return;
    selectedFilePath = path;
    activeView = "preview";
    diffViewFile = null;
  }

  function openFileDiff(filePath: string) {
    if (!canDiscardEdits()) return;
    diffViewFile = filePath;
    activeView = "diff";
  }

  function closeDiffView() {
    diffViewFile = null;
    activeView = "preview";
  }

  function handleLoaded(p: string) {
    setCachedFile(projectCwd, p);
    restoringFromCache = false;
  }

  function handleLoadFailed(p: string, err: string) {
    if (restoringFromCache) {
      dbgWarn("explorer", "cache restore failed, clearing", { cwd: projectCwd, cached: p, err });
      clearCachedFile(projectCwd);
      selectedFilePath = "";
      restoringFromCache = false;
      window.dispatchEvent(new CustomEvent("ocv:explorer-file-selected", { detail: { path: "" } }));
    }
  }

  // ── Lifecycle ──

  onMount(() => {
    function onExplorerFile(e: Event) {
      const path = (e as CustomEvent).detail?.path;
      if (path) selectFile(path);
    }
    window.addEventListener("ocv:explorer-file", onExplorerFile);

    function onExplorerDiff(e: Event) {
      const path = (e as CustomEvent).detail?.path;
      if (path) openFileDiff(path);
    }
    window.addEventListener("ocv:explorer-diff", onExplorerDiff);

    function onProjectChanged(e: Event) {
      const cwd = (e as CustomEvent).detail?.cwd ?? "";
      if (cwd === projectCwd) return;
      if (!canDiscardEdits()) return;

      // Save current project state before switching
      if (projectCwd && selectedFilePath) {
        setCachedFile(projectCwd, selectedFilePath);
      }

      // Switch project; pane will see scopeKey change and reset
      projectCwd = cwd;
      diffViewFile = null;
      activeView = "preview";

      const cached = getCachedFile(cwd);
      if (cached) {
        dbg("explorer", "restoring cached file on project switch", { cwd, cached });
        restoringFromCache = true;
        selectedFilePath = cached;
        window.dispatchEvent(
          new CustomEvent("ocv:explorer-file-selected", { detail: { path: cached } }),
        );
      } else {
        dbg("explorer", "no cache for project, clearing", { cwd });
        selectedFilePath = "";
        window.dispatchEvent(
          new CustomEvent("ocv:explorer-file-selected", { detail: { path: "" } }),
        );
      }
    }
    window.addEventListener("ocv:project-changed", onProjectChanged);

    // Restore cached file state on mount
    const cached = getCachedFile(projectCwd);
    if (cached && !selectedFilePath) {
      dbg("explorer", "restoring cached file on mount", { cwd: projectCwd, cached });
      restoringFromCache = true;
      selectedFilePath = cached;
      window.dispatchEvent(
        new CustomEvent("ocv:explorer-file-selected", { detail: { path: cached } }),
      );
    }

    return () => {
      if (projectCwd && selectedFilePath) {
        dbg("explorer", "saving file state on unmount", {
          cwd: projectCwd,
          file: selectedFilePath,
        });
        setCachedFile(projectCwd, selectedFilePath);
      }
      window.removeEventListener("ocv:explorer-file", onExplorerFile);
      window.removeEventListener("ocv:explorer-diff", onExplorerDiff);
      window.removeEventListener("ocv:project-changed", onProjectChanged);
    };
  });

  // Path passed to pane: in diff mode use diffViewFile, else selectedFilePath
  let panePath = $derived(activeView === "diff" ? (diffViewFile ?? "") : selectedFilePath);
</script>

<div class="flex h-full flex-col overflow-hidden">
  <FilePreviewPane
    cwd={projectCwd}
    path={panePath}
    mode={activeView}
    editable={true}
    isRemote={false}
    scopeKey={projectCwd}
    onLoaded={handleLoaded}
    onLoadFailed={handleLoadFailed}
    onCloseDiff={closeDiffView}
    onDirtyChange={(d) => (paneDirty = d)}
  />
</div>

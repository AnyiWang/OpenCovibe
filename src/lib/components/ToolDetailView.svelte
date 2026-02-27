<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import type { BusToolItem } from "$lib/types";
  import {
    extractOutputText,
    getLanguageFromPath,
    isImagePath,
    isPlanFilePath,
    extractImageBlocks,
    copyToClipboard,
  } from "$lib/utils/tool-rendering";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import TeamToolDetail from "$lib/components/TeamToolDetail.svelte";
  import hljs from "highlight.js";
  import { structuredPatch } from "diff";

  const TEAM_TOOLS = new Set([
    "TeamCreate",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
    "TeamDelete",
    "SendMessage",
  ]);
  function isTeamTool(name: string): boolean {
    return TEAM_TOOLS.has(name);
  }

  let {
    tool,
    isInputStreaming = false,
  }: {
    tool: BusToolItem;
    isInputStreaming?: boolean;
  } = $props();

  // ── Helpers ──

  function escapeHtml(str: string): string {
    return str
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  /** Highlight entire code block — safe, never throws. */
  function highlightBlock(code: string, lang: string): string {
    if (!lang || !hljs.getLanguage(lang)) return escapeHtml(code);
    try {
      return hljs.highlight(code, { language: lang }).value;
    } catch {
      return escapeHtml(code);
    }
  }

  /** Render code with line numbers. Highlights the whole block first for correct
   *  multi-line syntax (block comments, strings, etc.), then splits by line. */
  function renderCodeWithLineNumbers(code: string, lang: string, startLine = 1): string {
    const lines = code.split("\n");
    const skipHighlight = lines.length > 500 || code.length > 100_000;
    const highlighted = skipHighlight ? escapeHtml(code) : highlightBlock(code, lang);
    return highlighted
      .split("\n")
      .map((line, i) => {
        const num = `<span class="tool-line-num">${startLine + i}</span>`;
        return `${num}${line}`;
      })
      .join("\n");
  }

  /** Render a diff hunk as a <table> with line numbers, +/- markers, and syntax
   *  highlighting. Table layout ensures perfect column alignment and full-width
   *  row backgrounds that work with horizontal scrolling. */
  function renderDiffHunk(hunk: PatchHunk, language: string): string {
    // Strip diff prefixes to get clean code for highlighting
    const cleanLines = hunk.lines.map((line) => {
      if (line.startsWith("+") || line.startsWith("-")) return line.slice(1);
      if (line.startsWith(" ")) return line.slice(1);
      return line;
    });

    // Highlight as single block for correct multi-line syntax (comments, strings, etc.)
    const highlighted = highlightBlock(cleanLines.join("\n"), language);
    const hlLines = highlighted.split("\n");

    // Build table rows
    let oldLine = hunk.oldStart;
    let newLine = hunk.newStart;

    const rows = hunk.lines
      .map((rawLine, i) => {
        const content = hlLines[i] ?? escapeHtml(cleanLines[i]);

        if (rawLine.startsWith("-")) {
          const row =
            `<tr class="diff-row-removed">` +
            `<td class="diff-gutter">${oldLine}</td>` +
            `<td class="diff-gutter"></td>` +
            `<td class="diff-sign diff-sign-del">-</td>` +
            `<td class="diff-code">${content}</td></tr>`;
          oldLine++;
          return row;
        } else if (rawLine.startsWith("+")) {
          const row =
            `<tr class="diff-row-added">` +
            `<td class="diff-gutter"></td>` +
            `<td class="diff-gutter">${newLine}</td>` +
            `<td class="diff-sign diff-sign-add">+</td>` +
            `<td class="diff-code">${content}</td></tr>`;
          newLine++;
          return row;
        } else {
          const row =
            `<tr class="diff-row-context">` +
            `<td class="diff-gutter">${oldLine}</td>` +
            `<td class="diff-gutter">${newLine}</td>` +
            `<td class="diff-sign"> </td>` +
            `<td class="diff-code">${content}</td></tr>`;
          oldLine++;
          newLine++;
          return row;
        }
      })
      .join("");

    return `<table class="diff-table">${rows}</table>`;
  }

  /** Adjust hunk line numbers when they're 1-based but we know the real file position.
   *  Uses originalFile + oldString to find the actual starting line. */
  function adjustHunkLineNumbers(
    hunks: PatchHunk[],
    oldString: string | undefined,
    originalFile: string | undefined,
  ): PatchHunk[] {
    if (!originalFile || !oldString || hunks.length === 0) return hunks;
    const idx = originalFile.indexOf(oldString);
    if (idx === -1) return hunks;
    const realStartLine = originalFile.substring(0, idx).split("\n").length;
    // If already correct, return as-is
    if (hunks[0].oldStart === realStartLine) return hunks;
    const offset = realStartLine - hunks[0].oldStart;
    if (offset === 0) return hunks;
    return hunks.map((h) => ({
      ...h,
      oldStart: h.oldStart + offset,
      newStart: h.newStart + offset,
    }));
  }

  /** Compute a unified diff from old_string + new_string, using the original file
   *  content (if available) to determine real line numbers. Returns PatchHunk[]. */
  function computeFallbackPatch(
    oldStr: string,
    newStr: string,
    originalFile?: string,
  ): PatchHunk[] {
    // Find real line offset by locating old_string in the original file
    let lineOffset = 0;
    if (originalFile) {
      const idx = originalFile.indexOf(oldStr);
      if (idx !== -1) {
        lineOffset = originalFile.substring(0, idx).split("\n").length - 1;
      }
    }

    const patch = structuredPatch("", "", oldStr, newStr, "", "", { context: 3 });
    // Adjust line numbers to real file positions
    if (lineOffset > 0) {
      for (const hunk of patch.hunks) {
        hunk.oldStart += lineOffset;
        hunk.newStart += lineOffset;
      }
    }
    return patch.hunks as PatchHunk[];
  }

  /** Check if content area exceeds collapsed height and needs expand toggle. */
  function countLines(text: string): number {
    return text.split("\n").length;
  }

  // ── Derived data ──

  let outputText = $derived(extractOutputText(tool.output));
  let imageBlocks = $derived(extractImageBlocks(tool.output));
  let filePath = $derived((tool.input?.file_path as string) ?? (tool.input?.path as string) ?? "");
  let lang = $derived(getLanguageFromPath(filePath));
  let isPlanFile = $derived(isPlanFilePath(filePath));

  // Structured file result from tool_use_result (Read tool)
  interface FileResultMeta {
    filePath: string;
    content: string;
    numLines: number;
    startLine: number;
    totalLines: number;
  }
  let fileResult = $derived(tool.tool_use_result?.file as FileResultMeta | undefined);
  // Prefer clean content from tool_use_result, fallback to extractOutputText
  let readContent = $derived(fileResult?.content ?? outputText);
  let readStartLine = $derived(fileResult?.startLine ?? 1);
  let readLineInfo = $derived(
    fileResult
      ? `Lines ${fileResult.startLine}\u2013${fileResult.startLine + fileResult.numLines - 1} of ${fileResult.totalLines}`
      : "",
  );

  // Structured Bash result from tool_use_result
  interface BashResultMeta {
    stdout: string;
    stderr: string;
    interrupted: boolean;
    isImage: boolean;
    noOutputExpected: boolean;
  }
  let bashResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "stdout" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as BashResultMeta)
      : undefined,
  );

  // Structured Edit result from tool_use_result (structuredPatch)
  interface PatchHunk {
    oldStart: number;
    oldLines: number;
    newStart: number;
    newLines: number;
    lines: string[];
  }
  interface EditResultMeta {
    filePath: string;
    structuredPatch: PatchHunk[];
    oldString?: string;
    newString?: string;
    originalFile?: string;
    userModified?: boolean;
    replaceAll?: boolean;
  }
  let editResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "structuredPatch" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as EditResultMeta)
      : undefined,
  );
  let editHasPatches = $derived(
    editResult?.structuredPatch != null && editResult.structuredPatch.length > 0,
  );

  // Structured Glob result from tool_use_result
  interface GlobResultMeta {
    filenames: string[];
    durationMs: number;
    numFiles: number;
    truncated: boolean;
  }
  let globResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "filenames" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as GlobResultMeta)
      : undefined,
  );

  // Structured Grep result from tool_use_result
  interface GrepResultMeta {
    mode: string;
    numFiles: number;
    filenames: string[];
    content?: string;
    numLines?: number;
  }
  let grepResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "numFiles" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as GrepResultMeta)
      : undefined,
  );

  // Structured WebFetch result from tool_use_result
  interface WebFetchResultMeta {
    bytes: number;
    code: number;
    codeText: string;
    result: string;
    durationMs: number;
    url: string;
  }
  let webFetchResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "code" in tool.tool_use_result &&
      "bytes" in tool.tool_use_result &&
      "codeText" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as WebFetchResultMeta)
      : undefined,
  );

  // Structured WebSearch result from tool_use_result
  interface WebSearchResultMeta {
    query: string;
    results: Array<
      { tool_use_id: string; content: Array<{ title: string; url: string }> } | string
    >;
    durationSeconds: number;
  }
  let webSearchResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "results" in tool.tool_use_result &&
      Array.isArray((tool.tool_use_result as Record<string, unknown>).results)
      ? (tool.tool_use_result as unknown as WebSearchResultMeta)
      : undefined,
  );

  // Structured Task (subagent) result from tool_use_result
  interface TaskResultMeta {
    status: string;
    totalToolUseCount?: number;
    totalDurationMs?: number;
    totalTokens?: number;
    agentId?: string;
    description?: string;
    outputFile?: string;
    prompt?: string;
  }
  let taskResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "totalToolUseCount" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as TaskResultMeta)
      : tool.tool_use_result != null &&
          typeof tool.tool_use_result === "object" &&
          (tool.tool_use_result as Record<string, unknown>).status === "async_launched"
        ? (tool.tool_use_result as unknown as TaskResultMeta)
        : undefined,
  );

  // Write: reuse editResult pattern for structuredPatch
  let writeResult = $derived(
    (tool.tool_name === "Write" || tool.tool_name === "write_file") &&
      tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "structuredPatch" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as EditResultMeta)
      : undefined,
  );
  let writeHasPatches = $derived(
    writeResult?.structuredPatch != null && writeResult.structuredPatch.length > 0,
  );

  // Structured TodoWrite result from tool_use_result
  interface TodoItem {
    content: string;
    status: "pending" | "in_progress" | "completed";
    activeForm: string;
  }
  interface TodoWriteResultMeta {
    oldTodos: TodoItem[];
    newTodos: TodoItem[];
  }
  let todoResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "newTodos" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as TodoWriteResultMeta)
      : undefined,
  );

  // Structured Skill result from tool_use_result
  interface SkillResultMeta {
    success: boolean;
    commandName: string;
    status?: string;
    agentId?: string;
    result?: string;
  }
  let skillResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "commandName" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as SkillResultMeta)
      : undefined,
  );

  // Structured ExitPlanMode result from tool_use_result
  interface ExitPlanResultMeta {
    plan?: string;
    filePath?: string;
    awaitingLeaderApproval?: boolean;
  }
  let exitPlanResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "plan" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as ExitPlanResultMeta)
      : undefined,
  );

  // Structured NotebookEdit result from tool_use_result
  interface NotebookEditResultMeta {
    new_source: string;
    cell_type: string;
    language: string;
    edit_mode: string;
    notebook_path: string;
    cell_id?: string;
    error?: string;
  }
  let notebookResult = $derived(
    tool.tool_use_result != null &&
      typeof tool.tool_use_result === "object" &&
      "new_source" in tool.tool_use_result
      ? (tool.tool_use_result as unknown as NotebookEditResultMeta)
      : undefined,
  );

  // Output expand/collapse
  let outputExpanded = $state(false);
  let outputLineCount = $derived(countLines(outputText));
  let needsExpand = $derived(outputLineCount > 20);

  let copyFeedback = $state<string | null>(null);
  let copyTimeout: ReturnType<typeof setTimeout> | undefined;

  function handleCopy(text: string) {
    copyToClipboard(text);
    copyFeedback = t("common_copied");
    clearTimeout(copyTimeout);
    copyTimeout = setTimeout(() => {
      copyFeedback = null;
    }, 1500);
  }
</script>

<div class="mt-2 space-y-1.5" onclick={(e) => e.stopPropagation()}>
  {#if tool.tool_name === "Bash" || tool.tool_name === "bash"}
    <!-- Bash: terminal-style rendering -->
    {#if tool.input?.command}
      <div
        class="tool-terminal overflow-y-auto relative group/copy {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <div class="text-emerald-400/80">$ {tool.input.command}</div>
        {#if bashResult}
          {#if bashResult.stdout}
            <div class="mt-1 text-neutral-300/80">{bashResult.stdout}</div>
          {/if}
          {#if bashResult.stderr}
            <div class="mt-1 text-red-400/80">{bashResult.stderr}</div>
          {/if}
          {#if bashResult.interrupted}
            <div class="mt-1 text-amber-400/80 text-[10px]">{t("tool_interrupted")}</div>
          {/if}
        {:else if outputText}
          <div class="mt-1 text-neutral-300/80">{outputText}</div>
        {/if}
        {#if isInputStreaming}
          <span class="inline-block w-1.5 h-3 ml-0.5 bg-emerald-400/50 animate-pulse align-middle"
          ></span>
        {/if}
        <button
          class="absolute top-1.5 right-1.5 text-[10px] text-neutral-500 hover:text-neutral-300 opacity-0 group-hover/copy:opacity-100 transition-opacity"
          onclick={() => handleCopy(`$ ${tool.input?.command}\n${outputText}`)}
          >{copyFeedback ?? t("common_copy")}</button
        >
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "Read" || tool.tool_name === "read_file"}
    <!-- Read: syntax-highlighted code with line numbers or image -->
    {#if filePath}
      <div class="tool-file-header flex items-center justify-between rounded-t">
        <span class="truncate">{filePath}</span>
        <div class="flex items-center gap-2 shrink-0">
          {#if readLineInfo}
            <span class="text-[10px] text-muted-foreground/60">{readLineInfo}</span>
          {/if}
          {#if readContent}
            <button
              class="text-[10px] text-muted-foreground hover:text-foreground transition-colors"
              onclick={() => handleCopy(readContent)}>{copyFeedback ?? t("common_copy")}</button
            >
          {/if}
        </div>
      </div>
    {/if}
    {#if isImagePath(filePath)}
      {#if imageBlocks.length > 0}
        {#each imageBlocks as img}
          <img
            src="data:{img.source.media_type};base64,{img.source.data}"
            alt={filePath}
            class="max-h-60 rounded border border-border/50"
            loading="lazy"
          />
        {/each}
      {:else if readContent}
        <div class="rounded bg-muted p-2 max-h-60 overflow-y-auto">
          <pre
            class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{readContent}</pre>
        </div>
      {/if}
    {:else if readContent}
      <div
        class="rounded bg-muted overflow-y-auto tool-code-block {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <pre
          class="text-xs font-mono whitespace-pre-wrap p-2 leading-relaxed">{@html renderCodeWithLineNumbers(
            readContent,
            lang,
            readStartLine,
          )}</pre>
      </div>
      {#if countLines(readContent) > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(countLines(readContent)) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "Edit" || tool.tool_name === "edit_file"}
    <!-- Edit: diff view — structured patch (preferred) or old/new fallback -->
    {#if filePath}
      <div class="tool-file-header rounded-t">{filePath}</div>
    {/if}
    {#if editHasPatches}
      <!-- Structured unified diff from tool_use_result (adjust line numbers if needed) -->
      {@const adjustedEditHunks = adjustHunkLineNumbers(
        editResult!.structuredPatch,
        editResult!.oldString ?? (tool.input?.old_string as string | undefined),
        editResult!.originalFile,
      )}
      <div
        class="diff-section overflow-x-auto overflow-y-auto {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        {#each adjustedEditHunks as hunk}
          <div
            class="px-3 py-1 bg-muted/50 text-[10px] font-mono text-muted-foreground/60 border-b border-border/30"
          >
            @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
          </div>
          {@html renderDiffHunk(hunk, lang)}
        {/each}
      </div>
      {@const patchLineCount = adjustedEditHunks.reduce((n, h) => n + h.lines.length, 0)}
      {#if patchLineCount > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(patchLineCount) })}
        </button>
      {/if}
    {:else if tool.input?.old_string != null || tool.input?.new_string != null}
      <!-- Fallback: compute unified diff from old_string / new_string -->
      {@const origFile = (tool.tool_use_result as Record<string, unknown> | undefined)
        ?.originalFile as string | undefined}
      {@const fallbackHunks = computeFallbackPatch(
        String(tool.input?.old_string ?? ""),
        String(tool.input?.new_string ?? ""),
        origFile,
      )}
      <div
        class="diff-section overflow-x-auto overflow-y-auto {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        {#each fallbackHunks as hunk}
          <div
            class="px-3 py-1 bg-muted/50 text-[10px] font-mono text-muted-foreground/60 border-b border-border/30"
          >
            @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
          </div>
          {@html renderDiffHunk(hunk, lang)}
        {/each}
      </div>
      {@const fallbackLineCount = fallbackHunks.reduce((n, h) => n + h.lines.length, 0)}
      {#if fallbackLineCount > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(fallbackLineCount) })}
        </button>
      {/if}
    {/if}
    {#if outputText}
      <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
        <div class="text-[10px] font-medium text-muted-foreground/60 mb-1 uppercase tracking-wider">
          {t("tool_result")}
        </div>
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if tool.tool_name === "Write" || tool.tool_name === "write_file"}
    <!-- Write: structuredPatch diff (overwrite) or content preview (new file) -->
    {#if filePath}
      <div class="tool-file-header rounded-t">{filePath}</div>
    {/if}
    <!-- Plan file (.claude/plans/*.md): render content as markdown instead of diff/code.
         This intentionally takes priority over writeHasPatches — plan files are meant to be
         read as formatted prose, not reviewed as code diffs. -->
    {#if isPlanFile && typeof tool.input?.content === "string"}
      {@const planText = tool.input.content as string}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={planText} />
      </div>
      {#if countLines(planText) > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(countLines(planText)) })}
        </button>
      {/if}
    {:else if writeHasPatches}
      <!-- Overwrite: structured unified diff from tool_use_result -->
      {@const adjustedWriteHunks = adjustHunkLineNumbers(
        writeResult!.structuredPatch,
        writeResult!.oldString ?? (tool.input?.old_string as string | undefined),
        writeResult!.originalFile,
      )}
      <div
        class="diff-section overflow-x-auto overflow-y-auto {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        {#each adjustedWriteHunks as hunk}
          <div
            class="px-3 py-1 bg-muted/50 text-[10px] font-mono text-muted-foreground/60 border-b border-border/30"
          >
            @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
          </div>
          {@html renderDiffHunk(hunk, lang)}
        {/each}
      </div>
      {@const writePatchLines = adjustedWriteHunks.reduce((n, h) => n + h.lines.length, 0)}
      {#if writePatchLines > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(writePatchLines) })}
        </button>
      {/if}
    {:else if tool.input?.content}
      <!-- New file: content preview -->
      {@const content = String(tool.input.content)}
      {@const lines = content.split("\n")}
      {@const truncated = lines.length > 20}
      {@const preview = truncated ? lines.slice(0, 20).join("\n") + "\n..." : content}
      <div class="rounded bg-muted max-h-60 overflow-y-auto relative group/copy">
        <pre
          class="text-xs font-mono whitespace-pre-wrap p-2 leading-relaxed">{@html renderCodeWithLineNumbers(
            preview,
            lang,
          )}</pre>
        {#if truncated}
          <div class="px-2 pb-1.5 text-[10px] text-muted-foreground/50">
            {t("tool_linesTotal", { count: String(lines.length) })}
          </div>
        {/if}
        <button
          class="absolute top-1.5 right-1.5 text-[10px] text-muted-foreground hover:text-foreground opacity-0 group-hover/copy:opacity-100 transition-opacity"
          onclick={() => handleCopy(content)}>{copyFeedback ?? t("common_copy")}</button
        >
      </div>
    {/if}
    {#if outputText}
      <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
        <div class="text-[10px] font-medium text-muted-foreground/60 mb-1 uppercase tracking-wider">
          {t("tool_result")}
        </div>
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if tool.tool_name === "Grep" || tool.tool_name === "search_files"}
    <!-- Grep: pattern + path input, monospace results -->
    <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
      <div class="text-xs font-mono text-muted-foreground flex items-center gap-2">
        <span>
          {#if tool.input?.pattern}<span class="text-purple-400">/{tool.input.pattern}/</span>{/if}
          {#if tool.input?.path}<span class="text-muted-foreground/60 ml-2">{tool.input.path}</span
            >{/if}
          {#if tool.input?.glob}<span class="text-muted-foreground/60 ml-2"
              >--glob {tool.input.glob}</span
            >{/if}
        </span>
        {#if grepResult}
          <span class="text-[10px] text-muted-foreground/50 ml-auto shrink-0">
            {grepResult.numFiles !== 1
              ? t("tool_files", { count: String(grepResult.numFiles) })
              : t("tool_file", { count: String(grepResult.numFiles) })}
            {#if grepResult.numLines != null}, {grepResult.numLines !== 1
                ? t("tool_matches", { count: String(grepResult.numLines) })
                : t("tool_match", { count: String(grepResult.numLines) })}{/if}
          </span>
        {/if}
      </div>
    </div>
    {#if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto relative group/copy {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
        <button
          class="absolute top-1.5 right-1.5 text-[10px] text-muted-foreground hover:text-foreground opacity-0 group-hover/copy:opacity-100 transition-opacity"
          onclick={() => handleCopy(outputText)}>{copyFeedback ?? t("common_copy")}</button
        >
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "Glob" || tool.tool_name === "list_directory"}
    <!-- Glob: pattern + path input, file list -->
    <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
      <div class="text-xs font-mono text-muted-foreground flex items-center gap-2">
        <span>
          {#if tool.input?.pattern}<span class="text-purple-400">{tool.input.pattern}</span>{/if}
          {#if tool.input?.path}<span class="text-muted-foreground/60 ml-2"
              >in {tool.input.path}</span
            >{/if}
        </span>
        {#if globResult}
          <span class="text-[10px] text-muted-foreground/50 ml-auto shrink-0">
            {globResult.numFiles !== 1
              ? t("tool_files", { count: String(globResult.numFiles) })
              : t("tool_file", { count: String(globResult.numFiles) })}
            {#if globResult.truncated}<span class="text-amber-400/80">
                {t("tool_truncated")}</span
              >{/if}
          </span>
        {/if}
      </div>
    </div>
    {#if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto {outputExpanded ? 'max-h-[80vh]' : 'max-h-60'}"
      >
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "WebFetch"}
    <!-- WebFetch: URL + HTTP status + response metadata -->
    <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
      <div class="text-xs font-mono text-muted-foreground flex items-center gap-2">
        <span class="text-sky-400 truncate">{tool.input?.url ?? ""}</span>
        {#if webFetchResult}
          <span class="ml-auto shrink-0 flex items-center gap-1.5 text-[10px]">
            <span
              class="px-1.5 py-0.5 rounded font-medium {webFetchResult.code < 400
                ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
                : 'bg-red-500/15 text-red-600 dark:text-red-400'}"
            >
              {webFetchResult.code}
              {webFetchResult.codeText}
            </span>
            <span class="text-muted-foreground/50">
              {webFetchResult.bytes < 1024
                ? `${webFetchResult.bytes} B`
                : `${(webFetchResult.bytes / 1024).toFixed(1)} KB`}
            </span>
          </span>
        {/if}
      </div>
    </div>
    {#if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={outputText} />
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "WebSearch"}
    <!-- WebSearch: query + structured result links -->
    <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
      <div class="text-xs font-mono text-muted-foreground truncate">
        <span class="text-sky-400">{tool.input?.query ?? ""}</span>
        {#if webSearchResult}
          <span class="text-[10px] text-muted-foreground/50 ml-2">
            {t("tool_resultsCount", {
              count: String(webSearchResult.results.filter((r) => typeof r !== "string").length),
            })}
          </span>
        {/if}
      </div>
    </div>
    {#if webSearchResult}
      <div
        class="rounded bg-muted p-2 overflow-y-auto space-y-1 {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        {#each webSearchResult.results as entry}
          {#if typeof entry !== "string" && entry.content}
            {#each entry.content as link}
              <a
                href={link.url}
                target="_blank"
                rel="noopener noreferrer"
                class="block text-xs text-sky-500 hover:text-sky-400 hover:underline truncate"
              >
                {link.title}
                <span class="text-muted-foreground/40 text-[10px] ml-1">{link.url}</span>
              </a>
            {/each}
          {/if}
        {/each}
      </div>
    {:else if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={outputText} />
      </div>
    {/if}
    {#if needsExpand}
      <button
        class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
        onclick={() => (outputExpanded = !outputExpanded)}
      >
        {outputExpanded
          ? t("common_collapse")
          : t("common_showAllLines", { count: String(outputLineCount) })}
      </button>
    {/if}
  {:else if tool.tool_name === "Task"}
    <!-- Task (subagent): prompt + type + usage stats -->
    <div class="rounded bg-muted p-2 max-h-20 overflow-y-auto">
      <div class="text-xs text-muted-foreground">
        {#if tool.input?.subagent_type}
          <span class="text-cyan-400 font-medium">{tool.input.subagent_type}</span>
        {/if}
        {#if tool.input?.prompt}
          <span class="ml-1 truncate">{tool.input.prompt}</span>
        {/if}
      </div>
    </div>
    {#if taskResult}
      <div class="flex items-center gap-2 px-2 py-1 text-[10px] text-muted-foreground/60">
        {#if taskResult.status === "async_launched"}
          <span
            class="px-1.5 py-0.5 rounded bg-blue-500/15 text-blue-600 dark:text-blue-400 font-medium"
            >{t("tool_async")}</span
          >
          {#if taskResult.outputFile}
            <span class="font-mono truncate">{taskResult.outputFile}</span>
          {/if}
        {:else}
          {#if taskResult.totalToolUseCount != null}
            <span>{t("tool_toolsCount", { count: String(taskResult.totalToolUseCount) })}</span>
          {/if}
          {#if taskResult.totalDurationMs != null}
            <span>{(taskResult.totalDurationMs / 1000).toFixed(1)}s</span>
          {/if}
          {#if taskResult.totalTokens != null}
            <span
              >{t("tool_tokensCount", {
                count:
                  taskResult.totalTokens >= 1000
                    ? `${(taskResult.totalTokens / 1000).toFixed(1)}k`
                    : String(taskResult.totalTokens),
              })}</span
            >
          {/if}
        {/if}
      </div>
    {/if}
    {#if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={outputText} />
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {:else if tool.tool_name === "TodoWrite"}
    <!-- TodoWrite: todo list with status badges -->
    {#if todoResult}
      <div
        class="rounded bg-muted p-2 overflow-y-auto {outputExpanded ? 'max-h-[80vh]' : 'max-h-60'}"
      >
        <div class="space-y-1">
          {#each todoResult.newTodos as todo}
            <div class="flex items-center gap-2 text-xs">
              <span
                class="px-1.5 py-0.5 rounded text-[10px] font-medium {todo.status === 'completed'
                  ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
                  : todo.status === 'in_progress'
                    ? 'bg-blue-500/15 text-blue-600 dark:text-blue-400'
                    : 'bg-neutral-500/15 text-muted-foreground'}"
              >
                {todo.status === "completed"
                  ? t("tool_statusDone")
                  : todo.status === "in_progress"
                    ? t("tool_statusWip")
                    : t("tool_statusTodo")}
              </span>
              <span
                class="text-muted-foreground {todo.status === 'completed'
                  ? 'line-through opacity-60'
                  : ''}">{todo.content}</span
              >
            </div>
          {/each}
        </div>
      </div>
      {#if todoResult.newTodos.length > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllItems", { count: String(todoResult.newTodos.length) })}
        </button>
      {/if}
    {:else if outputText}
      <div class="rounded bg-muted p-2 max-h-60 overflow-y-auto">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if tool.tool_name === "Skill"}
    <!-- Skill: command name + execution mode badge -->
    <div class="rounded bg-muted p-2 overflow-y-auto max-h-20">
      <div class="text-xs text-muted-foreground flex items-center gap-2">
        <span class="font-medium text-foreground"
          >{skillResult?.commandName ?? tool.input?.skill ?? ""}</span
        >
        {#if skillResult?.status}
          <span
            class="px-1.5 py-0.5 rounded text-[10px] font-medium {skillResult.status === 'forked'
              ? 'bg-purple-500/15 text-purple-600 dark:text-purple-400'
              : 'bg-blue-500/15 text-blue-600 dark:text-blue-400'}"
          >
            {skillResult.status}
          </span>
        {/if}
      </div>
    </div>
    {#if skillResult?.status === "forked" && skillResult.result}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={skillResult.result} />
      </div>
    {:else if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={outputText} />
      </div>
    {/if}
    {#if needsExpand}
      <button
        class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
        onclick={() => (outputExpanded = !outputExpanded)}
      >
        {outputExpanded
          ? t("common_collapse")
          : t("common_showAllLines", { count: String(outputLineCount) })}
      </button>
    {/if}
  {:else if tool.tool_name === "ExitPlanMode"}
    <!-- ExitPlanMode: plan content as markdown -->
    {#if exitPlanResult?.awaitingLeaderApproval}
      <div class="flex items-center gap-1.5 px-2 py-1">
        <span
          class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-amber-500/15 text-amber-600 dark:text-amber-400"
        >
          {t("tool_awaitingApproval")}
        </span>
      </div>
    {/if}
    {#if exitPlanResult?.plan}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={exitPlanResult.plan} />
      </div>
      {#if countLines(exitPlanResult.plan) > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(countLines(exitPlanResult.plan)) })}
        </button>
      {/if}
    {:else if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto prose-chat {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-60'}"
      >
        <MarkdownContent text={outputText} />
      </div>
    {/if}
  {:else if tool.tool_name === "NotebookEdit"}
    <!-- NotebookEdit: cell source with syntax highlighting -->
    {@const nbPath = notebookResult?.notebook_path ?? (tool.input?.notebook_path as string) ?? ""}
    {#if nbPath}
      <div class="tool-file-header flex items-center justify-between rounded-t">
        <span class="truncate">{nbPath}</span>
        {#if notebookResult?.edit_mode}
          <span
            class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-blue-500/15 text-blue-600 dark:text-blue-400 shrink-0"
          >
            {notebookResult.edit_mode}
          </span>
        {/if}
      </div>
    {/if}
    {#if notebookResult?.new_source}
      {@const nbLang = notebookResult.language || "python"}
      <div class="rounded bg-muted overflow-y-auto {outputExpanded ? 'max-h-[80vh]' : 'max-h-60'}">
        <pre
          class="text-xs font-mono whitespace-pre-wrap p-2 leading-relaxed">{@html renderCodeWithLineNumbers(
            notebookResult.new_source,
            nbLang,
          )}</pre>
      </div>
      {#if countLines(notebookResult.new_source) > 20}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(countLines(notebookResult.new_source)) })}
        </button>
      {/if}
    {:else if outputText}
      <div class="rounded bg-muted p-2 max-h-60 overflow-y-auto">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if isTeamTool(tool.tool_name)}
    <TeamToolDetail {tool} />
  {:else}
    <!-- Default: JSON input, plain text output -->
    {#if tool.input && Object.keys(tool.input).length > 0}
      <div class="rounded bg-muted p-2 max-h-40 overflow-y-auto relative group/copy">
        <div class="text-[10px] font-medium text-muted-foreground/60 mb-1 uppercase tracking-wider">
          {t("tool_input")}{#if isInputStreaming}<span
              class="inline-block w-1.5 h-3 ml-1 bg-muted-foreground/40 animate-pulse align-middle"
            ></span>{/if}
        </div>
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{@html highlightBlock(
            JSON.stringify(tool.input, null, 2),
            "json",
          )}</pre>
        <button
          class="absolute top-1.5 right-1.5 text-[10px] text-muted-foreground hover:text-foreground opacity-0 group-hover/copy:opacity-100 transition-opacity"
          onclick={() => handleCopy(JSON.stringify(tool.input, null, 2))}
          >{copyFeedback ?? t("common_copy")}</button
        >
      </div>
    {/if}
    {#if outputText}
      <div
        class="rounded bg-muted p-2 overflow-y-auto relative group/copy {outputExpanded
          ? 'max-h-[80vh]'
          : 'max-h-40'}"
      >
        <div class="text-[10px] font-medium text-muted-foreground/60 mb-1 uppercase tracking-wider">
          {t("tool_output")}
        </div>
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
        <button
          class="absolute top-1.5 right-1.5 text-[10px] text-muted-foreground hover:text-foreground opacity-0 group-hover/copy:opacity-100 transition-opacity"
          onclick={() => handleCopy(outputText)}>{copyFeedback ?? t("common_copy")}</button
        >
      </div>
      {#if needsExpand}
        <button
          class="w-full text-[10px] text-muted-foreground/60 hover:text-muted-foreground py-1 transition-colors"
          onclick={() => (outputExpanded = !outputExpanded)}
        >
          {outputExpanded
            ? t("common_collapse")
            : t("common_showAllLines", { count: String(outputLineCount) })}
        </button>
      {/if}
    {/if}
  {/if}
</div>

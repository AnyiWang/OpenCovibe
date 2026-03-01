<script lang="ts">
  import type { BusToolItem, TimelineEntry, PermissionSuggestion } from "$lib/types";
  import type { TaskNotificationItem } from "$lib/stores/session-store.svelte";
  import { getToolColor } from "$lib/utils/tool-colors";
  import { fileName as pathFileName, isAbsolutePath } from "$lib/utils/format";
  import {
    extractOutputText,
    friendlyToolName,
    planFileName,
    extractTaskToolMeta,
  } from "$lib/utils/tool-rendering";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";
  import ToolDetailView from "$lib/components/ToolDetailView.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let {
    tool,
    subTimeline,
    runId,
    fetchToolResult,
    onAnswer,
    onApprove,
    onPermissionRespond,
    onExitPlanClearContext,
    taskNotifications,
  }: {
    tool: BusToolItem;
    subTimeline?: TimelineEntry[];
    /** Run ID for lazy-loading truncated tool results. */
    runId?: string;
    /** Callback to fetch full tool result from backend (with caching). */
    fetchToolResult?: (runId: string, toolUseId: string) => Promise<Record<string, unknown> | null>;
    onAnswer?: (answer: string) => void;
    onApprove?: (toolName: string) => void;
    /** Inline permission response (--permission-prompt-tool stdio). */
    onPermissionRespond?: (
      requestId: string,
      behavior: "allow" | "deny",
      updatedPermissions?: PermissionSuggestion[],
      updatedInput?: Record<string, unknown>,
      denyMessage?: string,
      interrupt?: boolean,
    ) => void;
    /** ExitPlanMode "clear context" handler. */
    onExitPlanClearContext?: () => void;
    /** Background task notifications map (keyed by task_id, matched via tool_use_id). */
    taskNotifications?: Map<string, TaskNotificationItem>;
  } = $props();

  // Look up the task notification for this specific Task tool
  let taskNotification = $derived.by(() => {
    if (tool.tool_name !== "Task" || !taskNotifications) return undefined;
    for (const n of taskNotifications.values()) {
      if (n.tool_use_id === tool.tool_use_id) return n;
    }
    return undefined;
  });

  let userExpanded = $state<boolean | null>(null);
  let submitting = $state(false);

  // ── Lazy loading for truncated tool results ──
  let lazyResult = $state<Record<string, unknown> | null>(null);
  let lazyLoading = $state(false);
  let lazyFailed = $state(false);
  let lazyReqId = 0; // request generation marker

  let isTruncated = $derived(
    (tool.tool_use_result as Record<string, unknown> | undefined)?._truncated === true,
  );

  // Merge lazy-loaded result into tool for rendering
  let enrichedTool = $derived.by(() => {
    if (!lazyResult) return tool;
    return { ...tool, tool_use_result: lazyResult };
  });

  // Auto-fetch full result when expanded + truncated
  $effect(() => {
    if (!expanded || !isTruncated || lazyResult || lazyLoading || lazyFailed) return;
    if (!runId || !fetchToolResult) return;
    const reqId = ++lazyReqId;
    lazyLoading = true;
    fetchToolResult(runId, tool.tool_use_id)
      .then((r) => {
        if (reqId !== lazyReqId) return; // stale — component switched/collapsed
        lazyResult = r;
        if (!r) lazyFailed = true; // not found → terminal state
      })
      .catch(() => {
        if (reqId !== lazyReqId) return;
        lazyFailed = true;
      })
      .finally(() => {
        if (reqId === lazyReqId) lazyLoading = false;
      });
  });

  function retryLazyLoad() {
    lazyFailed = false; // reset — effect will re-trigger
  }
  let multiChecked: Record<string, boolean> = $state({});
  // Per-question answers for multi-question AskUserQuestion
  let questionAnswers: Record<string, string> = $state({});
  // Per-question "Other" mode state
  let otherActive: Record<string, boolean> = $state({});
  let otherText: Record<string, string> = $state({});
  // ExitPlanMode "keep planning" feedback text
  let planFeedback = $state("");

  // Reset submitting when tool status changes (e.g. permission_prompt → error/permission_denied)
  $effect(() => {
    void tool.status;
    submitting = false;
  });

  // Auto-expand when input is streaming in (running + has input data)
  let isInputStreaming = $derived(
    tool.status === "running" &&
      !isAsk &&
      tool.input &&
      Object.keys(tool.input).length > 0 &&
      (tool as Record<string, unknown>)._inputJsonAccum != null,
  );
  let expanded = $derived(userExpanded ?? isInputStreaming);

  let style = $derived(getToolColor(tool.tool_name));

  // Extract a human-readable detail from tool input (file path, command, pattern, etc.)
  let detail = $derived.by(() => {
    const input = tool.input;
    if (!input || Object.keys(input).length === 0) return "";
    return (
      (input.file_path as string) ??
      (input.path as string) ??
      (input.command as string) ??
      (input.pattern as string) ??
      (input.query as string) ??
      (input.url as string) ??
      (input.team_name as string) ??
      (input.subject as string) ??
      (input.taskId != null || input.task_id != null
        ? `#${input.taskId ?? input.task_id}`
        : undefined) ??
      (input.skill as string) ??
      (input.recipient as string) ??
      ""
    );
  });

  let planLabel = $derived(planFileName(detail));
  let displayDetail = $derived(planLabel ? t("inline_planLabel", { name: planLabel }) : detail);

  // Detect if detail looks like an absolute file path (truncate from the front)
  // Plan labels are not paths — skip RTL and path truncation for them.
  let isPathLikeDetail = $derived(!planLabel && isAbsolutePath(detail));

  // For Bash commands, show description (preferred) or truncated command (fallback)
  let bashDescription = $derived.by(() => {
    if (tool.tool_name !== "Bash" && tool.tool_name !== "bash") return "";
    return (tool.input?.description as string) ?? "";
  });
  let bashPreview = $derived.by(() => {
    if (tool.tool_name !== "Bash" && tool.tool_name !== "bash") return "";
    const cmd = (tool.input?.command as string) ?? "";
    return cmd.length > 80 ? cmd.slice(0, 80) + "..." : cmd;
  });

  // Task (subagent) meta: extract agent type + model for enhanced header
  let taskMeta = $derived(tool.tool_name === "Task" ? extractTaskToolMeta(tool.input) : null);

  // Status display
  let statusKind = $derived(
    tool.status === "success"
      ? "done"
      : tool.status === "error" || tool.status === "denied"
        ? "error"
        : tool.status === "permission_denied"
          ? "permission_denied"
          : tool.status === "permission_prompt"
            ? "permission_prompt"
            : "running",
  );

  // AskUserQuestion detection
  let isAsk = $derived(tool.tool_name === "AskUserQuestion");
  let isAskDenied = $derived(isAsk && tool.status === "permission_denied");

  // Parse ALL questions from the input (supports multi-question)
  interface ParsedOption {
    label: string;
    description: string;
  }
  interface ParsedQuestion {
    question: string;
    header: string;
    options: ParsedOption[];
    multiSelect: boolean;
  }
  function extractOptions(raw: unknown): ParsedOption[] {
    if (!Array.isArray(raw)) return [];
    return raw.map((o: unknown) => {
      if (typeof o === "string") return { label: o, description: "" };
      if (o && typeof o === "object" && "label" in o) {
        const obj = o as Record<string, unknown>;
        return {
          label: String(obj.label),
          description: typeof obj.description === "string" ? obj.description : "",
        };
      }
      return { label: String(o), description: "" };
    });
  }
  let parsedQuestions = $derived.by((): ParsedQuestion[] => {
    if (!isAsk || !tool.input) return [];
    const questions = tool.input.questions as unknown;
    if (Array.isArray(questions) && questions.length > 0) {
      return questions.map((q: unknown) => {
        const qr = q as Record<string, unknown>;
        return {
          question: typeof qr?.question === "string" ? qr.question : "",
          header: typeof qr?.header === "string" ? qr.header : "",
          options: extractOptions(qr?.options),
          multiSelect: qr?.multiSelect === true,
        };
      });
    }
    // Legacy single-question format
    if (typeof tool.input.question === "string") {
      return [
        {
          question: tool.input.question,
          header: "",
          options: extractOptions(tool.input.options as unknown),
          multiSelect: tool.input.multiSelect === true,
        },
      ];
    }
    return [];
  });

  // Backward-compat: first question's text, options, multiSelect (used by existing templates)
  let askQuestion = $derived(parsedQuestions[0]?.question ?? "");
  // askOptions: string[] for backward-compat (non-permission mode, multiSelect tracking, done state)
  let askOptions = $derived(parsedQuestions[0]?.options.map((o) => o.label) ?? ([] as string[]));
  let isMultiSelect = $derived(parsedQuestions[0]?.multiSelect ?? false);
  let hasMultipleQuestions = $derived(parsedQuestions.length > 1);

  // Track how many questions are answered (for multi-question submit)
  let allQuestionsAnswered = $derived(
    parsedQuestions.length > 0 && parsedQuestions.every((q) => !!questionAnswers[q.question]),
  );

  // Output text (used by AskUserQuestion answer display)
  let outputText = $derived(extractOutputText(tool.output));

  // All answers for AskUserQuestion (supports multi-question via tool_use_result.answers)
  let askAnswersMap = $derived.by((): Record<string, string> => {
    if (!isAsk) return {};
    // Primary: structured answers from tool_use_result (stream-json mode with updatedInput)
    const tur = tool.tool_use_result as Record<string, unknown> | undefined;
    if (tur?.answers && typeof tur.answers === "object") {
      return tur.answers as Record<string, string>;
    }
    // Fallback: single answer from output
    if (tool.output) {
      const a = (tool.output as Record<string, unknown>).answer;
      if (typeof a === "string" && askQuestion) return { [askQuestion]: a };
    }
    // Fallback: parse from output text
    if (outputText && askQuestion) return { [askQuestion]: outputText };
    return {};
  });

  // Annotations map for "Other" free text (from tool_use_result.annotations)
  let askAnnotationsMap = $derived.by((): Record<string, string> => {
    if (!isAsk) return {};
    const tur = tool.tool_use_result as Record<string, unknown> | undefined;
    if (tur?.annotations && typeof tur.annotations === "object") {
      const ann = tur.annotations as Record<string, unknown>;
      const result: Record<string, string> = {};
      for (const [q, v] of Object.entries(ann)) {
        if (v && typeof v === "object" && "notes" in (v as Record<string, unknown>)) {
          const notes = (v as Record<string, unknown>).notes;
          if (typeof notes === "string") result[q] = notes;
        }
      }
      return result;
    }
    return {};
  });

  // Backward-compat: first question's answer (for single-question display)
  let askAnswer = $derived(askQuestion ? (askAnswersMap[askQuestion] ?? "") : "");
  // Set of selected answers for matching (handles "A, B, C" multi-select format)
  let askAnswerSet = $derived.by(() => {
    if (!askAnswer) return new Set<string>();
    return new Set(
      askAnswer
        .split(", ")
        .map((s) => s.trim())
        .filter(Boolean),
    );
  });

  // Duration display
  let durationLabel = $derived(
    tool.duration_ms != null
      ? tool.duration_ms < 1000
        ? `${tool.duration_ms}ms`
        : `${(tool.duration_ms / 1000).toFixed(1)}s`
      : "",
  );

  // Elapsed time from tool_progress (shown while running)
  let elapsedLabel = $derived(
    tool.status === "running" && tool.elapsed_time_seconds != null
      ? `${tool.elapsed_time_seconds.toFixed(1)}s`
      : "",
  );

  // Output size label (shown when tool is complete)
  let outputSizeLabel = $derived.by(() => {
    if (tool.status !== "success" && tool.status !== "error") return "";
    const tur = tool.tool_use_result;
    if (tur && typeof tur === "object") {
      // Read: line info from file metadata
      const fileResult = tur.file as { numLines?: number; totalLines?: number } | undefined;
      if (fileResult?.totalLines != null) {
        if (fileResult.numLines != null && fileResult.numLines < fileResult.totalLines) {
          return `${fileResult.numLines}/${fileResult.totalLines} lines`;
        }
        return `${fileResult.totalLines} lines`;
      }
      // Glob: file count
      if ("filenames" in tur && "numFiles" in tur && !("mode" in tur)) {
        const n = tur.numFiles as number;
        return `${n} file${n !== 1 ? "s" : ""}`;
      }
      // Grep: file + match counts
      if ("numFiles" in tur && "mode" in tur) {
        const nf = tur.numFiles as number;
        const nl = tur.numLines as number | undefined;
        if (nl != null)
          return `${nf} file${nf !== 1 ? "s" : ""}, ${nl} match${nl !== 1 ? "es" : ""}`;
        return `${nf} file${nf !== 1 ? "s" : ""}`;
      }
      // Edit: patch line count
      if ("structuredPatch" in tur) {
        // Backend pre-computed counts (summary mode — lines array stripped)
        if ("_patchAdded" in tur || "_patchRemoved" in tur) {
          return `+${(tur._patchAdded as number) ?? 0} -${(tur._patchRemoved as number) ?? 0}`;
        }
        // Fallback: original lines traversal (live mode / small payload not truncated)
        const patches = tur.structuredPatch as Array<{ lines: string[] }> | undefined;
        if (patches?.length) {
          const added = patches.reduce(
            (n, h) => n + (h.lines?.filter((l: string) => l.startsWith("+")).length ?? 0),
            0,
          );
          const removed = patches.reduce(
            (n, h) => n + (h.lines?.filter((l: string) => l.startsWith("-")).length ?? 0),
            0,
          );
          return `+${added} -${removed}`;
        }
      }
      // Bash: interrupted indicator
      if ("interrupted" in tur && (tur.interrupted as boolean)) {
        return "interrupted";
      }
      // WebFetch: HTTP status + response size
      if ("code" in tur && "bytes" in tur && "codeText" in tur) {
        const code = tur.code as number;
        const bytes = tur.bytes as number;
        const size = bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
        return `${code} ${tur.codeText} \u00b7 ${size}`;
      }
      // WebSearch: result count
      if ("results" in tur && Array.isArray(tur.results)) {
        const count = (tur.results as unknown[]).filter((r) => typeof r !== "string").length;
        return `${count} result${count !== 1 ? "s" : ""}`;
      }
      // Task (subagent): usage stats or async
      if ("totalToolUseCount" in tur) {
        const tools = tur.totalToolUseCount as number;
        const ms = tur.totalDurationMs as number | undefined;
        const tokens = tur.totalTokens as number | undefined;
        const parts: string[] = [`${tools} tools`];
        if (ms != null) parts.push(`${(ms / 1000).toFixed(1)}s`);
        if (tokens != null)
          parts.push(`${tokens >= 1000 ? `${(tokens / 1000).toFixed(1)}k` : tokens} tok`);
        return parts.join(" \u00b7 ");
      }
      if ((tur as Record<string, unknown>).status === "async_launched") {
        return "async";
      }
      // TodoWrite: item count
      if ("newTodos" in tur) {
        const n = (tur.newTodos as unknown[]).length;
        return `${n} item${n !== 1 ? "s" : ""}`;
      }
    }
    // Fallback: count output lines
    const output = extractOutputText(tool.output);
    if (!output) return "";
    const lines = output.split("\n").length;
    if (lines <= 1) return "";
    return `${lines} lines`;
  });

  function multiCount(): number {
    return Object.values(multiChecked).filter(Boolean).length;
  }

  function toggleMulti(option: string) {
    multiChecked = { ...multiChecked, [option]: !multiChecked[option] };
  }

  async function handleAnswer(answer: string) {
    if (submitting) return;
    submitting = true;
    try {
      onAnswer?.(answer);
    } catch {
      submitting = false;
    }
  }

  function handleAskPermissionAllow(answer: string) {
    if (submitting || !onPermissionRespond || !tool.permission_request_id) return;
    if (hasMultipleQuestions) {
      // Multi-question: store answer and wait for all questions
      questionAnswers[askQuestion] = answer;
      return;
    }
    submitting = true;
    const answers: Record<string, string> = { [askQuestion]: answer };
    const updatedInput = { ...tool.input, answers };
    onPermissionRespond(tool.permission_request_id, "allow", undefined, updatedInput);
  }

  function submitMultiSelectPermission() {
    if (submitting || !onPermissionRespond || !tool.permission_request_id) return;
    let selected = Object.keys(multiChecked).filter((k) => multiChecked[k]);
    const otherVal = otherActive[askQuestion] && otherText[askQuestion]?.trim();
    if (otherVal) selected = [...selected, otherVal];
    if (selected.length === 0) return;
    submitting = true;
    const answers: Record<string, string> = { [askQuestion]: selected.join(", ") };
    const updatedInput = { ...tool.input, answers };
    onPermissionRespond(tool.permission_request_id, "allow", undefined, updatedInput);
  }

  // Multi-question: select answer for a specific question
  function selectQuestionAnswer(questionText: string, answer: string) {
    questionAnswers[questionText] = answer;
  }

  // Multi-question: submit all answers at once
  function submitAllQuestionAnswers() {
    if (submitting || !onPermissionRespond || !tool.permission_request_id) return;
    if (!allQuestionsAnswered) return;
    submitting = true;
    const annotations: Record<string, { notes: string }> = {};
    for (const [q, ans] of Object.entries(questionAnswers)) {
      if (ans === "Other" && otherText[q]?.trim()) {
        annotations[q] = { notes: otherText[q].trim() };
      }
    }
    const updatedInput = {
      ...tool.input,
      answers: { ...questionAnswers },
      ...(Object.keys(annotations).length > 0 ? { annotations } : {}),
    };
    onPermissionRespond(tool.permission_request_id, "allow", undefined, updatedInput);
  }

  // Single-select "Other" submit (permission mode)
  function handleAskPermissionOther(questionText: string) {
    if (submitting || !onPermissionRespond || !tool.permission_request_id) return;
    const text = (otherText[questionText] ?? "").trim();
    if (!text) return;
    submitting = true;
    const answers: Record<string, string> = { [questionText]: "Other" };
    const annotations: Record<string, { notes: string }> = {
      [questionText]: { notes: text },
    };
    const updatedInput = { ...tool.input, answers, annotations };
    onPermissionRespond(tool.permission_request_id, "allow", undefined, updatedInput);
  }

  function formatSuggestionLabel(s: PermissionSuggestion): string {
    if (s.type === "addRules" && s.rules?.length && s.behavior === "allow") {
      return t("inline_alwaysAllow") + ` ${s.rules[0]}`;
    }
    if (s.type === "setMode" && s.mode) {
      return t("inline_switchToMode", { mode: s.mode });
    }
    if (s.type === "addDirectories" && s.directories?.length) {
      return t("inline_addDirectory", { dir: s.directories[0] });
    }
    if (s.type === "additionalContext") {
      return t("inline_applyHookContext");
    }
    return `Apply: ${s.type}`;
  }
</script>

<!-- Inline tool card, left-aligned with assistant messages (ml-11 = avatar 8 + mr-3) -->
<div class="flex justify-start mb-2 animate-fade-in ml-11">
  <div class="w-full max-w-[80%]">
    {#if isAsk && (tool.status === "running" || tool.status === "ask_pending") && askQuestion}
      <!-- AskUserQuestion: show question + option buttons -->
      <div class="rounded-lg border border-yellow-500/30 bg-yellow-500/5 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d={style.icon} />
            </svg>
          </div>
          <span class="text-xs font-medium text-foreground">{t("inline_question")}</span>
          <div class="h-3 w-3 shrink-0">
            <div
              class="h-2.5 w-2.5 rounded-full border-2 border-border border-t-yellow-500 animate-spin"
            ></div>
          </div>
        </div>
        <MarkdownContent
          text={askQuestion}
          class="text-sm text-foreground mb-3 [&>*:last-child]:mb-0"
        />
        {#if askOptions.length > 0 && onAnswer}
          {#if isMultiSelect}
            <div class="flex flex-wrap items-center gap-2">
              {#each askOptions as option}
                <button
                  class="rounded-md border px-3 py-1.5 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed {multiChecked[
                    option
                  ]
                    ? 'border-primary bg-primary/10 text-primary'
                    : 'border-border bg-background text-foreground hover:bg-accent hover:border-ring/30'}"
                  disabled={submitting}
                  onclick={() => toggleMulti(option)}
                >
                  {#if multiChecked[option]}
                    <svg
                      class="inline h-3 w-3 mr-1 -mt-0.5"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d="M20 6 9 17l-5-5" />
                    </svg>
                  {/if}
                  {option}
                </button>
              {/each}
              <button
                class="rounded-md border border-dashed px-3 py-1.5 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed {otherActive[
                  askQuestion
                ]
                  ? 'border-primary bg-primary/10 text-primary'
                  : 'border-border bg-background text-muted-foreground hover:bg-accent hover:border-ring/30'}"
                disabled={submitting}
                onclick={() => {
                  otherActive = { ...otherActive, [askQuestion]: !otherActive[askQuestion] };
                }}
              >
                {t("inline_other")}
              </button>
              {#if otherActive[askQuestion]}
                <input
                  type="text"
                  bind:value={otherText[askQuestion]}
                  placeholder={t("inline_otherPlaceholder")}
                  class="w-full rounded-md border border-border bg-transparent px-2 py-1 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
                />
              {/if}
              <button
                class="rounded-md bg-primary px-4 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={submitting ||
                  (multiCount() === 0 &&
                    !(otherActive[askQuestion] && otherText[askQuestion]?.trim()))}
                onclick={() => {
                  const selected = Object.keys(multiChecked).filter((k) => multiChecked[k]);
                  const otherVal = otherActive[askQuestion] && otherText[askQuestion]?.trim();
                  if (otherVal) selected.push(otherVal);
                  if (selected.length > 0) handleAnswer(selected.join(", "));
                }}
              >
                {multiCount() > 0
                  ? t("inline_submitCount", { count: String(multiCount()) })
                  : t("inline_submit")}
              </button>
            </div>
          {:else}
            <div class="flex flex-wrap gap-2">
              {#each askOptions as option}
                <button
                  class="rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium text-foreground hover:bg-accent hover:border-ring/30 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                  disabled={submitting}
                  onclick={() => handleAnswer(option)}
                >
                  {option}
                </button>
              {/each}
              <button
                class="rounded-md border border-dashed border-border bg-background px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-accent hover:border-ring/30 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={submitting}
                onclick={() => {
                  otherActive = { ...otherActive, [askQuestion]: true };
                }}
              >
                {t("inline_other")}
              </button>
              {#if otherActive[askQuestion]}
                <div class="flex gap-1.5 w-full mt-0.5">
                  <input
                    type="text"
                    bind:value={otherText[askQuestion]}
                    placeholder={t("inline_otherPlaceholder")}
                    class="flex-1 rounded-md border border-border bg-transparent px-2 py-1 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
                  />
                  <button
                    class="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled={submitting || !otherText[askQuestion]?.trim()}
                    onclick={() => {
                      const text = otherText[askQuestion]?.trim();
                      if (text) handleAnswer(text);
                    }}
                  >
                    {t("inline_submitOther")}
                  </button>
                </div>
              {/if}
            </div>
          {/if}
        {/if}
      </div>
    {:else if isAsk && tool.status !== "running" && tool.status !== "ask_pending" && tool.status !== "permission_prompt"}
      <!-- AskUserQuestion done: show question(s) + options with selected highlighted -->
      <div class="rounded-lg border border-border/50 bg-muted/30 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d={style.icon} />
            </svg>
          </div>
          <span class="text-xs font-medium text-muted-foreground">{t("inline_question")}</span>
          {#if isAskDenied}
            <span
              class="ml-auto rounded-full border border-red-500/30 bg-red-500/10 px-2 py-0.5 text-[10px] font-medium text-red-500"
              >{t("common_denied")}</span
            >
          {:else}
            <svg
              class="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-400 shrink-0 ml-auto"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M20 6 9 17l-5-5" />
            </svg>
          {/if}
        </div>
        {#if hasMultipleQuestions}
          <!-- Multi-question done: show all questions with answers -->
          <div class="space-y-2.5">
            {#each parsedQuestions as pq}
              <div>
                {#if pq.header}
                  <span
                    class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground"
                    >{pq.header}</span
                  >
                {/if}
                <MarkdownContent
                  text={pq.question}
                  class="text-sm text-foreground mb-1 [&>*:last-child]:mb-0"
                />
                {#if pq.options.length > 0}
                  <div class="flex flex-wrap gap-1.5">
                    {#each pq.options as option}
                      {@const isSelected =
                        askAnswersMap[pq.question] === option.label ||
                        askAnswersMap[pq.question]?.split(", ").includes(option.label)}
                      <span
                        class="rounded-md border px-3 py-1 text-xs font-medium {isSelected
                          ? 'border-emerald-500/50 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
                          : 'border-border/50 bg-transparent text-muted-foreground/50'}"
                      >
                        {#if isSelected}
                          <svg
                            class="inline h-3 w-3 mr-0.5 -mt-0.5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                          >
                        {/if}
                        {option.label}
                      </span>
                    {/each}
                    {#if askAnnotationsMap[pq.question]}
                      <span
                        class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-400"
                      >
                        <svg
                          class="inline h-3 w-3 mr-0.5 -mt-0.5"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2.5"
                          stroke-linecap="round"
                          stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                        >
                        {askAnnotationsMap[pq.question]}
                      </span>
                    {/if}
                  </div>
                {:else if askAnnotationsMap[pq.question]}
                  <span
                    class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-400"
                  >
                    <svg
                      class="inline h-3 w-3 mr-0.5 -mt-0.5"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                    >
                    {askAnnotationsMap[pq.question]}
                  </span>
                {:else if askAnswersMap[pq.question]}
                  <span
                    class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-400"
                  >
                    <svg
                      class="inline h-3 w-3 mr-0.5 -mt-0.5"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                    >
                    {askAnswersMap[pq.question]}
                  </span>
                {/if}
              </div>
            {/each}
          </div>
        {:else}
          <!-- Single question done -->
          <MarkdownContent
            text={askQuestion}
            class="text-sm text-foreground mb-3 [&>*:last-child]:mb-0"
          />
          {#if askOptions.length > 0}
            <div class="flex flex-wrap gap-2">
              {#each askOptions as option}
                <span
                  class="rounded-md border px-3 py-1.5 text-xs font-medium transition-all {askAnswerSet.has(
                    option,
                  )
                    ? 'border-emerald-500/50 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
                    : 'border-border/50 bg-transparent text-muted-foreground/50'}"
                >
                  {#if askAnswerSet.has(option)}
                    <svg
                      class="inline h-3 w-3 mr-1 -mt-0.5"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d="M20 6 9 17l-5-5" />
                    </svg>
                  {/if}
                  {option}
                </span>
              {/each}
              {#if askAnnotationsMap[askQuestion]}
                <span
                  class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1.5 text-xs font-medium text-emerald-700 dark:text-emerald-400"
                >
                  <svg
                    class="inline h-3 w-3 mr-1 -mt-0.5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <path d="M20 6 9 17l-5-5" />
                  </svg>
                  {askAnnotationsMap[askQuestion]}
                </span>
              {/if}
            </div>
          {:else if askAnnotationsMap[askQuestion]}
            <span
              class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1.5 text-xs font-medium text-emerald-700 dark:text-emerald-400"
            >
              <svg
                class="inline h-3 w-3 mr-1 -mt-0.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M20 6 9 17l-5-5" />
              </svg>
              {askAnnotationsMap[askQuestion]}
            </span>
          {:else if askAnswer}
            <span
              class="rounded-md border border-emerald-500/50 bg-emerald-500/10 px-3 py-1.5 text-xs font-medium text-emerald-700 dark:text-emerald-400"
            >
              <svg
                class="inline h-3 w-3 mr-1 -mt-0.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M20 6 9 17l-5-5" />
              </svg>
              {askAnswer}
            </span>
          {/if}
        {/if}
      </div>
    {:else if isAsk && tool.status === "permission_prompt" && askQuestion && tool.permission_request_id}
      <!-- AskUserQuestion permission prompt: show question(s) + options with Allow/Deny semantics -->
      <div class="rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d={style.icon} />
            </svg>
          </div>
          <span class="text-xs font-medium text-foreground">
            {parsedQuestions.length > 1
              ? t("inline_questionsCount", {
                  answered: String(Object.keys(questionAnswers).length),
                  total: String(parsedQuestions.length),
                })
              : t("inline_question")}
          </span>
          {#if !submitting}
            <div class="h-3 w-3 shrink-0">
              <div
                class="h-2.5 w-2.5 rounded-full border-2 border-border border-t-amber-500 animate-spin"
              ></div>
            </div>
          {/if}
        </div>
        {#if onPermissionRespond}
          {#if hasMultipleQuestions}
            <!-- Multi-question layout -->
            <div class="space-y-3">
              {#each parsedQuestions as pq}
                <div>
                  {#if pq.header}
                    <span
                      class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground"
                      >{pq.header}</span
                    >
                  {/if}
                  <MarkdownContent
                    text={pq.question}
                    class="text-sm text-foreground mb-1.5 [&>*:last-child]:mb-0"
                  />
                  <div class="flex flex-wrap gap-1.5">
                    {#each pq.options as option}
                      <button
                        class="rounded-md border px-3 py-1 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed text-left {questionAnswers[
                          pq.question
                        ] === option.label
                          ? 'border-primary bg-primary/10 text-primary'
                          : 'border-border bg-background text-foreground hover:bg-accent hover:border-ring/30'}"
                        disabled={submitting}
                        onclick={() => {
                          otherActive = { ...otherActive, [pq.question]: false };
                          selectQuestionAnswer(pq.question, option.label);
                        }}
                      >
                        {#if questionAnswers[pq.question] === option.label}
                          <svg
                            class="inline h-3 w-3 mr-0.5 -mt-0.5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                          >
                        {/if}
                        <span>{option.label}</span>
                        {#if option.description}
                          <span
                            class="block text-[10px] text-muted-foreground/70 font-normal mt-0.5"
                          >
                            <MarkdownContent
                              text={option.description}
                              class="[&>*:last-child]:mb-0 [&_p]:text-[10px] [&_p]:leading-snug"
                            />
                          </span>
                        {/if}
                      </button>
                    {/each}
                    <!-- Other option -->
                    <button
                      class="rounded-md border border-dashed px-3 py-1 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed {otherActive[
                        pq.question
                      ] && questionAnswers[pq.question] === 'Other'
                        ? 'border-primary bg-primary/10 text-primary'
                        : 'border-border bg-background text-muted-foreground hover:bg-accent hover:border-ring/30'}"
                      disabled={submitting}
                      onclick={() => {
                        const wasActive = otherActive[pq.question];
                        otherActive = { ...otherActive, [pq.question]: !wasActive };
                        if (!wasActive) {
                          selectQuestionAnswer(pq.question, "Other");
                        } else if (questionAnswers[pq.question] === "Other") {
                          const { [pq.question]: _, ...rest } = questionAnswers;
                          questionAnswers = rest;
                        }
                      }}
                    >
                      {t("inline_other")}
                    </button>
                    {#if otherActive[pq.question]}
                      <input
                        type="text"
                        bind:value={otherText[pq.question]}
                        placeholder={t("inline_otherPlaceholder")}
                        class="w-full mt-0.5 rounded-md border border-border bg-transparent px-2 py-1 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
                      />
                    {/if}
                  </div>
                </div>
              {/each}
              <div class="flex gap-2 pt-1">
                <button
                  class="rounded-md bg-primary px-4 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                  disabled={submitting || !allQuestionsAnswered}
                  onclick={submitAllQuestionAnswers}
                >
                  {t("inline_submitCount", {
                    count: `${Object.keys(questionAnswers).length}/${parsedQuestions.length}`,
                  })}
                </button>
                <button
                  class="rounded-md border border-border px-4 py-1.5 text-xs font-medium text-muted-foreground hover:bg-accent transition-all disabled:opacity-50"
                  disabled={submitting}
                  onclick={() => {
                    submitting = true;
                    onPermissionRespond?.(tool.permission_request_id!, "deny");
                  }}>{t("common_deny")}</button
                >
              </div>
            </div>
          {:else if isMultiSelect}
            <!-- Single multi-select question -->
            <MarkdownContent
              text={askQuestion}
              class="text-sm text-foreground mb-3 [&>*:last-child]:mb-0"
            />
            <div class="flex flex-wrap items-center gap-2">
              {#each askOptions as option}
                <button
                  class="rounded-md border px-3 py-1.5 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed {multiChecked[
                    option
                  ]
                    ? 'border-primary bg-primary/10 text-primary'
                    : 'border-border bg-background text-foreground hover:bg-accent hover:border-ring/30'}"
                  disabled={submitting}
                  onclick={() => toggleMulti(option)}
                >
                  {#if multiChecked[option]}
                    <svg
                      class="inline h-3 w-3 mr-1 -mt-0.5"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d="M20 6 9 17l-5-5" />
                    </svg>
                  {/if}
                  {option}
                </button>
              {/each}
              <button
                class="rounded-md border border-dashed px-3 py-1.5 text-xs font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed {otherActive[
                  askQuestion
                ]
                  ? 'border-primary bg-primary/10 text-primary'
                  : 'border-border bg-background text-muted-foreground hover:bg-accent hover:border-ring/30'}"
                disabled={submitting}
                onclick={() => {
                  otherActive = { ...otherActive, [askQuestion]: !otherActive[askQuestion] };
                }}
              >
                {t("inline_other")}
              </button>
              {#if otherActive[askQuestion]}
                <input
                  type="text"
                  bind:value={otherText[askQuestion]}
                  placeholder={t("inline_otherPlaceholder")}
                  class="w-full rounded-md border border-border bg-transparent px-2 py-1 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
                />
              {/if}
              <button
                class="rounded-md bg-primary px-4 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={submitting ||
                  (multiCount() === 0 &&
                    !(otherActive[askQuestion] && otherText[askQuestion]?.trim()))}
                onclick={submitMultiSelectPermission}
              >
                {multiCount() > 0
                  ? t("inline_submitCount", { count: String(multiCount()) })
                  : t("inline_submit")}
              </button>
              <button
                class="rounded-md border border-border px-4 py-1.5 text-xs font-medium text-foreground hover:bg-accent transition-all disabled:opacity-50"
                disabled={submitting}
                onclick={() => {
                  submitting = true;
                  onPermissionRespond?.(tool.permission_request_id!, "deny");
                }}>{t("common_deny")}</button
              >
            </div>
          {:else}
            <!-- Single question, single select -->
            <MarkdownContent
              text={askQuestion}
              class="text-sm text-foreground mb-3 [&>*:last-child]:mb-0"
            />
            <div class="flex flex-wrap gap-2">
              {#each askOptions as option}
                <button
                  class="rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium text-foreground hover:bg-accent hover:border-ring/30 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                  disabled={submitting}
                  onclick={() => handleAskPermissionAllow(option)}
                >
                  {option}
                </button>
              {/each}
              <button
                class="rounded-md border border-dashed border-border bg-background px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-accent hover:border-ring/30 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={submitting}
                onclick={() => {
                  otherActive = { ...otherActive, [askQuestion]: true };
                }}
              >
                {t("inline_other")}
              </button>
              {#if otherActive[askQuestion]}
                <div class="flex gap-1.5 w-full mt-0.5">
                  <input
                    type="text"
                    bind:value={otherText[askQuestion]}
                    placeholder={t("inline_otherPlaceholder")}
                    class="flex-1 rounded-md border border-border bg-transparent px-2 py-1 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring"
                  />
                  <button
                    class="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled={submitting || !otherText[askQuestion]?.trim()}
                    onclick={() => handleAskPermissionOther(askQuestion)}
                  >
                    {t("inline_submitOther")}
                  </button>
                </div>
              {/if}
              <button
                class="rounded-md border border-border px-4 py-1.5 text-xs font-medium text-muted-foreground hover:bg-accent transition-all disabled:opacity-50"
                disabled={submitting}
                onclick={() => {
                  submitting = true;
                  onPermissionRespond?.(tool.permission_request_id!, "deny");
                }}>{t("common_deny")}</button
              >
            </div>
          {/if}
        {/if}
      </div>
    {:else if tool.status === "permission_prompt" && tool.permission_request_id && tool.tool_name === "ExitPlanMode"}
      <!-- ExitPlanMode: 4-option plan approval card (indigo theme) -->
      <div class="rounded-lg border border-indigo-500/30 bg-indigo-500/5 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-indigo-500/10">
            <svg
              class="h-3 w-3 text-indigo-500"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M9 11l3 3L22 4M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
            </svg>
          </div>
          <span class="text-xs font-medium text-foreground">{t("plan_readyToCode")}</span>
          <div class="h-3 w-3 shrink-0">
            <div
              class="h-2.5 w-2.5 rounded-full border-2 border-border border-t-indigo-500 animate-spin"
            ></div>
          </div>
        </div>

        <p class="text-xs text-muted-foreground mb-3">{t("plan_approvalDesc")}</p>

        <!-- allowedPrompts (from tool.input, set during tool_start) -->
        {#if tool.input?.allowedPrompts && Array.isArray(tool.input.allowedPrompts) && tool.input.allowedPrompts.length > 0}
          <div class="mb-3 rounded border border-indigo-500/10 bg-indigo-500/5 px-2.5 py-2">
            <p class="text-[10px] font-medium text-indigo-400 mb-1.5">
              {t("plan_requestedPermissions")}
            </p>
            <ul class="space-y-0.5">
              {#each tool.input.allowedPrompts as ap}
                {@const toolName = String((ap as Record<string, unknown>).tool ?? "")}
                {@const prompt = String((ap as Record<string, unknown>).prompt ?? "")}
                <li class="flex items-start gap-1.5 text-[10px] text-muted-foreground/80">
                  <span class="shrink-0 mt-0.5 text-indigo-400/60">&bull;</span>
                  <span
                    ><span class="font-medium text-indigo-300">{friendlyToolName(toolName)}</span
                    >{#if prompt}: {prompt}{/if}</span
                  >
                </li>
              {/each}
            </ul>
          </div>
        {/if}

        <!-- pushToRemote link (from tool.input, when ExitPlanMode sends remote session info) -->
        {#if tool.input?.pushToRemote && tool.input?.remoteSessionUrl}
          <a
            href={String(tool.input.remoteSessionUrl)}
            target="_blank"
            rel="noopener noreferrer"
            class="mb-3 flex items-center gap-1.5 rounded border border-blue-500/20 bg-blue-500/5 px-2.5 py-1.5 text-[11px] font-medium text-blue-400 hover:bg-blue-500/10 transition-colors w-fit"
          >
            <svg
              class="h-3 w-3 shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" /><polyline
                points="15 3 21 3 21 9"
              /><line x1="10" y1="14" x2="21" y2="3" />
            </svg>
            {t("plan_openRemote")}
          </a>
        {/if}

        {#if onPermissionRespond}
          <div class="flex flex-col gap-1.5">
            <!-- Option 1: Clear context + auto-accept -->
            <button
              class="w-full rounded-md border border-indigo-500/30 bg-indigo-500/10 px-3 py-1.5 text-left text-xs font-medium text-indigo-300 hover:bg-indigo-500/20 transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onExitPlanClearContext?.();
              }}
            >
              {t("plan_clearContextAutoAccept")}
            </button>
            <!-- Option 2: Auto-accept (keep context) -->
            <button
              class="w-full rounded-md border border-emerald-500/30 bg-emerald-500/10 px-3 py-1.5 text-left text-xs font-medium text-emerald-400 hover:bg-emerald-500/20 transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onPermissionRespond?.(
                  tool.permission_request_id!,
                  "allow",
                  [{ type: "setMode", mode: "acceptEdits", destination: "session" }],
                  tool.input,
                );
              }}
            >
              {t("plan_autoAcceptEdits")}
            </button>
            <!-- Option 3: Manually approve -->
            <button
              class="w-full rounded-md border border-border px-3 py-1.5 text-left text-xs font-medium text-foreground hover:bg-accent transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onPermissionRespond?.(tool.permission_request_id!, "allow", undefined, tool.input);
              }}
            >
              {t("plan_manuallyApprove")}
            </button>
            <!-- Option 4: Keep planning (with feedback) -->
            <div class="flex gap-1.5 items-end">
              <textarea
                bind:value={planFeedback}
                placeholder={t("plan_feedbackPlaceholder")}
                rows="1"
                class="flex-1 rounded-md border border-border bg-transparent px-2 py-1.5 text-xs placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-ring resize-none overflow-hidden"
                oninput={(e) => {
                  const el = e.currentTarget;
                  el.style.height = "auto";
                  el.style.height = Math.min(el.scrollHeight, 120) + "px";
                }}
                onkeydown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !submitting) {
                    e.preventDefault();
                    submitting = true;
                    const msg = planFeedback.trim() || undefined;
                    onPermissionRespond?.(
                      tool.permission_request_id!,
                      "deny",
                      undefined,
                      undefined,
                      msg,
                    );
                  }
                }}
              ></textarea>
              <button
                class="shrink-0 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-1.5 text-xs font-medium text-amber-400 hover:bg-amber-500/20 transition-all disabled:opacity-50"
                disabled={submitting}
                onclick={() => {
                  submitting = true;
                  const msg = planFeedback.trim() || undefined;
                  onPermissionRespond?.(
                    tool.permission_request_id!,
                    "deny",
                    undefined,
                    undefined,
                    msg,
                  );
                }}
              >
                {t("plan_keepPlanning")}
              </button>
            </div>
          </div>
        {/if}
      </div>
    {:else if tool.status === "permission_prompt" && tool.permission_request_id}
      <!-- Inline permission prompt (--permission-prompt-tool stdio): amber card with Allow/Deny -->
      <div class="rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d={style.icon} /></svg
            >
          </div>
          <span class="text-xs font-medium text-foreground">{t("inline_permissionRequired")}</span>
          <div class="h-3 w-3 shrink-0">
            <div
              class="h-2.5 w-2.5 rounded-full border-2 border-border border-t-amber-500 animate-spin"
            ></div>
          </div>
        </div>
        <p class="text-sm text-foreground mb-1">
          {t("inline_claudeWantsToUse")} <strong>{tool.tool_name}</strong>
        </p>
        {#if detail}
          <p
            class="text-xs text-muted-foreground mb-2 truncate"
            style:direction={isPathLikeDetail ? "rtl" : undefined}
            style:text-align={isPathLikeDetail ? "left" : undefined}
          >
            {#if isPathLikeDetail}<bdi>{detail}</bdi>{:else}{detail}{/if}
          </p>
        {/if}
        {#if onPermissionRespond}
          <div class="flex gap-2">
            <button
              class="rounded-md bg-emerald-600 px-4 py-1.5 text-xs font-medium text-white hover:bg-emerald-500 transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onPermissionRespond?.(tool.permission_request_id!, "allow", undefined, tool.input);
              }}>{t("common_allow")}</button
            >
            <button
              class="rounded-md border border-border px-4 py-1.5 text-xs font-medium text-foreground hover:bg-accent transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onPermissionRespond?.(tool.permission_request_id!, "deny");
              }}>{t("common_deny")}</button
            >
            <button
              class="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-1.5 text-xs font-medium text-red-500 hover:bg-red-500/20 transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onPermissionRespond?.(
                  tool.permission_request_id!,
                  "deny",
                  undefined,
                  undefined,
                  undefined,
                  true,
                );
              }}>{t("common_denyAndStop")}</button
            >
          </div>
          {#if tool.suggestions && tool.suggestions.length > 0}
            <div class="flex flex-wrap gap-2 mt-2 pt-2 border-t border-amber-500/20">
              {#each tool.suggestions as suggestion}
                {@const label = formatSuggestionLabel(suggestion)}
                <button
                  class="rounded-md border border-blue-500/30 bg-blue-500/5 px-3 py-1.5 text-xs font-medium text-blue-600 dark:text-blue-400 hover:bg-blue-500/10 transition-all disabled:opacity-50"
                  disabled={submitting}
                  onclick={() => {
                    submitting = true;
                    onPermissionRespond?.(
                      tool.permission_request_id!,
                      "allow",
                      [suggestion],
                      tool.input,
                    );
                  }}>{label}</button
                >
              {/each}
            </div>
          {/if}
        {/if}
      </div>
    {:else if tool.status === "permission_denied"}
      <!-- Permission denied (legacy: stop/restart flow): amber card with Allow/Deny -->
      <div class="rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3">
        <div class="flex items-center gap-2 mb-2">
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d={style.icon} /></svg
            >
          </div>
          <span class="text-xs font-medium text-foreground">{t("inline_permissionRequired")}</span>
          <svg
            class="h-3.5 w-3.5 text-amber-500 shrink-0 ml-auto"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path
              d="M12 9v4M12 17h.01M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"
            />
          </svg>
        </div>
        <p class="text-sm text-foreground mb-1">
          {t("inline_claudeWantsToUse")} <strong>{tool.tool_name}</strong>
        </p>
        {#if detail}
          <p
            class="text-xs text-muted-foreground mb-3 truncate"
            style:direction={isPathLikeDetail ? "rtl" : undefined}
            style:text-align={isPathLikeDetail ? "left" : undefined}
          >
            {#if isPathLikeDetail}<bdi>{detail}</bdi>{:else}{detail}{/if}
          </p>
        {/if}
        {#if onApprove}
          <div class="flex gap-2">
            <button
              class="rounded-md bg-emerald-600 px-4 py-1.5 text-xs font-medium text-white hover:bg-emerald-500 transition-all disabled:opacity-50"
              disabled={submitting}
              onclick={() => {
                submitting = true;
                onApprove?.(tool.tool_name);
              }}>{t("common_allow")}</button
            >
            <button
              class="rounded-md border border-border px-4 py-1.5 text-xs font-medium text-foreground hover:bg-accent transition-all"
              onclick={() => {
                /* no-op: user continues conversation */
              }}>{t("common_deny")}</button
            >
          </div>
        {/if}
      </div>
    {:else}
      <!-- Standard tool card: compact single-line, expandable -->
      <!-- div instead of button: ToolDetailView renders block elements (table, div)
           which are invalid inside <button> and cause browser DOM rearrangement -->
      <div
        role="button"
        tabindex="0"
        class="w-full text-left rounded-lg border border-border/50 bg-muted/30 px-3 py-2 hover:bg-muted/50 transition-colors group cursor-pointer"
        onclick={() => (userExpanded = userExpanded === null ? !isInputStreaming : !userExpanded)}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            userExpanded = userExpanded === null ? !isInputStreaming : !userExpanded;
          }
        }}
      >
        <div class="flex items-center gap-2">
          <!-- Tool icon -->
          <div class="flex h-5 w-5 shrink-0 items-center justify-center rounded {style.bg}">
            <svg
              class="h-3 w-3 {style.text}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d={style.icon} />
            </svg>
          </div>

          <!-- Tool name + detail -->
          <div class="flex-1 min-w-0 flex items-center gap-1.5">
            {#if taskMeta}
              <!-- Task tool: show agent type + model badge -->
              <span class="text-xs font-medium text-foreground">{taskMeta.subagentType}</span>
              {#if taskMeta.model}
                <span
                  class="text-[10px] px-1 py-0.5 rounded bg-cyan-500/15 text-cyan-600 dark:text-cyan-400 font-medium"
                  >{taskMeta.model}</span
                >
              {/if}
              {#if taskMeta.description}
                <span class="text-xs text-muted-foreground truncate">{taskMeta.description}</span>
              {/if}
            {:else}
              <span class="text-xs font-medium text-foreground">{tool.tool_name}</span>
              {#if displayDetail && !bashPreview}
                <span
                  class="text-xs text-muted-foreground truncate"
                  style:direction={isPathLikeDetail ? "rtl" : undefined}
                  style:text-align={isPathLikeDetail ? "left" : undefined}
                  >{#if isPathLikeDetail}<bdi>{displayDetail}</bdi>{:else}{displayDetail}{/if}</span
                >
              {:else if bashDescription}
                <span class="text-xs text-muted-foreground truncate">{bashDescription}</span>
              {:else if bashPreview}
                <span class="text-xs text-muted-foreground font-mono truncate">$ {bashPreview}</span
                >
              {:else if tool.status === "running"}
                <span class="text-xs text-muted-foreground italic">{t("inline_starting")}</span>
              {/if}
            {/if}
          </div>

          <!-- Duration + output size -->
          <div class="flex items-center gap-1.5 shrink-0">
            {#if outputSizeLabel}
              <span class="text-[10px] text-muted-foreground/40">{outputSizeLabel}</span>
            {/if}
            {#if durationLabel}
              <span class="text-[10px] text-muted-foreground/60">{durationLabel}</span>
            {:else if elapsedLabel}
              <span class="text-[10px] text-muted-foreground/60">{elapsedLabel}</span>
            {/if}
          </div>

          <!-- Status icon -->
          {#if statusKind === "done"}
            <svg
              class="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-400 shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M20 6 9 17l-5-5" />
            </svg>
          {:else if statusKind === "error"}
            <svg
              class="h-3.5 w-3.5 text-destructive shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          {:else}
            <div class="h-3.5 w-3.5 shrink-0">
              <div
                class="h-3 w-3 rounded-full border-2 border-border border-t-muted-foreground animate-spin"
              ></div>
            </div>
          {/if}

          <!-- Expand chevron -->
          <svg
            class="h-3 w-3 text-muted-foreground/40 shrink-0 transition-transform {expanded
              ? 'rotate-180'
              : ''}"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="m6 9 6 6 6-6" />
          </svg>
        </div>

        <!-- Tool summary (from tool_use_summary) -->
        {#if tool.summary}
          <div class="mt-1 text-xs text-muted-foreground italic truncate">{tool.summary}</div>
        {/if}

        <!-- Task notification status (background task) -->
        {#if taskNotification}
          <div class="mt-1 flex items-center gap-1.5 text-[10px] text-muted-foreground">
            {#if taskNotification.status === "running" || taskNotification.status === "pending"}
              <div
                class="h-2 w-2 rounded-full border border-border border-t-muted-foreground animate-spin shrink-0"
              ></div>
              <span>{taskNotification.summary || taskNotification.message}</span>
            {:else if taskNotification.status === "completed" || taskNotification.status === "done"}
              <svg
                class="h-2.5 w-2.5 text-emerald-500 shrink-0"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
              >
              <span>{taskNotification.summary || taskNotification.message}</span>
            {:else if taskNotification.status === "error" || taskNotification.status === "failed"}
              <svg
                class="h-2.5 w-2.5 text-destructive shrink-0"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                ><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg
              >
              <span>{taskNotification.summary || taskNotification.message}</span>
            {:else}
              <span
                >{taskNotification.status}: {taskNotification.summary ||
                  taskNotification.message}</span
              >
            {/if}
            {#if taskNotification.output_file}
              <button
                class="font-mono text-muted-foreground/60 truncate max-w-[150px] hover:text-foreground transition-colors underline decoration-dotted"
                title="Copy path: {taskNotification.output_file}"
                onclick={(e) => {
                  e.stopPropagation();
                  navigator.clipboard.writeText(taskNotification!.output_file!);
                }}
              >
                {pathFileName(taskNotification.output_file)}
              </button>
            {/if}
          </div>
        {/if}

        <!-- Expanded detail -->
        {#if expanded}
          {#if isTruncated && !lazyResult}
            <!-- Loading skeleton / error — don't render ToolDetailView with truncated data -->
            {#if lazyFailed}
              <div class="px-4 py-3 text-center text-xs text-muted-foreground">
                Failed to load details
                <button class="ml-2 underline hover:text-foreground" onclick={retryLazyLoad}
                  >Retry</button
                >
              </div>
            {:else}
              <div class="px-4 py-6 text-center text-xs text-muted-foreground animate-pulse">
                Loading tool details...
              </div>
            {/if}
          {:else}
            <ToolDetailView tool={enrichedTool} {isInputStreaming} />
          {/if}
        {/if}
      </div>
    {/if}
    <!-- Subagent subTimeline: nested entries from child agents -->
    {#if subTimeline && subTimeline.length > 0}
      <div class="mt-2 ml-4 pl-3 border-l-2 border-blue-500/30 space-y-1">
        {#each subTimeline as subEntry (subEntry.id)}
          {#if subEntry.kind === "assistant"}
            <div class="text-sm text-muted-foreground py-1">
              {#if subEntry.thinkingText}
                <pre
                  class="text-xs font-mono whitespace-pre-wrap break-words text-violet-300/70 italic mb-1 leading-relaxed">{subEntry.thinkingText}</pre>
              {/if}
              <MarkdownContent text={subEntry.content} />
            </div>
          {:else if subEntry.kind === "tool"}
            <svelte:self
              tool={subEntry.tool}
              subTimeline={subEntry.subTimeline}
              {runId}
              {fetchToolResult}
              {onAnswer}
              {onApprove}
              {onPermissionRespond}
              {taskNotifications}
            />
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</div>

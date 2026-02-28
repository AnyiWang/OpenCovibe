<script lang="ts">
  import type { BusToolItem } from "$lib/types";
  import { extractOutputText, extractStructuredOutput } from "$lib/utils/tool-rendering";
  import { t } from "$lib/i18n/index.svelte";
  import MarkdownContent from "$lib/components/MarkdownContent.svelte";

  let { tool }: { tool: BusToolItem } = $props();

  let outputText = $derived(extractOutputText(tool.output));
</script>

<!-- Team tool renderers -->
<div class="mt-2 space-y-1.5" onclick={(e) => e.stopPropagation()}>
  {#if tool.tool_name === "TeamCreate"}
    <!-- TeamCreate: team name + description -->
    <div class="rounded bg-muted p-2">
      <div class="text-xs text-muted-foreground">
        {#if tool.input?.team_name}
          <span class="font-medium text-teal-600 dark:text-teal-400">{tool.input.team_name}</span>
        {/if}
        {#if tool.input?.description}
          <p class="mt-1 text-muted-foreground/80 line-clamp-2">{tool.input.description}</p>
        {/if}
      </div>
    </div>
    {#if tool.status === "success" && outputText}
      <div class="rounded bg-muted p-2">
        <span
          class="inline-flex items-center gap-1 rounded-full bg-teal-500/10 px-2 py-0.5 text-[11px] font-medium text-teal-600 dark:text-teal-400"
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
          {t("tool_teamCreated")}
        </span>
      </div>
    {/if}
  {:else if tool.tool_name === "TaskCreate"}
    <!-- TaskCreate: subject + description + activeForm -->
    <div class="rounded bg-muted p-2 space-y-1">
      {#if tool.input?.subject}
        <div class="text-xs font-medium text-foreground">{tool.input.subject}</div>
      {/if}
      {#if tool.input?.description}
        <MarkdownContent text={String(tool.input.description)} class="text-xs text-muted-foreground line-clamp-3 [&>*:last-child]:mb-0" />
      {/if}
      {#if tool.input?.activeForm}
        <div class="text-[10px] text-muted-foreground/60 italic">{tool.input.activeForm}</div>
      {/if}
    </div>
    {#if tool.status === "success"}
      {@const raw = extractStructuredOutput(tool.output)}
      {@const taskId =
        raw && typeof raw === "object" && !Array.isArray(raw)
          ? ((raw as Record<string, unknown>).id ?? (raw as Record<string, unknown>).taskId)
          : null}
      {@const taskStatus =
        raw && typeof raw === "object" && !Array.isArray(raw)
          ? ((raw as Record<string, unknown>).status as string | undefined)
          : undefined}
      <div class="flex items-center gap-2">
        {#if taskId}
          <span
            class="inline-flex items-center rounded bg-muted px-1.5 py-0.5 text-[11px] font-mono font-medium text-foreground"
            >#{taskId}</span
          >
        {/if}
        {#if taskStatus}
          <span
            class="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium {taskStatus ===
            'completed'
              ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
              : taskStatus === 'in_progress'
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                : 'bg-neutral-500/10 text-muted-foreground'}">{taskStatus}</span
          >
        {/if}
      </div>
    {/if}
  {:else if tool.tool_name === "TaskUpdate"}
    <!-- TaskUpdate: task ID + changed fields -->
    <div class="rounded bg-muted p-2 space-y-1">
      {#if tool.input?.taskId || tool.input?.task_id}
        <span
          class="inline-flex items-center rounded bg-background px-1.5 py-0.5 text-[11px] font-mono font-medium text-foreground"
          >#{tool.input.taskId ?? tool.input.task_id}</span
        >
      {/if}
      {#if tool.input?.status}
        <div class="text-xs text-muted-foreground">
          {t("tool_labelStatus")}
          <span class="font-medium text-foreground">{tool.input.status}</span>
        </div>
      {/if}
      {#if tool.input?.subject}
        <div class="text-xs text-muted-foreground">
          {t("tool_labelSubject")}
          {tool.input.subject}
        </div>
      {/if}
      {#if tool.input?.owner}
        <div class="text-xs text-muted-foreground">{t("tool_labelOwner")} {tool.input.owner}</div>
      {/if}
      {#if tool.input?.addBlockedBy}
        <div class="text-xs text-muted-foreground">
          {t("tool_labelBlockedBy")}
          {#each tool.input.addBlockedBy as string[] as dep}
            <span
              class="inline-flex items-center rounded bg-background px-1 py-0.5 text-[10px] font-mono mr-1"
              >#{dep}</span
            >
          {/each}
        </div>
      {/if}
      {#if tool.input?.addBlocks}
        <div class="text-xs text-muted-foreground">
          {t("tool_labelBlocks")}
          {#each tool.input.addBlocks as string[] as dep}
            <span
              class="inline-flex items-center rounded bg-background px-1 py-0.5 text-[10px] font-mono mr-1"
              >#{dep}</span
            >
          {/each}
        </div>
      {/if}
    </div>
    {#if tool.status === "success"}
      {@const tur = tool.tool_use_result as Record<string, unknown> | undefined}
      {@const statusChange = tur?.statusChange as { from: string; to: string } | undefined}
      {@const raw = extractStructuredOutput(tool.output)}
      {@const updatedStatus =
        statusChange?.to ??
        (raw && typeof raw === "object" && !Array.isArray(raw)
          ? ((raw as Record<string, unknown>).status as string | undefined)
          : undefined)}
      {#if statusChange}
        <div class="flex items-center gap-1.5 text-[10px]">
          <span
            class="inline-flex items-center rounded-full px-1.5 py-0.5 font-medium {statusChange.from ===
            'completed'
              ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
              : statusChange.from === 'in_progress'
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                : 'bg-neutral-500/10 text-muted-foreground'}">{statusChange.from}</span
          >
          <span class="text-muted-foreground/60">&rarr;</span>
          <span
            class="inline-flex items-center rounded-full px-1.5 py-0.5 font-medium {statusChange.to ===
            'completed'
              ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
              : statusChange.to === 'in_progress'
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                : 'bg-neutral-500/10 text-muted-foreground'}">{statusChange.to}</span
          >
        </div>
      {:else if updatedStatus}
        <span
          class="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium {updatedStatus ===
          'completed'
            ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
            : updatedStatus === 'in_progress'
              ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
              : 'bg-neutral-500/10 text-muted-foreground'}">{updatedStatus}</span
        >
      {/if}
    {/if}
  {:else if tool.tool_name === "TaskList"}
    <!-- TaskList: mini task table -->
    {@const turTasks = (tool.tool_use_result as Record<string, unknown> | undefined)?.tasks as
      | Record<string, unknown>[]
      | undefined}
    {@const raw = turTasks ?? extractStructuredOutput(tool.output)}
    {@const tasks = Array.isArray(raw) ? (raw as Record<string, unknown>[]) : []}
    {#if tasks.length > 0}
      <div class="rounded bg-muted overflow-hidden">
        <table class="w-full text-xs">
          <thead>
            <tr class="border-b border-border/50 text-muted-foreground/60">
              <th class="px-2 py-1 text-left font-medium">{t("tool_taskListId")}</th>
              <th class="px-2 py-1 text-left font-medium">{t("tool_taskListSubject")}</th>
              <th class="px-2 py-1 text-left font-medium">{t("tool_taskListStatus")}</th>
              <th class="px-2 py-1 text-left font-medium">{t("tool_taskListOwner")}</th>
              <th class="px-2 py-1 text-left font-medium">{t("tool_taskListBlocked")}</th>
            </tr>
          </thead>
          <tbody>
            {#each tasks as task}
              <tr class="border-b border-border/30 last:border-0">
                <td class="px-2 py-1 font-mono text-foreground">#{task.id}</td>
                <td class="px-2 py-1 text-foreground truncate max-w-[200px]">{task.subject}</td>
                <td class="px-2 py-1">
                  <span
                    class="inline-flex items-center rounded-full px-1.5 py-0.5 text-[10px] font-medium {task.status ===
                    'completed'
                      ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                      : task.status === 'in_progress'
                        ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                        : 'bg-neutral-500/10 text-muted-foreground'}">{task.status}</span
                  >
                </td>
                <td class="px-2 py-1 text-muted-foreground">{task.owner ?? ""}</td>
                <td class="px-2 py-1">
                  {#if Array.isArray(task.blockedBy) && task.blockedBy.length > 0}
                    {#each task.blockedBy as dep}
                      <span
                        class="inline-flex items-center rounded bg-background px-1 py-0.5 text-[10px] font-mono mr-0.5"
                        >#{dep}</span
                      >
                    {/each}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if outputText}
      <div class="rounded bg-muted p-2">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if tool.tool_name === "TaskGet"}
    <!-- TaskGet: full task card -->
    <div class="rounded bg-muted p-2 space-y-1">
      {#if tool.input?.taskId || tool.input?.task_id}
        <span
          class="inline-flex items-center rounded bg-background px-1.5 py-0.5 text-[11px] font-mono font-medium text-foreground"
          >#{tool.input.taskId ?? tool.input.task_id}</span
        >
      {/if}
    </div>
    {#if tool.status === "success"}
      {@const raw = extractStructuredOutput(tool.output)}
      {@const task =
        raw && typeof raw === "object" && !Array.isArray(raw)
          ? (raw as Record<string, unknown>)
          : null}
      {#if task}
        <div class="rounded bg-muted p-2 space-y-1.5">
          <div class="flex items-center gap-2">
            <span class="text-xs font-medium text-foreground">{task.subject}</span>
            {#if task.status}
              <span
                class="inline-flex items-center rounded-full px-1.5 py-0.5 text-[10px] font-medium {task.status ===
                'completed'
                  ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                  : task.status === 'in_progress'
                    ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                    : 'bg-neutral-500/10 text-muted-foreground'}">{task.status}</span
              >
            {/if}
          </div>
          {#if task.description}
            <MarkdownContent text={String(task.description)} class="text-xs text-muted-foreground line-clamp-4 [&>*:last-child]:mb-0" />
          {/if}
          <div class="flex flex-wrap gap-2 text-[10px] text-muted-foreground/70">
            {#if task.owner}
              <span>{t("tool_labelOwner")} {task.owner}</span>
            {/if}
            {#if Array.isArray(task.blockedBy) && task.blockedBy.length > 0}
              <span
                >{t("tool_labelBlockedBy")}
                {#each task.blockedBy as dep}<span class="font-mono">#{dep}</span
                  >{#if dep !== task.blockedBy[task.blockedBy.length - 1]},
                  {/if}{/each}</span
              >
            {/if}
            {#if Array.isArray(task.blocks) && task.blocks.length > 0}
              <span
                >{t("tool_labelBlocks")}
                {#each task.blocks as dep}<span class="font-mono">#{dep}</span
                  >{#if dep !== task.blocks[task.blocks.length - 1]},
                  {/if}{/each}</span
              >
            {/if}
          </div>
        </div>
      {:else if outputText}
        <div class="rounded bg-muted p-2">
          <pre
            class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
        </div>
      {/if}
    {/if}
  {:else if tool.tool_name === "TeamDelete"}
    <!-- TeamDelete: confirmation -->
    {#if tool.status === "success"}
      <div class="rounded bg-muted p-2">
        <span
          class="inline-flex items-center gap-1 rounded-full bg-red-500/10 px-2 py-0.5 text-[11px] font-medium text-red-600 dark:text-red-400"
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
          {t("tool_teamDeleted")}
        </span>
      </div>
    {:else if outputText}
      <div class="rounded bg-muted p-2">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {:else if tool.tool_name === "SendMessage"}
    <!-- SendMessage: type badge + recipient + content -->
    <div class="rounded bg-muted p-2 space-y-1">
      <div class="flex items-center gap-2 text-xs">
        {#if tool.input?.type}
          <span
            class="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium {tool
              .input.type === 'broadcast'
              ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
              : tool.input.type === 'shutdown_request'
                ? 'bg-red-500/10 text-red-600 dark:text-red-400'
                : 'bg-violet-500/10 text-violet-600 dark:text-violet-400'}">{tool.input.type}</span
          >
        {/if}
        {#if tool.input?.recipient}
          <span class="text-muted-foreground"
            >{t("tool_messageTo")}
            <span class="font-medium text-foreground">{tool.input.recipient}</span></span
          >
        {/if}
      </div>
      {#if tool.input?.content}
        <MarkdownContent text={String(tool.input.content)} class="text-xs text-muted-foreground line-clamp-3 [&>*:last-child]:mb-0" />
      {/if}
      {#if tool.input?.summary}
        <div class="text-[10px] text-muted-foreground/60 italic">{tool.input.summary}</div>
      {/if}
    </div>
    {#if tool.status === "success"}
      <div class="rounded bg-muted p-2">
        <span
          class="inline-flex items-center gap-1 text-[11px] text-emerald-600 dark:text-emerald-400"
        >
          <svg
            class="h-3 w-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
          {t("tool_messageSent")}
        </span>
      </div>
    {/if}
  {:else}
    <!-- Fallback: raw JSON -->
    {#if tool.input && Object.keys(tool.input).length > 0}
      <div class="rounded bg-muted p-2 max-h-40 overflow-y-auto">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{JSON.stringify(
            tool.input,
            null,
            2,
          )}</pre>
      </div>
    {/if}
    {#if outputText}
      <div class="rounded bg-muted p-2 max-h-40 overflow-y-auto">
        <pre
          class="text-xs font-mono whitespace-pre-wrap break-all text-muted-foreground">{outputText}</pre>
      </div>
    {/if}
  {/if}
</div>

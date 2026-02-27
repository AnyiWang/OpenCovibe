<script lang="ts">
  import { getContext } from "svelte";
  import { getTeamTask } from "$lib/api";
  import type { TeamStore } from "$lib/stores/team-store.svelte";
  import type { TeamTask, TeamInboxMessage } from "$lib/types";
  import { dbg } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  const teamStore = getContext<TeamStore>("teamStore");

  // Collapsible sections (task board)
  let expandPending = $state(true);
  let expandInProgress = $state(true);
  let expandCompleted = $state(false);

  // Inbox tab: "all" or agent name
  let inboxTab = $state("all");

  // Task description loading cache
  let taskDescriptions = $state<Record<string, string>>({});
  let taskDescLoading = $state<Record<string, boolean>>({});

  // Message expand state (stable key = from::timestamp)
  let expandedMsgKey = $state<string | null>(null);

  // Sidebar (task board) collapsed state
  let sidebarCollapsed = $state(false);

  // Expanded member detail in status bar
  let expandedMemberName = $state<string | null>(null);

  // Responsive breakpoint
  let _isLargeScreen = $state(true);

  // Delete confirmation
  let deleteConfirm = $state<string | null>(null);
  let deleting = $state(false);

  // Reset local UI state when team changes (R1)
  $effect(() => {
    const _team = teamStore.selectedTeam;
    inboxTab = "all";
    expandedMsgKey = null;
    expandedMemberName = null;
    taskDescriptions = {};
    taskDescLoading = {};
    teamStore.expandedTaskId = null;
  });

  // Responsive breakpoint tracking (R2)
  $effect(() => {
    const mql = window.matchMedia("(min-width: 1024px)");
    _isLargeScreen = mql.matches;
    if (!mql.matches) sidebarCollapsed = true;
    const handler = (e: MediaQueryListEvent) => {
      _isLargeScreen = e.matches;
      if (!e.matches) sidebarCollapsed = true;
    };
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  });

  // Auto-select first team when teams load and none selected
  $effect(() => {
    if (teamStore.teams.length > 0 && !teamStore.selectedTeam) {
      teamStore.selectTeam(teamStore.teams[0].name);
    }
  });

  // Messages to display based on selected inbox tab
  let displayedMessages = $derived.by((): TeamInboxMessage[] => {
    if (inboxTab === "all") return teamStore.allInbox;
    return teamStore.inbox;
  });

  function msgKey(msg: TeamInboxMessage): string {
    return `${msg.from}::${msg.timestamp}`;
  }

  function handleInboxTabClick(agentName: string) {
    inboxTab = agentName;
    if (!teamStore.selectedTeam) return;
    teamStore.loadInbox(teamStore.selectedTeam, agentName);
  }

  function handleAllInboxClick() {
    inboxTab = "all";
    if (teamStore.selectedTeam) {
      teamStore.loadAllInbox(teamStore.selectedTeam);
    }
  }

  /** Toggle task description expand. Fetches full task on first expand. */
  async function toggleTaskExpand(task: TeamTask) {
    if (teamStore.expandedTaskId === task.id) {
      teamStore.expandedTaskId = null;
      return;
    }
    teamStore.expandedTaskId = task.id;

    // Load description if not cached and has a description to load
    if (!taskDescriptions[task.id] && !taskDescLoading[task.id]) {
      taskDescLoading = { ...taskDescLoading, [task.id]: true };
      try {
        const detail = await getTeamTask(teamStore.selectedTeam, task.id);
        taskDescriptions = { ...taskDescriptions, [task.id]: detail.description };
        dbg("teams", "task detail loaded", { id: task.id });
      } catch {
        taskDescriptions = {
          ...taskDescriptions,
          [task.id]: task.description || t("team_noDescription"),
        };
      } finally {
        taskDescLoading = { ...taskDescLoading, [task.id]: false };
      }
    }
  }

  /** Try to parse embedded JSON in inbox message text. */
  function parseMessageType(text: string): { type: string; data: Record<string, unknown> } | null {
    try {
      const parsed = JSON.parse(text);
      if (parsed?.type) {
        const payload =
          parsed.data && typeof parsed.data === "object" && !Array.isArray(parsed.data)
            ? parsed.data
            : parsed;
        return { type: parsed.type, data: payload };
      }
    } catch {
      /* plain text */
    }
    return null;
  }

  /** Relative time from epoch ms. */
  function timeAgoEpoch(epochMs: number): string {
    if (!epochMs || epochMs <= 0) return "";
    const diff = Date.now() - epochMs;
    const minutes = Math.floor(diff / 60000);
    if (minutes < 1) return t("team_justNow");
    if (minutes < 60) return t("team_minutesAgo", { count: String(minutes) });
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return t("team_hoursAgo", { count: String(hours) });
    return t("team_daysAgo", { count: String(Math.floor(hours / 24)) });
  }

  /** Relative time label from ISO string. */
  function timeAgo(timestamp: string): string {
    const ts = new Date(timestamp).getTime();
    if (isNaN(ts)) return "";
    return timeAgoEpoch(ts);
  }

  /** Truncate long paths for display. */
  function truncatePath(path: string, maxLen: number = 35): string {
    if (path.length <= maxLen) return path;
    return "..." + path.slice(path.length - maxLen + 3);
  }

  async function handleDeleteTeam(name: string) {
    deleting = true;
    try {
      await teamStore.deleteTeam(name);
      deleteConfirm = null;
    } catch (e) {
      console.error("delete team failed:", e);
    } finally {
      deleting = false;
    }
  }

  /** Color dot CSS class from member color string. */
  function memberColorClass(color: string): string {
    const map: Record<string, string> = {
      purple: "bg-purple-500",
      blue: "bg-blue-500",
      green: "bg-green-500",
      red: "bg-red-500",
      orange: "bg-orange-500",
      yellow: "bg-yellow-500",
      cyan: "bg-cyan-500",
      pink: "bg-pink-500",
      teal: "bg-teal-500",
    };
    return map[color] ?? "bg-muted-foreground";
  }

  /** Text color for inbox message sender. */
  function msgColorClass(color: string): string {
    const map: Record<string, string> = {
      purple: "text-purple-500",
      blue: "text-blue-500",
      green: "text-green-500",
      red: "text-red-500",
      orange: "text-orange-500",
      yellow: "text-yellow-500",
      cyan: "text-cyan-500",
      pink: "text-pink-500",
      teal: "text-teal-500",
    };
    return map[color] ?? "text-muted-foreground";
  }

  /** Short model display name. */
  function shortModel(model: string): string {
    if (!model) return "";
    // New format: claude-sonnet-4-5-20250929 → "Sonnet 4.5"
    const newFmt = model.match(/(opus|sonnet|haiku)-(\d+)-(\d+)/i);
    if (newFmt) {
      const name = newFmt[1].charAt(0).toUpperCase() + newFmt[1].slice(1);
      return `${name} ${newFmt[2]}.${newFmt[3]}`;
    }
    // Legacy format: claude-3-5-sonnet-20241022 → "Sonnet 3.5"
    const legacyFmt = model.match(/(\d+)-(\d+)-(opus|sonnet|haiku)/i);
    if (legacyFmt) {
      const name = legacyFmt[3].charAt(0).toUpperCase() + legacyFmt[3].slice(1);
      return `${name} ${legacyFmt[1]}.${legacyFmt[2]}`;
    }
    if (model.includes("opus")) return "Opus";
    if (model.includes("sonnet")) return "Sonnet";
    if (model.includes("haiku")) return "Haiku";
    return model.split("-").slice(0, 2).join("-");
  }
</script>

<div class="h-full overflow-hidden">
  {#if teamStore.teams.length === 0 && !teamStore.loading}
    <!-- Empty state -->
    <div class="flex flex-col items-center justify-center h-full text-center px-6">
      <div
        class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border border-border bg-muted"
      >
        <svg
          class="h-6 w-6 text-muted-foreground"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
          <circle cx="9" cy="7" r="4" />
          <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
          <path d="M16 3.13a4 4 0 0 1 0 7.75" />
        </svg>
      </div>
      <h2 class="text-sm font-medium text-foreground mb-1">{t("team_noActiveTeams")}</h2>
      <p class="text-xs text-muted-foreground max-w-sm">
        {t("team_emptyDesc")}
      </p>
    </div>
  {:else if teamStore.loading}
    <div class="flex items-center justify-center h-full">
      <div
        class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
    </div>
  {:else if teamStore.selectedTeam && teamStore.teamConfig}
    <div class="flex h-full overflow-hidden">
      <!-- Left: Status Bar + Conversation -->
      <div class="flex flex-1 flex-col min-w-0">
        <!-- ═══ Team Status Bar ═══ -->
        <div class="shrink-0 border-b border-border">
          <!-- Row 1: team name + badges + description + delete -->
          <div class="flex items-center gap-3 px-4 h-9">
            <div class="flex-1 min-w-0 flex items-center gap-2">
              <h1 class="text-sm font-semibold text-foreground truncate">
                {teamStore.teamConfig.name}
              </h1>
              <span
                class="rounded-full bg-teal-500/10 px-2 py-0.5 text-[10px] font-medium text-teal-600 dark:text-teal-400"
                >{t("team_membersCount", {
                  count: String(teamStore.teamConfig.members.length),
                })}</span
              >
              <span
                class="rounded-full bg-blue-500/10 px-2 py-0.5 text-[10px] font-medium text-blue-600 dark:text-blue-400"
                >{t("team_tasksCount", { count: String(teamStore.tasks.length) })}</span
              >
              {#if teamStore.teamConfig.createdAt}
                <span class="text-[10px] text-muted-foreground/50"
                  >{timeAgoEpoch(teamStore.teamConfig.createdAt)}</span
                >
              {/if}
              {#if teamStore.teamConfig.description}
                <span class="text-[10px] text-muted-foreground truncate hidden sm:inline"
                  >{teamStore.teamConfig.description}</span
                >
              {/if}
            </div>
            <!-- Delete -->
            <div class="shrink-0">
              {#if deleteConfirm === teamStore.selectedTeam}
                <div class="flex items-center gap-1.5">
                  <span class="text-xs text-muted-foreground">{t("team_deleteConfirm")}</span>
                  <button
                    class="rounded px-2 py-1 text-xs font-medium bg-destructive text-destructive-foreground hover:bg-destructive/90 transition-colors disabled:opacity-50"
                    disabled={deleting}
                    onclick={() => handleDeleteTeam(teamStore.selectedTeam)}
                    >{deleting ? "..." : t("team_deleteYes")}</button
                  >
                  <button
                    class="rounded px-2 py-1 text-xs font-medium border border-border text-foreground hover:bg-accent transition-colors"
                    onclick={() => (deleteConfirm = null)}>{t("team_deleteNo")}</button
                  >
                </div>
              {:else}
                <button
                  class="rounded p-1 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                  title={t("team_deleteTeam")}
                  onclick={() => (deleteConfirm = teamStore.selectedTeam)}
                >
                  <svg
                    class="h-3.5 w-3.5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <path d="M3 6h18" /><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" /><path
                      d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"
                    />
                  </svg>
                </button>
              {/if}
            </div>
          </div>

          <!-- Row 2: member chips horizontal scroll -->
          <div class="flex items-center gap-1.5 px-4 py-1.5 overflow-x-auto">
            {#each teamStore.teamConfig.members as member}
              {@const isLead = member.agentId === teamStore.teamConfig.leadAgentId}
              <button
                class="shrink-0 flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] transition-colors {expandedMemberName ===
                member.name
                  ? 'border-primary/40 bg-primary/5'
                  : 'border-border/40 bg-card hover:bg-accent/50'}"
                onclick={() =>
                  (expandedMemberName = expandedMemberName === member.name ? null : member.name)}
              >
                <!-- Color dot with active ping -->
                <span class="relative h-2 w-2 shrink-0">
                  <span class="absolute inset-0 rounded-full {memberColorClass(member.color)}"
                  ></span>
                  {#if member.isActive}
                    <span
                      class="absolute inset-0 rounded-full {memberColorClass(
                        member.color,
                      )} animate-ping opacity-50"
                    ></span>
                  {/if}
                </span>
                <span class="font-medium text-foreground">{member.name}</span>
                {#if member.agentType}
                  <span class="rounded bg-muted px-1 py-0.5 text-[9px] font-medium"
                    >{member.agentType}</span
                  >
                {/if}
                {#if member.model}
                  <span class="text-muted-foreground">{shortModel(member.model)}</span>
                {/if}
                {#if member.isActive}
                  <span
                    class="rounded bg-emerald-500/10 px-1 py-0.5 text-[9px] font-medium text-emerald-600 dark:text-emerald-400"
                    >{t("team_badgeActive")}</span
                  >
                {/if}
                {#if member.planModeRequired}
                  <span
                    class="rounded bg-violet-500/10 px-1 py-0.5 text-[9px] font-medium text-violet-600 dark:text-violet-400"
                    >{t("team_badgePlan")}</span
                  >
                {/if}
                {#if isLead}
                  <span
                    class="rounded bg-primary/10 px-1 py-0.5 text-[9px] font-bold text-primary uppercase"
                    >{t("team_badgeLead")}</span
                  >
                {/if}
              </button>
            {/each}
          </div>

          <!-- Row 3: expanded member detail panel (conditional) -->
          {#if expandedMemberName}
            {@const member = teamStore.teamConfig.members.find(
              (m) => m.name === expandedMemberName,
            )}
            {#if member}
              <div class="border-t border-border/40 px-4 py-2.5 bg-muted/20">
                <div class="flex items-start gap-4 text-[11px] text-muted-foreground flex-wrap">
                  {#if member.cwd}
                    <div class="flex items-center gap-1">
                      <span class="text-muted-foreground/50">{t("team_labelCwd")}</span>
                      <span class="font-mono text-foreground/70" title={member.cwd}
                        >{truncatePath(member.cwd, 50)}</span
                      >
                    </div>
                  {/if}
                  {#if member.model}
                    <div class="flex items-center gap-1">
                      <span class="text-muted-foreground/50">{t("team_labelModel")}</span>
                      <span class="text-foreground/70">{member.model}</span>
                    </div>
                  {/if}
                  {#if member.backendType && member.backendType !== "in-process"}
                    <div class="flex items-center gap-1">
                      <span class="text-muted-foreground/50">{t("team_labelBackend")}</span>
                      <span class="text-foreground/70">{member.backendType}</span>
                    </div>
                  {/if}
                  {#if member.joinedAt}
                    <div class="flex items-center gap-1">
                      <span class="text-muted-foreground/50">{t("team_labelJoined")}</span>
                      <span class="text-foreground/70">{timeAgoEpoch(member.joinedAt)}</span>
                    </div>
                  {/if}
                </div>
                {#if member.prompt}
                  <div class="mt-1.5">
                    <span class="text-[10px] text-muted-foreground/50">{t("team_labelPrompt")}</span
                    >
                    <p
                      class="text-[11px] text-foreground/70 whitespace-pre-wrap break-words mt-0.5"
                    >
                      {member.prompt}
                    </p>
                  </div>
                {/if}
              </div>
            {/if}
          {/if}
        </div>

        <!-- ═══ Conversation Area ═══ -->
        <div class="flex flex-1 flex-col min-h-0">
          {#if teamStore.teamConfig.members.length > 0}
            <!-- Agent tabs -->
            <div class="shrink-0 flex gap-0.5 border-b border-border px-4 overflow-x-auto">
              <button
                class="shrink-0 px-3 py-1.5 text-[11px] font-medium transition-colors border-b-2 {inboxTab ===
                'all'
                  ? 'text-foreground border-primary'
                  : 'text-muted-foreground hover:text-foreground border-transparent'}"
                onclick={handleAllInboxClick}>{t("team_inboxAll")}</button
              >
              {#each teamStore.teamConfig.members as member}
                <button
                  class="shrink-0 px-3 py-1.5 text-[11px] font-medium transition-colors border-b-2 {inboxTab ===
                  member.name
                    ? 'text-foreground border-primary'
                    : 'text-muted-foreground hover:text-foreground border-transparent'}"
                  onclick={() => handleInboxTabClick(member.name)}
                >
                  <span
                    class="inline-block h-1.5 w-1.5 rounded-full mr-1 {memberColorClass(
                      member.color,
                    )}"
                  ></span>{member.name}
                </button>
              {/each}
            </div>

            <!-- Message timeline (flex-1, full-height scroll) -->
            <div class="flex-1 overflow-y-auto px-4 py-2 space-y-1">
              {#if displayedMessages.length === 0}
                <div class="flex items-center justify-center h-full">
                  <p class="text-xs text-muted-foreground">{t("team_noMessages")}</p>
                </div>
              {:else}
                {#each displayedMessages as msg}
                  {@const parsed = parseMessageType(msg.text)}
                  {@const isExpMsg = expandedMsgKey === msgKey(msg)}
                  <button
                    class="w-full text-left flex gap-2 rounded-lg px-3 py-2 hover:bg-muted/30 transition-colors {!msg.read
                      ? 'border-l-2 border-l-primary/60'
                      : ''}"
                    onclick={() =>
                      (expandedMsgKey = expandedMsgKey === msgKey(msg) ? null : msgKey(msg))}
                  >
                    <!-- Color dot -->
                    <span class="h-2 w-2 rounded-full shrink-0 mt-1.5 {memberColorClass(msg.color)}"
                    ></span>
                    <div class="flex-1 min-w-0">
                      <!-- Sender + time -->
                      <div class="flex items-center gap-2">
                        <span class="text-[11px] font-medium {msgColorClass(msg.color)}"
                          >{msg.from}</span
                        >
                        <span class="text-[10px] text-muted-foreground/40 ml-auto shrink-0"
                          >{timeAgo(msg.timestamp)}</span
                        >
                        {#if !msg.read}
                          <span class="h-1.5 w-1.5 rounded-full bg-primary shrink-0"></span>
                        {/if}
                      </div>
                      <!-- Message body — rich rendering by type -->
                      <div class="mt-0.5">
                        {#if parsed}
                          {#if parsed.type === "message"}
                            <div class="text-[11px]">
                              {#if parsed.data.recipient}
                                {@const recipientMember = teamStore.teamConfig?.members.find(
                                  (m) => m.name === parsed.data.recipient,
                                )}
                                <span class="text-muted-foreground">{t("team_msgTo")} </span>
                                <span
                                  class="font-medium {recipientMember
                                    ? msgColorClass(recipientMember.color)
                                    : 'text-foreground/70'}">{parsed.data.recipient}</span
                                >
                                <span class="text-muted-foreground"> · </span>
                              {/if}
                              {#if isExpMsg}
                                <span class="text-foreground/80 whitespace-pre-wrap break-words"
                                  >{parsed.data.content ?? ""}</span
                                >
                              {:else}
                                <span class="text-foreground/80 line-clamp-3"
                                  >{parsed.data.summary ?? parsed.data.content ?? ""}</span
                                >
                              {/if}
                            </div>
                          {:else if parsed.type === "idle_notification"}
                            <div
                              class="flex items-center gap-1.5 text-[11px] text-muted-foreground flex-wrap"
                            >
                              <span class="text-blue-400"
                                >{parsed.data.idleReason ?? t("team_msgIdle")}</span
                              >
                              {#if parsed.data.completedTaskId}
                                <span
                                  >{t("team_msgCompleted", {
                                    id: String(parsed.data.completedTaskId),
                                  })}{parsed.data.completedTaskSubject
                                    ? `: ${parsed.data.completedTaskSubject}`
                                    : ""}</span
                                >
                              {/if}
                              {#if parsed.data.failureReason}
                                <span class="text-red-400">{parsed.data.failureReason}</span>
                              {/if}
                              {#if parsed.data.peerDmSummary}
                                <span class="text-muted-foreground/60"
                                  >| {parsed.data.peerDmSummary}</span
                                >
                              {/if}
                            </div>
                          {:else if parsed.type === "task_completed"}
                            <div class="text-[11px] text-emerald-600 dark:text-emerald-400">
                              {t("team_msgCompleted", { id: String(parsed.data.taskId) })}{parsed
                                .data.taskSubject
                                ? `: ${parsed.data.taskSubject}`
                                : ""}
                            </div>
                          {:else if parsed.type === "task_assignment"}
                            <div class="text-[11px] text-teal-600 dark:text-teal-400">
                              {t("team_msgAssigned", { id: String(parsed.data.taskId) })}{parsed
                                .data.subject
                                ? `: ${parsed.data.subject}`
                                : ""}{parsed.data.assignedBy ? ` by ${parsed.data.assignedBy}` : ""}
                              {#if isExpMsg && parsed.data.description}
                                <p
                                  class="mt-0.5 text-muted-foreground whitespace-pre-wrap break-words"
                                >
                                  {parsed.data.description}
                                </p>
                              {/if}
                            </div>
                          {:else if parsed.type === "shutdown_request"}
                            <div class="text-[11px] text-red-500">
                              {t("team_msgShutdownRequested")}{parsed.data.reason
                                ? `: ${parsed.data.reason}`
                                : ""}
                            </div>
                          {:else if parsed.type === "shutdown_approved"}
                            <div class="text-[11px] text-red-400/70">{t("team_msgShutDown")}</div>
                          {:else if parsed.type === "shutdown_rejected"}
                            <div class="text-[11px] text-amber-500">
                              {t("team_msgShutdownRejected")}{parsed.data.reason
                                ? `: ${parsed.data.reason}`
                                : ""}
                            </div>
                          {:else if parsed.type === "plan_approval_request"}
                            <div class="text-[11px] text-violet-600 dark:text-violet-400">
                              {t("team_msgPlanApprovalNeeded")}
                              {#if parsed.data.planFilePath}
                                <span class="text-muted-foreground/60 ml-1"
                                  >{parsed.data.planFilePath}</span
                                >
                              {/if}
                              {#if parsed.data.planContent}
                                {#if isExpMsg}
                                  <p
                                    class="mt-0.5 text-muted-foreground whitespace-pre-wrap break-words"
                                  >
                                    {parsed.data.planContent}
                                  </p>
                                {:else}
                                  <p
                                    class="mt-0.5 text-muted-foreground line-clamp-3 whitespace-pre-wrap"
                                  >
                                    {parsed.data.planContent}
                                  </p>
                                {/if}
                              {/if}
                            </div>
                          {:else if parsed.type === "plan_approval_response"}
                            <div
                              class="text-[11px] {parsed.data.approved
                                ? 'text-emerald-500'
                                : 'text-red-500'}"
                            >
                              {parsed.data.approved
                                ? t("team_msgPlanApproved")
                                : t("team_msgPlanRejected")}{parsed.data.feedback
                                ? `: ${parsed.data.feedback}`
                                : ""}
                              {#if parsed.data.permissionMode}
                                <span class="text-muted-foreground/60 ml-1"
                                  >({parsed.data.permissionMode})</span
                                >
                              {/if}
                            </div>
                          {:else if parsed.type === "permission_request"}
                            <div class="text-[11px] text-amber-600 dark:text-amber-400">
                              {t("team_msgPermission", {
                                tool: parsed.data.tool_name ?? "tool",
                              })}{parsed.data.description ? ` — ${parsed.data.description}` : ""}
                              {#if isExpMsg && parsed.data.input}
                                <pre
                                  class="mt-1 text-[10px] text-muted-foreground bg-muted/50 rounded p-1.5 overflow-x-auto whitespace-pre-wrap break-words">{typeof parsed
                                    .data.input === "string"
                                    ? parsed.data.input
                                    : JSON.stringify(parsed.data.input, null, 2)}</pre>
                              {/if}
                            </div>
                          {:else if parsed.type === "mode_set_request"}
                            <div class="text-[11px] text-muted-foreground">
                              {t("team_msgModeSet", {
                                mode: String(parsed.data.mode ?? "unknown"),
                              })}
                            </div>
                          {:else}
                            <!-- Unknown structured type -->
                            <div class="flex items-center gap-1.5 text-[11px]">
                              <span
                                class="rounded bg-muted px-1 py-0.5 text-[9px] text-muted-foreground"
                                >{parsed.type}</span
                              >
                              <span class="text-muted-foreground"
                                >{parsed.data.content ?? parsed.data.summary ?? ""}</span
                              >
                            </div>
                          {/if}
                        {:else}
                          <!-- Plain text message -->
                          {#if isExpMsg}
                            <p
                              class="text-[11px] text-foreground/80 whitespace-pre-wrap break-words"
                            >
                              {msg.text}
                            </p>
                          {:else if msg.summary}
                            <p class="text-[11px] text-foreground/80">{msg.summary}</p>
                          {:else}
                            <p
                              class="text-[11px] text-foreground/80 line-clamp-4 whitespace-pre-wrap"
                            >
                              {msg.text}
                            </p>
                          {/if}
                        {/if}
                      </div>
                    </div>
                  </button>
                {/each}
              {/if}
            </div>
          {:else}
            <div class="flex items-center justify-center flex-1">
              <p class="text-xs text-muted-foreground">{t("team_noMembers")}</p>
            </div>
          {/if}
        </div>
      </div>

      <!-- ═══ Task Board Sidebar ═══ -->
      <div
        class="shrink-0 border-l border-border flex flex-col h-full transition-all duration-200 {sidebarCollapsed
          ? 'w-8'
          : 'w-[280px]'}"
      >
        {#if sidebarCollapsed}
          <!-- Collapsed: narrow strip with toggle + task count -->
          <button
            class="flex flex-col items-center py-3 gap-2 h-full hover:bg-accent/30 transition-colors"
            title={t("team_expandTaskBoard")}
            onclick={() => (sidebarCollapsed = false)}
          >
            <svg
              class="h-3.5 w-3.5 text-muted-foreground"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="m15 18-6-6 6-6" />
            </svg>
            <span
              class="text-[10px] text-muted-foreground font-medium"
              style="writing-mode: vertical-rl"
              >{t("team_tasksBoardCount", { count: String(teamStore.tasks.length) })}</span
            >
          </button>
        {:else}
          <!-- Expanded: full task board -->
          <div
            class="shrink-0 flex items-center justify-between px-3 h-9 border-b border-border/40"
          >
            <span class="text-xs font-medium text-muted-foreground uppercase tracking-wider"
              >{t("team_tasksBoardCount", { count: String(teamStore.tasks.length) })}</span
            >
            <button
              class="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent/50 transition-colors"
              title={t("team_collapseTaskBoard")}
              onclick={() => (sidebarCollapsed = true)}
            >
              <svg
                class="h-3.5 w-3.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="m9 18 6-6-6-6" />
              </svg>
            </button>
          </div>

          <div class="flex-1 overflow-y-auto px-3 py-2 space-y-2">
            {#if teamStore.tasks.length === 0}
              <p class="text-xs text-muted-foreground py-6 text-center">{t("team_noTasks")}</p>
            {/if}

            <!-- In Progress -->
            {#if teamStore.inProgressTasks.length > 0}
              <div>
                <button
                  class="flex w-full items-center gap-1.5 text-[11px] font-medium text-blue-600 dark:text-blue-400 hover:text-blue-500 transition-colors py-1"
                  onclick={() => (expandInProgress = !expandInProgress)}
                >
                  <svg
                    class="h-3 w-3 transition-transform {expandInProgress ? 'rotate-90' : ''}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"><path d="m9 18 6-6-6-6" /></svg
                  >
                  {t("team_inProgress", { count: String(teamStore.inProgressTasks.length) })}
                </button>
                {#if expandInProgress}
                  <div class="space-y-1">
                    {#each teamStore.inProgressTasks as task}
                      {@const isExpanded = teamStore.expandedTaskId === task.id}
                      <button
                        class="w-full text-left rounded-lg border border-blue-500/20 bg-blue-500/5 px-2.5 py-2 hover:bg-blue-500/10 transition-colors"
                        onclick={() => toggleTaskExpand(task)}
                      >
                        <div class="flex items-start gap-1.5">
                          <span class="text-[11px] font-mono text-blue-500/60 shrink-0 mt-0.5"
                            >#{task.id}</span
                          >
                          <div class="flex-1 min-w-0">
                            <div class="text-xs font-medium text-foreground">{task.subject}</div>
                            <div class="flex items-center gap-2 mt-0.5 flex-wrap">
                              {#if task.owner}
                                <span class="text-[10px] text-muted-foreground">{task.owner}</span>
                              {/if}
                              {#if task.activeForm}
                                <span class="text-[10px] text-blue-400 italic"
                                  >{task.activeForm}</span
                                >
                              {/if}
                            </div>
                            {#if task.blockedBy.length > 0}
                              <div class="flex items-center gap-1 mt-1 flex-wrap">
                                <span class="text-[9px] text-amber-600 dark:text-amber-400"
                                  >{t("team_blockedBy")}</span
                                >
                                {#each task.blockedBy as dep}
                                  <span
                                    class="rounded bg-amber-500/10 px-1 py-0.5 text-[9px] font-mono text-amber-600 dark:text-amber-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                            {#if task.blocks.length > 0}
                              <div class="flex items-center gap-1 mt-0.5 flex-wrap">
                                <span class="text-[9px] text-emerald-600 dark:text-emerald-400"
                                  >{t("team_unblocks")}</span
                                >
                                {#each task.blocks as dep}
                                  <span
                                    class="rounded bg-emerald-500/10 px-1 py-0.5 text-[9px] font-mono text-emerald-600 dark:text-emerald-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                          <svg
                            class="h-3 w-3 shrink-0 text-muted-foreground/40 mt-1 transition-transform {isExpanded
                              ? 'rotate-180'
                              : ''}"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
                          >
                        </div>
                        {#if isExpanded}
                          <div class="mt-2 pt-2 border-t border-blue-500/10">
                            {#if taskDescLoading[task.id]}
                              <div
                                class="flex items-center gap-1.5 text-[10px] text-muted-foreground"
                              >
                                <div
                                  class="h-3 w-3 border border-primary/30 border-t-primary rounded-full animate-spin"
                                ></div>
                                {t("team_loading")}
                              </div>
                            {:else}
                              <p
                                class="text-[11px] text-muted-foreground whitespace-pre-wrap break-words"
                              >
                                {taskDescriptions[task.id] ||
                                  task.description ||
                                  t("team_noDescription")}
                              </p>
                            {/if}
                            {#if task.metadata}
                              <div class="mt-1.5 flex flex-wrap gap-1">
                                {#each Object.entries(task.metadata as Record<string, unknown>) as [k, v]}
                                  <span
                                    class="rounded bg-muted px-1.5 py-0.5 text-[9px] text-muted-foreground"
                                    >{k}: {typeof v === "string" ? v : JSON.stringify(v)}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}

            <!-- Pending -->
            {#if teamStore.pendingTasks.length > 0}
              <div>
                <button
                  class="flex w-full items-center gap-1.5 text-[11px] font-medium text-muted-foreground hover:text-foreground transition-colors py-1"
                  onclick={() => (expandPending = !expandPending)}
                >
                  <svg
                    class="h-3 w-3 transition-transform {expandPending ? 'rotate-90' : ''}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"><path d="m9 18 6-6-6-6" /></svg
                  >
                  {t("team_pending", { count: String(teamStore.pendingTasks.length) })}
                </button>
                {#if expandPending}
                  <div class="space-y-1">
                    {#each teamStore.pendingTasks as task}
                      {@const isExpanded = teamStore.expandedTaskId === task.id}
                      <button
                        class="w-full text-left rounded-lg border border-border/30 bg-muted/20 px-2.5 py-2 hover:bg-muted/40 transition-colors"
                        onclick={() => toggleTaskExpand(task)}
                      >
                        <div class="flex items-start gap-1.5">
                          <span
                            class="text-[11px] font-mono text-muted-foreground/60 shrink-0 mt-0.5"
                            >#{task.id}</span
                          >
                          <div class="flex-1 min-w-0">
                            <div class="text-xs text-foreground">{task.subject}</div>
                            <div class="flex items-center gap-2 mt-0.5 flex-wrap">
                              {#if task.owner}
                                <span class="text-[10px] text-muted-foreground">{task.owner}</span>
                              {/if}
                              {#if task.activeForm}
                                <span class="text-[10px] text-muted-foreground/60 italic"
                                  >{task.activeForm}</span
                                >
                              {/if}
                            </div>
                            {#if task.blockedBy.length > 0}
                              <div class="flex items-center gap-1 mt-1 flex-wrap">
                                <span class="text-[9px] text-amber-600 dark:text-amber-400"
                                  >{t("team_blockedBy")}</span
                                >
                                {#each task.blockedBy as dep}
                                  <span
                                    class="rounded bg-amber-500/10 px-1 py-0.5 text-[9px] font-mono text-amber-600 dark:text-amber-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                            {#if task.blocks.length > 0}
                              <div class="flex items-center gap-1 mt-0.5 flex-wrap">
                                <span class="text-[9px] text-emerald-600 dark:text-emerald-400"
                                  >{t("team_unblocks")}</span
                                >
                                {#each task.blocks as dep}
                                  <span
                                    class="rounded bg-emerald-500/10 px-1 py-0.5 text-[9px] font-mono text-emerald-600 dark:text-emerald-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                          <svg
                            class="h-3 w-3 shrink-0 text-muted-foreground/40 mt-1 transition-transform {isExpanded
                              ? 'rotate-180'
                              : ''}"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
                          >
                        </div>
                        {#if isExpanded}
                          <div class="mt-2 pt-2 border-t border-border/20">
                            {#if taskDescLoading[task.id]}
                              <div
                                class="flex items-center gap-1.5 text-[10px] text-muted-foreground"
                              >
                                <div
                                  class="h-3 w-3 border border-primary/30 border-t-primary rounded-full animate-spin"
                                ></div>
                                {t("team_loading")}
                              </div>
                            {:else}
                              <p
                                class="text-[11px] text-muted-foreground whitespace-pre-wrap break-words"
                              >
                                {taskDescriptions[task.id] ||
                                  task.description ||
                                  t("team_noDescription")}
                              </p>
                            {/if}
                            {#if task.metadata}
                              <div class="mt-1.5 flex flex-wrap gap-1">
                                {#each Object.entries(task.metadata as Record<string, unknown>) as [k, v]}
                                  <span
                                    class="rounded bg-muted px-1.5 py-0.5 text-[9px] text-muted-foreground"
                                    >{k}: {typeof v === "string" ? v : JSON.stringify(v)}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}

            <!-- Completed -->
            {#if teamStore.completedTasks.length > 0}
              <div>
                <button
                  class="flex w-full items-center gap-1.5 text-[11px] font-medium text-emerald-600 dark:text-emerald-400 hover:text-emerald-500 transition-colors py-1"
                  onclick={() => (expandCompleted = !expandCompleted)}
                >
                  <svg
                    class="h-3 w-3 transition-transform {expandCompleted ? 'rotate-90' : ''}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"><path d="m9 18 6-6-6-6" /></svg
                  >
                  {t("team_completed", { count: String(teamStore.completedTasks.length) })}
                </button>
                {#if expandCompleted}
                  <div class="space-y-1">
                    {#each teamStore.completedTasks as task}
                      {@const isExpanded = teamStore.expandedTaskId === task.id}
                      <button
                        class="w-full text-left rounded-lg border border-emerald-500/15 bg-emerald-500/5 px-2.5 py-2 hover:bg-emerald-500/10 transition-colors"
                        onclick={() => toggleTaskExpand(task)}
                      >
                        <div class="flex items-start gap-1.5">
                          <span class="text-[11px] font-mono text-emerald-500/50 shrink-0 mt-0.5"
                            >#{task.id}</span
                          >
                          <div class="flex-1 min-w-0">
                            <div
                              class="text-xs text-foreground/70 line-through decoration-foreground/20"
                            >
                              {task.subject}
                            </div>
                            <div class="flex items-center gap-2 mt-0.5 flex-wrap">
                              {#if task.owner}
                                <span class="text-[10px] text-muted-foreground/50"
                                  >{task.owner}</span
                                >
                              {/if}
                              {#if task.activeForm}
                                <span class="text-[10px] text-emerald-400/60 italic"
                                  >{task.activeForm}</span
                                >
                              {/if}
                            </div>
                            {#if task.blockedBy.length > 0}
                              <div class="flex items-center gap-1 mt-1 flex-wrap">
                                <span class="text-[9px] text-amber-600 dark:text-amber-400"
                                  >{t("team_blockedBy")}</span
                                >
                                {#each task.blockedBy as dep}
                                  <span
                                    class="rounded bg-amber-500/10 px-1 py-0.5 text-[9px] font-mono text-amber-600 dark:text-amber-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                            {#if task.blocks.length > 0}
                              <div class="flex items-center gap-1 mt-0.5 flex-wrap">
                                <span class="text-[9px] text-emerald-600 dark:text-emerald-400"
                                  >{t("team_unblocks")}</span
                                >
                                {#each task.blocks as dep}
                                  <span
                                    class="rounded bg-emerald-500/10 px-1 py-0.5 text-[9px] font-mono text-emerald-600 dark:text-emerald-400"
                                    >#{dep}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                          <svg
                            class="h-3 w-3 shrink-0 text-muted-foreground/30 mt-1 transition-transform {isExpanded
                              ? 'rotate-180'
                              : ''}"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"><path d="m6 9 6 6 6-6" /></svg
                          >
                        </div>
                        {#if isExpanded}
                          <div class="mt-2 pt-2 border-t border-emerald-500/10">
                            {#if taskDescLoading[task.id]}
                              <div
                                class="flex items-center gap-1.5 text-[10px] text-muted-foreground"
                              >
                                <div
                                  class="h-3 w-3 border border-primary/30 border-t-primary rounded-full animate-spin"
                                ></div>
                                {t("team_loading")}
                              </div>
                            {:else}
                              <p
                                class="text-[11px] text-muted-foreground whitespace-pre-wrap break-words"
                              >
                                {taskDescriptions[task.id] ||
                                  task.description ||
                                  t("team_noDescription")}
                              </p>
                            {/if}
                            {#if task.metadata}
                              <div class="mt-1.5 flex flex-wrap gap-1">
                                {#each Object.entries(task.metadata as Record<string, unknown>) as [k, v]}
                                  <span
                                    class="rounded bg-muted px-1.5 py-0.5 text-[9px] text-muted-foreground"
                                    >{k}: {typeof v === "string" ? v : JSON.stringify(v)}</span
                                  >
                                {/each}
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  {:else}
    <!-- No team selected (multiple teams but none clicked) -->
    <div class="flex flex-col items-center justify-center h-full text-center px-6">
      <p class="text-sm text-muted-foreground">{t("team_selectTeam")}</p>
    </div>
  {/if}
</div>

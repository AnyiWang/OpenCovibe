/**
 * SessionStore: single source of truth for chat session state.
 *
 * Replaces 25 scattered $state variables and 3 booleans (running x sending x sessionStarted)
 * with a state-machine (SessionPhase) + organized fields + idempotent reducers.
 */
import * as api from "$lib/api";
import type {
  TaskRun,
  HookEvent,
  BusEvent,
  BusToolItem,
  TimelineEntry,
  Attachment,
  CliCommand,
  McpServerInfo,
  ElicitationSchema,
} from "$lib/types";
import { dbg, dbgWarn } from "$lib/utils/debug";
import type { SessionMode } from "$lib/types";
import { IMAGE_TYPES } from "$lib/utils/file-types";
import { uuid } from "$lib/utils/uuid";
import {
  type SessionPhase,
  type UsageState,
  type TurnUsage,
  ACTIVE_PHASES,
  TERMINAL_PHASES,
  SESSION_ALIVE_PHASES,
  assertTransition,
} from "./types";
import { getEventMiddleware } from "./event-middleware";
import { updateInstalledVersion, getCliCommands } from "./cli-info.svelte";
import * as snapshotCache from "$lib/utils/snapshot-cache";
import { getTransport } from "$lib/transport";

// ── CLI permission mode normalization ──
// CLI may return different names for the same mode across versions.
// Normalize to the canonical names used throughout the app.
const CLI_PERM_MODE_ALIASES: Record<string, string> = {
  delegate: "acceptEdits", // CLI v2.1.81+ renamed acceptEdits → delegate
};

function normalizePermissionMode(mode: string): string {
  return CLI_PERM_MODE_ALIASES[mode] ?? mode;
}

// ── OpGuard: async operation guard with mounted check ──

class OpGuard {
  private _active = false;
  private _mounted = true;

  get busy(): boolean {
    return this._active;
  }
  get isMounted(): boolean {
    return this._mounted;
  }

  acquire(): boolean {
    if (this._active) return false;
    this._active = true;
    return true;
  }
  release(): void {
    this._active = false;
  }
  unmount(): void {
    this._mounted = false;
  }
}

// ── Helpers ──

function eventTs(ev: BusEvent): string {
  const r = ev as Record<string, unknown>;
  return (r.ts as string) ?? (r.timestamp as string) ?? new Date().toISOString();
}

/** Backfill anchorId for old snapshots/entries that predate the anchor system. Recursive for subTimelines. */
function backfillAnchorId(entry: TimelineEntry): TimelineEntry {
  const e = entry as Record<string, unknown>;
  if (e.anchorId) return entry; // already has anchorId
  const anchor = (e.cliUuid as string) || (e.id as string);
  const patched = { ...entry, anchorId: anchor } as TimelineEntry;
  if (patched.kind === "tool" && patched.subTimeline) {
    (patched as { subTimeline: TimelineEntry[] }).subTimeline =
      patched.subTimeline.map(backfillAnchorId);
  }
  return patched;
}

/** Parse event timestamp to epoch milliseconds (falls back to Date.now()). */
function eventTsMs(ev: BusEvent): number {
  const iso = eventTs(ev);
  const ms = new Date(iso).getTime();
  return Number.isFinite(ms) ? ms : Date.now();
}

// ── Internal batch state (plain objects, no reactivity) ──

interface ReduceCtx {
  tl: TimelineEntry[];
  he: HookEvent[];
  streamText: string;
  thinkingText: string;
  model: string;
  phase: SessionPhase;
  usage: UsageState;
  error: string;
  seenMessageIds: Set<string>;
  seenToolIds: Set<string>;
  /** Track run.status changes from non-terminal run_state events (running/idle). */
  runStatus: string | null;
  /** New session_id from session_init (e.g. fork generates a new CLI session). */
  sessionId: string | null;
  /** Whether this run uses stream-json mode (skip tools mirror writes). */
  isStream: boolean;
  /** Per-turn usage snapshots. */
  turnUsages: TurnUsage[];
  /** tool_use_id → tl[] index (only tool entries, first-match semantics). */
  toolTlIndex: Map<string, number>;
  /** tool_use_id → he[] index (only HookEvent entries with tool_use_id). */
  toolHeIndex: Map<string, number>;
}

// ── Helpers ──

/** Strip contentBase64 from non-image attachments to avoid storing MB of data in reactive state.
 *  Images keep base64 for inline <img> preview; PDF/other show as file chip (metadata only). */
function timelineAttachments(atts: Attachment[]): Attachment[] | undefined {
  if (atts.length === 0) return undefined;
  return atts.map((a) =>
    (IMAGE_TYPES as readonly string[]).includes(a.type) ? a : { ...a, contentBase64: "" },
  );
}

/** Map frontend Attachment[] to backend AttachmentData format for IPC. */
function mapAttachments(
  atts: Attachment[],
): Array<{ content_base64: string; media_type: string; filename: string }> | null {
  if (atts.length === 0) return null;
  return atts.map((a) => ({
    content_base64: a.contentBase64,
    media_type: a.type,
    filename: a.name,
  }));
}

// ── Exported types ──

export interface ElicitationState {
  requestId: string;
  mcpServerName: string;
  message: string;
  elicitationId?: string;
  mode?: string;
  url?: string;
  requestedSchema?: ElicitationSchema;
}

export interface TaskNotificationItem {
  task_id: string;
  status: string;
  message: string;
  startedAt: number;
  data: unknown;
  output_file?: string;
  task_type?: string;
  summary?: string;
  tool_use_id?: string;
}

// ── Store ──

export class SessionStore {
  // ── State fields ──
  phase: SessionPhase = $state("empty");
  run: TaskRun | null = $state(null);
  timeline: TimelineEntry[] = $state([]);
  streamingText: string = $state("");
  /** Accumulated thinking/reasoning text (extended thinking). Cleared on new turn. */
  thinkingText: string = $state("");
  /** Timestamp (ms) of the first thinking_delta event in the current turn. 0 = no thinking yet. */
  thinkingStartMs: number = $state(0);
  /** Timestamp (ms) when thinking ended (first message_delta after thinking). 0 = still thinking or no thinking. */
  thinkingEndMs: number = $state(0);
  tools: HookEvent[] = $state([]);
  usage: UsageState = $state({
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    cost: 0,
  });
  model: string = $state("");
  error: string = $state("");
  agent: string = $state("claude");
  authMode: string = $state("cli");
  ptySpawned: boolean = $state(false);

  // ── Protocol extension fields ──
  systemStatus = $state<{ status?: string } | null>(null);
  authStatus = $state<{ is_authenticating: boolean; output: string[] } | null>(null);
  hookEvents = $state<
    Array<{
      type: string;
      hook_id: string;
      data: unknown;
      request_id?: string;
      status?: "hook_pending" | "allowed" | "denied" | "cancelled";
      hook_name?: string;
      stdout?: string;
      stderr?: string;
      exit_code?: number;
    }>
  >([]);
  taskNotifications = $state<Map<string, TaskNotificationItem>>(new Map());
  /** Pending MCP elicitation prompts keyed by request_id. */
  pendingElicitations = $state<Map<string, ElicitationState>>(new Map());
  persistedFiles = $state<unknown[]>([]);

  /** Ralph loop state — null when no loop has been active. */
  ralphLoop = $state<{
    active: boolean;
    prompt: string;
    iteration: number;
    maxIterations: number;
    completionPromise: string | null;
    startedAt: string;
    reason: import("$lib/types").RalphCompleteReason | "interrupted" | null;
  } | null>(null);

  /** CLI slash commands from session_init (session-specific, includes custom commands). */
  sessionCommands = $state<CliCommand[]>([]);
  /** MCP servers from session_init (per-session state). */
  mcpServers = $state<McpServerInfo[]>([]);

  // ── CLI verbose fields (from session_init / usage_update) ──
  cliVersion = $state<string>("");
  permissionMode = $state<string>("");
  /** True when permissionMode was set by user/settings AND successfully persisted.
   *  Prevents session_init / snapshot from overwriting.
   *  NOT cleared by _clearContentState() unless permissionModePersistFailed is true. */
  permissionModeSetByUser = $state<boolean>(false);
  /** True when mode was switched via control protocol but settings persist failed.
   *  Cleared on _clearContentState() (new run/session), allowing session_init to
   *  re-sync from CLI's actual startup mode. */
  permissionModePersistFailed = $state<boolean>(false);
  fastModeState = $state<string>("");
  apiKeySource = $state<string>("");
  availableAgents = $state<string[]>([]);
  availableSkills = $state<string[]>([]);
  availablePlugins = $state<unknown[]>([]);
  /** CLI's current working directory (updated from session_init). */
  sessionCwd = $state<string>("");
  /** CLI's available tools (updated from session_init). */
  sessionTools = $state<string[]>([]);
  /** CLI's output style (updated from session_init). */
  outputStyle = $state<string>("");
  /** Saved permission mode before plan mode (restored on ExitPlanMode). */
  previousPermissionMode = $state<string>("");
  /** Override mode after ExitPlanMode completes (user chose specific mode via approval card). */
  pendingPermissionModeOverride = $state<string | null>(null);
  /** Plan content for "clear context" restart (set before allow, consumed after tool_end). */
  pendingClearContextPlan = $state<string | null>(null);
  /** True after first session_init received for this session (gates sessionCommands exposure). */
  sessionInitReceived = $state<boolean>(false);
  numTurns = $state<number>(0);
  durationMs = $state<number>(0);
  /** Count of unknown event types hitting _reduce default case. */
  unknownEventCount = $state<number>(0);
  /** Count of Raw events with non-stdout/stderr source (fallback path). */
  rawFallbackCount = $state<number>(0);
  /** When true, throw on unknown/fallback events instead of counting. Test-only. */
  strictMode = false;
  /** Per-turn usage snapshots (append-only, one per usage_update event). */
  turnUsages: TurnUsage[] = $state([]);
  /** Timestamp of the most recent compact_boundary event (0 = never). */
  lastCompactedAt: number = $state(0);
  /** Number of full compaction events in this session. */
  compactCount: number = $state(0);
  /** Number of micro-compaction events in this session. */
  microcompactCount: number = $state(0);

  /** Remote host name (if this session runs on a remote machine). */
  remoteHostName = $state<string | null>(null);
  /** Derived: true if running on a remote host via SSH. */
  get isRemote(): boolean {
    return !!this.remoteHostName;
  }

  /** Per-session platform_id — set before first message, locked after. */
  platformId = $state<string | null>(null);

  /** True while stop() is in progress — suppresses RunState error display from dying CLI. */
  private _stopping = false;

  // Internal dedup sets (not reactive — only used inside reducers)
  private _seenMessageIds = new Set<string>();
  private _seenToolIds = new Set<string>();

  /** Highest _seq processed — used for WS checkpoint on reconnect/subscribe */
  private _lastProcessedSeq = 0;

  // ── Reducer tool indexes (runtime-only, not serialized) ──
  /** tool_use_id → timeline[] index for tool entries (first-match, reducer fast-path). */
  private _toolTlIndex = new Map<string, number>();
  /** tool_use_id → tools[] (HookEvent) index (first-match, reducer fast-path). */
  private _toolHeIndex = new Map<string, number>();
  /** _lastProcessedSeq at last snapshot write — throttles idle snapshot rewrites. */
  private _lastSnapshotSeq = 0;

  // Generation counter: prevents stale async loadRun from overwriting state
  private _loadGen = 0;
  /** True while loadRun is replaying events — suppresses isThinking flash on session switch. */
  private _isLoadingReplay = false;

  // Spawn timeout: fail if CLI never emits session_init
  private _spawnTimer: ReturnType<typeof setTimeout> | null = null;
  private static readonly _SPAWN_TIMEOUT_MS = 30_000;

  // Response timeout: warn if no content after sending a message
  private _responseTimer: ReturnType<typeof setTimeout> | null = null;
  private static readonly _RESPONSE_TIMEOUT_MS = 60_000;
  /** True when current error was set by the response timeout (cleared when content arrives). */
  private _isTimeoutError = false;

  /** Set phase with dev-mode transition guard. */
  private _setPhase(to: SessionPhase): void {
    assertTransition(this.phase, to);
    this.phase = to;
    // Any phase change away from spawning clears the spawn timeout
    if (to !== "spawning") {
      this._clearSpawnTimeout();
    }
    // Clear response timeout on terminal/idle phases
    if (to !== "running") {
      this._clearResponseTimeout();
    }
  }

  /** Start a timeout that fails the session if phase stays at spawning.
   *  Also kills the backend process to prevent orphan CLI processes. */
  private _startSpawnTimeout(runId: string): void {
    this._clearSpawnTimeout();
    this._spawnTimer = setTimeout(async () => {
      if (this.phase === "spawning" && this.run?.id === runId) {
        dbgWarn(
          "store",
          "spawn timeout: CLI did not respond within",
          SessionStore._SPAWN_TIMEOUT_MS,
          "ms",
        );
        this.error =
          "Session failed to start (CLI did not respond). Try again or check CLI installation.";
        // Kill the hung backend process to prevent orphans
        try {
          await api.stopSession(runId);
        } catch (e) {
          dbgWarn("store", "spawn timeout: failed to stop session:", e);
        }
        this._setPhase("failed");
        if (this.run) {
          this.run = { ...this.run, status: "failed" };
        }
      }
    }, SessionStore._SPAWN_TIMEOUT_MS);
  }

  private _clearSpawnTimeout(): void {
    if (this._spawnTimer) {
      clearTimeout(this._spawnTimer);
      this._spawnTimer = null;
    }
  }

  /** Start a timeout that warns if no response content arrives after sending a message. */
  private _startResponseTimeout(runId: string): void {
    this._clearResponseTimeout();
    this._responseTimer = setTimeout(() => {
      if (
        this.run?.id === runId &&
        this.phase === "running" &&
        !this.streamingText &&
        !this.thinkingText
      ) {
        this._isTimeoutError = true;
        this.error = "No response after 60s — still waiting for API.";
        dbgWarn("store", "response timeout: no content after 60s");
      }
    }, SessionStore._RESPONSE_TIMEOUT_MS);
  }

  private _clearResponseTimeout(): void {
    if (this._responseTimer) {
      clearTimeout(this._responseTimer);
      this._responseTimer = null;
    }
  }

  /** Clear response timeout error (only if it was set by the timeout, not a real error). */
  private _clearTimeoutError(): void {
    this._clearResponseTimeout();
    if (this._isTimeoutError) {
      this.error = "";
      this._isTimeoutError = false;
    }
  }

  /** Check if text is a known CLI slash command (not path like /home/user). */
  isKnownSlashCommand(text: string): boolean {
    const m = text.match(/^\/([a-z][\w-]*)(?:\s|$)/i);
    if (!m) return false;
    const name = m[1].toLowerCase();
    // Check available skills (preloaded from filesystem, available before session_init)
    if (this.availableSkills.some((s) => s.toLowerCase() === name)) return true;
    // Check session commands (available after session_init) or static CLI info
    const cmds = this.sessionCommands.length > 0 ? this.sessionCommands : getCliCommands();
    if (cmds.length > 0) {
      return cmds.some(
        (c) =>
          c.name.toLowerCase() === name || (c.aliases ?? []).some((a) => a.toLowerCase() === name),
      );
    }
    // Cold start: no command list available yet — trust the regex pattern.
    // The (?:\s|$) boundary already filters out paths like /home/user.
    // False positive risk (e.g. "/hello") only skips 60s timeout, acceptable.
    dbg("store", "isKnownSlashCommand cold-start fallback", { name });
    return true;
  }

  // ── Derived getters ──

  get isRunning(): boolean {
    return ACTIVE_PHASES.includes(this.phase);
  }

  get isIdle(): boolean {
    return this.phase === "idle";
  }

  get sessionAlive(): boolean {
    return SESSION_ALIVE_PHASES.includes(this.phase);
  }

  get canSend(): boolean {
    return ["empty", "ready", "idle"].includes(this.phase);
  }

  get activeToolName(): string {
    // Stream mode: scan timeline (top-level only, equivalent to previous HookEvent behavior)
    if (this.useStreamSession) {
      for (let i = this.timeline.length - 1; i >= 0; i--) {
        const e = this.timeline[i];
        if (e.kind === "tool" && e.tool.status === "running") return e.tool.tool_name;
      }
      return "";
    }
    // Pipe/PTY fallback: use HookEvent tools array
    return this.tools.filter((e) => e.status === "running").at(-1)?.tool_name ?? "";
  }

  /** Recursive walk: short-circuit check for any permission_prompt in timeline + subTimelines. */
  private _hasPermission(predicate?: (t: BusToolItem) => boolean): boolean {
    function walk(entries: TimelineEntry[]): boolean {
      for (const entry of entries) {
        if (entry.kind !== "tool") continue;
        if (
          entry.tool.status === "permission_prompt" &&
          entry.tool.permission_request_id &&
          (!predicate || predicate(entry.tool))
        )
          return true;
        if (entry.subTimeline && walk(entry.subTimeline)) return true;
      }
      return false;
    }
    return walk(this.timeline);
  }

  /** Whether any permission prompt is pending user approval (recursive, includes subTimelines). */
  get hasPendingPermission(): boolean {
    return this._hasPermission();
  }

  /** Whether any MCP elicitation prompt is pending user response. */
  get hasElicitation(): boolean {
    return this.pendingElicitations.size > 0;
  }

  /** Whether an inline-only permission (AskUserQuestion / ExitPlanMode) is pending. */
  get hasInlinePermission(): boolean {
    return this._hasPermission(
      (t) => t.tool_name === "AskUserQuestion" || t.tool_name === "ExitPlanMode",
    );
  }

  /** Pending generic tool permission prompts (recursive, excludes AskUserQuestion/ExitPlanMode). */
  get pendingToolPermissions(): Array<{ tool: BusToolItem; requestId: string }> {
    const map = new Map<string, BusToolItem>();
    function walk(entries: TimelineEntry[]) {
      for (const entry of entries) {
        if (entry.kind !== "tool") continue;
        if (
          entry.tool.status === "permission_prompt" &&
          entry.tool.permission_request_id &&
          entry.tool.tool_name !== "AskUserQuestion" &&
          entry.tool.tool_name !== "ExitPlanMode"
        ) {
          const rid = entry.tool.permission_request_id;
          map.delete(rid);
          map.set(rid, entry.tool);
        }
        if (entry.subTimeline) walk(entry.subTimeline);
      }
    }
    walk(this.timeline);
    return Array.from(map, ([requestId, tool]) => ({ tool, requestId }));
  }

  get isThinking(): boolean {
    if (!this.isRunning || this.streamingText) return false;
    // During loadRun replay, phase is set to "running" before events are loaded.
    // Without this guard, isThinking flashes true on session switch (especially on
    // Windows where replay is slower). Suppress during the loading window.
    if (this._isLoadingReplay) return false;
    return !this.hasPendingPermission && !this.hasElicitation;
  }

  /** isRunning but not blocked on a permission prompt or elicitation.
   *  Used for UI elements (stop button, spinner) that should hide during approval. */
  get isActivelyRunning(): boolean {
    return this.isRunning && !this.hasPendingPermission && !this.hasElicitation;
  }

  /** Duration of extended thinking in seconds (0 if no thinking happened). */
  get thinkingDurationSec(): number {
    if (!this.thinkingStartMs) return 0;
    const end = this.thinkingEndMs || Date.now();
    return Math.floor((end - this.thinkingStartMs) / 1000);
  }

  get totalTokens(): number {
    return (
      this.usage.inputTokens +
      this.usage.outputTokens +
      this.usage.cacheReadTokens +
      this.usage.cacheWriteTokens
    );
  }

  get contextWindow(): number {
    if (!this.usage.modelUsage) return 0;
    const entries = Object.values(this.usage.modelUsage);
    let max = 0;
    for (const e of entries) {
      if (e.context_window && e.context_window > max) max = e.context_window;
    }
    return max;
  }

  get contextUtilization(): number {
    // Total tokens sent to the model in the latest API call:
    //   input_tokens (new non-cached) + cache_read (from cache) + cache_write (first-time cached)
    // All three are part of the context window. Divide by contextWindow for fill %.
    // These are per-turn values (usage_update replaces, not accumulates).
    const cw = this.contextWindow;
    if (cw <= 0) return 0;
    const used = this.usage.inputTokens + this.usage.cacheReadTokens + this.usage.cacheWriteTokens;
    if (used <= 0) return 0;
    return Math.min(used / cw, 1);
  }

  get contextWarningLevel(): "none" | "moderate" | "high" | "critical" {
    const u = this.contextUtilization;
    if (u >= 0.9) return "critical";
    if (u >= 0.75) return "high";
    if (u >= 0.5) return "moderate";
    return "none";
  }

  /** Background tasks that are still running/started (not completed/failed). */
  get activeBackgroundTasks(): TaskNotificationItem[] {
    const active: TaskNotificationItem[] = [];
    for (const item of this.taskNotifications.values()) {
      if (item.status !== "completed" && item.status !== "failed" && item.status !== "error") {
        active.push(item);
      }
    }
    return active;
  }

  /** Whether there are any background tasks (active or recently completed). */
  get hasBackgroundTasks(): boolean {
    return this.taskNotifications.size > 0;
  }

  get effectiveCwd(): string {
    return this.sessionCwd || this.run?.cwd || "";
  }

  get isApiMode(): boolean {
    return this.run ? this.run.auth_mode === "api" : this.authMode === "api";
  }

  get useStreamSession(): boolean {
    // Both OAuth (auth_mode=cli) and API Key (auth_mode=api) go through CLI stream-json
    return this.agent === "claude";
  }

  /** CLI-reported authentication source label. Empty before session_init. */
  get authSourceLabel(): string {
    if (!this.apiKeySource) return "";
    // When auth_mode="api", the App may inject ANTHROPIC_AUTH_TOKEN (Bearer auth)
    // and explicitly clear ANTHROPIC_API_KEY. CLI only tracks ANTHROPIC_API_KEY,
    // so it reports "none" even though auth works via ANTHROPIC_AUTH_TOKEN.
    if (this.apiKeySource === "none" && this.isApiMode) return "API Key";
    const map: Record<string, string> = {
      "/login managed key": "Login Key",
      ANTHROPIC_API_KEY: "API Key",
      apiKeyHelper: "Key Helper",
      none: "No Auth",
    };
    return map[this.apiKeySource] ?? this.apiKeySource;
  }

  /** Authentication source category for badge coloring. */
  get authSourceCategory(): string {
    if (!this.apiKeySource) return "unknown";
    if (this.apiKeySource === "/login managed key") return "login";
    if (this.apiKeySource === "ANTHROPIC_API_KEY") return "env_key";
    // Same ANTHROPIC_AUTH_TOKEN case — treat as env_key (blue badge)
    if (this.apiKeySource === "none" && this.isApiMode) return "env_key";
    if (this.apiKeySource === "none") return "none";
    return "other";
  }

  // ── Reducer index helpers ──

  /** Append a timeline entry and update tool index if applicable.
   *  Index uses first-match semantics (matching findIndex behavior) — only set if not already present. */
  private _pushTimeline(ctx: ReduceCtx | null, entry: TimelineEntry): void {
    if (ctx) {
      ctx.tl.push(entry);
      if (entry.kind === "tool" && !ctx.toolTlIndex.has(entry.id)) {
        ctx.toolTlIndex.set(entry.id, ctx.tl.length - 1);
      }
    } else {
      this.timeline = [...this.timeline, entry];
      if (entry.kind === "tool" && !this._toolTlIndex.has(entry.id)) {
        this._toolTlIndex.set(entry.id, this.timeline.length - 1);
      }
    }
  }

  /** Push an optimistic user message to the timeline (deduped by content in _reduce). */
  private _pushOptimisticUser(content: string, attachments?: Attachment[]): void {
    const id = uuid();
    this._pushTimeline(null, {
      kind: "user",
      id,
      anchorId: id,
      content,
      ts: new Date().toISOString(),
      ...(attachments && attachments.length > 0
        ? { attachments: timelineAttachments(attachments) }
        : {}),
    });
  }

  /** Append a hook event entry and update tool index if applicable.
   *  Index uses first-match semantics — only set if not already present. */
  private _pushHookEntry(ctx: ReduceCtx | null, heEntry: HookEvent): void {
    const toolUseId = (heEntry as Record<string, unknown>).tool_use_id as string | undefined;
    if (ctx) {
      ctx.he.push(heEntry);
      if (toolUseId && !ctx.toolHeIndex.has(toolUseId))
        ctx.toolHeIndex.set(toolUseId, ctx.he.length - 1);
    } else {
      this.tools = [...this.tools, heEntry];
      if (toolUseId && !this._toolHeIndex.has(toolUseId))
        this._toolHeIndex.set(toolUseId, this.tools.length - 1);
    }
  }

  /** Find tool timeline entry by tool_use_id. Map fast-path + findIndex fallback. */
  private _findToolIdx(ctx: ReduceCtx | null, toolUseId: string): number {
    const tl = ctx ? ctx.tl : this.timeline;
    const idx = ctx ? ctx.toolTlIndex.get(toolUseId) : this._toolTlIndex.get(toolUseId);
    // Fast path: Map hit + validation
    if (idx !== undefined && tl[idx]?.kind === "tool" && tl[idx].id === toolUseId) return idx;
    // Fallback: linear scan (covers stale/missing index entries)
    const fallback = tl.findIndex((e) => e.kind === "tool" && e.id === toolUseId);
    if (fallback >= 0) {
      dbgWarn("store", "_findToolIdx: index miss, found via scan", {
        toolUseId,
        mapIdx: idx,
        scanIdx: fallback,
      });
    }
    return fallback;
  }

  /** Simple id-only lookup for hook events. Map fast-path + findIndex fallback. */
  private _findHeIdx(ctx: ReduceCtx | null, toolUseId: string): number {
    const he = ctx ? ctx.he : this.tools;
    const idx = ctx ? ctx.toolHeIndex.get(toolUseId) : this._toolHeIndex.get(toolUseId);
    if (
      idx !== undefined &&
      he[idx] &&
      (he[idx] as Record<string, unknown>).tool_use_id === toolUseId
    )
      return idx;
    const fallback = he.findIndex((e) => (e as Record<string, unknown>).tool_use_id === toolUseId);
    if (fallback >= 0) {
      dbgWarn("store", "_findHeIdx: index miss, found via scan", {
        toolUseId,
        mapIdx: idx,
        scanIdx: fallback,
      });
    }
    return fallback;
  }

  /** Status-aware hook event lookup: Map fast-path + status validation + scan fallback.
   *  Used by user_message and tool_end which filter by status==="running". */
  private _findHeIdxByStatus(ctx: ReduceCtx | null, toolUseId: string, status: string): number {
    const he = ctx ? ctx.he : this.tools;
    const idx = ctx ? ctx.toolHeIndex.get(toolUseId) : this._toolHeIndex.get(toolUseId);
    // Fast path: Map hit + status match
    if (
      idx !== undefined &&
      he[idx] &&
      (he[idx] as Record<string, unknown>).tool_use_id === toolUseId &&
      he[idx].status === status
    ) {
      return idx;
    }
    // Fallback: linear scan (covers status mismatch or stale index)
    return he.findIndex(
      (e) => (e as Record<string, unknown>).tool_use_id === toolUseId && e.status === status,
    );
  }

  // ── SubTimeline helpers (subagent routing) ──

  /** Find the parent tool entry in timeline by tool_use_id; return index or -1.
   *  Uses _findToolIdx for Map fast-path with findIndex fallback. */
  private _findParentToolIdx(ctx: ReduceCtx | null, parentToolUseId: string): number {
    return this._findToolIdx(ctx, parentToolUseId);
  }

  /** Search ALL subTimelines (one level deep) for a tool with the given id.
   *  Used when parent_tool_use_id is missing but the tool exists in a subTimeline.
   *  Returns true if found and updated; false if not found. */
  private _updateToolInAnySubTimeline(
    toolUseId: string,
    updater: (old: BusToolItem) => BusToolItem,
    ctx: ReduceCtx | null,
  ): boolean {
    const tl = ctx ? ctx.tl : this.timeline;
    for (let pIdx = 0; pIdx < tl.length; pIdx++) {
      const entry = tl[pIdx];
      if (entry.kind !== "tool" || !entry.subTimeline) continue;
      const sub = entry.subTimeline;
      const cIdx = sub.findIndex((e) => e.kind === "tool" && e.id === toolUseId);
      if (cIdx < 0) continue;
      // Found in this parent's subTimeline — update it
      const oldChild = sub[cIdx] as Extract<TimelineEntry, { kind: "tool" }>;
      const newSub = [...sub];
      newSub[cIdx] = { ...oldChild, tool: updater(oldChild.tool) };
      const updatedParent: TimelineEntry = { ...entry, subTimeline: newSub };
      if (ctx) {
        ctx.tl[pIdx] = updatedParent;
      } else {
        const u = [...this.timeline];
        u[pIdx] = updatedParent;
        this.timeline = u;
      }
      dbg("store", "found tool in subTimeline (missing parent_tool_use_id)", {
        tool: toolUseId,
        parent: entry.id,
      });
      return true;
    }
    return false;
  }

  /** Append an entry to a parent tool's subTimeline. */
  private _appendToSubTimeline(
    tl: TimelineEntry[],
    parentIdx: number,
    entry: TimelineEntry,
    ctx: ReduceCtx | null,
  ): void {
    const old = tl[parentIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const updated: TimelineEntry = { ...old, subTimeline: [...(old.subTimeline ?? []), entry] };
    if (ctx) {
      ctx.tl[parentIdx] = updated;
    } else {
      const u = [...this.timeline];
      u[parentIdx] = updated;
      this.timeline = u;
    }
  }

  /** Update a tool entry inside a parent tool's subTimeline (3-level immutable update). */
  private _updateSubTimelineTool(
    parentToolUseId: string,
    childToolUseId: string,
    updater: (old: BusToolItem) => BusToolItem,
    ctx: ReduceCtx | null,
  ): boolean {
    const tl = ctx ? ctx.tl : this.timeline;
    const pIdx = this._findParentToolIdx(ctx, parentToolUseId);
    if (pIdx < 0) return false;
    const parent = tl[pIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const sub = parent.subTimeline ?? [];
    const cIdx = sub.findIndex((e) => e.kind === "tool" && e.id === childToolUseId);
    if (cIdx < 0) return false;
    const oldChild = sub[cIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const newSub = [...sub];
    newSub[cIdx] = { ...oldChild, tool: updater(oldChild.tool) };
    const updatedParent: TimelineEntry = { ...parent, subTimeline: newSub };
    if (ctx) {
      ctx.tl[pIdx] = updatedParent;
    } else {
      const u = [...this.timeline];
      u[pIdx] = updatedParent;
      this.timeline = u;
    }
    return true;
  }

  /** Append/update a synthetic assistant entry in a parent tool's subTimeline for streaming deltas.
   *  Single-active-stream per parent: synthetic ID = `__sub_stream_{parentToolUseId}`.
   *  If the entry doesn't exist yet, creates it; otherwise appends to content or thinkingText. */
  private _appendSubTimelineStreamingDelta(
    parentToolUseId: string,
    field: "content" | "thinkingText",
    text: string,
    ctx: ReduceCtx | null,
  ): void {
    const tl = ctx ? ctx.tl : this.timeline;
    const pIdx = this._findParentToolIdx(ctx, parentToolUseId);
    if (pIdx < 0) return;
    const parent = tl[pIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const sub = parent.subTimeline ?? [];
    const syntheticId = `__sub_stream_${parentToolUseId}`;
    const sIdx = sub.findIndex((e) => e.kind === "assistant" && e.id === syntheticId);
    let newSub: TimelineEntry[];
    if (sIdx >= 0) {
      // Update existing synthetic entry
      const old = sub[sIdx] as Extract<TimelineEntry, { kind: "assistant" }>;
      newSub = [...sub];
      if (field === "content") {
        newSub[sIdx] = { ...old, content: old.content + text };
      } else {
        newSub[sIdx] = { ...old, thinkingText: (old.thinkingText ?? "") + text };
      }
    } else {
      // Create new synthetic entry
      const entry: TimelineEntry =
        field === "content"
          ? {
              kind: "assistant",
              id: syntheticId,
              anchorId: syntheticId,
              content: text,
              ts: new Date().toISOString(),
            }
          : {
              kind: "assistant",
              id: syntheticId,
              anchorId: syntheticId,
              content: "",
              thinkingText: text,
              ts: new Date().toISOString(),
            };
      newSub = [...sub, entry];
    }
    const updatedParent: TimelineEntry = { ...parent, subTimeline: newSub };
    if (ctx) {
      ctx.tl[pIdx] = updatedParent;
    } else {
      const u = [...this.timeline];
      u[pIdx] = updatedParent;
      this.timeline = u;
    }
  }

  /** Extract thinkingText from a parent tool's synthetic streaming entry (before removal). */
  private _extractSubTimelineThinking(
    parentToolUseId: string,
    ctx: ReduceCtx | null,
  ): string | undefined {
    const tl = ctx ? ctx.tl : this.timeline;
    const pIdx = this._findParentToolIdx(ctx, parentToolUseId);
    if (pIdx < 0) return undefined;
    const parent = tl[pIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const sub = parent.subTimeline ?? [];
    const syntheticId = `__sub_stream_${parentToolUseId}`;
    const entry = sub.find((e) => e.kind === "assistant" && e.id === syntheticId);
    if (!entry || entry.kind !== "assistant") return undefined;
    return entry.thinkingText;
  }

  /** Remove the synthetic streaming entry from a parent tool's subTimeline (called on message_complete). */
  private _removeSubTimelineStreamingEntry(parentToolUseId: string, ctx: ReduceCtx | null): void {
    const tl = ctx ? ctx.tl : this.timeline;
    const pIdx = this._findParentToolIdx(ctx, parentToolUseId);
    if (pIdx < 0) return;
    const parent = tl[pIdx] as Extract<TimelineEntry, { kind: "tool" }>;
    const sub = parent.subTimeline ?? [];
    const syntheticId = `__sub_stream_${parentToolUseId}`;
    const sIdx = sub.findIndex((e) => e.kind === "assistant" && e.id === syntheticId);
    if (sIdx < 0) return;
    const newSub = [...sub];
    newSub.splice(sIdx, 1);
    const updatedParent: TimelineEntry = { ...parent, subTimeline: newSub };
    if (ctx) {
      ctx.tl[pIdx] = updatedParent;
    } else {
      const u = [...this.timeline];
      u[pIdx] = updatedParent;
      this.timeline = u;
    }
  }

  /** Accumulate partial JSON and try to parse. Returns merged tool fields. */
  private static _accumulateJsonInput(
    tool: Record<string, unknown>,
    partialJson: string,
  ): { input?: Record<string, unknown>; _inputJsonAccum: string } {
    const prevAccum = ((tool as Record<string, unknown>)._inputJsonAccum as string) ?? "";
    const newAccum = prevAccum + partialJson;
    let parsed: Record<string, unknown> | null = null;
    try {
      parsed = JSON.parse(newAccum);
    } catch {
      /* incomplete JSON */
    }
    return { ...(parsed ? { input: parsed } : {}), _inputJsonAccum: newAccum };
  }

  /** Route tool_input_delta to a child tool inside a parent's subTimeline. */
  private _updateSubTimelineToolInput(
    parentToolUseId: string,
    childToolUseId: string,
    partialJson: string,
    ctx: ReduceCtx | null,
  ): void {
    this._updateSubTimelineTool(
      parentToolUseId,
      childToolUseId,
      (t) => {
        const accum = SessionStore._accumulateJsonInput(t as Record<string, unknown>, partialJson);
        return { ...t, ...accum } as typeof t;
      },
      ctx,
    );
  }

  // ── Reducers ──

  /** Apply a single live bus event (mutates $state directly). */
  applyEvent(ev: BusEvent): void {
    // Guard: drop events for a run we're no longer viewing
    if (!this.run || ev.run_id !== this.run.id) {
      dbg("store", "drop stale event", ev.type, "run_id=", ev.run_id, "current=", this.run?.id);
      return;
    }
    // Track WS sequence checkpoint — skip already-processed events (dedup)
    const evSeq = ((ev as Record<string, unknown>)._seq as number) ?? 0;
    if (evSeq > 0) {
      if (evSeq <= this._lastProcessedSeq) {
        dbg(
          "store",
          "drop duplicate event",
          ev.type,
          "seq=",
          evSeq,
          "last=",
          this._lastProcessedSeq,
        );
        return;
      }
      this._lastProcessedSeq = evSeq;
    }
    this._reduce(ev, null);
  }

  /** Apply a batch of events (e.g. during loadRun replay). Avoids N reactive updates.
   *  opts.replayOnly=true skips phase and error assignments (used during resume).
   *  Returns elapsed milliseconds. */
  applyEventBatch(events: BusEvent[], opts?: { replayOnly?: boolean }): number {
    const t0 = performance.now();
    const replayOnly = opts?.replayOnly ?? false;
    // Build tool indexes from existing state for batch processing
    const batchTlIndex = new Map<string, number>();
    for (let i = 0; i < this.timeline.length; i++) {
      const e = this.timeline[i];
      if (e.kind === "tool" && !batchTlIndex.has(e.id)) batchTlIndex.set(e.id, i);
    }
    const batchHeIndex = new Map<string, number>();
    for (let i = 0; i < this.tools.length; i++) {
      const tid = (this.tools[i] as Record<string, unknown>).tool_use_id as string | undefined;
      if (tid && !batchHeIndex.has(tid)) batchHeIndex.set(tid, i);
    }
    const ctx: ReduceCtx = {
      tl: [...this.timeline],
      he: [...this.tools],
      streamText: this.streamingText,
      thinkingText: this.thinkingText,
      model: this.model,
      phase: this.phase,
      usage: { ...this.usage },
      error: this.error,
      seenMessageIds: new Set(this._seenMessageIds),
      seenToolIds: new Set(this._seenToolIds),
      runStatus: null,
      sessionId: null,
      isStream: this.useStreamSession,
      turnUsages: [...this.turnUsages],
      toolTlIndex: batchTlIndex,
      toolHeIndex: batchHeIndex,
    };
    for (const ev of events) {
      // Track WS sequence checkpoint
      const evSeq = ((ev as Record<string, unknown>)._seq as number) ?? 0;
      if (evSeq > 0) this._lastProcessedSeq = Math.max(this._lastProcessedSeq, evSeq);
      this._reduce(ev, ctx, replayOnly);
    }
    // If the session ended, resolve any leftover incomplete tools
    // (running, ask_pending, permission_prompt — these will never receive results)
    const runStatus = this.run?.status;
    const sessionDead =
      runStatus === "stopped" ||
      runStatus === "completed" ||
      runStatus === "failed" ||
      runStatus === "error";
    if (sessionDead) {
      const staleStatuses = new Set(["running", "ask_pending", "permission_prompt"]);
      const finalizeTools = (tl: TimelineEntry[]): TimelineEntry[] => {
        let changed = false;
        const result = tl.map((e) => {
          if (e.kind !== "tool") return e;
          // Recurse into subTimeline first
          const newSub = e.subTimeline ? finalizeTools(e.subTimeline) : e.subTimeline;
          const needsFinalize = staleStatuses.has(e.tool.status);
          if (!needsFinalize && newSub === e.subTimeline) return e;
          changed = true;
          return {
            ...e,
            ...(newSub !== e.subTimeline ? { subTimeline: newSub } : {}),
            tool: needsFinalize
              ? { ...e.tool, status: "error" as const, output: { error: "Session ended" } }
              : e.tool,
          };
        });
        return changed ? result : tl;
      };
      ctx.tl = finalizeTools(ctx.tl);
    }

    // Single reactive assignment
    const t1 = performance.now();
    dbg(
      "store",
      `applyEventBatch: ${events.length} events processed in ${(t1 - t0).toFixed(1)}ms, timeline=${ctx.tl.length} entries`,
    );
    this.timeline = ctx.tl;
    this.tools = ctx.he;
    this.streamingText = ctx.streamText;
    this.thinkingText = ctx.thinkingText;
    this.model = ctx.model;
    this.usage = ctx.usage;
    this.turnUsages = ctx.turnUsages;
    this._seenMessageIds = ctx.seenMessageIds;
    this._seenToolIds = ctx.seenToolIds;
    this._toolTlIndex = ctx.toolTlIndex;
    this._toolHeIndex = ctx.toolHeIndex;
    // Phase and error only assigned in live mode (not during resume replay)
    if (!replayOnly) {
      this._setPhase(ctx.phase);
      this.error = ctx.error;
      // Sync run.status and session_id for non-terminal states from batch
      if ((ctx.runStatus || ctx.sessionId) && this.run) {
        const updates: Partial<TaskRun> = {};
        if (ctx.runStatus) updates.status = ctx.runStatus;
        if (ctx.sessionId) {
          dbg("store", "batch: updating session_id", {
            old: this.run.session_id,
            new: ctx.sessionId,
          });
          updates.session_id = ctx.sessionId;
        }
        this.run = { ...this.run, ...updates };
      }
    }

    // Ralph: mark interrupted loops after replay
    // If ralphLoop.active is true but session is not live, the loop was interrupted
    if (this.ralphLoop?.active && replayOnly) {
      this.ralphLoop = { ...this.ralphLoop, active: false, reason: "interrupted" };
      dbg("store", "ralph loop marked interrupted after replay");
    }

    return performance.now() - t0;
  }

  /** Apply a hook event (from hook-event Tauri listener). */
  applyHookEvent(event: HookEvent): void {
    if (!this.run || event.run_id !== this.run.id) return;

    // In stream session mode, bus events already handle tool tracking
    if (
      (this.useStreamSession || this.sessionAlive) &&
      (event.hook_type === "PreToolUse" || event.hook_type === "PostToolUse")
    ) {
      dbg("store", "skip hook tool event (stream mode):", event.hook_type, event.tool_name);
      return;
    }

    // PostToolUse should update matching PreToolUse entry
    if (event.hook_type === "PostToolUse" && event.tool_name) {
      const idx = this.tools.findLastIndex(
        (e) =>
          e.tool_name === event.tool_name && e.hook_type === "PreToolUse" && e.status === "running",
      );
      if (idx >= 0) {
        const updated = [...this.tools];
        updated[idx] = {
          ...updated[idx],
          status: "done",
          hook_type: "PostToolUse",
          tool_output: event.tool_output,
        };
        this.tools = updated;
        return;
      }
    }

    this.tools = [...this.tools, event];
  }

  /** Apply hook usage (cumulative += not overwrite). */
  applyHookUsage(usage: {
    run_id: string;
    input_tokens: number;
    output_tokens: number;
    cost: number;
  }): void {
    if (!this.run || usage.run_id !== this.run.id) return;
    this.usage = {
      ...this.usage,
      inputTokens: this.usage.inputTokens + usage.input_tokens,
      outputTokens: this.usage.outputTokens + usage.output_tokens,
      cost: this.usage.cost + usage.cost,
    };
  }

  // ── Actions ──

  /** Clear all content/display state fields. Does not touch phase, run, or agent. */
  private _clearContentState(): void {
    this.timeline = [];
    this.streamingText = "";
    this.thinkingText = "";
    this.thinkingStartMs = 0;
    this.thinkingEndMs = 0;
    this.tools = [];
    this.usage = {
      inputTokens: 0,
      outputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      cost: 0,
    };
    this.model = "";
    this.error = "";
    this.ptySpawned = false;
    this.systemStatus = null;
    this.authStatus = null;
    this.hookEvents = [];
    this.taskNotifications = new Map();
    this.pendingElicitations = new Map();
    this.persistedFiles = [];
    this.ralphLoop = null;
    this.sessionCommands = [];
    this.mcpServers = [];
    this.cliVersion = "";
    // NOTE: permissionMode intentionally NOT cleared — user-level preference, same as platformId.
    // However, if persist had failed, reset the flag so next session_init can re-sync.
    if (this.permissionModePersistFailed) {
      this.permissionModeSetByUser = false;
      this.permissionModePersistFailed = false;
      dbg("store", "permissionMode flag reset — persist had failed, allowing session_init re-sync");
    }
    this.fastModeState = "";
    this.apiKeySource = "";
    this.availableAgents = [];
    this.availableSkills = [];
    this.availablePlugins = [];
    this.numTurns = 0;
    this.durationMs = 0;
    this.turnUsages = [];
    this.lastCompactedAt = 0;
    this.compactCount = 0;
    this.microcompactCount = 0;
    this.sessionCwd = "";
    this.sessionTools = [];
    this.outputStyle = "";
    // If agent entered plan mode (previousPermissionMode is non-empty), restore the user's
    // actual preference. If user manually selected plan (previousPermissionMode is empty),
    // leave it alone — it's a user-level preference.
    if (this.permissionMode === "plan" && this.previousPermissionMode) {
      const restored = this.previousPermissionMode;
      this.permissionMode = restored;
      dbg("store", "permissionMode restored from agent plan on clear", { restored });
    }
    this.previousPermissionMode = "";
    this.pendingPermissionModeOverride = null;
    this.pendingClearContextPlan = null;
    this.sessionInitReceived = false;
    this.unknownEventCount = 0;
    this.rawFallbackCount = 0;
    // NOTE: remoteHostName and platformId are intentionally NOT cleared here —
    // they are run-level properties restored from run metadata, not per-session state.
    this._seenMessageIds.clear();
    this._seenToolIds.clear();
    this._lastProcessedSeq = 0;
    this._toolTlIndex.clear();
    this._toolHeIndex.clear();
    this._lastSnapshotSeq = 0;
  }

  /** Optimistically remove an elicitation after responding.
   *  Called by UI before the IPC call returns. */
  removeElicitation(requestId: string): void {
    if (!this.pendingElicitations.has(requestId)) return;
    const updated = new Map(this.pendingElicitations);
    updated.delete(requestId);
    this.pendingElicitations = updated;
  }

  /** Reset all state to empty. */
  reset(): void {
    this._setPhase("empty");
    this.run = null;
    this._isLoadingReplay = false;
    this._clearContentState();
  }

  // ── Snapshot cache helpers ──

  /** Serialize current store state into a JSON string for IDB caching. */
  private _buildSnapshot(): string {
    const obj: Record<string, unknown> = {
      // A group (ReduceCtx-derived)
      timeline: this.timeline,
      tools: this.tools,
      hookEvents: this.hookEvents,
      streamingText: this.streamingText,
      thinkingText: this.thinkingText,
      model: this.model,
      usage: this.usage,
      turnUsages: this.turnUsages,
      _seenMessageIds: [...this._seenMessageIds],
      _seenToolIds: [...this._seenToolIds],
      // B group (direct fields)
      systemStatus: this.systemStatus,
      authStatus: this.authStatus,
      cliVersion: this.cliVersion,
      // NOTE: permissionMode intentionally excluded — user-level preference, not snapshot state.
      fastModeState: this.fastModeState,
      apiKeySource: this.apiKeySource,
      sessionCommands: this.sessionCommands,
      mcpServers: this.mcpServers,
      sessionTools: this.sessionTools,
      availableAgents: this.availableAgents,
      availableSkills: this.availableSkills,
      availablePlugins: this.availablePlugins,
      sessionCwd: this.sessionCwd,
      outputStyle: this.outputStyle,
      sessionInitReceived: this.sessionInitReceived,
      numTurns: this.numTurns,
      durationMs: this.durationMs,
      compactCount: this.compactCount,
      microcompactCount: this.microcompactCount,
      persistedFiles: this.persistedFiles,
      unknownEventCount: this.unknownEventCount,
      rawFallbackCount: this.rawFallbackCount,
      taskNotifications: [...this.taskNotifications.entries()],
      _lastProcessedSeq: this._lastProcessedSeq,
    };
    return JSON.stringify(obj);
  }

  /** Parse snapshot body string. Returns parsed object or null if invalid JSON. */
  private _parseSnapshotBody(body: string): Record<string, unknown> | null {
    try {
      return JSON.parse(body) as Record<string, unknown>;
    } catch {
      return null;
    }
  }

  /** Try to restore store state from a pre-parsed snapshot object (or string for compat).
   *  Returns true on success, false if shape validation fails. */
  private _tryApplySnapshot(bodyOrObj: string | Record<string, unknown>): boolean {
    try {
      const obj =
        typeof bodyOrObj === "string"
          ? (JSON.parse(bodyOrObj) as Record<string, unknown>)
          : bodyOrObj;
      // Shape validation: timeline must be array, usage must be object
      if (!Array.isArray(obj.timeline) || typeof obj.usage !== "object" || obj.usage === null) {
        dbgWarn("snapshot", "apply:shape-fail", {
          hasTimeline: Array.isArray(obj.timeline),
          hasUsage: typeof obj.usage,
        });
        return false;
      }

      // A group
      // Backfill anchorId for old snapshots that predate the anchor system
      this.timeline = (obj.timeline as TimelineEntry[]).map(backfillAnchorId);
      this.tools = (obj.tools ?? []) as HookEvent[];
      this.hookEvents = (obj.hookEvents ?? []) as typeof this.hookEvents;
      this.streamingText = (obj.streamingText as string) ?? "";
      this.thinkingText = (obj.thinkingText as string) ?? "";
      this.model = (obj.model as string) ?? "";
      this.usage = obj.usage as UsageState;
      this.turnUsages = (obj.turnUsages ?? []) as TurnUsage[];
      this._seenMessageIds = new Set((obj._seenMessageIds ?? []) as string[]);
      this._seenToolIds = new Set((obj._seenToolIds ?? []) as string[]);

      // B group
      this.systemStatus = (obj.systemStatus as typeof this.systemStatus) ?? null;
      this.authStatus = (obj.authStatus as typeof this.authStatus) ?? null;
      this.cliVersion = (obj.cliVersion as string) ?? "";
      // NOTE: permissionMode intentionally NOT restored from snapshot — user-level preference.
      this.fastModeState = (obj.fastModeState as string) ?? "";
      this.apiKeySource = (obj.apiKeySource as string) ?? "";
      this.sessionCommands = (obj.sessionCommands ?? []) as CliCommand[];
      this.mcpServers = (obj.mcpServers ?? []) as McpServerInfo[];
      this.sessionTools = (obj.sessionTools ?? []) as string[];
      this.availableAgents = (obj.availableAgents ?? []) as string[];
      this.availableSkills = (obj.availableSkills ?? []) as string[];
      this.availablePlugins = (obj.availablePlugins ?? []) as unknown[];
      this.sessionCwd = (obj.sessionCwd as string) ?? "";
      this.outputStyle = (obj.outputStyle as string) ?? "";
      this.sessionInitReceived = (obj.sessionInitReceived as boolean) ?? false;
      this.numTurns = (obj.numTurns as number) ?? 0;
      this.durationMs = (obj.durationMs as number) ?? 0;
      this.compactCount = (obj.compactCount as number) ?? 0;
      this.microcompactCount = (obj.microcompactCount as number) ?? 0;
      this.persistedFiles = (obj.persistedFiles ?? []) as unknown[];
      this.unknownEventCount = (obj.unknownEventCount as number) ?? 0;
      this.rawFallbackCount = (obj.rawFallbackCount as number) ?? 0;
      this.taskNotifications = new Map(
        (obj.taskNotifications ?? []) as Array<[string, TaskNotificationItem]>,
      );
      this._lastProcessedSeq = (obj._lastProcessedSeq as number) ?? 0;

      // Rebuild runtime tool indexes from restored state
      this._toolTlIndex.clear();
      for (let i = 0; i < this.timeline.length; i++) {
        const e = this.timeline[i];
        if (e.kind === "tool" && !this._toolTlIndex.has(e.id)) this._toolTlIndex.set(e.id, i);
      }
      this._toolHeIndex.clear();
      for (let i = 0; i < this.tools.length; i++) {
        const tid = (this.tools[i] as Record<string, unknown>).tool_use_id as string | undefined;
        if (tid && !this._toolHeIndex.has(tid)) this._toolHeIndex.set(tid, i);
      }

      dbg("snapshot", "apply:ok", { timeline: this.timeline.length });
      return true;
    } catch (err) {
      dbgWarn("snapshot", "apply:error", err);
      return false;
    }
  }

  /** Fire-and-forget: serialize current state and write to IDB.
   *  Deferred to next event-loop tick so JSON.stringify doesn't block loadRun.
   *  Caller must check write guard before calling. */
  private _saveSnapshotToIdb(runId: string): void {
    if (!this.run) return;
    const runStatus = this.run.status;
    const gen = this._loadGen;
    setTimeout(() => {
      // Guard: still viewing the same run (user may have navigated away)
      if (this._loadGen !== gen || this.run?.id !== runId) return;
      // Guard: status must still match (prevents stale write after idle→running transition)
      if (this.run.status !== runStatus) {
        dbg("snapshot", "save:skipped (status changed)", {
          runId,
          expected: runStatus,
          actual: this.run.status,
        });
        return;
      }
      const body = this._buildSnapshot();
      dbg("snapshot", "save", { runId, runStatus, bytes: body.length });
      snapshotCache.writeSnapshot(runId, runStatus, body).catch(() => {});
    }, 0);
  }

  /** Load a run by ID. Handles replay of bus events / run events. */
  async loadRun(
    id: string,
    xtermRef?: { clear(): void; writeText(s: string): void },
  ): Promise<void> {
    const gen = ++this._loadGen;
    const loadStart = performance.now();
    dbg("store", "loadRun id=", id, "gen=", gen);

    if (!id) {
      this.reset();
      return;
    }

    // Reset state for new load
    this._setPhase("loading");
    this._clearContentState();

    if (xtermRef) {
      xtermRef.clear();
      xtermRef.writeText("\x1b[0m\x1b[2J\x1b[H");
    }

    try {
      this.run = await api.getRun(id);
      if (gen !== this._loadGen) {
        dbg("store", "stale after getRun, gen=", gen);
        return;
      }

      // Auto-sync CLI imports to pick up events written after the initial import
      if (this.run.source === "cli_import") {
        try {
          const syncResult = await api.syncCliSession(id);
          if (syncResult.newEvents > 0) {
            dbg("store", "loadRun: auto-synced CLI import", {
              newEvents: syncResult.newEvents,
            });
            // Refresh run meta after sync (watermark/status may have updated)
            this.run = await api.getRun(id);
            // Sync appended events → IDB snapshot is stale
            snapshotCache.deleteSnapshot(id).catch(() => {});
          }
        } catch (e) {
          dbg("store", "loadRun: auto-sync failed (non-fatal)", String(e));
        }
        if (gen !== this._loadGen) {
          dbg("store", "stale after auto-sync, gen=", gen);
          return;
        }
      }

      this.agent = this.run.agent;
      this.remoteHostName = this.run.remote_host_name ?? null;
      this.platformId = this.run.platform_id ?? null;
      // Suppress isThinking during event replay (prevents "thinking" flash on session switch)
      this._isLoadingReplay = true;

      // Determine phase from run status
      const st = this.run.status;
      if (st === "running") {
        this._setPhase("running");
      } else if (st === "completed" || st === "failed" || st === "stopped") {
        this._setPhase(st as SessionPhase);
      } else {
        this._setPhase("ready");
      }

      // Terminal runs use replayOnly — historical run_state events must not
      // overwrite the phase we just set from run.status. Same pattern as resumeSession.
      const isTerminal = TERMINAL_PHASES.includes(this.phase);

      if (this.useStreamSession) {
        let reducerMs = 0;
        let snapshotHit = false;

        // Try IDB snapshot (terminal + idle sessions)
        const snapshotEligible = isTerminal || this.run!.status === "idle";
        let snapshotBody: string | null = null;
        if (snapshotEligible) {
          try {
            snapshotBody = await snapshotCache.readSnapshot(id, this.run!.status);
          } catch {
            /* IDB unavailable → miss */
          }
          if (gen !== this._loadGen) return;
        }

        if (snapshotBody) {
          const isIdleSnap = !isTerminal;
          // Parse once, used for both seq check and apply
          const parsed = this._parseSnapshotBody(snapshotBody);
          if (!parsed) {
            snapshotBody = null; // corrupted JSON
          } else {
            const snapSeq = isIdleSnap ? ((parsed._lastProcessedSeq as number) ?? 0) : 1;

            if (snapSeq === 0 && isIdleSnap) {
              // seq=0: skip snapshot, delete stale entry to prevent repeated hit-then-skip
              dbg("store", "idle snapshot seq=0, skipping for full replay");
              snapshotCache.deleteSnapshot(id).catch(() => {});
              snapshotBody = null; // fall through to miss path
            } else if (this._tryApplySnapshot(parsed)) {
              snapshotHit = true;
              // Align _lastSnapshotSeq to prevent unnecessary rewrite on first idle
              this._lastSnapshotSeq = this._lastProcessedSeq;

              // Fix: idle snapshot hit → phase must be "idle", not "ready"
              if (isIdleSnap) this._setPhase("idle");

              // Desktop idle: incremental catchup (no WS available)
              if (isIdleSnap && getTransport().isDesktop()) {
                const catchupEvents = await api.getBusEvents(id, this._lastProcessedSeq);
                if (gen !== this._loadGen) return;
                if (catchupEvents.length > 0) {
                  dbg("store", "idle snapshot catchup", { count: catchupEvents.length });
                  this.applyEventBatch(catchupEvents, { replayOnly: false });
                  const catchupSt = this.run?.status;
                  if (
                    catchupSt === "idle" ||
                    catchupSt === "completed" ||
                    catchupSt === "failed" ||
                    catchupSt === "stopped"
                  ) {
                    this._saveSnapshotToIdb(id);
                  }
                }
              } else if (isIdleSnap) {
                this._wsSubscribeWithSeq(id, this._lastProcessedSeq);
              }
              // Terminal: no catchup needed, just subscribe for WS if applicable
              if (!isIdleSnap) {
                this._wsSubscribeWithSeq(id, this._lastProcessedSeq);
              }
            } else {
              snapshotBody = null; // shape validation failed
            }
          }
        }

        if (!snapshotHit) {
          // Miss or snapshot corrupted → normal path
          const busEvents = await api.getBusEvents(id);
          if (gen !== this._loadGen) {
            dbg("store", "stale after getBusEvents, gen=", gen);
            return;
          }
          reducerMs = this.applyEventBatch(busEvents, { replayOnly: isTerminal });
          this._wsSubscribeAfterLoad(id, busEvents);
          // Write guard: distinguish "legit empty session" from "reducer anomaly"
          if (snapshotEligible && (this.timeline.length > 0 || busEvents.length === 0)) {
            this._saveSnapshotToIdb(id);
          }
        }

        this._isLoadingReplay = false;
        dbg("store", "loadRun", {
          total: Math.round(performance.now() - loadStart),
          snapshotHit,
          reducer: Math.round(reducerMs),
          entries: this.timeline.length,
        });
      } else {
        this._isLoadingReplay = false;
        // CLI mode: replay history in terminal
        const events = await api.getRunEvents(id);
        if (gen !== this._loadGen) {
          dbg("store", "stale after getRunEvents, gen=", gen);
          return;
        }
        let hasHistory = false;
        for (const event of events) {
          const text = String(
            (event.payload as Record<string, unknown>).text ??
              (event.payload as Record<string, unknown>).message ??
              "",
          );
          if (!text || !xtermRef) continue;
          if (event.type === "user") {
            xtermRef.writeText(`\x1b[1;36m> ${text}\x1b[0m\r\n`);
            hasHistory = true;
          } else if (event.type === "system") {
            xtermRef.writeText(`\x1b[90m${text}\x1b[0m\r\n`);
          }
        }
        if (hasHistory && !this.isRunning) {
          xtermRef.writeText(`\r\n\x1b[90m--- Session ended ---\x1b[0m\r\n`);
        }
      }

      // After replay, reconcile phase with run.status:
      // bus events may leave phase as "idle"/"running" even though the run
      // is actually terminal (e.g. process crashed without emitting run_state).
      const finalStatus = this.run?.status;
      if (finalStatus === "completed" || finalStatus === "failed" || finalStatus === "stopped") {
        if (!TERMINAL_PHASES.includes(this.phase as SessionPhase)) {
          dbg("store", "reconcile phase", this.phase, "→", finalStatus);
          this._setPhase(finalStatus as SessionPhase);
        }
        // Clear replayed errors for terminal runs — they're historical, not active
        this.error = "";
      }

      // Restore per-run model from meta.json (overrides session_init if user hot-switched)
      if (this.run?.model) {
        dbg("store", "restore run model from meta:", this.run.model);
        this.model = this.run.model;
      }
    } catch (e) {
      if (gen !== this._loadGen) return;
      this._isLoadingReplay = false;
      this.error = String(e);
      this._setPhase("failed");
    }
  }

  /** Create a new run and start the session. Returns the run ID. */
  async startSession(prompt: string, cwd: string, attachments: Attachment[]): Promise<string> {
    this.error = "";
    this._setPhase("spawning");

    try {
      // Refresh platformId and permissionMode from latest settings for new sessions.
      // Both may be stale (carried over from a previous run / earlier user selection)
      // if the user changed settings without navigating away from chat.
      try {
        const freshSettings = await api.getUserSettings();
        if (freshSettings.auth_mode === "api") {
          const freshPid = freshSettings.active_platform_id ?? "anthropic";
          if (freshPid !== this.platformId) {
            dbg("store", "startSession: refreshing platformId", {
              old: this.platformId,
              new: freshPid,
            });
            this.platformId = freshPid;
          }
        }
        // Settings stores app-internal names (auto_all, auto_read, etc.)
        // Store uses CLI names (bypassPermissions, acceptEdits, etc.)
        const APP_TO_CLI: Record<string, string> = {
          ask: "default",
          auto_read: "acceptEdits",
          auto_all: "bypassPermissions",
          plan: "plan",
          auto: "auto",
          dont_ask: "dontAsk",
        };
        if (freshSettings.permission_mode) {
          const freshPerm =
            APP_TO_CLI[freshSettings.permission_mode] ?? freshSettings.permission_mode;
          if (freshPerm !== this.permissionMode) {
            dbg("store", "startSession: refreshing permissionMode", {
              old: this.permissionMode,
              new: freshPerm,
            });
            this.permissionMode = freshPerm;
            this.permissionModeSetByUser = true;
          }
        }
      } catch {
        // Non-fatal: fall through with current store values
      }

      const run = await api.startRun(
        prompt,
        cwd,
        this.agent,
        this.model || undefined,
        this.remoteHostName || undefined,
        this.platformId || undefined,
      );
      this.run = run;

      if (this.useStreamSession) {
        // Optimistic user message — the backend emits UserMessage during
        // api.startSession(), but the middleware subscription isn't set up
        // until after goto() triggers the URL $effect.  Content-based dedup
        // in _reduce(user_message) prevents double display.
        this._pushOptimisticUser(prompt, attachments);
        // Subscribe middleware BEFORE spawning so no bus-events are dropped.
        // The $effect in chat page will call subscribeCurrent again (idempotent).
        const mw = getEventMiddleware();
        mw.subscribeCurrent(run.id, this);
        this._wsSubscribeNewSession(run.id);
        dbg("store", "stream session start, run=", run.id);
        const backendAtt = mapAttachments(attachments) ?? undefined;
        await api.startSession(
          run.id,
          undefined,
          undefined,
          undefined,
          backendAtt,
          this.platformId || undefined,
        );
        dbg("store", "startSession resolved");
        // phase will be set by run_state bus event
        this._startSpawnTimeout(run.id);
        if (this.isKnownSlashCommand(prompt)) {
          dbg("store", "skip response timeout for slash command", { cmd: prompt.split(" ")[0] });
        } else {
          this._startResponseTimeout(run.id);
        }
      } else if (this.agent === "claude") {
        // CLI PTY mode — caller handles PTY spawn
        // Return run ID; page will queue pendingMessage and spawn PTY
      } else {
        // Codex pipe mode
        this._setPhase("running");
        await api.sendChatMessage(run.id, prompt, attachments.length > 0 ? attachments : undefined);
      }

      return run.id;
    } catch (e) {
      this.error = String(e);
      this._setPhase("failed");
      throw e;
    }
  }

  /** Send a subsequent message in an active session. */
  async sendMessage(text: string, attachments: Attachment[]): Promise<void> {
    if (!this.run) return;
    this.error = "";
    // Invalidate idle snapshot — user is sending a new message
    snapshotCache.deleteSnapshot(this.run.id).catch(() => {});

    try {
      if (this.useStreamSession && this.sessionAlive) {
        // Optimistic user message — matches the pattern in startSession().
        // Content-based dedup in _reduce(user_message) prevents double display
        // when the backend's UserMessage bus event arrives.
        this._pushOptimisticUser(text, attachments);
        await api.sendSessionMessage(this.run.id, text, mapAttachments(attachments) ?? undefined);
        if (this.isKnownSlashCommand(text)) {
          dbg("store", "skip response timeout for slash command", { cmd: text.split(" ")[0] });
        } else {
          this._startResponseTimeout(this.run.id);
        }
      } else if (this.agent === "claude" && this.ptySpawned) {
        await api.sendChatMessage(
          this.run.id,
          text,
          attachments.length > 0 ? attachments : undefined,
        );
      } else {
        this._setPhase("running");
        await api.sendChatMessage(
          this.run.id,
          text,
          attachments.length > 0 ? attachments : undefined,
        );
      }
    } catch (e) {
      this.error = String(e);
      throw e;
    }
  }

  /** Interrupt current turn. Falls back to kill if interrupt fails. */
  async interrupt(): Promise<void> {
    if (!this.run || !this.isRunning) return;
    if (!this.sessionAlive) {
      // Phase shows running but session is not alive — force cleanup
      this._setPhase("stopped");
      this.run = { ...this.run, status: "stopped" };
      return;
    }
    try {
      dbg("store", "interrupt current turn");
      await api.sendSessionControl(this.run.id, "interrupt");
    } catch (e) {
      // interrupt failed (timeout or actor dead) — kill process directly
      dbg("store", "interrupt failed, killing process:", e);
      try {
        await api.stopSession(this.run.id);
      } catch {
        // Session may already be dead
      }
      this._setPhase("stopped");
      this.ptySpawned = false;
      this.run = { ...this.run, status: "stopped" };
    }
  }

  /** Stop the current session. */
  async stop(): Promise<void> {
    if (!this.run) return;
    this._stopping = true;
    this._clearResponseTimeout();
    try {
      if (this.sessionAlive) {
        // Try graceful interrupt first if agent is currently running.
        // Skip during "spawning" — CLI hasn't initialized yet, interrupt would
        // wait for a control_response that may never come.
        if (this.phase === "running") {
          try {
            dbg("store", "sending interrupt before stop");
            await api.sendSessionControl(this.run.id, "interrupt");
            // Brief wait for CLI to process the interrupt
            await new Promise((r) => setTimeout(r, 500));
          } catch (e) {
            dbg("store", "interrupt failed (proceeding to kill):", e);
          }
        }
        try {
          await api.stopSession(this.run.id);
        } catch (e) {
          // Session may already be dead (process exited, actor cleaned up).
          // Force frontend state to stopped regardless.
          dbgWarn("store", "stopSession failed (forcing stopped):", e);
        }
      } else {
        await api.stopRun(this.run.id);
      }
    } catch (e) {
      dbgWarn("store", "stop failed:", e);
    } finally {
      // Always clean up frontend state, even if backend calls failed.
      // If the process is already dead, the UI must not stay stuck in "running".
      this._setPhase("stopped");
      this.ptySpawned = false;
      this.run = { ...this.run!, status: "stopped" };
      this._stopping = false;
    }
  }

  // ── Resume ──

  private _resumeGuard = new OpGuard();

  /** Whether a resume/continue/fork operation is currently in progress. */
  get resumeInFlight(): boolean {
    return this._resumeGuard.busy;
  }

  /** Resume/continue/fork a finished session. Returns the target run ID.
   *  Avoids flash by NOT calling reset() — clears content fields individually
   *  and uses replayOnly=true so replay doesn't overwrite phase.
   *  When initialMessage is provided, the message is written to CLI stdin atomically
   *  with the spawn — no separate send_session_message needed. */
  async resumeSession(
    runId: string,
    mode: SessionMode,
    initialMessage?: string,
    attachments?: Attachment[],
  ): Promise<string | null> {
    if (!this._resumeGuard.acquire()) return null;

    try {
      let run = await api.getRun(runId);
      if (!this._resumeGuard.isMounted) return runId;

      let metaActive = ACTIVE_PHASES.includes(run.status as SessionPhase);
      if (metaActive && mode !== "fork") {
        // meta.json says "running" — likely a stale status from an orphaned/crashed session.
        // Try to stop it first (kills process if alive, updates meta if not), then proceed.
        dbg("store", "resumeSession: meta says active, attempting stop first", {
          runId,
          status: run.status,
        });
        try {
          await api.stopRun(runId);
          // Re-fetch meta after stop to get updated status
          const refreshed = await api.getRun(runId);
          run = refreshed;
          metaActive = ACTIVE_PHASES.includes(run.status as SessionPhase);
        } catch (e) {
          dbgWarn("store", "resumeSession: stop attempt failed:", e);
        }
        if (metaActive) {
          // Still running after stop attempt — genuinely active, can't resume
          throw new Error("Session is still running");
        }
      }
      // Fork validates session_id internally; resume/continue need it here.
      if (mode !== "continue" && mode !== "fork" && !run.session_id) {
        throw new Error("No session_id available for resume");
      }

      // Invalidate any concurrent loadRun
      this._loadGen++;
      const resumeT0 = performance.now();

      // ★ Phase 1: async data fetch BEFORE clearing state (avoids flash)
      const isStream = run.agent === "claude"; // use run.agent, not this.useStreamSession
      let snapshotBody: string | null = null;
      let busEvents: BusEvent[] = [];

      if (isStream) {
        try {
          snapshotBody = await snapshotCache.readSnapshot(runId, run.status);
        } catch {
          /* IDB unavailable */
        }
        if (!this._resumeGuard.isMounted) return runId;
        if (!snapshotBody) {
          busEvents = await api.getBusEvents(runId);
          if (!this._resumeGuard.isMounted) return runId;
          dbg("store", "resumeSession: fetched", busEvents.length, "bus events for replay");
        }
      }

      // ★ Phase 2: clear + set run metadata (sync frame, no await)
      this.run = run;
      this.agent = run.agent;
      this.platformId = run.platform_id ?? null;
      this._clearContentState();

      // ★ Phase 3: apply snapshot or events + force invalidate
      let reducerMs = 0;
      let snapshotHit = false;
      if (isStream) {
        if (snapshotBody && this._tryApplySnapshot(snapshotBody)) {
          snapshotHit = true;
          this._wsSubscribeWithSeq(runId, this._lastProcessedSeq);
        } else {
          // Fallback: snapshot corrupted → re-fetch events if needed
          if (!busEvents.length && snapshotBody) {
            busEvents = await api.getBusEvents(runId);
            if (!this._resumeGuard.isMounted) return runId;
          }
          if (busEvents.length > 0) {
            reducerMs = this.applyEventBatch(busEvents, { replayOnly: true });
          }
          // Always subscribe — even empty history needs real-time events
          this._wsSubscribeAfterLoad(runId, busEvents);
        }

        // Resume makes session go live → old snapshot is always stale
        snapshotCache.deleteSnapshot(runId).catch(() => {});
      }

      dbg("store", "resumeSession", {
        total: Math.round(performance.now() - resumeT0),
        snapshotHit,
        reducer: Math.round(reducerMs),
        entries: this.timeline.length,
      });

      // Restore per-run model from meta.json (overrides session_init if user hot-switched)
      if (run.model) {
        dbg("store", "resume: restore run model from meta:", run.model);
        this.model = run.model;
      }

      // Optimistic user message: add AFTER replay so it appears at the end of timeline.
      // Must be before startSession IPC so the user sees their message immediately.
      // Backend's UserMessage bus event will be deduped by content match in _reduce.
      if (initialMessage) {
        this._pushOptimisticUser(initialMessage, attachments ?? undefined);
      }

      // Explicitly set phase — replay didn't touch it
      this._setPhase("spawning");

      let targetRunId = runId;

      if (mode === "fork") {
        targetRunId = await this._handleFork(runId);
      } else {
        const sessionId = run.session_id;
        const backendAtt = attachments ? (mapAttachments(attachments) ?? undefined) : undefined;
        dbg("store", "resumeSession", {
          runId,
          targetRunId,
          mode,
          sessionId,
          hasMessage: !!initialMessage,
          attachments: backendAtt?.length ?? 0,
        });
        await api.startSession(
          targetRunId,
          mode,
          sessionId ?? undefined,
          initialMessage,
          backendAtt,
          run.platform_id ?? undefined,
        );
      }
      // Bus events via applyEvent (live) will transition phase:
      // - With message: spawning → running → idle (from CLI result)
      // - Without message: spawning → idle (synthetic, waiting for user input)

      // Timeout guard: if CLI never emits session_init, the UI would spin forever.
      // Fork skips this — connectSession() handles its own spawn timeout.
      if (mode !== "fork") {
        this._startSpawnTimeout(targetRunId);
        if (initialMessage && !this.isKnownSlashCommand(initialMessage)) {
          this._startResponseTimeout(targetRunId);
        } else if (initialMessage) {
          dbg("store", "skip response timeout for slash command (resume)", {
            cmd: initialMessage.split(" ")[0],
          });
        }
        // No initialMessage → no response timeout (just waiting for user input)
      }

      return targetRunId;
    } catch (e) {
      if (!this._resumeGuard.isMounted) return null;
      this.error = String(e);
      this._setPhase("failed");
      dbgWarn("store", "resumeSession failed:", e);
      return null;
    } finally {
      this._resumeGuard.release();
    }
  }

  /** Step 1 of two-step fork: create forked run, replay parent events.
   *  Returns the new run ID. Called from resumeSession when mode === "fork".
   *  Step 2 (connectSession) is called by the frontend after dismissing the fork overlay. */
  private async _handleFork(runId: string): Promise<string> {
    dbg("store", "resumeSession: two-step fork", { runId });

    // Clear any subscription to prevent source RunState(stopped) interference
    getEventMiddleware().subscribeCurrent("", this);

    // Step 1: One-shot fork (backend does fork_oneshot, returns new run_id with new session_id)
    const newRunId = await api.forkSession(runId);
    if (!this._resumeGuard.isMounted) throw new Error("Unmounted during fork");

    const newRun = await api.getRun(newRunId);
    if (!this._resumeGuard.isMounted) throw new Error("Unmounted during fork");

    this.run = newRun;

    // Subscribe to NEW run — live events from stream-json will route here
    getEventMiddleware().subscribeCurrent(newRunId, this);
    dbg("store", "fork: middleware subscribed to new run", newRunId);

    // Reset display state — start fresh for the fork run.
    // Without this, the source session's timeline stays in state and
    // message_delta events accumulate as duplicate streamingText.
    this._clearContentState();

    // Replay copied parent events for immediate display
    const allForkEvents = await api.getBusEvents(newRunId);
    if (!this._resumeGuard.isMounted) throw new Error("Unmounted during fork");
    const newEvents = allForkEvents.filter((ev) => ev.run_id === newRunId);
    if (newEvents.length > 0) {
      dbg("store", "fork: replaying", newEvents.length, "parent events");
      this.applyEventBatch(newEvents, { replayOnly: true });
    }
    this._wsSubscribeAfterLoad(newRunId, allForkEvents);

    // Step 2 (stream-json resume) is NOT started here.
    // handleResume will dismiss the overlay first, then call connectSession()
    // so the user sees normal "Starting session..." spinner instead of the fork overlay.
    dbg("store", "fork: step 1 complete, returning newRunId for step 2", {
      newRunId,
      sessionId: newRun.session_id,
    });
    return newRunId;
  }

  /**
   * Step 2 of two-step fork: establish stream-json connection to an already-forked session.
   * Called from handleResume AFTER the fork overlay is dismissed, so the user sees
   * the normal "Starting session..." spinner instead of the fork overlay.
   */
  async connectSession(runId: string, sessionId?: string): Promise<void> {
    const sid = sessionId ?? this.run?.session_id;
    if (!sid) throw new Error("No session_id available for connectSession");
    dbg("store", "connectSession: establishing stream-json connection", { runId, sessionId: sid });
    this._wsSubscribeNewSession(runId);
    this._setPhase("spawning");
    await api.startSession(
      runId,
      "resume",
      sid,
      undefined,
      undefined,
      this.platformId || undefined,
    );
    this._startSpawnTimeout(runId);
  }

  // ── WS subscribe helpers (browser-only, no-op on desktop) ──

  /** Browser: notify WS server to start pushing real-time events after history load */
  private _wsSubscribeAfterLoad(runId: string, events: BusEvent[]): void {
    if (getTransport().isDesktop()) return;
    const maxSeq =
      events.length > 0
        ? (((events[events.length - 1] as Record<string, unknown>)._seq as number) ?? 0)
        : 0;
    getTransport().subscribeRun(runId, maxSeq);
  }

  private _wsSubscribeNewSession(runId: string): void {
    if (getTransport().isDesktop()) return;
    getTransport().subscribeRun(runId, 0);
  }

  private _wsSubscribeWithSeq(runId: string, lastSeq: number): void {
    if (getTransport().isDesktop()) return;
    getTransport().subscribeRun(runId, lastSeq);
  }

  /** Call from page cleanup to prevent stale async writes after unmount. */
  unmountGuards(): void {
    this._resumeGuard.unmount();
    this._clearSpawnTimeout();
  }

  /** Update MCP servers (e.g. after getMcpStatus refresh). */
  updateMcpServers(servers: McpServerInfo[]): void {
    this.mcpServers = servers;
  }

  /** Resolve an AskUserQuestion tool: transition from ask_pending → success. */
  resolveAskQuestion(toolUseId: string, answer: string): void {
    dbg("store", "resolveAskQuestion", { toolUseId, answer });
    const tIdx = this._findToolIdx(null, toolUseId);
    if (tIdx >= 0) {
      const old = this.timeline[tIdx] as Extract<TimelineEntry, { kind: "tool" }>;
      const u = [...this.timeline];
      u[tIdx] = { ...old, tool: { ...old.tool, status: "success", output: { answer } } };
      this.timeline = u;
    }
    // Mirror to tools[] only in non-stream mode
    if (!this.useStreamSession) {
      const hIdx = this._findHeIdx(null, toolUseId);
      if (hIdx >= 0) {
        const u = [...this.tools];
        u[hIdx] = { ...u[hIdx], status: "done", hook_type: "PostToolUse" };
        this.tools = u;
      }
    }
  }

  /** Answer an AskUserQuestion tool via session message. */
  async answerToolQuestion(toolUseId: string, answer: string): Promise<void> {
    if (!this.run) return;
    dbg("store", "tool answer", { toolUseId, answer });
    // Transition UI immediately
    this.resolveAskQuestion(toolUseId, answer);
    try {
      // Send the user's answer as a follow-up message.
      // The session should be alive (idle phase) after the CLI auto-failed AskUserQuestion.
      if (this.sessionAlive) {
        await api.sendSessionMessage(this.run.id, answer);
      } else {
        dbgWarn("store", "session not alive for tool answer, skipping send");
      }
    } catch (e) {
      dbgWarn("store", "tool answer failed:", e);
      this.error = String(e);
      throw e;
    }
  }

  /** Unified permission resolution: traverses ALL timeline + subTimeline entries
   *  (no early return) to handle duplicate requestId entries from fallback/synthetic sources.
   *  - "deny" → permission_denied
   *  - "allow" → running (skips AskUserQuestion to avoid double-submit) */
  private _resolvePermission(action: "allow" | "deny", requestId: string): void {
    dbg("store", `resolvePermission${action === "allow" ? "Allow" : "Deny"}`, { requestId });
    const targetStatus = action === "allow" ? ("running" as const) : ("permission_denied" as const);
    const skipAsk = action === "allow";
    let changed = false;
    const u = [...this.timeline];
    for (let i = 0; i < u.length; i++) {
      const entry = u[i];
      if (entry.kind !== "tool") continue;
      if (
        entry.tool.status === "permission_prompt" &&
        entry.tool.permission_request_id === requestId
      ) {
        if (!(skipAsk && entry.tool.tool_name === "AskUserQuestion")) {
          u[i] = { ...entry, tool: { ...entry.tool, status: targetStatus } };
          changed = true;
        }
      }
      if (entry.subTimeline) {
        let subChanged = false;
        const newSub = [...entry.subTimeline];
        for (let j = 0; j < newSub.length; j++) {
          const sub = newSub[j];
          if (
            sub.kind === "tool" &&
            sub.tool.status === "permission_prompt" &&
            sub.tool.permission_request_id === requestId &&
            !(skipAsk && sub.tool.tool_name === "AskUserQuestion")
          ) {
            newSub[j] = { ...sub, tool: { ...sub.tool, status: targetStatus } };
            subChanged = true;
          }
        }
        if (subChanged) {
          u[i] = { ...u[i], subTimeline: newSub };
          changed = true;
        }
      }
    }
    if (changed) this.timeline = u;
  }

  resolvePermissionDeny(requestId: string): void {
    this._resolvePermission("deny", requestId);
  }

  resolvePermissionAllow(requestId: string): void {
    this._resolvePermission("allow", requestId);
  }

  /** Handle PTY exit event. */
  handlePtyExit(): void {
    const target: SessionPhase =
      this.run?.status === "running"
        ? "completed"
        : this.phase === "stopped"
          ? "stopped"
          : "completed";
    this._setPhase(target);
    this.ptySpawned = false;
    if (this.run) {
      api
        .getRun(this.run.id)
        .then((r) => {
          this.run = r;
        })
        .catch((e) => dbgWarn("store", "getRun after pty exit failed:", e));
    }
  }

  /** Handle chat-done event (pipe mode). */
  handleChatDone(_done: { ok: boolean; code: number; error?: string }): void {
    if (!this.run) return;

    if (this.run.agent === "codex") {
      this._setPhase("completed");
      api
        .getRun(this.run.id)
        .then((r) => {
          this.run = r;
        })
        .catch((e) => dbgWarn("store", "getRun after codex done failed:", e));
    }
  }

  /** Handle chat-delta event (pipe mode). */
  handleChatDelta(text: string, xtermRef?: { writeText(s: string): void }): void {
    if (!this.run) return;
    if (this.run.agent === "codex" && xtermRef) {
      xtermRef.writeText(text);
    }
  }

  // ── Private ──

  /** Whether to skip tools (HookEvent[]) mirror writes. Stream mode tools are in timeline only. */
  private _isStreamMode(ctx: ReduceCtx | null): boolean {
    return ctx ? ctx.isStream : this.useStreamSession;
  }

  /**
   * Resolve stale tool entries to "error" across main timeline and all subTimelines.
   * Used by idle/spawning/control_cancelled cleanup.
   */
  private _resolveStaleTools(
    predicate: (tool: BusToolItem) => boolean,
    ctx: ReduceCtx | null,
  ): void {
    const tl = ctx ? ctx.tl : this.timeline;
    let cloned = !!ctx; // ctx.tl is already a mutable reference

    for (let i = 0; i < tl.length; i++) {
      const e = tl[i];
      if (e.kind !== "tool") continue;

      // Top-level tool
      let parentUpdated = e;
      if (predicate(e.tool)) {
        if (!cloned) {
          this.timeline = [...this.timeline];
          cloned = true;
        }
        parentUpdated = { ...e, tool: { ...e.tool, status: "error" as const } };
        const target = ctx ? ctx.tl : this.timeline;
        target[i] = parentUpdated;
        dbg("store", "resolved stale tool", { id: e.id, name: e.tool.tool_name });
        // Don't continue: even if top-level matched, still scan and converge subTimeline children
      }

      // subTimeline children
      const sub = parentUpdated.subTimeline;
      if (!sub) continue;
      let subChanged = false;
      let newSub = sub;
      for (let j = 0; j < newSub.length; j++) {
        const child = newSub[j];
        if (child.kind !== "tool" || !predicate(child.tool)) continue;
        if (!subChanged) {
          newSub = [...newSub];
          subChanged = true;
        }
        newSub[j] = { ...child, tool: { ...child.tool, status: "error" as const } };
        dbg("store", "resolved stale sub-tool", { id: child.id, name: child.tool.tool_name });
      }
      if (subChanged) {
        if (!cloned) {
          this.timeline = [...this.timeline];
          cloned = true;
        }
        const target = ctx ? ctx.tl : this.timeline;
        target[i] = { ...parentUpdated, subTimeline: newSub };
      }
    }
  }

  /** Core reducer: apply a single bus event. When ctx is null, mutates $state directly.
   *  replayOnly=true skips phase and error assignments (used during resume replay). */
  private _reduce(ev: BusEvent, ctx: ReduceCtx | null, replayOnly = false): void {
    // Shorthand accessors — either batch ctx or this (reactive)
    const getTl = () => (ctx ? ctx.tl : this.timeline);
    const getHe = () => (ctx ? ctx.he : this.tools);
    const getSeenMsg = () => (ctx ? ctx.seenMessageIds : this._seenMessageIds);
    const getSeenTool = () => (ctx ? ctx.seenToolIds : this._seenToolIds);

    switch (ev.type) {
      case "session_init":
        if (ev.model) {
          if (ctx) {
            // Batch replay: always take CLI's model (loadRun restores per-run model afterward)
            ctx.model = ev.model;
          } else if (!this.run?.model) {
            // Live: only adopt CLI's model when no per-run model is set
            // (user's selection via ModelSelector takes priority)
            this.model = ev.model;
          }
        }
        // Persist the CLI's new session_id (important for fork — CLI generates a new ID)
        if (ev.session_id) {
          if (ctx) ctx.sessionId = ev.session_id;
          else if (this.run) {
            dbg("store", "session_init: updating session_id", {
              old: this.run.session_id,
              new: ev.session_id,
            });
            this.run = { ...this.run, session_id: ev.session_id };
          }
        }
        // Store CLI slash commands (session-specific, includes custom .claude/commands/)
        // CLI system/init returns slash_commands as string[] (just names) or CliCommand[]
        if (ev.slash_commands && ev.slash_commands.length > 0) {
          this.sessionCommands = ev.slash_commands.map((c: unknown) =>
            typeof c === "string" ? { name: c, description: "", aliases: [] } : (c as CliCommand),
          );
        }
        // Store MCP servers (per-session state)
        if (ev.mcp_servers && ev.mcp_servers.length > 0) {
          this.mcpServers = ev.mcp_servers;
        }
        // Store CLI verbose fields
        if (ev.claude_code_version) {
          this.cliVersion = ev.claude_code_version;
          // Only update global installed version from live sessions,
          // not from historical replay (which carries the old version).
          if (!replayOnly) {
            updateInstalledVersion(ev.claude_code_version);
            try {
              localStorage.setItem("ocv:cli-version", ev.claude_code_version);
            } catch {
              /* ignore */
            }
          }
        }
        // eslint-disable-next-line no-case-declarations -- scoped to session_init block
        const normalizedPermMode = ev.permissionMode
          ? normalizePermissionMode(ev.permissionMode)
          : undefined;
        if (normalizedPermMode && !this.permissionModeSetByUser) {
          this.permissionMode = normalizedPermMode;
        } else if (normalizedPermMode && this.permissionModeSetByUser) {
          dbg("store", "session_init permissionMode skipped — user already set", {
            cliValue: normalizedPermMode,
            userValue: this.permissionMode,
          });
          // CLI may have reset permission mode after compaction — re-send to resync.
          // Only in live mode (not batch replay) and when the run has a valid id.
          if (!ctx && this.run?.id && normalizedPermMode !== this.permissionMode) {
            dbg("store", "resync permissionMode to CLI after compaction", {
              mode: this.permissionMode,
            });
            api.setPermissionMode(this.run.id, this.permissionMode).catch((e) => {
              dbgWarn("store", "permissionMode resync failed", e);
            });
          }
        }
        if (ev.fast_mode_state) this.fastModeState = ev.fast_mode_state;
        if (ev.apiKeySource) this.apiKeySource = ev.apiKeySource;
        if (ev.agents && ev.agents.length > 0) this.availableAgents = ev.agents;
        if (ev.skills && ev.skills.length > 0) this.availableSkills = ev.skills;
        if (ev.plugins) this.availablePlugins = ev.plugins;
        // Always assign (not truthy-guarded) so CLI returning empty values clears stale state
        this.sessionCwd = ev.cwd ?? "";
        this.sessionTools = ev.tools ?? [];
        this.outputStyle = ev.output_style ?? "";
        this.sessionInitReceived = true;
        dbg("store", "session_init: cli verbose fields", {
          cliVersion: this.cliVersion,
          permissionMode: this.permissionMode,
          fastModeState: this.fastModeState,
          apiKeySource: this.apiKeySource,
          agents: this.availableAgents.length,
          skills: this.availableSkills.length,
          plugins: this.availablePlugins.length,
          sessionCwd: this.sessionCwd,
          sessionTools: this.sessionTools.length,
          outputStyle: this.outputStyle,
        });
        break;

      case "message_delta":
        this._clearTimeoutError();
        if (ev.parent_tool_use_id) {
          this._appendSubTimelineStreamingDelta(ev.parent_tool_use_id, "content", ev.text, ctx);
          break;
        }
        // Mark thinking end: first text delta after thinking started
        if (this.thinkingStartMs && !this.thinkingEndMs) {
          this.thinkingEndMs = eventTsMs(ev);
        }
        if (ctx) ctx.streamText += ev.text;
        else this.streamingText += ev.text;
        break;

      case "thinking_delta":
        this._clearTimeoutError();
        if (ev.parent_tool_use_id) {
          this._appendSubTimelineStreamingDelta(
            ev.parent_tool_use_id,
            "thinkingText",
            ev.text,
            ctx,
          );
          break;
        }
        if (!this.thinkingStartMs) this.thinkingStartMs = eventTsMs(ev);
        if (ctx) ctx.thinkingText += ev.text;
        else this.thinkingText += ev.text;
        break;

      case "tool_input_delta": {
        if (ev.parent_tool_use_id) {
          this._updateSubTimelineToolInput(
            ev.parent_tool_use_id,
            ev.tool_use_id,
            ev.partial_json,
            ctx,
          );
          break;
        }
        // Update matching tool entry's input in real-time with accumulated partial JSON
        const tl = getTl();
        const tIdx = this._findToolIdx(ctx, ev.tool_use_id);
        if (tIdx >= 0) {
          const old = tl[tIdx] as Extract<TimelineEntry, { kind: "tool" }>;
          const accum = SessionStore._accumulateJsonInput(
            old.tool as Record<string, unknown>,
            ev.partial_json,
          );
          const updated: TimelineEntry = {
            ...old,
            tool: { ...old.tool, ...accum } as typeof old.tool,
          };
          if (ctx) {
            ctx.tl[tIdx] = updated;
          } else {
            const u = [...this.timeline];
            u[tIdx] = updated;
            this.timeline = u;
          }
        }
        break;
      }

      case "message_complete": {
        // Dedup guard — but always clean up synthetic entry first to prevent leaks
        if (getSeenMsg().has(ev.message_id)) {
          if (ev.parent_tool_use_id)
            this._removeSubTimelineStreamingEntry(ev.parent_tool_use_id, ctx);
          break;
        }
        getSeenMsg().add(ev.message_id);
        if (getTl().some((e) => e.kind === "assistant" && e.id === ev.message_id)) {
          if (ev.parent_tool_use_id)
            this._removeSubTimelineStreamingEntry(ev.parent_tool_use_id, ctx);
          break;
        }

        // Subagent path: extract thinking → remove synthetic → create entry → append
        if (ev.parent_tool_use_id) {
          const subThinking = this._extractSubTimelineThinking(ev.parent_tool_use_id, ctx);
          this._removeSubTimelineStreamingEntry(ev.parent_tool_use_id, ctx);

          const entry: TimelineEntry = {
            kind: "assistant",
            id: ev.message_id,
            anchorId: ev.message_id,
            content: ev.text,
            ts: eventTs(ev),
            ...(ev.model ? { model: ev.model } : {}),
            ...(subThinking ? { thinkingText: subThinking } : {}),
          };
          dbg("store", "subagent thinking persisted", {
            parent: ev.parent_tool_use_id,
            len: subThinking?.length ?? 0,
          });

          const parentIdx = this._findParentToolIdx(ctx, ev.parent_tool_use_id);
          if (parentIdx >= 0) {
            this._appendToSubTimeline(getTl(), parentIdx, entry, ctx);
            break;
          }
          dbgWarn(
            "store",
            "subagent message_complete: parent not found, fallback to main timeline",
            { parent: ev.parent_tool_use_id },
          );
          this._pushTimeline(ctx, entry);
          break;
        }

        // Main session path: save thinking before clearing
        const savedThinking = ctx ? ctx.thinkingText : this.thinkingText;
        if (ctx) {
          ctx.streamText = "";
          ctx.thinkingText = "";
        } else {
          this.streamingText = "";
          this.thinkingText = "";
        }
        this.thinkingStartMs = 0;
        this.thinkingEndMs = 0;

        const entry: TimelineEntry = {
          kind: "assistant",
          id: ev.message_id,
          anchorId: ev.message_id,
          content: ev.text,
          ts: eventTs(ev),
          ...(ev.model ? { model: ev.model } : {}),
          ...(savedThinking ? { thinkingText: savedThinking } : {}),
        };
        if (savedThinking)
          dbg("store", "thinking persisted to timeline", {
            id: ev.message_id,
            len: savedThinking.length,
          });

        this._pushTimeline(ctx, entry);
        break;
      }

      case "user_message": {
        const tl = getTl();
        // Content-based dedup: only in live mode (replayOnly=false) where an optimistic
        // user entry was already added by startSession/sendMessage.  During replay
        // (replayOnly=true), every event from events.jsonl is authoritative —
        // the user may legitimately send the same text twice in different turns.
        if (!replayOnly) {
          // Find the most recent user entry with matching text that hasn't been UUID-confirmed yet.
          // Using backward search (findLast) avoids matching old replayed entries from before
          // Phase 1 (which also lack cliUuid). For rapid-fire identical messages, UUID assignment
          // order is reversed (LIFO), but this is functionally correct — each entry still gets a
          // unique UUID for checkpoint identification.
          const match = tl.findLast(
            (e) => e.kind === "user" && e.content === ev.text && !e.cliUuid,
          );
          if (match && match.kind === "user") {
            // Merge cliUuid + anchorId from the confirmed backend event into the optimistic entry
            if (ev.uuid) {
              const idx = tl.indexOf(match);
              const updated = { ...match, cliUuid: ev.uuid, anchorId: ev.uuid };
              if (ctx) ctx.tl[idx] = updated;
              else {
                const u = [...this.timeline];
                u[idx] = updated;
                this.timeline = u;
              }
            }
            break;
          }
        }
        const newId = uuid();
        const entry: TimelineEntry = {
          kind: "user",
          id: newId,
          anchorId: ev.uuid || newId,
          content: ev.text,
          ts: eventTs(ev),
          ...(ev.uuid ? { cliUuid: ev.uuid } : {}),
        };
        this._pushTimeline(ctx, entry);

        // Resolve any ask_pending AskUserQuestion tool — the user_message following
        // a tool_end(AskUserQuestion) is the user's answer.  Without this, navigating
        // away and back replays bus events and resets the tool to ask_pending.
        const pendingIdx = tl.findIndex(
          (e) => e.kind === "tool" && e.tool.status === "ask_pending",
        );
        if (pendingIdx >= 0) {
          const old = tl[pendingIdx] as Extract<TimelineEntry, { kind: "tool" }>;
          const resolved: TimelineEntry = {
            ...old,
            tool: { ...old.tool, status: "success", output: { answer: ev.text } },
          };
          if (ctx) {
            ctx.tl[pendingIdx] = resolved;
          } else {
            const u = [...this.timeline];
            u[pendingIdx] = resolved;
            this.timeline = u;
          }
          // Also resolve the matching HookEvent (non-stream mode only)
          if (!this._isStreamMode(ctx)) {
            const he = getHe();
            const hIdx = this._findHeIdxByStatus(ctx, old.id, "running");
            if (hIdx >= 0) {
              const updatedHe: HookEvent = {
                ...he[hIdx],
                status: "done",
                hook_type: "PostToolUse",
              };
              if (ctx) {
                ctx.he[hIdx] = updatedHe;
              } else {
                const u = [...this.tools];
                u[hIdx] = updatedHe;
                this.tools = u;
              }
            }
          }
        }
        break;
      }

      case "tool_start": {
        this._clearTimeoutError();
        if (getSeenTool().has(ev.tool_use_id)) break;
        getSeenTool().add(ev.tool_use_id);
        // Subagent routing: nest inside parent tool's subTimeline
        if (ev.parent_tool_use_id) {
          const parentIdx = this._findParentToolIdx(ctx, ev.parent_tool_use_id);
          if (parentIdx >= 0) {
            const subEntry: TimelineEntry = {
              kind: "tool",
              id: ev.tool_use_id,
              anchorId: ev.tool_use_id,
              tool: {
                tool_use_id: ev.tool_use_id,
                tool_name: ev.tool_name,
                input: (ev.input as Record<string, unknown>) ?? {},
                status: "running",
              },
              ts: eventTs(ev),
            };
            this._appendToSubTimeline(getTl(), parentIdx, subEntry, ctx);
            break;
          }
          dbgWarn("store", "subagent tool_start: parent not found, fallback to main timeline", {
            parent: ev.parent_tool_use_id,
          });
        }
        if (this._findToolIdx(ctx, ev.tool_use_id) >= 0) break;

        const tlEntry: TimelineEntry = {
          kind: "tool",
          id: ev.tool_use_id,
          anchorId: ev.tool_use_id,
          tool: {
            tool_use_id: ev.tool_use_id,
            tool_name: ev.tool_name,
            input: (ev.input as Record<string, unknown>) ?? {},
            status: "running",
          },
          ts: eventTs(ev),
        };
        this._pushTimeline(ctx, tlEntry);

        // Mirror to tools[] (HookEvent) only in non-stream mode (pipe/PTY)
        if (!this._isStreamMode(ctx)) {
          const heEntry: HookEvent = {
            run_id: ev.run_id,
            hook_type: "PreToolUse",
            tool_name: ev.tool_name,
            tool_input: ev.input as Record<string, unknown>,
            status: "running",
            timestamp: new Date().toISOString(),
          };
          (heEntry as Record<string, unknown>).tool_use_id = ev.tool_use_id;
          this._pushHookEntry(ctx, heEntry);
        }
        break;
      }

      case "tool_end": {
        // AskUserQuestion handling:
        // - pipe mode: CLI returns error → ask_pending (frontend shows interactive options)
        // - stream-json mode: CLI returns success (answer provided via updatedInput) → success
        const isAskUser = ev.tool_name === "AskUserQuestion";
        const resolvedStatus =
          isAskUser && ev.status === "error"
            ? ("ask_pending" as const)
            : ev.status === "error"
              ? ("error" as const)
              : ("success" as const);

        // Subagent routing: update child tool inside parent's subTimeline
        if (ev.parent_tool_use_id) {
          if (
            this._updateSubTimelineTool(
              ev.parent_tool_use_id,
              ev.tool_use_id,
              (t) => ({
                ...t,
                status: resolvedStatus,
                output: ev.output as Record<string, unknown>,
                duration_ms: ev.duration_ms,
                tool_name: ev.tool_name || t.tool_name,
                tool_use_result: ev.tool_use_result as Record<string, unknown> | undefined,
              }),
              ctx,
            )
          ) {
            break;
          }
          dbgWarn(
            "store",
            "subagent tool_end: not found in subTimeline, fallback to main timeline",
            { parent: ev.parent_tool_use_id, tool: ev.tool_use_id },
          );
          // fall through to main timeline logic
        }

        const tl = getTl();
        const tIdx = this._findToolIdx(ctx, ev.tool_use_id);
        if (tIdx >= 0) {
          const old = tl[tIdx] as Extract<TimelineEntry, { kind: "tool" }>;
          const updated: TimelineEntry = {
            ...old,
            tool: {
              ...old.tool,
              status: resolvedStatus,
              output: ev.output as Record<string, unknown>,
              duration_ms: ev.duration_ms,
              tool_name: ev.tool_name || old.tool.tool_name,
              tool_use_result: ev.tool_use_result as Record<string, unknown> | undefined,
            },
          };
          if (ctx) {
            ctx.tl[tIdx] = updated;
          } else {
            const u = [...this.timeline];
            u[tIdx] = updated;
            this.timeline = u;
          }
        }

        // Plan mode inference: only top-level tools in live mode affect main session permissionMode.
        // Subagent EnterPlanMode should not change the parent session's mode.
        // replayOnly guard: replaying a historical session that ended mid-plan must not
        // pollute the current permissionMode (which is a user-level preference, not snapshot state).
        if (!replayOnly && ev.status !== "error" && !ev.parent_tool_use_id) {
          if (ev.tool_name === "EnterPlanMode") {
            this.previousPermissionMode = this.permissionMode || "default";
            this.permissionMode = "plan";
            dbg("store", "tool_end: EnterPlanMode → permissionMode=plan", {
              previous: this.previousPermissionMode,
            });
          } else if (ev.tool_name === "ExitPlanMode" && this.previousPermissionMode) {
            if (this.pendingPermissionModeOverride) {
              // User chose a specific mode via ExitPlanMode approval card
              this.permissionMode = this.pendingPermissionModeOverride;
              this.pendingPermissionModeOverride = null;
              dbg("store", "tool_end: ExitPlanMode → permissionMode overridden", {
                mode: this.permissionMode,
              });
            } else {
              const restored = this.previousPermissionMode;
              this.permissionMode = restored;
              dbg("store", "tool_end: ExitPlanMode → permissionMode restored", { restored });
            }
            this.previousPermissionMode = "";

            // "Clear context" deferred handling: extract plan from tool result
            if (this.pendingClearContextPlan === "__pending__") {
              const toolResult = ev.tool_use_result as Record<string, unknown> | undefined;
              const plan =
                (ev.output as Record<string, unknown> | undefined)?.plan || toolResult?.plan;
              if (plan && typeof plan === "string") {
                this.pendingClearContextPlan = plan;
                dbg("store", "ExitPlanMode: plan content captured for clear context");
              } else {
                this.pendingClearContextPlan = null;
                dbgWarn("store", "ExitPlanMode: no plan found in tool result for clear context");
              }
            }
          }
        }

        // Mirror to tools[] only in non-stream mode
        if (!isAskUser && !this._isStreamMode(ctx)) {
          const he = getHe();
          const hIdx = this._findHeIdxByStatus(ctx, ev.tool_use_id, "running");
          if (hIdx >= 0) {
            const updatedHe: HookEvent = {
              ...he[hIdx],
              status: "done",
              hook_type: "PostToolUse",
              tool_name: ev.tool_name || he[hIdx].tool_name,
              tool_output: ev.output as Record<string, unknown>,
            };
            if (ctx) {
              ctx.he[hIdx] = updatedHe;
            } else {
              const u = [...this.tools];
              u[hIdx] = updatedHe;
              this.tools = u;
            }
          }
        }
        break;
      }

      case "run_state":
        if (!replayOnly) {
          if (ev.state === "running" || ev.state === "spawning") {
            const newPhase: SessionPhase = ev.state === "spawning" ? "spawning" : "running";
            if (ctx) ctx.phase = newPhase;
            else this._setPhase(newPhase);
            // Invalidate idle snapshot — session is now active
            if (!ctx && this.run) {
              snapshotCache.deleteSnapshot(this.run.id).catch(() => {});
            }
          } else if (ev.state === "idle") {
            if (ctx) ctx.phase = "idle";
            else this._setPhase("idle");
          } else {
            // completed / failed / stopped
            const termPhase = ev.state as SessionPhase;
            if (ctx) ctx.phase = termPhase;
            else {
              this._setPhase(termPhase);
              if (this.run) {
                const snapId = this.run.id;
                api
                  .getRun(snapId)
                  .then((r) => {
                    // Guard: only update if we're still viewing the same run
                    if (this.run?.id === snapId) this.run = r;
                  })
                  .catch((e) => dbgWarn("store", "getRun after terminal state failed:", e));
              }
            }
          }
          // Sync run.status for non-terminal states so status bar reflects reality
          // (terminal states update run via api.getRun above)
          if (ev.state === "running" || ev.state === "idle") {
            if (ctx) ctx.runStatus = ev.state;
            else if (this.run) this.run = { ...this.run, status: ev.state };
          }
        }
        // Show error to user only for genuine failures, not user-initiated stops.
        // "stopped" = user clicked stop; "failed" after stop = CLI dying mid-request (expected).
        // _stopping flag: set by stop() before IPC call, covers the interrupt+kill window.
        if (!replayOnly && ev.error && ev.state !== "stopped" && !this._stopping) {
          if (ctx) ctx.error = ev.error;
          else this.error = ev.error;
        }
        // Resolve stale permission_prompt / optimistic-running tools on idle transition.
        // When CLI goes idle (turn complete), any remaining permission_prompt cards are stale
        // (e.g. user interrupted during a pending can_use_tool request).
        // Also resolve "optimistic running" tools (have permission_request_id) that never got a tool_end.
        // Covers both main timeline and subTimelines.
        if (ev.state === "idle") {
          this._resolveStaleTools(
            (t) =>
              t.status === "permission_prompt" ||
              (t.status === "running" && !!t.permission_request_id),
            ctx,
          );
          // Write idle snapshot (live mode only, throttled by _lastSnapshotSeq)
          if (!ctx && !replayOnly && this.run) {
            if (this._lastProcessedSeq > this._lastSnapshotSeq) {
              this._saveSnapshotToIdb(this.run.id);
              this._lastSnapshotSeq = this._lastProcessedSeq;
            }
          }
        }
        // Resolve permission_denied / permission_prompt tools on session restart (spawning).
        // After approval, the session restarts — those cards are no longer actionable.
        // Runs in both live and replay mode so replayed sessions show resolved state.
        // Covers both main timeline and subTimelines.
        if (ev.state === "spawning") {
          this._resolveStaleTools(
            (t) =>
              t.status === "permission_denied" ||
              t.status === "permission_prompt" ||
              (t.status === "running" && !!t.permission_request_id),
            ctx,
          );
        }
        // Clear stale elicitations on state transitions — CLI won't send control_cancelled
        // for these if the session ends abnormally or restarts.
        if (
          ev.state === "idle" ||
          ev.state === "spawning" ||
          ev.state === "completed" ||
          ev.state === "failed" ||
          ev.state === "stopped"
        ) {
          if (this.pendingElicitations.size > 0) {
            dbg("store", "run_state clearing stale elicitations", {
              state: ev.state,
              count: this.pendingElicitations.size,
            });
            this.pendingElicitations = new Map();
          }
        }
        break;

      case "usage_update": {
        const u: UsageState = {
          inputTokens: ev.input_tokens,
          outputTokens: ev.output_tokens,
          cacheReadTokens: ev.cache_read_tokens ?? 0,
          cacheWriteTokens: ev.cache_write_tokens ?? 0,
          cost: ev.total_cost_usd,
          modelUsage: ev.model_usage,
          durationApiMs: ev.duration_api_ms,
        };
        // Don't let an all-zero-token usage (from error results) overwrite real data.
        // CLI sometimes sends cost-only usage on error — preserve previous token counts.
        const prev = ctx ? ctx.usage : this.usage;
        const hasTokens =
          u.inputTokens > 0 ||
          u.outputTokens > 0 ||
          u.cacheReadTokens > 0 ||
          u.cacheWriteTokens > 0;
        const merged = hasTokens ? u : { ...prev, cost: Math.max(prev.cost, u.cost) };
        // Preserve modelUsage/durationApiMs even on zero-token update (error results may still have them)
        if (!hasTokens && u.modelUsage) merged.modelUsage = u.modelUsage;
        if (!hasTokens && u.durationApiMs) merged.durationApiMs = u.durationApiMs;
        if (ctx) ctx.usage = merged;
        else this.usage = merged;
        // Store duration_ms and num_turns from result events
        if (ev.duration_ms != null) this.durationMs = ev.duration_ms;
        if (ev.num_turns != null) this.numTurns = ev.num_turns;

        // Append per-turn usage snapshot (raw event data, not merged)
        // Use backend-authoritative turn_index when available; fall back to counting
        // user entries in the timeline (for backwards compat with older events).
        const tl = getTl();
        const fallbackIdx = tl.filter((e) => e.kind === "user").length;
        const turnIdx = ev.turn_index ?? fallbackIdx;
        dbg("store", "usage_update turn_index", {
          backend: ev.turn_index,
          fallback: fallbackIdx,
          used: turnIdx,
        });
        const turnSnap: TurnUsage = {
          turnIndex: turnIdx,
          inputTokens: u.inputTokens,
          outputTokens: u.outputTokens,
          cacheReadTokens: u.cacheReadTokens,
          cacheWriteTokens: u.cacheWriteTokens,
          cost: u.cost,
          durationApiMs: u.durationApiMs,
          durationMs: ev.duration_ms,
        };
        if (ctx) {
          ctx.turnUsages.push(turnSnap);
        } else {
          this.turnUsages = [...this.turnUsages, turnSnap];
        }
        break;
      }

      case "permission_denied": {
        // Retroactively update: find matching tool, change to "permission_denied"
        const tl = getTl();
        const tIdx = this._findToolIdx(ctx, ev.tool_use_id);
        if (tIdx >= 0) {
          const old = tl[tIdx] as Extract<TimelineEntry, { kind: "tool" }>;
          const updated: TimelineEntry = {
            ...old,
            tool: { ...old.tool, status: "permission_denied" },
          };
          if (ctx) {
            ctx.tl[tIdx] = updated;
          } else {
            const u = [...this.timeline];
            u[tIdx] = updated;
            this.timeline = u;
          }
        } else {
          // Not in main timeline — search subTimelines (CLI may omit parent_tool_use_id)
          this._updateToolInAnySubTimeline(
            ev.tool_use_id,
            (t) => ({ ...t, status: "permission_denied" as const }),
            ctx,
          );
        }
        break;
      }

      case "permission_prompt": {
        dbg("store", "permission_prompt received", {
          tool_use_id: ev.tool_use_id,
          request_id: ev.request_id,
          tool_name: ev.tool_name,
          parent: ev.parent_tool_use_id,
          batch: !!ctx,
        });
        // Subagent routing: update child tool inside parent's subTimeline
        if (ev.parent_tool_use_id) {
          if (
            this._updateSubTimelineTool(
              ev.parent_tool_use_id,
              ev.tool_use_id,
              (t) => ({
                ...t,
                status: "permission_prompt" as const,
                permission_request_id: ev.request_id,
                ...(ev.suggestions && ev.suggestions.length > 0
                  ? { suggestions: ev.suggestions }
                  : {}),
              }),
              ctx,
            )
          ) {
            break;
          }
          dbgWarn(
            "store",
            "subagent permission_prompt: not found in subTimeline, fallback to main timeline",
            { parent: ev.parent_tool_use_id, tool: ev.tool_use_id },
          );
          // fall through to main timeline logic
        }
        // Inline permission prompt from --permission-prompt-tool stdio.
        // Find matching tool (should be "running") and update to "permission_prompt" with request_id.
        const tl = getTl();
        const tIdx = this._findToolIdx(ctx, ev.tool_use_id);
        if (tIdx >= 0) {
          const old = tl[tIdx] as Extract<TimelineEntry, { kind: "tool" }>;
          const updated: TimelineEntry = {
            ...old,
            tool: {
              ...old.tool,
              status: "permission_prompt",
              permission_request_id: ev.request_id,
              // Merge suggestions from permission_prompt event (CLI provides these)
              ...(ev.suggestions && ev.suggestions.length > 0
                ? { suggestions: ev.suggestions }
                : {}),
            },
          };
          if (ctx) {
            ctx.tl[tIdx] = updated;
          } else {
            const u = [...this.timeline];
            u[tIdx] = updated;
            this.timeline = u;
          }
          dbg("store", "permission_prompt: updated existing entry", {
            tIdx,
            tool_use_id: ev.tool_use_id,
            request_id: ev.request_id,
          });
        } else {
          // Tool not in main timeline — check ALL subTimelines (CLI sometimes omits parent_tool_use_id)
          const foundInSub = this._updateToolInAnySubTimeline(
            ev.tool_use_id,
            (t) => ({
              ...t,
              status: "permission_prompt" as const,
              permission_request_id: ev.request_id,
              ...(ev.suggestions && ev.suggestions.length > 0
                ? { suggestions: ev.suggestions }
                : {}),
            }),
            ctx,
          );
          if (!foundInSub) {
            // Truly new — create a synthetic tool entry in main timeline
            dbg("store", "permission_prompt: creating synthetic entry", {
              tool_use_id: ev.tool_use_id,
              request_id: ev.request_id,
              tool_name: ev.tool_name,
            });
            const tlEntry: TimelineEntry = {
              kind: "tool",
              id: ev.tool_use_id,
              anchorId: ev.tool_use_id,
              tool: {
                tool_use_id: ev.tool_use_id,
                tool_name: ev.tool_name,
                input: ev.tool_input as Record<string, unknown>,
                status: "permission_prompt",
                permission_request_id: ev.request_id,
                ...(ev.suggestions && ev.suggestions.length > 0
                  ? { suggestions: ev.suggestions }
                  : {}),
              },
              ts: eventTs(ev),
            };
            this._pushTimeline(ctx, tlEntry);
          } else {
            dbg("store", "permission_prompt: updated in subTimeline", {
              tool_use_id: ev.tool_use_id,
              request_id: ev.request_id,
            });
          }
        }
        break;
      }

      case "compact_boundary": {
        const isMicro = (ev.trigger ?? "").startsWith("micro");
        if (isMicro) {
          this.microcompactCount++;
        } else {
          this.compactCount++;
          // Full compaction: insert timeline separator
          const tokensInfo = ev.pre_tokens ? ` (${Math.round(ev.pre_tokens / 1000)}k tokens)` : "";
          const sepId = uuid();
          const entry: TimelineEntry = {
            kind: "separator",
            id: sepId,
            anchorId: sepId,
            content: `Context compacted${tokensInfo}`,
            ts: eventTs(ev),
          };
          this._pushTimeline(ctx, entry);
          // Reset per-turn token counts so contextUtilization reflects the
          // compacted state instead of showing stale pre-compact values.
          // The next usage_update event will supply accurate post-compact numbers.
          // Only reset on full compaction — micro-compaction keeps the existing
          // usage so the progress bar does not flash 90%→0%→85%.
          dbg("store", "compact: reset context usage", { preTokens: ev.pre_tokens });
          const prev = ctx ? ctx.usage : this.usage;
          const reset = { ...prev, inputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 };
          if (ctx) ctx.usage = reset;
          else this.usage = reset;
        }
        // Only set lastCompactedAt during live mode — during replay
        // the timestamp would be meaningless (Date.now() ≠ original event time).
        if (!replayOnly) {
          this.lastCompactedAt = Date.now();
        }
        break;
      }

      case "command_output": {
        dbg("store", "command_output received", {
          contentLen: ev.content.length,
          hasBatchCtx: !!ctx,
        });
        const cmdId = uuid();
        const cmdEntry: TimelineEntry = {
          kind: "command_output",
          id: cmdId,
          anchorId: cmdId,
          content: ev.content,
          ts: eventTs(ev),
        };
        this._pushTimeline(ctx, cmdEntry);
        break;
      }

      case "elicitation_prompt": {
        dbg("store", "elicitation_prompt received", {
          request_id: ev.request_id,
          server: ev.mcp_server_name,
          mode: ev.mode,
        });
        const updated = new Map(this.pendingElicitations);
        updated.set(ev.request_id, {
          requestId: ev.request_id,
          mcpServerName: ev.mcp_server_name,
          message: ev.message,
          elicitationId: ev.elicitation_id,
          mode: ev.mode,
          url: ev.url,
          requestedSchema: ev.requested_schema,
        });
        this.pendingElicitations = updated;
        break;
      }

      case "raw": {
        const rawText = typeof ev.data === "string" ? ev.data : JSON.stringify(ev.data);
        if (rawText && (ev.source === "claude_stdout_text" || ev.source === "claude_stderr")) {
          const rawId = uuid();
          const entry: TimelineEntry = {
            kind: "assistant",
            id: rawId,
            anchorId: rawId,
            content: `\`[${ev.source}]\` ${rawText}`,
            ts: new Date().toISOString(),
          };
          this._pushTimeline(ctx, entry);
        } else {
          this.rawFallbackCount++;
          dbgWarn("store", "raw fallback event:", ev.source, rawText?.slice(0, 100));
          if (this.strictMode) {
            throw new Error(`[STRICT] raw fallback event: source=${ev.source}`);
          }
        }
        break;
      }

      case "system_status":
        this.systemStatus = { status: ev.status };
        break;

      case "auth_status":
        this.authStatus = { is_authenticating: ev.is_authenticating, output: ev.output };
        break;

      case "hook_started":
        this.hookEvents = [
          ...this.hookEvents,
          {
            type: ev.type,
            hook_id: ev.hook_id,
            data: ev,
            hook_name: ev.hook_name,
          },
        ];
        break;

      case "hook_progress":
        this.hookEvents = [
          ...this.hookEvents,
          {
            type: ev.type,
            hook_id: ev.hook_id,
            data: ev,
          },
        ];
        break;

      case "hook_response":
        this.hookEvents = [
          ...this.hookEvents,
          {
            type: ev.type,
            hook_id: ev.hook_id,
            data: ev,
            hook_name: ev.hook_name,
            stdout: ev.stdout,
            stderr: ev.stderr,
            exit_code: ev.exit_code,
          },
        ];
        break;

      case "hook_callback":
        // Hook callback from CLI — PreToolUse hooks are actionable (allow/deny)
        this.hookEvents = [
          ...this.hookEvents,
          {
            type: ev.type,
            hook_id: ev.hook_id,
            data: ev,
            request_id: ev.request_id,
            status: ev.hook_event === "PreToolUse" ? "hook_pending" : "allowed",
          },
        ];
        break;

      case "task_notification": {
        const existing = this.taskNotifications.get(ev.task_id);
        const rawData = ev.data as Record<string, unknown> | undefined;
        const message =
          (rawData?.summary as string) ??
          (rawData?.message as string) ??
          (rawData?.task_description as string) ??
          ev.task_id;
        const updated = new Map(this.taskNotifications);
        updated.set(ev.task_id, {
          task_id: ev.task_id,
          status: ev.status,
          message,
          startedAt: existing?.startedAt ?? Date.now(),
          data: ev,
          output_file:
            ((rawData?.output_file ?? rawData?.outputFile) as string | undefined) ??
            existing?.output_file,
          task_type:
            ((rawData?.task_type ?? rawData?.taskType) as string | undefined) ??
            existing?.task_type,
          summary: (rawData?.summary as string | undefined) ?? existing?.summary,
          tool_use_id:
            ((rawData?.tool_use_id ?? rawData?.toolUseId) as string | undefined) ??
            existing?.tool_use_id,
        });
        this.taskNotifications = updated;
        break;
      }

      case "files_persisted":
        this.persistedFiles = [
          ...this.persistedFiles,
          ...(Array.isArray(ev.files) ? ev.files : []),
        ];
        break;

      case "tool_progress": {
        if (ev.parent_tool_use_id) {
          this._updateSubTimelineTool(
            ev.parent_tool_use_id,
            ev.tool_use_id,
            (t) => ({
              ...t,
              elapsed_time_seconds: ev.elapsed_time_seconds,
            }),
            ctx,
          );
          break;
        }
        const tl = getTl();
        const idx = this._findToolIdx(ctx, ev.tool_use_id);
        if (idx >= 0) {
          const old = tl[idx] as Extract<TimelineEntry, { kind: "tool" }>;
          const updated: TimelineEntry = {
            ...old,
            tool: { ...old.tool, elapsed_time_seconds: ev.elapsed_time_seconds },
          };
          if (ctx) ctx.tl[idx] = updated;
          else {
            const u = [...this.timeline];
            u[idx] = updated;
            this.timeline = u;
          }
        }
        break;
      }

      case "tool_use_summary": {
        if (ev.parent_tool_use_id) {
          this._updateSubTimelineTool(
            ev.parent_tool_use_id,
            ev.tool_use_id,
            (t) => ({
              ...t,
              summary: ev.summary,
            }),
            ctx,
          );
          break;
        }
        const tl2 = getTl();
        const idx2 = this._findToolIdx(ctx, ev.tool_use_id);
        if (idx2 >= 0) {
          const old = tl2[idx2] as Extract<TimelineEntry, { kind: "tool" }>;
          const updated: TimelineEntry = { ...old, tool: { ...old.tool, summary: ev.summary } };
          if (ctx) ctx.tl[idx2] = updated;
          else {
            const u = [...this.timeline];
            u[idx2] = updated;
            this.timeline = u;
          }
        }
        break;
      }

      case "control_cancelled": {
        // Resolve any permission_prompt or optimistic-running tool with matching request_id to "error"
        // Covers both main timeline and subTimelines.
        this._resolveStaleTools(
          (t) =>
            (t.status === "permission_prompt" || t.status === "running") &&
            t.permission_request_id === ev.request_id,
          ctx,
        );
        // Also resolve pending hook callbacks
        this.hookEvents = this.hookEvents.map((h) =>
          h.request_id === ev.request_id && h.status === "hook_pending"
            ? { ...h, status: "cancelled" as const }
            : h,
        );
        // Remove cancelled elicitation
        if (this.pendingElicitations.has(ev.request_id)) {
          const elicUpdated = new Map(this.pendingElicitations);
          elicUpdated.delete(ev.request_id);
          this.pendingElicitations = elicUpdated;
        }
        break;
      }

      // ── Ralph Loop events ──
      case "ralph_started": {
        this.ralphLoop = {
          active: true,
          prompt: ev.prompt,
          iteration: 0,
          maxIterations: ev.max_iterations,
          completionPromise: ev.completion_promise,
          startedAt: ev.started_at,
          reason: null,
        };
        dbg("store", "ralph_started", {
          maxIterations: ev.max_iterations,
          promise: ev.completion_promise,
        });
        break;
      }
      case "ralph_iteration": {
        if (this.ralphLoop) {
          this.ralphLoop = {
            ...this.ralphLoop,
            iteration: ev.iteration,
            maxIterations: ev.max_iterations,
          };
        }
        // Insert iteration separator in timeline
        const iterLabel =
          ev.max_iterations > 0
            ? `Ralph iteration ${ev.iteration}/${ev.max_iterations}`
            : `Ralph iteration ${ev.iteration}`;
        const iterSepId = uuid();
        this._pushTimeline(ctx, {
          kind: "separator",
          id: iterSepId,
          anchorId: iterSepId,
          content: `🔄 ${iterLabel}`,
          ts: eventTs(ev),
        });
        dbg("store", "ralph_iteration", { iteration: ev.iteration });
        break;
      }
      case "ralph_complete": {
        // Insert completion separator in timeline
        const reasonLabels: Record<string, string> = {
          max_iterations: "max iterations reached",
          completion_promise: "completion promise matched",
          cancelled: "cancelled",
          fail_stopped: "stopped after consecutive failures",
        };
        const reasonText = reasonLabels[ev.reason] ?? ev.reason;
        const completeIcon =
          ev.reason === "cancelled" || ev.reason === "fail_stopped" ? "❌" : "✅";
        const completeSepId = uuid();
        this._pushTimeline(ctx, {
          kind: "separator",
          id: completeSepId,
          anchorId: completeSepId,
          content: `${completeIcon} Ralph Loop completed · ${ev.iteration} iterations · ${reasonText}`,
          ts: eventTs(ev),
        });
        if (this.ralphLoop) {
          this.ralphLoop = {
            ...this.ralphLoop,
            active: false,
            iteration: ev.iteration,
            reason: ev.reason,
          };
        }
        dbg("store", "ralph_complete", { reason: ev.reason, iteration: ev.iteration });
        break;
      }

      default:
        this.unknownEventCount++;
        dbgWarn("store", "unknown bus event type:", (ev as Record<string, unknown>).type);
        if (this.strictMode) {
          throw new Error(`[STRICT] unknown event type: ${(ev as Record<string, unknown>).type}`);
        }
    }
  }
}

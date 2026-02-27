/**
 * Session state machine phases.
 * Replaces the 3-boolean combo: running x sending x sessionStarted.
 */
export type SessionPhase =
  | "empty" // No run loaded
  | "loading" // Loading run data
  | "ready" // Run loaded, waiting for user input
  | "spawning" // Process being created
  | "running" // Agent processing a turn
  | "idle" // Turn complete, waiting for next message
  | "completed" // Session ended normally
  | "failed" // Session ended with error
  | "stopped"; // User stopped session

export interface UsageState {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  cost: number;
  modelUsage?: Record<string, import("$lib/types").ModelUsageEntry>;
  durationApiMs?: number;
}

/** Per-turn token usage snapshot (appended on each usage_update event). */
export interface TurnUsage {
  /** Matches ToolActivity's turnIndex (number of user_messages seen before this usage_update). */
  turnIndex: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  cost: number;
  durationApiMs?: number;
  /** Wall-clock duration for this turn (from result event's duration_ms). */
  durationMs?: number;
}

export const ACTIVE_PHASES: SessionPhase[] = ["spawning", "running"];
export const TERMINAL_PHASES: SessionPhase[] = ["completed", "failed", "stopped"];
export const SESSION_ALIVE_PHASES: SessionPhase[] = ["spawning", "running", "idle"];

/**
 * Valid phase transitions. Used by assertTransition() in dev mode
 * to catch illegal state changes early.
 *
 * Key = source phase, Value = set of allowed target phases.
 * Any transition not listed here will trigger a console.warn in dev.
 */
const VALID_TRANSITIONS: Record<SessionPhase, Set<SessionPhase>> = {
  empty: new Set(["loading", "ready", "spawning"]),
  loading: new Set(["ready", "running", "completed", "failed", "stopped", "empty"]),
  ready: new Set(["spawning", "running", "empty", "loading"]),
  spawning: new Set(["running", "failed", "stopped", "idle", "empty", "loading"]),
  running: new Set(["idle", "completed", "failed", "stopped", "empty", "loading"]),
  idle: new Set(["running", "spawning", "completed", "failed", "stopped", "empty", "loading"]),
  completed: new Set(["empty", "loading", "spawning", "ready"]),
  failed: new Set(["empty", "loading", "spawning", "ready"]),
  stopped: new Set(["empty", "loading", "spawning", "ready"]),
};

/**
 * Dev-mode guard: warns on invalid phase transitions.
 * No-op in production. Silent on identity transitions (from === to).
 */
export function assertTransition(from: SessionPhase, to: SessionPhase): void {
  if (from === to) return;
  const allowed = VALID_TRANSITIONS[from];
  if (!allowed || !allowed.has(to)) {
    console.warn(`[ocv:phase] invalid transition: ${from} → ${to}`, new Error().stack);
  }
}

/** Returns a warning string if the run's error matches context-full patterns, or null if safe to resume. */
export function getResumeWarning(
  run: { error_message?: string; result_subtype?: string } | null,
): string | null {
  if (!run) return null;

  const classified = classifyError(run.result_subtype, run.error_message);

  switch (classified.category) {
    case "context_limit":
      return "This session's context may be too large to resume. Consider using Fork instead, which starts a fresh context.";
    case "budget_limit":
      return "This session hit the budget limit. Resuming will continue spending. Consider adjusting max_budget_usd in settings.";
    case "tool_issue":
      // Special case: structured output retries
      if (run.result_subtype?.toLowerCase() === "error_max_structured_output_retries") {
        return "This session failed due to structured output validation retries. The JSON schema may need adjustment.";
      }
      return null;
    default:
      return null;
  }
}

// ── Error classification ──

export type ErrorCategory =
  | "context_limit"
  | "budget_limit"
  | "auth_issue"
  | "server_issue"
  | "tool_issue"
  | "unknown";

export interface ClassifiedError {
  category: ErrorCategory;
  canRetry: boolean;
  /** Fork is recommended at classification level; UI should also guard on session_id existence. */
  canFork: boolean;
  /** Settings route to link, or empty string. */
  settingsLink: string;
}

/**
 * Classify an error by result_subtype prefix and/or error message text.
 * Uses prefix matching so future CLI subtypes are auto-bucketed.
 */
export function classifyError(subtype?: string, errorMsg?: string): ClassifiedError {
  const s = (subtype ?? "").toLowerCase();

  // Category 1: context / turn limits
  if (s.startsWith("error_input_too_long") || s.startsWith("error_max_turns")) {
    return { category: "context_limit", canRetry: false, canFork: true, settingsLink: "" };
  }

  // Category 2: budget limits
  if (s.startsWith("error_max_budget")) {
    return { category: "budget_limit", canRetry: false, canFork: false, settingsLink: "/settings" };
  }

  // Category 3: auth issues
  if (s.startsWith("error_api_key") || s.startsWith("error_auth")) {
    return { category: "auth_issue", canRetry: false, canFork: false, settingsLink: "/settings" };
  }

  // Category 4: server / transient issues
  if (
    s.startsWith("error_rate_limit") ||
    s.startsWith("error_overloaded") ||
    s.startsWith("error_model") ||
    s.startsWith("error_timeout") ||
    s.startsWith("error_network")
  ) {
    return { category: "server_issue", canRetry: true, canFork: false, settingsLink: "" };
  }

  // Category 5: tool / permission issues
  if (
    s.startsWith("error_permission") ||
    s.startsWith("error_tool") ||
    s.startsWith("error_structured_output")
  ) {
    return { category: "tool_issue", canRetry: false, canFork: false, settingsLink: "" };
  }

  // Subtype is an error_* but doesn't match known categories — unknown
  if (s.startsWith("error_")) {
    return { category: "unknown", canRetry: true, canFork: false, settingsLink: "" };
  }

  // No subtype or non-error subtype: try text matching on errorMsg
  const msg = (errorMsg ?? "").toLowerCase();

  if (
    /input is too long|prompt is too long|too many tokens|context window|max_tokens|token limit/i.test(
      msg,
    )
  ) {
    return { category: "context_limit", canRetry: false, canFork: true, settingsLink: "" };
  }
  if (/budget|max_budget/i.test(msg)) {
    return { category: "budget_limit", canRetry: false, canFork: false, settingsLink: "/settings" };
  }
  if (/api.?key|auth|401|403/i.test(msg)) {
    return { category: "auth_issue", canRetry: false, canFork: false, settingsLink: "/settings" };
  }
  if (/rate.?limit|overloaded|timeout|network|connection|60s/i.test(msg)) {
    return { category: "server_issue", canRetry: true, canFork: false, settingsLink: "" };
  }

  return { category: "unknown", canRetry: true, canFork: false, settingsLink: "" };
}

/** Whether a finished run can be resumed/continued/forked. */
export function canResumeRun(
  run: { session_id?: string; status?: string } | null,
  phase: SessionPhase,
  noSessionPersistence: boolean,
): boolean {
  if (!run?.session_id) return false;
  if (noSessionPersistence) return false;
  if (ACTIVE_PHASES.includes(phase)) return false;
  return TERMINAL_PHASES.includes(phase);
}

//! Transport-agnostic session protocol abstraction.
//!
//! `SessionActor` owns a long-lived child process and a bidirectional pipe. The wire
//! format differs by agent — Claude speaks stream-json + control_request/response, Codex
//! speaks `app-server` JSON-RPC — but the actor's loop (mailbox, timeouts, quarantine,
//! cancel, ralph, stdin ownership) is identical. This trait localizes every wire-format
//! difference so the actor stays protocol-neutral.
//!
//! Implementations: [`crate::agent::codex_appserver::CodexAppServer`] (and, as the refactor
//! lands, a `ClaudeStreamJson` wrapper over the existing `ProtocolState`).

use crate::models::BusEvent;
use serde_json::Value;

/// Context the actor supplies once, right after spawn, so the protocol can build its
/// handshake / session-open messages.
#[derive(Debug, Clone, Default)]
pub struct StartupCtx {
    pub cwd: String,
    /// Resume an existing conversation instead of starting fresh.
    pub resume_thread_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    /// Codex `approval_policy` (e.g. "on-request", "untrusted", "never").
    pub approval_policy: Option<String>,
    /// Codex `sandbox` mode (e.g. "read-only", "workspace-write", "danger-full-access").
    pub sandbox: Option<String>,
    /// Codex `ReasoningEffort` ("minimal" | "low" | "medium" | "high"). `thread/start` has no
    /// effort field, so this is applied on the first `turn/start` (and persists server-side).
    pub effort: Option<String>,
    /// Extra directories the workspace-write sandbox may write to, beyond `cwd`. Mapped into
    /// the `workspaceWrite` policy's `writableRoots` at `thread/start` (empty = cwd-only).
    pub add_dirs: Vec<String>,
}

/// Live per-turn overrides for a Codex `app-server` session. These are injected into the next
/// `turn/start` and, per the protocol, persist "for this turn AND subsequent turns" server-side.
/// Set via the frontend's control subtypes (`set_permission_mode` / `set_model` / `set_effort`)
/// without respawning the process. All `None` = use the spawn-time defaults (no override).
#[derive(Debug, Clone, Default)]
pub struct CodexTurnOverrides {
    /// `AskForApproval` string: "untrusted" | "on-failure" | "on-request" | "never".
    pub approval_policy: Option<String>,
    /// App sandbox mode string ("read-only" | "workspace-write" | "danger-full-access"); the
    /// protocol impl converts it to the tagged `SandboxPolicy` object `turn/start` expects.
    pub sandbox: Option<String>,
    /// Model slug override.
    pub model: Option<String>,
    /// `ReasoningEffort` string ("minimal" | "low" | "medium" | "high").
    pub effort: Option<String>,
}

/// Which interactive surface a pending server request maps to. Determines which
/// frontend response command (and `frame_response` branch) applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// Command / file / permission approval → `respond_permission`.
    Permission,
    /// MCP elicitation → `respond_elicitation`.
    Elicitation,
    /// Multiple-choice `request_user_input` → `respond_user_input`.
    UserInput,
}

/// Bookkeeping for a server-initiated interactive request the actor must track until the
/// user responds. The prompt's `BusEvent`s are already in [`ParsedLine::events`]; this just
/// tells the actor what kind of pending request to register.
#[derive(Debug, Clone)]
pub struct PendingInteractive {
    pub request_id: String,
    pub kind: PendingKind,
}

/// Turn-boundary signal extracted from a wire line, mapped by the actor to `RunState` and
/// used to release quarantine / advance the turn queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleSignal {
    TurnStarted,
    TurnCompleted,
    TurnFailed(Option<String>),
}

/// Result of parsing one line of child stdout.
#[derive(Debug, Default)]
pub struct ParsedLine {
    /// BusEvents to persist + emit (message deltas, tool start/end, usage, and any
    /// interactive prompt events).
    pub events: Vec<BusEvent>,
    /// Set when this line is a server→client interactive request awaiting a user response.
    pub interactive: Option<PendingInteractive>,
    /// Set when this line marks a turn boundary.
    pub lifecycle: Option<LifecycleSignal>,
    /// Set when this line carries the (resume) conversation id to persist.
    pub thread_id: Option<String>,
    /// Set when this line is a reply to a client→server request we registered a waiter for
    /// (Codex `thread/fork`, `thread/rollback`, `thread/goal/get`, …). `(request_id, value)`
    /// where `request_id` is the frontend control request id and `value` is the JSON-RPC
    /// `result` (or `error`). The actor routes it to the matching `control_waiter`.
    pub control_response: Option<(String, Value)>,
}

/// Localizes all wire-format differences between agent transports. The actor calls these at
/// four seams: spawn (startup), user message (frame_user_turn), each stdout line
/// (parse_line), and each interactive response (frame_response) / stop (frame_interrupt).
///
/// Each `frame_*` returns the JSON value(s) to write as newline-delimited lines to stdin.
pub trait SessionProtocol: Send {
    /// Messages to send immediately after spawn, before the first user turn
    /// (e.g. Codex `initialize` + `thread/start`|`thread/resume`).
    fn startup_messages(&mut self, ctx: &StartupCtx) -> Vec<Value>;

    /// Frame a user message into wire line(s) (Codex: `turn/start`). `overrides` carries live
    /// per-turn model/effort/approval/sandbox overrides (Codex only; Claude impls ignore them).
    fn frame_user_turn(
        &mut self,
        text: &str,
        image_paths: &[String],
        overrides: &CodexTurnOverrides,
    ) -> Vec<Value>;

    /// Frame an interrupt/stop (Codex: `turn/interrupt`). Empty = nothing to send.
    fn frame_interrupt(&mut self) -> Vec<Value>;

    /// Frame a mid-turn steer (Codex: `turn/steer`) — inject guidance into the *currently
    /// running* turn without interrupting it. Empty = nothing to send (no active turn / no
    /// thread). Default = unsupported (Claude has no steer; returns empty).
    fn frame_steer(&mut self, _text: &str) -> Vec<Value> {
        vec![]
    }

    /// Parse one stdout line into events + lifecycle/interactive/thread-id signals.
    fn parse_line(&mut self, run_id: &str, line: &str) -> ParsedLine;

    /// Frame the user's response to a pending interactive request (Codex: a JSON-RPC
    /// response on the stored request id). Empty = nothing to send (e.g. unknown id).
    fn frame_response(
        &mut self,
        kind: PendingKind,
        request_id: &str,
        response: Value,
    ) -> Vec<Value>;
}

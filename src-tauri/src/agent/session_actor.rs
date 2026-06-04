//! Session Actor — single owner of a Claude CLI session's entire lifecycle.
//!
//! One actor per run_id. All session mutations (send, control, stop) go through
//! the actor's mailbox (bounded mpsc channel), guaranteeing sequential execution
//! without external locks. The actor owns the process, stdin, stdout/stderr readers,
//! protocol state, and RunState emission — eliminating the cross-system coordination
//! that previously caused race conditions.

use crate::agent::adapter::ActorSessionMap;
use crate::agent::claude_protocol::{validate_bus_event, ProtocolState};
use crate::agent::codex_appserver::CodexAppServer;
use crate::agent::notify::notify_if_background;
use crate::agent::session_protocol::{
    CodexSkillRef, CodexTurnOverrides, LifecycleSignal, PendingKind, SessionProtocol,
};
use crate::agent::turn_engine::{
    apply_activity_reset, ActiveTurn, ContextExtractor, InternalExtractor, InternalJob, TurnOrigin,
    TurnPhase, UserTurnKind, UserTurnTicket, INTERNAL_HARD_TIMEOUT, INTERNAL_SOFT_TIMEOUT,
    QUARANTINE_DEADLINE, TICK_INTERVAL, USER_HARD_TIMEOUT, USER_SOFT_TIMEOUT,
};
use crate::models::{
    max_attachment_size, now_iso, BusEvent, RalphCompleteReason, RunStatus, ALLOWED_DOC_TYPES,
    ALLOWED_IMAGE_TYPES,
};
use crate::storage;
use crate::storage::runs;
use crate::web_server::broadcaster::BroadcastEmitter;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Strip ANSI/CSI escape sequences (e.g. `\x1b[31m`) so colored CLI stderr renders cleanly.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... <final byte in @..~> — consume the whole CSI sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Extract content from `<promise>...</promise>` tag in text.
fn extract_promise_tag(text: &str) -> Option<&str> {
    let start = text.find("<promise>")?;
    let end = text.find("</promise>")?;
    if end <= start + 9 {
        return None;
    }
    Some(text[start + 9..end].trim())
}

/// Truncate a string to at most `max` bytes, snapping to a char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Build a control_response payload for the CLI. HC#2: the `request_id` MUST be nested
/// inside `response`, not at the top level. `Ok` → success with a response body; `Err` →
/// error with a message (the correct reply for a request the app cannot fulfill).
fn build_control_response(request_id: &str, outcome: Result<Value, String>) -> Value {
    let inner = match outcome {
        Ok(response) => serde_json::json!({
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        }),
        Err(error) => serde_json::json!({
            "subtype": "error",
            "request_id": request_id,
            "error": error,
        }),
    };
    serde_json::json!({ "type": "control_response", "response": inner })
}

// ── Ralph Loop types ──

#[derive(Debug, Clone, PartialEq)]
enum RalphPhase {
    Running,
    WaitingRetry,
    PausedByUser { was: Box<RalphPhase> },
    CancelPending,
}

#[allow(dead_code)] // started_at is stored for potential future use
struct RalphLoopState {
    prompt: String,
    phase: RalphPhase,
    iteration: u32,
    max_iterations: u32,
    completion_promise: Option<String>,
    started_at: String,
    consecutive_failures: u32,
    max_consecutive_failures: u32,
    retry_after: Option<Instant>,
    turn_toplevel_texts: Vec<String>,
}

/// Result returned by cancel_ralph_loop IPC command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RalphCancelResult {
    pub iteration: u32,
    pub immediate: bool,
}

/// Tracks a pending interactive control request (permission, hook, elicitation)
/// that was forwarded to the frontend and is waiting for user response.
/// Used for diagnosing hard-timeout causes.
#[derive(Debug)]
struct PendingInteractiveRequest {
    request_id: String,
    /// "can_use_tool" | "hook_callback" | "elicitation"
    subtype: String,
    /// tool_name / hook event / server name
    detail: String,
    received_at: Instant,
}

// ── Public types ──

/// Attachment data for multimodal messages (images, documents).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AttachmentData {
    pub content_base64: String,
    pub media_type: String,
    pub filename: String,
}

/// Commands sent to the actor via its mailbox.
pub enum ActorCommand {
    SendMessage {
        text: String,
        attachments: Vec<AttachmentData>,
        /// Codex skill picks → structured `{type:"skill"}` input items. Empty for Claude and for
        /// Codex turns with no skill selected (no behavior change).
        skills: Vec<CodexSkillRef>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Two-phase control: actor writes stdin + registers waiter → returns (request_id, response_rx).
    /// Caller awaits response_rx outside the actor to avoid deadlocking the select! loop.
    SendControl {
        request: Value,
        reply: oneshot::Sender<Result<(String, oneshot::Receiver<Value>), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Inline permission response: write control_response back to CLI stdin.
    /// Used with `--permission-prompt-tool stdio` (Phase 2).
    RespondPermission {
        request_id: String,
        response: Value,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Cancel a pending control_request (top-level message type, not a control_request subtype).
    CancelControlRequest {
        request_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Hook callback response: write control_response back to CLI stdin.
    RespondHookCallback {
        request_id: String,
        response: Value,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// MCP elicitation response: write control_response back to CLI stdin.
    RespondElicitation {
        request_id: String,
        response: Value,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Codex `request_user_input` (multiple-choice) answer → JSON-RPC response on the
    /// app-server thread. `response` carries `{answers: {qid: [labels]}}`.
    RespondUserInput {
        request_id: String,
        response: Value,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Start a Ralph loop (auto-iterate same prompt until completion).
    StartRalphLoop {
        prompt: String,
        max_iterations: u32,
        completion_promise: Option<String>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Cancel an active Ralph loop.
    CancelRalphLoop {
        reply: oneshot::Sender<Result<RalphCancelResult, String>>,
    },
}

/// External handle held in SessionMap. Provides the channel sender + metadata.
pub struct SessionActorHandle {
    pub cmd_tx: mpsc::Sender<ActorCommand>,
    pub run_id: String,
    /// Identity tag — shared Arc with the actor. cleanup uses Arc::ptr_eq to
    /// verify the map entry is still "us" (not a replacement actor).
    pub tag: Arc<()>,
    pub join_handle: tokio::task::JoinHandle<()>,
    /// Fires when the actor exits (normal or abnormal). Callers can await this
    /// to know when it's safe to spawn a replacement.
    pub shutdown_rx: oneshot::Receiver<()>,
}

// ── Actor internals ──

/// The actor's private state. Runs in a single tokio task.
struct SessionActor {
    emitter: Arc<BroadcastEmitter>,
    sessions: ActorSessionMap,
    run_id: String,
    tag: Arc<()>,
    protocol: ProtocolState,
    /// Codex `app-server` protocol driver. `Some` routes the wire seams (startup, user turn,
    /// stdout parse, response framing) through it; `None` = Claude stream-json (unchanged).
    codex: Option<CodexAppServer>,
    /// Codex handshake messages to write once at run start (initialize + thread/start|resume).
    codex_startup: Vec<Value>,
    /// Codex thread/started seen — gates turn dispatch until the thread is open.
    codex_ready: bool,
    /// Live per-turn Codex overrides (model/effort/approval/sandbox) set via control subtypes
    /// without respawning. Injected into each `turn/start`. Ignored for Claude.
    codex_overrides: CodexTurnOverrides,
    /// Current RunState string — identity dedup: skip emit if unchanged.
    state: String,
    stdin: Option<ChildStdin>,
    child: Option<Child>,
    cancel: CancellationToken,
    pending_interrupt: bool,
    control_waiters: HashMap<String, oneshot::Sender<Value>>,
    shutdown_tx: Option<oneshot::Sender<()>>,

    // ── Turn Transaction Engine fields ──
    /// Current active turn (None when idle).
    active_turn: Option<ActiveTurn>,
    /// Extractor for internal turns (e.g. ContextExtractor).
    active_extractor: Option<Box<dyn InternalExtractor>>,
    /// Queue of pending user messages.
    queued_user: VecDeque<UserTurnTicket>,
    /// Queue of pending internal jobs (auto-context).
    queued_internal: VecDeque<InternalJob>,
    /// Next turn index (all user messages including slash). Starts from resume baseline.
    next_turn_index: u32,
    /// Next auto_ctx_id (Normal user messages only). Starts from resume baseline.
    next_auto_ctx_id: u32,
    /// Monotonically increasing turn seq for ordering.
    next_turn_seq: u64,
    /// Last auto_ctx_id that triggered auto-context (dedup).
    last_auto_context_for: Option<u32>,
    /// Post-turn barrier: forces internal job for this turn_index before next user turn.
    must_run_internal_for_turn: Option<u32>,
    /// Quarantine: freeze dispatch until CLI reports a turn-boundary state.
    quarantine_until_result: bool,
    quarantine_deadline: Option<Instant>,
    interrupt_sent_for_quarantine: bool,
    /// Whether the current quarantine was triggered by an internal turn (auto-context).
    /// If true, quarantine hard-timeout abandons instead of killing the process.
    quarantine_from_internal: bool,
    /// Set after quarantine kill — reject new messages, break run loop.
    terminated: bool,
    /// JSON parse failures in handle_stdout_line (before map_event).
    /// Complements ParserStats.parse_warn_count (field-level malformation).
    json_parse_fail_count: u32,

    // ── Ralph Loop fields ──
    /// Ralph loop state (None = inactive / completed).
    ralph_loop: Option<RalphLoopState>,
    /// Flag set by on_tick_timeout when WaitingRetry expires, consumed by main loop.
    ralph_needs_dispatch: bool,

    // ── Observability: pending interactive request tracking ──
    /// Tracks the most recent interactive control request awaiting user response.
    /// Set when emitting PermissionPrompt / HookCallback(PreToolUse) / ElicitationPrompt.
    /// Cleared when the response is received. Retained during quarantine for diagnostics.
    pending_interactive_request: Option<PendingInteractiveRequest>,
}

// ── Spawn entry point ──

/// Spawn a new session actor. Returns the handle to insert into SessionMap.
///
/// `stdout` and `stderr` are passed as owned values (taken from the Child)
/// so the actor's select! loop can borrow them independently without conflicting
/// with `&mut self`.
///
/// `initial_turn_index` and `initial_auto_ctx_id` are the resume baseline
/// (from `count_user_messages`). For new sessions, pass (0, 0).
#[allow(clippy::too_many_arguments)]
pub fn spawn_actor(
    emitter: Arc<BroadcastEmitter>,
    sessions: ActorSessionMap,
    run_id: String,
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    is_resume: bool,
    cancel: CancellationToken,
    initial_turn_index: u32,
    initial_auto_ctx_id: u32,
    // Codex app-server transport: the driver + its handshake messages. `None`/empty = Claude.
    codex: Option<CodexAppServer>,
    codex_startup: Vec<Value>,
) -> SessionActorHandle {
    let tag = Arc::new(());
    let (cmd_tx, cmd_rx) = mpsc::channel::<ActorCommand>(64);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    log::debug!(
        "[actor] spawn: run_id={}, is_resume={}, initial_turn_index={}, initial_auto_ctx_id={}",
        run_id,
        is_resume,
        initial_turn_index,
        initial_auto_ctx_id
    );

    let actor = SessionActor {
        emitter,
        sessions,
        run_id: run_id.clone(),
        tag: tag.clone(),
        protocol: ProtocolState::new(is_resume),
        codex,
        codex_startup,
        codex_ready: false,
        codex_overrides: CodexTurnOverrides::default(),
        state: String::new(),
        stdin: Some(stdin),
        child: Some(child),
        cancel,
        pending_interrupt: false,
        control_waiters: HashMap::new(),
        shutdown_tx: Some(shutdown_tx),
        // Turn Transaction Engine
        active_turn: None,
        active_extractor: None,
        queued_user: VecDeque::new(),
        queued_internal: VecDeque::new(),
        next_turn_index: initial_turn_index,
        next_auto_ctx_id: initial_auto_ctx_id,
        next_turn_seq: 0,
        last_auto_context_for: None,
        must_run_internal_for_turn: None,
        quarantine_until_result: false,
        quarantine_deadline: None,
        interrupt_sent_for_quarantine: false,
        quarantine_from_internal: false,
        terminated: false,
        json_parse_fail_count: 0,
        ralph_loop: None,
        ralph_needs_dispatch: false,
        pending_interactive_request: None,
    };

    let join_handle = tokio::spawn(async move {
        actor.run(cmd_rx, stdout, stderr).await;
    });

    SessionActorHandle {
        cmd_tx,
        run_id,
        tag,
        join_handle,
        shutdown_rx,
    }
}

// ── Actor main loop ──

impl SessionActor {
    /// Main select! loop. Consumes self.
    async fn run(
        mut self,
        mut cmd_rx: mpsc::Receiver<ActorCommand>,
        stdout: ChildStdout,
        stderr: ChildStderr,
    ) {
        let mut stdout_lines = BufReader::new(stdout).lines();
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut line_count: u64 = 0;
        let mut tick = tokio::time::interval(TICK_INTERVAL);

        log::debug!(
            "[actor] started for run_id={}, is_resume={}",
            self.run_id,
            self.protocol.is_resume()
        );

        // Codex app-server handshake: write initialize + thread/start|resume before the loop.
        // The thread isn't open until thread/started arrives (gates try_dispatch via codex_ready).
        if !self.codex_startup.is_empty() {
            let msgs = std::mem::take(&mut self.codex_startup);
            for msg in &msgs {
                if let Err(e) = self.write_json_line(msg, "codex handshake").await {
                    log::error!("[actor] codex handshake write failed: {}", e);
                }
            }
        }

        loop {
            // HC #18: terminated → break loop
            if self.terminated {
                log::debug!(
                    "[turn] terminated: breaking actor loop for run_id={}",
                    self.run_id
                );
                break;
            }

            tokio::select! {
                // 1. Commands from IPC layer
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(ActorCommand::SendMessage { text, attachments, skills, reply }) => {
                            self.handle_send_message(text, attachments, skills, reply).await;
                        }
                        Some(ActorCommand::Stop { reply }) => {
                            let r = self.handle_stop().await;
                            let _ = reply.send(r);
                            break;
                        }
                        Some(ActorCommand::SendControl { request, reply }) => {
                            let r = self.handle_send_control_async(request).await;
                            let _ = reply.send(r);
                        }
                        Some(ActorCommand::RespondPermission { request_id, response, reply }) => {
                            let r = self.handle_respond_permission(&request_id, response).await;
                            let _ = reply.send(r);
                        }
                        Some(ActorCommand::CancelControlRequest { request_id, reply }) => {
                            self.clear_pending_interactive_request(&request_id);
                            let r = self.handle_cancel_control_request(&request_id).await;
                            let _ = reply.send(r);
                        }
                        Some(ActorCommand::RespondHookCallback { request_id, response, reply }) => {
                            log::debug!("[actor] RespondHookCallback: run_id={}, req_id={}", self.run_id, request_id);
                            self.clear_pending_interactive_request(&request_id);
                            let result = self.write_control_response(&request_id, response).await;
                            let _ = reply.send(result);
                        }
                        Some(ActorCommand::RespondElicitation { request_id, response, reply }) => {
                            log::debug!("[actor] RespondElicitation: run_id={}, req_id={}", self.run_id, request_id);
                            self.clear_pending_interactive_request(&request_id);
                            let result = self.write_interactive_response(PendingKind::Elicitation, &request_id, response).await;
                            let _ = reply.send(result);
                        }
                        Some(ActorCommand::RespondUserInput { request_id, response, reply }) => {
                            log::debug!("[actor] RespondUserInput: run_id={}, req_id={}", self.run_id, request_id);
                            self.clear_pending_interactive_request(&request_id);
                            let result = self.write_interactive_response(PendingKind::UserInput, &request_id, response).await;
                            let _ = reply.send(result);
                        }
                        Some(ActorCommand::StartRalphLoop { prompt, max_iterations, completion_promise, reply }) => {
                            if self.ralph_loop.is_some() {
                                let _ = reply.send(Err("Ralph loop already active".into()));
                            } else {
                                let started_at = crate::models::now_iso();
                                self.ralph_loop = Some(RalphLoopState {
                                    prompt: prompt.clone(),
                                    phase: RalphPhase::Running,
                                    iteration: 0,
                                    max_iterations,
                                    completion_promise: completion_promise.clone(),
                                    started_at: started_at.clone(),
                                    consecutive_failures: 0,
                                    max_consecutive_failures: 3,
                                    retry_after: None,
                                    turn_toplevel_texts: Vec::new(),
                                });
                                self.persist_and_emit(&BusEvent::RalphStarted {
                                    run_id: self.run_id.clone(),
                                    prompt,
                                    max_iterations,
                                    completion_promise,
                                    started_at,
                                });
                                log::info!("[ralph] loop started: run_id={}, max_iterations={}", self.run_id, max_iterations);
                                let _ = reply.send(Ok(()));
                                self.try_dispatch().await;
                            }
                        }
                        Some(ActorCommand::CancelRalphLoop { reply }) => {
                            match &self.ralph_loop {
                                None => {
                                    let _ = reply.send(Err("No active ralph loop".into()));
                                }
                                Some(ralph) => {
                                    let iteration = ralph.iteration;
                                    let has_active_ralph_turn = self
                                        .active_turn
                                        .as_ref()
                                        .map(|t| matches!(t.origin, TurnOrigin::Ralph))
                                        .unwrap_or(false);

                                    if has_active_ralph_turn {
                                        self.ralph_loop.as_mut().unwrap().phase =
                                            RalphPhase::CancelPending;
                                        log::info!("[ralph] cancel pending (active turn running)");
                                        let _ = reply.send(Ok(RalphCancelResult {
                                            iteration,
                                            immediate: false,
                                        }));
                                    } else {
                                        let _ = reply.send(Ok(RalphCancelResult {
                                            iteration,
                                            immediate: true,
                                        }));
                                        self.emit_ralph_complete(RalphCompleteReason::Cancelled);
                                    }
                                }
                            }
                        }
                        None => {
                            // All senders dropped — actor should exit
                            log::debug!("[actor] cmd_rx closed, exiting: run_id={}", self.run_id);
                            break;
                        }
                    }
                }
                // 2. stdout — main event stream from CLI
                result = stdout_lines.next_line() => {
                    match result {
                        Ok(Some(text)) => {
                            line_count += 1;
                            self.handle_stdout_line(&text, line_count).await;
                        }
                        Ok(None) => {
                            log::debug!("[actor] stdout EOF after {} lines: run_id={}", line_count, self.run_id);
                            self.handle_eof().await;
                            break;
                        }
                        Err(e) => {
                            log::debug!("[actor] stdout read error: run_id={}, err={}", self.run_id, e);
                            self.handle_eof().await;
                            break;
                        }
                    }
                }
                // 3. stderr
                result = stderr_lines.next_line() => {
                    match result {
                        Ok(Some(text)) => {
                            self.handle_stderr_line(&text);
                        }
                        Ok(None) | Err(_) => {
                            // stderr EOF is normal — don't break the actor loop for it.
                        }
                    }
                }
                // 4. Independent timeout clock (HC #4)
                _ = tick.tick() => {
                    self.on_tick_timeout().await;
                    // Ralph: dispatch retry after backoff expires
                    if self.ralph_needs_dispatch {
                        self.ralph_needs_dispatch = false;
                        self.try_dispatch().await;
                    }
                }
                // 5. External cancellation (app exit)
                _ = self.cancel.cancelled() => {
                    log::debug!("[actor] cancelled: run_id={}", self.run_id);
                    let _ = self.handle_stop().await;
                    break;
                }
            }
        }

        self.cleanup().await;
    }

    // ── Turn Transaction Engine ──

    /// Enqueue a user message and try to dispatch.
    async fn handle_send_message(
        &mut self,
        text: String,
        attachments: Vec<AttachmentData>,
        skills: Vec<CodexSkillRef>,
        reply: oneshot::Sender<Result<(), String>>,
    ) {
        if self.terminated {
            let _ = reply.send(Err("Session terminated".to_string()));
            return;
        }
        // Barrier: user messages are still enqueued (not rejected).
        // try_dispatch ensures internal queue runs first when barrier is set.
        if self.must_run_internal_for_turn.is_some() {
            log::debug!(
                "[turn] barrier active, user message queued (will dispatch after internal turn)"
            );
        }

        // Allocate turn_index and determine kind
        let trimmed = text.trim();
        let turn_index = self.next_turn_index;
        self.next_turn_index += 1;

        let kind = if trimmed.starts_with('/') {
            UserTurnKind::Slash {
                command: trimmed.to_string(),
            }
        } else {
            let auto_ctx_id = self.next_auto_ctx_id;
            self.next_auto_ctx_id += 1;
            UserTurnKind::Normal { auto_ctx_id }
        };

        let seq = self.next_turn_seq;
        self.next_turn_seq += 1;

        log::debug!(
            "[turn] enqueue user: turn_index={}, kind={:?}, seq={}",
            turn_index,
            kind,
            seq
        );

        self.queued_user.push_back(UserTurnTicket {
            ticket_seq: seq,
            text,
            attachments,
            skills,
            kind,
            turn_index,
            reply,
        });

        self.try_dispatch().await;
    }

    /// Try to dispatch next queued item. HC #1: One turn at a time.
    async fn try_dispatch(&mut self) {
        if self.active_turn.is_some() || self.quarantine_until_result || self.terminated {
            return;
        }
        // Codex: hold dispatch until the app-server thread is open (thread/started seen).
        if self.codex.is_some() && !self.codex_ready {
            return;
        }

        // HC #3: Barrier — try internal queue first when barrier is set
        if let Some(barrier_turn) = self.must_run_internal_for_turn {
            if let Some(pos) = self
                .queued_internal
                .iter()
                .position(|j| j.for_turn_index == barrier_turn)
            {
                let job = self.queued_internal.remove(pos).unwrap();
                self.start_internal_turn(job).await;
                return;
            }
        }

        // Try user queue first (unless barrier blocks). Ralph yields to user messages.
        if self.must_run_internal_for_turn.is_none() {
            if let Some(ticket) = self.queued_user.pop_front() {
                // Pause Ralph if it's active
                if let Some(ref mut ralph) = self.ralph_loop {
                    match &ralph.phase {
                        RalphPhase::Running => {
                            ralph.phase = RalphPhase::PausedByUser {
                                was: Box::new(RalphPhase::Running),
                            };
                            log::debug!("[ralph] paused by user message");
                        }
                        RalphPhase::WaitingRetry => {
                            ralph.phase = RalphPhase::PausedByUser {
                                was: Box::new(RalphPhase::WaitingRetry),
                            };
                            log::debug!("[ralph] paused by user message (was WaitingRetry)");
                        }
                        _ => {} // CancelPending — don't touch
                    }
                }
                self.start_user_turn(ticket).await;
                return;
            }
        }

        // Ralph loop: dispatch ralph prompt when user queue is empty and phase is Running
        if let Some(ref ralph) = self.ralph_loop {
            match ralph.phase {
                RalphPhase::Running => {
                    let prompt = ralph.prompt.clone();
                    self.start_ralph_turn(prompt).await;
                    return;
                }
                RalphPhase::WaitingRetry => {
                    if let Some(deadline) = ralph.retry_after {
                        if Instant::now() >= deadline {
                            // Backoff expired — transition to Running and dispatch
                            self.ralph_loop.as_mut().unwrap().phase = RalphPhase::Running;
                            self.ralph_loop.as_mut().unwrap().retry_after = None;
                            let prompt = self.ralph_loop.as_ref().unwrap().prompt.clone();
                            self.start_ralph_turn(prompt).await;
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        // Try internal queue
        if let Some(job) = self.queued_internal.pop_front() {
            self.start_internal_turn(job).await;
        }
    }

    /// Start a user turn: write to stdin, emit events, set active_turn.
    async fn start_user_turn(&mut self, ticket: UserTurnTicket) {
        log::debug!(
            "[turn] start_user: turn_index={}, kind={:?}, seq={}",
            ticket.turn_index,
            ticket.kind,
            ticket.ticket_seq
        );

        // Track pending slash commands for friendly hint
        match &ticket.kind {
            UserTurnKind::Slash { command } => {
                self.protocol
                    .set_pending_slash_command(Some(command.clone()));
            }
            UserTurnKind::Normal { .. } => {
                self.protocol.set_pending_slash_command(None);
            }
        }

        // Write to stdin
        let user_uuid = match self
            .write_user_to_stdin(&ticket.text, &ticket.attachments, &ticket.skills)
            .await
        {
            Ok(uuid) => uuid,
            Err(e) => {
                log::warn!("[turn] start_user: stdin write failed: {}", e);
                let _ = ticket.reply.send(Err(e));
                return;
            }
        };
        log::debug!("[turn] user_message_uuid={}", user_uuid);

        // Emit UserMessage + RunState(running)
        self.persist_and_emit(&BusEvent::UserMessage {
            run_id: self.run_id.clone(),
            text: ticket.text.clone(),
            uuid: Some(user_uuid),
            client_uuid: None,
            attachments: vec![],
        });
        self.emit_state("running", None, None, false);
        self.persist_idle_running(RunStatus::Running);

        // Reply success to caller
        let _ = ticket.reply.send(Ok(()));

        // Set active turn
        let now = Instant::now();
        self.active_turn = Some(ActiveTurn {
            turn_seq: ticket.ticket_seq,
            origin: TurnOrigin::User(ticket.kind.clone()),
            phase: TurnPhase::Active,
            started_at: now,
            soft_deadline: now + USER_SOFT_TIMEOUT,
            hard_deadline: now + USER_HARD_TIMEOUT,
            turn_index: ticket.turn_index,
        });
    }

    /// Start an internal turn (auto-context): write /context to stdin.
    async fn start_internal_turn(&mut self, job: InternalJob) {
        log::debug!(
            "[turn] start_internal: kind={:?}, for_auto_ctx_id={}, for_turn_index={}",
            job.kind,
            job.for_auto_ctx_id,
            job.for_turn_index
        );

        self.protocol
            .set_pending_slash_command(Some("/context".to_string()));

        if let Err(e) = self.write_user_to_stdin("/context", &[], &[]).await {
            log::warn!("[turn] start_internal: stdin write failed: {}", e);
            self.must_run_internal_for_turn = None;
            self.protocol.set_pending_slash_command(None);
            // Don't recurse into try_dispatch here — next tick will retry.
            return;
        }

        let now = Instant::now();
        let turn_index = job.for_turn_index;
        self.active_turn = Some(ActiveTurn {
            turn_seq: job.job_seq,
            origin: TurnOrigin::Internal(job.kind),
            phase: TurnPhase::Active,
            started_at: now,
            soft_deadline: now + INTERNAL_SOFT_TIMEOUT,
            hard_deadline: now + INTERNAL_HARD_TIMEOUT,
            turn_index,
        });
        self.active_extractor = Some(Box::new(ContextExtractor {
            app: self.emitter.app().clone(),
            run_id: self.run_id.clone(),
            for_turn_index: turn_index,
            captured: false,
        }));
        self.last_auto_context_for = Some(job.for_auto_ctx_id);
        self.must_run_internal_for_turn = None; // Barrier cleared

        log::debug!(
            "[turn] internal turn started, last_auto_context_for={}",
            job.for_auto_ctx_id
        );
    }

    /// End current turn and dispatch next.
    async fn end_turn_and_dispatch(&mut self) {
        if let Some(ref mut ext) = self.active_extractor {
            ext.finalize(false);
        }
        self.active_turn = None;
        self.active_extractor = None;
        self.protocol.set_pending_slash_command(None);
        self.try_dispatch().await;
    }

    /// Called when a user turn reaches idle — enqueue auto-context if applicable. (HC #24)
    /// NOTE: Auto-context is currently disabled because /context hangs CLI
    /// with certain API proxies, causing process kills and SESSION ISSUE errors.
    /// The /context slash command produces zero output and never completes,
    /// leading to hard timeout → quarantine → kill. Re-enable once root cause
    /// (likely proxy incompatibility with /context tokenization) is resolved.
    fn on_user_turn_finished(&mut self, turn: &ActiveTurn) {
        if let TurnOrigin::User(UserTurnKind::Normal { auto_ctx_id }) = &turn.origin {
            let auto_ctx_id = *auto_ctx_id;
            log::debug!(
                "[turn] auto-context skipped (disabled): auto_ctx_id={}",
                auto_ctx_id
            );
            // Update last_auto_context_for to maintain dedup state
            self.last_auto_context_for = Some(auto_ctx_id);

            /* Disabled: /context hangs with some API proxies
            if crate::agent::turn_engine::should_trigger_auto_context(
                auto_ctx_id,
                self.last_auto_context_for,
            ) {
                let seq = self.next_turn_seq;
                self.next_turn_seq += 1;
                self.queued_internal.push_back(InternalJob {
                    job_seq: seq,
                    kind: InternalJobKind::AutoContext,
                    for_auto_ctx_id: auto_ctx_id,
                    for_turn_index: turn.turn_index,
                });
                self.must_run_internal_for_turn = Some(turn.turn_index);
                log::debug!(
                    "[turn] barrier set: must_run_internal_for_turn={}, auto_ctx_id={}",
                    turn.turn_index,
                    auto_ctx_id
                );
            }
            */ // end disabled auto-context
        }
    }

    // ── Ralph Loop methods ──

    /// Start a Ralph loop turn: write prompt to stdin, set active_turn with TurnOrigin::Ralph.
    async fn start_ralph_turn(&mut self, prompt: String) {
        let turn_index = self.next_turn_index;
        self.next_turn_index += 1;
        // Ralph turns don't allocate auto_ctx_id (no auto-context)

        let seq = self.next_turn_seq;
        self.next_turn_seq += 1;

        // Clear per-turn text buffer
        if let Some(ref mut ralph) = self.ralph_loop {
            ralph.turn_toplevel_texts.clear();
        }

        let user_uuid = match self.write_user_to_stdin(&prompt, &[], &[]).await {
            Ok(uuid) => uuid,
            Err(e) => {
                log::error!("[ralph] stdin write failed: {}", e);
                // Compute action to avoid borrow conflict
                let action = if let Some(ref mut ralph) = self.ralph_loop {
                    ralph.consecutive_failures += 1;
                    if ralph.consecutive_failures >= ralph.max_consecutive_failures {
                        Some(RalphCompleteReason::FailStopped)
                    } else {
                        let backoff = Duration::from_secs(2 * ralph.consecutive_failures as u64);
                        ralph.retry_after = Some(Instant::now() + backoff);
                        ralph.phase = RalphPhase::WaitingRetry;
                        None
                    }
                } else {
                    None
                };
                if let Some(reason) = action {
                    self.emit_ralph_complete(reason);
                }
                return;
            }
        };

        self.persist_and_emit(&BusEvent::UserMessage {
            run_id: self.run_id.clone(),
            text: prompt,
            uuid: Some(user_uuid),
            client_uuid: None,
            attachments: vec![],
        });
        self.emit_state("running", None, None, false);
        self.persist_idle_running(RunStatus::Running);

        let now = Instant::now();
        self.active_turn = Some(ActiveTurn {
            turn_seq: seq,
            origin: TurnOrigin::Ralph,
            phase: TurnPhase::Active,
            started_at: now,
            soft_deadline: now + USER_SOFT_TIMEOUT,
            hard_deadline: now + USER_HARD_TIMEOUT,
            turn_index,
        });

        log::debug!(
            "[ralph] turn started: turn_index={}, seq={}, iteration={}",
            turn_index,
            seq,
            self.ralph_loop.as_ref().map(|r| r.iteration).unwrap_or(0)
        );
    }

    /// Emit RalphComplete and clean up ralph_loop. After this, self.ralph_loop == None.
    fn emit_ralph_complete(&mut self, reason: RalphCompleteReason) {
        let iteration = self.ralph_loop.as_ref().map(|r| r.iteration).unwrap_or(0);
        self.ralph_loop = None;
        self.persist_and_emit(&BusEvent::RalphComplete {
            run_id: self.run_id.clone(),
            reason,
            iteration,
        });
        log::info!(
            "[ralph] complete: reason={:?}, iteration={}",
            reason,
            iteration
        );
    }

    /// Ralph state transition on turn end. Uses action-first pattern to avoid borrow conflicts.
    /// `turn_failed`: caller's classification of the just-ended turn (true for error results
    /// or process-level failures, false for clean idle).
    fn ralph_on_turn_end(&mut self, turn: &ActiveTurn, turn_failed: bool) {
        if self.ralph_loop.is_none() {
            return;
        }

        // ── Step 1: compute action (borrows ralph_loop mutably, then drops) ──
        enum RalphAction {
            Complete(RalphCompleteReason),
            EmitIteration { iteration: u32, max_iterations: u32 },
            SetWaitingRetry { backoff: Duration },
            ResumeFrom(RalphPhase),
            Noop,
        }

        let action = {
            let ralph = self.ralph_loop.as_mut().unwrap();

            match turn.origin {
                TurnOrigin::Ralph => {
                    let is_cancel_pending = ralph.phase == RalphPhase::CancelPending;

                    if turn_failed {
                        if is_cancel_pending {
                            RalphAction::Complete(RalphCompleteReason::Cancelled)
                        } else {
                            ralph.consecutive_failures += 1;
                            if ralph.consecutive_failures >= ralph.max_consecutive_failures {
                                RalphAction::Complete(RalphCompleteReason::FailStopped)
                            } else {
                                let backoff =
                                    Duration::from_secs(2 * ralph.consecutive_failures as u64);
                                RalphAction::SetWaitingRetry { backoff }
                            }
                        }
                    } else {
                        // idle — process turn result normally
                        ralph.consecutive_failures = 0;
                        ralph.iteration += 1;

                        // Check natural completion conditions first
                        let natural_reason = if ralph.max_iterations > 0
                            && ralph.iteration >= ralph.max_iterations
                        {
                            Some(RalphCompleteReason::MaxIterations)
                        } else if let Some(ref promise) = ralph.completion_promise {
                            let matched = ralph.turn_toplevel_texts.iter().any(|text| {
                                extract_promise_tag(text)
                                    .map(|found| found == promise.as_str())
                                    .unwrap_or(false)
                            });
                            if matched {
                                Some(RalphCompleteReason::CompletionPromise)
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(reason) = natural_reason {
                            RalphAction::Complete(reason)
                        } else if is_cancel_pending {
                            RalphAction::Complete(RalphCompleteReason::Cancelled)
                        } else {
                            RalphAction::EmitIteration {
                                iteration: ralph.iteration,
                                max_iterations: ralph.max_iterations,
                            }
                        }
                    }
                }
                TurnOrigin::User(_) => {
                    if let RalphPhase::PausedByUser { ref was } = ralph.phase {
                        RalphAction::ResumeFrom(*was.clone())
                    } else {
                        RalphAction::Noop
                    }
                }
                _ => RalphAction::Noop,
            }
        };
        // ← ralph_loop borrow ends here

        // ── Step 2: execute action ──
        match action {
            RalphAction::Complete(reason) => {
                self.emit_ralph_complete(reason);
            }
            RalphAction::EmitIteration {
                iteration,
                max_iterations,
            } => {
                self.persist_and_emit(&BusEvent::RalphIteration {
                    run_id: self.run_id.clone(),
                    iteration,
                    max_iterations,
                });
            }
            RalphAction::SetWaitingRetry { backoff } => {
                if let Some(ref mut ralph) = self.ralph_loop {
                    ralph.phase = RalphPhase::WaitingRetry;
                    ralph.retry_after = Some(Instant::now() + backoff);
                    log::warn!(
                        "[ralph] turn failed ({}/{}), backing off {:?}",
                        ralph.consecutive_failures,
                        ralph.max_consecutive_failures,
                        backoff
                    );
                }
            }
            RalphAction::ResumeFrom(phase) => {
                if let Some(ref mut ralph) = self.ralph_loop {
                    ralph.phase = phase;
                    log::debug!("[ralph] resumed to {:?} after user turn", ralph.phase);
                }
            }
            RalphAction::Noop => {}
        }
    }

    /// Independent timeout clock — checks soft/hard deadlines and quarantine. (HC #4)
    async fn on_tick_timeout(&mut self) {
        // Check quarantine deadline first
        if self.quarantine_until_result {
            if let Some(deadline) = self.quarantine_deadline {
                if Instant::now() >= deadline {
                    // Quarantine secondary timeout → hard-kill
                    log::warn!(
                        "[turn] quarantine hard-timeout: run_id={}, from_internal={}, pending_request={:?}",
                        self.run_id,
                        self.quarantine_from_internal,
                        self.pending_interactive_request.as_ref().map(|r| (&r.subtype, &r.detail, r.received_at.elapsed().as_secs()))
                    );
                    self.protocol.set_pending_slash_command(None);
                    if let Some(ref mut child) = self.child {
                        let _ = child.kill().await;
                    }
                    let error_msg = if self.quarantine_from_internal {
                        "Auto-context hard timeout — process killed".to_string()
                    } else if let Some(ref req) = self.pending_interactive_request {
                        let wait_secs = req.received_at.elapsed().as_secs();
                        format!(
                            "Session timeout — waited {}s for {} response ({}). Process killed.",
                            wait_secs, req.subtype, req.detail
                        )
                    } else {
                        "Session timeout — no output from CLI for 30 minutes. Process killed."
                            .to_string()
                    };
                    self.emit_state("failed", None, Some(error_msg), true);
                    self.fail_all_pending_replies("Session hard timeout");
                    self.terminated = true;
                    return;
                }
            }
            // If quarantine but no deadline yet, and we haven't sent interrupt, send it now
            if !self.interrupt_sent_for_quarantine {
                self.send_interrupt_to_cli().await;
                self.interrupt_sent_for_quarantine = true;
                self.quarantine_deadline = Some(Instant::now() + QUARANTINE_DEADLINE);
                log::debug!(
                    "[turn] quarantine: interrupt sent, deadline set for run_id={}",
                    self.run_id
                );
            }
            return;
        }

        let Some(ref turn) = self.active_turn else {
            // No active turn — check Ralph WaitingRetry backoff expiry
            if let Some(ref ralph) = self.ralph_loop {
                if ralph.phase == RalphPhase::WaitingRetry {
                    if let Some(deadline) = ralph.retry_after {
                        if Instant::now() >= deadline {
                            log::debug!("[ralph] backoff expired, setting dispatch flag");
                            self.ralph_needs_dispatch = true;
                        }
                    }
                }
            }
            return;
        };
        let now = Instant::now();

        // Internal turn timeout checks
        if matches!(turn.origin, TurnOrigin::Internal(_)) {
            if now >= turn.hard_deadline {
                // Enter quarantine
                log::warn!(
                    "[turn] internal hard timeout: entering quarantine for run_id={} (turn_seq={})",
                    self.run_id,
                    turn.turn_seq
                );
                // HC #17: Clear pending_slash at quarantine entry
                self.protocol.set_pending_slash_command(None);
                if let Some(ref mut ext) = self.active_extractor {
                    ext.finalize(true);
                }
                self.active_extractor = None;
                self.active_turn = None;
                self.quarantine_until_result = true;
                self.interrupt_sent_for_quarantine = false;
                self.quarantine_deadline = None;
                self.quarantine_from_internal = true;
                // on_tick_timeout will send interrupt on next tick
            } else if now >= turn.soft_deadline && matches!(turn.phase, TurnPhase::Active) {
                // Transition to Draining
                log::debug!(
                    "[turn] internal soft timeout: draining for run_id={} (turn_seq={})",
                    self.run_id,
                    turn.turn_seq
                );
                if let Some(ref mut ext) = self.active_extractor {
                    ext.finalize(true);
                }
                // Mutate phase via raw pointer to self.active_turn
                if let Some(ref mut t) = self.active_turn {
                    t.phase = TurnPhase::Draining;
                }
            }
        }
        // User turns: typically don't time out (CLI manages its own flow)
        // but hard_deadline provides a safety net
        else if now >= turn.hard_deadline {
            log::warn!(
                "[turn] user hard timeout: entering quarantine for run_id={} (turn_seq={}), pending_request={:?}",
                self.run_id,
                turn.turn_seq,
                self.pending_interactive_request.as_ref().map(|r| (&r.subtype, &r.detail, r.received_at.elapsed().as_secs()))
            );
            self.protocol.set_pending_slash_command(None);
            self.active_turn = None;
            self.quarantine_until_result = true;
            self.interrupt_sent_for_quarantine = false;
            self.quarantine_deadline = None;
            self.quarantine_from_internal = false;
        }
    }

    /// Write a user-format message to CLI stdin. Returns the UUID embedded in the payload.
    async fn write_user_to_stdin(
        &mut self,
        text: &str,
        attachments: &[AttachmentData],
        skills: &[CodexSkillRef],
    ) -> Result<String, String> {
        // Codex app-server: frame the user message as turn/start instead of stream-json.
        // Codex takes attachments as local file *paths* (not base64 blocks like Claude):
        // images become `localImage` input items (real vision input), and every attachment
        // is also listed in a text breadcrumb so Codex can `Read` non-image files. This
        // mirrors the exec path (chat.rs / spawn.rs `--image=`).
        if self.codex.is_some() {
            let mut image_paths: Vec<String> = Vec::new();
            let mut breadcrumb_files: Vec<String> = Vec::new();
            for att in attachments {
                let Some(path) = save_attachment_to_disk(&self.run_id, att) else {
                    continue;
                };
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if att.media_type.starts_with("image/") {
                    image_paths.push(path.clone());
                }
                breadcrumb_files.push(format!(
                    "- {} ({}, {} bytes) => {}",
                    att.filename, att.media_type, size, path
                ));
            }
            let augmented = if breadcrumb_files.is_empty() {
                text.to_string()
            } else {
                format!(
                    "{}\n\nAttached files:\n{}\nUse these local file paths directly when needed.",
                    text,
                    breadcrumb_files.join("\n")
                )
            };
            if !breadcrumb_files.is_empty() {
                log::debug!(
                    "[codex] app-server turn with {} attachment(s), {} image(s)",
                    breadcrumb_files.len(),
                    image_paths.len()
                );
            }
            let overrides = self.codex_overrides.clone();
            let Some(codex) = self.codex.as_mut() else {
                return Err("codex driver missing".to_string());
            };
            let lines = codex.frame_user_turn(&augmented, &image_paths, skills, &overrides);
            for line in &lines {
                self.write_json_line(line, "codex turn/start").await?;
            }
            return Ok(uuid::Uuid::new_v4().to_string());
        }

        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;
        let (payload, user_uuid) = build_user_payload(text, attachments, &self.run_id);
        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        log::debug!(
            "[turn] write_user_to_stdin: run_id={}, len={}, attachments={}, uuid={}",
            self.run_id,
            text.len(),
            attachments.len(),
            user_uuid
        );
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("stdin write failed: {}", e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush failed: {}", e))?;
        Ok(user_uuid)
    }

    /// Persist a BusEvent to JSONL, emit to Tauri webview, and broadcast to WS clients. (HC #32)
    fn persist_and_emit(&self, event: &BusEvent) {
        self.emitter.persist_and_emit(&self.run_id, event);
    }

    /// Fail all pending user reply channels. (HC #12)
    fn fail_all_pending_replies(&mut self, reason: &str) {
        let count = self.queued_user.len();
        while let Some(ticket) = self.queued_user.pop_front() {
            let _ = ticket.reply.send(Err(reason.to_string()));
        }
        self.queued_internal.clear();
        self.must_run_internal_for_turn = None;
        if count > 0 {
            log::debug!(
                "[turn] fail_all_pending_replies: failed {} tickets, reason={}",
                count,
                reason
            );
        }
    }

    /// Send interrupt control request to CLI for quarantine recovery. (HC #15)
    async fn send_interrupt_to_cli(&mut self) {
        let request_id = format!("ocv_qint_{}", uuid::Uuid::new_v4());
        let payload = serde_json::json!({
            "type": "control_request",
            "request_id": &request_id,
            "request": {
                "subtype": "interrupt"
            },
        });

        if let Some(stdin) = self.stdin.as_mut() {
            let Ok(mut line) = serde_json::to_string(&payload) else {
                return;
            };
            line.push('\n');
            match stdin.write_all(line.as_bytes()).await {
                Ok(_) => {
                    let _ = stdin.flush().await;
                    log::debug!(
                        "[turn] quarantine interrupt sent: req_id={}, run_id={}",
                        request_id,
                        self.run_id
                    );
                }
                Err(e) => {
                    log::warn!("[turn] quarantine interrupt write failed: {}", e);
                }
            }
        }
    }

    /// Check if current turn is internal.
    fn is_internal_turn(&self) -> bool {
        self.active_turn
            .as_ref()
            .map(|t| matches!(t.origin, TurnOrigin::Internal(_)))
            .unwrap_or(false)
    }

    // ── Command handlers ──

    /// Write control request to stdin + register response waiter.
    /// Returns (request_id, response_rx) — caller awaits response_rx outside the actor.
    async fn handle_send_control_async(
        &mut self,
        request: Value,
    ) -> Result<(String, oneshot::Receiver<Value>), String> {
        let request_id = format!("ocv_ctrl_{}", uuid::Uuid::new_v4());
        let subtype = request
            .get("subtype")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        log::debug!(
            "[actor] send_control: run_id={}, subtype={}, req_id={}",
            self.run_id,
            subtype,
            request_id
        );

        // Codex app-server has no stream-json control protocol. Interpret the control subtypes
        // locally: set_* mutate the stored per-turn overrides (applied on the next turn/start);
        // interrupt/steer write a JSON-RPC frame to the app-server now. We resolve the control
        // waiter synchronously here so the IPC caller's `send_session_control` returns Ok without
        // a Claude `control_request` ever going to the wire.
        if self.codex.is_some() {
            return self
                .handle_codex_control(&subtype, &request, request_id)
                .await;
        }

        if subtype == "interrupt" {
            self.pending_interrupt = true;
            log::debug!("[actor] pending_interrupt set for run_id={}", self.run_id);
        }

        let payload = serde_json::json!({
            "type": "control_request",
            "request_id": &request_id,
            "request": request,
        });

        let (tx, rx) = oneshot::channel();
        self.control_waiters.insert(request_id.clone(), tx);

        self.write_json_line(&payload, "control request").await?;

        Ok((request_id, rx))
    }

    /// Codex-only control handler. Interprets the frontend's `sendSessionControl` subtypes
    /// against the bidirectional app-server. Two response shapes:
    ///   - Fire-and-forget (`set_*`, `interrupt`, `steer`, `compact`, `goal_set`, `goal_clear`):
    ///     the waiter is resolved synchronously with `{ok:true}` after the frame is written.
    ///   - Data-returning (`rollback`, `fork`, `goal_get`): the waiter is REGISTERED in
    ///     `control_waiters` keyed by `request_id` and the frame method maps the JSON-RPC id back
    ///     to that `request_id` (`client_waiters`). `parse_line` later routes the server reply via
    ///     `control_response`, which `handle_codex_line` forwards to the registered waiter. These
    ///     subtypes return WITHOUT sending `{ok:true}` — the wire reply is the resolution.
    async fn handle_codex_control(
        &mut self,
        subtype: &str,
        request: &Value,
        request_id: String,
    ) -> Result<(String, oneshot::Receiver<Value>), String> {
        let (tx, rx) = oneshot::channel();

        // Data-returning subtypes: register the waiter, write the tracked frame, and return the
        // rx WITHOUT resolving — parse_line routes the server reply back to this waiter.
        match subtype {
            "rollback" => {
                let num_turns = request
                    .get("num_turns")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1);
                let lines = self
                    .codex
                    .as_mut()
                    .unwrap()
                    .frame_rollback(&request_id, num_turns);
                if lines.is_empty() {
                    // No open thread → nothing to roll back; resolve so the caller isn't stuck.
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex thread/rollback").await?;
                }
                return Ok((request_id, rx));
            }
            "fork" => {
                let lines = self.codex.as_mut().unwrap().frame_fork(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex thread/fork").await?;
                }
                return Ok((request_id, rx));
            }
            "goal_get" => {
                let lines = self.codex.as_mut().unwrap().frame_goal_get(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex thread/goal/get").await?;
                }
                return Ok((request_id, rx));
            }
            // Codex MCP runtime status: reuse the shared "mcp_status" subtype (McpStatusPanel)
            // but route it to mcpServerStatus/list. Reply shape differs from Claude's mcp_status
            // ({data: McpServerStatus[]}); the frontend normalizes both.
            "mcp_status" => {
                let lines = self.codex.as_mut().unwrap().frame_mcp_status(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex mcpServerStatus/list")
                        .await?;
                }
                return Ok((request_id, rx));
            }
            "skills_list" => {
                let lines = self.codex.as_mut().unwrap().frame_skills_list(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex skills/list").await?;
                }
                return Ok((request_id, rx));
            }
            "experimental_feature_list" => {
                let lines = self
                    .codex
                    .as_mut()
                    .unwrap()
                    .frame_experimental_feature_list(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex experimentalFeature/list")
                        .await?;
                }
                return Ok((request_id, rx));
            }
            "model_list" => {
                let lines = self.codex.as_mut().unwrap().frame_model_list(&request_id);
                if lines.is_empty() {
                    let _ = tx.send(serde_json::json!({ "ok": false, "error": "no thread" }));
                    return Ok((request_id, rx));
                }
                self.control_waiters.insert(request_id.clone(), tx);
                for line in &lines {
                    self.write_json_line(line, "codex model/list").await?;
                }
                return Ok((request_id, rx));
            }
            _ => {}
        }

        match subtype {
            "interrupt" => {
                // Native stop: turn/interrupt halts the running turn in place (no process kill,
                // no respawn). The session stays alive for the next message.
                let lines = self.codex.as_mut().unwrap().frame_interrupt();
                if lines.is_empty() {
                    log::debug!(
                        "[actor] codex interrupt: no active turn/thread, nothing to send (run_id={})",
                        self.run_id
                    );
                }
                for line in &lines {
                    self.write_json_line(line, "codex turn/interrupt").await?;
                }
            }
            "set_permission_mode" => {
                // {mode} → Codex AskForApproval string + sandbox mode string. Normalize the app
                // mode through map_permission_mode first so both app names ("plan", "auto_all")
                // and CLI names ("bypassPermissions") map consistently. Stored, applied on the
                // next turn/start (which converts the sandbox string to a SandboxPolicy object).
                if let Some(mode) = request.get("mode").and_then(|v| v.as_str()) {
                    let cli_mode = crate::agent::adapter::map_permission_mode(mode);
                    self.codex_overrides.approval_policy = Some(
                        crate::commands::session::codex_approval_for(Some(&cli_mode)),
                    );
                    self.codex_overrides.sandbox =
                        Some(crate::commands::session::codex_sandbox_for(Some(&cli_mode)));
                    log::debug!(
                        "[actor] codex override set_permission_mode: mode={} → approval={:?} sandbox={:?}",
                        mode,
                        self.codex_overrides.approval_policy,
                        self.codex_overrides.sandbox
                    );
                }
            }
            "set_model" => {
                if let Some(model) = request.get("model").and_then(|v| v.as_str()) {
                    self.codex_overrides.model = Some(model.to_string());
                    log::debug!("[actor] codex override set_model: {}", model);
                }
            }
            "set_effort" => {
                if let Some(effort) = request.get("effort").and_then(|v| v.as_str()) {
                    self.codex_overrides.effort = Some(effort.to_string());
                    log::debug!("[actor] codex override set_effort: {}", effort);
                }
            }
            "steer" => {
                // Mid-turn steer: inject guidance into the currently-running turn. Drops silently
                // if there's no active turn (frame_steer returns empty + logs).
                if let Some(text) = request.get("text").and_then(|v| v.as_str()) {
                    let lines = self.codex.as_mut().unwrap().frame_steer(text);
                    if lines.is_empty() {
                        log::debug!(
                            "[actor] codex steer: no active turn, nothing to send (run_id={})",
                            self.run_id
                        );
                    }
                    for line in &lines {
                        self.write_json_line(line, "codex turn/steer").await?;
                    }
                }
            }
            "compact" => {
                // thread/compact/start returns an empty {} ack; the compaction itself surfaces
                // later via the thread/compacted notification. Fire-and-forget {ok:true}.
                let lines = self.codex.as_mut().unwrap().frame_compact(&request_id);
                if lines.is_empty() {
                    log::debug!(
                        "[actor] codex compact: no thread, nothing to send (run_id={})",
                        self.run_id
                    );
                }
                for line in &lines {
                    self.write_json_line(line, "codex thread/compact/start")
                        .await?;
                }
            }
            "goal_set" => {
                // {objective?, status?, token_budget?} → thread/goal/set. The authoritative goal
                // arrives via thread/goal/updated (→ GoalUpdate), so fire-and-forget {ok:true}.
                let objective = request.get("objective").and_then(|v| v.as_str());
                let status = request.get("status").and_then(|v| v.as_str());
                let token_budget = request.get("token_budget").and_then(|v| v.as_u64());
                let lines = self.codex.as_mut().unwrap().frame_goal_set(
                    &request_id,
                    objective,
                    status,
                    token_budget,
                );
                if lines.is_empty() {
                    log::debug!(
                        "[actor] codex goal_set: no thread, nothing to send (run_id={})",
                        self.run_id
                    );
                }
                for line in &lines {
                    self.write_json_line(line, "codex thread/goal/set").await?;
                }
            }
            "goal_clear" => {
                let lines = self.codex.as_mut().unwrap().frame_goal_clear(&request_id);
                if lines.is_empty() {
                    log::debug!(
                        "[actor] codex goal_clear: no thread, nothing to send (run_id={})",
                        self.run_id
                    );
                }
                for line in &lines {
                    self.write_json_line(line, "codex thread/goal/clear")
                        .await?;
                }
            }
            other => {
                log::debug!(
                    "[actor] codex control subtype '{}' not supported — acking (run_id={})",
                    other,
                    self.run_id
                );
            }
        }

        // Resolve the waiter immediately — these are local, no wire round-trip to await.
        let _ = tx.send(serde_json::json!({ "ok": true }));
        Ok((request_id, rx))
    }

    async fn handle_stop(&mut self) -> Result<(), String> {
        log::debug!("[actor] handle_stop: run_id={}", self.run_id);

        // Drop stdin to signal EOF to CLI
        self.stdin.take();

        // Kill process
        if let Some(ref mut child) = self.child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        Ok(())
    }

    /// Write control_response for a permission prompt back to CLI stdin.
    async fn handle_respond_permission(
        &mut self,
        request_id: &str,
        response: Value,
    ) -> Result<(), String> {
        log::debug!(
            "[actor] respond_permission: run_id={}, req_id={}",
            self.run_id,
            request_id,
        );
        self.clear_pending_interactive_request(request_id);
        self.write_interactive_response(PendingKind::Permission, request_id, response)
            .await
    }

    /// Write a response to a pending interactive request. Codex frames it as a JSON-RPC
    /// response via the protocol driver; Claude uses the stream-json control_response.
    async fn write_interactive_response(
        &mut self,
        kind: PendingKind,
        request_id: &str,
        response: Value,
    ) -> Result<(), String> {
        let Some(codex) = self.codex.as_mut() else {
            return self.write_control_response(request_id, response).await;
        };
        let lines = codex.frame_response(kind, request_id, response);
        for line in &lines {
            self.write_json_line(line, "codex interactive response")
                .await?;
        }
        Ok(())
    }

    /// Clear pending interactive request if it matches the given request_id.
    fn clear_pending_interactive_request(&mut self, request_id: &str) {
        if let Some(ref req) = self.pending_interactive_request {
            if req.request_id == request_id {
                log::debug!(
                    "[actor] clearing pending_interactive_request: subtype={}, detail={}, waited={}s",
                    req.subtype,
                    req.detail,
                    req.received_at.elapsed().as_secs()
                );
                self.pending_interactive_request = None;
            }
        }
    }

    /// Send a control_cancel_request to CLI stdin (top-level message type).
    /// Used only for explicit user-initiated cancellation (ActorCommand::CancelControlRequest,
    /// e.g. the user dismisses a permission prompt). NOT for auto-declining unsupported
    /// requests — those use write_control_response_error (the CLI waits for a response).
    async fn handle_cancel_control_request(&mut self, request_id: &str) -> Result<(), String> {
        let payload = serde_json::json!({
            "type": "control_cancel_request",
            "request_id": request_id,
        });
        log::debug!(
            "[actor] cancel_control_request: run_id={}, req_id={}",
            self.run_id,
            request_id,
        );
        self.write_json_line(&payload, "cancel control request")
            .await
    }

    /// Low-level helper: serialize JSON payload, write to stdin, flush.
    async fn write_json_line(&mut self, payload: &Value, context: &str) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;
        let mut line = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("{} write failed: {}", context, e))?;
        if let Err(e) = stdin.flush().await {
            log::warn!(
                "[actor] stdin flush failed for run_id={}: {}",
                self.run_id,
                e
            );
        }
        Ok(())
    }

    /// Shared helper: write a success control_response JSON to CLI stdin.
    async fn write_control_response(
        &mut self,
        request_id: &str,
        response: Value,
    ) -> Result<(), String> {
        let payload = build_control_response(request_id, Ok(response));
        log::debug!(
            "[actor] write_control_response: run_id={}, req_id={}",
            self.run_id,
            request_id,
        );
        self.write_json_line(&payload, "control response").await
    }

    /// Write an error control_response to CLI stdin — the protocol-correct reply for a
    /// control_request the app cannot fulfill (HC#2). Sending control_cancel_request here
    /// instead would leave the CLI waiting for a response until USER_HARD_TIMEOUT.
    async fn write_control_response_error(
        &mut self,
        request_id: &str,
        error: &str,
    ) -> Result<(), String> {
        let payload = build_control_response(request_id, Err(error.to_string()));
        log::debug!(
            "[actor] write_control_response_error: run_id={}, req_id={}, error={}",
            self.run_id,
            request_id,
            error,
        );
        self.write_json_line(&payload, "control response (error)")
            .await
    }

    // ── I/O handlers ──

    /// Handle a stdout line from CLI — three-way routing: quarantine → control → map events.
    /// Codex `app-server` stdout handling. Parses one JSON-RPC line via the protocol driver
    /// and routes its events / lifecycle / interactive / thread-id signals into the *shared*
    /// turn engine (`end_turn_and_dispatch`, `emit_state`) — no Claude control protocol.
    async fn handle_codex_line(&mut self, text: &str) {
        apply_activity_reset(self.quarantine_until_result, &mut self.active_turn);

        let parsed = self.codex.as_mut().unwrap().parse_line(&self.run_id, text);

        // Route a data-returning request reply (thread/fork, thread/rollback, thread/goal/get)
        // back to the waiting control caller — mirrors the Claude control_response path.
        if let Some((rid, resp)) = parsed.control_response {
            log::debug!("[codex] control_response for req_id={}", rid);
            if let Some(tx) = self.control_waiters.remove(&rid) {
                let _ = tx.send(resp);
            }
        }

        // Persist the thread id (resume key) as soon as it's known.
        if let Some(tid) = parsed.thread_id {
            let rid = self.run_id.clone();
            if let Err(e) = crate::storage::runs::with_meta(&rid, |meta| {
                meta.conversation_ref =
                    Some(crate::models::ConversationRef::CodexThread(tid.clone()));
                Ok(())
            }) {
                log::warn!("[codex] failed to persist conversation_ref: {}", e);
            } else {
                log::debug!(
                    "[codex] captured thread_id as conversation_ref for run_id={}",
                    rid
                );
            }
        }

        // Open the dispatch gate once the thread is ready (new thread/started or resume ack),
        // then flush the queued initial/user message.
        if !self.codex_ready && self.codex.as_ref().map(|c| c.is_ready()).unwrap_or(false) {
            self.codex_ready = true;
            log::debug!(
                "[codex] thread ready for run_id={}; dispatching queue",
                self.run_id
            );
            self.try_dispatch().await;
        }

        // Emit mapped events (message/tool/usage + any interactive prompt events).
        for event in &parsed.events {
            if let Some(warn) = validate_bus_event(event) {
                log::warn!(
                    "[actor] invalid codex event dropped: {}.{}: {}",
                    warn.event_type,
                    warn.field,
                    warn.detail
                );
                continue;
            }
            self.persist_and_emit(event);
        }

        // Track a pending interactive request (observability + desktop notification).
        if let Some(pi) = parsed.interactive {
            let subtype = match pi.kind {
                PendingKind::Permission => "can_use_tool",
                PendingKind::Elicitation => "elicitation",
                PendingKind::UserInput => "request_user_input",
            };
            self.pending_interactive_request = Some(PendingInteractiveRequest {
                request_id: pi.request_id,
                subtype: subtype.to_string(),
                detail: String::new(),
                received_at: Instant::now(),
            });
            notify_if_background(self.emitter.app(), "Codex", "needs your input");
        }

        // Turn lifecycle → RunState + advance the shared turn queue.
        if let Some(sig) = parsed.lifecycle {
            match sig {
                LifecycleSignal::TurnStarted => {
                    self.emit_state("running", None, None, false);
                }
                LifecycleSignal::TurnCompleted => {
                    self.emit_state("idle", Some(0), None, true);
                    self.persist_idle_running(RunStatus::Idle);
                    self.end_turn_and_dispatch().await;
                }
                LifecycleSignal::TurnFailed(err) => {
                    // Codex turn failure keeps the session alive (input box returns).
                    self.emit_state("idle", None, err, true);
                    self.persist_idle_running(RunStatus::Idle);
                    self.end_turn_and_dispatch().await;
                }
            }
        }
    }

    async fn handle_stdout_line(&mut self, text: &str, line_num: u64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        log::trace!("[actor] stdout #{}: {}", line_num, truncate_str(text, 200));

        // Codex app-server transport: own parse + lifecycle handling (bypasses the
        // stream-json control protocol path entirely; Claude code below is untouched).
        if self.codex.is_some() {
            self.handle_codex_line(text).await;
            return;
        }

        // Step 0: JSON parse
        let parsed = match serde_json::from_str::<Value>(text) {
            Ok(v) => v,
            Err(_) => {
                self.json_parse_fail_count += 1;
                log::debug!(
                    "[actor] JSON parse failure #{}: {}",
                    self.json_parse_fail_count,
                    truncate_str(text, 100)
                );
                // HC #16: parse failure during quarantine → swallow
                if self.quarantine_until_result {
                    log::trace!("[turn] quarantine: swallowed parse-fail line");
                    return;
                }
                // Internal turn → swallow
                if self.is_internal_turn() {
                    return;
                }
                // User turn or idle → emit Raw
                self.persist_and_emit(&BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: "claude_stdout_text".to_string(),
                    data: Value::String(text.to_string()),
                });
                return;
            }
        };

        // Activity-based deadline reset for user/ralph turns.
        if apply_activity_reset(self.quarantine_until_result, &mut self.active_turn) {
            log::trace!(
                "[turn] activity reset: hard_deadline extended for run_id={}",
                self.run_id
            );
        }

        let event_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_control = event_type == "control_response"
            || event_type == "control_cancel_request"
            || event_type == "control_request";

        // Step 1: Quarantine routing (HC #16)
        if self.quarantine_until_result {
            if is_control {
                // HC #33: control events during quarantine → internal handling
                self.handle_control_event_internal(&parsed, event_type)
                    .await;
                return;
            }
            // Map events, check for turn-boundary state
            let events = self.protocol.map_event(&self.run_id, &parsed);
            for event in &events {
                // Validate — RunState always passes through (returns None)
                if let Some(warn) = validate_bus_event(event) {
                    log::warn!(
                        "[actor] invalid event dropped (quarantine): {}.{}: {}",
                        warn.event_type,
                        warn.field,
                        warn.detail
                    );
                    self.protocol.stats.invalid_tool_count += 1;
                    continue;
                }
                if let BusEvent::RunState { state, .. } = event {
                    // HC #17: Only lift on turn-boundary states
                    if state == "idle" || state == "failed" {
                        log::debug!(
                            "[turn] quarantine lifted: state={}, run_id={}",
                            state,
                            self.run_id
                        );
                        self.quarantine_until_result = false;
                        self.quarantine_deadline = None;
                        self.interrupt_sent_for_quarantine = false;
                        self.quarantine_from_internal = false;
                        self.protocol.set_pending_slash_command(None);
                        // Don't emit quarantine RunState to frontend (it was an internal turn)
                        // Just try to dispatch next queued item
                        self.try_dispatch().await;
                        return;
                    }
                }
            }
            // Everything else during quarantine → swallow
            log::trace!("[turn] quarantine: swallowed event type={}", event_type);
            return;
        }

        // Step 2: Control event routing (HC #26, #33)
        if is_control {
            if self.is_internal_turn() {
                self.handle_control_event_internal(&parsed, event_type)
                    .await;
            } else {
                self.handle_control_event(&parsed, event_type).await;
            }
            return;
        }

        // Step 3: Map events via protocol
        let events = self.protocol.map_event(&self.run_id, &parsed);
        log::trace!("[actor] mapped to {} bus event(s)", events.len());

        for event in events {
            // Validate before dispatch — drops tool events with empty tool_use_id
            if let Some(warn) = validate_bus_event(&event) {
                log::warn!(
                    "[actor] invalid event dropped: {}.{}: {}",
                    warn.event_type,
                    warn.field,
                    warn.detail
                );
                self.protocol.stats.invalid_tool_count += 1;
                continue;
            }

            // Step 4a: Internal turn routing
            if self.is_internal_turn() {
                match &event {
                    // Capture context data in both Active and Draining phases.
                    // Soft timeout only warns; data is still accepted until RunState ends the turn.
                    BusEvent::CommandOutput { .. } => {
                        if let Some(ref mut ext) = self.active_extractor {
                            ext.on_event(&event);
                        }
                    }
                    BusEvent::MessageComplete {
                        ref text,
                        ref parent_tool_use_id,
                        ..
                    } => {
                        if let Some(ref mut ext) = self.active_extractor {
                            ext.on_event(&event);
                        }
                        // Ralph: accumulate top-level assistant text (only during ralph turns)
                        if parent_tool_use_id.is_none() {
                            let is_ralph_turn = self
                                .active_turn
                                .as_ref()
                                .map(|t| matches!(t.origin, TurnOrigin::Ralph))
                                .unwrap_or(false);
                            if is_ralph_turn {
                                if let Some(ref mut ralph) = self.ralph_loop {
                                    ralph.turn_toplevel_texts.push(text.clone());
                                }
                            }
                        }
                    }
                    BusEvent::RunState { state, .. } => {
                        log::debug!(
                            "[turn] internal turn ended: state={}, run_id={}",
                            state,
                            self.run_id
                        );
                        self.end_turn_and_dispatch().await;
                    }
                    _ => {
                        // Suppress all other events during internal turn
                        log::trace!(
                            "[turn] internal: suppressed {:?}",
                            std::mem::discriminant(&event)
                        );
                    }
                }
                continue;
            }

            // Step 4b: User turn (or idle) routing
            match &event {
                BusEvent::RunState {
                    state,
                    exit_code,
                    error,
                    ..
                } => {
                    // HC#1: parser emits idle on both success and error result events
                    // (CLI is still alive). On user interrupt, the result is just the
                    // cancel ack — suppress error and clear protocol error tracking so
                    // finalize_meta on EOF does not mark the run as Failed.
                    let (emit_state, emit_error) =
                        if self.pending_interrupt && (state == "idle" || state == "failed") {
                            self.pending_interrupt = false;
                            self.protocol.got_result_event = false;
                            self.protocol.result_subtype = None;
                            log::debug!("[actor] interrupt result → idle, error tracking cleared");
                            (String::from("idle"), None)
                        } else {
                            (state.clone(), error.clone())
                        };

                    self.emit_state(&emit_state, *exit_code, emit_error.clone(), false);

                    if emit_state == "idle" {
                        self.persist_idle_running(RunStatus::Idle);
                        // If the result event indicated an error, persist subtype+message
                        // to meta so finalize_meta on EOF can mark the run Failed.
                        let had_result_error = self
                            .protocol
                            .result_subtype
                            .as_deref()
                            .map(|s| s.starts_with("error"))
                            .unwrap_or(false);
                        if had_result_error {
                            log::debug!(
                                "[actor] persisting idle-with-error: subtype={:?}, error={:?}",
                                self.protocol.result_subtype,
                                emit_error
                            );
                            if let Err(e) = storage::runs::persist_result_error(
                                &self.run_id,
                                emit_error.clone(),
                                self.protocol.result_subtype.clone(),
                            ) {
                                log::warn!("[actor] failed to persist result error: {}", e);
                            }
                        }
                    }

                    // Defensive: handle_eof emits failed directly (not via process_event),
                    // but keep this branch in case any other emitter routes failed here.
                    if emit_state == "failed" {
                        log::debug!(
                            "[actor] persisting result error: subtype={:?}, error={:?}",
                            self.protocol.result_subtype,
                            emit_error
                        );
                        if let Err(e) = storage::runs::persist_result_error(
                            &self.run_id,
                            emit_error.clone(),
                            self.protocol.result_subtype.clone(),
                        ) {
                            log::warn!("[actor] failed to persist result error: {}", e);
                        }
                    }

                    // Turn completion: idle or failed → on_user_turn_finished + ralph + end turn
                    if (emit_state == "idle" || emit_state == "failed")
                        && self.active_turn.is_some()
                    {
                        let turn = self.active_turn.take().unwrap();
                        self.on_user_turn_finished(&turn);
                        self.active_extractor = None;
                        self.protocol.set_pending_slash_command(None);

                        // Ralph loop: classify turn outcome.
                        // emit_state is now "idle" on both success and error result events,
                        // so use the presence of emit_error (cleared above on interrupt) to
                        // distinguish a failed turn from a successful one.
                        let turn_failed = emit_state == "failed"
                            || (emit_state == "idle" && emit_error.is_some());
                        self.ralph_on_turn_end(&turn, turn_failed);

                        self.try_dispatch().await;
                    }

                    continue; // RunState handled
                }
                BusEvent::SessionInit {
                    session_id: Some(ref sid),
                    ..
                } => {
                    log::debug!("[actor] captured session_id={}", sid);
                    // Single with_meta write: session_id + conversation_ref (avoid double write + intermediate state)
                    let sid_clone = sid.clone();
                    if let Err(e) = storage::runs::with_meta(&self.run_id, |meta| {
                        meta.session_id = Some(sid_clone.clone());
                        meta.conversation_ref =
                            Some(crate::models::ConversationRef::ClaudeSession(sid_clone));
                        Ok(())
                    }) {
                        log::warn!(
                            "[actor] failed to persist session_id + conversation_ref: {}",
                            e
                        );
                    }
                    self.persist_and_emit(&event);
                }
                _ => {
                    // Inject backend-authoritative turn_index into UsageUpdate for user turns
                    if let BusEvent::UsageUpdate { .. } = &event {
                        if let Some(ref turn) = self.active_turn {
                            let mut enriched = event.clone();
                            if let BusEvent::UsageUpdate {
                                ref mut turn_index, ..
                            } = enriched
                            {
                                *turn_index = Some(turn.turn_index);
                                log::debug!(
                                    "[turn] usage_update injected turn_index={}",
                                    turn.turn_index
                                );
                            }
                            self.persist_and_emit(&enriched);
                        } else {
                            self.persist_and_emit(&event);
                        }
                    } else {
                        self.persist_and_emit(&event);
                    }
                }
            }
        }
    }

    /// Handle control events during user turns (or idle): permission prompts, hooks, etc.
    async fn handle_control_event(&mut self, parsed: &Value, event_type: &str) {
        if event_type == "control_response" {
            let req_id = parsed
                .get("response")
                .and_then(|r| r.get("request_id"))
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("request_id").and_then(|v| v.as_str()));
            if let Some(req_id) = req_id {
                log::debug!("[actor] got control_response for req_id={}", req_id);
                if let Some(tx) = self.control_waiters.remove(req_id) {
                    let response = parsed.get("response").cloned().unwrap_or(Value::Null);
                    let _ = tx.send(response);
                }
            } else {
                log::warn!(
                    "[actor] control_response missing request_id: {}",
                    truncate_str(&parsed.to_string(), 200)
                );
            }
            return;
        }

        if event_type == "control_cancel_request" {
            let cancel_request_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            log::debug!(
                "[actor] control_cancel_request for req_id={}",
                cancel_request_id
            );
            self.control_waiters.remove(&cancel_request_id);
            self.persist_and_emit(&BusEvent::ControlCancelled {
                run_id: self.run_id.clone(),
                request_id: cancel_request_id,
            });
            return;
        }

        // control_request
        let subtype = parsed
            .get("request")
            .and_then(|r| r.get("subtype"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if subtype == "hook_callback" {
            let request_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request = parsed.get("request").cloned().unwrap_or(Value::Null);
            let hook_event = request
                .get("hook_event")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let hook_id = request
                .get("hook_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let hook_name = request
                .get("tool_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            log::debug!(
                "[actor] hook_callback: run_id={}, req_id={}, event={}, id={}, tool={:?}",
                self.run_id,
                request_id,
                hook_event,
                hook_id,
                hook_name
            );

            let hook_label = hook_name.as_deref().unwrap_or("hook").to_string();
            self.persist_and_emit(&BusEvent::HookCallback {
                run_id: self.run_id.clone(),
                request_id: request_id.clone(),
                hook_event: hook_event.clone(),
                hook_id,
                hook_name,
                data: request.clone(),
            });

            if hook_event != "PreToolUse" {
                log::debug!("[actor] auto-allowing non-PreToolUse hook: {}", hook_event);
                if let Err(e) = self
                    .write_control_response(&request_id, serde_json::json!({ "decision": "allow" }))
                    .await
                {
                    log::warn!("[actor] hook_callback auto-response failed: {}", e);
                }
            }
            if hook_event == "PreToolUse" {
                self.pending_interactive_request = Some(PendingInteractiveRequest {
                    request_id: request_id.clone(),
                    subtype: "hook_callback".to_string(),
                    detail: format!("PreToolUse:{}", hook_label),
                    received_at: Instant::now(),
                });
                notify_if_background(
                    self.emitter.app(),
                    "Hook Review Required",
                    &format!(
                        "{} — PreToolUse: {}",
                        truncate_str(&self.run_id, 8),
                        hook_label
                    ),
                );
            }
        } else if subtype == "mcp_message" {
            log::debug!("[actor] mcp_message: run_id={}", self.run_id);
            self.emitter.emit_realtime(
                "bus-event",
                &BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: "mcp_message".to_string(),
                    data: parsed.clone(),
                },
                Some(&self.run_id),
            );
        } else if subtype == "elicitation" {
            // MCP elicitation: CLI requests user input for MCP server authentication/configuration.
            let request_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request = parsed.get("request").cloned().unwrap_or(Value::Null);
            let mcp_server_name = request
                .get("mcp_server_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let message = request
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let elicitation_id = request
                .get("elicitation_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let mode = request
                .get("mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let url = request
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let requested_schema = request.get("requested_schema").cloned();

            log::debug!(
                "[actor] elicitation: run_id={}, req_id={}, server={}, mode={:?}, has_schema={}",
                self.run_id,
                request_id,
                mcp_server_name,
                mode,
                requested_schema.is_some()
            );

            self.persist_and_emit(&BusEvent::ElicitationPrompt {
                run_id: self.run_id.clone(),
                request_id: request_id.clone(),
                mcp_server_name: mcp_server_name.clone(),
                message,
                elicitation_id,
                mode,
                url,
                requested_schema,
            });
            self.pending_interactive_request = Some(PendingInteractiveRequest {
                request_id: request_id.clone(),
                subtype: "elicitation".to_string(),
                detail: mcp_server_name.clone(),
                received_at: Instant::now(),
            });
            notify_if_background(
                self.emitter.app(),
                "MCP Input Required",
                &format!(
                    "{}: {} needs input",
                    truncate_str(&self.run_id, 8),
                    &mcp_server_name
                ),
            );
        } else if subtype == "can_use_tool" {
            let request_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request = parsed.get("request").cloned().unwrap_or(Value::Null);
            let tool_name = request
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let tool_use_id = request
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_input = request
                .get("input")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let decision_reason = request
                .get("decision_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let parent_tool_use_id = parsed
                .get("parent_tool_use_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let suggestions = request
                .get("suggestions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            log::debug!(
                "[actor] permission prompt: run_id={}, req_id={}, tool={}, reason={}, parent={:?}, suggestions={}",
                self.run_id, request_id, tool_name, decision_reason, parent_tool_use_id, suggestions.len()
            );

            let tool_label = tool_name.clone();
            self.persist_and_emit(&BusEvent::PermissionPrompt {
                run_id: self.run_id.clone(),
                request_id: request_id.clone(),
                tool_name,
                tool_use_id,
                tool_input,
                decision_reason,
                parent_tool_use_id,
                suggestions,
            });
            self.pending_interactive_request = Some(PendingInteractiveRequest {
                request_id,
                subtype: "can_use_tool".to_string(),
                detail: tool_label.clone(),
                received_at: Instant::now(),
            });
            notify_if_background(
                self.emitter.app(),
                "Permission Required",
                &format!(
                    "{} wants to use: {}",
                    truncate_str(&self.run_id, 8),
                    &tool_label
                ),
            );
        } else {
            // Fallback: unknown or malformed subtype — reply with an error control_response
            // so the CLI fails fast instead of waiting for a response until USER_HARD_TIMEOUT.
            let req_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            log::warn!(
                "[actor] unhandled control_request: run_id={}, subtype={}, req_id={}, keys={:?}",
                self.run_id,
                subtype,
                req_id,
                parsed
                    .get("request")
                    .map(|r| r.as_object().map(|o| o.keys().collect::<Vec<_>>()))
            );
            if !req_id.is_empty() {
                if let Err(e) = self
                    .write_control_response_error(
                        req_id,
                        &format!("unsupported control_request subtype: {}", subtype),
                    )
                    .await
                {
                    log::warn!(
                        "[actor] control_response(error) failed: run_id={}, req_id={}, subtype={}, err={}",
                        self.run_id, req_id, subtype, e
                    );
                }
            }
        }
    }

    /// Handle control events during internal turns or quarantine. (HC #26, #33)
    /// Silently resolve waiters, auto-respond to requests, suppress all emission.
    async fn handle_control_event_internal(&mut self, parsed: &Value, event_type: &str) {
        if event_type == "control_response" {
            let req_id = parsed
                .get("response")
                .and_then(|r| r.get("request_id"))
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("request_id").and_then(|v| v.as_str()));
            if let Some(req_id) = req_id {
                log::debug!("[turn] internal control_response for req_id={}", req_id);
                if let Some(tx) = self.control_waiters.remove(req_id) {
                    let response = parsed.get("response").cloned().unwrap_or(Value::Null);
                    let _ = tx.send(response);
                }
            } else {
                log::warn!(
                    "[turn] internal control_response missing request_id: {}",
                    truncate_str(&parsed.to_string(), 200)
                );
            }
            return;
        }

        if event_type == "control_cancel_request" {
            let cancel_request_id = parsed
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            log::debug!(
                "[turn] internal control_cancel_request for req_id={}",
                cancel_request_id
            );
            self.control_waiters.remove(&cancel_request_id);
            return;
        }

        // control_request during internal/quarantine: auto-respond (HC #33)
        let request_id = parsed
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if request_id.is_empty() {
            log::warn!("[turn] internal control_request with empty request_id, ignoring");
            return;
        }

        let subtype = parsed
            .get("request")
            .and_then(|r| r.get("subtype"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        log::debug!(
            "[turn] internal control_request: subtype={}, req_id={}",
            subtype,
            request_id
        );

        // can_use_tool/hook_callback get a success auto-response. Everything else
        // (elicitation can't be serviced without a user during an internal turn, and
        // unknown subtypes have no schema) gets a protocol-correct error response —
        // never control_cancel_request, which would leave the CLI waiting until timeout.
        let result: Result<Value, String> = match subtype {
            "can_use_tool" => Ok(serde_json::json!({
                "behavior": "deny",
                "message": "Tool use not allowed during internal turn"
            })),
            "hook_callback" => Ok(serde_json::json!({ "decision": "allow" })),
            _ => Err(format!(
                "unsupported control_request subtype during internal turn: {}",
                subtype
            )),
        };

        let write = match result {
            Ok(response) => self.write_control_response(&request_id, response).await,
            Err(msg) => {
                log::warn!(
                    "[turn] internal: unhandled control_request: run_id={}, subtype={}, req_id={}",
                    self.run_id,
                    subtype,
                    request_id
                );
                self.write_control_response_error(&request_id, &msg).await
            }
        };
        if let Err(e) = write {
            log::warn!(
                "[turn] internal control auto-response failed: req_id={}, err={}",
                request_id,
                e
            );
        }
    }

    fn handle_stderr_line(&mut self, text: &str) {
        // Suppress stderr after cancel
        if self.cancel.is_cancelled() {
            log::trace!(
                "[actor] stderr suppressed after cancel: {}",
                truncate_str(text, 200)
            );
            return;
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        log::trace!(
            "[actor] stderr: {}: {}",
            self.run_id,
            truncate_str(text, 200)
        );

        // Codex app-server writes verbose ANSI-colored tracing logs to stderr (DEBUG/INFO/
        // ERROR lines, including expected ones like "exec_command failed: Rejected(rejected by
        // user)"). Meaningful events (errors, results, approvals) all arrive via stdout
        // JSON-RPC, so surfacing stderr as timeline cards is pure noise (and renders garbled
        // due to the ANSI escapes). Log it for diagnostics, don't emit it.
        if self.codex.is_some() {
            log::debug!("[actor] codex stderr: {}", truncate_str(text, 300));
            return;
        }

        let event = BusEvent::Raw {
            run_id: self.run_id.clone(),
            source: "claude_stderr".to_string(),
            data: Value::String(strip_ansi(text)),
        };
        self.emitter.persist_and_emit(&self.run_id, &event);
    }

    /// Handle stdout EOF — determine terminal state.
    async fn handle_eof(&mut self) {
        let exit_code = if let Some(ref mut child) = self.child {
            match child.wait().await {
                Ok(s) => s.code(),
                Err(_) => Some(1),
            }
        } else {
            None
        };

        log::debug!(
            "[actor] EOF cleanup: run_id={}, got_result={}, exit_code={:?}",
            self.run_id,
            self.protocol.got_result_event,
            exit_code
        );

        // Fail all pending user replies on EOF (HC #12)
        self.fail_all_pending_replies("Session ended");
        self.active_turn = None;
        self.active_extractor = None;
        self.quarantine_until_result = false;

        if !self.protocol.got_result_event {
            let state_str = if self.cancel.is_cancelled() {
                "stopped"
            } else {
                match exit_code {
                    Some(0) => "completed",
                    _ => "failed",
                }
            };
            let error_msg = if state_str == "failed" {
                Some(format!("Process exited with code {:?}", exit_code))
            } else {
                None
            };
            self.emit_state(state_str, exit_code, error_msg, true);
        } else {
            self.finalize_meta(exit_code);
        }
    }

    // ── RunState emission (migrated from state.rs) ──

    /// Emit a RunState event with identity dedup. Single entry point.
    fn emit_state(
        &mut self,
        new_state: &str,
        exit_code: Option<i32>,
        error: Option<String>,
        update_meta: bool,
    ) {
        // 1. Identity dedup
        if self.state == new_state {
            log::debug!(
                "[actor] dedup skip: run={} state={} (already current)",
                self.run_id,
                new_state
            );
            return;
        }
        self.state = new_state.to_string();

        log::debug!(
            "[actor] emit_state: run={} -> {} (meta={})",
            self.run_id,
            new_state,
            update_meta
        );

        // 2. Build event
        let event = BusEvent::RunState {
            run_id: self.run_id.clone(),
            state: new_state.to_string(),
            exit_code,
            error: error.clone(),
        };

        // 3. Persist + Tauri emit + WS broadcast (unified)
        self.emitter.persist_and_emit(&self.run_id, &event);

        // 4. Conditional meta update
        if update_meta {
            if let Some(status) = map_state_to_run_status(new_state) {
                let meta_error = if new_state == "failed" {
                    error.clone()
                } else {
                    None
                };
                if let Err(e) = runs::update_status(&self.run_id, status, exit_code, meta_error) {
                    log::warn!(
                        "[actor] meta update failed: run={} state={} err={}",
                        self.run_id,
                        new_state,
                        e
                    );
                }
            }

            // Clear error fields on new turn
            if new_state == "running" {
                if let Err(e) = runs::with_meta(&self.run_id, |meta| {
                    if meta.error_message.is_some() || meta.result_subtype.is_some() {
                        meta.error_message = None;
                        meta.result_subtype = None;
                        log::debug!(
                            "[actor] cleared error_message/result_subtype for new turn: run={}",
                            self.run_id
                        );
                    }
                    Ok(())
                }) {
                    log::warn!(
                        "[actor] clear error fields failed: run={} err={}",
                        self.run_id,
                        e
                    );
                }
                // Also reset the in-mem error tracking. The parser sets result_subtype
                // only on error results and never clears it on success (see claude_protocol
                // test "success doesn't set got_result_event"). Without this, a stale error
                // subtype from an earlier turn survives into a later successful turn and
                // gets re-persisted at that turn's idle (had_result_error), making
                // finalize_meta on EOF wrongly mark a 0-exit run as Failed. Mirrors the
                // meta clear above and the interrupt path's reset.
                self.protocol.result_subtype = None;
            }

            // Persist result error details on failed
            if new_state == "failed" {
                log::debug!(
                    "[actor] emit_state persisting result error: subtype={:?}, error={:?}",
                    self.protocol.result_subtype,
                    error
                );
                if let Err(e) = runs::persist_result_error(
                    &self.run_id,
                    error,
                    self.protocol.result_subtype.clone(),
                ) {
                    log::warn!("[actor] failed to persist result error: {}", e);
                }
            }
        }
    }

    /// Finalize meta.json on EOF when result event already set RunState.
    /// Determines terminal status from result_subtype + exit_code.
    fn finalize_meta(&self, exit_code: Option<i32>) {
        if let Err(e) = runs::with_meta(&self.run_id, |meta| {
            let had_result_error = meta
                .result_subtype
                .as_ref()
                .map(|s| s.starts_with("error"))
                .unwrap_or(false);
            let terminal_status = if had_result_error {
                RunStatus::Failed
            } else {
                match exit_code {
                    Some(0) => RunStatus::Completed,
                    _ => RunStatus::Failed,
                }
            };
            meta.status = terminal_status.clone();
            meta.exit_code = exit_code;
            if meta.ended_at.is_none() {
                meta.ended_at = Some(now_iso());
            }
            log::debug!(
                "[actor] finalize_meta: run={} status={:?} exit_code={:?}",
                self.run_id,
                terminal_status,
                exit_code
            );
            Ok(())
        }) {
            log::warn!(
                "[actor] finalize_meta failed: run={} err={}",
                self.run_id,
                e
            );
        }
    }

    // ── Cleanup ──

    async fn cleanup(mut self) {
        log::debug!("[actor] cleanup starting: run_id={}", self.run_id);

        // Drop stdin
        self.stdin.take();

        // Fail all pending user replies (HC #12)
        self.fail_all_pending_replies("Session cleanup");

        // Drain control waiters
        if !self.control_waiters.is_empty() {
            log::debug!(
                "[actor] draining {} pending control waiters",
                self.control_waiters.len()
            );
            self.control_waiters.clear();
        }

        // Remove self from SessionMap (only if we're still the current entry)
        {
            let mut map = self.sessions.lock().await;
            if let Some(handle) = map.get(&self.run_id) {
                if Arc::ptr_eq(&self.tag, &handle.tag) {
                    map.remove(&self.run_id);
                    log::debug!(
                        "[actor] removed self from SessionMap: run_id={}",
                        self.run_id
                    );
                } else {
                    log::debug!(
                        "[actor] skipping SessionMap remove (replaced): run_id={}",
                        self.run_id
                    );
                }
            }
        }

        // Fire shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        log::debug!("[actor] cleanup complete: run_id={}", self.run_id);
    }
}

// ── Helpers ──

impl SessionActor {
    /// Persist idle↔running status transition to meta + notify all windows.
    /// Only allows Running→Idle and Idle→Running; other transitions are skipped.
    fn persist_idle_running(&self, target: RunStatus) {
        let meta = match storage::runs::get_run(&self.run_id) {
            Some(m) => m,
            None => return,
        };
        let allowed = matches!(
            (&meta.status, &target),
            (RunStatus::Running, RunStatus::Idle) | (RunStatus::Idle, RunStatus::Running)
        );
        if !allowed {
            log::debug!(
                "[actor] persist_idle_running skip: run={} from={:?} to={:?}",
                self.run_id,
                meta.status,
                target
            );
            return;
        }
        let status_str = target.to_string();
        if let Err(e) = storage::runs::update_status(&self.run_id, target, None, None) {
            log::warn!(
                "[actor] idle/running meta update failed: run={} target={} err={}",
                self.run_id,
                status_str,
                e
            );
        } else {
            self.emitter.emit_realtime(
                "ocv:status-changed",
                &serde_json::json!({"run_id": self.run_id.as_str(), "status": status_str}),
                Some(&self.run_id),
            );
        }
    }
}

fn map_state_to_run_status(state: &str) -> Option<RunStatus> {
    match state {
        "spawning" | "running" => Some(RunStatus::Running),
        "completed" => Some(RunStatus::Completed),
        "failed" => Some(RunStatus::Failed),
        "stopped" => Some(RunStatus::Stopped),
        "idle" => Some(RunStatus::Idle),
        _ => None,
    }
}

/// Sanitize a filename: keep only safe characters, truncate to 120 chars.
fn att_safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let truncated = if cleaned.len() > 120 {
        &cleaned[..120]
    } else {
        &cleaned
    };
    if truncated.is_empty() {
        "attachment.bin".to_string()
    } else {
        truncated.to_string()
    }
}

/// Map MIME type to file extension.
fn att_extension(mime: &str) -> &str {
    if mime.starts_with("image/png") {
        ".png"
    } else if mime.starts_with("image/jpeg") {
        ".jpg"
    } else if mime.starts_with("image/webp") {
        ".webp"
    } else if mime.starts_with("image/gif") {
        ".gif"
    } else if mime.starts_with("application/pdf") {
        ".pdf"
    } else {
        ""
    }
}

/// Save an attachment to `~/.opencovibe/runs/{run_id}/attachments/` and return the path.
/// Returns `None` on failure (non-fatal, logged as warning).
fn save_attachment_to_disk(run_id: &str, att: &AttachmentData) -> Option<String> {
    let att_dir = crate::storage::run_dir(run_id).join("attachments");
    if let Err(e) = std::fs::create_dir_all(&att_dir) {
        log::warn!("[actor] failed to create attachments dir: {}", e);
        return None;
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&att.content_base64)
        .map_err(|e| log::warn!("[actor] failed to decode attachment base64: {}", e))
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let safe_name = att_safe_filename(&att.filename);
    let ext = att_extension(&att.media_type);
    let filename = format!(
        "{}-{}-{}{}",
        chrono::Utc::now().timestamp_millis(),
        &uuid::Uuid::new_v4().to_string()[..6],
        safe_name,
        ext
    );
    let full_path = att_dir.join(&filename);
    if let Err(e) = std::fs::write(&full_path, &bytes) {
        log::warn!("[actor] failed to write attachment to disk: {}", e);
        return None;
    }
    let path_str = full_path.to_string_lossy().to_string();
    log::debug!("[actor] saved attachment to disk: {}", path_str);
    Some(path_str)
}

/// Build a stream-json `user` payload with optional multimodal attachments.
/// Shared between actor's `handle_send_message` and `session.rs` initial message paths.
/// When attachments are present, saves them to disk under the run directory and
/// includes file paths in the text block so the model can reference them later.
pub fn build_user_payload(
    text: &str,
    attachments: &[AttachmentData],
    run_id: &str,
) -> (serde_json::Value, String) {
    let content = if attachments.is_empty() {
        serde_json::json!(text)
    } else {
        let mut parts = Vec::new();
        let mut saved_paths: Vec<String> = Vec::new();
        for att in attachments {
            // Size check (base64 → raw bytes estimate: base64 len * 3/4)
            let raw_size = (att.content_base64.len() as u64) * 3 / 4;
            let limit = max_attachment_size(&att.media_type);
            if raw_size > limit {
                let limit_mb = limit / (1024 * 1024);
                log::warn!(
                    "[actor] skipping oversized attachment: {} ({:.1}MB > {}MB limit)",
                    att.filename,
                    raw_size as f64 / (1024.0 * 1024.0),
                    limit_mb
                );
                continue;
            }
            // Save to disk for later Read tool access
            if let Some(path) = save_attachment_to_disk(run_id, att) {
                saved_paths.push(path);
            }
            if ALLOWED_DOC_TYPES.contains(&att.media_type.as_str()) {
                parts.push(serde_json::json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": att.media_type,
                        "data": att.content_base64,
                    }
                }));
            } else if ALLOWED_IMAGE_TYPES.contains(&att.media_type.as_str()) {
                parts.push(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": att.media_type,
                        "data": att.content_base64,
                    }
                }));
            } else {
                log::warn!(
                    "[actor] skipping unsupported attachment type: {}",
                    att.media_type
                );
            }
        }
        // Augment text with saved file paths so the model can Read them later
        let augmented_text = if saved_paths.is_empty() {
            text.to_string()
        } else {
            let paths_list = saved_paths
                .iter()
                .map(|p| format!("- {}", p))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n\n[Attached files saved at:\n{}\nUse these file paths with the Read tool if you need to access them later.]",
                text, paths_list
            )
        };
        parts.insert(
            0,
            serde_json::json!({ "type": "text", "text": augmented_text }),
        );
        serde_json::json!(parts)
    };

    let uuid = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({
        "type": "user",
        "uuid": &uuid,
        "message": {
            "role": "user",
            "content": content,
        }
    });
    (payload, uuid)
}

#[cfg(test)]
mod tests {
    use super::build_control_response;
    use crate::models::{max_attachment_size, ALLOWED_DOC_TYPES, ALLOWED_IMAGE_TYPES};
    use serde_json::json;

    /// Helper: build a multimodal content array the same way handle_send_message does,
    /// including size validation (base64 len * 3/4 vs max_attachment_size).
    fn build_content_parts(
        text: &str,
        attachments: &[(&str, &str)], // (media_type, base64_data)
    ) -> Vec<serde_json::Value> {
        let mut parts = vec![json!({ "type": "text", "text": text })];
        for (media_type, data) in attachments {
            // Size check (mirrors handle_send_message)
            let raw_size = (data.len() as u64) * 3 / 4;
            let limit = max_attachment_size(media_type);
            if raw_size > limit {
                continue; // oversized — skip
            }
            if ALLOWED_DOC_TYPES.contains(media_type) {
                parts.push(json!({
                    "type": "document",
                    "source": { "type": "base64", "media_type": media_type, "data": data }
                }));
            } else if ALLOWED_IMAGE_TYPES.contains(media_type) {
                parts.push(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data }
                }));
            }
            // else: skipped (unsupported)
        }
        parts
    }

    #[test]
    fn image_attachment_produces_image_type() {
        let parts = build_content_parts("hello", &[("image/png", "abc123")]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["type"], "image");
        assert_eq!(parts[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn pdf_attachment_produces_document_type() {
        let parts = build_content_parts("hello", &[("application/pdf", "pdfdata")]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["type"], "document");
        assert_eq!(parts[1]["source"]["media_type"], "application/pdf");
    }

    #[test]
    fn unsupported_type_is_skipped() {
        let parts = build_content_parts("hello", &[("application/octet-stream", "data")]);
        assert_eq!(parts.len(), 1); // Only text part, attachment skipped
    }

    #[test]
    fn control_response_success_shape() {
        let p = build_control_response("r1", Ok(serde_json::json!({ "x": 1 })));
        assert_eq!(p["type"], "control_response");
        assert_eq!(p["response"]["subtype"], "success");
        // HC#2: request_id nested inside `response`, NOT at top level.
        assert_eq!(p["response"]["request_id"], "r1");
        assert!(p.get("request_id").is_none());
        assert_eq!(p["response"]["response"]["x"], 1);
    }

    #[test]
    fn control_response_error_shape() {
        let p = build_control_response("r2", Err("nope".to_string()));
        assert_eq!(p["type"], "control_response");
        assert_eq!(p["response"]["subtype"], "error");
        assert_eq!(p["response"]["request_id"], "r2");
        assert_eq!(p["response"]["error"], "nope");
        // Error responses carry no nested `response` body.
        assert!(p["response"].get("response").is_none());
    }

    #[test]
    fn mixed_attachments() {
        let parts = build_content_parts(
            "hello",
            &[
                ("image/jpeg", "img"),
                ("application/pdf", "doc"),
                ("application/zip", "zip"),
            ],
        );
        assert_eq!(parts.len(), 3); // text + image + document (zip skipped)
        assert_eq!(parts[1]["type"], "image");
        assert_eq!(parts[2]["type"], "document");
    }

    #[test]
    fn large_image_is_not_skipped() {
        // Images have no size limit (CLI handles compression via sharp)
        let large_b64 = "A".repeat(14_000_000); // ~10.5MB raw — still accepted
        let parts = build_content_parts("hello", &[("image/png", &large_b64)]);
        assert_eq!(parts.len(), 2); // text + image (not skipped)
        assert_eq!(parts[1]["type"], "image");
    }

    #[test]
    fn oversized_pdf_is_skipped() {
        // PDFs have 20MB limit. base64_len * 3/4 > 20*1024*1024 → skip
        let oversized_b64 = "A".repeat(28_000_000); // ~21MB raw → exceeds 20MB limit
        let parts = build_content_parts("hello", &[("application/pdf", &oversized_b64)]);
        assert_eq!(parts.len(), 1); // Only text part, oversized PDF skipped
    }

    #[test]
    fn build_user_payload_returns_uuid() {
        use super::build_user_payload;
        let (payload, uuid) = build_user_payload("hello", &[], "run-test");
        assert_eq!(payload["type"], "user");
        assert_eq!(payload["uuid"], uuid);
        assert!(uuid::Uuid::parse_str(&uuid).is_ok());
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        use super::strip_ansi;
        let input = "\u{1b}[2m2026-06-03\u{1b}[0m \u{1b}[31mERROR\u{1b}[0m codex_core: failed";
        assert_eq!(strip_ansi(input), "2026-06-03 ERROR codex_core: failed");
        // Plain text is unchanged.
        assert_eq!(strip_ansi("no codes here"), "no codes here");
    }
}

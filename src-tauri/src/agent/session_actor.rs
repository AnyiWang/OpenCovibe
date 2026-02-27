//! Session Actor — single owner of a Claude CLI session's entire lifecycle.
//!
//! One actor per run_id. All session mutations (send, control, stop) go through
//! the actor's mailbox (bounded mpsc channel), guaranteeing sequential execution
//! without external locks. The actor owns the process, stdin, stdout/stderr readers,
//! protocol state, and RunState emission — eliminating the cross-system coordination
//! that previously caused race conditions.

use crate::agent::adapter::ActorSessionMap;
use crate::agent::claude_protocol::{validate_bus_event, ProtocolState};
use crate::agent::notify::notify_if_background;
use crate::agent::turn_engine::{
    ActiveTurn, ContextExtractor, InternalExtractor, InternalJob, InternalJobKind, TurnOrigin,
    TurnPhase, UserTurnKind, UserTurnTicket, INTERNAL_HARD_TIMEOUT, INTERNAL_SOFT_TIMEOUT,
    QUARANTINE_DEADLINE, TICK_INTERVAL, USER_HARD_TIMEOUT, USER_SOFT_TIMEOUT,
};
use crate::models::{
    max_attachment_size, now_iso, BusEvent, RunStatus, ALLOWED_DOC_TYPES, ALLOWED_IMAGE_TYPES,
};
use crate::storage;
use crate::storage::events::EventWriter;
use crate::storage::runs;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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
    app: AppHandle,
    writer: Arc<EventWriter>,
    sessions: ActorSessionMap,
    run_id: String,
    tag: Arc<()>,
    protocol: ProtocolState,
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
    /// Set after quarantine kill — reject new messages, break run loop.
    terminated: bool,
    /// JSON parse failures in handle_stdout_line (before map_event).
    /// Complements ParserStats.parse_warn_count (field-level malformation).
    json_parse_fail_count: u32,
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
    app: AppHandle,
    writer: Arc<EventWriter>,
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
        app,
        writer,
        sessions,
        run_id: run_id.clone(),
        tag: tag.clone(),
        protocol: ProtocolState::new(is_resume),
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
        terminated: false,
        json_parse_fail_count: 0,
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
                        Some(ActorCommand::SendMessage { text, attachments, reply }) => {
                            self.handle_send_message(text, attachments, reply).await;
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
                            let r = self.handle_cancel_control_request(&request_id).await;
                            let _ = reply.send(r);
                        }
                        Some(ActorCommand::RespondHookCallback { request_id, response, reply }) => {
                            log::debug!("[actor] RespondHookCallback: run_id={}, req_id={}", self.run_id, request_id);
                            let result = self.write_control_response(&request_id, response).await;
                            let _ = reply.send(result);
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
        let trimmed = text.trim().to_string();
        let turn_index = self.next_turn_index;
        self.next_turn_index += 1;

        let kind = if trimmed.starts_with('/') {
            UserTurnKind::Slash {
                command: trimmed.clone(),
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

        // Try user queue first (unless barrier blocks)
        if self.must_run_internal_for_turn.is_none() {
            if let Some(ticket) = self.queued_user.pop_front() {
                self.start_user_turn(ticket).await;
                return;
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
        if let Err(e) = self
            .write_user_to_stdin(&ticket.text, &ticket.attachments)
            .await
        {
            log::warn!("[turn] start_user: stdin write failed: {}", e);
            let _ = ticket.reply.send(Err(e));
            return;
        }

        // Emit UserMessage + RunState(running)
        self.persist_and_emit(&BusEvent::UserMessage {
            run_id: self.run_id.clone(),
            text: ticket.text.clone(),
        });
        self.emit_state("running", None, None, false);

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

        if let Err(e) = self.write_user_to_stdin("/context", &[]).await {
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
            app: self.app.clone(),
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
    fn on_user_turn_finished(&mut self, turn: &ActiveTurn) {
        if let TurnOrigin::User(UserTurnKind::Normal { auto_ctx_id }) = &turn.origin {
            let auto_ctx_id = *auto_ctx_id;
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
                        "[turn] quarantine hard-timeout: killing process for run_id={}",
                        self.run_id
                    );
                    self.protocol.set_pending_slash_command(None);
                    if let Some(ref mut child) = self.child {
                        let _ = child.kill().await;
                    }
                    self.emit_state(
                        "failed",
                        None,
                        Some("Auto-context hard timeout — process killed".to_string()),
                        true,
                    );
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
                "[turn] user hard timeout: entering quarantine for run_id={} (turn_seq={})",
                self.run_id,
                turn.turn_seq
            );
            self.protocol.set_pending_slash_command(None);
            self.active_turn = None;
            self.quarantine_until_result = true;
            self.interrupt_sent_for_quarantine = false;
            self.quarantine_deadline = None;
        }
    }

    /// Write a user-format message to CLI stdin.
    async fn write_user_to_stdin(
        &mut self,
        text: &str,
        attachments: &[AttachmentData],
    ) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;
        let payload = build_user_payload(text, attachments, &self.run_id);
        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        log::debug!(
            "[turn] write_user_to_stdin: run_id={}, len={}, attachments={}",
            self.run_id,
            text.len(),
            attachments.len()
        );
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("stdin write failed: {}", e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush failed: {}", e))?;
        Ok(())
    }

    /// Persist a BusEvent to JSONL and emit to frontend. (HC #32)
    fn persist_and_emit(&self, event: &BusEvent) {
        if let Err(e) = self.writer.write_bus_event(&self.run_id, event) {
            log::warn!("[actor] persist failed for run_id={}: {}", self.run_id, e);
        }
        let _ = self.app.emit("bus-event", event);
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
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;

        let request_id = format!("ocv_ctrl_{}", uuid::Uuid::new_v4());
        let subtype = request
            .get("subtype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        log::debug!(
            "[actor] send_control: run_id={}, subtype={}, req_id={}",
            self.run_id,
            subtype,
            request_id
        );

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

        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        log::debug!(
            "[actor] writing control request to stdin: {}",
            truncate_str(&line, 200)
        );

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("control write failed: {}", e))?;
        if let Err(e) = stdin.flush().await {
            log::warn!(
                "[actor] stdin flush failed for run_id={}: {}",
                self.run_id,
                e
            );
        }

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
        self.write_control_response(request_id, response).await
    }

    /// Send a control_cancel_request to CLI stdin (top-level message type).
    async fn handle_cancel_control_request(&mut self, request_id: &str) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;

        let payload = serde_json::json!({
            "type": "control_cancel_request",
            "request_id": request_id,
        });
        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        log::debug!(
            "[actor] cancel_control_request: run_id={}, req_id={}",
            self.run_id,
            request_id,
        );

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("cancel control request write failed: {}", e))?;
        if let Err(e) = stdin.flush().await {
            log::warn!(
                "[actor] stdin flush failed for run_id={}: {}",
                self.run_id,
                e
            );
        }

        Ok(())
    }

    /// Shared helper: write a control_response JSON to CLI stdin.
    async fn write_control_response(
        &mut self,
        request_id: &str,
        response: Value,
    ) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "stdin closed".to_string())?;

        // CLI expects: {"type":"control_response","response":{"subtype":"success","request_id":"...","response":{...}}}
        // request_id must be INSIDE the response wrapper, with subtype:"success"
        let payload = serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        });
        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        log::debug!(
            "[actor] write_control_response: run_id={}, req_id={}, payload={}",
            self.run_id,
            request_id,
            truncate_str(&line, 200)
        );

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("control response write failed: {}", e))?;
        if let Err(e) = stdin.flush().await {
            log::warn!(
                "[actor] stdin flush failed for run_id={}: {}",
                self.run_id,
                e
            );
        }

        Ok(())
    }

    // ── I/O handlers ──

    /// Handle a stdout line from CLI — three-way routing: quarantine → control → map events.
    async fn handle_stdout_line(&mut self, text: &str, line_num: u64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        log::trace!("[actor] stdout #{}: {}", line_num, truncate_str(text, 200));

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
                    BusEvent::CommandOutput { .. } | BusEvent::MessageComplete { .. } => {
                        if let Some(ref mut ext) = self.active_extractor {
                            ext.on_event(&event);
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
                    // Handle interrupt: CLI emits result(error) but session is alive.
                    let (emit_state, emit_error) =
                        if self.pending_interrupt && (state == "idle" || state == "failed") {
                            self.pending_interrupt = false;
                            if state == "failed" {
                                self.protocol.got_result_event = false;
                                self.protocol.result_subtype = None;
                                log::debug!("[actor] interrupt result → converted failed to idle");
                                (String::from("idle"), None)
                            } else {
                                (state.clone(), error.clone())
                            }
                        } else {
                            (state.clone(), error.clone())
                        };

                    self.emit_state(&emit_state, *exit_code, emit_error.clone(), false);

                    // Persist result error on failed
                    if emit_state == "failed" {
                        log::debug!(
                            "[actor] persisting result error: subtype={:?}, error={:?}",
                            self.protocol.result_subtype,
                            emit_error
                        );
                        if let Err(e) = storage::runs::persist_result_error(
                            &self.run_id,
                            emit_error,
                            self.protocol.result_subtype.clone(),
                        ) {
                            log::warn!("[actor] failed to persist result error: {}", e);
                        }
                    }

                    // Turn completion: idle or failed → on_user_turn_finished + end turn
                    if (emit_state == "idle" || emit_state == "failed")
                        && self.active_turn.is_some()
                    {
                        // Take the active turn for on_user_turn_finished
                        let turn = self.active_turn.take().unwrap();
                        self.on_user_turn_finished(&turn);
                        self.active_turn = None;
                        self.active_extractor = None;
                        self.protocol.set_pending_slash_command(None);
                        self.try_dispatch().await;
                    }

                    continue; // RunState handled
                }
                BusEvent::SessionInit {
                    session_id: Some(ref sid),
                    ..
                } => {
                    log::debug!("[actor] captured session_id={}", sid);
                    if let Err(e) = storage::runs::update_session_id(&self.run_id, sid) {
                        log::warn!("[actor] failed to persist session_id: {}", e);
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
                notify_if_background(
                    &self.app,
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
            let _ = self.app.emit(
                "bus-event",
                &BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: "mcp_message".to_string(),
                    data: parsed.clone(),
                },
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
                request_id,
                tool_name,
                tool_use_id,
                tool_input,
                decision_reason,
                parent_tool_use_id,
                suggestions,
            });
            notify_if_background(
                &self.app,
                "Permission Required",
                &format!(
                    "{} wants to use: {}",
                    truncate_str(&self.run_id, 8),
                    &tool_label
                ),
            );
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

        let response = match subtype {
            "can_use_tool" => serde_json::json!({
                "behavior": "deny",
                "message": "Tool use not allowed during internal turn"
            }),
            "hook_callback" => serde_json::json!({ "decision": "allow" }),
            _ => serde_json::json!({
                "behavior": "deny",
                "message": "Request denied during internal turn"
            }),
        };

        if let Err(e) = self.write_control_response(&request_id, response).await {
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

        let event = BusEvent::Raw {
            run_id: self.run_id.clone(),
            source: "claude_stderr".to_string(),
            data: Value::String(text.to_string()),
        };
        if let Err(e) = self.writer.write_bus_event(&self.run_id, &event) {
            log::warn!("[actor] stderr persist failed: {}", e);
        }
        let _ = self.app.emit("bus-event", &event);
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

        // 3. Persist to JSONL
        if let Err(e) = self.writer.write_bus_event(&self.run_id, &event) {
            log::warn!(
                "[actor] persist failed: run={} state={} err={}",
                self.run_id,
                new_state,
                e
            );
        }

        // 4. Emit to Tauri bus
        if let Err(e) = self.app.emit("bus-event", &event) {
            log::warn!(
                "[actor] emit failed: run={} state={} err={}",
                self.run_id,
                new_state,
                e
            );
        }

        // 5. Conditional meta update
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
                if let Some(mut meta) = runs::get_run(&self.run_id) {
                    if meta.error_message.is_some() || meta.result_subtype.is_some() {
                        meta.error_message = None;
                        meta.result_subtype = None;
                        if let Err(e) = runs::save_meta(&meta) {
                            log::warn!(
                                "[actor] clear error fields failed: run={} err={}",
                                self.run_id,
                                e
                            );
                        }
                        log::debug!(
                            "[actor] cleared error_message/result_subtype for new turn: run={}",
                            self.run_id
                        );
                    }
                }
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
        match runs::get_run(&self.run_id) {
            Some(mut meta) => {
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
                if let Err(e) = runs::save_meta(&meta) {
                    log::warn!(
                        "[actor] finalize_meta failed: run={} err={}",
                        self.run_id,
                        e
                    );
                }
                log::debug!(
                    "[actor] finalize_meta: run={} status={:?} exit_code={:?}",
                    self.run_id,
                    terminal_status,
                    exit_code
                );
            }
            None => {
                log::warn!("[actor] finalize_meta: run={} not found", self.run_id);
            }
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

fn map_state_to_run_status(state: &str) -> Option<RunStatus> {
    match state {
        "spawning" | "running" => Some(RunStatus::Running),
        "completed" => Some(RunStatus::Completed),
        "failed" => Some(RunStatus::Failed),
        "stopped" => Some(RunStatus::Stopped),
        "idle" => None,
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
) -> serde_json::Value {
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

    serde_json::json!({
        "type": "user",
        "uuid": uuid::Uuid::new_v4().to_string(),
        "message": {
            "role": "user",
            "content": content,
        }
    })
}

#[cfg(test)]
mod tests {
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
}

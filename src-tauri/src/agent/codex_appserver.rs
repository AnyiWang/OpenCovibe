//! Codex `app-server` session protocol — bidirectional JSON-RPC transport.
//!
//! Unlike `codex exec` (one-way NDJSON, no interactivity), `codex app-server` is the
//! bidirectional channel Codex's own TUI uses. It unlocks the interactive tools:
//! command/file approvals, multiple-choice `request_user_input`, and MCP elicitation.
//!
//! Wire facts below were captured live against codex 0.136.0 (see the spike in plan
//! `jolly-sparking-metcalfe` and the `codex-cli-reference` memo). Framing is
//! newline-delimited JSON. Client→server requests use our id space (1=initialize,
//! 2=thread/start|resume, 3+=turn/start). Server→client interactive requests carry their
//! own json-rpc `id`; we reply with `{id, result}` on that id.
//!
//! GOTCHA: `request_user_input` is gated to Plan mode and errors `unavailable in Default
//! mode` even over app-server. The spawn (in the actor wiring) MUST pass
//! `--enable default_mode_request_user_input` to unlock it in normal sessions.

use crate::agent::codex_parser::{codex_normalize_status, CodexToolKind};
use crate::agent::session_protocol::{
    CodexTurnOverrides, LifecycleSignal, ParsedLine, PendingInteractive, PendingKind,
    SessionProtocol, StartupCtx,
};
use crate::models::BusEvent;
use serde_json::{json, Value};
use std::collections::HashMap;

/// app-server method names for server→client interactive requests we handle.
const M_CMD_APPROVAL: &str = "item/commandExecution/requestApproval";
const M_CMD_APPROVAL_LEGACY: &str = "execCommandApproval";
const M_FILE_APPROVAL: &str = "item/fileChange/requestApproval";
const M_FILE_APPROVAL_LEGACY: &str = "applyPatchApproval";
const M_PERM_APPROVAL: &str = "item/permissions/requestApproval";
const M_REQUEST_USER_INPUT: &str = "item/tool/requestUserInput";
const M_ELICITATION: &str = "mcpServer/elicitation/request";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Before `thread/started` — not yet able to send turns.
    Opening,
    Ready,
}

/// A server-initiated request awaiting our JSON-RPC response.
struct PendingServerReq {
    /// Raw json-rpc id to echo in the response.
    raw_id: Value,
    method: String,
}

pub struct CodexAppServer {
    phase: Phase,
    thread_id: Option<String>,
    /// Id of the currently-running turn, captured from `turn/started` (`params.turn.id`).
    /// Required by `turn/steer` (`expectedTurnId`); cleared on completion/failure/error.
    active_turn_id: Option<String>,
    /// Our outgoing client→server request id counter (turns use 3, 4, …).
    next_client_id: i64,
    /// Pending server→client requests keyed by the request_id we surfaced to the frontend.
    pending: HashMap<String, PendingServerReq>,
    /// Outgoing data-returning requests we sent (`thread/fork`, `thread/rollback`,
    /// `thread/goal/get`, …) keyed by their JSON-RPC id → the frontend control `request_id`.
    /// When the reply arrives (`parse_line` sees a matching id with no `method`) we resolve the
    /// actor's `control_waiter` for that frontend request_id with the JSON-RPC `result`/`error`.
    client_waiters: HashMap<i64, String>,
    /// Extra writable directories from settings (`StartupCtx.add_dirs`). `thread/start`'s
    /// `sandbox` is a bare mode string and can't carry writable roots, so these are injected
    /// into the `workspaceWrite` `sandboxPolicy` on `turn/start` (persists server-side).
    add_dirs: Vec<String>,
    /// Spawn-time model/effort/approval/sandbox defaults. `thread/start` carries model/approval/
    /// sandbox but NOT effort, and on RESUME `thread/resume` carries none of them — so these are
    /// (re-)applied on the first `turn/start` after spawn, where Codex persists them for the
    /// thread. Live `set_model`/`set_effort`/`set_permission_mode` overrides from the actor take
    /// precedence per turn.
    startup_overrides: CodexTurnOverrides,
    /// Cleared after the first `turn/start` — gates the one-shot replay of `startup_overrides`.
    pending_startup_replay: bool,
}

impl Default for CodexAppServer {
    fn default() -> Self {
        Self {
            phase: Phase::Opening,
            thread_id: None,
            active_turn_id: None,
            next_client_id: 3,
            pending: HashMap::new(),
            client_waiters: HashMap::new(),
            add_dirs: Vec::new(),
            startup_overrides: CodexTurnOverrides::default(),
            pending_startup_replay: true,
        }
    }
}

impl CodexAppServer {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_client_id;
        self.next_client_id += 1;
        id
    }

    /// True once the thread is open and `turn/start` can be sent.
    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// Register a data-returning client→server request: allocate a JSON-RPC id, map it to the
    /// frontend `request_id` so `parse_line` can route the reply back, and return the wire frame.
    /// Returns empty when there's no open thread (nothing to address the request to).
    fn frame_tracked(&mut self, request_id: &str, method: &str, extra: Value) -> Vec<Value> {
        let thread_id = match &self.thread_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] {method} before thread/started — dropping");
                return vec![];
            }
        };
        let id = self.next_id();
        self.client_waiters.insert(id, request_id.to_string());
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                params.insert(k.clone(), v.clone());
            }
        }
        vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": Value::Object(params),
        })]
    }

    /// Frame `thread/compact/start` — compacts conversation history. Response is empty (`{}`);
    /// the actual compaction surfaces later via the `thread/compacted` notification. We still
    /// register a waiter so the control caller resolves on the (empty) ack.
    pub fn frame_compact(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "thread/compact/start", json!({}))
    }

    /// Frame `thread/rollback` — drop `num_turns` (>=1) turns from the END of history. ⚠️ This
    /// only edits thread history; it does NOT revert local file changes (client's job).
    /// Response: `{thread}`.
    pub fn frame_rollback(&mut self, request_id: &str, num_turns: u64) -> Vec<Value> {
        let n = num_turns.max(1);
        self.frame_tracked(request_id, "thread/rollback", json!({ "numTurns": n }))
    }

    /// Frame `thread/fork` — fork the current thread into a new one. Response carries the new
    /// thread at `result.thread.id`.
    pub fn frame_fork(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "thread/fork", json!({}))
    }

    /// Frame `thread/goal/set` — set/update the thread goal. Only the provided fields are sent.
    /// Response: `{goal: ThreadGoal}`.
    pub fn frame_goal_set(
        &mut self,
        request_id: &str,
        objective: Option<&str>,
        status: Option<&str>,
        token_budget: Option<u64>,
    ) -> Vec<Value> {
        let mut extra = serde_json::Map::new();
        if let Some(o) = objective {
            extra.insert("objective".into(), json!(o));
        }
        if let Some(s) = status {
            extra.insert("status".into(), json!(s));
        }
        if let Some(b) = token_budget {
            extra.insert("tokenBudget".into(), json!(b));
        }
        self.frame_tracked(request_id, "thread/goal/set", Value::Object(extra))
    }

    /// Frame `thread/goal/get` — read the current goal. Response: `{goal: ThreadGoal | null}`.
    pub fn frame_goal_get(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "thread/goal/get", json!({}))
    }

    /// Frame `thread/goal/clear` — clear the goal. Response: `{cleared: bool}`.
    pub fn frame_goal_clear(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "thread/goal/clear", json!({}))
    }
}

/// The request_id we surface for a server request = its stringified json-rpc id.
fn req_id_str(raw_id: &Value) -> String {
    match raw_id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Map an app sandbox mode string to the tagged `SandboxPolicy` OBJECT that `turn/start`'s
/// per-turn override expects (distinct from `thread/start`'s `sandbox: SandboxMode` STRING).
/// Mode strings come from `codex_sandbox_for` (commands/session.rs): "read-only",
/// "workspace-write", "danger-full-access". Unknown values fall back to workspace-write.
/// `writable_roots` populates the `workspaceWrite` policy's writable dirs (ignored by the other
/// modes, which have no such field).
fn sandbox_policy_value(mode: &str, writable_roots: &[String]) -> Value {
    match mode {
        "read-only" => json!({ "type": "readOnly", "networkAccess": false }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        // "workspace-write" (and any unknown mode) → the standard writable-workspace policy.
        _ => json!({
            "type": "workspaceWrite",
            "writableRoots": writable_roots,
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        }),
    }
}

impl SessionProtocol for CodexAppServer {
    fn startup_messages(&mut self, ctx: &StartupCtx) -> Vec<Value> {
        // Remember the writable dirs + spawn-time turn defaults so the first turn/start can
        // (re-)apply them. `thread/resume` carries none of model/effort/approval/sandbox, and
        // `thread/start` carries no effort — so this is the only way those reach a resumed
        // thread or pick up the effort setting.
        self.add_dirs = ctx.add_dirs.clone();
        self.startup_overrides = CodexTurnOverrides {
            approval_policy: ctx.approval_policy.clone(),
            sandbox: ctx.sandbox.clone(),
            model: ctx.model.clone(),
            effort: ctx.effort.clone(),
        };
        self.pending_startup_replay = true;

        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "opencovibe",
                    "version": env!("CARGO_PKG_VERSION"),
                    "title": "OpenCovibe"
                }
            }
        });

        let open = if let Some(tid) = &ctx.resume_thread_id {
            // Resume: the thread id is already known; readiness comes from the id:2 ack.
            self.thread_id = Some(tid.clone());
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "thread/resume",
                "params": { "threadId": tid }
            })
        } else {
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), json!(ctx.cwd));
            if let Some(v) = &ctx.approval_policy {
                params.insert("approvalPolicy".into(), json!(v));
            }
            if let Some(v) = &ctx.sandbox {
                params.insert("sandbox".into(), json!(v));
            }
            if let Some(v) = &ctx.model {
                params.insert("model".into(), json!(v));
            }
            if let Some(v) = &ctx.model_provider {
                params.insert("modelProvider".into(), json!(v));
            }
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "thread/start",
                "params": Value::Object(params)
            })
        };

        vec![initialize, open]
    }

    fn frame_user_turn(
        &mut self,
        text: &str,
        image_paths: &[String],
        overrides: &CodexTurnOverrides,
    ) -> Vec<Value> {
        let thread_id = match &self.thread_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] frame_user_turn before thread/started — dropping");
                return vec![];
            }
        };
        let mut input = vec![json!({
            "type": "text",
            "text": text,
            "text_elements": []
        })];
        for path in image_paths {
            input.push(json!({ "type": "localImage", "path": path }));
        }
        let id = self.next_id();
        // turn/start overrides apply "for this turn AND subsequent turns" — they persist
        // server-side, so we only need to inject a given override on the first turn after it
        // changes, but emitting it every turn is harmless and keeps the actor stateless here.
        //
        // On the FIRST turn we fall back to the spawn-time defaults (`startup_overrides`) for
        // any field the actor hasn't overridden. This carries model/effort/approval/sandbox onto
        // a resumed thread (`thread/resume` sends none of them) and applies the effort setting
        // (`thread/start` has no effort field). Live actor overrides always win.
        let first_turn = self.pending_startup_replay;
        self.pending_startup_replay = false;
        let pick = |actor: &Option<String>, startup: &Option<String>| -> Option<String> {
            actor
                .clone()
                .or_else(|| if first_turn { startup.clone() } else { None })
        };
        let approval_policy = pick(
            &overrides.approval_policy,
            &self.startup_overrides.approval_policy,
        );
        let sandbox = pick(&overrides.sandbox, &self.startup_overrides.sandbox);
        let model = pick(&overrides.model, &self.startup_overrides.model);
        let effort = pick(&overrides.effort, &self.startup_overrides.effort);

        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        params.insert("input".into(), Value::Array(input));
        if let Some(p) = &approval_policy {
            params.insert("approvalPolicy".into(), json!(p));
        }
        // Emit a sandboxPolicy when the sandbox is explicitly set, OR (first turn only) when we
        // have writable dirs to inject — `thread/start`'s bare `sandbox` string can't carry
        // `writableRoots`, so the workspace-write policy object is the only channel for add_dirs.
        match &sandbox {
            Some(s) => {
                params.insert(
                    "sandboxPolicy".into(),
                    sandbox_policy_value(s, &self.add_dirs),
                );
            }
            None if first_turn && !self.add_dirs.is_empty() => {
                // No explicit mode → the server default is workspace-write; build that policy so
                // the writable roots take effect.
                params.insert(
                    "sandboxPolicy".into(),
                    sandbox_policy_value("workspace-write", &self.add_dirs),
                );
            }
            None => {}
        }
        if let Some(m) = &model {
            params.insert("model".into(), json!(m));
        }
        if let Some(e) = &effort {
            params.insert("effort".into(), json!(e));
        }
        vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/start",
            "params": Value::Object(params)
        })]
    }

    fn frame_interrupt(&mut self) -> Vec<Value> {
        let thread_id = match &self.thread_id {
            Some(t) => t.clone(),
            None => return vec![],
        };
        let id = self.next_id();
        vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/interrupt",
            "params": { "threadId": thread_id }
        })]
    }

    fn frame_steer(&mut self, text: &str) -> Vec<Value> {
        let thread_id = match &self.thread_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] frame_steer before thread/started — dropping");
                return vec![];
            }
        };
        // turn/steer requires the active turn id as a precondition; without it the server
        // rejects the request. No active turn → nothing to steer into.
        let expected = match &self.active_turn_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] frame_steer with no active turn — dropping");
                return vec![];
            }
        };
        let id = self.next_id();
        vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/steer",
            "params": {
                "threadId": thread_id,
                "input": [{ "type": "text", "text": text, "text_elements": [] }],
                "expectedTurnId": expected,
            }
        })]
    }

    fn parse_line(&mut self, run_id: &str, line: &str) -> ParsedLine {
        let mut out = ParsedLine::default();
        let line = line.trim();
        if line.is_empty() {
            return out;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return out,
        };

        let method = msg.get("method").and_then(|v| v.as_str());
        let has_id = msg.get("id").is_some();

        // Server→client interactive REQUEST (has both method and id).
        if let (Some(method), true) = (method, has_id) {
            if is_interactive_method(method) {
                return self.handle_server_request(run_id, method, &msg);
            }
            // Some other server-initiated request we don't handle — ignore (no reply).
            return out;
        }

        // Server→client NOTIFICATION (method, no id).
        if let Some(method) = method {
            self.handle_notification(
                run_id,
                method,
                msg.get("params").unwrap_or(&Value::Null),
                &mut out,
            );
            return out;
        }

        // Reply to one of our data-returning requests (id present in client_waiters, no method):
        // thread/fork, thread/rollback, thread/goal/get, … Route the JSON-RPC result (or error)
        // back to the actor's control waiter keyed by the frontend request_id.
        if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
            if let Some(request_id) = self.client_waiters.remove(&id) {
                let value = msg
                    .get("result")
                    .cloned()
                    .or_else(|| msg.get("error").cloned())
                    .unwrap_or(Value::Null);
                out.control_response = Some((request_id, value));
                return out;
            }
        }

        // Reply to one of our client→server requests (id, no method). The id:2 reply is the
        // thread/start|resume ack. It carries `result.thread.id` for new threads — capture it
        // here so `thread_id` is set BEFORE we mark Ready (otherwise frame_user_turn fires with
        // no thread id and silently drops the first turn). thread/started also sets Ready.
        if msg.get("id").and_then(|v| v.as_i64()) == Some(2) && msg.get("error").is_none() {
            if self.thread_id.is_none() {
                if let Some(id) = msg
                    .get("result")
                    .and_then(|r| r.get("thread"))
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
                {
                    self.thread_id = Some(id.to_string());
                    out.thread_id = Some(id.to_string());
                }
            }
            self.phase = Phase::Ready;
        }
        out
    }

    fn frame_response(
        &mut self,
        kind: PendingKind,
        request_id: &str,
        response: Value,
    ) -> Vec<Value> {
        let pending = match self.pending.remove(request_id) {
            Some(p) => p,
            None => {
                log::warn!("[codex_appserver] frame_response: unknown request_id {request_id}");
                return vec![];
            }
        };
        let result = match kind {
            PendingKind::Permission => {
                // respond_permission sends {behavior: "allow"|"deny"} → Codex decision.
                let behavior = response
                    .get("behavior")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny");
                let decision = if behavior == "allow" {
                    "accept"
                } else {
                    "decline"
                };
                json!({ "decision": decision })
            }
            PendingKind::Elicitation => {
                // respond_elicitation sends {action, content?}.
                let action = response
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("decline");
                let mut r = serde_json::Map::new();
                r.insert("action".into(), json!(action));
                if let Some(content) = response.get("content") {
                    r.insert("content".into(), content.clone());
                }
                Value::Object(r)
            }
            PendingKind::UserInput => {
                // respond_user_input sends {answers: {qid: [labels...]}} → Codex shape
                // {answers: {qid: {answers: [labels...]}}}.
                build_user_input_result(&response)
            }
        };
        let _ = pending.method; // method retained for diagnostics/future shape variance.
        vec![json!({ "jsonrpc": "2.0", "id": pending.raw_id, "result": result })]
    }
}

fn is_interactive_method(method: &str) -> bool {
    matches!(
        method,
        M_CMD_APPROVAL
            | M_CMD_APPROVAL_LEGACY
            | M_FILE_APPROVAL
            | M_FILE_APPROVAL_LEGACY
            | M_PERM_APPROVAL
            | M_REQUEST_USER_INPUT
            | M_ELICITATION
    )
}

/// Convert the frontend's `{answers: {qid: [labels]}}` into Codex's
/// `ToolRequestUserInputResponse` `{answers: {qid: {answers: [labels]}}}`.
fn build_user_input_result(response: &Value) -> Value {
    let mut answers = serde_json::Map::new();
    if let Some(map) = response.get("answers").and_then(|v| v.as_object()) {
        for (qid, val) in map {
            // Accept either ["label", ...] or already-wrapped {answers:[...]}.
            let arr = if let Some(inner) = val.get("answers") {
                inner.clone()
            } else if val.is_array() {
                val.clone()
            } else {
                json!([val])
            };
            answers.insert(qid.clone(), json!({ "answers": arr }));
        }
    }
    json!({ "answers": Value::Object(answers) })
}

impl CodexAppServer {
    fn handle_server_request(&mut self, run_id: &str, method: &str, msg: &Value) -> ParsedLine {
        let mut out = ParsedLine::default();
        let raw_id = msg.get("id").cloned().unwrap_or(Value::Null);
        let request_id = req_id_str(&raw_id);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let (kind, events) = match method {
            M_CMD_APPROVAL | M_CMD_APPROVAL_LEGACY => (
                PendingKind::Permission,
                vec![approval_prompt(run_id, &request_id, "Bash", &params)],
            ),
            M_FILE_APPROVAL | M_FILE_APPROVAL_LEGACY => (
                PendingKind::Permission,
                vec![approval_prompt(run_id, &request_id, "Edit", &params)],
            ),
            M_PERM_APPROVAL => (
                PendingKind::Permission,
                vec![approval_prompt(run_id, &request_id, "Bash", &params)],
            ),
            M_REQUEST_USER_INPUT => (
                PendingKind::UserInput,
                ask_user_question_events(run_id, &request_id, &params),
            ),
            M_ELICITATION => (
                PendingKind::Elicitation,
                vec![elicitation_prompt(run_id, &request_id, &params)],
            ),
            _ => return out,
        };

        self.pending.insert(
            request_id.clone(),
            PendingServerReq {
                raw_id,
                method: method.to_string(),
            },
        );
        out.events = events;
        out.interactive = Some(PendingInteractive { request_id, kind });
        out
    }

    fn handle_notification(
        &mut self,
        run_id: &str,
        method: &str,
        params: &Value,
        out: &mut ParsedLine,
    ) {
        match method {
            "thread/started" => {
                if let Some(id) = params
                    .get("thread")
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
                {
                    self.thread_id = Some(id.to_string());
                    out.thread_id = Some(id.to_string());
                }
                self.phase = Phase::Ready;
            }
            "turn/started" => {
                // Capture the active turn id (TurnStartedNotification.turn.id) for turn/steer's
                // `expectedTurnId`. `params` is sometimes `{}` — tolerate the absence.
                if let Some(id) = params
                    .get("turn")
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
                {
                    self.active_turn_id = Some(id.to_string());
                }
                out.lifecycle = Some(LifecycleSignal::TurnStarted);
            }
            "turn/completed" => {
                self.active_turn_id = None;
                out.lifecycle = Some(LifecycleSignal::TurnCompleted);
            }
            "turn/failed" => {
                self.active_turn_id = None;
                let err = params
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                out.lifecycle = Some(LifecycleSignal::TurnFailed(err));
            }
            "error" => {
                // ErrorNotification = { error: TurnError, willRetry: bool, threadId, turnId }.
                // The message lives in `error.message` (a TurnError) — NOT a top-level `message`.
                // `willRetry: true` is a transient failure Codex auto-retries (e.g. a flaky
                // upstream connection); the SAME turn keeps running, so don't alarm the user
                // and — crucially — keep `active_turn_id` so a steer issued during the retry
                // window still targets the live turn.
                let err = params.get("error");
                let m = err
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .or_else(|| params.get("message").and_then(|v| v.as_str()))
                    .unwrap_or("unknown error");
                let will_retry = params
                    .get("willRetry")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if will_retry {
                    log::debug!("[codex] transient error (will retry): {m}");
                } else {
                    // Terminal error — the turn is over; drop the stale id so a later
                    // steer doesn't target a turn that's no longer running.
                    self.active_turn_id = None;
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content: format!("[error] {m}"),
                    });
                }
            }
            "item/agentMessage/delta" => {
                if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                    out.events.push(BusEvent::MessageDelta {
                        run_id: run_id.to_string(),
                        text: delta.to_string(),
                        parent_tool_use_id: None,
                    });
                }
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                    out.events.push(BusEvent::ThinkingDelta {
                        run_id: run_id.to_string(),
                        text: delta.to_string(),
                        parent_tool_use_id: None,
                    });
                }
            }
            "item/started" => {
                if let Some(item) = params.get("item") {
                    if let Some(ev) = item_started_event(run_id, item) {
                        out.events.push(ev);
                    }
                }
            }
            "item/completed" => {
                if let Some(item) = params.get("item") {
                    item_completed_events(run_id, item, &mut out.events);
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(ev) = token_usage_event(run_id, params) {
                    out.events.push(ev);
                }
            }
            // Live goal progress: params = {threadId, turnId?, goal: ThreadGoal}. Surface the
            // ThreadGoal verbatim so the panel can render objective/status/tokensUsed/timeUsed.
            "thread/goal/updated" => {
                if let Some(goal) = params.get("goal") {
                    out.events.push(BusEvent::GoalUpdate {
                        run_id: run_id.to_string(),
                        goal: goal.clone(),
                    });
                }
            }
            // Goal cleared server-side: emit a null goal so the panel collapses.
            "thread/goal/cleared" => {
                out.events.push(BusEvent::GoalUpdate {
                    run_id: run_id.to_string(),
                    goal: Value::Null,
                });
            }
            // Plan update → render as a TodoWrite card. A stable tool_use_id derived from the
            // turn id means repeated updates refresh the SAME card instead of stacking.
            "turn/plan/updated" => {
                plan_updated_events(run_id, params, &mut out.events);
            }
            // Live command output: append the chunk into the open Bash card (keyed by
            // itemId == ToolStart's tool_use_id). The final item/completed still carries the
            // authoritative aggregatedOutput, which overwrites the accumulation (no dup).
            "item/commandExecution/outputDelta" => {
                if let (Some(id), Some(delta)) = (
                    params.get("itemId").and_then(|v| v.as_str()),
                    params.get("delta").and_then(|v| v.as_str()),
                ) {
                    if !id.is_empty() && !delta.is_empty() {
                        out.events.push(BusEvent::ToolOutputDelta {
                            run_id: run_id.to_string(),
                            tool_use_id: id.to_string(),
                            delta: delta.to_string(),
                            parent_tool_use_id: None,
                        });
                    }
                }
            }
            // Account rate limits → map the primary window to the existing RateLimitEvent.
            "account/rateLimits/updated" => {
                if let Some(ev) = rate_limit_event(run_id, params) {
                    out.events.push(ev);
                }
            }
            // Model reroute / warnings → concise one-line notices via CommandOutput.
            "model/rerouted" => {
                let from = params
                    .get("fromModel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let to = params
                    .get("toModel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                let content = if reason.is_empty() {
                    format!("[notice] model rerouted: {from} → {to}")
                } else {
                    format!("[notice] model rerouted: {from} → {to} ({reason})")
                };
                out.events.push(BusEvent::CommandOutput {
                    run_id: run_id.to_string(),
                    content,
                });
            }
            "warning" => {
                if let Some(msg) = params.get("message").and_then(|v| v.as_str()) {
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content: format!("[notice] {msg}"),
                    });
                }
            }
            "deprecationNotice" | "configWarning" => {
                // Both carry {summary, details?}.
                if let Some(summary) = params.get("summary").and_then(|v| v.as_str()) {
                    let details = params.get("details").and_then(|v| v.as_str());
                    let content = match details {
                        Some(d) if !d.is_empty() => format!("[notice] {summary}: {d}"),
                        _ => format!("[notice] {summary}"),
                    };
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content,
                    });
                }
            }
            // TODO(wave1): diff panel — surface turn/diff/updated as a reviewable diff view.
            "turn/diff/updated" => {}
            _ => {} // process/* deltas, realtime, fs, status — ignored in v1.
        }
    }
}

// ── ServerRequest → interactive BusEvent ──────────────────────────────────────────────

fn approval_prompt(run_id: &str, request_id: &str, tool_name: &str, params: &Value) -> BusEvent {
    let command = params
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let cwd = params.get("cwd").cloned();
    let reason = params
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = params
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or(request_id)
        .to_string();

    let mut input = serde_json::Map::new();
    if let Some(c) = command {
        input.insert("command".into(), json!(c));
    }
    if let Some(c) = cwd {
        input.insert("cwd".into(), c);
    }
    // Carry the file-change patch through verbatim if present.
    if let Some(changes) = params.get("changes") {
        input.insert("changes".into(), changes.clone());
    }

    BusEvent::PermissionPrompt {
        run_id: run_id.to_string(),
        request_id: request_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_use_id: item_id,
        tool_input: Value::Object(input),
        decision_reason: reason,
        parent_tool_use_id: None,
        suggestions: vec![],
    }
}

/// Map `item/tool/requestUserInput` to an AskUserQuestion tool (ToolStart + ToolEnd) so it
/// renders in the existing `InlineToolCard`. `tool_use_id == request_id` so the frontend can
/// route the answer back via `respond_user_input`.
fn ask_user_question_events(run_id: &str, request_id: &str, params: &Value) -> Vec<BusEvent> {
    let mut questions = vec![];
    if let Some(arr) = params.get("questions").and_then(|v| v.as_array()) {
        for q in arr {
            let header = q.get("header").and_then(|v| v.as_str()).unwrap_or("");
            let question = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
            let qid = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let mut options = vec![];
            if let Some(opts) = q.get("options").and_then(|v| v.as_array()) {
                for o in opts {
                    options.push(json!({
                        "label": o.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": o.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    }));
                }
            }
            questions.push(json!({
                "id": qid,
                "header": header,
                "question": question,
                "options": options,
                "multiSelect": false,
            }));
        }
    }
    let input = json!({ "questions": questions });

    vec![
        BusEvent::ToolStart {
            run_id: run_id.to_string(),
            tool_use_id: request_id.to_string(),
            tool_name: "AskUserQuestion".to_string(),
            input: input.clone(),
            parent_tool_use_id: None,
        },
        BusEvent::ToolEnd {
            run_id: run_id.to_string(),
            tool_use_id: request_id.to_string(),
            tool_name: "AskUserQuestion".to_string(),
            output: input,
            // "error" status is what the store maps to `ask_pending` for AskUserQuestion —
            // that's the state that renders the interactive option buttons (InlineToolCard).
            status: "error".to_string(),
            duration_ms: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        },
    ]
}

fn elicitation_prompt(run_id: &str, request_id: &str, params: &Value) -> BusEvent {
    let server_name = params
        .get("serverName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let message = params
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let requested_schema = params.get("requestedSchema").cloned();
    let elicitation_id = params
        .get("elicitationId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    BusEvent::ElicitationPrompt {
        run_id: run_id.to_string(),
        request_id: request_id.to_string(),
        mcp_server_name: server_name,
        message,
        elicitation_id,
        mode,
        url,
        requested_schema,
    }
}

// ── item.* → tool/message BusEvents ──────────────────────────────────────────────────

/// Map an app-server item to its tool name. Classification is shared with the exec parser via
/// `CodexToolKind`; the `mcpToolCall` name uses this transport's own field defaults (server "mcp",
/// tool "tool" — the exec transport defaults tool to "unknown").
fn item_tool_name(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match CodexToolKind::from_item_type(item_type)? {
        CodexToolKind::McpToolCall => {
            let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("mcp");
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
            Some(format!("{server}:{tool}"))
        }
        // `collabToolCall` IS reachable over app-server (multi-agent / spawn_agent sessions), but
        // this transport's `item_started_event` only copies a `command` field — it has none of the
        // collab fields (tool/prompt/agents_states), so rendering it would yield an empty Agent
        // card. The app-server path has never rendered collab items; preserve that (return None)
        // until the collab fields are properly extracted. The exec parser DOES render them.
        CodexToolKind::CollabToolCall => None,
        kind => kind.fixed_tool_name().map(|s| s.to_string()),
    }
}

fn item_started_event(run_id: &str, item: &Value) -> Option<BusEvent> {
    let tool_name = item_tool_name(item)?;
    let id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut input = serde_json::Map::new();
    if let Some(cmd) = item.get("command").and_then(|v| v.as_str()) {
        input.insert("command".into(), json!(cmd));
    }
    Some(BusEvent::ToolStart {
        run_id: run_id.to_string(),
        tool_use_id: id,
        tool_name,
        input: Value::Object(input),
        parent_tool_use_id: None,
    })
}

fn item_completed_events(run_id: &str, item: &Value, out: &mut Vec<BusEvent>) {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if item_type == "agentMessage" {
        let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(BusEvent::MessageComplete {
            run_id: run_id.to_string(),
            message_id: id,
            text: text.to_string(),
            parent_tool_use_id: None,
            model: None,
            stop_reason: None,
            message_usage: None,
        });
        return;
    }
    if item_type == "userMessage" || item_type == "reasoning" {
        return; // user echo / reasoning already streamed via deltas
    }
    if let Some(tool_name) = item_tool_name(item) {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let output = item
            .get("aggregatedOutput")
            .or_else(|| item.get("aggregated_output"))
            .or_else(|| item.get("output"))
            .or_else(|| item.get("changes"))
            .cloned()
            .unwrap_or(Value::Null);
        let status =
            codex_normalize_status(item.get("status").and_then(|v| v.as_str()).unwrap_or(""));
        out.push(BusEvent::ToolEnd {
            run_id: run_id.to_string(),
            tool_use_id: id,
            tool_name,
            output: json!({ "content": output }),
            status: status.to_string(),
            duration_ms: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        });
    }
}

fn token_usage_event(run_id: &str, params: &Value) -> Option<BusEvent> {
    let total = params.get("tokenUsage").and_then(|t| t.get("total"))?;
    let input = total
        .get("inputTokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = total
        .get("outputTokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached = total.get("cachedInputTokens").and_then(|v| v.as_u64());
    Some(BusEvent::UsageUpdate {
        run_id: run_id.to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cached,
        cache_write_tokens: None,
        total_cost_usd: 0.0,
        turn_index: None,
        model_usage: None,
        duration_api_ms: None,
        duration_ms: None,
        num_turns: None,
        stop_reason: None,
        service_tier: None,
        speed: None,
        web_fetch_requests: None,
        cache_creation_5m: None,
        cache_creation_1h: None,
    })
}

/// Map `turn/plan/updated` to a TodoWrite ToolStart+ToolEnd pair so the plan renders in the
/// existing TodoWrite card. Reuses pipe_parser's `newTodos` shape: `{content, status,
/// activeForm}` with status one of pending|in_progress|completed. The tool_use_id is derived
/// from the turn id so repeated plan updates refresh the SAME card instead of stacking.
fn plan_updated_events(run_id: &str, params: &Value, out: &mut Vec<BusEvent>) {
    let turn_id = params
        .get("turnId")
        .and_then(|v| v.as_str())
        .unwrap_or("turn");
    let tool_use_id = format!("codex-plan-{turn_id}");

    let new_todos: Vec<Value> = params
        .get("plan")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|step| {
                    let content = step.get("step").and_then(|v| v.as_str()).unwrap_or("");
                    // TurnPlanStepStatus: "pending" | "inProgress" | "completed".
                    let status = match step.get("status").and_then(|v| v.as_str()) {
                        Some("inProgress") => "in_progress",
                        Some("completed") => "completed",
                        _ => "pending",
                    };
                    json!({
                        "content": content,
                        "status": status,
                        "activeForm": content,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let new_todos = Value::Array(new_todos);

    out.push(BusEvent::ToolStart {
        run_id: run_id.to_string(),
        tool_use_id: tool_use_id.clone(),
        tool_name: "TodoWrite".to_string(),
        input: json!({ "todos": new_todos }),
        parent_tool_use_id: None,
    });
    out.push(BusEvent::ToolEnd {
        run_id: run_id.to_string(),
        tool_use_id,
        tool_name: "TodoWrite".to_string(),
        output: json!({}),
        status: "success".to_string(),
        duration_ms: None,
        parent_tool_use_id: None,
        tool_use_result: Some(json!({ "newTodos": new_todos })),
    });
}

/// Map `account/rateLimits/updated` to the existing RateLimitEvent. Codex reports per-window
/// `usedPercent` (0–100) on a `primary`/`secondary` snapshot; we surface the primary window.
/// `utilization` is normalized to 0–1 to match the Claude rate_limit_event contract.
fn rate_limit_event(run_id: &str, params: &Value) -> Option<BusEvent> {
    let limits = params.get("rateLimits")?;
    // Prefer the primary window; fall back to secondary if primary is absent.
    let window = limits
        .get("primary")
        .filter(|v| !v.is_null())
        .or_else(|| limits.get("secondary").filter(|v| !v.is_null()))?;
    let used_percent = window.get("usedPercent").and_then(|v| v.as_f64());
    let utilization = used_percent.map(|p| p / 100.0);
    let resets_at = window.get("resetsAt").and_then(|v| v.as_f64());
    // Derive a label from the window duration (minutes) when present.
    let rate_limit_type = window
        .get("windowDurationMins")
        .and_then(|v| v.as_u64())
        .map(|m| format!("{m}_min"));
    let status = match utilization {
        Some(u) if u >= 1.0 => "rejected",
        Some(u) if u >= 0.8 => "allowed_warning",
        _ => "allowed",
    }
    .to_string();
    Some(BusEvent::RateLimitEvent {
        run_id: run_id.to_string(),
        status,
        resets_at,
        rate_limit_type,
        utilization,
        data: limits.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_server() -> CodexAppServer {
        let mut s = CodexAppServer::new();
        s.parse_line(
            "run1",
            r#"{"method":"thread/started","params":{"thread":{"id":"th-123"}}}"#,
        );
        s
    }

    /// Default (no-op) overrides for the common case.
    fn no_overrides() -> CodexTurnOverrides {
        CodexTurnOverrides::default()
    }

    #[test]
    fn startup_new_thread() {
        let mut s = CodexAppServer::new();
        let msgs = s.startup_messages(&StartupCtx {
            cwd: "/tmp/x".into(),
            approval_policy: Some("on-request".into()),
            sandbox: Some("workspace-write".into()),
            ..Default::default()
        });
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["method"], "initialize");
        assert_eq!(msgs[1]["method"], "thread/start");
        assert_eq!(msgs[1]["params"]["cwd"], "/tmp/x");
        assert_eq!(msgs[1]["params"]["approvalPolicy"], "on-request");
    }

    #[test]
    fn startup_resume() {
        let mut s = CodexAppServer::new();
        let msgs = s.startup_messages(&StartupCtx {
            resume_thread_id: Some("th-9".into()),
            ..Default::default()
        });
        assert_eq!(msgs[1]["method"], "thread/resume");
        assert_eq!(msgs[1]["params"]["threadId"], "th-9");
    }

    #[test]
    fn thread_started_captures_id_and_readies() {
        let mut s = CodexAppServer::new();
        let out = s.parse_line(
            "run1",
            r#"{"method":"thread/started","params":{"thread":{"id":"th-123"}}}"#,
        );
        assert_eq!(out.thread_id.as_deref(), Some("th-123"));
        assert_eq!(s.phase, Phase::Ready);
    }

    #[test]
    fn user_turn_requires_thread_id() {
        let mut s = CodexAppServer::new();
        assert!(s.frame_user_turn("hi", &[], &no_overrides()).is_empty());
        let mut s = ready_server();
        let msgs = s.frame_user_turn("hi", &[], &no_overrides());
        assert_eq!(msgs[0]["method"], "turn/start");
        assert_eq!(msgs[0]["params"]["threadId"], "th-123");
        assert_eq!(msgs[0]["params"]["input"][0]["text"], "hi");
        // No overrides → none of the optional override keys are present.
        assert!(msgs[0]["params"].get("approvalPolicy").is_none());
        assert!(msgs[0]["params"].get("sandboxPolicy").is_none());
        assert!(msgs[0]["params"].get("model").is_none());
        assert!(msgs[0]["params"].get("effort").is_none());
    }

    #[test]
    fn user_turn_attaches_local_images() {
        let mut s = ready_server();
        let msgs = s.frame_user_turn("describe this", &["/x/a.png".to_string()], &no_overrides());
        let input = &msgs[0]["params"]["input"];
        // text first, then one localImage item per path.
        assert_eq!(input[0]["type"], "text");
        assert_eq!(input[0]["text"], "describe this");
        assert_eq!(input[1]["type"], "localImage");
        assert_eq!(input[1]["path"], "/x/a.png");
        // No images → no localImage items.
        let none = s.frame_user_turn("hi", &[], &no_overrides());
        assert_eq!(none[0]["params"]["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn user_turn_injects_overrides_when_set() {
        let mut s = ready_server();
        let overrides = CodexTurnOverrides {
            approval_policy: Some("never".into()),
            sandbox: Some("danger-full-access".into()),
            model: Some("gpt-5-codex".into()),
            effort: Some("high".into()),
        };
        let msgs = s.frame_user_turn("go", &[], &overrides);
        let params = &msgs[0]["params"];
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
        assert_eq!(params["model"], "gpt-5-codex");
        assert_eq!(params["effort"], "high");
        // Partial overrides: only the Some fields appear.
        let partial = CodexTurnOverrides {
            model: Some("gpt-5".into()),
            ..Default::default()
        };
        let msgs = s.frame_user_turn("go", &[], &partial);
        assert_eq!(msgs[0]["params"]["model"], "gpt-5");
        assert!(msgs[0]["params"].get("approvalPolicy").is_none());
        assert!(msgs[0]["params"].get("sandboxPolicy").is_none());
        assert!(msgs[0]["params"].get("effort").is_none());
    }

    #[test]
    fn first_turn_replays_startup_defaults_only_once() {
        // A resumed thread (thread/resume carries no model/effort/approval/sandbox) must have
        // the spawn-time defaults re-applied on the first turn — then NOT re-sent afterwards.
        let mut s = CodexAppServer::new();
        s.startup_messages(&StartupCtx {
            resume_thread_id: Some("th-r".into()),
            model: Some("gpt-5-codex".into()),
            approval_policy: Some("on-request".into()),
            sandbox: Some("workspace-write".into()),
            effort: Some("high".into()),
            ..Default::default()
        });
        // thread/resume ack readies the session.
        s.parse_line("r", r#"{"id":2,"result":{}}"#);

        let first = s.frame_user_turn("hi", &[], &no_overrides());
        let p = &first[0]["params"];
        assert_eq!(p["model"], "gpt-5-codex");
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["effort"], "high");
        assert_eq!(p["sandboxPolicy"]["type"], "workspaceWrite");

        // Second turn: defaults already persisted server-side → not re-sent.
        let second = s.frame_user_turn("again", &[], &no_overrides());
        let p2 = &second[0]["params"];
        assert!(p2.get("model").is_none());
        assert!(p2.get("approvalPolicy").is_none());
        assert!(p2.get("effort").is_none());
        assert!(p2.get("sandboxPolicy").is_none());
    }

    #[test]
    fn actor_override_wins_over_startup_default_on_first_turn() {
        let mut s = CodexAppServer::new();
        s.startup_messages(&StartupCtx {
            resume_thread_id: Some("th-r".into()),
            model: Some("gpt-5-codex".into()),
            ..Default::default()
        });
        s.parse_line("r", r#"{"id":2,"result":{}}"#);
        let overrides = CodexTurnOverrides {
            model: Some("o3".into()),
            ..Default::default()
        };
        let msgs = s.frame_user_turn("hi", &[], &overrides);
        assert_eq!(msgs[0]["params"]["model"], "o3");
    }

    #[test]
    fn add_dirs_populate_writable_roots_on_first_turn() {
        let mut s = CodexAppServer::new();
        s.startup_messages(&StartupCtx {
            cwd: "/work".into(),
            sandbox: Some("workspace-write".into()),
            add_dirs: vec!["/extra/a".into(), "/extra/b".into()],
            ..Default::default()
        });
        s.parse_line("r", r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#);
        let msgs = s.frame_user_turn("go", &[], &no_overrides());
        let policy = &msgs[0]["params"]["sandboxPolicy"];
        assert_eq!(policy["type"], "workspaceWrite");
        assert_eq!(policy["writableRoots"][0], "/extra/a");
        assert_eq!(policy["writableRoots"][1], "/extra/b");
    }

    #[test]
    fn add_dirs_emit_workspace_policy_when_sandbox_unset() {
        // No explicit sandbox at spawn → server default is workspace-write. The writable dirs
        // still need a policy object (the bare thread/start `sandbox` string can't carry them).
        let mut s = CodexAppServer::new();
        s.startup_messages(&StartupCtx {
            cwd: "/work".into(),
            add_dirs: vec!["/extra".into()],
            ..Default::default()
        });
        s.parse_line("r", r#"{"id":2,"result":{"thread":{"id":"th-1"}}}"#);
        let msgs = s.frame_user_turn("go", &[], &no_overrides());
        let policy = &msgs[0]["params"]["sandboxPolicy"];
        assert_eq!(policy["type"], "workspaceWrite");
        assert_eq!(policy["writableRoots"][0], "/extra");
        // No add_dirs and no sandbox → no policy at all.
        let mut s2 = ready_server();
        let m2 = s2.frame_user_turn("go", &[], &no_overrides());
        assert!(m2[0]["params"].get("sandboxPolicy").is_none());
    }

    #[test]
    fn sandbox_policy_value_mapping() {
        assert_eq!(sandbox_policy_value("read-only", &[])["type"], "readOnly");
        assert_eq!(
            sandbox_policy_value("read-only", &[])["networkAccess"],
            false
        );
        assert_eq!(
            sandbox_policy_value("danger-full-access", &[])["type"],
            "dangerFullAccess"
        );
        let ws = sandbox_policy_value("workspace-write", &[]);
        assert_eq!(ws["type"], "workspaceWrite");
        assert_eq!(ws["writableRoots"], json!([]));
        assert_eq!(ws["networkAccess"], false);
        assert_eq!(ws["excludeTmpdirEnvVar"], false);
        assert_eq!(ws["excludeSlashTmp"], false);
        // Unknown mode falls back to workspace-write.
        assert_eq!(sandbox_policy_value("bogus", &[])["type"], "workspaceWrite");
        // Writable roots populate the workspace-write policy (and are ignored elsewhere).
        let roots = vec!["/extra/a".to_string(), "/extra/b".to_string()];
        let ws2 = sandbox_policy_value("workspace-write", &roots);
        assert_eq!(ws2["writableRoots"][0], "/extra/a");
        assert_eq!(ws2["writableRoots"][1], "/extra/b");
        assert!(sandbox_policy_value("read-only", &roots)
            .get("writableRoots")
            .is_none());
    }

    #[test]
    fn steer_carries_expected_turn_id() {
        let mut s = ready_server();
        // No active turn yet → frame_steer drops (server would reject without expectedTurnId).
        assert!(s.frame_steer("hint").is_empty());
        // Capture the active turn id from turn/started.
        s.parse_line(
            "r",
            r#"{"method":"turn/started","params":{"turn":{"id":"turn-42"}}}"#,
        );
        let msgs = s.frame_steer("focus on tests");
        assert_eq!(msgs[0]["method"], "turn/steer");
        assert_eq!(msgs[0]["params"]["threadId"], "th-123");
        assert_eq!(msgs[0]["params"]["expectedTurnId"], "turn-42");
        assert_eq!(msgs[0]["params"]["input"][0]["type"], "text");
        assert_eq!(msgs[0]["params"]["input"][0]["text"], "focus on tests");
        // After completion the turn id clears → steer drops again.
        s.parse_line("r", r#"{"method":"turn/completed","params":{}}"#);
        assert!(s.frame_steer("late").is_empty());
    }

    #[test]
    fn active_turn_id_capture_and_clear() {
        let mut s = ready_server();
        assert!(s.active_turn_id.is_none());
        // turn/started with turn.id → captured.
        s.parse_line(
            "r",
            r#"{"method":"turn/started","params":{"turn":{"id":"t-1"}}}"#,
        );
        assert_eq!(s.active_turn_id.as_deref(), Some("t-1"));
        // turn/completed clears it.
        s.parse_line("r", r#"{"method":"turn/completed","params":{}}"#);
        assert!(s.active_turn_id.is_none());
        // turn/failed clears it.
        s.parse_line(
            "r",
            r#"{"method":"turn/started","params":{"turn":{"id":"t-2"}}}"#,
        );
        assert_eq!(s.active_turn_id.as_deref(), Some("t-2"));
        s.parse_line(
            "r",
            r#"{"method":"turn/failed","params":{"error":{"message":"boom"}}}"#,
        );
        assert!(s.active_turn_id.is_none());
        // top-level error clears it.
        s.parse_line(
            "r",
            r#"{"method":"turn/started","params":{"turn":{"id":"t-3"}}}"#,
        );
        assert_eq!(s.active_turn_id.as_deref(), Some("t-3"));
        s.parse_line(
            "r",
            r#"{"method":"error","params":{"error":{"message":"x"},"willRetry":false}}"#,
        );
        assert!(s.active_turn_id.is_none());
        // turn/started with empty params (params == {}) must not panic and leaves id unset.
        let mut s2 = ready_server();
        s2.parse_line("r", r#"{"method":"turn/started","params":{}}"#);
        assert!(s2.active_turn_id.is_none());
    }

    #[test]
    fn agent_message_delta_to_message_delta() {
        let mut s = ready_server();
        let out = s.parse_line(
            "run1",
            r#"{"method":"item/agentMessage/delta","params":{"delta":"hello"}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::MessageDelta { text, .. } => assert_eq!(text, "hello"),
            e => panic!("expected MessageDelta, got {e:?}"),
        }
    }

    #[test]
    fn turn_lifecycle() {
        let mut s = ready_server();
        assert_eq!(
            s.parse_line("r", r#"{"method":"turn/started","params":{}}"#)
                .lifecycle,
            Some(LifecycleSignal::TurnStarted)
        );
        assert_eq!(
            s.parse_line("r", r#"{"method":"turn/completed","params":{}}"#)
                .lifecycle,
            Some(LifecycleSignal::TurnCompleted)
        );
    }

    #[test]
    fn command_approval_request() {
        let mut s = ready_server();
        let line = r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{"itemId":"call_1","reason":"allow write?","command":"echo hi","cwd":"/tmp"}}"#;
        let out = s.parse_line("run1", line);
        let pi = out.interactive.expect("interactive");
        assert_eq!(pi.kind, PendingKind::Permission);
        assert_eq!(pi.request_id, "0");
        match &out.events[0] {
            BusEvent::PermissionPrompt {
                tool_name,
                tool_input,
                decision_reason,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input["command"], "echo hi");
                assert_eq!(decision_reason, "allow write?");
            }
            e => panic!("expected PermissionPrompt, got {e:?}"),
        }
        // Allow → {decision:"accept"} on id 0.
        let resp = s.frame_response(PendingKind::Permission, "0", json!({"behavior":"allow"}));
        assert_eq!(resp[0]["id"], 0);
        assert_eq!(resp[0]["result"]["decision"], "accept");
        // Deny path.
        let mut s2 = ready_server();
        s2.parse_line("run1", line);
        let resp2 = s2.frame_response(PendingKind::Permission, "0", json!({"behavior":"deny"}));
        assert_eq!(resp2[0]["result"]["decision"], "decline");
    }

    #[test]
    fn request_user_input_to_ask_question_and_back() {
        let mut s = ready_server();
        let line = r#"{"id":0,"method":"item/tool/requestUserInput","params":{"questions":[{"id":"word","header":"Word","question":"Which word?","isOther":true,"isSecret":false,"options":[{"label":"FOO","description":"Select FOO."},{"label":"BAR","description":"Select BAR."}]}]}}"#;
        let out = s.parse_line("run1", line);
        let pi = out.interactive.expect("interactive");
        assert_eq!(pi.kind, PendingKind::UserInput);
        // Renders as AskUserQuestion ToolStart+ToolEnd with tool_use_id == request_id.
        match &out.events[0] {
            BusEvent::ToolStart {
                tool_name,
                tool_use_id,
                input,
                ..
            } => {
                assert_eq!(tool_name, "AskUserQuestion");
                assert_eq!(tool_use_id, "0");
                assert_eq!(input["questions"][0]["header"], "Word");
                assert_eq!(input["questions"][0]["options"][0]["label"], "FOO");
            }
            e => panic!("expected ToolStart, got {e:?}"),
        }
        // ToolEnd must carry status "error" → store maps AskUserQuestion to ask_pending.
        match &out.events[1] {
            BusEvent::ToolEnd { status, .. } => assert_eq!(status, "error"),
            e => panic!("expected ToolEnd, got {e:?}"),
        }
        // Answer "FOO" → Codex map shape {answers:{word:{answers:["FOO"]}}} on id 0.
        let resp = s.frame_response(
            PendingKind::UserInput,
            "0",
            json!({"answers": {"word": ["FOO"]}}),
        );
        assert_eq!(resp[0]["id"], 0);
        assert_eq!(resp[0]["result"]["answers"]["word"]["answers"][0], "FOO");
    }

    #[test]
    fn elicitation_request() {
        let mut s = ready_server();
        let line = r#"{"id":1,"method":"mcpServer/elicitation/request","params":{"serverName":"srv","mode":"form","message":"Pick","requestedSchema":{"type":"object"}}}"#;
        let out = s.parse_line("run1", line);
        assert_eq!(out.interactive.unwrap().kind, PendingKind::Elicitation);
        match &out.events[0] {
            BusEvent::ElicitationPrompt {
                mcp_server_name,
                message,
                mode,
                ..
            } => {
                assert_eq!(mcp_server_name, "srv");
                assert_eq!(message, "Pick");
                assert_eq!(mode.as_deref(), Some("form"));
            }
            e => panic!("expected ElicitationPrompt, got {e:?}"),
        }
        let resp = s.frame_response(PendingKind::Elicitation, "1", json!({"action":"decline"}));
        assert_eq!(resp[0]["result"]["action"], "decline");
    }

    #[test]
    fn command_item_lifecycle() {
        let mut s = ready_server();
        let started = s.parse_line(
            "r",
            r#"{"method":"item/started","params":{"item":{"id":"call_1","type":"commandExecution","command":"ls"}}}"#,
        );
        assert!(matches!(started.events[0], BusEvent::ToolStart { .. }));
        let completed = s.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"item":{"id":"call_1","type":"commandExecution","status":"completed","aggregatedOutput":"file.txt"}}}"#,
        );
        match &completed.events[0] {
            BusEvent::ToolEnd {
                tool_name, status, ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(status, "success");
            }
            e => panic!("expected ToolEnd, got {e:?}"),
        }
    }

    #[test]
    fn collab_tool_call_emits_no_card_over_app_server() {
        // Regression: app-server has never rendered collabToolCall items (item_started only copies
        // a `command` field, which collab lacks → an empty Agent card). The shared CodexToolKind
        // classifier knows collab → Agent, but this transport must keep emitting NOTHING for it
        // until the collab fields are extracted. (The exec parser DOES render collab — separate.)
        let mut s = ready_server();
        let started = s.parse_line(
            "r",
            r#"{"method":"item/started","params":{"item":{"id":"col_1","type":"collabToolCall","tool":"code_review","prompt":"review"}}}"#,
        );
        assert!(
            started.events.is_empty(),
            "collabToolCall must emit no ToolStart over app-server, got {:?}",
            started.events
        );
        let completed = s.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"item":{"id":"col_1","type":"collabToolCall","status":"completed","agents_states":{}}}}"#,
        );
        assert!(
            completed.events.is_empty(),
            "collabToolCall must emit no ToolEnd over app-server, got {:?}",
            completed.events
        );
    }

    #[test]
    fn mcp_tool_name_default_diverges_from_exec_intentionally() {
        // The two transports use DIFFERENT defaults for a missing `tool` field on an MCP item:
        // app-server → "tool", exec (pipe_parser) → "unknown". This is intentional; lock it so a
        // future "cleanup" can't silently unify them. (Pair: pipe_parser::tests covers the exec side.)
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"item/started","params":{"item":{"id":"m_1","type":"mcpToolCall","server":"fs"}}}"#,
        );
        match &out.events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "fs:tool"),
            e => panic!("expected ToolStart, got {e:?}"),
        }
        // Both fields present → "{server}:{tool}".
        let out2 = s.parse_line(
            "r",
            r#"{"method":"item/started","params":{"item":{"id":"m_2","type":"mcpToolCall","server":"fs","tool":"read"}}}"#,
        );
        match &out2.events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "fs:read"),
            e => panic!("expected ToolStart, got {e:?}"),
        }
    }

    #[test]
    fn token_usage() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":100,"outputTokens":20,"cachedInputTokens":80}}}}"#,
        );
        match &out.events[0] {
            BusEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 20);
                assert_eq!(*cache_read_tokens, Some(80));
            }
            e => panic!("expected UsageUpdate, got {e:?}"),
        }
    }

    #[test]
    fn plan_updated_maps_to_todowrite() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"turn/plan/updated","params":{"turnId":"t-1","explanation":"go","plan":[
                {"step":"do x","status":"completed"},
                {"step":"do y","status":"inProgress"},
                {"step":"do z","status":"pending"}
            ]}}"#,
        );
        assert_eq!(out.events.len(), 2);
        match &out.events[0] {
            BusEvent::ToolStart {
                tool_name,
                tool_use_id,
                input,
                ..
            } => {
                assert_eq!(tool_name, "TodoWrite");
                assert_eq!(tool_use_id, "codex-plan-t-1");
                let todos = input["todos"].as_array().unwrap();
                assert_eq!(todos.len(), 3);
                assert_eq!(todos[0]["content"], "do x");
                assert_eq!(todos[0]["status"], "completed");
                assert_eq!(todos[1]["status"], "in_progress");
                assert_eq!(todos[2]["status"], "pending");
            }
            e => panic!("expected ToolStart, got {e:?}"),
        }
        match &out.events[1] {
            BusEvent::ToolEnd {
                tool_name,
                tool_use_id,
                status,
                tool_use_result,
                ..
            } => {
                assert_eq!(tool_name, "TodoWrite");
                assert_eq!(tool_use_id, "codex-plan-t-1");
                assert_eq!(status, "success");
                let todos = tool_use_result.as_ref().unwrap()["newTodos"]
                    .as_array()
                    .unwrap();
                assert_eq!(todos.len(), 3);
                assert_eq!(todos[1]["content"], "do y");
            }
            e => panic!("expected ToolEnd, got {e:?}"),
        }
    }

    #[test]
    fn command_output_delta_to_tool_output_delta() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"call_1","delta":"line 1\n"}}"#,
        );
        match &out.events[0] {
            BusEvent::ToolOutputDelta {
                tool_use_id, delta, ..
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert_eq!(delta, "line 1\n");
            }
            e => panic!("expected ToolOutputDelta, got {e:?}"),
        }
        // Empty itemId or empty delta → no event (can't key into a card / nothing to append).
        let out = s.parse_line(
            "r",
            r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"","delta":"x"}}"#,
        );
        assert!(out.events.is_empty());
        let out = s.parse_line(
            "r",
            r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"call_1","delta":""}}"#,
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn rate_limits_updated_to_rate_limit_event() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"account/rateLimits/updated","params":{"rateLimits":{
                "primary":{"usedPercent":85.0,"windowDurationMins":300,"resetsAt":1711900000},
                "secondary":null,"planType":"pro"
            }}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::RateLimitEvent {
                status,
                utilization,
                resets_at,
                rate_limit_type,
                ..
            } => {
                assert_eq!(status, "allowed_warning");
                assert!((utilization.unwrap() - 0.85).abs() < 0.001);
                assert!((resets_at.unwrap() - 1711900000.0).abs() < 0.1);
                assert_eq!(rate_limit_type.as_deref(), Some("300_min"));
            }
            e => panic!("expected RateLimitEvent, got {e:?}"),
        }
    }

    #[test]
    fn rate_limits_falls_back_to_secondary() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"account/rateLimits/updated","params":{"rateLimits":{
                "primary":null,
                "secondary":{"usedPercent":100.0,"windowDurationMins":10080,"resetsAt":1712000000}
            }}}"#,
        );
        match &out.events[0] {
            BusEvent::RateLimitEvent {
                status,
                utilization,
                ..
            } => {
                assert_eq!(status, "rejected");
                assert!((utilization.unwrap() - 1.0).abs() < 0.001);
            }
            e => panic!("expected RateLimitEvent, got {e:?}"),
        }
    }

    #[test]
    fn model_rerouted_to_notice() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"model/rerouted","params":{"fromModel":"gpt-5","toModel":"gpt-5-safe","reason":"highRiskCyberActivity"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(
                    content,
                    "[notice] model rerouted: gpt-5 → gpt-5-safe (highRiskCyberActivity)"
                );
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    #[test]
    fn error_notification_extracts_message_and_respects_will_retry() {
        let mut s = ready_server();
        // Terminal error: message is nested under `error.message` (TurnError), surfaced.
        let out = s.parse_line(
            "r",
            r#"{"method":"error","params":{"error":{"message":"model overloaded"},"willRetry":false,"threadId":"t","turnId":"u"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[error] model overloaded")
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        // Transient error (willRetry): Codex auto-retries → no user-facing event.
        let out = s.parse_line(
            "r",
            r#"{"method":"error","params":{"error":{"message":"tls handshake eof"},"willRetry":true,"threadId":"t","turnId":"u"}}"#,
        );
        assert!(
            out.events.is_empty(),
            "transient willRetry error must not surface, got {:?}",
            out.events
        );
    }

    #[test]
    fn warning_and_deprecation_to_notice() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"warning","params":{"message":"disk almost full"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[notice] disk almost full")
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        let out = s.parse_line(
            "r",
            r#"{"method":"deprecationNotice","params":{"summary":"flag X removed","details":"use Y"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[notice] flag X removed: use Y")
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        let out = s.parse_line(
            "r",
            r#"{"method":"configWarning","params":{"summary":"bad config"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => assert_eq!(content, "[notice] bad config"),
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    #[test]
    fn thread_start_ack_captures_id_before_ready() {
        // Regression: a new thread's id arrives in the id:2 (thread/start) reply, which also
        // marks Ready. thread_id MUST be set from that reply so the first turn isn't dropped.
        let mut s = CodexAppServer::new();
        assert!(!s.is_ready());
        let out = s.parse_line("r", r#"{"id":2,"result":{"thread":{"id":"th-ack"}}}"#);
        assert!(s.is_ready(), "id:2 reply must mark Ready");
        assert_eq!(out.thread_id.as_deref(), Some("th-ack"));
        // frame_user_turn now has a thread id and emits turn/start (not dropped).
        let msgs = s.frame_user_turn("hi", &[], &no_overrides());
        assert_eq!(msgs[0]["params"]["threadId"], "th-ack");
    }

    #[test]
    fn interrupt_after_ready() {
        let mut s = ready_server();
        let msgs = s.frame_interrupt();
        assert_eq!(msgs[0]["method"], "turn/interrupt");
        assert_eq!(msgs[0]["params"]["threadId"], "th-123");
    }

    // ── Wave-3: data-returning frame methods + response correlation ──────────────────────

    #[test]
    fn frame_methods_require_thread_and_shape() {
        // No thread → every frame method drops.
        let mut s = CodexAppServer::new();
        assert!(s.frame_compact("r1").is_empty());
        assert!(s.frame_rollback("r1", 2).is_empty());
        assert!(s.frame_fork("r1").is_empty());
        assert!(s.frame_goal_get("r1").is_empty());
        assert!(s.frame_goal_clear("r1").is_empty());
        assert!(s.frame_goal_set("r1", Some("x"), None, None).is_empty());

        let mut s = ready_server(); // thread th-123

        let compact = s.frame_compact("rc");
        assert_eq!(compact[0]["method"], "thread/compact/start");
        assert_eq!(compact[0]["params"]["threadId"], "th-123");

        let rollback = s.frame_rollback("rr", 3);
        assert_eq!(rollback[0]["method"], "thread/rollback");
        assert_eq!(rollback[0]["params"]["numTurns"], 3);
        // num_turns is clamped to >= 1.
        let rb0 = s.frame_rollback("rr0", 0);
        assert_eq!(rb0[0]["params"]["numTurns"], 1);

        let fork = s.frame_fork("rf");
        assert_eq!(fork[0]["method"], "thread/fork");
        assert_eq!(fork[0]["params"]["threadId"], "th-123");

        let gget = s.frame_goal_get("rg");
        assert_eq!(gget[0]["method"], "thread/goal/get");

        let gclear = s.frame_goal_clear("rgc");
        assert_eq!(gclear[0]["method"], "thread/goal/clear");

        // goal_set: only the provided fields are present.
        let gset = s.frame_goal_set("rgs", Some("ship it"), Some("active"), Some(50_000));
        assert_eq!(gset[0]["method"], "thread/goal/set");
        assert_eq!(gset[0]["params"]["objective"], "ship it");
        assert_eq!(gset[0]["params"]["status"], "active");
        assert_eq!(gset[0]["params"]["tokenBudget"], 50_000);
        let gset_partial = s.frame_goal_set("rgs2", Some("only obj"), None, None);
        assert_eq!(gset_partial[0]["params"]["objective"], "only obj");
        assert!(gset_partial[0]["params"].get("status").is_none());
        assert!(gset_partial[0]["params"].get("tokenBudget").is_none());
    }

    #[test]
    fn response_correlation_round_trip() {
        let mut s = ready_server();
        // Frame a fork (jsonrpc id 3) tracked to frontend request_id "ocv-fork".
        let fork = s.frame_fork("ocv-fork");
        let id = fork[0]["id"].as_i64().unwrap();
        // Feed a matching-id reply → control_response set with the right request_id + result.
        let line =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"thread":{{"id":"th-new"}}}}}}"#);
        let out = s.parse_line("run1", &line);
        let (rid, val) = out.control_response.expect("control_response");
        assert_eq!(rid, "ocv-fork");
        assert_eq!(val["thread"]["id"], "th-new");
        // The waiter is consumed — a second reply on the same id is not correlated.
        let out2 = s.parse_line("run1", &line);
        assert!(out2.control_response.is_none());
    }

    #[test]
    fn response_correlation_routes_error() {
        let mut s = ready_server();
        let rb = s.frame_rollback("ocv-rb", 2);
        let id = rb[0]["id"].as_i64().unwrap();
        // An error reply (no result) routes the error value back.
        let line =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32000,"message":"nope"}}}}"#);
        let out = s.parse_line("run1", &line);
        let (rid, val) = out.control_response.expect("control_response");
        assert_eq!(rid, "ocv-rb");
        assert_eq!(val["message"], "nope");
    }

    #[test]
    fn id_2_ack_still_special_cased_after_correlation() {
        // Regression: the client_waiters lookup must NOT swallow the id:2 thread/start ack.
        let mut s = CodexAppServer::new();
        let out = s.parse_line("r", r#"{"id":2,"result":{"thread":{"id":"th-ack"}}}"#);
        assert!(
            out.control_response.is_none(),
            "id:2 ack is not a tracked reply"
        );
        assert!(s.is_ready());
        assert_eq!(out.thread_id.as_deref(), Some("th-ack"));
    }

    #[test]
    fn goal_updated_maps_to_goal_update_event() {
        let mut s = ready_server();
        let out = s.parse_line(
            "run1",
            r#"{"method":"thread/goal/updated","params":{"threadId":"th-123","turnId":"t-1","goal":{"threadId":"th-123","objective":"ship","status":"active","tokenBudget":1000,"tokensUsed":42,"timeUsedSeconds":7,"createdAt":1,"updatedAt":2}}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::GoalUpdate { goal, .. } => {
                assert_eq!(goal["objective"], "ship");
                assert_eq!(goal["status"], "active");
                assert_eq!(goal["tokensUsed"], 42);
                assert_eq!(goal["tokenBudget"], 1000);
            }
            e => panic!("expected GoalUpdate, got {e:?}"),
        }
        // thread/goal/cleared → a null-goal GoalUpdate.
        let out = s.parse_line(
            "run1",
            r#"{"method":"thread/goal/cleared","params":{"threadId":"th-123"}}"#,
        );
        match &out.events[0] {
            BusEvent::GoalUpdate { goal, .. } => assert!(goal.is_null()),
            e => panic!("expected GoalUpdate(null), got {e:?}"),
        }
    }

    #[test]
    fn goal_update_passes_validation() {
        use crate::agent::claude_protocol::validate_bus_event;
        let ev = BusEvent::GoalUpdate {
            run_id: "r".into(),
            goal: json!({"objective":"x"}),
        };
        assert!(
            validate_bus_event(&ev).is_none(),
            "GoalUpdate must never be dropped"
        );
        // Null goal (cleared) also passes.
        let ev = BusEvent::GoalUpdate {
            run_id: "r".into(),
            goal: Value::Null,
        };
        assert!(validate_bus_event(&ev).is_none());
    }

    /// LIVE end-to-end test for COMMAND APPROVAL: drives a real `codex app-server`, forces a
    /// sandbox escape (read-only sandbox + a write command), and confirms the production
    /// driver surfaces `item/commandExecution/requestApproval` as a `PermissionPrompt`,
    /// accepts it, and the turn completes.
    ///   cargo test --lib codex_appserver::tests::live_command_approval -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_command_approval_roundtrip() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = std::env::temp_dir().join("ocv_codex_approval_test");
            std::fs::create_dir_all(&tmp).unwrap();

            let mut child = tokio::process::Command::new("codex")
                .arg("app-server")
                .arg("-c")
                .arg("suppress_unstable_features_warning=true")
                .current_dir(&tmp)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn codex app-server");

            let mut stdin = child.stdin.take().unwrap();
            let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();

            let mut driver = CodexAppServer::new();
            let ctx = StartupCtx {
                cwd: tmp.to_string_lossy().to_string(),
                approval_policy: Some("on-request".into()),
                sandbox: Some("read-only".into()), // any write now needs approval
                ..Default::default()
            };
            for msg in driver.startup_messages(&ctx) {
                let mut l = serde_json::to_string(&msg).unwrap();
                l.push('\n');
                stdin.write_all(l.as_bytes()).await.unwrap();
            }
            stdin.flush().await.unwrap();

            let mut sent = false;
            let mut saw_approval = false;
            let mut accepted = false;
            let mut completed = false;

            let run = tokio::time::timeout(std::time::Duration::from_secs(120), async {
                while let Ok(Some(line)) = lines.next_line().await {
                    let parsed = driver.parse_line("live", &line);
                    if !sent && driver.is_ready() {
                        sent = true;
                        let prompt = "Run the shell command: echo hi > probe.txt  (create that file now).";
                        for msg in driver.frame_user_turn(prompt, &[], &no_overrides()) {
                            let mut l = serde_json::to_string(&msg).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                        }
                        stdin.flush().await.unwrap();
                    }
                    if let Some(pi) = &parsed.interactive {
                        if pi.kind == PendingKind::Permission {
                            saw_approval = true;
                            assert!(matches!(parsed.events[0], BusEvent::PermissionPrompt { .. }));
                            for msg in driver.frame_response(
                                PendingKind::Permission,
                                &pi.request_id,
                                serde_json::json!({"behavior": "allow"}),
                            ) {
                                let mut l = serde_json::to_string(&msg).unwrap();
                                l.push('\n');
                                stdin.write_all(l.as_bytes()).await.unwrap();
                            }
                            stdin.flush().await.unwrap();
                            accepted = true;
                        }
                    }
                    if parsed.lifecycle == Some(LifecycleSignal::TurnCompleted) {
                        completed = true;
                        break;
                    }
                }
            })
            .await;

            let _ = child.kill().await;
            assert!(run.is_ok(), "approval live test timed out");
            assert!(saw_approval, "never received commandExecution/requestApproval");
            assert!(accepted && completed, "accepted={accepted} completed={completed}");
            eprintln!("LIVE APPROVAL OK: saw_approval={saw_approval} accepted={accepted} completed={completed}");
        });
    }

    /// LIVE end-to-end test: drives a REAL `codex app-server` through the production
    /// `CodexAppServer` driver — initialize → thread/start → turn/start (asking a
    /// multiple-choice question) → receive `item/tool/requestUserInput` → answer it →
    /// turn completes. Proves the driver's framing AND parsing against the real server.
    ///
    /// Ignored by default (spawns codex, needs auth, makes one real API call). Run with:
    ///   cargo test --lib codex_appserver::tests::live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_request_user_input_roundtrip() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = std::env::temp_dir().join("ocv_codex_live_test");
            std::fs::create_dir_all(&tmp).unwrap();

            let mut child = tokio::process::Command::new("codex")
                .arg("app-server")
                .arg("--enable")
                .arg("default_mode_request_user_input")
                .arg("-c")
                .arg("suppress_unstable_features_warning=true")
                .current_dir(&tmp)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn codex app-server (is `codex` on PATH + logged in?)");

            let mut stdin = child.stdin.take().unwrap();
            let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();

            let mut driver = CodexAppServer::new();
            let ctx = StartupCtx {
                cwd: tmp.to_string_lossy().to_string(),
                approval_policy: Some("on-request".into()),
                sandbox: Some("read-only".into()),
                ..Default::default()
            };
            for msg in driver.startup_messages(&ctx) {
                let mut line = serde_json::to_string(&msg).unwrap();
                line.push('\n');
                stdin.write_all(line.as_bytes()).await.unwrap();
            }
            stdin.flush().await.unwrap();

            let mut sent_turn = false;
            let mut saw_user_input = false;
            let mut answered = false;
            let mut turn_completed = false;

            let run = tokio::time::timeout(std::time::Duration::from_secs(120), async {
                while let Ok(Some(line)) = lines.next_line().await {
                    let parsed = driver.parse_line("live", &line);

                    if !sent_turn && driver.is_ready() {
                        sent_turn = true;
                        let prompt = "Call request_user_input to ask me ONE multiple-choice \
                                      question: header \"Pick\", question \"A or B?\", options \
                                      A and B. Call that tool now, before anything else.";
                        for msg in driver.frame_user_turn(prompt, &[], &no_overrides()) {
                            let mut l = serde_json::to_string(&msg).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                        }
                        stdin.flush().await.unwrap();
                    }

                    if let Some(pi) = &parsed.interactive {
                        if pi.kind == PendingKind::UserInput {
                            saw_user_input = true;
                            // Pull qid + first option label out of the AskUserQuestion ToolStart.
                            let (qid, label) = parsed
                                .events
                                .iter()
                                .find_map(|e| match e {
                                    BusEvent::ToolStart { input, .. } => {
                                        let q = input.get("questions")?.get(0)?;
                                        let id = q.get("id")?.as_str()?.to_string();
                                        let lbl = q.get("options")?.get(0)?.get("label")?.as_str()?.to_string();
                                        Some((id, lbl))
                                    }
                                    _ => None,
                                })
                                .expect("questions in AskUserQuestion event");
                            let answers = serde_json::json!({ "answers": { qid: [label] } });
                            for msg in driver.frame_response(PendingKind::UserInput, &pi.request_id, answers) {
                                let mut l = serde_json::to_string(&msg).unwrap();
                                l.push('\n');
                                stdin.write_all(l.as_bytes()).await.unwrap();
                            }
                            stdin.flush().await.unwrap();
                            answered = true;
                        }
                    }

                    if parsed.lifecycle == Some(LifecycleSignal::TurnCompleted) {
                        turn_completed = true;
                        break;
                    }
                }
            })
            .await;

            let _ = child.kill().await;

            assert!(run.is_ok(), "live test timed out");
            assert!(saw_user_input, "never received item/tool/requestUserInput");
            assert!(answered, "never sent a response");
            assert!(turn_completed, "turn never completed after answering");
            eprintln!(
                "LIVE OK: ready+turn sent={sent_turn}, requestUserInput seen={saw_user_input}, answered={answered}, turn_completed={turn_completed}"
            );
        });
    }
}

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
    CodexSkillRef, CodexTurnOverrides, InteractiveResponse, LifecycleSignal, ParsedLine,
    PendingInteractive, PendingKind, SessionProtocol, StartupCtx,
};
use crate::models::BusEvent;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

const MAX_UNCLAIMED_THREADS: usize = 32;
const MAX_DEFERRED_MESSAGES_PER_THREAD: usize = 64;
const MAX_DEFERRED_PAYLOAD_BYTES: usize = 512 * 1024;
const DEFERRED_MESSAGE_TTL: Duration = Duration::from_secs(30);

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
    kind: PendingKind,
    /// Requested grant profile for `item/permissions/requestApproval`. Codex requires the
    /// response to echo only the subset the user granted; command/file approvals use decisions.
    requested_permissions: Option<Value>,
}

enum DeferredThreadMessage {
    Notification { method: String, params: Value },
    Interactive { method: String, message: Value },
}

impl DeferredThreadMessage {
    fn is_interactive(&self) -> bool {
        matches!(self, Self::Interactive { .. })
    }

    fn payload_bytes(&self) -> usize {
        match self {
            Self::Notification { method, params } => method.len() + params.to_string().len(),
            Self::Interactive { method, message } => method.len() + message.to_string().len(),
        }
    }
}

fn reject_deferred_interactive(message: DeferredThreadMessage, reason: &str, out: &mut ParsedLine) {
    if let DeferredThreadMessage::Interactive { message, .. } = message {
        out.outbound.push(json!({
            "jsonrpc": "2.0",
            "id": message.get("id").cloned().unwrap_or(Value::Null),
            "error": { "code": -32600, "message": reason }
        }));
    }
}

struct DeferredThreadEntry {
    message: DeferredThreadMessage,
    received_at: Instant,
    sequence: u64,
    payload_bytes: usize,
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
    /// Autonomous `thread/read` requests keyed by JSON-RPC id → spawned thread id.
    internal_waiters: HashMap<i64, String>,
    /// Spawned threads already scheduled for identity lookup. Collab items can be repeated as
    /// their status changes, but their immutable identity only needs one read per session.
    seen_subagent_threads: HashSet<String>,
    /// Spawned thread id → Agent card id. Child notifications share the parent's app-server
    /// connection, so this mapping lets their activity enter the card's sub-timeline.
    child_parent_tools: HashMap<String, String>,
    /// Notifications and requests can race ahead of the parent's spawn item. Keep them isolated
    /// until that item proves ownership; both dimensions are capped so unrelated threads cannot
    /// grow session memory without bound.
    deferred_thread_messages: HashMap<String, VecDeque<DeferredThreadEntry>>,
    deferred_payload_bytes: usize,
    next_deferred_sequence: u64,
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
            internal_waiters: HashMap::new(),
            seen_subagent_threads: HashSet::new(),
            child_parent_tools: HashMap::new(),
            deferred_thread_messages: HashMap::new(),
            deferred_payload_bytes: 0,
            next_deferred_sequence: 0,
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

    /// Expire unclaimed requests even while app-server stdout is quiet waiting for a response.
    /// The actor calls this from its existing periodic timeout tick and writes returned frames.
    pub fn expire_deferred(&mut self) -> Vec<Value> {
        let mut out = ParsedLine::default();
        self.expire_deferred_messages(&mut out);
        out.outbound
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_client_id;
        self.next_client_id += 1;
        id
    }

    /// Register newly spawned threads and request their identity. Codex 0.147 reports spawn via
    /// `subAgentActivity.agentThreadId`; older spawnAgent payloads use `receiverThreadIds`.
    fn register_subagents(
        &mut self,
        run_id: &str,
        item: &Value,
        containing_parent_tool_id: Option<&str>,
        out: &mut ParsedLine,
    ) {
        // The frontend supports one sub-timeline level. Descendants therefore stay in the
        // original top-level Agent card instead of targeting a nested Agent item as parent.
        let parent_tool_id = containing_parent_tool_id
            .or_else(|| item.get("id").and_then(Value::as_str))
            .unwrap_or("");
        let thread_ids: Vec<&str> = match item.get("type").and_then(Value::as_str) {
            Some("subAgentActivity")
                if item.get("kind").and_then(Value::as_str) == Some("started") =>
            {
                item.get("agentThreadId")
                    .and_then(Value::as_str)
                    .into_iter()
                    .collect()
            }
            Some("collabAgentToolCall")
                if item.get("tool").and_then(Value::as_str) == Some("spawnAgent") =>
            {
                item.get("receiverThreadIds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect()
            }
            _ => return,
        };

        for thread_id in thread_ids {
            if thread_id.is_empty() {
                continue;
            }
            if !parent_tool_id.is_empty() {
                self.child_parent_tools
                    .insert(thread_id.to_string(), parent_tool_id.to_string());
                self.replay_deferred_thread(run_id, thread_id, out);
            }
            if !self.seen_subagent_threads.insert(thread_id.to_string()) {
                continue;
            }

            let id = self.next_id();
            self.internal_waiters.insert(id, thread_id.to_string());
            out.outbound.push(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "thread/read",
                "params": { "threadId": thread_id, "includeTurns": false }
            }));
            log::debug!(
                "[codex_appserver] sub-agent identity requested: thread_id={}, request_id={}",
                thread_id,
                id
            );
        }
    }

    fn defer_thread_message(
        &mut self,
        thread_id: &str,
        message: DeferredThreadMessage,
        out: &mut ParsedLine,
    ) -> bool {
        self.expire_deferred_messages(out);
        let payload_bytes = message.payload_bytes();
        if payload_bytes > MAX_DEFERRED_PAYLOAD_BYTES {
            reject_deferred_interactive(message, "Interactive request exceeds buffer limit", out);
            return false;
        }
        let interactive = message.is_interactive();
        while self.deferred_thread_messages.len() >= MAX_UNCLAIMED_THREADS
            && !self.deferred_thread_messages.contains_key(thread_id)
        {
            if !self.evict_oldest_deferred(interactive, out) {
                reject_deferred_interactive(message, "Interactive request buffer is full", out);
                return false;
            }
        }
        while self
            .deferred_thread_messages
            .get(thread_id)
            .is_some_and(|queue| queue.len() >= MAX_DEFERRED_MESSAGES_PER_THREAD)
        {
            if !self.evict_oldest_deferred_in_thread(thread_id, interactive, out) {
                reject_deferred_interactive(message, "Interactive request buffer is full", out);
                return false;
            }
        }
        while self.deferred_payload_bytes + payload_bytes > MAX_DEFERRED_PAYLOAD_BYTES {
            if !self.evict_oldest_deferred(interactive, out) {
                reject_deferred_interactive(message, "Interactive request buffer is full", out);
                return false;
            }
        }
        let sequence = self.next_deferred_sequence;
        self.next_deferred_sequence = self.next_deferred_sequence.wrapping_add(1);
        self.deferred_payload_bytes += payload_bytes;
        let queue = self
            .deferred_thread_messages
            .entry(thread_id.to_string())
            .or_default();
        queue.push_back(DeferredThreadEntry {
            message,
            received_at: Instant::now(),
            sequence,
            payload_bytes,
        });
        log::debug!(
            "[codex_appserver] unclaimed thread message deferred: thread_id={}, queued={}, total_bytes={}",
            thread_id,
            queue.len(),
            self.deferred_payload_bytes
        );
        true
    }

    fn evict_oldest_deferred_in_thread(
        &mut self,
        thread_id: &str,
        incoming_interactive: bool,
        out: &mut ParsedLine,
    ) -> bool {
        let Some(queue) = self.deferred_thread_messages.get(thread_id) else {
            return false;
        };
        let oldest_notification = queue
            .iter()
            .find(|entry| !entry.message.is_interactive())
            .map(|entry| entry.sequence);
        let sequence = oldest_notification.or_else(|| {
            incoming_interactive
                .then(|| queue.front().map(|entry| entry.sequence))
                .flatten()
        });
        let Some(sequence) = sequence else {
            return false;
        };
        self.remove_deferred_entry(
            thread_id,
            sequence,
            out,
            "Interactive request evicted from buffer",
        );
        true
    }

    fn expire_deferred_messages(&mut self, out: &mut ParsedLine) {
        let now = Instant::now();
        let expired: Vec<(String, u64)> = self
            .deferred_thread_messages
            .iter()
            .flat_map(|(thread_id, queue)| {
                queue
                    .iter()
                    .take_while(|entry| {
                        now.duration_since(entry.received_at) >= DEFERRED_MESSAGE_TTL
                    })
                    .map(|entry| (thread_id.clone(), entry.sequence))
            })
            .collect();
        for (thread_id, sequence) in expired {
            self.remove_deferred_entry(&thread_id, sequence, out, "Interactive request expired");
        }
    }

    fn evict_oldest_deferred(&mut self, incoming_interactive: bool, out: &mut ParsedLine) -> bool {
        let oldest_matching = |interactive: bool| {
            self.deferred_thread_messages
                .iter()
                .flat_map(|(thread_id, queue)| {
                    queue.iter().filter_map(move |entry| {
                        (entry.message.is_interactive() == interactive)
                            .then_some((thread_id.clone(), entry.sequence))
                    })
                })
                .min_by_key(|(_, sequence)| *sequence)
        };
        // Ordinary notifications are disposable telemetry. Never evict an unanswered request
        // for another notification; an incoming request may evict a request only after every
        // buffered notification has already been removed.
        let oldest = oldest_matching(false).or_else(|| {
            incoming_interactive
                .then(|| oldest_matching(true))
                .flatten()
        });
        let Some((thread_id, sequence)) = oldest else {
            return false;
        };
        self.remove_deferred_entry(
            &thread_id,
            sequence,
            out,
            "Interactive request evicted from buffer",
        );
        true
    }

    fn remove_deferred_entry(
        &mut self,
        thread_id: &str,
        sequence: u64,
        out: &mut ParsedLine,
        reason: &str,
    ) {
        let mut removed = None;
        let mut empty = false;
        if let Some(queue) = self.deferred_thread_messages.get_mut(thread_id) {
            if let Some(index) = queue.iter().position(|entry| entry.sequence == sequence) {
                removed = queue.remove(index);
            }
            empty = queue.is_empty();
        }
        if empty {
            self.deferred_thread_messages.remove(thread_id);
        }
        if let Some(entry) = removed {
            self.deferred_payload_bytes = self
                .deferred_payload_bytes
                .saturating_sub(entry.payload_bytes);
            reject_deferred_interactive(entry.message, reason, out);
        }
    }

    fn replay_deferred_thread(&mut self, run_id: &str, thread_id: &str, out: &mut ParsedLine) {
        let Some(mut messages) = self.deferred_thread_messages.remove(thread_id) else {
            return;
        };
        log::debug!(
            "[codex_appserver] replaying deferred thread messages: thread_id={}, count={}",
            thread_id,
            messages.len()
        );
        while let Some(entry) = messages.pop_front() {
            self.deferred_payload_bytes = self
                .deferred_payload_bytes
                .saturating_sub(entry.payload_bytes);
            if entry.received_at.elapsed() >= DEFERRED_MESSAGE_TTL {
                reject_deferred_interactive(entry.message, "Interactive request expired", out);
                continue;
            }
            match entry.message {
                DeferredThreadMessage::Notification { method, params } => {
                    self.handle_notification(run_id, &method, &params, out);
                }
                DeferredThreadMessage::Interactive { method, message } => {
                    let replayed = self.handle_server_request(run_id, &method, &message);
                    out.events.extend(replayed.events);
                    out.interactive.extend(replayed.interactive);
                    out.outbound.extend(replayed.outbound);
                }
            }
        }
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

    /// Frame `mcpServerStatus/list` — read runtime status of every configured MCP server.
    /// Response: `{data: McpServerStatus[], nextCursor}` where each entry carries
    /// `{name, serverInfo, tools, resources, authStatus}`. Note this is a DIFFERENT shape than
    /// Claude's `mcp_status` reply — the frontend normalizes both. `threadId` scopes the query.
    pub fn frame_mcp_status(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "mcpServerStatus/list", json!({}))
    }

    /// Frame `skills/list` — the skills the agent actually sees this session (vs the static
    /// file scan in the Extend page). Response: `{data: SkillsListEntry[]}` where each entry is
    /// `{cwd, skills: SkillMetadata[], errors}`. We omit `cwds` so it defaults to the session cwd.
    pub fn frame_skills_list(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "skills/list", json!({}))
    }

    /// Frame `experimentalFeature/list` — the feature flags + their current enablement for this
    /// session's config (incl. project-local). Response: `{data: ExperimentalFeature[]}` where each
    /// is `{name, stage, displayName, description, enabled, defaultEnabled}`.
    pub fn frame_experimental_feature_list(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "experimentalFeature/list", json!({}))
    }

    /// Frame `model/list` — the authoritative model catalog for the installed CLI. Response:
    /// `{data: Model[]}` (`{id, displayName, supportedReasoningEfforts, defaultReasoningEffort,
    /// hidden, isDefault, …}`). The picker caches this so it stays accurate across CLI versions.
    pub fn frame_model_list(&mut self, request_id: &str) -> Vec<Value> {
        self.frame_tracked(request_id, "model/list", json!({}))
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
        skills: &[CodexSkillRef],
        overrides: &CodexTurnOverrides,
    ) -> Vec<Value> {
        let thread_id = match &self.thread_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] frame_user_turn before thread/started — dropping");
                return vec![];
            }
        };
        // Skill directives lead the input (they scope the turn), then the user text, then images.
        // Codex only triggers a skill via this typed `{type:"skill"}` item — not via `/name` text.
        let mut input: Vec<Value> = skills
            .iter()
            .map(|s| json!({ "type": "skill", "name": s.name, "path": s.path }))
            .collect();
        input.push(json!({
            "type": "text",
            "text": text,
            "text_elements": []
        }));
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
            None => {
                log::warn!("[codex_appserver] frame_interrupt before thread/started — dropping");
                return vec![];
            }
        };
        // Codex 0.147 requires both ids. Without an active turn there is nothing valid to
        // interrupt, and sending only threadId is rejected by the stable app-server schema.
        let turn_id = match &self.active_turn_id {
            Some(t) => t.clone(),
            None => {
                log::warn!("[codex_appserver] frame_interrupt with no active turn — dropping");
                return vec![];
            }
        };
        let id = self.next_id();
        vec![json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/interrupt",
            "params": { "threadId": thread_id, "turnId": turn_id }
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

        // Autonomous metadata replies are not frontend control responses. A failed identity
        // lookup is non-fatal: the Agent card still renders with its thread id and live status.
        if let Some(id) = msg.get("id").and_then(Value::as_i64) {
            if let Some(thread_id) = self.internal_waiters.remove(&id) {
                if let Some(thread) = msg.get("result").and_then(|r| r.get("thread")) {
                    let string_field =
                        |name: &str| thread.get(name).and_then(Value::as_str).map(str::to_string);
                    out.events.push(BusEvent::CodexAgentInfo {
                        run_id: run_id.to_string(),
                        thread_id,
                        nickname: string_field("agentNickname"),
                        role: string_field("agentRole"),
                        parent_thread_id: string_field("parentThreadId"),
                    });
                } else {
                    log::debug!(
                        "[codex_appserver] sub-agent identity unavailable: thread_id={}",
                        thread_id
                    );
                }
                return out;
            }
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
                    // Root notifications can also race ahead of the thread/start reply that
                    // proves ownership. Replay them now using the same bounded isolation path.
                    self.replay_deferred_thread(run_id, id, &mut out);
                }
            }
            self.phase = Phase::Ready;
        }
        out
    }

    fn frame_response(
        &self,
        kind: PendingKind,
        request_id: &str,
        response: Value,
    ) -> Result<InteractiveResponse, String> {
        let pending = match self.pending.get(request_id) {
            Some(p) => p,
            None => {
                log::warn!("[codex_appserver] frame_response: unknown request_id {request_id}");
                return Err(format!("unknown interactive request_id: {request_id}"));
            }
        };
        if pending.kind != kind {
            log::warn!(
                "[codex_appserver] frame_response: request kind mismatch for request_id={request_id}"
            );
            return Err(format!(
                "interactive request kind mismatch for request_id: {request_id}"
            ));
        }
        let result = match kind {
            PendingKind::Permission => {
                let behavior = response
                    .get("behavior")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny");
                if pending.method == M_PERM_APPROVAL {
                    // Permission-profile requests use a granted subset, not the decision enum
                    // used by command/file approvals. An empty profile denies every request.
                    let permissions = if behavior == "allow" {
                        pending
                            .requested_permissions
                            .clone()
                            .unwrap_or_else(|| json!({}))
                    } else {
                        json!({})
                    };
                    log::debug!(
                        "[codex_appserver] permissions response: request_id={}, behavior={}",
                        request_id,
                        behavior
                    );
                    json!({ "permissions": permissions, "scope": "turn" })
                } else {
                    // A deny-and-stop action maps to Codex's cancel decision for command/file
                    // approvals. Plain deny keeps the current turn alive with decline.
                    let decision = if behavior == "allow" {
                        "accept"
                    } else if response.get("interrupt").and_then(Value::as_bool) == Some(true) {
                        "cancel"
                    } else {
                        "decline"
                    };
                    json!({ "decision": decision })
                }
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
        let primary = json!({
            "jsonrpc": "2.0",
            "id": pending.raw_id,
            "result": result
        });
        // The permission-profile response has no cancel decision. Preserve the UI's
        // deny-and-stop contract by denying the grant first, then interrupting the active turn.
        let interrupt = pending.method == M_PERM_APPROVAL
            && response.get("behavior").and_then(Value::as_str) != Some("allow")
            && response.get("interrupt").and_then(Value::as_bool) == Some(true);
        Ok(InteractiveResponse {
            primary,
            interrupt_after_commit: interrupt,
        })
    }

    fn frame_cancel(&self, request_id: &str) -> Result<Value, String> {
        let pending = self.pending.get(request_id).ok_or_else(|| {
            log::warn!("[codex_appserver] frame_cancel: unknown request_id {request_id}");
            format!("unknown interactive request_id: {request_id}")
        })?;
        Ok(json!({
            "jsonrpc": "2.0",
            "id": pending.raw_id,
            "error": {
                "code": -32000,
                "message": "Interactive request cancelled by user"
            }
        }))
    }

    fn commit_response(&mut self, request_id: &str) -> bool {
        let removed = self.pending.remove(request_id).is_some();
        if removed {
            log::debug!(
                "[codex_appserver] interactive response committed: request_id={}",
                request_id
            );
        }
        removed
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

fn is_legacy_interactive_method(method: &str) -> bool {
    matches!(method, M_CMD_APPROVAL_LEGACY | M_FILE_APPROVAL_LEGACY)
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
        let request_thread = params.get("threadId").and_then(Value::as_str);
        // Stable app-server requests always carry threadId. Only the two pre-v2 approval
        // methods predate that field, so keeping their compatibility cannot weaken v2 routing.
        if !is_legacy_interactive_method(method) && request_thread.is_none_or(str::is_empty) {
            log::warn!(
                "[codex_appserver] interactive request missing thread id: method={}",
                method
            );
            out.outbound.push(json!({
                "jsonrpc": "2.0",
                "id": raw_id,
                "error": {
                    "code": -32602,
                    "message": "Interactive request is missing threadId"
                }
            }));
            return out;
        }
        let parent_tool_id = request_thread
            .and_then(|id| self.child_parent_tools.get(id))
            .map(String::as_str);
        if request_thread.is_some_and(|id| {
            self.thread_id.as_deref() != Some(id) && !self.child_parent_tools.contains_key(id)
        }) {
            let thread_id = request_thread.unwrap_or_default();
            if self.defer_thread_message(
                thread_id,
                DeferredThreadMessage::Interactive {
                    method: method.to_string(),
                    message: msg.clone(),
                },
                &mut out,
            ) {
                return out;
            }
            log::warn!(
                "[codex_appserver] unclaimed interactive request rejected at buffer limit: method={}, thread_id={}",
                method,
                thread_id
            );
            return out;
        }
        log::debug!(
            "[codex_appserver] interactive request: method={}, request_id={}",
            method,
            request_id
        );

        let (kind, events) = match method {
            M_CMD_APPROVAL | M_CMD_APPROVAL_LEGACY => (
                PendingKind::Permission,
                vec![approval_prompt(
                    run_id,
                    &request_id,
                    "Bash",
                    &params,
                    parent_tool_id,
                )],
            ),
            M_FILE_APPROVAL | M_FILE_APPROVAL_LEGACY => (
                PendingKind::Permission,
                vec![approval_prompt(
                    run_id,
                    &request_id,
                    "Edit",
                    &params,
                    parent_tool_id,
                )],
            ),
            M_PERM_APPROVAL => (
                PendingKind::Permission,
                vec![approval_prompt(
                    run_id,
                    &request_id,
                    "RequestPermissions",
                    &params,
                    parent_tool_id,
                )],
            ),
            M_REQUEST_USER_INPUT => (
                PendingKind::UserInput,
                ask_user_question_events(run_id, &request_id, &params, parent_tool_id),
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
                kind,
                requested_permissions: if method == M_PERM_APPROVAL {
                    params.get("permissions").cloned()
                } else {
                    None
                },
            },
        );
        out.events = events;
        out.interactive
            .push(PendingInteractive { request_id, kind });
        out
    }

    fn handle_notification(
        &mut self,
        run_id: &str,
        method: &str,
        params: &Value,
        out: &mut ParsedLine,
    ) {
        let notification_thread = params.get("threadId").and_then(Value::as_str);
        let parent_tool = notification_thread
            .and_then(|id| self.child_parent_tools.get(id))
            .cloned();
        let is_child = parent_tool.is_some();
        let parent_tool_id = parent_tool.as_deref();
        let is_foreign = notification_thread.is_some_and(|id| {
            self.thread_id.as_deref() != Some(id) && !self.child_parent_tools.contains_key(id)
        });

        // Codex app-server can multiplex notifications from multiple active threads. Until a
        // spawn item proves the relationship, isolate the thread so it cannot mutate this run.
        if is_foreign && method != "thread/started" {
            let thread_id = notification_thread.unwrap_or_default();
            if !self.defer_thread_message(
                thread_id,
                DeferredThreadMessage::Notification {
                    method: method.to_string(),
                    params: params.clone(),
                },
                out,
            ) {
                log::warn!(
                    "[codex_appserver] unclaimed notification dropped at buffer limit: method={}, thread_id={}",
                    method,
                    thread_id
                );
            }
            return;
        }

        if is_child
            && !matches!(
                method,
                "turn/started"
                    | "turn/completed"
                    | "turn/failed"
                    | "error"
                    | "item/agentMessage/delta"
                    | "item/reasoning/textDelta"
                    | "item/reasoning/summaryTextDelta"
                    | "item/started"
                    | "item/completed"
                    | "item/commandExecution/outputDelta"
                    | "item/mcpToolCall/progress"
            )
        {
            log::debug!(
                "[codex_appserver] sub-agent state notification ignored: method={}, thread_id={}",
                method,
                notification_thread.unwrap_or_default()
            );
            return;
        }

        match method {
            "thread/started" => {
                if let Some(id) = params
                    .get("thread")
                    .and_then(|t| t.get("id"))
                    .and_then(|v| v.as_str())
                {
                    if self.thread_id.as_deref() == Some(id) {
                        out.thread_id = Some(id.to_string());
                        self.phase = Phase::Ready;
                    } else {
                        // The id:2 thread/start|resume reply is the authoritative root. Global
                        // app-server notifications can be interleaved before that reply arrives.
                        log::debug!(
                            "[codex_appserver] uncorrelated thread/started ignored: thread_id={}",
                            id
                        );
                    }
                }
            }
            "turn/started" if !is_child => {
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
            "turn/completed" if !is_child => {
                self.active_turn_id = None;
                out.lifecycle = Some(LifecycleSignal::TurnCompleted);
            }
            "turn/failed" if !is_child => {
                self.active_turn_id = None;
                let err = params
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                out.lifecycle = Some(LifecycleSignal::TurnFailed(err));
            }
            // Child turns are represented by the parent collab item. They must not transition
            // the top-level run or replace the parent's active turn id.
            "turn/started" | "turn/completed" | "turn/failed" => {}
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
                } else if is_child {
                    log::debug!("[codex] sub-agent terminal error: {m}");
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
                        parent_tool_use_id: parent_tool_id.map(str::to_string),
                    });
                }
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                    out.events.push(BusEvent::ThinkingDelta {
                        run_id: run_id.to_string(),
                        text: delta.to_string(),
                        parent_tool_use_id: parent_tool_id.map(str::to_string),
                    });
                }
            }
            "item/started" => {
                if let Some(item) = params.get("item") {
                    if let Some(ev) = item_started_event(run_id, item, parent_tool_id) {
                        out.events.push(ev);
                    }
                    self.register_subagents(run_id, item, parent_tool_id, out);
                }
            }
            "item/completed" => {
                if let Some(item) = params.get("item") {
                    item_completed_events(run_id, item, parent_tool_id, &mut out.events);
                    self.register_subagents(run_id, item, parent_tool_id, out);
                }
            }
            "thread/tokenUsage/updated" if !is_child => {
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
                            parent_tool_use_id: parent_tool_id.map(str::to_string),
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
            // NEW in 0.137: concise guardian safety warning — surface like a plain warning notice.
            "guardianWarning" => {
                if let Some(msg) = params.get("message").and_then(|v| v.as_str()) {
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content: format!("[guardian] {msg}"),
                    });
                }
            }
            // NEW in 0.137: model verification results (ModelVerification enum strings, e.g.
            // "trustedAccessForCyber"). Surface a concise notice listing them.
            "model/verification" => {
                let names: Vec<String> = params
                    .get("verifications")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if !names.is_empty() {
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content: format!("[notice] model verification: {}", names.join(", ")),
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
            // Turn-level aggregated unified diff → surface for a reviewable diff view. params =
            // {threadId, turnId, diff}. diff is cumulative across the turn; latest supersedes.
            "turn/diff/updated" => {
                if let Some(diff) = params.get("diff").and_then(|v| v.as_str()) {
                    let turn_id = params
                        .get("turnId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    out.events.push(BusEvent::CodexTurnDiff {
                        run_id: run_id.to_string(),
                        turn_id,
                        diff: diff.to_string(),
                    });
                }
            }
            // Hook lifecycle: started fires when a hook begins, completed when it finishes (with a
            // terminal HookRunStatus + duration). Both carry the full HookRunSummary at `run`; the
            // stable `run.id` lets the frontend update one card in place instead of stacking.
            "hook/started" | "hook/completed" => {
                if let Some(ev) = hook_run_event(run_id, method, params) {
                    out.events.push(ev);
                }
            }
            // MCP tool-call progress: append the human message into the open MCP tool card (keyed
            // by itemId == ToolStart's tool_use_id), reusing the same delta path as shell output.
            "item/mcpToolCall/progress" => {
                if let (Some(id), Some(msg)) = (
                    params.get("itemId").and_then(|v| v.as_str()),
                    params.get("message").and_then(|v| v.as_str()),
                ) {
                    if !id.is_empty() && !msg.is_empty() {
                        out.events.push(BusEvent::ToolOutputDelta {
                            run_id: run_id.to_string(),
                            tool_use_id: id.to_string(),
                            delta: format!("{msg}\n"),
                            parent_tool_use_id: parent_tool_id.map(str::to_string),
                        });
                    }
                }
            }
            // Guardian auto-approval review → a concise notice so auto-approved/denied actions are
            // visible. Only the terminal `completed` is surfaced (started would double the noise).
            // Upstream marks this payload [UNSTABLE] — extract defensively, never hard-depend on it.
            "item/autoApprovalReview/completed" => {
                if let Some(content) = guardian_notice(params) {
                    out.events.push(BusEvent::CommandOutput {
                        run_id: run_id.to_string(),
                        content,
                    });
                }
            }
            // MCP server startup-state change → live-update the status panel (no manual refresh).
            // params = {name, status: "starting"|"ready"|"failed"|"cancelled", error: string|null}.
            "mcpServer/startupStatus/updated" => {
                if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
                    let status = params
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("starting")
                        .to_string();
                    let error = params
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    out.events.push(BusEvent::CodexMcpStatus {
                        run_id: run_id.to_string(),
                        name: name.to_string(),
                        status,
                        error,
                    });
                }
            }
            // Skills changed on disk mid-session → emit a notice so the user knows the runtime
            // skill set shifted (the Extend panel / autocomplete re-fetch via skills/list on demand).
            "skills/changed" => {
                out.events.push(BusEvent::CommandOutput {
                    run_id: run_id.to_string(),
                    content: "[notice] skills changed".to_string(),
                });
            }
            _ => {} // process/* deltas, realtime, fs, status — ignored in v1.
        }
    }
}

// ── ServerRequest → interactive BusEvent ──────────────────────────────────────────────

fn approval_prompt(
    run_id: &str,
    request_id: &str,
    tool_name: &str,
    params: &Value,
    parent_tool_id: Option<&str>,
) -> BusEvent {
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
    if let Some(permissions) = params.get("permissions") {
        input.insert("permissions".into(), permissions.clone());
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
        parent_tool_use_id: parent_tool_id.map(str::to_string),
        suggestions: vec![],
    }
}

/// Map `item/tool/requestUserInput` to an AskUserQuestion tool (ToolStart + ToolEnd) so it
/// renders in the existing `InlineToolCard`. `tool_use_id == request_id` so the frontend can
/// route the answer back via `respond_user_input`.
fn ask_user_question_events(
    run_id: &str,
    request_id: &str,
    params: &Value,
    parent_tool_id: Option<&str>,
) -> Vec<BusEvent> {
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
            parent_tool_use_id: parent_tool_id.map(str::to_string),
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
            parent_tool_use_id: parent_tool_id.map(str::to_string),
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
        // Collab items are handled before this generic mapper because they need their full
        // operation/agent payload rather than the command-only input assembled below.
        CodexToolKind::CollabToolCall => None,
        kind => kind.fixed_tool_name().map(|s| s.to_string()),
    }
}

/// Build the rich tool input for a `collabAgentToolCall` item (Codex multi-agent / spawn_agent).
/// `codexCollab: true` lets the frontend render the collab shape (operation + per-agent states),
/// which differs from Claude's AgentInput (subagent_type/prompt). Shared by started + completed.
fn collab_input(item: &Value) -> Value {
    let agents: Vec<Value> = item
        .get("agentsStates")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(tid, st)| {
                    json!({
                        "thread_id": tid,
                        "status": st.get("status").and_then(|v| v.as_str()),
                        "message": st.get("message").and_then(|v| v.as_str()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "codexCollab": true,
        "operation": item.get("tool").and_then(|v| v.as_str()).unwrap_or("collab"),
        "prompt": item.get("prompt").and_then(|v| v.as_str()).unwrap_or(""),
        "model": item.get("model").and_then(|v| v.as_str()),
        "reasoningEffort": item.get("reasoningEffort").and_then(|v| v.as_str()),
        "status": item.get("status").and_then(|v| v.as_str()),
        "senderThreadId": item.get("senderThreadId").and_then(|v| v.as_str()),
        "receiverThreadIds": item.get("receiverThreadIds").cloned().unwrap_or(json!([])),
        "agents": agents,
    })
}

fn subagent_activity_input(item: &Value, completed: bool) -> Value {
    let thread_id = item
        .get("agentThreadId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
    let operation = match kind {
        "started" => "spawnAgent",
        "interacted" => "interacted",
        "interrupted" => "interrupted",
        _ => "subAgentActivity",
    };
    let activity_status = if kind == "interrupted" {
        "interrupted"
    } else if completed {
        "completed"
    } else {
        "inProgress"
    };
    let agent_status = if kind == "interrupted" {
        "interrupted"
    } else {
        "running"
    };
    json!({
        "codexCollab": true,
        "operation": operation,
        "activityKind": kind,
        "status": activity_status,
        "agentPath": item.get("agentPath").and_then(Value::as_str),
        "receiverThreadIds": if thread_id.is_empty() { json!([]) } else { json!([thread_id]) },
        "agents": if thread_id.is_empty() {
            json!([])
        } else {
            json!([{ "thread_id": thread_id, "status": agent_status, "message": null }])
        }
    })
}

fn item_started_event(
    run_id: &str,
    item: &Value,
    parent_tool_id: Option<&str>,
) -> Option<BusEvent> {
    let id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if item.get("type").and_then(Value::as_str) == Some("subAgentActivity") {
        return Some(BusEvent::ToolStart {
            run_id: run_id.to_string(),
            tool_use_id: id,
            tool_name: "Agent".to_string(),
            input: subagent_activity_input(item, false),
            parent_tool_use_id: parent_tool_id.map(str::to_string),
        });
    }
    // Codex multi-agent: collabAgentToolCall (spawn_agent etc.) renders as an "Agent" subagent
    // card. item_tool_name doesn't cover it (it's not a plain tool), so handle it up front —
    // otherwise it's dropped entirely on the app-server path.
    if item.get("type").and_then(|v| v.as_str()) == Some("collabAgentToolCall") {
        return Some(BusEvent::ToolStart {
            run_id: run_id.to_string(),
            tool_use_id: id,
            tool_name: "Agent".to_string(),
            input: collab_input(item),
            parent_tool_use_id: parent_tool_id.map(str::to_string),
        });
    }
    let tool_name = item_tool_name(item)?;
    let mut input = serde_json::Map::new();
    if let Some(cmd) = item.get("command").and_then(|v| v.as_str()) {
        input.insert("command".into(), json!(cmd));
    }
    Some(BusEvent::ToolStart {
        run_id: run_id.to_string(),
        tool_use_id: id,
        tool_name,
        input: Value::Object(input),
        parent_tool_use_id: parent_tool_id.map(str::to_string),
    })
}

fn item_completed_events(
    run_id: &str,
    item: &Value,
    parent_tool_id: Option<&str>,
    out: &mut Vec<BusEvent>,
) {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if item_type == "subAgentActivity" {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let payload = subagent_activity_input(item, true);
        let status = if item.get("kind").and_then(Value::as_str) == Some("interrupted") {
            "error"
        } else {
            "success"
        };
        out.push(BusEvent::ToolEnd {
            run_id: run_id.to_string(),
            tool_use_id: id,
            tool_name: "Agent".to_string(),
            output: json!({ "content": payload.clone() }),
            status: status.to_string(),
            duration_ms: None,
            parent_tool_use_id: parent_tool_id.map(str::to_string),
            tool_use_result: Some(payload),
        });
        return;
    }
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
            parent_tool_use_id: parent_tool_id.map(str::to_string),
            model: None,
            stop_reason: None,
            message_usage: None,
        });
        return;
    }
    if item_type == "userMessage" || item_type == "reasoning" {
        return; // user echo / reasoning already streamed via deltas
    }
    // Context compaction completed. In 0.137 this surfaces as a `contextCompaction` item (the
    // legacy `thread/compacted` notification is deprecated); we close the loop on our own
    // `thread/compact/start` request with a user-visible notice. The item carries only an id.
    if item_type == "contextCompaction" {
        out.push(BusEvent::CommandOutput {
            run_id: run_id.to_string(),
            content: "[notice] context compacted".to_string(),
        });
        return;
    }
    // Codex multi-agent collab tool call finished → close the "Agent" subagent card. status
    // "failed" → error; otherwise success. The rich collab payload rides on output + tool_use_result.
    if item_type == "collabAgentToolCall" {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = match item.get("status").and_then(|v| v.as_str()) {
            Some("failed") => "error",
            _ => "success",
        };
        let payload = collab_input(item);
        out.push(BusEvent::ToolEnd {
            run_id: run_id.to_string(),
            tool_use_id: id,
            tool_name: "Agent".to_string(),
            output: json!({ "content": payload.clone() }),
            status: status.to_string(),
            duration_ms: None,
            parent_tool_use_id: parent_tool_id.map(str::to_string),
            tool_use_result: Some(payload),
        });
        return;
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
            parent_tool_use_id: parent_tool_id.map(str::to_string),
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

/// Map `hook/started` | `hook/completed` to a `CodexHookRun` event. Both notifications carry the
/// full HookRunSummary at `run`; `run.id` is stable across the started→completed pair so the
/// frontend upserts one card. On `started` we force status "running" (the summary's status field
/// is not yet terminal); on `completed` we pass through the terminal HookRunStatus.
fn hook_run_event(run_id: &str, method: &str, params: &Value) -> Option<BusEvent> {
    let run = params.get("run")?;
    let hook_id = run.get("id").and_then(|v| v.as_str())?.to_string();
    let event_name = run
        .get("eventName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = if method == "hook/started" {
        "running".to_string()
    } else {
        run.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed")
            .to_string()
    };
    let status_message = run
        .get("statusMessage")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let duration_ms = run.get("durationMs").and_then(|v| v.as_u64());
    Some(BusEvent::CodexHookRun {
        run_id: run_id.to_string(),
        hook_id,
        event_name,
        status,
        status_message,
        duration_ms,
    })
}

/// Build a one-line `[guardian]` notice from an `item/autoApprovalReview/completed` payload.
/// The shape is [UNSTABLE] upstream, so every field is best-effort: we summarize the reviewed
/// action (command / patch / mcp tool / network) and the review status/rationale when present,
/// and fall back to a bare notice rather than dropping the signal.
fn guardian_notice(params: &Value) -> Option<String> {
    let action = params.get("action");
    let what = action
        .and_then(|a| a.get("type"))
        .and_then(|v| v.as_str())
        .map(|t| match t {
            "command" | "execve" => {
                let cmd = action
                    .and_then(|a| a.get("command").or_else(|| a.get("program")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("command");
                format!("command `{cmd}`")
            }
            "applyPatch" => "file edit".to_string(),
            "mcpToolCall" => {
                let tool = action
                    .and_then(|a| a.get("toolName"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                format!("MCP tool `{tool}`")
            }
            "networkAccess" => {
                let host = action
                    .and_then(|a| a.get("host"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("network");
                format!("network access to {host}")
            }
            other => other.to_string(),
        })
        .unwrap_or_else(|| "action".to_string());
    let status = params
        .get("review")
        .and_then(|r| r.get("status"))
        .and_then(|v| v.as_str());
    let rationale = params
        .get("review")
        .and_then(|r| r.get("rationale"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let head = match status {
        Some(s) => format!("[guardian] auto-review {s}: {what}"),
        None => format!("[guardian] auto-review: {what}"),
    };
    Some(match rationale {
        Some(r) => format!("{head} — {r}"),
        None => head,
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
        s.parse_line("run1", r#"{"id":2,"result":{"thread":{"id":"th-123"}}}"#);
        s
    }

    /// Default (no-op) overrides for the common case.
    fn no_overrides() -> CodexTurnOverrides {
        CodexTurnOverrides::default()
    }

    /// No skill directives — the common case for turns that aren't skill invocations.
    fn no_skills() -> &'static [CodexSkillRef] {
        &[]
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
    fn uncorrelated_thread_started_is_ignored() {
        let mut s = CodexAppServer::new();
        let out = s.parse_line(
            "run1",
            r#"{"method":"thread/started","params":{"thread":{"id":"th-123"}}}"#,
        );
        assert!(out.thread_id.is_none());
        assert_eq!(s.phase, Phase::Opening);
    }

    #[test]
    fn user_turn_requires_thread_id() {
        let mut s = CodexAppServer::new();
        assert!(s
            .frame_user_turn("hi", &[], no_skills(), &no_overrides())
            .is_empty());
        let mut s = ready_server();
        let msgs = s.frame_user_turn("hi", &[], no_skills(), &no_overrides());
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
        let msgs = s.frame_user_turn(
            "describe this",
            &["/x/a.png".to_string()],
            no_skills(),
            &no_overrides(),
        );
        let input = &msgs[0]["params"]["input"];
        // text first, then one localImage item per path.
        assert_eq!(input[0]["type"], "text");
        assert_eq!(input[0]["text"], "describe this");
        assert_eq!(input[1]["type"], "localImage");
        assert_eq!(input[1]["path"], "/x/a.png");
        // No images → no localImage items.
        let none = s.frame_user_turn("hi", &[], no_skills(), &no_overrides());
        assert_eq!(none[0]["params"]["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn user_turn_leads_input_with_skill_items() {
        let mut s = ready_server();
        // P-2 skill send: Codex only triggers a skill via a typed {type:"skill"} item, so skills
        // must LEAD params.input, then the text item, then any localImage items.
        let skills = [
            CodexSkillRef {
                name: "review".into(),
                path: "/skills/review.md".into(),
            },
            CodexSkillRef {
                name: "lint".into(),
                path: "/skills/lint.md".into(),
            },
        ];
        let msgs = s.frame_user_turn("go", &["/x/a.png".to_string()], &skills, &no_overrides());
        let input = &msgs[0]["params"]["input"];
        // input[0..2] = skill items in order with name+path; input[2] = text; input[3] = image.
        assert_eq!(input[0]["type"], "skill");
        assert_eq!(input[0]["name"], "review");
        assert_eq!(input[0]["path"], "/skills/review.md");
        assert_eq!(input[1]["type"], "skill");
        assert_eq!(input[1]["name"], "lint");
        assert_eq!(input[1]["path"], "/skills/lint.md");
        assert_eq!(input[2]["type"], "text");
        assert_eq!(input[2]["text"], "go");
        assert_eq!(input[3]["type"], "localImage");
        assert_eq!(input[3]["path"], "/x/a.png");

        // Empty skills → no regression: input[0] is the text item (no skill items leading).
        let plain = s.frame_user_turn("go", &[], no_skills(), &no_overrides());
        let pin = &plain[0]["params"]["input"];
        assert_eq!(pin.as_array().unwrap().len(), 1);
        assert_eq!(pin[0]["type"], "text");
        assert_eq!(pin[0]["text"], "go");
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
        let msgs = s.frame_user_turn("go", &[], no_skills(), &overrides);
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
        let msgs = s.frame_user_turn("go", &[], no_skills(), &partial);
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

        let first = s.frame_user_turn("hi", &[], no_skills(), &no_overrides());
        let p = &first[0]["params"];
        assert_eq!(p["model"], "gpt-5-codex");
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["effort"], "high");
        assert_eq!(p["sandboxPolicy"]["type"], "workspaceWrite");

        // Second turn: defaults already persisted server-side → not re-sent.
        let second = s.frame_user_turn("again", &[], no_skills(), &no_overrides());
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
        let msgs = s.frame_user_turn("hi", &[], no_skills(), &overrides);
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
        let msgs = s.frame_user_turn("go", &[], no_skills(), &no_overrides());
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
        let msgs = s.frame_user_turn("go", &[], no_skills(), &no_overrides());
        let policy = &msgs[0]["params"]["sandboxPolicy"];
        assert_eq!(policy["type"], "workspaceWrite");
        assert_eq!(policy["writableRoots"][0], "/extra");
        // No add_dirs and no sandbox → no policy at all.
        let mut s2 = ready_server();
        let m2 = s2.frame_user_turn("go", &[], no_skills(), &no_overrides());
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
        let line = r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{"threadId":"th-123","turnId":"turn-1","itemId":"call_1","startedAtMs":1,"reason":"allow write?","command":"echo hi","cwd":"/tmp"}}"#;
        let out = s.parse_line("run1", line);
        let pi = out.interactive.first().expect("interactive");
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
        let resp = s
            .frame_response(PendingKind::Permission, "0", json!({"behavior":"allow"}))
            .unwrap();
        assert_eq!(resp.primary["id"], 0);
        assert_eq!(resp.primary["result"]["decision"], "accept");
        assert!(s.pending.contains_key("0"));
        assert!(s.commit_response("0"));
        assert!(!s.pending.contains_key("0"));
        // Deny path.
        let mut s2 = ready_server();
        s2.parse_line("run1", line);
        let resp2 = s2
            .frame_response(PendingKind::Permission, "0", json!({"behavior":"deny"}))
            .unwrap();
        assert_eq!(resp2.primary["result"]["decision"], "decline");

        let mut s3 = ready_server();
        s3.parse_line("run1", line);
        let resp3 = s3
            .frame_response(
                PendingKind::Permission,
                "0",
                json!({"behavior":"deny", "interrupt":true}),
            )
            .unwrap();
        assert_eq!(resp3.primary["result"]["decision"], "cancel");
    }

    #[test]
    fn interactive_response_requires_known_request_and_matching_kind() {
        let mut server = ready_server();
        let unknown = server.frame_response(
            PendingKind::Permission,
            "missing",
            json!({"behavior":"allow"}),
        );
        assert!(unknown
            .unwrap_err()
            .contains("unknown interactive request_id"));

        server.parse_line(
            "run1",
            r#"{"id":7,"method":"mcpServer/elicitation/request","params":{"threadId":"th-123","serverName":"srv","message":"Pick"}}"#,
        );
        let mismatch =
            server.frame_response(PendingKind::Permission, "7", json!({"behavior":"allow"}));
        assert!(mismatch.unwrap_err().contains("kind mismatch"));
        assert!(server.pending.contains_key("7"));
    }

    #[test]
    fn interactive_responses_prepare_retry_and_commit_independently() {
        let mut server = ready_server();
        for id in [10, 11] {
            server.parse_line(
                "run1",
                &format!(
                    r#"{{"id":{id},"method":"mcpServer/elicitation/request","params":{{"threadId":"th-123","serverName":"srv","message":"Pick"}}}}"#
                ),
            );
        }

        let first = server
            .frame_response(PendingKind::Elicitation, "10", json!({"action":"decline"}))
            .unwrap();
        let retry = server
            .frame_response(PendingKind::Elicitation, "10", json!({"action":"decline"}))
            .unwrap();
        assert_eq!(first.primary, retry.primary);
        assert!(server.pending.contains_key("10"));
        assert!(server.pending.contains_key("11"));

        assert!(server.commit_response("10"));
        assert!(!server.pending.contains_key("10"));
        assert!(server.pending.contains_key("11"));
        assert!(!server.commit_response("10"));
    }

    #[test]
    fn interactive_cancel_uses_raw_id_and_requires_commit() {
        let mut server = ready_server();
        server.parse_line(
            "run1",
            r#"{"id":"request-10","method":"mcpServer/elicitation/request","params":{"threadId":"th-123","serverName":"srv","message":"Pick"}}"#,
        );

        let frame = server.frame_cancel("request-10").unwrap();
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["id"], "request-10");
        assert_eq!(frame["error"]["code"], -32000);
        assert_eq!(
            frame["error"]["message"],
            "Interactive request cancelled by user"
        );
        assert!(server.pending.contains_key("request-10"));

        assert!(server.commit_response("request-10"));
        assert!(!server.pending.contains_key("request-10"));
        assert!(server.frame_cancel("request-10").is_err());
    }

    #[test]
    fn interactive_cancel_rejects_unknown_request() {
        let server = ready_server();
        assert!(server
            .frame_cancel("missing")
            .unwrap_err()
            .contains("unknown interactive request_id"));
    }

    #[test]
    fn legacy_approval_without_thread_id_remains_supported() {
        let mut server = ready_server();
        let out = server.parse_line(
            "r",
            r#"{"id":87,"method":"execCommandApproval","params":{"itemId":"cmd-1","command":"pwd"}}"#,
        );

        assert!(matches!(
            out.interactive.first(),
            Some(PendingInteractive {
                kind: PendingKind::Permission,
                ..
            })
        ));
        assert!(matches!(
            out.events.first(),
            Some(BusEvent::PermissionPrompt { .. })
        ));
        assert!(out.outbound.is_empty());
    }

    #[test]
    fn modern_interactive_request_without_thread_id_is_rejected() {
        for (index, method) in [
            M_CMD_APPROVAL,
            M_FILE_APPROVAL,
            M_PERM_APPROVAL,
            M_REQUEST_USER_INPUT,
            M_ELICITATION,
        ]
        .into_iter()
        .enumerate()
        {
            for params in [json!({}), json!({ "threadId": "" })] {
                let mut server = ready_server();
                let out = server.parse_line(
                    "r",
                    &json!({
                        "id": 90 + index,
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                );

                assert!(out.events.is_empty(), "method={method}");
                assert!(out.interactive.is_empty(), "method={method}");
                assert_eq!(out.outbound[0]["error"]["code"], -32602, "method={method}");
                assert!(server.pending.is_empty(), "method={method}");
            }
        }
    }

    #[test]
    fn unclaimed_interactive_request_evicts_notification_when_thread_buffer_is_full() {
        let mut server = ready_server();
        for index in 0..MAX_DEFERRED_MESSAGES_PER_THREAD {
            let line = json!({
                "method": "item/agentMessage/delta",
                "params": {"threadId": "another-thread", "delta": index.to_string()}
            });
            assert!(server.parse_line("r", &line.to_string()).events.is_empty());
        }
        let out = server.parse_line(
            "r",
            r#"{"id":88,"method":"item/commandExecution/requestApproval","params":{"threadId":"another-thread","turnId":"turn-1","itemId":"cmd-1","startedAtMs":1,"command":"touch foreign"}}"#,
        );

        assert!(out.events.is_empty());
        assert!(out.interactive.is_empty());
        assert!(out.outbound.is_empty());
        assert!(server.pending.is_empty());
        let queue = &server.deferred_thread_messages["another-thread"];
        assert_eq!(queue.len(), MAX_DEFERRED_MESSAGES_PER_THREAD);
        assert_eq!(
            queue
                .iter()
                .filter(|entry| entry.message.is_interactive())
                .count(),
            1
        );
    }

    #[test]
    fn permissions_approval_returns_granted_subset_or_empty_profile() {
        let line = r#"{"id":61,"method":"item/permissions/requestApproval","params":{"threadId":"th-123","turnId":"turn-1","itemId":"call-1","cwd":"/project","reason":"Allow workspace writes","permissions":{"fileSystem":{"write":["/project","/shared"]},"network":{"enabled":true}}}}"#;

        let mut allowed = ready_server();
        let out = allowed.parse_line("run1", line);
        let pi = out.interactive.first().expect("interactive");
        assert_eq!(pi.kind, PendingKind::Permission);
        match &out.events[0] {
            BusEvent::PermissionPrompt {
                tool_name,
                tool_input,
                decision_reason,
                ..
            } => {
                assert_eq!(tool_name, "RequestPermissions");
                assert_eq!(tool_input["cwd"], "/project");
                assert_eq!(tool_input["permissions"]["network"]["enabled"], true);
                assert_eq!(decision_reason, "Allow workspace writes");
            }
            event => panic!("expected PermissionPrompt, got {event:?}"),
        }
        let response = allowed
            .frame_response(
                PendingKind::Permission,
                &pi.request_id,
                json!({"behavior":"allow"}),
            )
            .unwrap();
        assert_eq!(response.primary["id"], 61);
        assert_eq!(
            response.primary["result"]["permissions"],
            json!({
                "fileSystem": {"write": ["/project", "/shared"]},
                "network": {"enabled": true}
            })
        );
        assert_eq!(response.primary["result"]["scope"], "turn");
        assert!(response.primary["result"].get("decision").is_none());

        let mut denied = ready_server();
        denied.parse_line(
            "run1",
            r#"{"method":"turn/started","params":{"turn":{"id":"turn-1"}}}"#,
        );
        let out = denied.parse_line("run1", line);
        let request_id = out
            .interactive
            .first()
            .expect("interactive")
            .request_id
            .clone();
        let next_id_before_prepare = denied.next_client_id;
        let response = denied
            .frame_response(
                PendingKind::Permission,
                &request_id,
                json!({"behavior":"deny", "interrupt":true}),
            )
            .unwrap();
        assert_eq!(denied.next_client_id, next_id_before_prepare);
        assert_eq!(response.primary["result"]["permissions"], json!({}));
        assert_eq!(response.primary["result"]["scope"], "turn");
        assert!(response.primary["result"].get("decision").is_none());
        assert!(response.interrupt_after_commit);
        assert!(denied.pending.contains_key(&request_id));
        assert!(denied.commit_response(&request_id));
        assert!(!denied.pending.contains_key(&request_id));
        let follow_up = denied.frame_interrupt();
        assert_eq!(denied.next_client_id, next_id_before_prepare + 1);
        assert_eq!(follow_up[0]["method"], "turn/interrupt");
        assert_eq!(follow_up[0]["params"]["threadId"], "th-123");
        assert_eq!(follow_up[0]["params"]["turnId"], "turn-1");
        assert!(denied
            .frame_response(
                PendingKind::Permission,
                &request_id,
                json!({"behavior":"deny", "interrupt":true}),
            )
            .is_err());
    }

    #[test]
    fn request_user_input_to_ask_question_and_back() {
        let mut s = ready_server();
        let line = r#"{"id":0,"method":"item/tool/requestUserInput","params":{"threadId":"th-123","turnId":"turn-1","itemId":"ask-1","isBlocking":true,"questions":[{"id":"word","header":"Word","question":"Which word?","isOther":true,"isSecret":false,"options":[{"label":"FOO","description":"Select FOO."},{"label":"BAR","description":"Select BAR."}]}]}}"#;
        let out = s.parse_line("run1", line);
        let pi = out.interactive.first().expect("interactive");
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
        let resp = s
            .frame_response(
                PendingKind::UserInput,
                "0",
                json!({"answers": {"word": ["FOO"]}}),
            )
            .unwrap();
        assert_eq!(resp.primary["id"], 0);
        assert_eq!(
            resp.primary["result"]["answers"]["word"]["answers"][0],
            "FOO"
        );
    }

    #[test]
    fn elicitation_request() {
        let mut s = ready_server();
        let line = r#"{"id":1,"method":"mcpServer/elicitation/request","params":{"threadId":"th-123","serverName":"srv","mode":"form","message":"Pick","requestedSchema":{"type":"object"}}}"#;
        let out = s.parse_line("run1", line);
        assert_eq!(
            out.interactive.first().expect("interactive").kind,
            PendingKind::Elicitation
        );
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
        let resp = s
            .frame_response(PendingKind::Elicitation, "1", json!({"action":"decline"}))
            .unwrap();
        assert_eq!(resp.primary["result"]["action"], "decline");
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
    fn collab_agent_item_lifecycle() {
        let mut s = ready_server();
        // collabAgentToolCall renders as an "Agent" subagent card with the rich collab input
        // (codexCollab flag + per-agent states), not item_tool_name's plain-tool shape.
        let started = s.parse_line(
            "r",
            r#"{"method":"item/started","params":{"item":{"id":"col1","type":"collabAgentToolCall","tool":"spawnAgent","prompt":"explore X","agentsStates":{"t2":{"status":"running","message":null}}}}}"#,
        );
        match &started.events[0] {
            BusEvent::ToolStart {
                tool_name, input, ..
            } => {
                assert_eq!(tool_name, "Agent");
                assert_eq!(input["codexCollab"], true);
                assert_eq!(input["operation"], "spawnAgent");
                assert_eq!(input["prompt"], "explore X");
                assert_eq!(input["agents"][0]["thread_id"], "t2");
                assert_eq!(input["agents"][0]["status"], "running");
            }
            e => panic!("expected ToolStart, got {e:?}"),
        }
        // completed with status "completed" → success; rich payload rides on tool_use_result.
        let completed = s.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"item":{"id":"col1","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","agentsStates":{"t2":{"status":"completed","message":"done"}}}}}"#,
        );
        match &completed.events[0] {
            BusEvent::ToolEnd {
                tool_name,
                status,
                tool_use_result,
                ..
            } => {
                assert_eq!(tool_name, "Agent");
                assert_eq!(status, "success");
                assert_eq!(tool_use_result.as_ref().unwrap()["codexCollab"], true);
                assert_eq!(tool_use_result.as_ref().unwrap()["operation"], "spawnAgent");
            }
            e => panic!("expected ToolEnd, got {e:?}"),
        }
    }

    #[test]
    fn spawned_thread_identity_and_activity_are_correlated() {
        let mut server = ready_server();
        let spawn = server.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"threadId":"th-123","item":{"id":"agent-1","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","senderThreadId":"th-123","receiverThreadIds":["child-1"],"agentsStates":{"child-1":{"status":"running","message":null}}}}}"#,
        );
        let read = spawn
            .outbound
            .iter()
            .find(|frame| frame["method"] == "thread/read")
            .expect("spawn schedules identity lookup");
        assert_eq!(read["params"]["threadId"], "child-1");
        assert_eq!(read["params"]["includeTurns"], false);

        let read_id = read["id"].as_i64().unwrap();
        let info = server.parse_line(
            "r",
            &format!(
                r#"{{"id":{read_id},"result":{{"thread":{{"id":"child-1","agentNickname":"Dewey","agentRole":"explorer","parentThreadId":"th-123"}}}}}}"#
            ),
        );
        match &info.events[0] {
            BusEvent::CodexAgentInfo {
                thread_id,
                nickname,
                role,
                parent_thread_id,
                ..
            } => {
                assert_eq!(thread_id, "child-1");
                assert_eq!(nickname.as_deref(), Some("Dewey"));
                assert_eq!(role.as_deref(), Some("explorer"));
                assert_eq!(parent_thread_id.as_deref(), Some("th-123"));
            }
            event => panic!("expected CodexAgentInfo, got {event:?}"),
        }

        let child_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"id":"cmd-1","type":"commandExecution","command":"rg TODO"}}}"#,
        );
        match &child_tool.events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("agent-1")),
            event => panic!("expected nested ToolStart, got {event:?}"),
        }

        let child_completed = server.parse_line(
            "r",
            r#"{"method":"turn/completed","params":{"threadId":"child-1"}}"#,
        );
        assert!(child_completed.lifecycle.is_none());
        let root_completed = server.parse_line(
            "r",
            r#"{"method":"turn/completed","params":{"threadId":"th-123"}}"#,
        );
        assert_eq!(
            root_completed.lifecycle,
            Some(LifecycleSignal::TurnCompleted)
        );
    }

    #[test]
    fn stable_subagent_activity_registers_child_thread() {
        let mut server = ready_server();
        let started = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"agent-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        match &started.events[0] {
            BusEvent::ToolStart {
                tool_name, input, ..
            } => {
                assert_eq!(tool_name, "Agent");
                assert_eq!(input["operation"], "spawnAgent");
                assert_eq!(input["agentPath"], "/root/explorer");
                assert_eq!(input["receiverThreadIds"][0], "child-1");
            }
            event => panic!("expected Agent ToolStart, got {event:?}"),
        }
        assert_eq!(started.outbound[0]["method"], "thread/read");
        assert_eq!(started.outbound[0]["params"]["threadId"], "child-1");

        let child_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        match &child_tool.events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("agent-1")),
            event => panic!("expected nested ToolStart, got {event:?}"),
        }
    }

    #[test]
    fn child_notification_before_spawn_registration_is_replayed_in_parent_card() {
        let mut server = ready_server();
        let early = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        assert!(early.events.is_empty());
        assert_eq!(server.deferred_thread_messages["child-1"].len(), 1);

        let registered = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        assert_eq!(registered.events.len(), 2);
        match &registered.events[1] {
            BusEvent::ToolStart {
                tool_use_id,
                parent_tool_use_id,
                ..
            } => {
                assert_eq!(tool_use_id, "cmd-1");
                assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1"));
            }
            event => panic!("expected replayed child ToolStart, got {event:?}"),
        }
        assert!(!server.deferred_thread_messages.contains_key("child-1"));
    }

    #[test]
    fn child_approval_before_spawn_registration_is_replayed_without_early_rejection() {
        let mut server = ready_server();
        let early = server.parse_line(
            "r",
            r#"{"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"child-turn","itemId":"cmd-1","startedAtMs":1,"command":"touch file"}}"#,
        );
        assert!(early.events.is_empty());
        assert!(early.interactive.is_empty());
        assert!(early.outbound.is_empty());
        assert!(server.pending.is_empty());

        let registered = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        assert_eq!(registered.interactive.len(), 1);
        assert_eq!(registered.interactive[0].request_id, "77");
        assert!(server.pending.contains_key("77"));
        match &registered.events[1] {
            BusEvent::PermissionPrompt {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1")),
            event => panic!("expected replayed child PermissionPrompt, got {event:?}"),
        }
        assert!(registered
            .outbound
            .iter()
            .all(|frame| frame.get("error").is_none()));
    }

    #[test]
    fn unclaimed_thread_buffer_caps_distinct_threads() {
        let mut server = ready_server();
        for index in 0..=MAX_UNCLAIMED_THREADS {
            let line = json!({
                "method": "item/agentMessage/delta",
                "params": {"threadId": format!("foreign-{index}"), "delta": "x"}
            });
            assert!(server.parse_line("r", &line.to_string()).events.is_empty());
        }

        assert_eq!(server.deferred_thread_messages.len(), MAX_UNCLAIMED_THREADS);
        assert!(server
            .deferred_thread_messages
            .contains_key(&format!("foreign-{MAX_UNCLAIMED_THREADS}")));
        assert!(!server.deferred_thread_messages.contains_key("foreign-0"));
    }

    #[test]
    fn deferred_payload_budget_rejects_oversized_interactive_request() {
        let mut server = ready_server();
        let out = server.parse_line(
            "r",
            &json!({
                "id": 99,
                "method": M_FILE_APPROVAL,
                "params": {
                    "threadId": "child-oversized",
                    "changes": "x".repeat(MAX_DEFERRED_PAYLOAD_BYTES)
                }
            })
            .to_string(),
        );

        assert_eq!(out.outbound[0]["id"], 99);
        assert_eq!(out.outbound[0]["error"]["code"], -32600);
        assert!(server.deferred_thread_messages.is_empty());
        assert_eq!(server.deferred_payload_bytes, 0);
    }

    #[test]
    fn expired_deferred_interactive_request_receives_error_and_frees_capacity() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-old","command":"pwd"}}"#,
        );
        server
            .deferred_thread_messages
            .get_mut("child-old")
            .unwrap()[0]
            .received_at = Instant::now() - DEFERRED_MESSAGE_TTL - Duration::from_secs(1);

        let out = server.parse_line(
            "r",
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"child-new","delta":"x"}}"#,
        );

        assert_eq!(out.outbound[0]["id"], 77);
        assert_eq!(
            out.outbound[0]["error"]["message"],
            "Interactive request expired"
        );
        assert!(!server.deferred_thread_messages.contains_key("child-old"));
        assert!(server.deferred_thread_messages.contains_key("child-new"));
    }

    #[test]
    fn expired_interactive_is_rejected_instead_of_replayed_on_registration() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"id":78,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-old","command":"pwd"}}"#,
        );
        server
            .deferred_thread_messages
            .get_mut("child-old")
            .unwrap()[0]
            .received_at = Instant::now() - DEFERRED_MESSAGE_TTL - Duration::from_secs(1);

        let registered = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-old"}}}"#,
        );

        assert!(registered.interactive.is_empty());
        assert!(!server.pending.contains_key("78"));
        assert_eq!(registered.outbound[0]["id"], 78);
        assert_eq!(
            registered.outbound[0]["error"]["message"],
            "Interactive request expired"
        );
    }

    #[test]
    fn spawned_thread_lookup_is_deduplicated() {
        let mut server = ready_server();
        let line = r#"{"method":"item/completed","params":{"item":{"id":"agent-1","type":"collabAgentToolCall","tool":"spawnAgent","receiverThreadIds":["child-1"]}}}"#;
        assert_eq!(server.parse_line("r", line).outbound.len(), 1);
        assert!(server.parse_line("r", line).outbound.is_empty());
    }

    #[test]
    fn wait_agent_does_not_reparent_spawned_thread() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        let wait = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"id":"wait-1","type":"collabAgentToolCall","tool":"wait","receiverThreadIds":["child-1"]}}}"#,
        );
        assert!(wait.outbound.is_empty());

        let child_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        match &child_tool.events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1")),
            event => panic!("expected nested ToolStart, got {event:?}"),
        }
    }

    #[test]
    fn later_subagent_activity_does_not_reparent_spawned_thread() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        let interacted = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"interaction-1","kind":"interacted","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );
        assert!(interacted.outbound.is_empty());
        match &interacted.events[0] {
            BusEvent::ToolStart { input, .. } => {
                assert_eq!(input["operation"], "interacted");
                assert_eq!(input["activityKind"], "interacted");
                assert_eq!(input["status"], "inProgress");
            }
            event => panic!("expected interaction ToolStart, got {event:?}"),
        }

        let child_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        match &child_tool.events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1")),
            event => panic!("expected nested ToolStart, got {event:?}"),
        }
    }

    #[test]
    fn interrupted_subagent_activity_maps_to_terminal_error() {
        let mut server = ready_server();
        let completed = server.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"interrupt-1","kind":"interrupted","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        match &completed.events[0] {
            BusEvent::ToolEnd {
                status,
                tool_use_result,
                ..
            } => {
                assert_eq!(status, "error");
                let result = tool_use_result.as_ref().unwrap();
                assert_eq!(result["operation"], "interrupted");
                assert_eq!(result["activityKind"], "interrupted");
                assert_eq!(result["status"], "interrupted");
                assert_eq!(result["agents"][0]["status"], "interrupted");
            }
            event => panic!("expected interrupted ToolEnd, got {event:?}"),
        }
    }

    #[test]
    fn nested_spawn_stays_in_top_level_agent_card() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );
        let nested = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"child-1","item":{"type":"subAgentActivity","id":"spawn-2","kind":"started","agentThreadId":"grandchild-1","agentPath":"/root/explorer/worker"}}}"#,
        );
        assert_eq!(nested.outbound[0]["params"]["threadId"], "grandchild-1");

        let grandchild_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"grandchild-1","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        match &grandchild_tool.events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1")),
            event => panic!("expected nested ToolStart, got {event:?}"),
        }
    }

    #[test]
    fn child_state_notifications_do_not_mutate_parent_state() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        for line in [
            r#"{"method":"thread/goal/updated","params":{"threadId":"child-1","goal":{"objective":"child"}}}"#,
            r#"{"method":"turn/plan/updated","params":{"threadId":"child-1","turnId":"child-turn","plan":[]}}"#,
            r#"{"method":"turn/diff/updated","params":{"threadId":"child-1","turnId":"child-turn","diff":"child diff"}}"#,
            r#"{"method":"account/rateLimits/updated","params":{"threadId":"child-1","rateLimits":{}}}"#,
        ] {
            assert!(server.parse_line("r", line).events.is_empty());
        }
    }

    #[test]
    fn child_interactive_request_is_nested_in_agent_card() {
        let mut server = ready_server();
        server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"th-123","item":{"type":"subAgentActivity","id":"spawn-1","kind":"started","agentThreadId":"child-1","agentPath":"/root/explorer"}}}"#,
        );

        let request = server.parse_line(
            "r",
            r#"{"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"child-turn","itemId":"cmd-1","startedAtMs":1,"command":"touch file"}}"#,
        );
        match &request.events[0] {
            BusEvent::PermissionPrompt {
                parent_tool_use_id, ..
            } => assert_eq!(parent_tool_use_id.as_deref(), Some("spawn-1")),
            event => panic!("expected nested PermissionPrompt, got {event:?}"),
        }
    }

    #[test]
    fn unrelated_thread_notifications_do_not_mutate_the_run() {
        let mut server = ready_server();
        let foreign_tool = server.parse_line(
            "r",
            r#"{"method":"item/started","params":{"threadId":"another-thread","item":{"id":"cmd-1","type":"commandExecution","command":"pwd"}}}"#,
        );
        assert!(foreign_tool.events.is_empty());

        let foreign_turn = server.parse_line(
            "r",
            r#"{"method":"turn/completed","params":{"threadId":"another-thread"}}"#,
        );
        assert!(foreign_turn.lifecycle.is_none());
    }

    #[test]
    fn codex_agent_info_serializes_as_bus_contract() {
        let event = BusEvent::CodexAgentInfo {
            run_id: "r".into(),
            thread_id: "child-1".into(),
            nickname: Some("Dewey".into()),
            role: None,
            parent_thread_id: Some("root".into()),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "codex_agent_info");
        assert_eq!(value["thread_id"], "child-1");
        assert!(value.get("role").is_none());
        assert!(crate::agent::claude_protocol::validate_bus_event(&event).is_none());
    }

    #[test]
    fn collab_agent_failed_maps_to_error_status() {
        let mut s = ready_server();
        let completed = s.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"item":{"id":"col1","type":"collabAgentToolCall","tool":"spawnAgent","status":"failed"}}}"#,
        );
        match &completed.events[0] {
            BusEvent::ToolEnd {
                tool_name, status, ..
            } => {
                assert_eq!(tool_name, "Agent");
                assert_eq!(status, "error");
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
        // NOTE: distinct from `collabAgentToolCall` above, which DOES render an Agent card here.
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

    // ── Wave-4 ecosystem notifications: hook lifecycle, MCP progress, guardian, skills ──

    #[test]
    fn hook_started_emits_running_codex_hook_run() {
        let mut s = ready_server();
        // HookStartedNotification: { threadId, turnId, run: HookRunSummary }. On started the
        // summary's own status is "running"; the parser forces "running" regardless.
        let out = s.parse_line(
            "r",
            r#"{"method":"hook/started","params":{"threadId":"th-123","turnId":"t-1","run":{
                "id":"hook-run-7","eventName":"preToolUse","handlerType":"command",
                "executionMode":"blocking","scope":"project","sourcePath":"/x/.codex/hooks.toml",
                "source":"local","displayOrder":0,"status":"running","statusMessage":null,
                "startedAt":1711900000,"completedAt":null,"durationMs":null,"entries":[]
            }}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CodexHookRun {
                hook_id,
                event_name,
                status,
                status_message,
                duration_ms,
                ..
            } => {
                assert_eq!(hook_id, "hook-run-7");
                assert_eq!(event_name, "preToolUse");
                assert_eq!(status, "running");
                assert_eq!(status_message.as_deref(), None);
                assert_eq!(*duration_ms, None);
            }
            e => panic!("expected CodexHookRun, got {e:?}"),
        }
    }

    #[test]
    fn hook_completed_passes_through_terminal_status_and_duration() {
        let mut s = ready_server();
        // HookCompletedNotification carries the terminal HookRunStatus + durationMs. The stable
        // run.id matches the started event so the frontend upserts a single card.
        let out = s.parse_line(
            "r",
            r#"{"method":"hook/completed","params":{"threadId":"th-123","turnId":"t-1","run":{
                "id":"hook-run-7","eventName":"postToolUse","handlerType":"command",
                "executionMode":"blocking","scope":"project","sourcePath":"/x/.codex/hooks.toml",
                "source":"local","displayOrder":0,"status":"blocked","statusMessage":"denied by policy",
                "startedAt":1711900000,"completedAt":1711900002,"durationMs":1234,"entries":[]
            }}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CodexHookRun {
                hook_id,
                event_name,
                status,
                status_message,
                duration_ms,
                ..
            } => {
                assert_eq!(hook_id, "hook-run-7");
                assert_eq!(event_name, "postToolUse");
                assert_eq!(status, "blocked");
                assert_eq!(status_message.as_deref(), Some("denied by policy"));
                assert_eq!(*duration_ms, Some(1234));
            }
            e => panic!("expected CodexHookRun, got {e:?}"),
        }
    }

    #[test]
    fn hook_event_missing_run_or_id_is_dropped() {
        let mut s = ready_server();
        // No `run` object → nothing to surface (defensive against partial upstream payloads).
        let out = s.parse_line(
            "r",
            r#"{"method":"hook/started","params":{"threadId":"th-123"}}"#,
        );
        assert!(out.events.is_empty());
        // `run` present but no stable id → can't key a card, drop.
        let out = s.parse_line(
            "r",
            r#"{"method":"hook/completed","params":{"run":{"eventName":"stop","status":"completed"}}}"#,
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn mcp_tool_call_progress_to_tool_output_delta() {
        let mut s = ready_server();
        // McpToolCallProgressNotification: { threadId, turnId, itemId, message }. Appends to the
        // open MCP tool card keyed by itemId, with a trailing newline like shell output.
        let out = s.parse_line(
            "r",
            r#"{"method":"item/mcpToolCall/progress","params":{"threadId":"th-123","turnId":"t-1","itemId":"call_mcp_1","message":"fetching page 3/10"}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::ToolOutputDelta {
                tool_use_id, delta, ..
            } => {
                assert_eq!(tool_use_id, "call_mcp_1");
                assert_eq!(delta, "fetching page 3/10\n");
            }
            e => panic!("expected ToolOutputDelta, got {e:?}"),
        }
        // Empty itemId or empty message → no event (can't key a card / nothing to append).
        let out = s.parse_line(
            "r",
            r#"{"method":"item/mcpToolCall/progress","params":{"itemId":"","message":"x"}}"#,
        );
        assert!(out.events.is_empty());
        let out = s.parse_line(
            "r",
            r#"{"method":"item/mcpToolCall/progress","params":{"itemId":"call_mcp_1","message":""}}"#,
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn guardian_auto_review_to_command_output() {
        let mut s = ready_server();
        // ItemGuardianApprovalReviewCompletedNotification [UNSTABLE]: action describes the reviewed
        // action, review carries status + rationale. Command action → "command `...`".
        let out = s.parse_line(
            "r",
            r#"{"method":"item/autoApprovalReview/completed","params":{
                "threadId":"th-123","turnId":"t-1","startedAtMs":1,"completedAtMs":2,
                "reviewId":"rev-1","targetItemId":"call_1","decisionSource":"model",
                "action":{"type":"command","source":"shell","command":"rm -rf build","cwd":"/x"},
                "review":{"status":"approved","riskLevel":"low","userAuthorization":null,"rationale":"safe within workspace"}
            }}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(
                    content,
                    "[guardian] auto-review approved: command `rm -rf build` — safe within workspace"
                );
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    #[test]
    fn guardian_auto_review_action_variants() {
        let mut s = ready_server();
        // applyPatch → "file edit"; no rationale → no trailing "— ...".
        let out = s.parse_line(
            "r",
            r#"{"method":"item/autoApprovalReview/completed","params":{
                "action":{"type":"applyPatch","cwd":"/x","files":["/x/a.rs"]},
                "review":{"status":"denied","rationale":null}
            }}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[guardian] auto-review denied: file edit");
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        // mcpToolCall → uses toolName; networkAccess → uses host.
        let out = s.parse_line(
            "r",
            r#"{"method":"item/autoApprovalReview/completed","params":{
                "action":{"type":"mcpToolCall","server":"srv","toolName":"search","connectorId":null,"connectorName":null,"toolTitle":null},
                "review":{"status":"approved","rationale":"read-only"}
            }}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(
                    content,
                    "[guardian] auto-review approved: MCP tool `search` — read-only"
                );
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        let out = s.parse_line(
            "r",
            r#"{"method":"item/autoApprovalReview/completed","params":{
                "action":{"type":"networkAccess","target":"example.com","host":"example.com","protocol":"https","port":443},
                "review":{"status":"timedOut"}
            }}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(
                    content,
                    "[guardian] auto-review timedOut: network access to example.com"
                );
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    #[test]
    fn skills_changed_to_notice() {
        let mut s = ready_server();
        // SkillsChangedNotification is an empty object — invalidation signal only.
        let out = s.parse_line("r", r#"{"method":"skills/changed","params":{}}"#);
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[notice] skills changed");
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    #[test]
    fn mcp_startup_status_to_codex_mcp_status() {
        let mut s = ready_server();
        // Real codex 0.137 shape: starting → then failed with an error string.
        let out = s.parse_line(
            "r",
            r#"{"method":"mcpServer/startupStatus/updated","params":{"name":"codex_apps","status":"starting","error":null}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CodexMcpStatus {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "codex_apps");
                assert_eq!(status, "starting");
                assert_eq!(error.as_deref(), None);
            }
            e => panic!("expected CodexMcpStatus, got {e:?}"),
        }

        let out = s.parse_line(
            "r",
            r#"{"method":"mcpServer/startupStatus/updated","params":{"name":"codex_apps","status":"failed","error":"handshake failed"}}"#,
        );
        match &out.events[0] {
            BusEvent::CodexMcpStatus { status, error, .. } => {
                assert_eq!(status, "failed");
                assert_eq!(error.as_deref(), Some("handshake failed"));
            }
            e => panic!("expected CodexMcpStatus, got {e:?}"),
        }

        // Missing name → dropped (no server to address).
        let out = s.parse_line(
            "r",
            r#"{"method":"mcpServer/startupStatus/updated","params":{"status":"ready"}}"#,
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
            r#"{"method":"error","params":{"error":{"message":"model overloaded"},"willRetry":false,"threadId":"th-123","turnId":"u"}}"#,
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
            r#"{"method":"error","params":{"error":{"message":"tls handshake eof"},"willRetry":true,"threadId":"th-123","turnId":"u"}}"#,
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

    // ── Batch E: guardian + model verification notices (NEW in 0.137) ────────────────────
    #[test]
    fn guardian_warning_and_model_verification_to_notice() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"guardianWarning","params":{"threadId":"th-123","message":"high-risk action detected"}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[guardian] high-risk action detected")
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        let out = s.parse_line(
            "r",
            r#"{"method":"model/verification","params":{"threadId":"th-123","turnId":"u","verifications":["trustedAccessForCyber"]}}"#,
        );
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(
                    content,
                    "[notice] model verification: trustedAccessForCyber"
                )
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
        // Empty verifications → no event.
        let out = s.parse_line(
            "r",
            r#"{"method":"model/verification","params":{"threadId":"th-123","turnId":"u","verifications":[]}}"#,
        );
        assert!(out.events.is_empty());
    }

    // ── G5: context compaction completes via a `contextCompaction` item/completed ────────
    #[test]
    fn context_compaction_item_to_notice() {
        let mut s = ready_server();
        // 0.137 surfaces compaction completion as an item (id only); the legacy
        // thread/compacted notification is deprecated and intentionally NOT handled.
        let out = s.parse_line(
            "r",
            r#"{"method":"item/completed","params":{"item":{"id":"item_1","type":"contextCompaction"}}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert_eq!(content, "[notice] context compacted");
            }
            e => panic!("expected CommandOutput, got {e:?}"),
        }
    }

    // ── G4: turn-level aggregated diff → CodexTurnDiff (latest supersedes) ────────────────
    #[test]
    fn turn_diff_updated_to_codex_turn_diff() {
        let mut s = ready_server();
        let out = s.parse_line(
            "r",
            r#"{"method":"turn/diff/updated","params":{"threadId":"th-123","turnId":"tu","diff":"--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new"}}"#,
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            BusEvent::CodexTurnDiff { turn_id, diff, .. } => {
                assert_eq!(turn_id, "tu");
                assert_eq!(diff, "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new");
            }
            e => panic!("expected CodexTurnDiff, got {e:?}"),
        }
    }

    #[test]
    fn turn_diff_missing_diff_is_dropped() {
        let mut s = ready_server();
        // No `diff` field → nothing to surface; emit no event (no empty-diff card).
        let out = s.parse_line(
            "r",
            r#"{"method":"turn/diff/updated","params":{"threadId":"t","turnId":"tu"}}"#,
        );
        assert!(
            out.events.is_empty(),
            "turn/diff/updated without diff must emit no event, got {:?}",
            out.events
        );
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
        let msgs = s.frame_user_turn("hi", &[], no_skills(), &no_overrides());
        assert_eq!(msgs[0]["params"]["threadId"], "th-ack");
    }

    #[test]
    fn root_notification_before_start_ack_is_replayed_after_ownership_is_known() {
        let mut server = CodexAppServer::new();
        let early = server.parse_line(
            "r",
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"root-1","delta":"hello"}}"#,
        );
        assert!(early.events.is_empty());

        let ack = server.parse_line(
            "r",
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"root-1"}}}"#,
        );
        assert_eq!(ack.thread_id.as_deref(), Some("root-1"));
        match &ack.events[0] {
            BusEvent::MessageDelta {
                text,
                parent_tool_use_id,
                ..
            } => {
                assert_eq!(text, "hello");
                assert!(parent_tool_use_id.is_none());
            }
            event => panic!("expected replayed root MessageDelta, got {event:?}"),
        }
    }

    #[test]
    fn interrupt_after_ready() {
        let mut s = ready_server();
        assert!(
            s.frame_interrupt().is_empty(),
            "ready thread with no active turn must not send an invalid interrupt"
        );
        s.parse_line(
            "r",
            r#"{"method":"turn/started","params":{"turn":{"id":"turn-123"}}}"#,
        );
        let msgs = s.frame_interrupt();
        assert_eq!(msgs[0]["method"], "turn/interrupt");
        assert_eq!(msgs[0]["params"]["threadId"], "th-123");
        assert_eq!(msgs[0]["params"]["turnId"], "turn-123");
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

    // ── Wave-4: ecosystem data-returning frames (experimentalFeature/list, model/list) ─────

    #[test]
    fn frame_experimental_feature_list_shape() {
        // No thread → drops (mirrors frame_skills_list / frame_goal_get).
        let mut s = CodexAppServer::new();
        assert!(s.frame_experimental_feature_list("r1").is_empty());

        let mut s = ready_server(); // thread th-123
        let frame = s.frame_experimental_feature_list("ocv-feat");
        assert_eq!(frame[0]["method"], "experimentalFeature/list");
        assert_eq!(frame[0]["params"]["threadId"], "th-123");
        // Tracked: a jsonrpc id is allocated so the reply correlates back to "ocv-feat".
        let id = frame[0]["id"].as_i64().unwrap();
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"data":[{{"name":"hooks","enabled":true}}]}}}}"#
        );
        let out = s.parse_line("run1", &line);
        let (rid, val) = out.control_response.expect("control_response");
        assert_eq!(rid, "ocv-feat");
        assert_eq!(val["data"][0]["name"], "hooks");
    }

    #[test]
    fn frame_model_list_shape() {
        // No thread → drops.
        let mut s = CodexAppServer::new();
        assert!(s.frame_model_list("r1").is_empty());

        let mut s = ready_server(); // thread th-123
        let frame = s.frame_model_list("ocv-models");
        assert_eq!(frame[0]["method"], "model/list");
        assert_eq!(frame[0]["params"]["threadId"], "th-123");
        let id = frame[0]["id"].as_i64().unwrap();
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"data":[{{"id":"gpt-5","isDefault":true}}]}}}}"#
        );
        let out = s.parse_line("run1", &line);
        let (rid, val) = out.control_response.expect("control_response");
        assert_eq!(rid, "ocv-models");
        assert_eq!(val["data"][0]["id"], "gpt-5");
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
                        for msg in driver.frame_user_turn(prompt, &[], no_skills(), &no_overrides()) {
                            let mut l = serde_json::to_string(&msg).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                        }
                        stdin.flush().await.unwrap();
                    }
                    for pi in &parsed.interactive {
                        if pi.kind == PendingKind::Permission {
                            saw_approval = true;
                            assert!(matches!(parsed.events[0], BusEvent::PermissionPrompt { .. }));
                            let response = driver
                                .frame_response(
                                    PendingKind::Permission,
                                    &pi.request_id,
                                    serde_json::json!({"behavior": "allow"}),
                                )
                                .unwrap();
                            let mut l = serde_json::to_string(&response.primary).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                            stdin.flush().await.unwrap();
                            assert!(driver.commit_response(&pi.request_id));
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
                        for msg in driver.frame_user_turn(prompt, &[], no_skills(), &no_overrides()) {
                            let mut l = serde_json::to_string(&msg).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                        }
                        stdin.flush().await.unwrap();
                    }

                    for pi in &parsed.interactive {
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
                            let response = driver
                                .frame_response(PendingKind::UserInput, &pi.request_id, answers)
                                .unwrap();
                            let mut l = serde_json::to_string(&response.primary).unwrap();
                            l.push('\n');
                            stdin.write_all(l.as_bytes()).await.unwrap();
                            stdin.flush().await.unwrap();
                            assert!(driver.commit_response(&pi.request_id));
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

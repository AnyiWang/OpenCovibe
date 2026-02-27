//! Claude CLI stream-json protocol parser.
//!
//! Pure-function protocol mapping: raw JSON events → `Vec<BusEvent>`.
//! Encapsulates all accumulator state (tool tracking, JSON accumulation)
//! so callers only need `ProtocolState::new()` + `state.map_event(...)`.

use crate::models::BusEvent;
use serde_json::Value;
use std::collections::HashMap;

/// Parsing statistics for Claude protocol — accumulated per-session, never reset.
/// Codex stats are NOT included here; Codex path only logs, no counters
/// (codex_parser lives in a separate stream.rs path, not ProtocolState).
#[derive(Debug, Clone, Default)]
pub struct ParserStats {
    /// Events with unknown type (→ Raw fallback)
    pub unknown_event_count: u32,
    /// Events with known type but malformed/missing fields (logged but emitted)
    pub parse_warn_count: u32,
    /// Events that failed output validation and were dropped
    pub invalid_tool_count: u32,
    /// Events explicitly dropped (empty type field)
    pub dropped_count: u32,
}

/// Validation outcome — None means valid, Some means invalid with reason.
pub struct ValidationWarn {
    pub event_type: &'static str,
    pub field: &'static str,
    pub detail: String,
}

/// Validate a BusEvent before emission. Returns Some(warn) if the event
/// should be dropped (invalid), None if it should be emitted (valid).
///
/// IMPORTANT: Only TOOL-class events can return Some (drop).
/// STATE-class events (RunState, SessionInit, UsageUpdate) NEVER return Some
/// because session_actor's quarantine/turn state machine depends on them.
pub fn validate_bus_event(ev: &BusEvent) -> Option<ValidationWarn> {
    match ev {
        // Tool-class: tool_use_id must be non-empty
        BusEvent::ToolStart { tool_use_id, .. }
        | BusEvent::ToolEnd { tool_use_id, .. }
        | BusEvent::ToolInputDelta { tool_use_id, .. } => {
            if tool_use_id.is_empty() {
                return Some(ValidationWarn {
                    event_type: "tool",
                    field: "tool_use_id",
                    detail: format!("empty tool_use_id in {:?}", std::mem::discriminant(ev)),
                });
            }
            None
        }
        BusEvent::ToolProgress { tool_use_id, .. }
        | BusEvent::ToolUseSummary { tool_use_id, .. } => {
            if tool_use_id.is_empty() {
                return Some(ValidationWarn {
                    event_type: "tool",
                    field: "tool_use_id",
                    detail: format!("empty tool_use_id in {:?}", std::mem::discriminant(ev)),
                });
            }
            None
        }
        // State-class: warn but NEVER drop
        BusEvent::RunState { state, .. } => {
            if state.is_empty() {
                log::warn!("[validate] RunState with empty state — passing through");
            }
            None // ALWAYS pass through
        }
        BusEvent::SessionInit { model, .. } => {
            if model.is_none() {
                log::debug!("[validate] SessionInit with no model — passing through");
            }
            None // ALWAYS pass through
        }
        // Everything else: pass through
        _ => None,
    }
}

/// Strict wrapper — panics if validate returns Some. Only exists in test binary.
#[cfg(test)]
pub fn validate_strict(ev: &BusEvent) {
    if let Some(warn) = validate_bus_event(ev) {
        panic!(
            "[STRICT] invalid event: {}.{}: {}",
            warn.event_type, warn.field, warn.detail
        );
    }
}

/// Accumulator state for a single Claude CLI session.
pub struct ProtocolState {
    /// Map tool_use_id → tool_name for reliable ToolEnd association
    emitted_tool_ids: HashMap<String, String>,
    /// Accumulate partial JSON input per tool_use_id
    input_json_accum: HashMap<String, String>,
    /// Track the most recently started tool_use_id (HashMap has no iteration order)
    last_tool_use_id: Option<String>,
    /// Whether a `result` event already emitted a terminal RunState
    pub got_result_event: bool,
    /// The `subtype` from the last `result` event (e.g. "error_max_turns", "error_input_too_long")
    pub result_subtype: Option<String>,
    /// Resume/continue/fork session — first system/init should emit RunState(idle)
    /// because the CLI is waiting for stdin input, not processing a prompt.
    is_resume: bool,
    /// Whether we've seen the first system/init. After the first one, subsequent
    /// system/init events (in multi-turn sessions) should NOT emit RunState at all —
    /// send_session_message already emits running, and result emits idle.
    seen_first_init: bool,
    /// Pending slash command (e.g. "/cost", "/context") — set by session_actor.
    /// If CLI doesn't emit `<local-command-stdout>` (cf6 bug), a friendly hint
    /// is emitted as CommandOutput on `result`.
    pending_slash_command: Option<String>,
    /// Parsing statistics — accumulated per-session, never reset.
    pub stats: ParserStats,
    /// When true, map_event panics on unknown/invalid events instead of degrading gracefully.
    /// Only available in test builds — production always degrades.
    #[cfg(test)]
    strict_mode: bool,
}

impl ProtocolState {
    pub fn is_resume(&self) -> bool {
        self.is_resume
    }

    /// Set a pending slash command for friendly-hint fallback.
    pub fn set_pending_slash_command(&mut self, cmd: Option<String>) {
        log::debug!("[protocol] set_pending_slash_command: {:?}", cmd);
        self.pending_slash_command = cmd;
    }

    pub fn new(is_resume: bool) -> Self {
        Self {
            emitted_tool_ids: HashMap::new(),
            input_json_accum: HashMap::new(),
            last_tool_use_id: None,
            got_result_event: false,
            result_subtype: None,
            is_resume,
            seen_first_init: false,
            pending_slash_command: None,
            stats: ParserStats::default(),
            #[cfg(test)]
            strict_mode: false,
        }
    }

    /// Create a strict-mode parser that panics on unknown/invalid events.
    /// Only available in test builds for strict fixture replay.
    #[cfg(test)]
    pub fn new_strict(is_resume: bool) -> Self {
        let mut s = Self::new(is_resume);
        s.strict_mode = true;
        s
    }

    /// Map a single raw Claude CLI JSON event into zero or more `BusEvent`s.
    pub fn map_event(&mut self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let mut events = Vec::new();
        let event_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Extract parent_tool_use_id for subagent tracking.
        // Non-null = event belongs to a subagent spawned by a Task tool with this ID.
        let parent_tool_use_id = raw
            .get("parent_tool_use_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        match event_type {
            // ── system init ──
            "system" => {
                let subtype = raw.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                if subtype == "init" {
                    let session_id = raw
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let model = raw
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let tools = raw
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| {
                                    v.get("name")
                                        .and_then(|n| n.as_str())
                                        .or_else(|| v.as_str())
                                        .map(|s| s.to_string())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let cwd = raw
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Parse CLI slash commands (raw JSON pass-through)
                    let slash_commands = raw
                        .get("slash_commands")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    // Parse MCP server info
                    let mcp_raw_count = raw
                        .get("mcp_servers")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    let mcp_servers: Vec<crate::models::McpServerInfo> = raw
                        .get("mcp_servers")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    let name = s.get("name").and_then(|v| v.as_str())?.to_string();
                                    let status = s
                                        .get("status")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("pending")
                                        .to_string();
                                    let server_type =
                                        s.get("type").and_then(|v| v.as_str()).map(String::from);
                                    let error =
                                        s.get("error").and_then(|v| v.as_str()).map(String::from);
                                    Some(crate::models::McpServerInfo {
                                        name,
                                        status,
                                        server_type,
                                        error,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mcp_dropped = mcp_raw_count.saturating_sub(mcp_servers.len());
                    if mcp_dropped > 0 {
                        log::debug!(
                            "[protocol] {} MCP server(s) dropped: missing name",
                            mcp_dropped
                        );
                        self.stats.parse_warn_count += mcp_dropped as u32;
                    }

                    // Extract verbose fields from system/init
                    let permission_mode = raw
                        .get("permissionMode")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let api_key_source = raw
                        .get("apiKeySource")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let claude_code_version = raw
                        .get("claude_code_version")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let output_style = raw
                        .get("output_style")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let agents: Vec<String> = raw
                        .get("agents")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let skills: Vec<String> = raw
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let plugins = raw
                        .get("plugins")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let fast_mode_state = raw
                        .get("fast_mode_state")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    log::debug!(
                        "[protocol] session_init: version={:?}, permission_mode={:?}, fast_mode={:?}, agents={}, skills={}",
                        claude_code_version,
                        permission_mode,
                        fast_mode_state,
                        agents.len(),
                        skills.len()
                    );

                    events.push(BusEvent::SessionInit {
                        run_id: run_id.to_string(),
                        session_id,
                        model,
                        tools,
                        cwd,
                        slash_commands,
                        mcp_servers,
                        permission_mode,
                        api_key_source,
                        claude_code_version,
                        output_style,
                        agents,
                        skills,
                        plugins,
                        fast_mode_state,
                    });
                    // Only emit RunState on the FIRST system/init:
                    // - New session: CLI is processing the initial prompt → "running"
                    // - Resume/continue: CLI loaded context, waiting for stdin → "idle"
                    // Subsequent system/init events (multi-turn) should NOT emit RunState:
                    // send_session_message already emits "running" and result emits "idle".
                    // Only emit RunState on the FIRST system/init, and only for NEW sessions.
                    // Resume/continue/fork: start_session already emits synthetic RunState(idle),
                    // so emitting another idle here would race with send_session_message's "running".
                    if !self.seen_first_init && !self.is_resume {
                        events.push(BusEvent::RunState {
                            run_id: run_id.to_string(),
                            state: "running".to_string(),
                            exit_code: None,
                            error: None,
                        });
                    }
                    self.seen_first_init = true;
                } else if subtype == "compact_boundary" {
                    let metadata = raw.get("compact_metadata");
                    let trigger = metadata
                        .and_then(|m| m.get("trigger"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("auto")
                        .to_string();
                    let pre_tokens = metadata
                        .and_then(|m| m.get("pre_tokens"))
                        .and_then(|v| v.as_u64());
                    log::debug!(
                        "[protocol] compact_boundary: trigger={}, pre_tokens={:?}",
                        trigger,
                        pre_tokens
                    );
                    events.push(BusEvent::CompactBoundary {
                        run_id: run_id.to_string(),
                        trigger,
                        pre_tokens,
                    });
                } else if subtype == "microcompact_boundary" {
                    log::debug!("[protocol] microcompact_boundary");
                    events.push(BusEvent::CompactBoundary {
                        run_id: run_id.to_string(),
                        trigger: "micro_auto".to_string(),
                        pre_tokens: None,
                    });
                } else if subtype == "status" {
                    let status = raw.get("status").and_then(|v| v.as_str()).map(String::from);
                    log::debug!("[protocol] system/status: {:?}", status);
                    events.push(BusEvent::SystemStatus {
                        run_id: run_id.to_string(),
                        status,
                        data: raw.clone(),
                    });
                } else if subtype == "hook_started" {
                    let hook_event = raw
                        .get("hook_event")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let hook_id = raw
                        .get("hook_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let hook_name = raw
                        .get("hook_name")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    log::debug!(
                        "[protocol] hook_started: event={}, id={}, name={:?}",
                        hook_event,
                        hook_id,
                        hook_name
                    );
                    events.push(BusEvent::HookStarted {
                        run_id: run_id.to_string(),
                        hook_event,
                        hook_id,
                        data: raw.clone(),
                        hook_name,
                    });
                } else if subtype == "hook_progress" {
                    let hook_id = raw
                        .get("hook_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    log::trace!("[protocol] hook_progress: id={}", hook_id);
                    events.push(BusEvent::HookProgress {
                        run_id: run_id.to_string(),
                        hook_id,
                        data: raw.clone(),
                    });
                } else if subtype == "hook_response" {
                    let hook_id = raw
                        .get("hook_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let hook_event = raw
                        .get("hook_event")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let outcome = raw
                        .get("outcome")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let hook_name = raw
                        .get("hook_name")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let hook_stdout = raw.get("stdout").and_then(|v| v.as_str()).map(String::from);
                    let hook_stderr = raw.get("stderr").and_then(|v| v.as_str()).map(String::from);
                    let hook_exit_code = raw
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                    log::debug!(
                        "[protocol] hook_response: id={}, event={}, outcome={}, name={:?}, exit_code={:?}",
                        hook_id,
                        hook_event,
                        outcome,
                        hook_name,
                        hook_exit_code
                    );
                    events.push(BusEvent::HookResponse {
                        run_id: run_id.to_string(),
                        hook_id,
                        hook_event,
                        outcome,
                        data: raw.clone(),
                        hook_name,
                        stdout: hook_stdout,
                        stderr: hook_stderr,
                        exit_code: hook_exit_code,
                    });
                } else if subtype == "task_notification" {
                    let task_id = raw
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let status = raw
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    log::debug!(
                        "[protocol] task_notification: task_id={}, status={}",
                        task_id,
                        status
                    );
                    events.push(BusEvent::TaskNotification {
                        run_id: run_id.to_string(),
                        task_id,
                        status,
                        data: raw.clone(),
                    });
                } else if subtype == "files_persisted" {
                    let files = raw.get("files").cloned().unwrap_or(Value::Array(vec![]));
                    log::debug!(
                        "[protocol] files_persisted: {} files",
                        files.as_array().map(|a| a.len()).unwrap_or(0)
                    );
                    events.push(BusEvent::FilesPersisted {
                        run_id: run_id.to_string(),
                        files,
                        data: raw.clone(),
                    });
                } else if subtype == "auth_status" {
                    let is_authenticating = raw
                        .get("isAuthenticating")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let output = raw
                        .get("output")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    log::debug!(
                        "[protocol] auth_status: authenticating={}",
                        is_authenticating
                    );
                    events.push(BusEvent::AuthStatus {
                        run_id: run_id.to_string(),
                        is_authenticating,
                        output,
                        data: raw.clone(),
                    });
                } else if !subtype.is_empty() {
                    // Unknown system subtype — wrap as Raw for forward compatibility
                    log::debug!("[protocol] unknown system subtype: {}", subtype);
                    self.stats.unknown_event_count += 1;
                    #[cfg(test)]
                    if self.strict_mode {
                        panic!("[STRICT] unknown system subtype: {}", subtype);
                    }
                    events.push(BusEvent::Raw {
                        run_id: run_id.to_string(),
                        source: format!("claude_system_{}", subtype),
                        data: raw.clone(),
                    });
                }
            }

            // ── streaming events (partial messages) ──
            "content_block_start" => {
                // From --include-partial-messages: content block starting
                if let Some(content_block) = raw.get("content_block") {
                    let block_type = content_block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    match block_type {
                        "tool_use" => {
                            let tool_use_id = content_block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let tool_name = content_block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !tool_use_id.is_empty() {
                                self.emitted_tool_ids
                                    .insert(tool_use_id.clone(), tool_name.clone());
                                self.input_json_accum
                                    .insert(tool_use_id.clone(), String::new());
                                self.last_tool_use_id = Some(tool_use_id.clone());
                                events.push(BusEvent::ToolStart {
                                    run_id: run_id.to_string(),
                                    tool_use_id,
                                    tool_name,
                                    input: Value::Null,
                                    parent_tool_use_id: parent_tool_use_id.clone(),
                                });
                            }
                        }
                        "thinking" => {
                            // Extended thinking block starting — initial thinking text (usually empty)
                            let text = content_block
                                .get("thinking")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !text.is_empty() {
                                events.push(BusEvent::ThinkingDelta {
                                    run_id: run_id.to_string(),
                                    text: text.to_string(),
                                    parent_tool_use_id: parent_tool_use_id.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }

            "content_block_delta" => {
                if let Some(delta) = raw.get("delta") {
                    let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    events.push(BusEvent::MessageDelta {
                                        run_id: run_id.to_string(),
                                        text: text.to_string(),
                                        parent_tool_use_id: parent_tool_use_id.clone(),
                                    });
                                }
                            }
                        }
                        "thinking_delta" | "thinking" => {
                            // Extended thinking: stream reasoning text
                            let text = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                events.push(BusEvent::ThinkingDelta {
                                    run_id: run_id.to_string(),
                                    text: text.to_string(),
                                    parent_tool_use_id: parent_tool_use_id.clone(),
                                });
                            }
                        }
                        "input_json_delta" => {
                            // Accumulate partial JSON for tool input
                            if let Some(partial) =
                                delta.get("partial_json").and_then(|v| v.as_str())
                            {
                                // Route to the most recently started tool_use_id
                                if let Some(ref id) = self.last_tool_use_id {
                                    if let Some(accum) = self.input_json_accum.get_mut(id.as_str())
                                    {
                                        accum.push_str(partial);
                                    }
                                    // Emit delta event for real-time UI preview
                                    if !partial.is_empty() {
                                        events.push(BusEvent::ToolInputDelta {
                                            run_id: run_id.to_string(),
                                            tool_use_id: id.clone(),
                                            partial_json: partial.to_string(),
                                            parent_tool_use_id: parent_tool_use_id.clone(),
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            "content_block_stop" => {
                // Content block finished — if it was a tool_use, we can now parse accumulated input
                // The full input will come with the `assistant` message, so no action needed here
            }

            "message_stop" => {
                // No-op: wait for the full `assistant` message
            }

            // ── complete assistant message ──
            "assistant" => {
                let message = raw.get("message").unwrap_or(raw);
                let message_id = message
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| raw.get("id").and_then(|v| v.as_str()).unwrap_or(""))
                    .to_string();

                // Extract per-message metadata from message object
                let msg_model = message
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let msg_stop_reason = message
                    .get("stop_reason")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let msg_usage = message.get("usage").cloned();

                if msg_model.is_some() {
                    log::debug!(
                        "[protocol] assistant message: model={:?}, stop_reason={:?}",
                        msg_model,
                        msg_stop_reason
                    );
                }

                if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                    let mut text_parts: Vec<String> = Vec::new();

                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                    text_parts.push(t.to_string());
                                }
                            }
                            "tool_use" => {
                                let tool_use_id = block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tool_name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                // Check if already emitted via streaming content_block_start
                                let already_emitted =
                                    self.emitted_tool_ids.contains_key(&tool_use_id);
                                // Always record id→name for ToolEnd lookup
                                self.emitted_tool_ids
                                    .entry(tool_use_id.clone())
                                    .or_insert_with(|| tool_name.clone());
                                // Only emit ToolStart if not already emitted from streaming
                                if !already_emitted {
                                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                                    events.push(BusEvent::ToolStart {
                                        run_id: run_id.to_string(),
                                        tool_use_id,
                                        tool_name,
                                        input,
                                        parent_tool_use_id: parent_tool_use_id.clone(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }

                    if !text_parts.is_empty() {
                        let full_text = text_parts.join("");
                        let mid = if message_id.is_empty() {
                            uuid::Uuid::new_v4().to_string()[..12].to_string()
                        } else {
                            message_id
                        };
                        events.push(BusEvent::MessageComplete {
                            run_id: run_id.to_string(),
                            message_id: mid,
                            text: full_text,
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            model: msg_model.clone(),
                            stop_reason: msg_stop_reason.clone(),
                            message_usage: msg_usage.clone(),
                        });
                    }
                }
            }

            // ── user message (tool_result / command output) ──
            "user" => {
                // Extract tool_use_result (top-level on raw event, structured metadata)
                let tool_use_result = raw.get("tool_use_result").cloned();
                if tool_use_result.is_some() {
                    log::debug!(
                        "[protocol] tool_use_result present on user event: {}",
                        tool_use_result
                            .as_ref()
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                    );
                }

                let message = raw.get("message").unwrap_or(raw);

                // Check for slash command output: content is a string wrapped in
                // <local-command-stdout>...</local-command-stdout> tags.
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    if text.starts_with("<local-command-stdout>") {
                        let inner = text.strip_prefix("<local-command-stdout>").unwrap_or(text);
                        let inner = inner
                            .strip_suffix("</local-command-stdout>")
                            .unwrap_or(inner)
                            .trim()
                            .to_string();
                        log::debug!("[protocol] command output detected ({} chars)", inner.len());
                        // Normal path succeeded — clear pending so hint won't fire
                        self.pending_slash_command = None;
                        events.push(BusEvent::CommandOutput {
                            run_id: run_id.to_string(),
                            content: inner,
                        });
                        return events;
                    }
                }

                if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if block_type == "tool_result" {
                            let tool_use_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Look up tool_name from id→name map
                            let tool_name = self
                                .emitted_tool_ids
                                .get(&tool_use_id)
                                .cloned()
                                .unwrap_or_default();
                            let output = block.get("content").cloned().unwrap_or(Value::Null);
                            let is_error = block
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let status = if is_error {
                                "error".to_string()
                            } else {
                                "success".to_string()
                            };

                            events.push(BusEvent::ToolEnd {
                                run_id: run_id.to_string(),
                                tool_use_id,
                                tool_name,
                                output,
                                status,
                                duration_ms: None,
                                parent_tool_use_id: parent_tool_use_id.clone(),
                                tool_use_result: tool_use_result.clone(),
                            });
                        }
                    }
                }
            }

            // ── result (turn complete) ──
            "result" => {
                let subtype = raw.get("subtype").and_then(|v| v.as_str()).unwrap_or("");

                // Extract usage
                if let Some(usage) = raw.get("usage") {
                    let input_tokens = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64());
                    let cache_write = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64());
                    let cost = raw
                        .get("cost_usd")
                        .and_then(|v| v.as_f64())
                        .or_else(|| raw.get("total_cost_usd").and_then(|v| v.as_f64()))
                        .unwrap_or(0.0);

                    // Parse per-model usage breakdown (camelCase keys from CLI)
                    let model_usage =
                        raw.get("modelUsage")
                            .and_then(|v| v.as_object())
                            .map(|obj| {
                                obj.iter()
                                    .map(|(model_name, entry)| {
                                        let mu = crate::models::ModelUsageEntry {
                                            input_tokens: entry
                                                .get("inputTokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            output_tokens: entry
                                                .get("outputTokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            cache_read_tokens: entry
                                                .get("cacheReadInputTokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            cache_write_tokens: entry
                                                .get("cacheCreationInputTokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            web_search_requests: entry
                                                .get("webSearchRequests")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            cost_usd: entry
                                                .get("costUSD")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(0.0),
                                            context_window: entry
                                                .get("contextWindow")
                                                .and_then(|v| v.as_u64()),
                                            max_output_tokens: entry
                                                .get("maxOutputTokens")
                                                .and_then(|v| v.as_u64()),
                                        };
                                        (model_name.clone(), mu)
                                    })
                                    .collect::<HashMap<_, _>>()
                            });

                    // Recalculate cost using our pricing table for accurate third-party model costs.
                    // CLI uses its own (often Claude-based) pricing, which is wrong for providers
                    // like DeepSeek, MiniMax, etc.
                    let (cost, model_usage) = if let Some(mut mu) = model_usage {
                        let mut total = 0.0_f64;
                        for (model_name, entry) in mu.iter_mut() {
                            let recalculated = crate::pricing::estimate_cost(
                                model_name,
                                entry.input_tokens,
                                entry.output_tokens,
                                entry.cache_read_tokens,
                                entry.cache_write_tokens,
                            );
                            entry.cost_usd = recalculated;
                            total += recalculated;
                        }
                        (total, Some(mu))
                    } else {
                        (cost, None)
                    };

                    let duration_api_ms = raw.get("duration_api_ms").and_then(|v| v.as_u64());

                    // Extract new result-level fields
                    let duration_ms = raw.get("duration_ms").and_then(|v| v.as_u64());
                    let num_turns = raw.get("num_turns").and_then(|v| v.as_u64());
                    let result_stop_reason = raw
                        .get("stop_reason")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    // Extract from usage sub-fields
                    let service_tier = usage
                        .get("service_tier")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let speed = usage
                        .get("speed")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let web_fetch_requests = usage
                        .get("server_tool_use")
                        .and_then(|v| v.get("web_fetch_requests"))
                        .and_then(|v| v.as_u64());
                    let cache_creation = usage.get("cache_creation");
                    let cache_creation_5m = cache_creation
                        .and_then(|c| c.get("ephemeral_5m_input_tokens"))
                        .and_then(|v| v.as_u64());
                    let cache_creation_1h = cache_creation
                        .and_then(|c| c.get("ephemeral_1h_input_tokens"))
                        .and_then(|v| v.as_u64());

                    if duration_ms.is_some() || num_turns.is_some() {
                        log::debug!(
                            "[protocol] result: duration_ms={:?}, num_turns={:?}, service_tier={:?}, speed={:?}",
                            duration_ms,
                            num_turns,
                            service_tier,
                            speed
                        );
                    }

                    events.push(BusEvent::UsageUpdate {
                        run_id: run_id.to_string(),
                        input_tokens,
                        output_tokens,
                        cache_read_tokens: cache_read,
                        cache_write_tokens: cache_write,
                        total_cost_usd: cost,
                        turn_index: None, // Injected by session_actor for user turns
                        model_usage,
                        duration_api_ms,
                        duration_ms,
                        num_turns,
                        stop_reason: result_stop_reason,
                        service_tier,
                        speed,
                        web_fetch_requests,
                        cache_creation_5m,
                        cache_creation_1h,
                    });

                    // Hint: if CLI didn't emit <local-command-stdout> for a pending
                    // slash command, show a friendly message instead of silent failure.
                    if let Some(cmd) = self.pending_slash_command.take() {
                        let hint = match cmd.as_str() {
                            "/cost" => "The /cost output is not available in the current CLI version. Cumulative cost is shown in the status bar.",
                            "/context" => "The /context output is not available in the current CLI version. Run /context in a terminal session instead.",
                            _ => "",
                        };
                        if !hint.is_empty() {
                            log::debug!("[protocol] slash command hint for {}", cmd);
                            events.push(BusEvent::CommandOutput {
                                run_id: run_id.to_string(),
                                content: hint.to_string(),
                            });
                        }
                    }
                }

                // Parse permission_denials from result event
                if let Some(denials) = raw.get("permission_denials").and_then(|v| v.as_array()) {
                    for denial in denials {
                        let tool_name = denial
                            .get("tool_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool_use_id = denial
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool_input = denial.get("tool_input").cloned().unwrap_or(Value::Null);
                        if !tool_name.is_empty() {
                            log::debug!(
                                "[protocol] permission_denied: tool={}, id={}",
                                tool_name,
                                tool_use_id
                            );
                            events.push(BusEvent::PermissionDenied {
                                run_id: run_id.to_string(),
                                tool_name,
                                tool_use_id,
                                tool_input,
                            });
                        }
                    }
                }

                if subtype.starts_with("error") {
                    // Read both `error` (singular string) and `errors` (plural array)
                    let error_msg = raw
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| {
                            raw.get("errors").and_then(|v| v.as_array()).map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            })
                        })
                        .unwrap_or_else(|| "Unknown error".to_string());
                    self.got_result_event = true;
                    self.result_subtype = Some(subtype.to_string());
                    log::debug!(
                        "[protocol] result error: subtype={}, msg={}",
                        subtype,
                        &error_msg[..error_msg.len().min(200)]
                    );
                    events.push(BusEvent::RunState {
                        run_id: run_id.to_string(),
                        state: "failed".to_string(),
                        exit_code: None,
                        error: Some(error_msg),
                    });
                } else {
                    // "idle" = turn complete, waiting for next user input.
                    // The actual "completed" state is emitted on process EOF (read_stdout cleanup).
                    events.push(BusEvent::RunState {
                        run_id: run_id.to_string(),
                        state: "idle".to_string(),
                        exit_code: None,
                        error: None,
                    });
                }
            }

            // ── tool progress (top-level event type) ──
            "tool_progress" => {
                let tool_use_id = raw
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let elapsed_time_seconds = raw.get("elapsed_time_seconds").and_then(|v| v.as_f64());
                log::trace!(
                    "[protocol] tool_progress: tool={}, elapsed={:?}s",
                    tool_use_id,
                    elapsed_time_seconds
                );
                events.push(BusEvent::ToolProgress {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    elapsed_time_seconds,
                    data: raw.clone(),
                    parent_tool_use_id: parent_tool_use_id.clone(),
                });
            }

            // ── tool use summary (top-level event type) ──
            "tool_use_summary" => {
                let tool_use_id = raw
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let summary = raw
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let preceding = raw
                    .get("preceding_tool_use_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                log::debug!(
                    "[protocol] tool_use_summary: tool={}, summary_len={}",
                    tool_use_id,
                    summary.len()
                );
                events.push(BusEvent::ToolUseSummary {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    summary,
                    preceding_tool_use_ids: preceding,
                    data: raw.clone(),
                    parent_tool_use_id: parent_tool_use_id.clone(),
                });
            }

            // ── fallback: raw ──
            _ => {
                if !event_type.is_empty() {
                    log::debug!("[protocol] unknown event type: {}", event_type);
                    self.stats.unknown_event_count += 1;
                    #[cfg(test)]
                    if self.strict_mode {
                        panic!("[STRICT] unknown event type: {}", event_type);
                    }
                    events.push(BusEvent::Raw {
                        run_id: run_id.to_string(),
                        source: format!("claude_{}", event_type),
                        data: raw.clone(),
                    });
                } else {
                    self.stats.dropped_count += 1;
                    #[cfg(test)]
                    if self.strict_mode {
                        panic!("[STRICT] empty event type (dropped)");
                    }
                }
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RUN: &str = "run-test";

    // ══════════════════════════════════════════════════════════════════
    //  Group A: Golden tests — one per event type, locks current behavior
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_system_init_new_session() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "init",
            "model": "opus-4",
            "tools": [{"name": "Bash"}, {"name": "Read"}],
            "cwd": "/project",
            "session_id": "ses-1"
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 2, "SessionInit + RunState(running)");
        match &events[0] {
            BusEvent::SessionInit {
                model,
                tools,
                cwd,
                session_id,
                ..
            } => {
                assert_eq!(model.as_deref(), Some("opus-4"));
                assert_eq!(tools, &vec!["Bash".to_string(), "Read".to_string()]);
                assert_eq!(cwd, "/project");
                assert_eq!(session_id.as_deref(), Some("ses-1"));
            }
            other => panic!("expected SessionInit, got {:?}", other),
        }
        match &events[1] {
            BusEvent::RunState { state, .. } => assert_eq!(state, "running"),
            other => panic!("expected RunState, got {:?}", other),
        }
    }

    #[test]
    fn test_system_init_resume_session() {
        let mut ps = ProtocolState::new(true); // resume
        let raw = json!({
            "type": "system",
            "subtype": "init",
            "model": "opus-4",
            "tools": [],
            "cwd": "/project",
            "session_id": "ses-2"
        });
        let events = ps.map_event(RUN, &raw);
        // Resume: SessionInit only, no RunState (start_session emits synthetic idle)
        assert_eq!(events.len(), 1, "resume: SessionInit only");
        assert!(matches!(&events[0], BusEvent::SessionInit { .. }));
    }

    #[test]
    fn test_system_init_second_call() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "init", "model": "opus-4", "tools": [], "cwd": "/"});
        let _first = ps.map_event(RUN, &raw);
        // Second call: SessionInit emitted but NO RunState (seen_first_init=true)
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], BusEvent::SessionInit { .. }));
    }

    #[test]
    fn test_system_compact_boundary() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "manual", "pre_tokens": 50000}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CompactBoundary {
                trigger,
                pre_tokens,
                ..
            } => {
                assert_eq!(trigger, "manual");
                assert_eq!(*pre_tokens, Some(50000));
            }
            other => panic!("expected CompactBoundary, got {:?}", other),
        }
    }

    #[test]
    fn test_system_microcompact_boundary() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "microcompact_boundary"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CompactBoundary {
                trigger,
                pre_tokens,
                ..
            } => {
                assert_eq!(trigger, "micro_auto");
                assert_eq!(*pre_tokens, None);
            }
            other => panic!("expected CompactBoundary, got {:?}", other),
        }
    }

    #[test]
    fn test_system_status() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "status", "status": "compacting"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::SystemStatus { status, .. } => {
                assert_eq!(status.as_deref(), Some("compacting"));
            }
            other => panic!("expected SystemStatus, got {:?}", other),
        }
    }

    #[test]
    fn test_system_hook_started() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "hook_started",
            "hook_event": "PreToolUse",
            "hook_id": "h1",
            "hook_name": "lint-check"
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::HookStarted {
                hook_event,
                hook_id,
                hook_name,
                ..
            } => {
                assert_eq!(hook_event, "PreToolUse");
                assert_eq!(hook_id, "h1");
                assert_eq!(hook_name.as_deref(), Some("lint-check"));
            }
            other => panic!("expected HookStarted, got {:?}", other),
        }
    }

    #[test]
    fn test_system_hook_progress() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "hook_progress", "hook_id": "h1"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::HookProgress { hook_id, .. } => assert_eq!(hook_id, "h1"),
            other => panic!("expected HookProgress, got {:?}", other),
        }
    }

    #[test]
    fn test_system_hook_response() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "hook_response",
            "hook_id": "h1",
            "hook_event": "PreToolUse",
            "outcome": "approved",
            "hook_name": "lint-check",
            "stdout": "ok",
            "stderr": "",
            "exit_code": 0
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::HookResponse {
                hook_id,
                hook_event,
                outcome,
                hook_name,
                stdout,
                stderr,
                exit_code,
                ..
            } => {
                assert_eq!(hook_id, "h1");
                assert_eq!(hook_event, "PreToolUse");
                assert_eq!(outcome, "approved");
                assert_eq!(hook_name.as_deref(), Some("lint-check"));
                assert_eq!(stdout.as_deref(), Some("ok"));
                assert_eq!(stderr.as_deref(), Some(""));
                assert_eq!(*exit_code, Some(0));
            }
            other => panic!("expected HookResponse, got {:?}", other),
        }
    }

    #[test]
    fn test_system_task_notification() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "task_notification", "task_id": "t1", "status": "started"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::TaskNotification {
                task_id, status, ..
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(status, "started");
            }
            other => panic!("expected TaskNotification, got {:?}", other),
        }
    }

    #[test]
    fn test_system_files_persisted() {
        let mut ps = ProtocolState::new(false);
        let raw =
            json!({"type": "system", "subtype": "files_persisted", "files": ["a.rs", "b.rs"]});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::FilesPersisted { files, .. } => {
                assert_eq!(files.as_array().unwrap().len(), 2);
            }
            other => panic!("expected FilesPersisted, got {:?}", other),
        }
    }

    #[test]
    fn test_system_auth_status() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "auth_status",
            "isAuthenticating": true,
            "output": ["Logging in...", "Success"]
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::AuthStatus {
                is_authenticating,
                output,
                ..
            } => {
                assert!(*is_authenticating);
                assert_eq!(output, &vec!["Logging in...", "Success"]);
            }
            other => panic!("expected AuthStatus, got {:?}", other),
        }
    }

    #[test]
    fn test_content_block_start_tool() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "id": "tu-1", "name": "Bash"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart {
                tool_use_id,
                tool_name,
                input,
                parent_tool_use_id,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(tool_name, "Bash");
                assert_eq!(*input, Value::Null);
                assert!(parent_tool_use_id.is_none());
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
        // Verify accumulator state
        assert!(ps.emitted_tool_ids.contains_key("tu-1"));
    }

    #[test]
    fn test_content_block_start_thinking() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_start",
            "content_block": {"type": "thinking", "thinking": "hmm let me think"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ThinkingDelta { text, .. } => assert_eq!(text, "hmm let me think"),
            other => panic!("expected ThinkingDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_content_block_start_thinking_empty() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_start",
            "content_block": {"type": "thinking", "thinking": ""}
        });
        let events = ps.map_event(RUN, &raw);
        assert!(events.is_empty(), "empty thinking text: no event emitted");
    }

    #[test]
    fn test_content_block_delta_text() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_delta",
            "delta": {"type": "text_delta", "text": "hello world"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::MessageDelta { text, .. } => assert_eq!(text, "hello world"),
            other => panic!("expected MessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_content_block_delta_thinking() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_delta",
            "delta": {"type": "thinking_delta", "thinking": "reasoning step"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ThinkingDelta { text, .. } => assert_eq!(text, "reasoning step"),
            other => panic!("expected ThinkingDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_content_block_delta_input_json() {
        let mut ps = ProtocolState::new(false);
        // First: start a tool so last_tool_use_id is set
        let start = json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "id": "tu-1", "name": "Bash"}
        });
        ps.map_event(RUN, &start);
        // Then: input_json_delta
        let raw = json!({
            "type": "content_block_delta",
            "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolInputDelta {
                tool_use_id,
                partial_json,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(partial_json, "{\"cmd\":");
            }
            other => panic!("expected ToolInputDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_assistant_message_text_only() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "assistant",
            "message": {
                "id": "m1",
                "model": "opus-4",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Hello there!"}]
            }
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::MessageComplete {
                message_id,
                text,
                model,
                stop_reason,
                ..
            } => {
                assert_eq!(message_id, "m1");
                assert_eq!(text, "Hello there!");
                assert_eq!(model.as_deref(), Some("opus-4"));
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
            }
            other => panic!("expected MessageComplete, got {:?}", other),
        }
    }

    #[test]
    fn test_assistant_message_with_tool() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "assistant",
            "message": {
                "id": "m1",
                "content": [
                    {"type": "text", "text": "Let me read that."},
                    {"type": "tool_use", "id": "tu-1", "name": "Read", "input": {"path": "/x"}}
                ]
            }
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(
            events.len(),
            2,
            "ToolStart (during loop) + MessageComplete (after loop)"
        );
        // Note: ToolStart is emitted during content iteration, MessageComplete after
        match &events[0] {
            BusEvent::ToolStart {
                tool_use_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(tool_name, "Read");
                assert_eq!(input.get("path").unwrap().as_str().unwrap(), "/x");
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
        match &events[1] {
            BusEvent::MessageComplete {
                message_id, text, ..
            } => {
                assert_eq!(message_id, "m1");
                assert_eq!(text, "Let me read that.");
            }
            other => panic!("expected MessageComplete, got {:?}", other),
        }
    }

    #[test]
    fn test_assistant_tool_dedup() {
        let mut ps = ProtocolState::new(false);
        // Step 1: tool already emitted via streaming content_block_start
        let start = json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "id": "tu-1", "name": "Bash"}
        });
        ps.map_event(RUN, &start);
        // Step 2: assistant message arrives with same tool
        let raw = json!({
            "type": "assistant",
            "message": {
                "id": "m1",
                "content": [
                    {"type": "text", "text": "Running."},
                    {"type": "tool_use", "id": "tu-1", "name": "Bash", "input": {"cmd": "ls"}}
                ]
            }
        });
        let events = ps.map_event(RUN, &raw);
        // Only MessageComplete emitted, ToolStart is deduped
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], BusEvent::MessageComplete { .. }));
    }

    #[test]
    fn test_user_tool_result() {
        let mut ps = ProtocolState::new(false);
        // Pre-register tool id→name
        ps.emitted_tool_ids
            .insert("tu-1".to_string(), "Bash".to_string());
        let raw = json!({
            "type": "user",
            "message": {
                "content": [
                    {"type": "tool_result", "tool_use_id": "tu-1", "content": "files listed ok"}
                ]
            }
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolEnd {
                tool_use_id,
                tool_name,
                status,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(tool_name, "Bash");
                assert_eq!(status, "success");
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_user_tool_result_error() {
        let mut ps = ProtocolState::new(false);
        ps.emitted_tool_ids
            .insert("tu-1".to_string(), "Bash".to_string());
        let raw = json!({
            "type": "user",
            "message": {
                "content": [
                    {"type": "tool_result", "tool_use_id": "tu-1", "content": "command failed", "is_error": true}
                ]
            }
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolEnd { status, .. } => assert_eq!(status, "error"),
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_user_command_output() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "user",
            "message": {
                "content": "<local-command-stdout>cost info here</local-command-stdout>"
            }
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CommandOutput { content, .. } => assert_eq!(content, "cost info here"),
            other => panic!("expected CommandOutput, got {:?}", other),
        }
    }

    #[test]
    fn test_result_success() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "result",
            "subtype": "success",
            "usage": {"input_tokens": 100, "output_tokens": 50},
            "cost_usd": 0.01
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 2, "UsageUpdate + RunState(idle)");
        match &events[0] {
            BusEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                total_cost_usd,
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert!(*total_cost_usd > 0.0);
            }
            other => panic!("expected UsageUpdate, got {:?}", other),
        }
        match &events[1] {
            BusEvent::RunState { state, error, .. } => {
                assert_eq!(state, "idle");
                assert!(error.is_none());
            }
            other => panic!("expected RunState, got {:?}", other),
        }
        assert!(
            !ps.got_result_event,
            "success doesn't set got_result_event (only error does)"
        );
    }

    #[test]
    fn test_result_error() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "result",
            "subtype": "error_max_turns",
            "error": "Max turns reached",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let events = ps.map_event(RUN, &raw);
        // UsageUpdate + RunState(failed)
        assert!(events.len() >= 2);
        let run_state = events
            .iter()
            .find(|e| matches!(e, BusEvent::RunState { .. }))
            .unwrap();
        match run_state {
            BusEvent::RunState { state, error, .. } => {
                assert_eq!(state, "failed");
                assert_eq!(error.as_deref(), Some("Max turns reached"));
            }
            _ => unreachable!(),
        }
        assert!(ps.got_result_event);
        assert_eq!(ps.result_subtype.as_deref(), Some("error_max_turns"));
    }

    #[test]
    fn test_result_with_model_usage() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "result",
            "subtype": "success",
            "usage": {"input_tokens": 100, "output_tokens": 50},
            "modelUsage": {
                "opus-4": {
                    "inputTokens": 80,
                    "outputTokens": 40,
                    "cacheReadInputTokens": 0,
                    "cacheCreationInputTokens": 0,
                    "webSearchRequests": 0,
                    "costUSD": 0.005
                }
            }
        });
        let events = ps.map_event(RUN, &raw);
        let usage = events
            .iter()
            .find(|e| matches!(e, BusEvent::UsageUpdate { .. }))
            .unwrap();
        match usage {
            BusEvent::UsageUpdate { model_usage, .. } => {
                let mu = model_usage.as_ref().expect("should have model_usage");
                assert!(mu.contains_key("opus-4"));
                let entry = &mu["opus-4"];
                assert_eq!(entry.input_tokens, 80);
                assert_eq!(entry.output_tokens, 40);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_result_permission_denials() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "result",
            "subtype": "success",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "permission_denials": [
                {"tool_name": "Bash", "tool_use_id": "tu-1", "tool_input": {"cmd": "rm -rf /"}}
            ]
        });
        let events = ps.map_event(RUN, &raw);
        let denials: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, BusEvent::PermissionDenied { .. }))
            .collect();
        assert_eq!(denials.len(), 1);
        match &denials[0] {
            BusEvent::PermissionDenied {
                tool_name,
                tool_use_id,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_use_id, "tu-1");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_tool_progress() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "tool_progress",
            "tool_use_id": "tu-1",
            "elapsed_time_seconds": 3.5
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolProgress {
                tool_use_id,
                elapsed_time_seconds,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(*elapsed_time_seconds, Some(3.5));
            }
            other => panic!("expected ToolProgress, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_use_summary() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "tool_use_summary",
            "tool_use_id": "tu-1",
            "summary": "Ran tests successfully",
            "preceding_tool_use_ids": ["tu-0"]
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolUseSummary {
                tool_use_id,
                summary,
                preceding_tool_use_ids,
                ..
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(summary, "Ran tests successfully");
                assert_eq!(preceding_tool_use_ids, &vec!["tu-0".to_string()]);
            }
            other => panic!("expected ToolUseSummary, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_type_raw_fallback() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "new_feature_xyz", "data": {"hello": "world"}});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, data, .. } => {
                assert_eq!(source, "claude_new_feature_xyz");
                assert_eq!(data["type"].as_str().unwrap(), "new_feature_xyz");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
        assert_eq!(
            ps.stats.unknown_event_count, 1,
            "unknown type should increment counter"
        );
    }

    #[test]
    fn test_empty_type_no_output() {
        let mut ps = ProtocolState::new(false);
        // type="" → fallback, but empty string is filtered → dropped_count
        let raw = json!({"type": "", "data": {}});
        let events = ps.map_event(RUN, &raw);
        assert!(events.is_empty(), "empty type should produce no events");
        assert_eq!(
            ps.stats.dropped_count, 1,
            "empty type increments dropped_count"
        );
    }

    #[test]
    fn test_no_type_field_no_output() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"data": {"stuff": true}});
        let events = ps.map_event(RUN, &raw);
        assert!(
            events.is_empty(),
            "missing type field should produce no events"
        );
    }

    #[test]
    fn test_content_block_stop_no_op() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "content_block_stop", "index": 0});
        let events = ps.map_event(RUN, &raw);
        assert!(events.is_empty(), "content_block_stop is a no-op");
    }

    #[test]
    fn test_message_stop_no_op() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "message_stop"});
        let events = ps.map_event(RUN, &raw);
        assert!(events.is_empty(), "message_stop is a no-op");
    }

    #[test]
    fn test_parent_tool_use_id_propagation() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_start",
            "parent_tool_use_id": "parent-task-1",
            "content_block": {"type": "tool_use", "id": "tu-sub", "name": "Bash"}
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart {
                parent_tool_use_id, ..
            } => {
                assert_eq!(parent_tool_use_id.as_deref(), Some("parent-task-1"));
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    // ══════════════════════════════════════════════════════════════════
    //  Group B: Malformed field tests — lock current behavior for known defects
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_content_block_start_missing_id() {
        // content_block_start path has `if !tool_use_id.is_empty()` guard → safe
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "name": "Bash"}
        });
        let events = ps.map_event(RUN, &raw);
        assert!(
            events.is_empty(),
            "content_block_start with missing id: guarded, no output"
        );
    }

    #[test]
    fn test_content_block_start_no_content_block() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "content_block_start"});
        let events = ps.map_event(RUN, &raw);
        assert!(events.is_empty(), "no content_block field: no output");
    }

    #[test]
    fn test_assistant_tool_missing_id() {
        // assistant path has NO guard on tool_use_id — empty id leaks out
        // This test locks the CURRENT (defective) behavior
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "assistant",
            "message": {
                "id": "m1",
                "content": [{"type": "tool_use", "name": "Bash", "input": {}}]
            }
        });
        let events = ps.map_event(RUN, &raw);
        // ⚠️ Current behavior: ToolStart with empty tool_use_id leaks out (L584 has no guard)
        let tool_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, BusEvent::ToolStart { .. }))
            .collect();
        assert_eq!(
            tool_starts.len(),
            1,
            "empty id tool_use leaks through assistant path"
        );
        match &tool_starts[0] {
            BusEvent::ToolStart { tool_use_id, .. } => {
                assert_eq!(
                    tool_use_id, "",
                    "tool_use_id is empty string (known defect)"
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_assistant_no_content() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "assistant", "message": {}});
        let events = ps.map_event(RUN, &raw);
        assert!(
            events.is_empty(),
            "assistant with no content array: no output"
        );
    }

    #[test]
    fn test_result_no_usage() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "result", "subtype": "success"});
        let events = ps.map_event(RUN, &raw);
        // No usage → no UsageUpdate, just RunState(idle)
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::RunState { state, .. } => assert_eq!(state, "idle"),
            other => panic!("expected RunState, got {:?}", other),
        }
    }

    #[test]
    fn test_system_init_no_model() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "init", "tools": [], "cwd": "/"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 2);
        match &events[0] {
            BusEvent::SessionInit { model, .. } => {
                assert!(model.is_none(), "model is None when missing")
            }
            other => panic!("expected SessionInit, got {:?}", other),
        }
    }

    #[test]
    fn test_mcp_server_missing_name() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({
            "type": "system",
            "subtype": "init",
            "tools": [],
            "cwd": "/",
            "mcp_servers": [{"status": "running"}]
        });
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 2);
        match &events[0] {
            BusEvent::SessionInit { mcp_servers, .. } => {
                assert!(
                    mcp_servers.is_empty(),
                    "MCP server without name is dropped by filter_map"
                );
            }
            other => panic!("expected SessionInit, got {:?}", other),
        }
        assert_eq!(
            ps.stats.parse_warn_count, 1,
            "dropped MCP server counted as parse_warn"
        );
    }

    #[test]
    fn test_system_unknown_subtype() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "brand_new_feature"});
        let events = ps.map_event(RUN, &raw);
        // Phase 2: unknown system subtype now emits Raw (forward compatibility)
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, .. } => {
                assert_eq!(source, "claude_system_brand_new_feature");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
        assert_eq!(ps.stats.unknown_event_count, 1);
    }

    #[test]
    fn test_tool_progress_no_tool_id() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "tool_progress"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolProgress {
                tool_use_id,
                elapsed_time_seconds,
                ..
            } => {
                assert_eq!(tool_use_id, "", "⚠️ empty tool_use_id leaks through");
                assert_eq!(*elapsed_time_seconds, None);
            }
            other => panic!("expected ToolProgress, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_use_summary_no_tool_id() {
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "tool_use_summary"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolUseSummary {
                tool_use_id,
                summary,
                ..
            } => {
                assert_eq!(tool_use_id, "", "⚠️ empty tool_use_id leaks through");
                assert_eq!(summary, "");
            }
            other => panic!("expected ToolUseSummary, got {:?}", other),
        }
    }

    // ══════════════════════════════════════════════════════════════════
    //  Phase 2 regression tests — guard validate_bus_event invariants
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_validate_never_drops_run_state() {
        // RunState must ALWAYS pass through validate, even with empty state.
        // Quarantine/turn state machine depends on these events.
        let ev = BusEvent::RunState {
            run_id: "r1".to_string(),
            state: "".to_string(),
            exit_code: None,
            error: None,
        };
        assert!(
            validate_bus_event(&ev).is_none(),
            "validate must NEVER drop RunState (even with empty state)"
        );

        // Normal states too
        for state in &["idle", "running", "failed", "completed"] {
            let ev = BusEvent::RunState {
                run_id: "r1".to_string(),
                state: state.to_string(),
                exit_code: None,
                error: None,
            };
            assert!(
                validate_bus_event(&ev).is_none(),
                "RunState({}) must pass",
                state
            );
        }
    }

    #[test]
    fn test_validate_drops_empty_tool_id() {
        // ToolStart with empty id should be flagged as invalid
        let ev = BusEvent::ToolStart {
            run_id: "r1".to_string(),
            tool_use_id: "".to_string(),
            tool_name: "Bash".to_string(),
            input: Value::Null,
            parent_tool_use_id: None,
        };
        let warn = validate_bus_event(&ev);
        assert!(
            warn.is_some(),
            "empty tool_use_id in ToolStart should be invalid"
        );
        assert_eq!(warn.unwrap().field, "tool_use_id");

        // ToolEnd with empty id
        let ev = BusEvent::ToolEnd {
            run_id: "r1".to_string(),
            tool_use_id: "".to_string(),
            tool_name: "Bash".to_string(),
            output: Value::Null,
            status: "success".to_string(),
            duration_ms: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        };
        assert!(
            validate_bus_event(&ev).is_some(),
            "empty tool_use_id in ToolEnd should be invalid"
        );

        // Valid tool passes
        let ev = BusEvent::ToolStart {
            run_id: "r1".to_string(),
            tool_use_id: "tu-1".to_string(),
            tool_name: "Bash".to_string(),
            input: Value::Null,
            parent_tool_use_id: None,
        };
        assert!(
            validate_bus_event(&ev).is_none(),
            "valid ToolStart should pass"
        );
    }

    #[test]
    fn test_unknown_system_subtype_emits_raw() {
        // Phase 2: system unknown subtype now emits Raw + increments unknown_event_count
        let mut ps = ProtocolState::new(false);
        let raw = json!({"type": "system", "subtype": "brand_new"});
        let events = ps.map_event(RUN, &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, .. } => {
                assert_eq!(source, "claude_system_brand_new");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
        assert_eq!(
            ps.stats.unknown_event_count, 1,
            "unknown system subtype increments counter"
        );
    }

    // ══════════════════════════════════════════════════════════════════
    //  Group D: Strict mode — standard parse sequence through new_strict()
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_strict_standard_session() {
        // A complete session_init → tool → result sequence through strict parser.
        // validate_strict panics if any emitted event has invalid fields.
        let mut ps = ProtocolState::new_strict(false);

        // 1. system/init
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "system", "subtype": "init",
                "model": "opus-4", "tools": [{"name": "Bash"}], "cwd": "/test"
            }),
        );
        assert!(!events.is_empty());
        for ev in &events {
            validate_strict(ev);
        }

        // 2. content_block_start (tool_use)
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "content_block_start",
                "content_block": { "type": "tool_use", "id": "tu-1", "name": "Read" }
            }),
        );
        for ev in &events {
            validate_strict(ev);
        }

        // 3. content_block_delta (input_json)
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "content_block_delta",
                "delta": { "type": "input_json_delta", "partial_json": "{\"path\":\"/a\"}" }
            }),
        );
        for ev in &events {
            validate_strict(ev);
        }

        // 4. assistant (message_complete + tool dedup)
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "assistant",
                "message": {
                    "id": "msg-1",
                    "content": [
                        { "type": "text", "text": "Let me read." },
                        { "type": "tool_use", "id": "tu-1", "name": "Read",
                          "input": { "path": "/a" } }
                    ]
                }
            }),
        );
        for ev in &events {
            validate_strict(ev);
        }

        // 5. user (tool_result)
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "user",
                "message": {
                    "content": [
                        { "type": "tool_result", "tool_use_id": "tu-1",
                          "content": "file contents" }
                    ]
                }
            }),
        );
        for ev in &events {
            validate_strict(ev);
        }

        // 6. result (success)
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "result", "subtype": "success",
                "cost_usd": 0.01, "duration_ms": 1000,
                "usage": { "input_tokens": 100, "output_tokens": 20 }
            }),
        );
        for ev in &events {
            validate_strict(ev);
        }

        // Post-conditions: strict session has 0 unknown/invalid/dropped
        assert_eq!(ps.stats.unknown_event_count, 0);
        assert_eq!(ps.stats.invalid_tool_count, 0);
        assert_eq!(ps.stats.dropped_count, 0);
    }

    #[test]
    fn test_strict_resume_session() {
        // Resume session — init emits SessionInit only (session_actor emits synthetic idle)
        let mut ps = ProtocolState::new_strict(true);
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "system", "subtype": "init",
                "model": "opus-4", "tools": [], "cwd": "/",
                "session_id": "sess-resume"
            }),
        );
        assert_eq!(events.len(), 1, "resume: SessionInit only");
        for ev in &events {
            validate_strict(ev);
        }
        assert!(matches!(&events[0], BusEvent::SessionInit { .. }));
        assert_eq!(ps.stats.unknown_event_count, 0);
    }

    #[test]
    fn test_strict_tool_progress_and_summary() {
        // tool_progress and tool_use_summary with valid ids pass strict
        let mut ps = ProtocolState::new_strict(false);
        let events = ps.map_event(
            RUN,
            &json!({
                "type": "tool_progress",
                "tool_use_id": "tu-1",
                "elapsed_time_seconds": 1.5
            }),
        );
        assert_eq!(events.len(), 1);
        for ev in &events {
            validate_strict(ev);
        }

        let events = ps.map_event(
            RUN,
            &json!({
                "type": "tool_use_summary",
                "tool_use_id": "tu-1",
                "summary": "Done"
            }),
        );
        assert_eq!(events.len(), 1);
        for ev in &events {
            validate_strict(ev);
        }
        assert_eq!(ps.stats.invalid_tool_count, 0);
    }
}

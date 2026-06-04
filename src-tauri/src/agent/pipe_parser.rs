use crate::agent::codex_parser::{codex_normalize_status, CodexToolKind};
use crate::models::BusEvent;
use serde_json::Value;

/// Trait for parsing structured stdout in pipe-exec mode.
/// NOT a general protocol parser — session_actor has its own protocol handling.
/// Implementations parse agent-specific NDJSON into normalized BusEvents.
pub trait PipeStdoutParser: Send {
    /// Parse one NDJSON line into zero or more BusEvents.
    fn parse_line(&mut self, run_id: &str, raw: &Value) -> Vec<BusEvent>;
}

/// Codex NDJSON parser — stateful, maps all 8 event types to BusEvents.
///
/// Events: thread.started, turn.started, turn.completed, turn.failed,
///         item.started, item.updated, item.completed, error
///
/// Item types: agent_message, reasoning, command_execution, file_change,
///             mcp_tool_call, collab_tool_call, web_search, todo_list, error
pub struct CodexStdoutParser {
    /// Invocation sequence — scopes IDs across resume processes within the same run.
    process_seq: u32,
    /// Turn counter — incremented on each turn.started within this process.
    turn_counter: u32,
}

impl CodexStdoutParser {
    pub fn new(process_seq: u32) -> Self {
        Self {
            process_seq,
            turn_counter: 0,
        }
    }

    /// Generate a scoped ID: `codex-{process_seq}-{turn}-{item_id}`
    fn scoped_id(&self, item_id: &str) -> String {
        format!(
            "codex-{}-{}-{}",
            self.process_seq, self.turn_counter, item_id
        )
    }

    fn map_item_started(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let item = match raw.get("item") {
            Some(i) => i,
            None => return vec![],
        };
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let tool_use_id = self.scoped_id(item_id);

        match CodexToolKind::from_item_type(item_type) {
            Some(CodexToolKind::Command) => {
                let command = item
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Bash".to_string(),
                    input: serde_json::json!({ "command": command }),
                    parent_tool_use_id: None,
                }]
            }
            Some(CodexToolKind::FileChange) => {
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Edit".to_string(),
                    input: file_change_input(item),
                    parent_tool_use_id: None,
                }]
            }
            Some(CodexToolKind::McpToolCall) => {
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: mcp_tool_name(item),
                    input: item
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    parent_tool_use_id: None,
                }]
            }
            Some(CodexToolKind::WebSearch) => {
                let query = item
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut input = serde_json::json!({ "query": query });
                // Include action (tagged enum: search/open_page/find_in_page) if present
                if let Some(action) = item.get("action") {
                    input["action"] = action.clone();
                }
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "WebSearch".to_string(),
                    input,
                    parent_tool_use_id: None,
                }]
            }
            Some(CodexToolKind::CollabToolCall) => {
                let tool = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let prompt = item
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Agent".to_string(),
                    input: serde_json::json!({ "tool": tool, "prompt": prompt }),
                    parent_tool_use_id: None,
                }]
            }
            // agent_message, reasoning: wait for completed
            None => vec![],
        }
    }

    fn map_item_completed(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let item = match raw.get("item") {
            Some(i) => i,
            None => return vec![],
        };
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let tool_use_id = self.scoped_id(item_id);
        let raw_status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed");
        let status = codex_normalize_status(raw_status).to_string();

        match item_type {
            "agent_message" => {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                vec![BusEvent::MessageComplete {
                    run_id: run_id.to_string(),
                    message_id: tool_use_id,
                    text,
                    parent_tool_use_id: None,
                    model: None,
                    stop_reason: None,
                    message_usage: None,
                }]
            }
            "command_execution" => {
                // Codex uses `aggregated_output`, not `output` (see exec_events.rs:158)
                let output = item
                    .get("aggregated_output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Bash".to_string(),
                    output: serde_json::json!({ "content": output }),
                    status,
                    duration_ms: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }]
            }
            "file_change" => {
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Edit".to_string(),
                    // Keep the raw changes list as output; the ToolStart carries file_path
                    // (reducer joins start↔end by tool_use_id for the label).
                    output: item
                        .get("changes")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    status,
                    duration_ms: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }]
            }
            "mcp_tool_call" => {
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: mcp_tool_name(item),
                    output: item.get("result").cloned().unwrap_or(serde_json::json!({})),
                    status,
                    duration_ms: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }]
            }
            "web_search" => {
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "WebSearch".to_string(),
                    output: item.get("action").cloned().unwrap_or(serde_json::json!({})),
                    status,
                    duration_ms: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }]
            }
            "collab_tool_call" => {
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Agent".to_string(),
                    output: item
                        .get("agents_states")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    status,
                    duration_ms: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }]
            }
            "todo_list" => self.map_todo_list(run_id, item),
            "error" => {
                let msg = item
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                vec![BusEvent::CommandOutput {
                    run_id: run_id.to_string(),
                    content: format!("[error] {}", msg),
                }]
            }
            "reasoning" => {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return vec![];
                }
                vec![BusEvent::ThinkingDelta {
                    run_id: run_id.to_string(),
                    text,
                    parent_tool_use_id: None,
                }]
            }
            _ => vec![],
        }
    }

    fn map_turn_completed(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let usage = match raw.get("usage") {
            Some(u) => u,
            None => return vec![],
        };
        let input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("cached_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        // Codex v0.130+ reports reasoning tokens separately. Merge into output_tokens so
        // reasoning-model sessions don't undercount usage (mirrors Claude's thinking-token
        // handling, which also folds them into output_tokens).
        let reasoning_out = usage
            .get("reasoning_output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_total = output.saturating_add(reasoning_out);
        vec![BusEvent::UsageUpdate {
            run_id: run_id.to_string(),
            input_tokens: input,
            output_tokens: output_total,
            cache_read_tokens: if cached > 0 { Some(cached) } else { None },
            cache_write_tokens: None,
            total_cost_usd: 0.0, // Codex doesn't provide cost
            turn_index: Some(self.turn_counter),
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
        }]
    }

    fn map_turn_failed(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let error_msg = raw
            .get("error")
            .and_then(|e| e.get("message").and_then(|v| v.as_str()))
            .unwrap_or("turn failed");
        vec![BusEvent::RunState {
            run_id: run_id.to_string(),
            state: "failed".to_string(),
            exit_code: None,
            error: Some(human_error_message(error_msg)),
        }]
    }

    /// Map a Codex `todo_list` item to a TodoWrite ToolStart+ToolEnd pair so it reuses
    /// the existing TodoWrite renderer (tool_use_result.newTodos) and persists/replays
    /// (Raw events don't). Stable scoped id → repeated item.updated refreshes one card.
    fn map_todo_list(&self, run_id: &str, item: &Value) -> Vec<BusEvent> {
        let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("todo");
        let tool_use_id = self.scoped_id(item_id);

        let new_todos: Vec<Value> = item
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(todo_item_to_new_todo).collect())
            .unwrap_or_default();
        let new_todos = Value::Array(new_todos);

        vec![
            BusEvent::ToolStart {
                run_id: run_id.to_string(),
                tool_use_id: tool_use_id.clone(),
                tool_name: "TodoWrite".to_string(),
                input: serde_json::json!({ "todos": new_todos }),
                parent_tool_use_id: None,
            },
            BusEvent::ToolEnd {
                run_id: run_id.to_string(),
                tool_use_id,
                tool_name: "TodoWrite".to_string(),
                output: serde_json::json!({}),
                status: "success".to_string(),
                duration_ms: None,
                parent_tool_use_id: None,
                tool_use_result: Some(serde_json::json!({ "newTodos": new_todos })),
            },
        ]
    }

    fn map_error(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let msg = raw
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| raw.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        vec![BusEvent::CommandOutput {
            run_id: run_id.to_string(),
            content: format!("[codex error] {}", human_error_message(msg)),
        }]
    }
}

/// Codex sometimes nests a stringified-JSON error blob inside the `message` field
/// (e.g. `message = "{\"error\":{\"message\":\"...\"}}"`). Drill into `.error.message`
/// then `.message` until a plain (non-JSON) string remains. Plain messages pass through.
fn human_error_message(raw: &str) -> String {
    let mut current = raw.to_string();
    for _ in 0..3 {
        let Ok(v) = serde_json::from_str::<Value>(&current) else {
            break;
        };
        let next = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .or_else(|| v.get("message").and_then(|m| m.as_str()));
        match next {
            Some(s) if s != current => current = s.to_string(),
            _ => break,
        }
    }
    current
}

/// `"{server}:{tool}"` name for an exec `mcp_tool_call` item. Defaults: server "mcp", tool
/// "unknown" (the app-server transport defaults tool to "tool" — kept distinct intentionally).
fn mcp_tool_name(item: &Value) -> String {
    let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("mcp");
    let tool = item
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    format!("{}:{}", server, tool)
}

/// Build Edit ToolStart input from a Codex `file_change` item. Live shape is
/// `changes: [{path, kind}]` (array). Surface the first path as `file_path` so the Edit
/// renderer shows a label; keep the full `changes` list for context. Codex live file_change
/// carries no diff body, so no old/new strings are available.
fn file_change_input(item: &Value) -> Value {
    let changes = item
        .get("changes")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    // Tolerate either an array of {path,kind} or an object map keyed by path.
    let (first_path, first_kind) = match &changes {
        Value::Array(arr) => {
            let first = arr.first();
            (
                first
                    .and_then(|c| c.get("path"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                first
                    .and_then(|c| c.get("kind"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            )
        }
        Value::Object(map) => (map.keys().next().cloned(), None),
        _ => (None, None),
    };
    match first_path {
        Some(path) => serde_json::json!({
            "file_path": path,
            "kind": first_kind,
            "changes": changes,
        }),
        None => serde_json::json!({ "changes": changes }),
    }
}

/// Map one Codex todo item to the TodoWrite `newTodos` shape
/// (`{content, status, activeForm}`). Defensive about field names across versions.
fn todo_item_to_new_todo(item: &Value) -> Value {
    let content = item
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("content").and_then(|v| v.as_str()))
        .or_else(|| item.get("title").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let status = match item.get("status").and_then(|v| v.as_str()) {
        Some("in_progress") | Some("in-progress") => "in_progress",
        Some("completed") | Some("complete") | Some("done") => "completed",
        Some("pending") => "pending",
        _ => {
            if item.get("completed").and_then(|v| v.as_bool()) == Some(true) {
                "completed"
            } else {
                "pending"
            }
        }
    };
    serde_json::json!({
        "content": content,
        "status": status,
        "activeForm": content,
    })
}

impl PipeStdoutParser for CodexStdoutParser {
    fn parse_line(&mut self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let type_str = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match type_str {
            "turn.started" => {
                self.turn_counter += 1;
                vec![]
            }
            "item.started" => self.map_item_started(run_id, raw),
            "item.completed" => self.map_item_completed(run_id, raw),
            "item.updated" => {
                // Codex v0.130 only emits item.updated for todo_list. Other item types
                // go item.started → item.completed directly. Emit Raw for unknown types
                // so future upstream additions (e.g. streaming command output) aren't
                // silently dropped.
                let item = raw.get("item");
                let item_type = item
                    .and_then(|i| i.get("type").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if item_type == "todo_list" {
                    if let Some(item) = item {
                        // Same scoped id as item.completed → updates the same TodoWrite card
                        // in place (reducer dedupes ToolStart, updates ToolEnd by id).
                        return self.map_todo_list(run_id, item);
                    }
                }
                if let Some(item) = item {
                    return vec![BusEvent::Raw {
                        run_id: run_id.to_string(),
                        source: "codex_item_updated_unknown".to_string(),
                        data: item.clone(),
                    }];
                }
                vec![]
            }
            "turn.completed" => self.map_turn_completed(run_id, raw),
            "turn.failed" => self.map_turn_failed(run_id, raw),
            "error" => self.map_error(run_id, raw),
            // thread.started (+ any unknown type) → no events.
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parser() -> CodexStdoutParser {
        CodexStdoutParser::new(1)
    }

    #[test]
    fn error_message_unwrapped() {
        // plain text passes through
        assert_eq!(human_error_message("plain text"), "plain text");
        // single-level nested blob
        assert_eq!(
            human_error_message(r#"{"error":{"message":"boom"}}"#),
            "boom"
        );
        // doubly-nested stringified blob (Codex's real shape)
        let inner = r#"{"type":"error","status":400,"message":"The x model is not supported"}"#;
        let outer = json!({ "message": inner }).to_string();
        assert_eq!(human_error_message(&outer), "The x model is not supported");
    }

    // ── thread.started / turn.started ──

    #[test]
    fn thread_started_returns_empty() {
        let mut p = parser();
        let events = p.parse_line(
            "run-1",
            &json!({"type": "thread.started", "thread_id": "t1"}),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn turn_started_increments_counter() {
        let mut p = parser();
        p.parse_line("run-1", &json!({"type": "turn.started"}));
        assert_eq!(p.turn_counter, 1);
        p.parse_line("run-1", &json!({"type": "turn.started"}));
        assert_eq!(p.turn_counter, 2);
    }

    // ── item.started → ToolStart ──

    #[test]
    fn command_execution_started() {
        let mut p = parser();
        p.turn_counter = 1;
        let raw = json!({
            "type": "item.started",
            "item": {"id": "cmd_0", "type": "command_execution", "command": "ls -la"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart {
                tool_name,
                tool_use_id,
                input,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_use_id, "codex-1-1-cmd_0");
                assert_eq!(input["command"], "ls -la");
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn file_change_started() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "fc_0", "type": "file_change", "changes": {"file": "a.rs"}}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "Edit"),
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn mcp_tool_call_started() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "mcp_0", "type": "mcp_tool_call", "server": "fs", "tool": "read", "arguments": {"path": "/tmp"}}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "fs:read"),
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn mcp_tool_call_missing_tool_defaults_to_unknown() {
        // Exec parser defaults a missing `tool` field to "unknown" — INTENTIONALLY different from
        // the app-server transport, which defaults to "tool". Lock it so a future "cleanup" can't
        // silently unify them. (Pair: codex_appserver::tests covers the app-server "tool" default.)
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "mcp_1", "type": "mcp_tool_call", "server": "fs"}
        });
        let events = p.parse_line("run-1", &raw);
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "fs:unknown"),
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn web_search_started() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "ws_0", "type": "web_search", "query": "rust async", "action": {"type": "search", "query": "rust async"}}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart {
                tool_name, input, ..
            } => {
                assert_eq!(tool_name, "WebSearch");
                assert_eq!(input["query"], "rust async");
                assert!(input.get("action").is_some());
                assert_eq!(input["action"]["type"], "search");
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn collab_tool_call_started() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "col_0", "type": "collab_tool_call", "tool": "code_review", "prompt": "review this"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "Agent"),
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn agent_message_started_returns_empty() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "msg_0", "type": "agent_message"}
        });
        assert!(p.parse_line("run-1", &raw).is_empty());
    }

    // ── item.completed → MessageComplete / ToolEnd ──

    #[test]
    fn agent_message_completed() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "msg_0", "type": "agent_message", "text": "Hello world"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::MessageComplete { text, .. } => assert_eq!(text, "Hello world"),
            other => panic!("expected MessageComplete, got {:?}", other),
        }
    }

    #[test]
    fn command_execution_completed() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "cmd_0", "type": "command_execution", "command": "ls", "aggregated_output": "a.rs\nb.rs", "status": "completed"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolEnd {
                tool_name,
                status,
                output,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(status, "success"); // "completed" → "success"
                assert_eq!(output["content"], "a.rs\nb.rs");
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn command_execution_failed_maps_to_error() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "cmd_1", "type": "command_execution", "command": "false", "aggregated_output": "", "exit_code": 1, "status": "failed"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolEnd { status, .. } => {
                assert_eq!(status, "error"); // "failed" → "error"
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn command_execution_declined_maps_to_error() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "cmd_2", "type": "command_execution", "command": "rm -rf /", "aggregated_output": "", "status": "declined"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolEnd { status, .. } => {
                assert_eq!(status, "error"); // "declined" → "error"
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn error_item_completed() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "err_0", "type": "error", "message": "rate limited"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CommandOutput { content, .. } => assert!(content.contains("rate limited")),
            other => panic!("expected CommandOutput, got {:?}", other),
        }
    }

    #[test]
    fn todo_list_completed_maps_to_todowrite() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "todo_0", "type": "todo_list", "items": [
                {"text": "do x", "completed": false},
                {"text": "do y", "completed": true},
            ]}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 2, "ToolStart + ToolEnd");
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "TodoWrite"),
            other => panic!("expected ToolStart, got {:?}", other),
        }
        match &events[1] {
            BusEvent::ToolEnd {
                tool_name,
                status,
                tool_use_result,
                ..
            } => {
                assert_eq!(tool_name, "TodoWrite");
                assert_eq!(status, "success");
                let todos = tool_use_result.as_ref().unwrap()["newTodos"]
                    .as_array()
                    .unwrap();
                assert_eq!(todos.len(), 2);
                assert_eq!(todos[0]["content"], "do x");
                assert_eq!(todos[0]["status"], "pending");
                assert_eq!(todos[1]["status"], "completed");
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn file_change_started_sets_file_path() {
        let p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "fc_0", "type": "file_change",
                     "changes": [{"path": "/a/b.rs", "kind": "update"}]}
        });
        let events = p.map_item_started("run-1", &raw);
        match &events[0] {
            BusEvent::ToolStart {
                tool_name, input, ..
            } => {
                assert_eq!(tool_name, "Edit");
                assert_eq!(input["file_path"], "/a/b.rs");
            }
            other => panic!("expected ToolStart Edit, got {:?}", other),
        }
    }

    // ── turn.completed → UsageUpdate ──

    #[test]
    fn turn_completed_emits_usage() {
        let mut p = parser();
        p.turn_counter = 1;
        let raw = json!({
            "type": "turn.completed",
            "usage": {
                "input_tokens": 500,
                "cached_input_tokens": 100,
                "output_tokens": 200,
                "reasoning_output_tokens": 50
            }
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                turn_index,
                ..
            } => {
                assert_eq!(*input_tokens, 500);
                // reasoning_output_tokens (50) folded into output_tokens (200) → 250
                assert_eq!(*output_tokens, 250);
                assert_eq!(*cache_read_tokens, Some(100));
                assert_eq!(*turn_index, Some(1));
            }
            other => panic!("expected UsageUpdate, got {:?}", other),
        }
    }

    #[test]
    fn turn_completed_without_reasoning_tokens() {
        // Backward compat: usage without reasoning_output_tokens treats it as 0.
        let mut p = parser();
        let raw = json!({
            "type": "turn.completed",
            "usage": {"input_tokens": 100, "output_tokens": 30}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::UsageUpdate { output_tokens, .. } => assert_eq!(*output_tokens, 30),
            other => panic!("expected UsageUpdate, got {:?}", other),
        }
    }

    #[test]
    fn turn_completed_no_usage_returns_empty() {
        let mut p = parser();
        let raw = json!({"type": "turn.completed"});
        assert!(p.parse_line("run-1", &raw).is_empty());
    }

    // ── turn.failed → RunState ──

    #[test]
    fn turn_failed_emits_run_state() {
        let mut p = parser();
        let raw = json!({
            "type": "turn.failed",
            "error": {"message": "timeout"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::RunState { state, error, .. } => {
                assert_eq!(state, "failed");
                assert_eq!(error.as_deref(), Some("timeout"));
            }
            other => panic!("expected RunState, got {:?}", other),
        }
    }

    // ── error → CommandOutput ──

    #[test]
    fn error_event_emits_command_output() {
        let mut p = parser();
        let raw = json!({"type": "error", "message": "API failure"});
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CommandOutput { content, .. } => {
                assert!(content.contains("API failure"));
            }
            other => panic!("expected CommandOutput, got {:?}", other),
        }
    }

    // ── item.updated ──

    #[test]
    fn item_updated_todo_list_maps_to_todowrite() {
        let mut p = parser();
        let events = p.parse_line(
            "run-1",
            &json!({
                "type": "item.updated",
                "item": {
                    "type": "todo_list",
                    "id": "tl-1",
                    "items": [{"text": "step 1", "status": "in_progress"}]
                }
            }),
        );
        assert_eq!(events.len(), 2, "ToolStart + ToolEnd");
        match &events[1] {
            BusEvent::ToolEnd {
                tool_name,
                tool_use_result,
                ..
            } => {
                assert_eq!(tool_name, "TodoWrite");
                let todos = tool_use_result.as_ref().unwrap()["newTodos"]
                    .as_array()
                    .unwrap();
                assert_eq!(todos[0]["content"], "step 1");
                assert_eq!(todos[0]["status"], "in_progress");
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn item_updated_unknown_emits_raw() {
        // Defensive: unknown item.updated types must produce a Raw event so future
        // upstream additions are not silently dropped.
        let mut p = parser();
        let events = p.parse_line(
            "run-1",
            &json!({
                "type": "item.updated",
                "item": { "type": "agent_message", "id": "m1", "text": "hello" }
            }),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, .. } => assert_eq!(source, "codex_item_updated_unknown"),
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    #[test]
    fn item_updated_missing_item_returns_empty() {
        // Boundary: item.updated with no item field is unparseable, return empty.
        let mut p = parser();
        let events = p.parse_line("run-1", &json!({ "type": "item.updated" }));
        assert!(events.is_empty());
    }

    // ── reasoning → ThinkingDelta ──

    #[test]
    fn reasoning_completed() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "r_0", "type": "reasoning", "text": "Let me think about this..."}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ThinkingDelta { text, .. } => {
                assert_eq!(text, "Let me think about this...");
            }
            other => panic!("expected ThinkingDelta, got {:?}", other),
        }
    }

    #[test]
    fn reasoning_empty_text_returns_empty() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "r_1", "type": "reasoning", "text": ""}
        });
        assert!(p.parse_line("run-1", &raw).is_empty());
    }

    #[test]
    fn web_search_started_without_action() {
        let mut p = parser();
        let raw = json!({
            "type": "item.started",
            "item": {"id": "ws_1", "type": "web_search", "query": "hello"}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::ToolStart { input, .. } => {
                assert_eq!(input["query"], "hello");
                assert!(input.get("action").is_none());
            }
            other => panic!("expected ToolStart, got {:?}", other),
        }
    }

    // ── ID scoping ──

    #[test]
    fn id_scoping_across_turns() {
        let mut p = CodexStdoutParser::new(2);
        p.parse_line("run-1", &json!({"type": "turn.started"}));
        let events = p.parse_line(
            "run-1",
            &json!({
                "type": "item.started",
                "item": {"id": "item_0", "type": "command_execution", "command": "echo hi"}
            }),
        );
        match &events[0] {
            BusEvent::ToolStart { tool_use_id, .. } => {
                assert_eq!(tool_use_id, "codex-2-1-item_0");
            }
            _ => panic!("expected ToolStart"),
        }
    }
}

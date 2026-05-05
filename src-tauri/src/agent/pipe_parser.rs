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

        match item_type {
            "command_execution" => {
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
            "file_change" => {
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: "Edit".to_string(),
                    input: item
                        .get("changes")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    parent_tool_use_id: None,
                }]
            }
            "mcp_tool_call" => {
                let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("mcp");
                let tool = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                vec![BusEvent::ToolStart {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: format!("{}:{}", server, tool),
                    input: item
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    parent_tool_use_id: None,
                }]
            }
            "web_search" => {
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
            "collab_tool_call" => {
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
            _ => vec![],
        }
    }

    /// Map Codex item status to app convention: "success" or "error".
    /// Codex uses: completed/failed/declined/in_progress (see exec_events.rs).
    fn normalize_status(raw_status: &str) -> String {
        match raw_status {
            "completed" => "success".to_string(),
            "failed" | "declined" => "error".to_string(),
            // Treat unknown / missing as success (same as "completed")
            _ => "success".to_string(),
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
        let status = Self::normalize_status(raw_status);

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
                let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("mcp");
                let tool = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                vec![BusEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    tool_use_id,
                    tool_name: format!("{}:{}", server, tool),
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
            "todo_list" => {
                vec![BusEvent::Raw {
                    run_id: run_id.to_string(),
                    source: "codex_todo_list".to_string(),
                    data: item.clone(),
                }]
            }
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
        vec![BusEvent::UsageUpdate {
            run_id: run_id.to_string(),
            input_tokens: input,
            output_tokens: output,
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
            .unwrap_or("turn failed")
            .to_string();
        vec![BusEvent::RunState {
            run_id: run_id.to_string(),
            state: "failed".to_string(),
            exit_code: None,
            error: Some(error_msg),
        }]
    }

    fn map_error(&self, run_id: &str, raw: &Value) -> Vec<BusEvent> {
        let msg = raw
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| raw.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error")
            .to_string();
        vec![BusEvent::CommandOutput {
            run_id: run_id.to_string(),
            content: format!("[codex error] {}", msg),
        }]
    }
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
                // item.updated is only emitted for todo_list items (plan updates)
                let item = raw.get("item");
                let item_type = item
                    .and_then(|i| i.get("type").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if item_type == "todo_list" {
                    if let Some(item) = item {
                        return vec![BusEvent::Raw {
                            run_id: run_id.to_string(),
                            source: "codex_todo_list".to_string(),
                            data: item.clone(),
                        }];
                    }
                }
                vec![]
            }
            "turn.completed" => self.map_turn_completed(run_id, raw),
            "turn.failed" => self.map_turn_failed(run_id, raw),
            "error" => self.map_error(run_id, raw),
            "thread.started" | _ => vec![],
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
    fn todo_list_completed_returns_raw() {
        let mut p = parser();
        let raw = json!({
            "type": "item.completed",
            "item": {"id": "todo_0", "type": "todo_list", "items": []}
        });
        let events = p.parse_line("run-1", &raw);
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, .. } => assert_eq!(source, "codex_todo_list"),
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    // ── turn.completed → UsageUpdate ──

    #[test]
    fn turn_completed_emits_usage() {
        let mut p = parser();
        p.turn_counter = 1;
        let raw = json!({
            "type": "turn.completed",
            "usage": {"input_tokens": 500, "cached_input_tokens": 100, "output_tokens": 200}
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
                assert_eq!(*output_tokens, 200);
                assert_eq!(*cache_read_tokens, Some(100));
                assert_eq!(*turn_index, Some(1));
            }
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
    fn item_updated_todo_list_emits_raw() {
        let mut p = parser();
        let events = p.parse_line(
            "run-1",
            &json!({
                "type": "item.updated",
                "item": {
                    "type": "todo_list",
                    "id": "tl-1",
                    "items": [{"text": "step 1", "done": true}]
                }
            }),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::Raw { source, .. } => assert_eq!(source, "codex_todo_list"),
            _ => panic!("expected Raw"),
        }
    }

    #[test]
    fn item_updated_non_todo_is_ignored() {
        let mut p = parser();
        let events = p.parse_line(
            "run-1",
            &json!({
                "type": "item.updated",
                "item": { "type": "agent_message", "id": "m1", "text": "hello" }
            }),
        );
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

use serde_json::Value;

/// The tool category a Codex item maps to. Shared by both Codex transports — the one-way `exec`
/// NDJSON parser ([`crate::agent::pipe_parser`], snake_case item types) and the bidirectional
/// `app-server` driver ([`crate::agent::codex_appserver`], camelCase item types) — so the
/// item→tool-name decision lives in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexToolKind {
    /// `command_execution` / `commandExecution` → "Bash".
    Command,
    /// `file_change` / `fileChange` → "Edit".
    FileChange,
    /// `web_search` / `webSearch` → "WebSearch".
    WebSearch,
    /// `mcp_tool_call` / `mcpToolCall` → "{server}:{tool}" (caller supplies server/tool).
    McpToolCall,
    /// `collab_tool_call` / `collabToolCall` → "Agent".
    CollabToolCall,
}

impl CodexToolKind {
    /// Classify a Codex item `type` string, tolerating BOTH the exec snake_case and the
    /// app-server camelCase spellings. Returns `None` for non-tool items (agent_message,
    /// reasoning, user_message, todo_list, error, …) which each transport handles separately.
    pub fn from_item_type(item_type: &str) -> Option<Self> {
        match item_type {
            "command_execution" | "commandExecution" => Some(Self::Command),
            "file_change" | "fileChange" => Some(Self::FileChange),
            "web_search" | "webSearch" => Some(Self::WebSearch),
            "mcp_tool_call" | "mcpToolCall" => Some(Self::McpToolCall),
            "collab_tool_call" | "collabToolCall" => Some(Self::CollabToolCall),
            _ => None,
        }
    }

    /// The fixed tool name for non-MCP kinds. `McpToolCall` returns `None` because its name is
    /// `"{server}:{tool}"`, which depends on item fields the caller extracts (with its own
    /// defaults for missing values, which differ slightly between transports).
    pub fn fixed_tool_name(self) -> Option<&'static str> {
        match self {
            Self::Command => Some("Bash"),
            Self::FileChange => Some("Edit"),
            Self::WebSearch => Some("WebSearch"),
            Self::CollabToolCall => Some("Agent"),
            Self::McpToolCall => None,
        }
    }
}

/// Normalize a Codex item `status` to the app convention ("success" | "error"). Codex emits
/// `completed` / `failed` / `declined` / `in_progress` (see exec_events.rs); only `failed` and
/// `declined` are surfaced as errors, everything else (including missing/unknown) is "success".
/// Shared by both Codex transports so the two parsers can't drift.
pub fn codex_normalize_status(raw_status: &str) -> &'static str {
    match raw_status {
        "failed" | "declined" => "error",
        _ => "success",
    }
}

/// Extract text delta from a Codex NDJSON payload.
///
/// Codex CLI v0.98+ output format (NDJSON):
///   {"type":"thread.started","thread_id":"..."}
///   {"type":"turn.started"}
///   {"type":"item.completed","item":{"id":"...","type":"agent_message","text":"Hello!"}}
///   {"type":"item.completed","item":{"id":"...","type":"command_execution","command":"ls","output":"..."}}
///   {"type":"turn.completed","usage":{"input_tokens":N,"output_tokens":N}}
pub fn extract_codex_delta(payload: &Value) -> Option<String> {
    let type_str = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Codex v0.98+: item.completed with nested item object
    if type_str == "item.completed" {
        if let Some(item) = payload.get("item") {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "agent_message" => {
                    return item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                "command_execution" => {
                    // Show command + output in terminal
                    let cmd = item.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    let output = item
                        .get("aggregated_output")
                        .or_else(|| item.get("output")) // fallback for older Codex versions
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !cmd.is_empty() {
                        return Some(format!("$ {}\n{}", cmd, output));
                    }
                }
                _ => {}
            }
        }
    }

    // Direct delta field (older Codex versions)
    if type_str.contains("delta") {
        if let Some(delta) = payload.get("delta").and_then(|v| v.as_str()) {
            return Some(delta.to_string());
        }
        if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }

    // output_text field
    if let Some(text) = payload.get("output_text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    // Nested data field
    if let Some(data) = payload.get("data") {
        if let Some(delta) = data.get("delta").and_then(|v| v.as_str()) {
            return Some(delta.to_string());
        }
        if type_str.contains("delta") {
            if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
        }
        if let Some(text) = data.get("output_text").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_agent_message() {
        let payload =
            json!({"type": "item.completed", "item": {"type": "agent_message", "text": "Hello"}});
        assert_eq!(extract_codex_delta(&payload), Some("Hello".to_string()));
    }

    #[test]
    fn test_command_execution_aggregated_output() {
        // Codex v0.98+ uses aggregated_output (not output)
        let payload = json!({
            "type": "item.completed",
            "item": {"type": "command_execution", "command": "ls", "aggregated_output": "file.txt"}
        });
        assert_eq!(
            extract_codex_delta(&payload),
            Some("$ ls\nfile.txt".to_string())
        );
    }

    #[test]
    fn test_command_execution_legacy_output() {
        // Older Codex versions used "output" field — fallback still works
        let payload = json!({
            "type": "item.completed",
            "item": {"type": "command_execution", "command": "ls", "output": "file.txt"}
        });
        assert_eq!(
            extract_codex_delta(&payload),
            Some("$ ls\nfile.txt".to_string())
        );
    }

    #[test]
    fn test_command_execution_empty_cmd() {
        let payload = json!({
            "type": "item.completed",
            "item": {"type": "command_execution", "command": "", "aggregated_output": ""}
        });
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_unknown_item_type() {
        let payload = json!({
            "type": "item.completed",
            "item": {"type": "new_type", "data": 123}
        });
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_turn_completed() {
        let payload = json!({"type": "turn.completed", "usage": {"input_tokens": 100}});
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_thread_started() {
        let payload = json!({"type": "thread.started", "thread_id": "t1"});
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_delta_type() {
        let payload = json!({"type": "response.delta", "delta": "partial text"});
        assert_eq!(
            extract_codex_delta(&payload),
            Some("partial text".to_string())
        );
    }

    #[test]
    fn test_delta_text_field() {
        let payload = json!({"type": "some_delta", "text": "hello"});
        assert_eq!(extract_codex_delta(&payload), Some("hello".to_string()));
    }

    #[test]
    fn test_output_text() {
        let payload = json!({"type": "response", "output_text": "full output"});
        assert_eq!(
            extract_codex_delta(&payload),
            Some("full output".to_string())
        );
    }

    #[test]
    fn test_output_text_empty() {
        let payload = json!({"type": "response", "output_text": ""});
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_nested_data_delta() {
        let payload = json!({"type": "event", "data": {"delta": "nested text"}});
        assert_eq!(
            extract_codex_delta(&payload),
            Some("nested text".to_string())
        );
    }

    #[test]
    fn test_no_type() {
        let payload = json!({"data": {}});
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn test_empty_payload() {
        let payload = json!({});
        assert_eq!(extract_codex_delta(&payload), None);
    }

    #[test]
    fn tool_kind_accepts_both_casings() {
        use CodexToolKind::*;
        // snake_case (exec) and camelCase (app-server) classify identically.
        for (snake, camel, kind) in [
            ("command_execution", "commandExecution", Command),
            ("file_change", "fileChange", FileChange),
            ("web_search", "webSearch", WebSearch),
            ("mcp_tool_call", "mcpToolCall", McpToolCall),
            ("collab_tool_call", "collabToolCall", CollabToolCall),
        ] {
            assert_eq!(CodexToolKind::from_item_type(snake), Some(kind));
            assert_eq!(CodexToolKind::from_item_type(camel), Some(kind));
        }
        // Non-tool item types → None.
        for t in [
            "agent_message",
            "agentMessage",
            "reasoning",
            "todo_list",
            "error",
            "",
        ] {
            assert_eq!(CodexToolKind::from_item_type(t), None);
        }
    }

    #[test]
    fn tool_kind_fixed_names() {
        use CodexToolKind::*;
        assert_eq!(Command.fixed_tool_name(), Some("Bash"));
        assert_eq!(FileChange.fixed_tool_name(), Some("Edit"));
        assert_eq!(WebSearch.fixed_tool_name(), Some("WebSearch"));
        assert_eq!(CollabToolCall.fixed_tool_name(), Some("Agent"));
        // MCP has no fixed name — it's "{server}:{tool}", built by the caller.
        assert_eq!(McpToolCall.fixed_tool_name(), None);
    }

    #[test]
    fn normalize_status_failed_and_declined_are_errors() {
        assert_eq!(codex_normalize_status("failed"), "error");
        assert_eq!(codex_normalize_status("declined"), "error");
        // Everything else (completed / in_progress / unknown / missing) is success.
        assert_eq!(codex_normalize_status("completed"), "success");
        assert_eq!(codex_normalize_status("in_progress"), "success");
        assert_eq!(codex_normalize_status("whatever"), "success");
        assert_eq!(codex_normalize_status(""), "success");
    }
}

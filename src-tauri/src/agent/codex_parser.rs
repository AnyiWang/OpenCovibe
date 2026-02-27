use serde_json::Value;

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
                    let output = item.get("output").and_then(|v| v.as_str()).unwrap_or("");
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
    fn test_command_execution() {
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
            "item": {"type": "command_execution", "command": "", "output": ""}
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
}

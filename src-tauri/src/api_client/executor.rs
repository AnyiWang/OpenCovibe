use crate::api_client::anthropic::{stream_messages, StreamEvent};
use crate::api_client::tools::{coding_tools, is_read_only};
use crate::models::{ChatDelta, RunEventType, RunStatus, ToolRequest, ToolResult};
use crate::storage;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex};

/// Map of request_id → oneshot sender for permission decisions
pub type PermissionMap = Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>;

pub fn new_permission_map() -> PermissionMap {
    Arc::new(Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

/// Run the API agent loop: send messages, handle tool calls, request permissions
#[allow(clippy::too_many_arguments)]
pub async fn run_api_agent(
    app: AppHandle,
    permission_map: PermissionMap,
    run_id: String,
    api_key: String,
    model: String,
    initial_prompt: String,
    cwd: String,
    permission_mode: String,
    base_url: Option<String>,
) -> Result<(), String> {
    log::debug!(
        "[executor] run_api_agent start: run_id={}, model={}, cwd={}",
        run_id,
        model,
        cwd
    );
    if let Err(e) = storage::runs::update_status(&run_id, RunStatus::Running, None, None) {
        log::warn!("[executor] failed to update status to Running: {}", e);
    }

    let system_prompt = format!(
        "You are an AI coding assistant. You have access to tools for reading, writing, and editing files, \
         running bash commands, listing directories, and searching files. \
         The current working directory is: {}\n\
         Use relative paths from this directory when possible.",
        cwd
    );

    let tools = coding_tools();
    let mut messages: Vec<Value> = vec![json!({
        "role": "user",
        "content": initial_prompt,
    })];

    // Log initial prompt
    if let Err(e) = storage::events::append_event(
        &run_id,
        RunEventType::User,
        json!({ "text": initial_prompt, "source": "api_chat" }),
    ) {
        log::warn!("[executor] failed to log initial prompt: {}", e);
    }

    let max_iterations = 50;
    let mut iteration = 0;

    loop {
        iteration += 1;
        if iteration > max_iterations {
            let _ = app.emit(
                "chat-delta",
                ChatDelta {
                    text: "\n\n[Agent loop exceeded maximum iterations]".to_string(),
                },
            );
            break;
        }

        // Send to API
        log::debug!(
            "[executor] iteration {}/{}, sending {} messages to API",
            iteration,
            max_iterations,
            messages.len()
        );
        let mut rx = stream_messages(
            &api_key,
            &model,
            messages.clone(),
            tools.clone(),
            Some(&system_prompt),
            8192,
            base_url.as_deref(),
        )
        .await?;

        // Collect the response
        let mut text_content = String::new();
        let mut tool_uses: Vec<Value> = Vec::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input_json = String::new();
        let mut stop_reason = String::new();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::MessageStart { message } => {
                    if let Some(usage) = message.get("usage") {
                        total_input_tokens += usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                }
                StreamEvent::ContentBlockStart { content_block, .. } => {
                    let block_type = content_block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if block_type == "tool_use" {
                        current_tool_id = content_block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        current_tool_name = content_block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        current_tool_input_json.clear();
                    }
                }
                StreamEvent::ContentBlockDelta { delta, .. } => {
                    let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    if delta_type == "text_delta" {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                            text_content.push_str(text);
                            let _ = app.emit(
                                "chat-delta",
                                ChatDelta {
                                    text: text.to_string(),
                                },
                            );
                        }
                    } else if delta_type == "input_json_delta" {
                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            current_tool_input_json.push_str(partial);
                        }
                    }
                }
                StreamEvent::ContentBlockStop { .. } => {
                    if !current_tool_id.is_empty() {
                        let input: Value =
                            serde_json::from_str(&current_tool_input_json).unwrap_or(Value::Null);
                        tool_uses.push(json!({
                            "type": "tool_use",
                            "id": current_tool_id,
                            "name": current_tool_name,
                            "input": input,
                        }));
                        current_tool_id.clear();
                        current_tool_name.clear();
                        current_tool_input_json.clear();
                    }
                }
                StreamEvent::MessageDelta { delta, usage } => {
                    if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                        stop_reason = sr.to_string();
                    }
                    if let Some(u) = usage {
                        total_output_tokens +=
                            u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    }
                }
                StreamEvent::MessageStop => {}
                StreamEvent::Error { error } => {
                    log::error!("[executor] stream error: {:?}", error);
                    let msg = error
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown API error");
                    return Err(format!("API error: {}", msg));
                }
            }
        }

        // Emit usage
        let cost =
            crate::pricing::estimate_cost(&model, total_input_tokens, total_output_tokens, 0, 0);
        let _ = app.emit(
            "hook-usage",
            json!({
                "run_id": run_id,
                "input_tokens": total_input_tokens,
                "output_tokens": total_output_tokens,
                "cost": cost,
            }),
        );

        // Log assistant response
        if !text_content.is_empty() {
            if let Err(e) = storage::events::append_event(
                &run_id,
                RunEventType::Assistant,
                json!({ "text": text_content, "source": "api_chat" }),
            ) {
                log::warn!("[executor] failed to log assistant response: {}", e);
            }
        }

        // Build the assistant message for conversation history
        let mut assistant_content: Vec<Value> = Vec::new();
        if !text_content.is_empty() {
            assistant_content.push(json!({
                "type": "text",
                "text": text_content,
            }));
        }
        for tu in &tool_uses {
            assistant_content.push(tu.clone());
        }
        messages.push(json!({
            "role": "assistant",
            "content": assistant_content,
        }));

        log::debug!(
            "[executor] response: {} chars text, {} tool_uses, stop_reason={}, tokens={}in/{}out",
            text_content.len(),
            tool_uses.len(),
            stop_reason,
            total_input_tokens,
            total_output_tokens
        );

        // If stop_reason is end_turn (no tool use), we're done
        if stop_reason == "end_turn" || tool_uses.is_empty() {
            break;
        }

        // Process tool calls
        let mut tool_results: Vec<Value> = Vec::new();

        for tool_use in &tool_uses {
            let tool_id = tool_use.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let tool_name = tool_use.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let input = tool_use.get("input").cloned().unwrap_or(Value::Null);

            let request_id = uuid::Uuid::new_v4().to_string();

            // Check permission
            let allowed = match permission_mode.as_str() {
                "auto_all" => true,
                "auto_read" => {
                    if is_read_only(tool_name) {
                        true
                    } else {
                        request_permission(
                            &app,
                            &permission_map,
                            &run_id,
                            &request_id,
                            tool_name,
                            &input,
                        )
                        .await
                    }
                }
                _ => {
                    // "ask" mode — always ask
                    request_permission(
                        &app,
                        &permission_map,
                        &run_id,
                        &request_id,
                        tool_name,
                        &input,
                    )
                    .await
                }
            };

            let result = if allowed {
                // Emit tool-request with status running
                let _ = app.emit(
                    "hook-event",
                    json!({
                        "run_id": run_id,
                        "hook_type": "ToolExec",
                        "tool_name": tool_name,
                        "tool_input": input,
                        "status": "running",
                        "timestamp": crate::models::now_iso(),
                    }),
                );

                let output = execute_tool(tool_name, &input, &cwd).await;

                // Emit tool result
                let status = if output.get("error").is_some() {
                    "error"
                } else {
                    "done"
                };
                let _ = app.emit(
                    "hook-event",
                    json!({
                        "run_id": run_id,
                        "hook_type": "PostToolUse",
                        "tool_name": tool_name,
                        "tool_input": input,
                        "tool_output": output,
                        "status": status,
                        "timestamp": crate::models::now_iso(),
                    }),
                );

                let _ = app.emit(
                    "tool-result",
                    ToolResult {
                        run_id: run_id.clone(),
                        request_id: request_id.clone(),
                        tool_name: tool_name.to_string(),
                        output: output.clone(),
                        status: status.to_string(),
                    },
                );

                // Log to events
                if let Err(e) = storage::events::append_event(
                    &run_id,
                    RunEventType::Command,
                    json!({
                        "tool": tool_name,
                        "input": input,
                        "output": output,
                        "status": status,
                        "source": "api_chat",
                    }),
                ) {
                    log::warn!("[executor] failed to log tool result: {}", e);
                }

                let content = if let Some(text) = output.get("content").and_then(|v| v.as_str()) {
                    text.to_string()
                } else {
                    serde_json::to_string(&output).unwrap_or_default()
                };

                json!({
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": content,
                })
            } else {
                // Denied
                let _ = app.emit(
                    "tool-result",
                    ToolResult {
                        run_id: run_id.clone(),
                        request_id,
                        tool_name: tool_name.to_string(),
                        output: json!({ "error": "Permission denied by user" }),
                        status: "denied".to_string(),
                    },
                );

                json!({
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": "Error: Permission denied by user",
                    "is_error": true,
                })
            };

            tool_results.push(result);
        }

        // Add tool results to messages
        messages.push(json!({
            "role": "user",
            "content": tool_results,
        }));
    }

    // Done
    if let Err(e) = storage::runs::update_status(&run_id, RunStatus::Completed, Some(0), None) {
        log::warn!("[executor] failed to update status to Completed: {}", e);
    }
    let _ = app.emit(
        "chat-done",
        crate::models::ChatDone {
            ok: true,
            code: 0,
            error: None,
        },
    );

    Ok(())
}

/// Request permission from the frontend
async fn request_permission(
    app: &AppHandle,
    permission_map: &PermissionMap,
    run_id: &str,
    request_id: &str,
    tool_name: &str,
    input: &Value,
) -> bool {
    let (tx, rx) = oneshot::channel::<PermissionDecision>();

    // Store the sender
    {
        let mut map = permission_map.lock().await;
        map.insert(request_id.to_string(), tx);
    }

    // Emit request to frontend
    let _ = app.emit(
        "tool-request",
        ToolRequest {
            run_id: run_id.to_string(),
            request_id: request_id.to_string(),
            tool_name: tool_name.to_string(),
            input: input.clone(),
        },
    );

    // Wait for response
    matches!(rx.await, Ok(PermissionDecision::Allow))
}

/// Execute a tool locally
async fn execute_tool(tool_name: &str, input: &Value, cwd: &str) -> Value {
    match tool_name {
        "read_file" => {
            let path = match resolve_path(
                input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                cwd,
            ) {
                Ok(p) => p,
                Err(e) => return json!({ "error": e }),
            };
            match std::fs::read_to_string(&path) {
                Ok(content) => json!({ "content": content }),
                Err(e) => json!({ "error": format!("Failed to read {}: {}", path, e) }),
            }
        }
        "write_file" => {
            let path = match resolve_path(
                input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                cwd,
            ) {
                Ok(p) => p,
                Err(e) => return json!({ "error": e }),
            };
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            // Ensure parent directory exists
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, content) {
                Ok(()) => json!({ "content": format!("Successfully wrote to {}", path) }),
                Err(e) => json!({ "error": format!("Failed to write {}: {}", path, e) }),
            }
        }
        "edit_file" => {
            let path = match resolve_path(
                input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                cwd,
            ) {
                Ok(p) => p,
                Err(e) => return json!({ "error": e }),
            };
            let old_string = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_string = input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let count = content.matches(old_string).count();
                    if count == 0 {
                        json!({ "error": format!("old_string not found in {}", path) })
                    } else if count > 1 {
                        json!({ "error": format!("old_string found {} times in {} (must be unique)", count, path) })
                    } else {
                        let new_content = content.replacen(old_string, new_string, 1);
                        match std::fs::write(&path, new_content) {
                            Ok(()) => json!({ "content": format!("Successfully edited {}", path) }),
                            Err(e) => {
                                json!({ "error": format!("Failed to write {}: {}", path, e) })
                            }
                        }
                    }
                }
                Err(e) => json!({ "error": format!("Failed to read {}: {}", path, e) }),
            }
        }
        "bash" => {
            let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let timeout_ms = input
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(120_000);

            let result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(command)
                    .current_dir(cwd)
                    .output(),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let code = output.status.code().unwrap_or(-1);
                    let mut content = String::new();
                    if !stdout.is_empty() {
                        content.push_str(&stdout);
                    }
                    if !stderr.is_empty() {
                        if !content.is_empty() {
                            content.push_str("\n--- stderr ---\n");
                        }
                        content.push_str(&stderr);
                    }
                    if code != 0 {
                        content.push_str(&format!("\n(exit code: {})", code));
                    }
                    if content.is_empty() {
                        content = format!("Command completed with exit code {}", code);
                    }
                    json!({ "content": content })
                }
                Ok(Err(e)) => json!({ "error": format!("Failed to execute command: {}", e) }),
                Err(_) => json!({ "error": format!("Command timed out after {}ms", timeout_ms) }),
            }
        }
        "list_directory" => {
            let path = match resolve_path(
                input.get("path").and_then(|v| v.as_str()).unwrap_or("."),
                cwd,
            ) {
                Ok(p) => p,
                Err(e) => return json!({ "error": e }),
            };
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut items: Vec<String> = Vec::new();
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        items.push(if is_dir { format!("{}/", name) } else { name });
                    }
                    items.sort();
                    json!({ "content": items.join("\n") })
                }
                Err(e) => json!({ "error": format!("Failed to list {}: {}", path, e) }),
            }
        }
        "search_files" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = match resolve_path(
                input.get("path").and_then(|v| v.as_str()).unwrap_or("."),
                cwd,
            ) {
                Ok(p) => p,
                Err(e) => return json!({ "error": e }),
            };
            let content_search = input
                .get("content_search")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if content_search {
                // Use grep
                let output = tokio::process::Command::new("grep")
                    .args(["-rn", "--include=*", "-m", "50", pattern, &path])
                    .output()
                    .await;

                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout).to_string();
                        if text.is_empty() {
                            json!({ "content": "No matches found" })
                        } else {
                            json!({ "content": text })
                        }
                    }
                    Err(e) => json!({ "error": format!("Search failed: {}", e) }),
                }
            } else {
                // Use find
                let output = tokio::process::Command::new("find")
                    .args([&path, "-name", pattern, "-maxdepth", "5"])
                    .output()
                    .await;

                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout).to_string();
                        if text.is_empty() {
                            json!({ "content": "No files found" })
                        } else {
                            json!({ "content": text })
                        }
                    }
                    Err(e) => json!({ "error": format!("Search failed: {}", e) }),
                }
            }
        }
        _ => json!({ "error": format!("Unknown tool: {}", tool_name) }),
    }
}

/// Resolve a potentially relative path against the working directory,
/// with canonicalization and cwd containment check.
fn resolve_path(path: &str, cwd: &str) -> Result<String, String> {
    let resolved = if path.starts_with('/') {
        std::path::PathBuf::from(path)
    } else if path.starts_with('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home).join(path.strip_prefix("~/").unwrap_or(path))
    } else {
        std::path::PathBuf::from(cwd).join(path)
    };

    // Canonicalize to resolve .. and symlinks
    let canonical = if resolved.exists() {
        std::fs::canonicalize(&resolved)
            .map_err(|e| format!("Cannot resolve path '{}': {}", path, e))?
    } else {
        resolved
    };

    // Must be under cwd
    let cwd_canonical =
        std::fs::canonicalize(cwd).unwrap_or_else(|_| std::path::PathBuf::from(cwd));
    if !canonical.starts_with(&cwd_canonical) {
        log::warn!(
            "[executor] path '{}' resolved to '{}' which is outside cwd '{}'",
            path,
            canonical.display(),
            cwd_canonical.display()
        );
        return Err(format!("Path '{}' is outside working directory", path));
    }

    log::debug!(
        "[executor] resolve_path: '{}' -> '{}'",
        path,
        canonical.display()
    );
    Ok(canonical.to_string_lossy().to_string())
}

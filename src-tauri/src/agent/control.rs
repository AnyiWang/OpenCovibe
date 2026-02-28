use crate::agent::claude_stream::{augmented_path, resolve_claude_path};
use crate::models::{now_iso, CliAccount, CliCommand, CliInfo, CliInfoError, CliModelInfo};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

/// Cached CLI info with TTL
pub struct CliInfoCache {
    inner: Arc<RwLock<Option<(CliInfo, std::time::Instant)>>>,
}

impl Default for CliInfoCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CliInfoCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }
}

const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes
const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Get CLI info, using cache if available and not expired.
pub async fn get_cli_info(cache: &CliInfoCache, force: bool) -> Result<CliInfo, CliInfoError> {
    // Check cache
    if !force {
        let guard = cache.inner.read().await;
        if let Some((ref info, ref instant)) = *guard {
            if instant.elapsed() < CACHE_TTL {
                log::debug!(
                    "[control] returning cached CLI info ({} models)",
                    info.models.len()
                );
                return Ok(info.clone());
            }
        }
    }

    // Resolve binary
    let claude_bin = resolve_claude_path();
    log::debug!("[control] resolved claude binary: {}", claude_bin);

    if claude_bin == "claude" && crate::agent::claude_stream::which_binary("claude").is_none() {
        return Err(CliInfoError {
            code: "cli_not_found".to_string(),
            message: "Claude CLI binary not found".to_string(),
        });
    }

    // Spawn process
    let path_env = augmented_path();
    let mut cmd = tokio::process::Command::new(&claude_bin);
    cmd.arg("-p")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--verbose")
        .env("PATH", &path_env)
        .env_remove("CLAUDECODE") // Allow running inside a Claude Code session
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        log::error!("[control] failed to spawn claude: {}", e);
        CliInfoError {
            code: "cli_not_found".to_string(),
            message: format!("Failed to spawn claude: {}", e),
        }
    })?;

    log::debug!("[control] spawned claude process pid={:?}", child.id());

    // Send initialize request
    let init_request = serde_json::json!({
        "type": "control_request",
        "request_id": "ocv_init_1",
        "request": { "subtype": "initialize" }
    });

    let mut stdin = child.stdin.take().ok_or_else(|| CliInfoError {
        code: "protocol_error".to_string(),
        message: "Failed to capture stdin".to_string(),
    })?;

    let mut line = serde_json::to_string(&init_request).map_err(|e| CliInfoError {
        code: "protocol_error".to_string(),
        message: format!("Failed to serialize request: {}", e),
    })?;
    line.push('\n');

    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| CliInfoError {
            code: "protocol_error".to_string(),
            message: format!("Failed to write to stdin: {}", e),
        })?;
    if let Err(e) = stdin.flush().await {
        log::warn!("[control] stdin flush failed: {}", e);
    }
    drop(stdin); // Close stdin to signal we're done

    log::debug!("[control] sent initialize request, reading stdout...");

    // Read stdout with timeout
    let stdout = child.stdout.take().ok_or_else(|| CliInfoError {
        code: "protocol_error".to_string(),
        message: "Failed to capture stdout".to_string(),
    })?;

    let result = timeout(PROCESS_TIMEOUT, read_control_response(stdout)).await;

    // Kill process regardless
    let _ = child.kill().await;
    let _ = child.wait().await;

    let cli_info = match result {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(CliInfoError {
                code: "timeout".to_string(),
                message: format!(
                    "Timed out after {}s waiting for CLI response",
                    PROCESS_TIMEOUT.as_secs()
                ),
            });
        }
    };

    // Read current model from ~/.claude/settings.json
    let current_model = read_claude_settings_model();
    let cli_info = CliInfo {
        current_model,
        ..cli_info
    };

    log::debug!(
        "[control] got {} models, {} commands, current_model={:?}",
        cli_info.models.len(),
        cli_info.commands.len(),
        &cli_info.current_model
    );

    // Update cache
    let mut guard = cache.inner.write().await;
    *guard = Some((cli_info.clone(), std::time::Instant::now()));

    Ok(cli_info)
}

/// Read stdout lines looking for a control_response event.
async fn read_control_response(
    stdout: tokio::process::ChildStdout,
) -> Result<CliInfo, CliInfoError> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut reader = BufReader::new(stdout).lines();
    let mut line_count = 0u32;

    while let Ok(Some(text)) = reader.next_line().await {
        line_count += 1;
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        log::trace!(
            "[control] stdout line #{}: {}",
            line_count,
            &text[..text.len().min(200)]
        );

        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if event_type == "control_response" {
            // Extract the response body
            let response = parsed.get("response").ok_or_else(|| CliInfoError {
                code: "protocol_error".to_string(),
                message: "control_response missing 'response' field".to_string(),
            })?;

            // Check for auth errors
            if let Some(error) = response.get("error").and_then(|v| v.as_str()) {
                if error.contains("auth") || error.contains("token") || error.contains("login") {
                    return Err(CliInfoError {
                        code: "not_authenticated".to_string(),
                        message: error.to_string(),
                    });
                }
                return Err(CliInfoError {
                    code: "protocol_error".to_string(),
                    message: format!("Control response error: {}", error),
                });
            }

            // The response may be nested: response.response contains the actual data
            // (CLI returns { subtype, request_id, response: { models, commands, ... } })
            let data = response.get("response").unwrap_or(response);

            // Parse models
            let models: Vec<CliModelInfo> = data
                .get("models")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            let commands: Vec<CliCommand> = data
                .get("commands")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            let available_output_styles: Vec<String> = data
                .get("available_output_styles")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            let account: Option<CliAccount> = data
                .get("account")
                .and_then(|v| serde_json::from_value(v.clone()).ok());

            return Ok(CliInfo {
                models,
                commands,
                available_output_styles,
                account,
                current_model: None, // populated by caller from ~/.claude/settings.json
                fetched_at: now_iso(),
            });
        }

        // Safety: don't read forever
        if line_count > 50 {
            return Err(CliInfoError {
                code: "protocol_error".to_string(),
                message: "No control_response found in first 50 lines".to_string(),
            });
        }
    }

    Err(CliInfoError {
        code: "protocol_error".to_string(),
        message: format!("EOF after {} lines without control_response", line_count),
    })
}

/// Read the "model" field from ~/.claude/settings.json (Claude Code's active model).
fn read_claude_settings_model() -> Option<String> {
    let home = crate::storage::home_dir()?;
    let path = std::path::Path::new(&home)
        .join(".claude")
        .join("settings.json");
    let contents = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let model = parsed.get("model")?.as_str()?;
    if model.is_empty() {
        return None;
    }
    log::debug!("[control] read current model from {:?}: {:?}", path, model);
    Some(model.to_string())
}

/// Fallback model list when CLI is unavailable.
pub fn fallback_cli_info() -> CliInfo {
    CliInfo {
        models: vec![
            CliModelInfo {
                value: "default".to_string(),
                display_name: "Default (recommended)".to_string(),
                description: "Sonnet 4.5".to_string(),
                supports_effort: None,
                supported_effort_levels: None,
                supports_adaptive_thinking: None,
            },
            CliModelInfo {
                value: "opus".to_string(),
                display_name: "Opus".to_string(),
                description: "Opus 4.6".to_string(),
                supports_effort: None,
                supported_effort_levels: None,
                supports_adaptive_thinking: None,
            },
            CliModelInfo {
                value: "haiku".to_string(),
                display_name: "Haiku".to_string(),
                description: "Haiku 4.5".to_string(),
                supports_effort: None,
                supported_effort_levels: None,
                supports_adaptive_thinking: None,
            },
        ],
        commands: vec![],
        available_output_styles: vec!["default".to_string()],
        account: None,
        current_model: read_claude_settings_model(),
        fetched_at: now_iso(),
    }
}

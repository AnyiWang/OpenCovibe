//! Claude CLI utility functions.
//!
//! Process spawning and event streaming are handled by `session_actor.rs`.
//! This module provides shared utilities: binary resolution, PATH augmentation,
//! and one-shot fork execution.

use crate::agent::adapter;
use crate::models::RemoteHost;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::Duration;

/// Build a PATH that includes common binary locations
pub fn augmented_path() -> String {
    let home = crate::storage::home_dir().unwrap_or_default();
    let current_path = std::env::var("PATH").unwrap_or_default();
    let extra_dirs = [
        format!("{}/.local/bin", home),
        format!("{}/.cargo/bin", home),
        // Node version managers
        format!("{}/.nvm/versions/node", home),      // nvm
        format!("{}/.volta/bin", home),              // volta
        format!("{}/.fnm/current/bin", home),        // fnm (current symlink)
        format!("{}/.local/share/mise/shims", home), // mise
        format!("{}/.asdf/shims", home),             // asdf
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    let path_entries: Vec<&str> = current_path.split(':').collect();
    let mut parts: Vec<String> = Vec::new();
    for d in &extra_dirs {
        if !path_entries.contains(&d.as_str()) {
            // For nvm, prefer the default alias, then fall back to highest version
            if d.contains(".nvm/versions/node") {
                // Check nvm default alias first (symlink at ~/.nvm/alias/default)
                let alias_path = format!("{}/.nvm/alias/default", home);
                let default_ver = std::fs::read_to_string(&alias_path)
                    .ok()
                    .map(|s| s.trim().to_string());
                if let Some(ref ver) = default_ver {
                    // nvm alias can be "20" or "v20.20.0" — find matching dir
                    if let Ok(entries) = std::fs::read_dir(d) {
                        let mut found = false;
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name
                                .trim_start_matches('v')
                                .starts_with(ver.trim_start_matches('v'))
                            {
                                let bin = entry.path().join("bin");
                                if bin.exists() {
                                    parts.push(bin.to_string_lossy().to_string());
                                    found = true;
                                    break;
                                }
                            }
                        }
                        if found {
                            // Skip the fallback below
                        } else {
                            // Default alias didn't match, fall through to highest version
                            let default_ver = None::<String>;
                            let _ = default_ver; // suppress unused warning
                        }
                    }
                }
                // Fallback: add highest version (sort by version descending)
                if !parts.iter().any(|p| p.contains(".nvm/versions/node")) {
                    if let Ok(entries) = std::fs::read_dir(d) {
                        let mut version_dirs: Vec<_> = entries
                            .flatten()
                            .filter(|e| e.path().join("bin").exists())
                            .collect();
                        // Sort by directory name descending (v20 > v17)
                        version_dirs.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
                        if let Some(entry) = version_dirs.first() {
                            let bin = entry.path().join("bin");
                            parts.push(bin.to_string_lossy().to_string());
                        }
                    }
                }
            } else if std::path::Path::new(d).exists() {
                parts.push(d.clone());
            }
        }
    }
    if parts.is_empty() {
        current_path
    } else {
        parts.push(current_path);
        parts.join(":")
    }
}

/// One-shot fork: spawns `claude --resume <sid> --fork-session -p "(fork checkpoint)"
/// --output-format json --max-turns 1`, waits for completion, parses result JSON,
/// returns new session_id.
/// Avoids stream-json hang bug (CLI #1920).
#[allow(clippy::too_many_arguments)]
pub async fn fork_oneshot(
    source_session_id: &str,
    cwd: &str,
    settings: &adapter::AdapterSettings,
    remote_host: Option<&RemoteHost>,
    api_key: Option<&str>,
    auth_token: Option<&str>,
    base_url: Option<&str>,
    default_model: Option<&str>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
) -> Result<String, String> {
    let claude_bin = resolve_claude_path();
    log::debug!(
        "[fork_oneshot] source_sid={}, cwd={}, binary={}, remote={:?}",
        source_session_id,
        cwd,
        claude_bin,
        remote_host.map(|r| &r.name)
    );

    // Build CLI args (shared between local and remote)
    let flag_args = adapter::build_settings_args(settings, false);
    let mut claude_args: Vec<String> = vec![
        "--resume".into(),
        source_session_id.into(),
        "--fork-session".into(),
        "-p".into(),
        "(fork checkpoint)".into(),
        "--output-format".into(),
        "json".into(),
        "--max-turns".into(),
        "1".into(),
    ];
    claude_args.extend(flag_args.iter().cloned());

    let mut cmd = if let Some(remote) = remote_host {
        // SSH branch: wrap claude command in ssh
        let remote_cmd = super::ssh::build_remote_claude_command(
            remote,
            cwd,
            &claude_args,
            api_key,
            auth_token,
            base_url,
            default_model,
            extra_env,
        );
        let mut ssh_cmd = super::ssh::build_ssh_command(remote, &remote_cmd);
        ssh_cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        log::debug!(
            "[fork_oneshot] spawning remote fork process via SSH, flags={:?}",
            flag_args
        );
        ssh_cmd
    } else {
        // Local branch: existing logic
        let mut local_cmd = Command::new(&claude_bin);
        for arg in &claude_args {
            local_cmd.arg(arg);
        }
        let path_env = augmented_path();
        local_cmd
            .current_dir(cwd)
            .env("PATH", &path_env)
            .env_remove("CLAUDECODE")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // Inject auth environment variables (mutually exclusive — remove the other to
        // prevent inherited shell env vars from interfering).
        // Use env_remove (not empty string) — CLI may treat empty as "set but invalid".
        if let Some(key) = api_key {
            local_cmd.env("ANTHROPIC_API_KEY", key);
            local_cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        }
        if let Some(token) = auth_token {
            local_cmd.env("ANTHROPIC_AUTH_TOKEN", token);
            local_cmd.env_remove("ANTHROPIC_API_KEY");
        }
        if let Some(url) = base_url {
            local_cmd.env("ANTHROPIC_BASE_URL", url);
        }
        // Inject default model for third-party platforms
        if let Some(model) = default_model {
            local_cmd.env("ANTHROPIC_MODEL", model);
            local_cmd.env("ANTHROPIC_DEFAULT_HAIKU_MODEL", model);
            local_cmd.env("ANTHROPIC_DEFAULT_SONNET_MODEL", model);
            local_cmd.env("ANTHROPIC_DEFAULT_OPUS_MODEL", model);
        }
        // Inject extra env vars for third-party platforms
        if let Some(extra) = extra_env {
            for (k, v) in extra {
                local_cmd.env(k, v);
            }
        }
        log::debug!(
            "[fork_oneshot] spawning local fork process, flags={:?}",
            flag_args
        );
        local_cmd
    };

    let output = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| "fork_oneshot timed out after 60s".to_string())?
        .map_err(|e| format!("fork_oneshot spawn failed: {}", e))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    log::debug!(
        "[fork_oneshot] exit={:?}, stdout_len={}, stderr_len={}",
        output.status.code(),
        stdout_str.len(),
        stderr_str.len(),
    );
    if !stderr_str.is_empty() {
        log::trace!(
            "[fork_oneshot] stderr: {}",
            &stderr_str[..stderr_str.len().min(500)]
        );
    }

    if !output.status.success() {
        return Err(format!(
            "fork_oneshot failed (exit {:?}): {}",
            output.status.code(),
            stderr_str.chars().take(500).collect::<String>(),
        ));
    }

    // Parse JSON result — extract session_id.
    let parsed: Value = serde_json::from_str(stdout_str.trim()).map_err(|e| {
        format!(
            "fork_oneshot: failed to parse JSON: {} (stdout: {})",
            e,
            &stdout_str[..stdout_str.len().min(300)]
        )
    })?;

    let result_obj = if let Some(arr) = parsed.as_array() {
        log::debug!(
            "[fork_oneshot] response is JSON array with {} elements",
            arr.len()
        );
        arr.iter()
            .rev()
            .find(|el| {
                el.get("type").and_then(|v| v.as_str()) == Some("result")
                    || el.get("session_id").is_some()
            })
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        parsed
    };

    if result_obj
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let err_msg = result_obj
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("fork_oneshot: CLI error: {}", err_msg));
    }

    let new_session_id = result_obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "fork_oneshot: no session_id in response: {}",
                &stdout_str[..stdout_str.len().min(300)]
            )
        })?
        .to_string();

    log::debug!("[fork_oneshot] success: new_session_id={}", new_session_id);
    Ok(new_session_id)
}

/// Shared cache for the resolved claude binary path.
static CLAUDE_PATH_CACHE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Resolve the full path to the claude binary.
/// Cached after first resolution. Use `invalidate_claude_path_cache()` to clear
/// (e.g. after installing the CLI) so the next call re-scans.
pub(crate) fn resolve_claude_path() -> String {
    let mut cached = CLAUDE_PATH_CACHE.lock().unwrap();
    if let Some(ref path) = *cached {
        return path.clone();
    }
    let home = crate::storage::home_dir().unwrap_or_default();
    let candidates = [
        format!("{}/.local/bin/claude", home),
        "/usr/local/bin/claude".to_string(),
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            log::debug!("[claude_stream] resolved claude binary (cached): {}", c);
            *cached = Some(c.clone());
            return c.clone();
        }
    }
    log::debug!(
        "[claude_stream] claude binary not found in candidates, falling back to PATH lookup"
    );
    let fallback = "claude".to_string();
    *cached = Some(fallback.clone());
    fallback
}

/// Clear the cached claude binary path so the next `resolve_claude_path()` re-scans.
pub fn invalidate_claude_path_cache() {
    *CLAUDE_PATH_CACHE.lock().unwrap() = None;
    log::debug!("[claude_stream] claude path cache invalidated");
}

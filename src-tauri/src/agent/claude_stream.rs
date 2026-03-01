//! Claude CLI utility functions.
//!
//! Process spawning and event streaming are handled by `session_actor.rs`.
//! This module provides shared utilities: binary resolution, PATH augmentation,
//! and one-shot fork execution.

use crate::agent::adapter;
use crate::models::RemoteHost;
use serde_json::Value;
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::Duration;

/// Resolve an nvm alias recursively (e.g., default → lts/jod → 22).
/// Returns the terminal version string (e.g., "22", "v22.22.0") or None.
/// Handles chains like: default → lts/jod → 22, or default → node (unresolvable → None).
#[cfg(not(windows))]
fn resolve_nvm_alias(home: &std::path::Path, alias_name: &str, max_depth: u8) -> Option<String> {
    if max_depth == 0 {
        return None;
    }
    let alias_path = home.join(".nvm").join("alias").join(alias_name);
    let content = std::fs::read_to_string(&alias_path).ok()?;
    let content = content.trim().to_string();
    if content.is_empty() {
        return None;
    }
    // If it looks like a version number (starts with digit or 'v'), return it
    let first = content.chars().next()?;
    if first.is_ascii_digit() || first == 'v' {
        return Some(content);
    }
    // Otherwise it's another alias name (e.g., "lts/jod"), resolve recursively
    resolve_nvm_alias(home, &content, max_depth - 1)
}

/// Collect extra directories to prepend to PATH (platform-specific).
/// Returns empty dirs when home is unavailable to avoid relative-path mis-hits.
fn extra_path_dirs() -> Vec<PathBuf> {
    let home = match crate::storage::home_dir() {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => {
            log::debug!("[claude_stream] home_dir unavailable, skipping home-based PATH dirs");
            #[cfg(not(windows))]
            return vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ];
            #[cfg(windows)]
            return {
                let mut dirs = Vec::new();
                if let Ok(d) = std::env::var("APPDATA") {
                    if !d.is_empty() {
                        dirs.push(PathBuf::from(&d).join("npm"));
                    }
                }
                if let Ok(d) = std::env::var("LOCALAPPDATA") {
                    if !d.is_empty() {
                        dirs.push(PathBuf::from(&d).join("npm"));
                    }
                }
                dirs
            };
        }
    };

    #[cfg(windows)]
    {
        let mut dirs = Vec::new();
        if let Ok(d) = std::env::var("APPDATA") {
            if !d.is_empty() {
                dirs.push(PathBuf::from(&d).join("npm"));
            }
        }
        if let Ok(d) = std::env::var("LOCALAPPDATA") {
            if !d.is_empty() {
                dirs.push(PathBuf::from(&d).join("npm"));
            }
        }
        dirs.extend([
            home.join(".npm-global").join("bin"),
            home.join(".claude").join("bin"),
            home.join(".local").join("bin"),
            home.join(".cargo").join("bin"),
            home.join(".nvm").join("current").join("bin"),
            home.join(".volta").join("bin"),
            home.join(".fnm").join("current").join("bin"),
        ]);
        dirs
    }
    #[cfg(not(windows))]
    {
        let nvm_dir = home.join(".nvm").join("versions").join("node");

        let mut dirs = vec![
            home.join(".local").join("bin"),
            home.join(".cargo").join("bin"),
        ];

        // nvm: prefer the default alias, then fall back to highest version
        let mut nvm_resolved = false;
        if let Some(ver) = resolve_nvm_alias(&home, "default", 5) {
            log::debug!("[path] nvm alias 'default' resolved to: {ver}");
            if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name
                        .trim_start_matches('v')
                        .starts_with(ver.trim_start_matches('v'))
                    {
                        let bin = entry.path().join("bin");
                        if bin.exists() {
                            log::debug!("[path] nvm: using alias-resolved path: {}", bin.display());
                            dirs.push(bin);
                            nvm_resolved = true;
                            break;
                        }
                    }
                }
            }
        } else {
            log::debug!("[path] nvm alias 'default' could not be resolved");
        }
        if !nvm_resolved {
            if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
                let mut version_dirs: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.path().join("bin").exists())
                    .collect();
                version_dirs.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
                if let Some(entry) = version_dirs.first() {
                    dirs.push(entry.path().join("bin"));
                }
            }
        }

        dirs.extend([
            home.join(".volta").join("bin"),
            home.join(".fnm").join("current").join("bin"),
            home.join(".local").join("share").join("mise").join("shims"),
            home.join(".asdf").join("shims"),
            // Linuxbrew paths (user-local and system-wide)
            home.join(".linuxbrew").join("bin"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);

        dirs
    }
}

/// Build a PATH that includes common binary locations (cross-platform).
pub fn augmented_path() -> String {
    let extra = extra_path_dirs();
    let current_path = std::env::var("PATH").unwrap_or_default();
    let existing: Vec<PathBuf> = std::env::split_paths(&current_path).collect();

    #[cfg(windows)]
    let eq = |a: &PathBuf, b: &PathBuf| {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    };
    #[cfg(not(windows))]
    let eq = |a: &PathBuf, b: &PathBuf| a == b;

    let mut parts: Vec<PathBuf> = Vec::new();
    for dir in extra {
        if dir.is_dir()
            && !parts.iter().any(|p| eq(p, &dir))
            && !existing.iter().any(|e| eq(e, &dir))
        {
            parts.push(dir);
        }
    }
    parts.extend(existing);

    std::env::join_paths(&parts)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(current_path)
}

/// Cross-platform binary lookup.
/// - Windows: uses `where` command.
/// - Unix: pure Rust PATH traversal (avoids dependency on `which` binary,
///   which is not pre-installed on all Linux distros).
pub fn which_binary(name: &str) -> Option<String> {
    let result = which_binary_inner(name);
    match &result {
        Some(path) => log::debug!("[path] which_binary({name}) → {path}"),
        None => log::warn!("[path] which_binary({name}) → not found"),
    }
    result
}

fn which_binary_inner(name: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("where")
            .arg(name)
            .env("PATH", augmented_path())
            .output()
            .ok()?;
        if output.status.success() {
            let out = String::from_utf8_lossy(&output.stdout);
            out.lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .map(|l| l.to_string())
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path_str = augmented_path();
        log::debug!("[path] searching PATH for '{name}': {path_str}");
        let path_os = std::ffi::OsString::from(&path_str);
        for dir in std::env::split_paths(&path_os) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                if let Ok(meta) = std::fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate.to_string_lossy().into_owned());
                    }
                }
            }
        }
        None
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
    let home = crate::storage::home_dir()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);

    #[cfg(windows)]
    let candidates = {
        let mut bases = Vec::new();
        if let Ok(d) = std::env::var("APPDATA") {
            if !d.is_empty() {
                bases.push(PathBuf::from(&d).join("npm"));
            }
        }
        if let Ok(d) = std::env::var("LOCALAPPDATA") {
            if !d.is_empty() {
                bases.push(PathBuf::from(&d).join("npm"));
            }
        }
        if let Some(ref h) = home {
            bases.push(h.join(".claude").join("bin"));
            bases.push(h.join(".local").join("bin"));
        }
        let names = ["claude.cmd", "claude.exe", "claude.bat", "claude"];
        let mut cands = Vec::new();
        for base in &bases {
            for name in &names {
                cands.push(base.join(name));
            }
        }
        cands
    };
    #[cfg(not(windows))]
    let candidates = {
        let mut cands = Vec::new();
        if let Some(ref h) = home {
            cands.push(h.join(".local").join("bin").join("claude"));
        }
        cands.push(PathBuf::from("/usr/local/bin/claude"));
        cands
    };

    for c in &candidates {
        if c.exists() {
            let path_str = c.to_string_lossy().to_string();
            log::debug!(
                "[claude_stream] resolved claude binary (cached): {}",
                path_str
            );
            *cached = Some(path_str.clone());
            return path_str;
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

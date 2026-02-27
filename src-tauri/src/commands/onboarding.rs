use crate::agent::claude_stream;
use crate::models::{AuthCheckResult, AuthOverview, InstallMethod};
use crate::storage;
use tauri::{AppHandle, Emitter};
use tokio::process::Command;

/// Check whether the user has an active OAuth session or API key configured.
#[tauri::command]
pub async fn check_auth_status() -> Result<AuthCheckResult, String> {
    log::debug!("[onboarding] check_auth_status");

    // Check API key from app settings + CLI sources (settings.json, env vars, shell configs)
    let user_settings = storage::settings::get_user_settings();
    let has_app_key = user_settings
        .anthropic_api_key
        .as_ref()
        .is_some_and(|k| !k.is_empty());
    let cli_config = storage::cli_config::load_cli_config();
    let (cli_key, cli_key_source) = detect_cli_api_key(&cli_config);
    let has_api_key = has_app_key || cli_key.is_some();

    // Check OAuth via shared helper
    let (has_oauth, oauth_account) = check_cli_oauth().await;

    log::debug!(
        "[onboarding] auth check result: has_oauth={}, has_api_key={} (app={}, cli={:?}), account={:?}",
        has_oauth,
        has_api_key,
        has_app_key,
        cli_key_source,
        oauth_account
    );

    Ok(AuthCheckResult {
        has_oauth,
        has_api_key,
        oauth_account,
    })
}

/// Detect which CLI installation methods are available on this system.
#[tauri::command]
pub async fn detect_install_methods() -> Result<Vec<InstallMethod>, String> {
    log::debug!("[onboarding] detect_install_methods");

    let mut methods = Vec::new();

    // 1. Homebrew — macOS/Linux (recommended when available)
    let has_brew = which_binary("brew").await;
    methods.push(InstallMethod {
        id: "brew".into(),
        name: "Homebrew".into(),
        command: "brew install claude-code".into(),
        available: has_brew,
        unavailable_reason: if has_brew {
            None
        } else {
            Some("Homebrew not installed".into())
        },
    });

    // 2. npm — requires Node.js 18+
    let has_npm = check_npm_available().await;
    methods.push(InstallMethod {
        id: "npm".into(),
        name: "npm (Node.js)".into(),
        command: "npm install -g @anthropic-ai/claude-code".into(),
        available: has_npm,
        unavailable_reason: if has_npm {
            None
        } else {
            Some("Requires Node.js 18+".into())
        },
    });

    // 3. Native install (curl script) — nearly all Unix systems have curl
    let has_curl = which_binary("curl").await;
    methods.push(InstallMethod {
        id: "native".into(),
        name: "Native Install (curl)".into(),
        command: "curl -fsSL https://claude.ai/install.sh | bash".into(),
        available: has_curl,
        unavailable_reason: if has_curl {
            None
        } else {
            Some("curl not found".into())
        },
    });

    log::debug!(
        "[onboarding] install methods: {:?}",
        methods
            .iter()
            .map(|m| format!("{}={}", m.id, m.available))
            .collect::<Vec<_>>()
    );
    Ok(methods)
}

/// Run `claude login` to start the OAuth flow. The CLI opens a browser automatically.
#[tauri::command]
pub async fn run_claude_login(app: AppHandle) -> Result<bool, String> {
    log::debug!("[onboarding] run_claude_login");

    let claude_bin = claude_stream::resolve_claude_path();
    let path_env = claude_stream::augmented_path();

    let mut child = Command::new(&claude_bin)
        .arg("login")
        .env("PATH", &path_env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude login: {}", e))?;

    if let Some(stdout) = child.stdout.take() {
        let app_clone = app.clone();
        tokio::spawn(async move { stream_pipe_to_events(stdout, app_clone, "login stdout").await });
    }
    if let Some(stderr) = child.stderr.take() {
        let app_clone = app.clone();
        tokio::spawn(async move { stream_pipe_to_events(stderr, app_clone, "login stderr").await });
    }

    // Wait for exit (3 min timeout — user needs to complete browser auth)
    let status = tokio::time::timeout(std::time::Duration::from_secs(180), child.wait())
        .await
        .map_err(|_| "Login timed out after 3 minutes".to_string())?
        .map_err(|e| format!("Login process error: {}", e))?;

    let success = status.success();
    log::debug!(
        "[onboarding] run_claude_login: exit={:?}, success={}",
        status.code(),
        success
    );

    Ok(success)
}

/// Get an overview of all authentication sources (configuration state only).
#[tauri::command]
pub async fn get_auth_overview() -> Result<AuthOverview, String> {
    log::debug!("[onboarding] get_auth_overview");

    // 1. Read user settings → auth_mode, platform_credentials, active_platform_id
    let user_settings = storage::settings::get_user_settings();
    let auth_mode = user_settings.auth_mode.clone();

    // 2. CLI OAuth login — not detectable without subprocess, default to false.
    //    (Onboarding wizard uses check_cli_oauth() subprocess for accurate status.)
    let cli_login_available = false;
    let cli_login_account: Option<String> = None;

    // 3. Check CLI API Key from multiple sources (first non-empty wins):
    //    a) ~/.claude/settings.json "apiKey"
    //    b) ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN process env var
    //    c) Same vars in shell config files (.zshrc, .bashrc, etc.)
    let cli_config = storage::cli_config::load_cli_config();
    let (cli_api_key_str, cli_api_key_source) = detect_cli_api_key(&cli_config);
    let cli_has_api_key = cli_api_key_str.is_some();
    let cli_api_key_hint = cli_api_key_str.as_ref().map(|k| {
        let suffix: String = k
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("...{}", suffix)
    });

    // 4. Check App platform credentials
    let active_pid = user_settings.active_platform_id.clone();
    let app_has_credentials = active_pid.as_ref().is_some_and(|pid| {
        user_settings
            .platform_credentials
            .iter()
            .any(|c| &c.platform_id == pid && c.api_key.as_ref().is_some_and(|k| !k.is_empty()))
    });

    // Platform name: use credential name, fallback to preset name, fallback to pid
    let app_platform_name = active_pid.as_ref().map(|pid| {
        // Try credential name first
        let cred_name = user_settings
            .platform_credentials
            .iter()
            .find(|c| &c.platform_id == pid)
            .and_then(|c| c.name.clone());
        if let Some(name) = cred_name {
            if !name.is_empty() {
                return name;
            }
        }
        // Fallback to preset name
        preset_name(pid)
    });

    log::debug!(
        "[onboarding] auth overview: mode={}, cli_login={}, cli_key={} (source={:?}), app_cred={}",
        auth_mode,
        cli_login_available,
        cli_has_api_key,
        cli_api_key_source,
        app_has_credentials
    );

    Ok(AuthOverview {
        auth_mode,
        cli_login_available,
        cli_login_account,
        cli_has_api_key,
        cli_api_key_hint,
        cli_api_key_source,
        app_has_credentials,
        app_platform_id: active_pid,
        app_platform_name,
    })
}

/// Set API key in CLI config (~/.claude/settings.json).
#[tauri::command]
pub async fn set_cli_api_key(key: String) -> Result<(), String> {
    log::debug!("[onboarding] set_cli_api_key");
    let trimmed = key.trim().to_string();
    if trimmed.is_empty() {
        return Err("API key cannot be empty".to_string());
    }
    storage::cli_config::update_cli_config(serde_json::json!({ "apiKey": trimmed }))?;
    Ok(())
}

/// Remove API key from CLI config (~/.claude/settings.json).
#[tauri::command]
pub async fn remove_cli_api_key() -> Result<(), String> {
    log::debug!("[onboarding] remove_cli_api_key");
    storage::cli_config::update_cli_config(serde_json::json!({ "apiKey": null }))?;
    Ok(())
}

// ── Helpers ──

/// Env var names that Claude CLI recognizes for API key authentication.
const CLI_KEY_ENV_VARS: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

/// Detect CLI API key from settings file, process env vars, and shell config files.
/// Returns (key_value, source_label).
pub(crate) fn detect_cli_api_key(
    cli_config: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    // a) ~/.claude/settings.json "apiKey"
    let settings_key = cli_config
        .get("apiKey")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());
    if let Some(k) = settings_key {
        return (Some(k), Some("settings".to_string()));
    }

    // b) Process env vars
    for var in CLI_KEY_ENV_VARS {
        if let Ok(val) = std::env::var(var) {
            if !val.trim().is_empty() {
                return (Some(val), Some("env".to_string()));
            }
        }
    }

    // c) Shell config files
    for var in CLI_KEY_ENV_VARS {
        if let Some((val, path)) = read_env_from_shell_config(var) {
            return (Some(val), Some(format!("shell_config:{}", path)));
        }
    }

    (None, None)
}

/// Parse shell config files to find `export VAR_NAME=value`.
/// Handles: `export VAR=val`, `export VAR="val"`, `export VAR='val'`.
/// Skips commented lines. Returns (value, file_path) of the first match.
fn read_env_from_shell_config(var_name: &str) -> Option<(String, String)> {
    let home = std::env::var("HOME").ok()?;
    let config_files = [
        format!("{}/.zshrc", home),
        format!("{}/.zprofile", home),
        format!("{}/.bashrc", home),
        format!("{}/.bash_profile", home),
        format!("{}/.profile", home),
    ];
    let pattern = format!("{}=", var_name);
    for path in &config_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            // Match "export VAR_NAME=..." or "VAR_NAME=..."
            let after_export = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            if let Some(rest) = after_export.strip_prefix(&pattern) {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    log::debug!("[onboarding] found {} in shell config: {}", var_name, path);
                    return Some((val.to_string(), path.clone()));
                }
            }
        }
    }
    None
}

/// Check CLI OAuth status via subprocess. Used by onboarding wizard (slower but gets account email).
pub(crate) async fn check_cli_oauth() -> (bool, Option<String>) {
    let claude_bin = claude_stream::resolve_claude_path();
    if claude_bin != "claude" || which_binary("claude").await {
        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Command::new(&claude_bin)
                .arg("auth")
                .arg("status")
                .env("PATH", claude_stream::augmented_path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output(),
        )
        .await
        {
            Ok(Ok(output)) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let account = stdout
                    .lines()
                    .find(|l| l.contains('@'))
                    .map(|l| l.trim().to_string());
                (true, account)
            }
            _ => (false, None),
        }
    } else {
        (false, None)
    }
}

/// Get display name for a platform preset ID.
fn preset_name(pid: &str) -> String {
    // Known preset names (mirrors frontend PLATFORM_PRESETS)
    match pid {
        "anthropic" => "Anthropic",
        "deepseek" => "DeepSeek",
        "kimi" => "Kimi (Moonshot)",
        "kimi-coding" => "Kimi For Coding",
        "zhipu" => "Zhipu (智谱)",
        "bailian" => "Bailian (百炼)",
        "doubao" => "DouBao (豆包)",
        "minimax" => "MiniMax",
        "mimo" => "Xiaomi MiMo (小米)",
        "vercel" => "Vercel AI Gateway",
        "openrouter" => "OpenRouter",
        "siliconflow" => "SiliconFlow (硅基流动)",
        "modelscope" => "ModelScope (魔搭)",
        "aihubmix" => "AiHubMix",
        "ollama" => "Ollama",
        "custom" => "Custom",
        _ => return pid.to_string(),
    }
    .to_string()
}

/// Stream a pipe (stdout or stderr) to the frontend via setup-progress events.
/// Reads in chunks and splits on both `\r` and `\n`:
///   - `\n`-terminated lines → `setup-progress` event (frontend appends)
///   - `\r`-terminated segments → `setup-progress-replace` event (frontend replaces last line)
///     Throttled to 300ms to avoid flooding with rapid progress bar updates.
async fn stream_pipe_to_events(
    pipe: impl tokio::io::AsyncRead + Unpin,
    app: AppHandle,
    label: &'static str,
) {
    use tokio::io::AsyncReadExt;

    let mut reader = tokio::io::BufReader::new(pipe);
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    let mut last_replace = std::time::Instant::now();
    let throttle = std::time::Duration::from_millis(300);

    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };

        pending.push_str(&String::from_utf8_lossy(&buf[..n]));

        // Process complete segments (terminated by \r or \n)
        loop {
            let cr_pos = pending.find('\r');
            let lf_pos = pending.find('\n');

            let pos = match (cr_pos, lf_pos) {
                (Some(cr), Some(lf)) => cr.min(lf),
                (Some(cr), None) => cr,
                (None, Some(lf)) => lf,
                (None, None) => break,
            };

            let segment = pending[..pos].to_string();
            let is_cr = pending.as_bytes()[pos] == b'\r';
            pending = pending[pos + 1..].to_string();

            if segment.trim().is_empty() {
                continue;
            }

            if let Some(cleaned) = sanitize_progress_line(&segment) {
                if is_cr {
                    // \r = progress bar update → replace last line (throttled)
                    if last_replace.elapsed() >= throttle {
                        log::trace!("[onboarding] {} (replace): {}", label, cleaned);
                        let _ = app.emit("setup-progress-replace", &cleaned);
                        last_replace = std::time::Instant::now();
                    }
                } else {
                    // \n = new line → append
                    log::trace!("[onboarding] {} (append): {}", label, cleaned);
                    let _ = app.emit("setup-progress", &cleaned);
                }
            }
        }
    }

    // Emit any remaining content
    let remaining = pending.trim().to_string();
    if !remaining.is_empty() {
        if let Some(cleaned) = sanitize_progress_line(&remaining) {
            log::trace!("[onboarding] {} (final): {}", label, cleaned);
            let _ = app.emit("setup-progress", &cleaned);
        }
    }
}

/// Sanitize a progress line by handling ANSI cursor movement and stripping
/// escape sequences.  Cursor-forward (`\x1b[nC`) is replaced with *n* spaces
/// so that "Checking\x1b[1Cinstallation" becomes "Checking installation".
/// All other CSI / private-mode sequences are silently dropped.
/// Returns `None` when the sanitized result is empty (pure control line).
fn sanitize_progress_line(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI sequence: \x1b[ [?] [digits;]* <letter>
            i += 2; // skip \x1b[

            let is_private = i < bytes.len() && bytes[i] == b'?';
            if is_private {
                i += 1;
            }

            // Parse numeric parameter (only last segment matters for CUF)
            let mut num = 0u32;
            let mut has_num = false;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
                if bytes[i].is_ascii_digit() {
                    num = num * 10 + (bytes[i] - b'0') as u32;
                    has_num = true;
                } else {
                    num = 0;
                    has_num = false;
                }
                i += 1;
            }
            if !has_num {
                num = 1;
            }

            if i < bytes.len() {
                if !is_private && bytes[i] == b'C' {
                    // CUF — Cursor Forward: replace with spaces
                    result.extend(std::iter::repeat_n(b' ', num.min(20) as usize));
                }
                // All other sequences are dropped
                i += 1;
            }
        } else if bytes[i] == 0x1b {
            // Non-CSI escape (e.g. \x1bM) — skip 2 bytes
            i += 2;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    let cleaned = String::from_utf8_lossy(&result).trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Check if a binary is available on PATH.
async fn which_binary(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .env("PATH", claude_stream::augmented_path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if npm is available and Node.js version >= 18.
async fn check_npm_available() -> bool {
    if !which_binary("npm").await {
        return false;
    }
    // Check node version
    let output = Command::new("node")
        .arg("--version")
        .env("PATH", claude_stream::augmented_path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let version = String::from_utf8_lossy(&o.stdout);
            // Parse "v22.22.0" → 22 >= 18
            let major = version
                .trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            major >= 18
        }
        _ => false,
    }
}

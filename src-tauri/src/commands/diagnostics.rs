use crate::agent::claude_stream::augmented_path;
use crate::agent::ssh::{expand_local_tilde, shell_escape};
use crate::models::{
    ApiTestResult, AuthDiagnostics, ClaudeMdInfo, CliCheckResult, CliDiagnostics, CliDistTags,
    ConfigDiagnostics, ConfigIssue, DiagnosticsReport, LocalProxyStatus, ProjectDiagnostics,
    ProjectInitStatus, RemoteTestResult, ServicesDiagnostics, SshKeyInfo, SystemDiagnostics,
};
use crate::process_ext::HideConsole;
use std::path::Path;
use std::process::Command;

#[tauri::command]
pub async fn check_agent_cli(agent: String) -> Result<CliCheckResult, String> {
    let binary = match agent.as_str() {
        "claude" => "claude",
        "codex" => "codex",
        _ => return Err(format!("Unknown agent: {}", agent)),
    };

    log::debug!("[diagnostics] check_agent_cli: agent={}", agent);
    let aug_path = augmented_path();

    // Check if binary exists (cross-platform: uses `where` on Windows, `which` on Unix)
    let (found, path) = match crate::agent::claude_stream::which_binary(binary) {
        Some(p) => (true, Some(p)),
        None => (false, None),
    };

    // Get version if found
    let version = if found {
        let ver_output = Command::new(binary)
            .arg("--version")
            .env("PATH", &aug_path)
            .hide_console()
            .output();
        match ver_output {
            Ok(output) if output.status.success() => {
                let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
                // Strip trailing suffix like " (Claude Code)" to get bare semver
                Some(raw.find(" (").map(|i| raw[..i].to_string()).unwrap_or(raw))
            }
            _ => None,
        }
    } else {
        None
    };

    log::debug!(
        "[diagnostics] check_agent_cli result: agent={}, found={}, path={:?}",
        agent,
        found,
        path
    );
    Ok(CliCheckResult {
        agent,
        found,
        path,
        version,
    })
}

// ── Local proxy detection ──

async fn detect_proxy_inner(proxy_id: &str, base_url: &str) -> LocalProxyStatus {
    log::debug!(
        "[diagnostics] detect_local_proxy: proxy_id={}, base_url={}",
        proxy_id,
        base_url
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy() // Local services must be reached directly, never via system proxy
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::debug!(
                "[diagnostics] detect_local_proxy: client build failed: {}",
                e
            );
            return LocalProxyStatus {
                proxy_id: proxy_id.to_string(),
                running: false,
                needs_auth: false,
                base_url: base_url.to_string(),
                error: Some(format!("HTTP client build failed: {}", e)),
            };
        }
    };
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            // Any HTTP response = service is running (connection succeeded).
            // 401/403 = running but needs auth. All others = running normally.
            let needs_auth = status == 401 || status == 403;
            log::debug!(
                "[diagnostics] detect_local_proxy result: proxy_id={}, running=true, status={}, needs_auth={}",
                proxy_id,
                status,
                needs_auth
            );
            LocalProxyStatus {
                proxy_id: proxy_id.to_string(),
                running: true,
                needs_auth,
                base_url: base_url.to_string(),
                error: None,
            }
        }
        Err(e) => {
            log::debug!(
                "[diagnostics] detect_local_proxy result: proxy_id={}, running=false, err={}",
                proxy_id,
                e
            );
            LocalProxyStatus {
                proxy_id: proxy_id.to_string(),
                running: false,
                needs_auth: false,
                base_url: base_url.to_string(),
                error: Some(e.to_string()),
            }
        }
    }
}

#[tauri::command]
pub async fn detect_local_proxy(
    proxy_id: String,
    base_url: String,
) -> Result<LocalProxyStatus, String> {
    Ok(detect_proxy_inner(&proxy_id, &base_url).await)
}

// ── API connectivity test ──

/// Probe model used when the user hasn't configured one — just for connectivity testing.
const PROBE_MODEL: &str = "claude-sonnet-4-6";

async fn test_api_inner(
    api_key: &str,
    base_url: &str,
    auth_env_var: &str,
    model: &str,
) -> ApiTestResult {
    let is_probe = model.is_empty();
    let effective_model = if is_probe { PROBE_MODEL } else { model };
    let effective_base_url = if base_url.is_empty() {
        "https://api.anthropic.com"
    } else {
        base_url
    };
    let url = format!("{}/v1/messages", effective_base_url.trim_end_matches('/'));

    log::debug!(
        "[diagnostics] test_api_connectivity: url={}, auth={}, model={}, probe={}",
        url,
        auth_env_var,
        effective_model,
        is_probe
    );
    log::debug!(
        "[diagnostics] test_api_connectivity: key_len={}",
        api_key.len()
    );

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::debug!(
                "[diagnostics] test_api_connectivity: client build failed: {}",
                e
            );
            return ApiTestResult {
                success: false,
                latency_ms: 0,
                reply: None,
                error: Some(format!("HTTP client build failed: {}", e)),
                partial: false,
            };
        }
    };

    let mut req = client
        .post(&url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01");

    req = match auth_env_var {
        "ANTHROPIC_AUTH_TOKEN" => req.header("authorization", format!("Bearer {}", api_key)),
        _ => req.header("x-api-key", api_key),
    };

    let body = serde_json::json!({
        "model": effective_model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}]
    });

    let start = std::time::Instant::now();
    let resp = match req.json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let error = if e.is_timeout() {
                "Connection timed out".to_string()
            } else if e.is_connect() {
                "Connection refused — is the service running?".to_string()
            } else {
                format!("Request failed: {}", e)
            };
            log::debug!(
                "[diagnostics] test_api_connectivity: failed, error={}",
                error
            );
            return ApiTestResult {
                success: false,
                latency_ms,
                reply: None,
                error: Some(error),
                partial: false,
            };
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    let status = resp.status().as_u16();

    if status == 200 {
        // Parse successful response — must contain content[0].text to count as valid
        let body_text = resp.text().await.unwrap_or_default();
        let reply = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| {
                v.get("content")?
                    .get(0)?
                    .get("text")?
                    .as_str()
                    .map(String::from)
            })
            .map(|s| {
                if s.len() > 50 {
                    format!("{}…", &s[..50])
                } else {
                    s
                }
            });

        if reply.is_some() {
            log::debug!(
                "[diagnostics] test_api_connectivity: success, latency={}ms",
                latency_ms
            );
            ApiTestResult {
                success: true,
                latency_ms,
                reply,
                error: None,
                partial: false,
            }
        } else {
            log::debug!("[diagnostics] test_api_connectivity: 200 but invalid response body");
            ApiTestResult {
                success: false,
                latency_ms,
                reply: None,
                error: Some(
                    "Received 200 but response is not a valid Messages API reply".to_string(),
                ),
                partial: false,
            }
        }
    } else {
        // Parse error response
        let body_text = resp.text().await.unwrap_or_default();
        let api_error = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| v.get("error")?.get("message")?.as_str().map(String::from));

        // Probe mode: 400 with model-related error means auth+connectivity OK, just wrong model
        let is_model_error = status == 400
            && api_error
                .as_deref()
                .map(|m| {
                    let lower = m.to_lowercase();
                    lower.contains("model") || lower.contains("not found")
                })
                .unwrap_or(false);

        if is_probe && is_model_error {
            log::debug!(
                "[diagnostics] test_api_connectivity: partial success (probe model rejected), latency={}ms",
                latency_ms
            );
            return ApiTestResult {
                success: true,
                latency_ms,
                reply: None,
                error: None,
                partial: true,
            };
        }

        let error = match status {
            401 | 403 => "Authentication failed — check your API key".to_string(),
            404 => "Endpoint not found — check your base URL".to_string(),
            429 => "Rate limited — try again later".to_string(),
            _ => {
                if let Some(msg) = api_error {
                    format!("HTTP {}: {}", status, msg)
                } else {
                    format!("HTTP {}", status)
                }
            }
        };

        log::debug!(
            "[diagnostics] test_api_connectivity: failed, status={}, error={}",
            status,
            error
        );
        ApiTestResult {
            success: false,
            latency_ms,
            reply: None,
            error: Some(error),
            partial: false,
        }
    }
}

#[tauri::command]
pub async fn test_api_connectivity(
    api_key: String,
    base_url: String,
    auth_env_var: String,
    model: String,
) -> Result<ApiTestResult, String> {
    Ok(test_api_inner(&api_key, &base_url, &auth_env_var, &model).await)
}

/// Platform-aware message for missing SSH binaries.
fn ssh_not_found_msg(binary: &str) -> String {
    #[cfg(windows)]
    {
        format!(
            "{} not found. Install OpenSSH: Settings → Apps → Optional Features → OpenSSH Client.",
            binary
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "{} not found. Please install OpenSSH (e.g. apt install openssh-client / brew install openssh).",
            binary
        )
    }
}

/// Test SSH connectivity and Claude CLI availability on a remote host.
/// Uses async tokio::process::Command with timeout (audit #8).
#[tauri::command]
pub async fn test_remote_host(
    host: String,
    user: String,
    port: Option<u16>,
    key_path: Option<String>,
    remote_claude_path: Option<String>,
) -> Result<RemoteTestResult, String> {
    use tokio::process::Command as TokioCommand;

    if crate::agent::claude_stream::which_binary("ssh").is_none() {
        return Ok(RemoteTestResult {
            ssh_ok: false,
            cli_found: false,
            cli_path: None,
            cli_version: None,
            error: Some(ssh_not_found_msg("ssh")),
        });
    }

    let port = port.unwrap_or(22);
    let target = format!("{}@{}", user, host);
    log::debug!(
        "[diagnostics] test_remote_host: target={}, port={}, key={:?}",
        target,
        port,
        key_path
    );

    // Step 1: SSH connectivity check (15s timeout)
    let mut ssh_cmd = TokioCommand::new("ssh");
    ssh_cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=accept-new",
    ]);
    ssh_cmd.arg("-p").arg(port.to_string());
    if let Some(ref key) = key_path {
        ssh_cmd.args(["-i", &expand_local_tilde(key)]);
    }
    ssh_cmd.arg(&target).arg("echo ok");
    ssh_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .hide_console()
        .kill_on_drop(true);

    let ssh_result =
        tokio::time::timeout(std::time::Duration::from_secs(15), ssh_cmd.output()).await;

    let (ssh_ok, ssh_error) = match ssh_result {
        Ok(Ok(output)) if output.status.success() => (true, None),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (
                false,
                Some(format!(
                    "SSH failed (exit {:?}): {}",
                    output.status.code(),
                    stderr
                )),
            )
        }
        Ok(Err(e)) => (false, Some(format!("SSH spawn failed: {}", e))),
        Err(_) => (false, Some("SSH connection timed out (15s)".into())),
    };

    if !ssh_ok {
        log::debug!(
            "[diagnostics] test_remote_host: SSH failed: {:?}",
            ssh_error
        );
        return Ok(RemoteTestResult {
            ssh_ok: false,
            cli_found: false,
            cli_version: None,
            cli_path: None,
            error: ssh_error,
        });
    }

    // Step 2: CLI check (15s timeout)
    let claude_bin = remote_claude_path.as_deref().unwrap_or("claude");
    let escaped_bin = shell_escape(claude_bin);
    // `command -v` is POSIX-portable (works on Linux, macOS, and most BSDs).
    // `which` is not guaranteed on all systems and behaves inconsistently.
    let check_cmd_str = format!("command -v {} && {} --version", escaped_bin, escaped_bin);

    let mut cli_cmd = TokioCommand::new("ssh");
    cli_cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
    cli_cmd.arg("-p").arg(port.to_string());
    if let Some(ref key) = key_path {
        cli_cmd.args(["-i", &expand_local_tilde(key)]);
    }
    cli_cmd.arg(&target).arg(&check_cmd_str);
    cli_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .hide_console()
        .kill_on_drop(true);

    let cli_result =
        tokio::time::timeout(std::time::Duration::from_secs(15), cli_cmd.output()).await;

    let (cli_found, cli_path, cli_version, cli_error) = match cli_result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let lines: Vec<&str> = stdout.lines().collect();
            let path = lines.first().map(|s| s.to_string());
            let version = lines.get(1).map(|s| s.to_string());
            (true, path, version, None)
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (
                false,
                None,
                None,
                Some(format!("CLI not found: {}", stderr)),
            )
        }
        Ok(Err(e)) => (false, None, None, Some(format!("CLI check failed: {}", e))),
        Err(_) => (false, None, None, Some("CLI check timed out (15s)".into())),
    };

    log::debug!(
        "[diagnostics] test_remote_host result: ssh_ok={}, cli_found={}, path={:?}, version={:?}",
        ssh_ok,
        cli_found,
        cli_path,
        cli_version
    );

    Ok(RemoteTestResult {
        ssh_ok,
        cli_found,
        cli_version,
        cli_path,
        error: cli_error,
    })
}

/// Check if a project directory has been initialized (has CLAUDE.md).
#[tauri::command]
pub fn check_project_init(cwd: String) -> Result<ProjectInitStatus, String> {
    log::debug!("[diagnostics] check_project_init: cwd={}", cwd);
    let root = std::path::Path::new(&cwd);
    if !root.is_dir() {
        return Ok(ProjectInitStatus {
            cwd,
            has_claude_md: false,
        });
    }
    // Canonicalize path (resolve symlinks + normalize case)
    let canonical = std::fs::canonicalize(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.clone());
    let has_claude_md = root.join("CLAUDE.md").is_file();
    log::debug!(
        "[diagnostics] check_project_init: canonical={}, has_claude_md={}",
        canonical,
        has_claude_md
    );
    Ok(ProjectInitStatus {
        cwd: canonical,
        has_claude_md,
    })
}

// ── run_diagnostics: comprehensive system check ──

const ENV_VAR_LIMITS: &[(&str, u64, u64)] = &[
    ("BASH_MAX_OUTPUT_LENGTH", 1, 1_000_000),
    ("TASK_MAX_OUTPUT_LENGTH", 1, 1_000_000),
    ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", 1, 128_000),
];

#[tauri::command]
pub async fn run_diagnostics(cwd: String) -> Result<DiagnosticsReport, String> {
    let has_valid_cwd = !cwd.trim().is_empty() && Path::new(&cwd).is_dir();
    log::debug!(
        "[diagnostics] run_diagnostics: cwd={:?}, has_valid_cwd={}",
        cwd,
        has_valid_cwd
    );

    // Async checks in parallel
    let (cli, dist, auth, community, mcp_reg) = tokio::join!(
        check_cli_inner(),
        fetch_dist_tags_inner(),
        check_auth_inner(),
        check_community_inner(),
        check_mcp_reg_inner(),
    );

    // Merge CLI + dist tags
    let cli = CliDiagnostics {
        latest: dist.0,
        stable: dist.1,
        auto_update_channel: dist.2,
        ..cli
    };

    // Sync checks
    let home = crate::storage::dirs_next()
        .map(|h| h.join(".claude"))
        .unwrap_or_default();
    let settings_issues = validate_config_files_at(&home, &cwd, has_valid_cwd);
    let keybinding_issues = validate_keybindings_at(&home);
    let mcp_issues = validate_mcp_configs_at(&home, &cwd, has_valid_cwd);
    let env_var_issues = check_env_vars();
    let claude_md_files = scan_claude_md_files_at(&home, &cwd, has_valid_cwd);
    let has_claude_md = claude_md_files
        .iter()
        .any(|f| f.path.ends_with("CLAUDE.md"));
    let sandbox = check_sandbox();
    let locks = list_lock_files_at(&home);

    log::debug!(
        "[diagnostics] cli check: found={}, version={:?}",
        cli.found,
        cli.version
    );
    log::debug!(
        "[diagnostics] auth check: oauth={}, api_key={}",
        auth.has_oauth,
        auth.has_api_key
    );
    log::debug!(
        "[diagnostics] config validation: settings_issues={}, mcp_issues={}, keybinding_issues={}, env_issues={}",
        settings_issues.len(),
        mcp_issues.len(),
        keybinding_issues.len(),
        env_var_issues.len()
    );

    Ok(DiagnosticsReport {
        cli,
        auth,
        project: ProjectDiagnostics {
            cwd: cwd.clone(),
            has_claude_md,
            claude_md_files,
            skipped_project_scope: !has_valid_cwd,
        },
        configs: ConfigDiagnostics {
            settings_issues,
            keybinding_issues,
            mcp_issues,
            env_var_issues,
        },
        services: ServicesDiagnostics {
            community_registry: community,
            mcp_registry: mcp_reg,
        },
        system: SystemDiagnostics {
            sandbox_available: sandbox,
            lock_files: locks,
        },
    })
}

// ── Sub-check: CLI ──

async fn check_cli_inner() -> CliDiagnostics {
    let aug_path = augmented_path();
    // Reuse check_agent_cli for CLI detection + version
    let cli = check_agent_cli("claude".into())
        .await
        .unwrap_or(CliCheckResult {
            agent: "claude".into(),
            found: false,
            path: None,
            version: None,
        });

    let ripgrep_available = Command::new("rg")
        .arg("--version")
        .env("PATH", &aug_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .hide_console()
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    CliDiagnostics {
        found: cli.found,
        version: cli.version,
        path: cli.path,
        latest: None,              // filled by caller after dist tags fetch
        stable: None,              // filled by caller
        auto_update_channel: None, // filled by caller
        ripgrep_available,
    }
}

// ── Sub-check: dist tags + auto-update channel ──

async fn fetch_dist_tags_inner() -> (Option<String>, Option<String>, Option<String>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[diagnostics] dist tags: client build failed: {}", e);
            return (None, None, None);
        }
    };

    let resp = client
        .get("https://registry.npmjs.org/-/package/@anthropic-ai/claude-code/dist-tags")
        .header("Accept", "application/json")
        .send()
        .await;

    let (latest, stable) = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("[diagnostics] dist tags: json parse failed: {}", e);
                    return (None, None, None);
                }
            };
            (
                body.get("latest")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                body.get("stable")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            )
        }
        Ok(r) => {
            log::debug!("[diagnostics] dist tags: HTTP {}", r.status());
            (None, None)
        }
        Err(e) => {
            log::warn!("[diagnostics] dist tags fetch failed: {}", e);
            (None, None)
        }
    };

    // Auto-update channel from CLI config
    let cli_config = crate::storage::cli_config::load_cli_config();
    let auto_update_channel = cli_config
        .get("autoUpdatesChannel")
        .and_then(|v| v.as_str())
        .map(String::from);

    (latest, stable, auto_update_channel)
}

// ── Sub-check: Auth ──

async fn check_auth_inner() -> AuthDiagnostics {
    let (has_oauth, oauth_account) = match tokio::time::timeout(
        std::time::Duration::from_secs(12),
        super::onboarding::check_cli_oauth(),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            log::warn!("[diagnostics] oauth check timed out");
            (false, None)
        }
    };

    let cli_config = crate::storage::cli_config::load_cli_config();
    let (api_key, api_key_source) = super::onboarding::detect_cli_api_key(&cli_config);
    let has_api_key = api_key.is_some();
    let api_key_hint = api_key.as_ref().map(|k| {
        if k.len() > 4 {
            format!("...{}", &k[k.len() - 4..])
        } else {
            "***".to_string()
        }
    });

    let user_settings = crate::storage::settings::get_user_settings();
    let app_has_credentials =
        user_settings.anthropic_api_key.is_some() || !user_settings.platform_credentials.is_empty();
    let app_platform_name = user_settings.active_platform_id.clone();

    log::debug!(
        "[diagnostics] auth: oauth={}, api_key={}, app_creds={}",
        has_oauth,
        has_api_key,
        app_has_credentials
    );

    AuthDiagnostics {
        has_oauth,
        oauth_account,
        has_api_key,
        api_key_hint,
        api_key_source,
        app_has_credentials,
        app_platform_name,
    }
}

// ── Sub-check: Community & MCP registry health ──

async fn check_community_inner() -> Option<bool> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::storage::community_skills::health_check(),
    )
    .await
    {
        Ok(health) => Some(health.available),
        Err(_) => {
            log::warn!("[diagnostics] community health check timed out");
            None
        }
    }
}

async fn check_mcp_reg_inner() -> Option<bool> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::storage::mcp_registry::health_check(),
    )
    .await
    {
        Ok(health) => Some(health.available),
        Err(_) => {
            log::warn!("[diagnostics] mcp registry health check timed out");
            None
        }
    }
}

// ── Sub-check: Config file validation ──

fn validate_config_files_at(home: &Path, cwd: &str, has_valid_cwd: bool) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();

    // User scope: ~/.claude/settings.json
    let user_settings_path = home.join("settings.json");
    validate_json_file(&user_settings_path, "user", &mut issues);

    // Project scope: {cwd}/.claude/settings.json
    if has_valid_cwd {
        let project_settings_path = Path::new(cwd).join(".claude").join("settings.json");
        validate_json_file(&project_settings_path, "project", &mut issues);
    }

    issues
}

fn validate_json_file(path: &Path, scope: &str, issues: &mut Vec<ConfigIssue>) {
    match std::fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => {} // Empty file = OK (same as not found)
        Ok(content) => {
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&content) {
                issues.push(ConfigIssue {
                    scope: scope.to_string(),
                    file: path.display().to_string(),
                    severity: "error".to_string(),
                    message: format!("Invalid JSON: {}", e),
                });
            }
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            issues.push(ConfigIssue {
                scope: scope.to_string(),
                file: path.display().to_string(),
                severity: "warning".to_string(),
                message: format!("Cannot read file: {}", e),
            });
        }
        _ => {} // File not found is OK
    }
}

// ── Sub-check: Keybindings validation ──

fn validate_keybindings_at(home: &Path) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();
    let path = home.join("keybindings.json");

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(v) => {
                    if !v.is_object() {
                        issues.push(ConfigIssue {
                            scope: "user".to_string(),
                            file: path.display().to_string(),
                            severity: "error".to_string(),
                            message: "Top-level value must be an object".to_string(),
                        });
                    } else if let Some(obj) = v.as_object() {
                        // Best-effort: values should be string or null
                        for (key, val) in obj {
                            if !val.is_string() && !val.is_null() {
                                issues.push(ConfigIssue {
                                    scope: "user".to_string(),
                                    file: path.display().to_string(),
                                    severity: "warning".to_string(),
                                    message: format!(
                                        "Key \"{}\" — value should be string or null (best-effort)",
                                        key
                                    ),
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    issues.push(ConfigIssue {
                        scope: "user".to_string(),
                        file: path.display().to_string(),
                        severity: "error".to_string(),
                        message: format!("Invalid JSON: {}", e),
                    });
                }
            }
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            issues.push(ConfigIssue {
                scope: "user".to_string(),
                file: path.display().to_string(),
                severity: "warning".to_string(),
                message: format!("Cannot read file: {}", e),
            });
        }
        _ => {} // Not found is OK
    }

    issues
}

// ── Sub-check: MCP config validation ──

fn validate_mcp_configs_at(home: &Path, cwd: &str, has_valid_cwd: bool) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();
    let home_parent = home.parent().unwrap_or(home);

    // 1. ~/.claude.json → top-level mcpServers (user scope)
    let claude_json_path = home_parent.join(".claude.json");
    if let Some(root) = read_json_file(&claude_json_path) {
        if let Some(servers) = root.get("mcpServers") {
            validate_mcp_servers(servers, "user", &claude_json_path, &mut issues);
        }

        // 2. ~/.claude.json → projects[cwd].mcpServers (local scope)
        if has_valid_cwd {
            if let Some(projects) = root.get("projects").and_then(|p| p.as_object()) {
                if let Some(proj) = projects.get(cwd).and_then(|p| p.as_object()) {
                    if let Some(servers) = proj.get("mcpServers") {
                        validate_mcp_servers(servers, "local", &claude_json_path, &mut issues);
                    }
                }
            }
        }
    }

    // 3. ~/.claude/settings.json → mcpServers (user scope fallback)
    let settings_path = home.join("settings.json");
    if let Some(root) = read_json_file(&settings_path) {
        if let Some(servers) = root.get("mcpServers") {
            validate_mcp_servers(servers, "user", &settings_path, &mut issues);
        }
    }

    // 4. {cwd}/.mcp.json → mcpServers (project scope)
    if has_valid_cwd {
        let mcp_json_path = Path::new(cwd).join(".mcp.json");
        if let Some(root) = read_json_file(&mcp_json_path) {
            if let Some(servers) = root.get("mcpServers") {
                validate_mcp_servers(servers, "project", &mcp_json_path, &mut issues);
            }
        }
    }

    issues
}

fn read_json_file(path: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn validate_mcp_servers(
    servers: &serde_json::Value,
    scope: &str,
    file: &Path,
    issues: &mut Vec<ConfigIssue>,
) {
    let obj = match servers.as_object() {
        Some(o) => o,
        None => {
            issues.push(ConfigIssue {
                scope: scope.to_string(),
                file: file.display().to_string(),
                severity: "error".to_string(),
                message: "mcpServers must be an object".to_string(),
            });
            return;
        }
    };

    for (name, entry) in obj {
        let entry_obj = match entry.as_object() {
            Some(o) => o,
            None => {
                issues.push(ConfigIssue {
                    scope: scope.to_string(),
                    file: file.display().to_string(),
                    severity: "error".to_string(),
                    message: format!("\"{}\" — entry must be an object", name),
                });
                continue;
            }
        };

        let transport_type = entry_obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("stdio");

        match transport_type {
            "stdio" => {
                if !entry_obj.contains_key("command") || !entry_obj["command"].is_string() {
                    issues.push(ConfigIssue {
                        scope: scope.to_string(),
                        file: file.display().to_string(),
                        severity: "error".to_string(),
                        message: format!("\"{}\" — missing \"command\" field (type=stdio)", name),
                    });
                }
            }
            "http" | "sse" => {
                if !entry_obj.contains_key("url") || !entry_obj["url"].is_string() {
                    issues.push(ConfigIssue {
                        scope: scope.to_string(),
                        file: file.display().to_string(),
                        severity: "error".to_string(),
                        message: format!(
                            "\"{}\" — missing \"url\" field (type={})",
                            name, transport_type
                        ),
                    });
                }
            }
            _ => {} // Unknown transport type: don't validate further
        }
    }
}

// ── Sub-check: Environment variables ──

fn check_env_vars() -> Vec<ConfigIssue> {
    let mut issues = Vec::new();

    for &(name, min, max) in ENV_VAR_LIMITS {
        if let Ok(val_str) = std::env::var(name) {
            match val_str.parse::<u64>() {
                Ok(val) if val < min || val > max => {
                    issues.push(ConfigIssue {
                        scope: "env".to_string(),
                        file: name.to_string(),
                        severity: "warning".to_string(),
                        message: format!("{}={} (valid range: {}–{})", name, val, min, max),
                    });
                }
                Err(_) => {
                    issues.push(ConfigIssue {
                        scope: "env".to_string(),
                        file: name.to_string(),
                        severity: "warning".to_string(),
                        message: format!("{}={} — not a valid integer", name, val_str),
                    });
                }
                _ => {} // In range, OK
            }
        }
    }

    issues
}

// ── Sub-check: CLAUDE.md files ──

fn scan_claude_md_files_at(home: &Path, cwd: &str, has_valid_cwd: bool) -> Vec<ClaudeMdInfo> {
    let mut files = Vec::new();

    // ~/.claude/CLAUDE.md
    let global_path = home.join("CLAUDE.md");
    if let Ok(content) = std::fs::read_to_string(&global_path) {
        files.push(ClaudeMdInfo {
            path: global_path.display().to_string(),
            size_chars: content.chars().count(),
        });
    }

    if has_valid_cwd {
        // {cwd}/CLAUDE.md
        let cwd_path = Path::new(cwd).join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&cwd_path) {
            files.push(ClaudeMdInfo {
                path: cwd_path.display().to_string(),
                size_chars: content.chars().count(),
            });
        }

        // {cwd}/.claude/CLAUDE.md
        let cwd_dot_path = Path::new(cwd).join(".claude").join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&cwd_dot_path) {
            files.push(ClaudeMdInfo {
                path: cwd_dot_path.display().to_string(),
                size_chars: content.chars().count(),
            });
        }
    }

    files
}

// ── Sub-check: Sandbox ──

fn check_sandbox() -> Option<bool> {
    if cfg!(target_os = "macos") {
        Some(Path::new("/usr/bin/sandbox-exec").exists())
    } else {
        None
    }
}

// ── Sub-check: Lock files ──

fn list_lock_files_at(home: &Path) -> Vec<String> {
    let locks_dir = home.join("locks");
    match std::fs::read_dir(&locks_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_project_init_no_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent_subdir");
        let result = check_project_init(nonexistent.to_string_lossy().into()).unwrap();
        assert!(!result.has_claude_md);
    }

    #[test]
    fn test_check_project_init_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = check_project_init(dir.path().to_string_lossy().into()).unwrap();
        assert!(!result.has_claude_md);
    }

    #[test]
    fn test_check_project_init_with_claude_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# Project").unwrap();
        let result = check_project_init(dir.path().to_string_lossy().into()).unwrap();
        assert!(result.has_claude_md);
    }

    // ── run_diagnostics sub-check tests ──

    #[test]
    fn test_validate_settings_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("settings.json"), "{ invalid json }").unwrap();
        let issues = validate_config_files_at(home, "/nonexistent", false);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, "error");
        assert!(issues[0].message.contains("Invalid JSON"));
    }

    #[test]
    fn test_validate_settings_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("settings.json"), r#"{"key": "value"}"#).unwrap();
        let issues = validate_config_files_at(home, "/nonexistent", false);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_invalid_cwd_skips_project_scope() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // Write project-scope settings — should be skipped when cwd invalid
        let proj_dir = dir.path().join("project").join(".claude");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("settings.json"), "invalid").unwrap();
        let issues = validate_config_files_at(home, "/nonexistent_xyz", false);
        // Only user scope checked, no project scope issue
        assert!(issues.iter().all(|i| i.scope == "user" || i.scope == "env"));
    }

    #[test]
    fn test_validate_keybindings_non_object() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("keybindings.json"), "[]").unwrap();
        let issues = validate_keybindings_at(home);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, "error");
        assert!(issues[0].message.contains("must be an object"));
    }

    #[test]
    fn test_check_env_vars_out_of_range() {
        // Temporarily set an env var with an out-of-range value
        let key = "CLAUDE_CODE_MAX_OUTPUT_TOKENS";
        let orig = std::env::var(key).ok();
        std::env::set_var(key, "999999");
        let issues = check_env_vars();
        // Restore
        match orig {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let found = issues.iter().any(|i| i.message.contains(key));
        assert!(found, "Expected warning for out-of-range {}", key);
    }

    #[test]
    fn test_scan_claude_md_files_at() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home_claude");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("CLAUDE.md"), "# Global").unwrap();

        let cwd_dir = dir.path().join("project");
        std::fs::create_dir_all(&cwd_dir).unwrap();
        std::fs::write(cwd_dir.join("CLAUDE.md"), "# Project content here").unwrap();

        let files = scan_claude_md_files_at(&home, &cwd_dir.to_string_lossy(), true);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].size_chars, 8); // "# Global"
        assert_eq!(files[1].size_chars, 22); // "# Project content here"
    }

    #[test]
    fn test_validate_mcp_stdio_missing_command() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(
            home.join("settings.json"),
            r#"{"mcpServers":{"s1":{"type":"stdio"}}}"#,
        )
        .unwrap();
        let issues = validate_mcp_configs_at(home, "/nonexistent", false);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, "error");
        assert!(issues[0].message.contains("missing \"command\""));
    }

    #[test]
    fn test_validate_mcp_http_missing_url() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(
            home.join("settings.json"),
            r#"{"mcpServers":{"s1":{"type":"http"}}}"#,
        )
        .unwrap();
        let issues = validate_mcp_configs_at(home, "/nonexistent", false);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, "error");
        assert!(issues[0].message.contains("missing \"url\""));
    }

    #[test]
    fn test_validate_mcp_valid_entry() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(
            home.join("settings.json"),
            r#"{"mcpServers":{"s1":{"command":"node","args":["server.js"]},"s2":{"type":"http","url":"http://localhost:3000"}}}"#,
        )
        .unwrap();
        let issues = validate_mcp_configs_at(home, "/nonexistent", false);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    // ── detect_local_proxy tests ──

    #[tokio::test]
    async fn test_detect_proxy_not_running() {
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            // Use a non-routable address to guarantee connection failure
            let url = "http://192.0.2.1:1";
            let result = detect_proxy_inner("test-proxy", url).await;
            assert!(
                !result.running,
                "expected not running, error={:?}",
                result.error
            );
            assert!(!result.needs_auth);
            assert!(result.error.is_some());
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_detect_proxy_running_200() {
        use tokio::io::AsyncWriteExt;
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            // Spawn a minimal HTTP server that returns 200
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = detect_proxy_inner("test-proxy", &url).await;
            assert!(result.running);
            assert!(!result.needs_auth);
            assert!(result.error.is_none());
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_detect_proxy_running_401_needs_auth() {
        use tokio::io::AsyncWriteExt;
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            // Spawn a minimal HTTP server that returns 401
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
                    let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = detect_proxy_inner("test-proxy", &url).await;
            assert!(result.running);
            assert!(result.needs_auth);
            assert!(result.error.is_none());
        });
        timeout.await.expect("test timed out");
    }

    // ── test_api_connectivity tests ──

    #[tokio::test]
    async fn test_api_connectivity_success_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = AsyncReadExt::read(&mut stream, &mut buf).await;
                    let body = r#"{"content":[{"text":"Hello!"}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = test_api_inner("test-key", &url, "ANTHROPIC_API_KEY", "test-model").await;
            assert!(result.success);
            assert!(!result.partial);
            assert!(result.latency_ms > 0);
            assert_eq!(result.reply, Some("Hello!".to_string()));
            assert!(result.error.is_none());
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_auth_failure_401() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = AsyncReadExt::read(&mut stream, &mut buf).await;
                    let body = r#"{"error":{"message":"Invalid API key"}}"#;
                    let resp = format!(
                        "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = test_api_inner("bad-key", &url, "ANTHROPIC_API_KEY", "test-model").await;
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Authentication failed"),
                "error was: {:?}",
                result.error
            );
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_not_found_404() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = AsyncReadExt::read(&mut stream, &mut buf).await;
                    let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = test_api_inner("test-key", &url, "ANTHROPIC_API_KEY", "test-model").await;
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Endpoint not found"),
                "error was: {:?}",
                result.error
            );
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_header_x_api_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, rx) = tokio::sync::oneshot::channel::<String>();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let n = AsyncReadExt::read(&mut stream, &mut buf).await.unwrap();
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = tx.send(req_str);
                    let body = r#"{"content":[{"text":"ok"}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result =
                test_api_inner("test-key-123", &url, "ANTHROPIC_API_KEY", "test-model").await;
            assert!(result.success);
            let req_str = rx.await.unwrap();
            let req_lower = req_str.to_lowercase();
            assert!(
                req_lower.contains("x-api-key: test-key-123"),
                "expected x-api-key header, got: {}",
                req_str
            );
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_header_bearer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, rx) = tokio::sync::oneshot::channel::<String>();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let n = AsyncReadExt::read(&mut stream, &mut buf).await.unwrap();
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = tx.send(req_str);
                    let body = r#"{"content":[{"text":"ok"}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            let result = test_api_inner(
                "test-bearer-key",
                &url,
                "ANTHROPIC_AUTH_TOKEN",
                "test-model",
            )
            .await;
            assert!(result.success);
            let req_str = rx.await.unwrap();
            let req_lower = req_str.to_lowercase();
            assert!(
                req_lower.contains("authorization: bearer test-bearer-key"),
                "expected Bearer header, got: {}",
                req_str
            );
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_probe_partial_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = AsyncReadExt::read(&mut stream, &mut buf).await;
                    // Simulate a 400 "model not found" response
                    let body = r#"{"error":{"message":"model: claude-sonnet-4-6 not found"}}"#;
                    let resp = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            // model="" triggers probe mode
            let result = test_api_inner("test-key", &url, "ANTHROPIC_API_KEY", "").await;
            assert!(result.success, "expected partial success");
            assert!(result.partial, "expected partial=true");
            assert!(result.error.is_none());
        });
        timeout.await.expect("test timed out");
    }

    #[tokio::test]
    async fn test_api_connectivity_non_probe_400_is_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = AsyncReadExt::read(&mut stream, &mut buf).await;
                    let body = r#"{"error":{"message":"model: foo not found"}}"#;
                    let resp = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
            let url = format!("http://127.0.0.1:{}", port);
            // model="foo" — NOT probe mode, so 400 should be a real failure
            let result = test_api_inner("test-key", &url, "ANTHROPIC_API_KEY", "foo").await;
            assert!(!result.success, "expected failure for non-probe 400");
            assert!(!result.partial);
            assert!(result.error.is_some());
        });
        timeout.await.expect("test timed out");
    }
}

/// Fetch npm dist-tags for @anthropic-ai/claude-code.
/// Returns latest/stable version strings. Non-fatal: returns None on failure.
#[tauri::command]
pub async fn get_cli_dist_tags() -> Result<CliDistTags, String> {
    log::debug!("[diagnostics] get_cli_dist_tags");
    let (latest, stable, _channel) = fetch_dist_tags_inner().await;
    Ok(CliDistTags { latest, stable })
}

/// Check for existing SSH key pairs. Returns info about the first usable pair found.
/// Priority: ed25519 > rsa. A "usable pair" means both private key and .pub exist.
#[tauri::command]
pub fn check_ssh_key() -> Result<SshKeyInfo, String> {
    let candidates = [("~/.ssh/id_ed25519", "ed25519"), ("~/.ssh/id_rsa", "rsa")];

    #[cfg(unix)]
    let ssh_copy_id_available = crate::agent::claude_stream::which_binary("ssh-copy-id").is_some();
    #[cfg(not(unix))]
    let ssh_copy_id_available = false;

    log::debug!(
        "[diagnostics] check_ssh_key: ssh_copy_id_available={}",
        ssh_copy_id_available
    );

    // First pass: find a complete pair (private + pub both exist)
    for (tilde_path, key_type) in &candidates {
        let expanded = expand_local_tilde(tilde_path);
        let pub_expanded = format!("{}.pub", expanded);
        let priv_exists = std::path::Path::new(&expanded).is_file();
        let pub_exists = std::path::Path::new(&pub_expanded).is_file();

        log::debug!(
            "[diagnostics] check_ssh_key: {} priv={} pub={}",
            tilde_path,
            priv_exists,
            pub_exists
        );

        if priv_exists && pub_exists {
            return Ok(SshKeyInfo {
                key_path: tilde_path.to_string(),
                key_path_expanded: expanded,
                pub_key_path: format!("{}.pub", tilde_path),
                key_type: key_type.to_string(),
                exists: true,
                pub_exists: true,
                ssh_copy_id_available,
            });
        }
    }

    // Second pass: report first partial match (private exists but pub missing)
    for (tilde_path, key_type) in &candidates {
        let expanded = expand_local_tilde(tilde_path);
        let priv_exists = std::path::Path::new(&expanded).is_file();

        if priv_exists {
            return Ok(SshKeyInfo {
                key_path: tilde_path.to_string(),
                key_path_expanded: expanded,
                pub_key_path: format!("{}.pub", tilde_path),
                key_type: key_type.to_string(),
                exists: true,
                pub_exists: false,
                ssh_copy_id_available,
            });
        }
    }

    // Nothing found at all
    Ok(SshKeyInfo {
        key_path: "~/.ssh/id_ed25519".into(),
        key_path_expanded: expand_local_tilde("~/.ssh/id_ed25519"),
        pub_key_path: "~/.ssh/id_ed25519.pub".into(),
        key_type: "ed25519".into(),
        exists: false,
        pub_exists: false,
        ssh_copy_id_available,
    })
}

/// Generate an ed25519 SSH key pair. Fails if key already exists.
/// Returns SshKeyInfo for the newly created key.
#[tauri::command]
pub fn generate_ssh_key() -> Result<SshKeyInfo, String> {
    if crate::agent::claude_stream::which_binary("ssh-keygen").is_none() {
        return Err(ssh_not_found_msg("ssh-keygen"));
    }

    let ssh_dir = expand_local_tilde("~/.ssh");
    let key_path = expand_local_tilde("~/.ssh/id_ed25519");

    // Check if key already exists
    if std::path::Path::new(&key_path).is_file() {
        return Err("Key already exists at ~/.ssh/id_ed25519".into());
    }

    // Ensure ~/.ssh directory exists with correct permissions
    std::fs::create_dir_all(&ssh_dir).map_err(|e| format!("Failed to create ~/.ssh: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Failed to set ~/.ssh permissions: {}", e))?;
    }

    // Get hostname for comment
    let hostname = Command::new("hostname")
        .hide_console()
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "localhost".into());

    let comment = format!("opencovibe@{}", hostname);
    let aug_path = augmented_path();

    log::debug!(
        "[diagnostics] generate_ssh_key: path={}, comment={}",
        key_path,
        comment
    );

    let output = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", &comment, "-f", &key_path])
        .env("PATH", &aug_path)
        .hide_console()
        .output()
        .map_err(|e| format!("Failed to run ssh-keygen: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ssh-keygen failed: {}", stderr));
    }

    log::debug!("[diagnostics] generate_ssh_key: success");

    // Return fresh check result
    check_ssh_key()
}

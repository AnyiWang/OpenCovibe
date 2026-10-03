pub mod auth;
pub mod broadcaster;
pub mod dispatch;
pub mod router;
pub mod state;
pub mod ws;

use crate::storage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use state::{AppState, CoreState};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Allowed bind addresses (whitelist).
const ALLOWED_BINDS: &[&str] = &["127.0.0.1", "0.0.0.0", "::1", "::"];

/// How the initial server start should resolve its configuration.
///
/// Desktop mode defers to `settings.json` (including `web_server_enabled: false`);
/// headless mode treats the CLI flag as the user's explicit intent, so settings
/// cannot veto the start and CLI port/bind override the stored ones.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Ignore `web_server_enabled` in settings and start regardless.
    pub force: bool,
    /// Override `web_server_port` from settings.
    pub port: Option<u16>,
    /// Override `web_server_bind` from settings.
    pub bind: Option<String>,
}

impl StartOptions {
    /// Desktop launch behavior — settings are authoritative.
    pub fn from_settings() -> Self {
        Self::default()
    }

    /// `--headless` launch behavior — CLI flags win.
    pub fn forced(port: Option<u16>, bind: Option<String>) -> Self {
        Self {
            force: true,
            port,
            bind,
        }
    }
}

/// Config passed from frontend to restart_with_config.
#[derive(Debug, Deserialize)]
pub struct WebServerConfig {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
    pub allowed_origins: Option<Vec<String>>,
    #[serde(default)]
    pub tunnel_url: Option<String>,
}

/// Result of a restart operation.
#[derive(Debug, Serialize)]
pub struct RestartResult {
    pub started: bool,
    pub config_saved: bool,
}

/// Build web server status JSON — pure function, shared by IPC and WS dispatch.
/// Takes runtime effective values as params, reads settings for configured state.
pub fn build_status(
    effective_port: u16,
    effective_bind: &str,
    warning: &Option<String>,
) -> serde_json::Value {
    let settings = storage::settings::get_user_settings();
    let running = effective_port > 0;
    let mut status = json!({
        "enabled": settings.web_server_enabled.unwrap_or(false),
        "running": running,
        "port": if running { effective_port } else { settings.web_server_port.unwrap_or(9476) },
        "bind": if running && !effective_bind.is_empty() {
            effective_bind.to_string()
        } else {
            settings.web_server_bind.unwrap_or_else(|| "127.0.0.1".into())
        },
    });
    if let Some(w) = warning {
        status["warning"] = json!(w);
    }
    status
}

/// Initial start at app launch. Reads config from settings, unless `opts` overrides it.
/// Returns Ok(true) if started, Ok(false) if disabled, Err on failure.
pub async fn start_server(core: &CoreState, opts: StartOptions) -> Result<bool, String> {
    let _guard = core.ws_lock.lock().await;

    let settings = storage::settings::get_user_settings();
    if !opts.force && !settings.web_server_enabled.unwrap_or(false) {
        log::debug!("[web_server] disabled in settings, not starting");
        return Ok(false);
    }

    let port = opts.port.or(settings.web_server_port).unwrap_or(9476);
    let bind = opts
        .bind
        .or(settings.web_server_bind)
        .unwrap_or_else(|| "127.0.0.1".into());

    // Guard against hand-edited settings.json
    if !ALLOWED_BINDS.contains(&bind.as_str()) {
        return Err(format!(
            "invalid bind '{}' in settings (allowed: {})",
            bind,
            ALLOWED_BINDS.join(", ")
        ));
    }
    if port < 1024 {
        return Err(format!(
            "invalid port {} in settings (range: 1024-65535)",
            port
        ));
    }

    // Validate tunnel URL on startup (collect warnings, don't block)
    let mut warnings: Vec<String> = Vec::new();
    let tunnel_origin = match &settings.web_server_tunnel_url {
        Some(raw) if !raw.trim().is_empty() => match validate_tunnel_url(raw) {
            Ok(origin) => {
                log::debug!("[web_server] tunnel origin on startup: {}", origin);
                Some(origin)
            }
            Err(e) => {
                let msg = format!(
                    "Invalid tunnel URL in settings: {}. Tunnel CORS disabled.",
                    e
                );
                log::error!("[web_server] {}", msg);
                warnings.push(msg);
                None
            }
        },
        _ => None,
    };

    // Normalize manual origins on startup (handles legacy/hand-edited settings).
    let manual_origins = match normalize_origins(&settings.web_server_allowed_origins) {
        Ok(normalized) => normalized,
        Err(e) => {
            let msg = format!(
                "Invalid allowed_origins in settings: {}. \
                 Reverse proxy CORS will fail. Fix in Settings > Web Server.",
                e
            );
            log::error!("[web_server] {}", msg);
            warnings.push(msg);
            None
        }
    };

    // Merge manual + tunnel origins
    let effective = merge_effective_origins(&manual_origins, &tunnel_origin);

    if warnings.is_empty() {
        *core.warning.0.write().await = None;
    } else {
        *core.warning.0.write().await = Some(warnings.join("\n"));
    }

    spawn_server(core, port, &bind, effective).await?;
    Ok(true)
}

/// Restart: stop → start → save.
/// Config is passed directly, NOT read from settings.
/// On start failure: settings unchanged, server stopped, returns Err.
/// On start success + save failure: server running, started=true, config_saved=false.
pub async fn restart_with_config(
    core: &CoreState,
    config: WebServerConfig,
) -> Result<RestartResult, String> {
    let _guard = core.ws_lock.lock().await;

    // Disable path: stop server, partial disable (only write enabled=false).
    if !config.enabled {
        stop_server_inner(core).await;
        // Disable is security-sensitive: if save fails, next startup re-enables.
        storage::settings::save_web_server_partial_disable()?;
        return Ok(RestartResult {
            started: false,
            config_saved: true,
        });
    }

    // Enable path: validate config before doing anything destructive
    if config.port < 1024 {
        return Err(format!("port must be 1024-65535, got {}", config.port));
    }
    if !ALLOWED_BINDS.contains(&config.bind.as_str()) {
        return Err(format!("bind must be one of: {}", ALLOWED_BINDS.join(", ")));
    }
    let normalized_origins = normalize_origins(&config.allowed_origins)?;

    // Validate tunnel URL (if provided)
    let tunnel_origin = match &config.tunnel_url {
        Some(raw) if !raw.trim().is_empty() => {
            let origin = validate_tunnel_url(raw)?;
            log::debug!("[web_server] tunnel origin: {}", origin);
            Some(origin)
        }
        _ => None,
    };

    // Merge manual + tunnel origins for runtime CORS
    let effective = merge_effective_origins(&normalized_origins, &tunnel_origin);

    // Validation passed → clear startup warning, stop old server
    *core.warning.0.write().await = None;
    stop_server_inner(core).await;

    // Start new server — bind happens here, errors propagate to caller
    match spawn_server(core, config.port, &config.bind, effective).await {
        Ok(_actual_port) => {
            // Success → save config; manual origins + tunnel stored separately
            let saved = match storage::settings::save_web_server_config(
                config.enabled,
                config.port,
                &config.bind,
                &normalized_origins,
                &tunnel_origin,
            ) {
                Ok(()) => true,
                Err(e) => {
                    log::error!(
                        "[web_server] server started but config save failed: {}. \
                         Server is running; next launch may use stale config.",
                        e
                    );
                    false
                }
            };
            Ok(RestartResult {
                started: true,
                config_saved: saved,
            })
        }
        Err(e) => {
            // Bind failed, settings NOT modified (old config preserved).
            log::error!("[web_server] restart failed, settings unchanged: {}", e);
            Err(e)
        }
    }
}

/// Internal: cancel serve task + await JoinHandle + reset effective state.
/// Caller MUST hold WebServerLock.
async fn stop_server_inner(core: &CoreState) {
    // 1. Cancel the serve task
    {
        let mut guard = core.ws_cancel.lock().await;
        guard.cancel();
        *guard = CancellationToken::new();
    }

    // 2. Await JoinHandle to ensure listener is dropped (port released)
    let ws_handle = core.ws_handle.clone();
    {
        let mut handle_guard = ws_handle.lock().await;
        if let Some(mut handle) = handle_guard.take() {
            // Use select! to keep handle ownership on timeout branch
            let completed = tokio::select! {
                result = &mut handle => {
                    match result {
                        Ok(()) => log::debug!("[web_server] serve task exited cleanly"),
                        Err(e) => log::warn!("[web_server] serve task join error: {}", e),
                    }
                    true
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    false
                }
            };
            if !completed {
                // Graceful shutdown timed out — forcefully abort, then re-await
                log::warn!("[web_server] serve task did not exit in 5s, aborting");
                handle.abort();
                match tokio::time::timeout(std::time::Duration::from_secs(1), handle).await {
                    Ok(_) => log::debug!("[web_server] aborted task confirmed terminated"),
                    Err(_) => log::error!(
                        "[web_server] aborted task did not terminate in 1s, \
                         proceeding anyway (port may still be held)"
                    ),
                }
            }
        }
    }

    // 3. Reset effective state
    core.effective_port.store(0, Ordering::Relaxed);
    *core.effective_bind.0.write().await = String::new();
    *core.warning.0.write().await = None;
    log::debug!("[web_server] stopped");
}

/// Internal: build AppState, bind port, spawn serve task, store JoinHandle.
/// Returns actual bound port.
async fn spawn_server(
    core: &CoreState,
    port: u16,
    bind: &str,
    allowed_origins: Option<Vec<String>>,
) -> Result<u16, String> {
    let live_token = core.token.clone();
    let token = live_token.read().await.clone();
    if token.is_empty() {
        return Err("no web server token".into());
    }

    let effective_port = core.effective_port.clone();

    // Bind BEFORE spawning — errors propagate to caller
    let listener = bind_with_fallback(bind, port).await?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    effective_port.store(actual_port, Ordering::Relaxed);
    *core.effective_bind.0.write().await = bind.to_string();
    log::info!("[web_server] bound to {}:{}", bind, actual_port);

    // Increment generation — stale tasks check this before cleanup.
    let generation = core.generation.0.clone();
    let my_gen = generation.load(Ordering::SeqCst) + 1;
    generation.store(my_gen, Ordering::SeqCst);

    let app_state = AppState {
        process_map: core.process_map.clone(),
        sessions: core.sessions.clone(),
        spawn_locks: core.spawn_locks.clone(),
        writer: core.writer.clone(),
        cancel_token: core.cancel_token.clone(),
        cli_info_cache: core.cli_info_cache.clone(),
        emitter: core.emitter.clone(),
        broadcaster: core.broadcaster.clone(),
        token: live_token,
        token_version: core.token_version.clone(),
        http_sessions: Arc::new(Mutex::new(HashMap::new())),
        effective_port: effective_port.clone(),
        bind_addr: Arc::new(bind.to_string()),
        allowed_origins,
        ws_shutdown: core.ws_shutdown.clone(),
    };

    // Set up cancel tokens
    let ws_cancel = core.ws_cancel.clone();
    let ws_cancel_token = ws_cancel.lock().await.clone();
    let app_cancel = core.cancel_token.clone();
    let effective_port_cleanup = effective_port.clone();
    let generation_cleanup = generation.clone();

    let router = router::build_router(app_state);

    // Spawn serve task (use tokio::spawn directly for tokio::task::JoinHandle compatibility)
    let join_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                tokio::select! {
                    _ = ws_cancel_token.cancelled() => {
                        log::debug!("[web_server] shutting down (restart)");
                    }
                    _ = app_cancel.cancelled() => {
                        log::debug!("[web_server] shutting down (app exit)");
                    }
                }
            })
            .await
            .unwrap_or_else(|e| log::error!("[web_server] server error: {}", e));

        // Serve task exited — only clear effective_port if generation matches
        let current_gen = generation_cleanup.load(Ordering::SeqCst);
        if current_gen == my_gen {
            effective_port_cleanup.store(0, Ordering::Relaxed);
            log::debug!(
                "[web_server] serve task exited, effective_port reset (gen={})",
                my_gen
            );
        } else {
            log::debug!(
                "[web_server] stale serve task exited, NOT resetting effective_port \
                 (my_gen={}, current_gen={})",
                my_gen,
                current_gen
            );
        }
    });

    // Store JoinHandle for later await during stop
    *core.ws_handle.lock().await = Some(join_handle);

    log::info!("[web_server] serving on http://{}:{}", bind, actual_port);
    Ok(actual_port)
}

/// Bind to target port, fallback to +1/+2 if busy.
async fn bind_with_fallback(bind: &str, port: u16) -> Result<tokio::net::TcpListener, String> {
    for offset in 0..=2u16 {
        let Some(try_port) = port.checked_add(offset) else {
            break; // overflow — stop trying
        };
        let addr = format!("{}:{}", bind, try_port);
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                if offset > 0 {
                    log::warn!("[web_server] port {} busy, fell back to {}", port, try_port);
                }
                return Ok(listener);
            }
            Err(e) => {
                log::warn!("[web_server] bind {}:{} failed: {}", bind, try_port, e);
            }
        }
    }
    Err(format!("failed to bind {}:{} (tried +0/+1/+2)", bind, port))
}

/// Validate and normalize a tunnel URL → return its origin (scheme://host[:port]).
/// Accepts raw user input; trims, strips trailing slashes, extracts origin.
fn validate_tunnel_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("tunnel URL is empty".into());
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|e| format!("Invalid tunnel URL '{}': {}", trimmed, e))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("Tunnel URL must use http or https: '{}'", trimmed));
    }
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err(format!("Tunnel URL must have a host: '{}'", trimmed));
    }
    let origin = parsed.origin();
    if !origin.is_tuple() {
        return Err(format!("Invalid tunnel URL (opaque): '{}'", trimmed));
    }
    Ok(origin.ascii_serialization())
}

/// Merge manual origins with tunnel origin. Tunnel origin is appended if not already present.
fn merge_effective_origins(
    manual: &Option<Vec<String>>,
    tunnel_origin: &Option<String>,
) -> Option<Vec<String>> {
    match (manual, tunnel_origin) {
        (None, None) => None,
        (Some(m), None) => {
            if m.is_empty() {
                None
            } else {
                Some(m.clone())
            }
        }
        (None, Some(t)) => Some(vec![t.clone()]),
        (Some(m), Some(t)) => {
            let mut merged = m.clone();
            if !merged.contains(t) {
                merged.push(t.clone());
            }
            if merged.is_empty() {
                None
            } else {
                Some(merged)
            }
        }
    }
}

/// Normalize origins: parse URL → extract origin (scheme://host[:port]) → dedupe.
fn normalize_origins(origins: &Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
    let Some(raw) = origins else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }

    let mut normalized = Vec::new();
    for o in raw {
        let parsed = url::Url::parse(o).map_err(|e| format!("Invalid origin '{}': {}", o, e))?;

        // Only allow http/https
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(format!("Origin must use http or https: '{}'", o));
        }
        // Must have a host
        if parsed.host_str().is_none() || parsed.host_str().unwrap().is_empty() {
            return Err(format!("Origin must have a host: '{}'", o));
        }

        let origin = parsed.origin();
        if !origin.is_tuple() {
            return Err(format!("Invalid origin (opaque): '{}'", o));
        }

        let serialized = origin.ascii_serialization();
        if !normalized.contains(&serialized) {
            normalized.push(serialized);
        }
    }

    Ok(if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_origins tests ──

    #[test]
    fn normalize_strips_path() {
        let input = Some(vec!["https://example.com/path".to_string()]);
        let result = normalize_origins(&input).unwrap();
        assert_eq!(result, Some(vec!["https://example.com".to_string()]));
    }

    #[test]
    fn normalize_ipv6_preserved() {
        let input = Some(vec!["http://[::1]:8080".to_string()]);
        let result = normalize_origins(&input).unwrap();
        assert_eq!(result, Some(vec!["http://[::1]:8080".to_string()]));
    }

    #[test]
    fn normalize_rejects_ftp() {
        let input = Some(vec!["ftp://x".to_string()]);
        assert!(normalize_origins(&input).is_err());
    }

    #[test]
    fn normalize_rejects_no_host() {
        let input = Some(vec!["http:///".to_string()]);
        assert!(normalize_origins(&input).is_err());
    }

    #[test]
    fn normalize_dedupes() {
        let input = Some(vec![
            "https://example.com".to_string(),
            "https://example.com/different-path".to_string(),
        ]);
        let result = normalize_origins(&input).unwrap();
        assert_eq!(result, Some(vec!["https://example.com".to_string()]));
    }

    #[test]
    fn normalize_none_passthrough() {
        assert_eq!(normalize_origins(&None).unwrap(), None);
    }

    #[test]
    fn normalize_empty_vec_returns_none() {
        assert_eq!(normalize_origins(&Some(vec![])).unwrap(), None);
    }

    // ── build_status tests ──

    // ── validate_tunnel_url tests ──

    #[test]
    fn validate_tunnel_strips_path_and_trailing_slash() {
        let result = validate_tunnel_url("https://abc123.ngrok-free.app/some/path/").unwrap();
        assert_eq!(result, "https://abc123.ngrok-free.app");
    }

    #[test]
    fn validate_tunnel_http_with_port() {
        let result = validate_tunnel_url("http://localhost:4040").unwrap();
        assert_eq!(result, "http://localhost:4040");
    }

    #[test]
    fn validate_tunnel_rejects_ftp() {
        assert!(validate_tunnel_url("ftp://example.com").is_err());
    }

    #[test]
    fn validate_tunnel_rejects_empty() {
        assert!(validate_tunnel_url("").is_err());
        assert!(validate_tunnel_url("   ").is_err());
    }

    #[test]
    fn validate_tunnel_rejects_no_host() {
        assert!(validate_tunnel_url("http:///").is_err());
    }

    // ── merge_effective_origins tests ──

    #[test]
    fn merge_both_none() {
        assert_eq!(merge_effective_origins(&None, &None), None);
    }

    #[test]
    fn merge_manual_only() {
        let manual = Some(vec!["https://a.com".to_string()]);
        let result = merge_effective_origins(&manual, &None);
        assert_eq!(result, Some(vec!["https://a.com".to_string()]));
    }

    #[test]
    fn merge_tunnel_only() {
        let tunnel = Some("https://tunnel.app".to_string());
        let result = merge_effective_origins(&None, &tunnel);
        assert_eq!(result, Some(vec!["https://tunnel.app".to_string()]));
    }

    #[test]
    fn merge_dedupes_tunnel_in_manual() {
        let manual = Some(vec!["https://tunnel.app".to_string()]);
        let tunnel = Some("https://tunnel.app".to_string());
        let result = merge_effective_origins(&manual, &tunnel);
        assert_eq!(result, Some(vec!["https://tunnel.app".to_string()]));
    }

    #[test]
    fn merge_appends_tunnel() {
        let manual = Some(vec!["https://a.com".to_string()]);
        let tunnel = Some("https://tunnel.app".to_string());
        let result = merge_effective_origins(&manual, &tunnel);
        assert_eq!(
            result,
            Some(vec![
                "https://a.com".to_string(),
                "https://tunnel.app".to_string()
            ])
        );
    }

    // ── build_status tests ──

    #[test]
    fn build_status_running() {
        let status = build_status(9476, "0.0.0.0", &None);
        assert_eq!(status["running"], true);
        assert_eq!(status["port"], 9476);
        assert_eq!(status["bind"], "0.0.0.0");
    }

    #[test]
    fn build_status_stopped_uses_settings_fallback() {
        let status = build_status(0, "", &None);
        assert_eq!(status["running"], false);
        // port and bind come from settings (defaults when no settings file)
    }

    #[test]
    fn build_status_with_warning() {
        let warning = Some("test warning".to_string());
        let status = build_status(9476, "127.0.0.1", &warning);
        assert_eq!(status["warning"], "test warning");
    }

    #[test]
    fn build_status_no_warning_field_when_none() {
        let status = build_status(9476, "127.0.0.1", &None);
        assert!(status.get("warning").is_none());
    }

    // ── candidate_ports (bind_with_fallback edge cases) ──

    #[test]
    fn candidate_ports_normal() {
        let ports: Vec<u16> = (0..=2u16)
            .filter_map(|offset| 9476u16.checked_add(offset))
            .collect();
        assert_eq!(ports, vec![9476, 9477, 9478]);
    }

    #[test]
    fn candidate_ports_near_overflow() {
        let ports: Vec<u16> = (0..=2u16)
            .filter_map(|offset| 65534u16.checked_add(offset))
            .collect();
        assert_eq!(ports, vec![65534, 65535]);
    }

    #[test]
    fn candidate_ports_at_max() {
        let ports: Vec<u16> = (0..=2u16)
            .filter_map(|offset| 65535u16.checked_add(offset))
            .collect();
        assert_eq!(ports, vec![65535]);
    }
}

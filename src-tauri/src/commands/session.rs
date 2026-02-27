use crate::agent::adapter::{self, ActorSessionMap};
use crate::agent::claude_stream;
use crate::agent::session_actor::{self, ActorCommand, AttachmentData};
use crate::agent::spawn_locks::SpawnLocks;
use crate::models::{BusEvent, RemoteHost, RunMeta, RunStatus, SessionMode, UserSettings};
use crate::storage;
use crate::storage::events::EventWriter;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

/// Truncate a string to at most `max` bytes, snapping to a char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Helper: stop an existing actor for a run_id, await its shutdown.
/// Returns true if an actor was stopped.
async fn stop_actor(sessions: &ActorSessionMap, run_id: &str) -> Result<bool, String> {
    let handle = {
        let mut map = sessions.lock().await;
        map.remove(run_id)
    };

    let Some(handle) = handle else {
        return Ok(false);
    };

    log::debug!("[session] stopping actor for run_id={}", run_id);

    // Send Stop command
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if handle
        .cmd_tx
        .send(ActorCommand::Stop { reply: reply_tx })
        .await
        .is_ok()
    {
        // Wait for reply (actor acknowledged stop)
        let _ = reply_rx.await;
    }

    // Wait for actor task to finish (with timeout)
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.join_handle).await;

    Ok(true)
}

/// Helper: emit a BusEvent (persist + Tauri emit).
fn emit_bus_event(app: &AppHandle, writer: &EventWriter, run_id: &str, event: &BusEvent) {
    if let Err(e) = writer.write_bus_event(run_id, event) {
        log::warn!("[session] persist failed: {}", e);
    }
    let _ = app.emit("bus-event", event);
}

/// Resolve a RemoteHost from RunMeta.
/// Prefers the snapshot (self-contained), falls back to name lookup for old runs.
fn resolve_remote_host(meta: &RunMeta) -> Result<Option<RemoteHost>, String> {
    // Prefer snapshot (new runs have this)
    if let Some(ref snapshot) = meta.remote_host_snapshot {
        log::debug!(
            "[session] resolve_remote_host: using snapshot for '{}'",
            snapshot.name
        );
        return Ok(Some(snapshot.clone()));
    }
    // Fallback: name-based lookup (old runs without snapshot)
    match &meta.remote_host_name {
        Some(name) => {
            let settings = storage::settings::get_user_settings();
            settings
                .remote_hosts
                .iter()
                .find(|h| h.name == *name)
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("Remote host '{}' not found in settings", name))
        }
        None => Ok(None),
    }
}

/// Resolved authentication and environment info for spawning CLI.
struct ResolvedAuth {
    api_key: Option<String>,
    auth_token: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    extra_env: Option<std::collections::HashMap<String, String>>,
}

/// Resolve API authentication environment variables.
/// Returns ResolvedAuth with (api_key, auth_token, base_url, default_model, extra_env).
/// - `api_key`: for Anthropic official (`x-api-key` header)
/// - `auth_token`: for third-party platforms (`Authorization: Bearer` header)
/// - `base_url`: custom API endpoint
///
/// `api_key` and `auth_token` are mutually exclusive.
fn resolve_auth_env(remote: &Option<RemoteHost>, settings: &UserSettings) -> ResolvedAuth {
    let base_url = settings
        .anthropic_base_url
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned();

    // SSH remote: forward_api_key=false → no credentials forwarded
    if let Some(r) = remote.as_ref() {
        if !r.forward_api_key {
            return ResolvedAuth {
                api_key: None,
                auth_token: None,
                base_url: None,
                default_model: None,
                extra_env: None,
            };
        }
        // forward_api_key=true: fall through to normal resolution
    }

    // Local API Key mode (also used for remote with forward_api_key=true)
    if settings.auth_mode == "api" {
        if let Some(ref key) = settings.anthropic_api_key {
            if !key.is_empty() {
                // Use auth_env_var from platform preset to decide which header to use.
                // "ANTHROPIC_AUTH_TOKEN" → Bearer header (most third-party platforms)
                // "ANTHROPIC_API_KEY" (or unset) → x-api-key header (Anthropic, AiHubMix, Kimi Coding)
                let use_bearer = settings.auth_env_var.as_deref() == Some("ANTHROPIC_AUTH_TOKEN");

                if use_bearer {
                    return ResolvedAuth {
                        api_key: None,
                        auth_token: Some(key.clone()),
                        base_url,
                        default_model: None,
                        extra_env: None,
                    };
                } else {
                    return ResolvedAuth {
                        api_key: Some(key.clone()),
                        auth_token: None,
                        base_url,
                        default_model: None,
                        extra_env: None,
                    };
                }
            }
        }
    }

    ResolvedAuth {
        api_key: None,
        auth_token: None,
        base_url,
        default_model: None,
        extra_env: None,
    }
}

/// Resolve auth env using per-session platform_id.
/// Looks up the credential from `settings.platform_credentials` by platform_id,
/// then returns ResolvedAuth matching the credential's auth_env_var.
/// Falls back to global `resolve_auth_env()` if platform_id is None or credential not found.
///
/// For SSH remote sessions:
/// - `forward_api_key=true`: resolve credentials normally (platform-aware) and forward them
/// - `forward_api_key=false`: return empty ResolvedAuth — remote uses its own auth
fn resolve_auth_env_for_platform(
    remote: &Option<RemoteHost>,
    settings: &UserSettings,
    platform_id: Option<&str>,
) -> ResolvedAuth {
    // SSH remote with forward_api_key=false: don't forward any credentials
    if let Some(r) = remote.as_ref() {
        if !r.forward_api_key {
            log::debug!("[session] resolve_auth_env_for_platform: remote forward_api_key=false, no credentials forwarded");
            return ResolvedAuth {
                api_key: None,
                auth_token: None,
                base_url: None,
                default_model: None,
                extra_env: None,
            };
        }
        // forward_api_key=true: fall through to normal platform-aware resolution
    }

    // If we have a platform_id, try to find a matching credential
    if let Some(pid) = platform_id {
        if let Some(cred) = settings
            .platform_credentials
            .iter()
            .find(|c| c.platform_id == pid)
        {
            let key = cred.api_key.as_ref().filter(|k| !k.is_empty()).cloned();
            let base_url = cred.base_url.as_ref().filter(|s| !s.is_empty()).cloned();
            let use_bearer = cred.auth_env_var.as_deref() == Some("ANTHROPIC_AUTH_TOKEN");
            let default_model = cred.models.as_ref().and_then(|m| m.first()).cloned();
            let extra_env = cred.extra_env.clone();

            if let Some(k) = key {
                log::debug!(
                    "[session] resolve_auth_env_for_platform: platform={}, use_bearer={}, has_base_url={}, default_model={:?}, extra_env_count={}",
                    pid,
                    use_bearer,
                    base_url.is_some(),
                    default_model,
                    extra_env.as_ref().map_or(0, |e| e.len())
                );
                return if use_bearer {
                    ResolvedAuth {
                        api_key: None,
                        auth_token: Some(k),
                        base_url,
                        default_model,
                        extra_env,
                    }
                } else {
                    ResolvedAuth {
                        api_key: Some(k),
                        auth_token: None,
                        base_url,
                        default_model,
                        extra_env,
                    }
                };
            }
            // Credential found but no API key — fallback to global
            log::warn!(
                "[session] resolve_auth_env_for_platform: credential for platform '{}' has no api_key, falling back to global",
                pid
            );
        } else {
            log::warn!(
                "[session] resolve_auth_env_for_platform: no credential found for platform '{}', falling back to global",
                pid
            );
        }
    }

    // Fallback to global auth env
    resolve_auth_env(remote, settings)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    sessions: State<'_, ActorSessionMap>,
    seq_counter: State<'_, Arc<EventWriter>>,
    spawn_locks: State<'_, SpawnLocks>,
    cancel_token: State<'_, CancellationToken>,
    run_id: String,
    mode: Option<SessionMode>,
    session_id: Option<String>,
    initial_message: Option<String>,
    attachments: Option<Vec<AttachmentData>>,
    platform_id: Option<String>,
) -> Result<(), String> {
    let _guard = spawn_locks.acquire(&run_id).await;
    let session_mode = mode.unwrap_or_default();
    let att_list = attachments.unwrap_or_default();
    log::debug!(
        "[session] start_session called, run_id={}, mode={:?}, session_id={:?}, has_message={}, attachments={}",
        run_id,
        session_mode,
        session_id,
        initial_message.is_some(),
        att_list.len()
    );

    // 1. Read run metadata
    let meta =
        storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;
    log::debug!(
        "[session] meta loaded: agent={}, prompt={:?}, cwd={}",
        meta.agent,
        truncate_str(&meta.prompt, 80),
        meta.cwd
    );

    // 2. Read settings and build unified adapter settings
    let agent_settings = storage::settings::get_agent_settings(&meta.agent);
    let user_settings = storage::settings::get_user_settings();
    let adapter_settings =
        adapter::build_adapter_settings(&agent_settings, &user_settings, meta.model.clone());

    // 2b. Resolve remote host from RunMeta (audit #2: single truth source)
    let remote = resolve_remote_host(&meta)?;
    // Use per-session platform_id: prefer IPC param, fallback to RunMeta's saved platform_id
    let effective_pid = platform_id.as_deref().or(meta.platform_id.as_deref());
    let resolved = resolve_auth_env_for_platform(&remote, &user_settings, effective_pid);
    if remote.is_some() {
        log::debug!(
            "[session] remote mode: host={:?}, remote_cwd={:?}, has_key={}",
            meta.remote_host_name,
            meta.remote_cwd,
            resolved.api_key.is_some() || resolved.auth_token.is_some()
        );
    }

    // 3. Resolve resume session_id
    let resume_session_id = match &session_mode {
        SessionMode::Resume | SessionMode::Continue => {
            let sid = session_id
                .or_else(|| meta.session_id.clone())
                .ok_or_else(|| {
                    format!(
                        "session_id required for {:?} but not found in params or run metadata",
                        session_mode
                    )
                })?;
            Some(sid)
        }
        SessionMode::Fork => {
            return Err(
                "Fork mode not supported in start_session — use fork_session command instead"
                    .into(),
            );
        }
        SessionMode::New => None,
    };

    // Validate
    adapter::validate_session_params(&adapter_settings, &session_mode)?;

    let is_new = matches!(session_mode, SessionMode::New);

    // 4. Emit RunState(spawning) — UserMessage now handled by actor
    let spawning_event = BusEvent::RunState {
        run_id: run_id.clone(),
        state: "spawning".to_string(),
        exit_code: None,
        error: None,
    };
    emit_bus_event(&app, &seq_counter, &run_id, &spawning_event);
    storage::runs::update_status(&run_id, RunStatus::Running, None, None).ok();

    // 5. Stop any existing actor for this run_id
    let had_session = stop_actor(sessions.inner(), &run_id).await?;
    if had_session {
        log::debug!(
            "[session] old actor teardown complete for run_id={}",
            run_id
        );
    }

    // 6. Spawn CLI process (no initial stdin write — actor handles it)
    let effective_cwd = meta.remote_cwd.as_deref().unwrap_or(&meta.cwd);
    let (child, stdin, stdout, stderr) = spawn_cli_process(
        effective_cwd,
        &meta.prompt,
        &adapter_settings,
        &session_mode,
        resume_session_id.as_deref(),
        is_new,
        &att_list,
        remote.as_ref(),
        meta.remote_cwd.as_deref(),
        resolved.api_key.as_deref(),
        resolved.auth_token.as_deref(),
        resolved.base_url.as_deref(),
        &run_id,
        resolved.default_model.as_deref(),
        resolved.extra_env.as_ref(),
    )
    .await?;

    // 7. Compute turn baselines — 1-based: next_turn_index = N means next message gets turnIndex=N.
    // New session: first message gets turnIndex=1. Resume: first new message gets total+1.
    let (initial_turn_index, initial_auto_ctx_id) = if is_new {
        (1_u32, 1_u32)
    } else {
        let (total, normal) = crate::storage::events::count_user_messages(&run_id);
        (total + 1, normal + 1)
    };
    log::debug!(
        "[session] turn baselines: initial_turn_index={}, initial_auto_ctx_id={}",
        initial_turn_index,
        initial_auto_ctx_id
    );

    // 8. Spawn actor
    let actor_handle = session_actor::spawn_actor(
        app.clone(),
        Arc::clone(&seq_counter),
        sessions.inner().clone(),
        run_id.clone(),
        child,
        stdin,
        stdout,
        stderr,
        !is_new,
        cancel_token.inner().clone(),
        initial_turn_index,
        initial_auto_ctx_id,
    );
    let cmd_tx = actor_handle.cmd_tx.clone();
    sessions.lock().await.insert(run_id.clone(), actor_handle);

    // 9. Send initial message through actor (unified entry point for Turn Engine)
    let initial_text = if is_new {
        Some(meta.prompt.clone())
    } else {
        initial_message.clone()
    };
    if let Some(text) = initial_text {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        cmd_tx
            .send(ActorCommand::SendMessage {
                text,
                attachments: att_list,
                reply: reply_tx,
            })
            .await
            .map_err(|_| "Actor dead before initial message".to_string())?;
        reply_rx
            .await
            .map_err(|_| "Actor dropped initial message reply".to_string())??;
        log::debug!(
            "[session] initial message sent through actor for run_id={}",
            run_id
        );
    } else {
        // Resume/continue without message: emit synthetic idle so frontend shows input box
        let idle_event = BusEvent::RunState {
            run_id: run_id.clone(),
            state: "idle".to_string(),
            exit_code: None,
            error: None,
        };
        emit_bus_event(&app, &seq_counter, &run_id, &idle_event);
        log::debug!(
            "[session] resume/continue: emitted synthetic RunState(idle) for run_id={}",
            run_id
        );
    }

    log::debug!("[session] actor spawned successfully for run_id={}", run_id);
    Ok(())
}

#[tauri::command]
pub async fn send_session_message(
    _app: AppHandle,
    sessions: State<'_, ActorSessionMap>,
    _seq_counter: State<'_, Arc<EventWriter>>,
    run_id: String,
    message: String,
    attachments: Option<Vec<AttachmentData>>,
) -> Result<(), String> {
    // No SpawnLock — data operation, routed through actor channel
    let att_count = attachments.as_ref().map_or(0, |v| v.len());
    log::debug!(
        "[session] send_session_message: run_id={}, msg_len={}, attachments={}",
        run_id,
        message.len(),
        att_count
    );

    // Get channel sender
    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found", run_id))?
    };

    // Send message through actor channel
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::SendMessage {
            text: message.clone(),
            attachments: attachments.unwrap_or_default(),
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead".to_string())?;
    reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    // Turn Transaction Engine: actor's start_user_turn now handles
    // UserMessage + RunState(running) emission. No post-emit needed here.

    log::debug!(
        "[session] send_session_message: delivered to actor, run_id={}",
        run_id
    );
    Ok(())
}

#[tauri::command]
pub async fn stop_session(
    app: AppHandle,
    sessions: State<'_, ActorSessionMap>,
    seq_counter: State<'_, Arc<EventWriter>>,
    spawn_locks: State<'_, SpawnLocks>,
    run_id: String,
) -> Result<(), String> {
    let _guard = spawn_locks.acquire(&run_id).await;

    let was_active = stop_actor(sessions.inner(), &run_id).await?;
    if was_active {
        // Actor was active — emit stopped
        let event = BusEvent::RunState {
            run_id: run_id.clone(),
            state: "stopped".to_string(),
            exit_code: None,
            error: None,
        };
        emit_bus_event(&app, &seq_counter, &run_id, &event);
        storage::runs::update_status(&run_id, RunStatus::Stopped, None, None).ok();
    }

    Ok(())
}

#[tauri::command]
pub async fn send_session_control(
    sessions: State<'_, ActorSessionMap>,
    run_id: String,
    subtype: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    // No SpawnLock — data operation through actor channel
    log::debug!(
        "[session] send_session_control: run_id={}, subtype={}",
        run_id,
        subtype
    );

    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found", run_id))?
    };

    // Build control request
    let mut request = serde_json::json!({ "subtype": subtype });
    if let Some(p) = params {
        if let Some(obj) = p.as_object() {
            for (k, v) in obj {
                request[k] = v.clone();
            }
        }
    }

    // Phase 1: send control request, get response receiver
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::SendControl {
            request,
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead".to_string())?;

    let (request_id, response_rx) = reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    // Phase 2: await response outside actor (no lock held)
    match tokio::time::timeout(std::time::Duration::from_secs(10), response_rx).await {
        Ok(Ok(response)) => {
            log::debug!(
                "[session] control response received for req_id={}",
                request_id
            );
            Ok(response)
        }
        Ok(Err(_)) => {
            log::warn!(
                "[session] control response channel closed for req_id={}",
                request_id
            );
            Err("Control response channel closed (session may have ended)".to_string())
        }
        Err(_) => {
            log::warn!(
                "[session] control request timed out for req_id={}",
                request_id
            );
            Err("Timeout waiting for control response".to_string())
        }
    }
}

#[tauri::command]
pub fn get_bus_events(id: String, since_seq: Option<u64>) -> Vec<serde_json::Value> {
    storage::events::list_bus_events(&id, since_seq)
}

#[tauri::command]
pub async fn fork_session(
    app: AppHandle,
    sessions: State<'_, ActorSessionMap>,
    seq_counter: State<'_, Arc<EventWriter>>,
    spawn_locks: State<'_, SpawnLocks>,
    run_id: String,
) -> Result<String, String> {
    let _guard = spawn_locks.acquire(&run_id).await;
    log::debug!("[session] fork_session: source run_id={}", run_id);

    // 1. Read source run metadata
    let source =
        storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;
    let session_id = source
        .session_id
        .clone()
        .ok_or_else(|| "No session_id available for fork".to_string())?;

    // 2. Stop source actor if alive
    let was_active = stop_actor(sessions.inner(), &run_id).await?;
    if was_active {
        log::debug!("[session] fork_session: stopped active source actor");
        let event = BusEvent::RunState {
            run_id: run_id.clone(),
            state: "stopped".to_string(),
            exit_code: None,
            error: None,
        };
        emit_bus_event(&app, &seq_counter, &run_id, &event);
        storage::runs::update_status(&run_id, RunStatus::Stopped, None, None).ok();
    }

    // 3. Create new run (audit #3: inherit remote_host_name + remote_cwd + snapshot + platform_id)
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut meta = storage::runs::create_run(
        &new_id,
        &source.prompt,
        &source.cwd,
        &source.agent,
        RunStatus::Pending,
        source.model.clone(),
        Some(run_id.clone()),
        source.remote_host_name.clone(),
        source.remote_cwd.clone(),
        source.remote_host_snapshot.clone(),
        source.platform_id.clone(),
    )?;
    log::debug!(
        "[session] fork_session: fork {} ← parent {}, remote={:?}",
        new_id,
        run_id,
        source.remote_host_name
    );

    // 4. Copy parent events
    storage::events::copy_bus_events(&run_id, &new_id)?;

    // 5. Set parent session_id on fork run
    meta.session_id = Some(session_id.clone());
    storage::runs::save_meta(&meta)?;

    // 6. Build adapter settings + resolve remote (audit #3)
    let agent_settings = storage::settings::get_agent_settings(&source.agent);
    let user_settings = storage::settings::get_user_settings();
    let adapter = adapter::build_adapter_settings(&agent_settings, &user_settings, None);
    let remote = resolve_remote_host(&source)?;
    let resolved =
        resolve_auth_env_for_platform(&remote, &user_settings, source.platform_id.as_deref());
    let effective_cwd = source.remote_cwd.as_deref().unwrap_or(&source.cwd);

    // 7. One-shot fork: get new session_id
    log::debug!(
        "[session] fork_session: starting fork_oneshot, source_sid={}, remote={:?}",
        session_id,
        remote.as_ref().map(|r| &r.name)
    );
    let new_session_id = match claude_stream::fork_oneshot(
        &session_id,
        effective_cwd,
        &adapter,
        remote.as_ref(),
        resolved.api_key.as_deref(),
        resolved.auth_token.as_deref(),
        resolved.base_url.as_deref(),
        resolved.default_model.as_deref(),
        resolved.extra_env.as_ref(),
    )
    .await
    {
        Ok(sid) => sid,
        Err(e) => {
            log::error!(
                "[session] fork_oneshot failed, cleaning up run {}: {}",
                new_id,
                e
            );
            storage::runs::update_status(&new_id, RunStatus::Failed, None, Some(e.clone()))?;
            return Err(e);
        }
    };
    log::debug!(
        "[session] fork_session: fork_oneshot returned new_sid={}",
        new_session_id
    );

    // 8. Persist new session_id
    meta.session_id = Some(new_session_id);
    storage::runs::save_meta(&meta)?;

    log::debug!(
        "[session] fork_session completed: {} → {} (frontend will start_session to connect)",
        run_id,
        new_id
    );
    Ok(new_id)
}

#[tauri::command]
pub async fn approve_session_tool(
    app: AppHandle,
    sessions: State<'_, ActorSessionMap>,
    seq_counter: State<'_, Arc<EventWriter>>,
    spawn_locks: State<'_, SpawnLocks>,
    cancel_token: State<'_, CancellationToken>,
    run_id: String,
    tool_name: String,
) -> Result<(), String> {
    let _guard = spawn_locks.acquire(&run_id).await;
    log::debug!(
        "[session] approve_session_tool: run_id={}, tool={}",
        run_id,
        tool_name
    );

    // 1. Read run metadata
    let meta =
        storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;

    // 2. Persist tool to agent allowed_tools
    let mut agent_settings = storage::settings::get_agent_settings(&meta.agent);
    if !agent_settings.allowed_tools.contains(&tool_name) {
        agent_settings.allowed_tools.push(tool_name.clone());
        let patch = serde_json::json!({
            "allowed_tools": agent_settings.allowed_tools,
        });
        storage::settings::update_agent_settings(&meta.agent, patch)?;
        log::debug!(
            "[session] added {} to allowed_tools for {}",
            tool_name,
            meta.agent
        );
    }

    // 3. Stop current actor
    stop_actor(sessions.inner(), &run_id).await?;

    // 4. Resolve remote + extract fields before consuming meta (audit #3)
    let remote = resolve_remote_host(&meta)?;
    let effective_cwd = meta.remote_cwd.clone().unwrap_or_else(|| meta.cwd.clone());
    let prompt = meta.prompt.clone();
    let session_id = meta
        .session_id
        .clone()
        .ok_or_else(|| "No session_id for continue".to_string())?;

    // 5. Rebuild adapter settings (now includes new tool) + resolve auth env
    let refreshed_agent = storage::settings::get_agent_settings(&meta.agent);
    let user = storage::settings::get_user_settings();
    let adapter = adapter::build_adapter_settings(&refreshed_agent, &user, None);
    let resolved = resolve_auth_env_for_platform(&remote, &user, meta.platform_id.as_deref());

    // 7. Emit spawning
    let spawning_event = BusEvent::RunState {
        run_id: run_id.clone(),
        state: "spawning".to_string(),
        exit_code: None,
        error: None,
    };
    emit_bus_event(&app, &seq_counter, &run_id, &spawning_event);
    storage::runs::update_status(&run_id, RunStatus::Running, None, None).ok();

    // 8. Spawn CLI with Continue mode (audit #3: SSH path if remote)
    let (child, stdin, stdout, stderr) = spawn_cli_process(
        &effective_cwd,
        &prompt,
        &adapter,
        &SessionMode::Continue,
        Some(&session_id),
        false,
        &[], // approve_session_tool: no attachments
        remote.as_ref(),
        Some(&effective_cwd),
        resolved.api_key.as_deref(),
        resolved.auth_token.as_deref(),
        resolved.base_url.as_deref(),
        &run_id,
        resolved.default_model.as_deref(),
        resolved.extra_env.as_ref(),
    )
    .await?;

    // 8. Compute turn baselines from existing events (1-based: next message gets total+1)
    let (total, normal) = crate::storage::events::count_user_messages(&run_id);
    log::debug!(
        "[session] approve: turn baselines total={}, normal={} → next=({}, {})",
        total,
        normal,
        total + 1,
        normal + 1
    );

    // 8b. Spawn actor
    let actor_handle = session_actor::spawn_actor(
        app.clone(),
        Arc::clone(&seq_counter),
        sessions.inner().clone(),
        run_id.clone(),
        child,
        stdin,
        stdout,
        stderr,
        true, // is_resume
        cancel_token.inner().clone(),
        total + 1,
        normal + 1,
    );
    sessions.lock().await.insert(run_id.clone(), actor_handle);

    // 9. Wait briefly for CLI to be ready.
    // TODO: Replace with event-based approach (WaitForReady actor command).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 10. Send retry guidance message via actor.
    // Turn Transaction Engine: actor's start_user_turn handles UserMessage + RunState(running).
    let retry_msg = format!(
        "The tool {} is now allowed. Please retry your previous action using this tool.",
        tool_name
    );
    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found after approve restart", run_id))?
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::SendMessage {
            text: retry_msg,
            attachments: Vec::new(),
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead after approve restart".to_string())?;
    reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    log::debug!(
        "[session] approve_session_tool completed for run_id={}",
        run_id
    );
    Ok(())
}

/// Respond to an inline permission prompt (--permission-prompt-tool stdio).
/// Writes a control_response back to CLI stdin via the actor.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn respond_permission(
    sessions: State<'_, ActorSessionMap>,
    run_id: String,
    request_id: String,
    behavior: String,
    updated_permissions: Option<Vec<serde_json::Value>>,
    updated_input: Option<serde_json::Value>,
    deny_message: Option<String>,
    interrupt: Option<bool>,
) -> Result<(), String> {
    log::debug!(
        "[session] respond_permission: run_id={}, req_id={}, behavior={}, updated_perms={}, has_updated_input={}, has_deny_message={}, interrupt={:?}",
        run_id,
        request_id,
        behavior,
        updated_permissions.as_ref().map_or(0, |v| v.len()),
        updated_input.is_some(),
        deny_message.is_some(),
        interrupt,
    );

    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found", run_id))?
    };

    // Build the response payload for Claude CLI.
    // CLI validates with Zod: allow requires `updatedInput` (record<string,unknown>),
    // deny requires `message` (string). Missing fields cause ZodError.
    let mut response = if behavior == "allow" {
        // updatedInput is REQUIRED by CLI schema — use provided value or empty object
        let input_val = updated_input.unwrap_or_else(|| serde_json::json!({}));
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": input_val,
        })
    } else {
        let msg = deny_message.unwrap_or_else(|| "User denied permission".to_string());
        let mut deny_obj = serde_json::json!({
            "behavior": "deny",
            "message": msg,
        });
        if interrupt == Some(true) {
            deny_obj["interrupt"] = serde_json::json!(true);
        }
        deny_obj
    };
    // Include updatedPermissions when allowing with suggestions (camelCase per CLI schema)
    if let Some(perms) = updated_permissions {
        if behavior == "allow" && !perms.is_empty() {
            response["updatedPermissions"] = serde_json::Value::Array(perms);
        }
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::RespondPermission {
            request_id: request_id.clone(),
            response,
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead".to_string())?;
    reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    log::debug!(
        "[session] respond_permission: delivered req_id={}",
        request_id
    );
    Ok(())
}

/// Respond to a hook callback control request (PreToolUse hooks only).
/// Writes a control_response back to CLI stdin via the actor.
#[tauri::command]
pub async fn respond_hook_callback(
    sessions: State<'_, ActorSessionMap>,
    run_id: String,
    request_id: String,
    decision: String, // "allow" or "deny"
) -> Result<(), String> {
    log::debug!(
        "[session] respond_hook_callback: run_id={}, req_id={}, decision={}",
        run_id,
        request_id,
        decision
    );

    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found", run_id))?
    };

    let response = serde_json::json!({ "decision": decision });

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::RespondHookCallback {
            request_id: request_id.clone(),
            response,
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead".to_string())?;
    reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    log::debug!(
        "[session] respond_hook_callback: delivered req_id={}",
        request_id
    );
    Ok(())
}

/// Cancel a pending control_request (top-level message type, not a control_request subtype).
#[tauri::command]
pub async fn cancel_control_request(
    sessions: State<'_, ActorSessionMap>,
    run_id: String,
    request_id: String,
) -> Result<(), String> {
    log::debug!(
        "[session] cancel_control_request: run_id={}, req_id={}",
        run_id,
        request_id
    );

    let cmd_tx = {
        let map = sessions.lock().await;
        map.get(&run_id)
            .map(|h| h.cmd_tx.clone())
            .ok_or_else(|| format!("Session {} not found", run_id))?
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(ActorCommand::CancelControlRequest {
            request_id,
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Actor dead".to_string())?;
    reply_rx
        .await
        .map_err(|_| "Actor dropped reply".to_string())??;

    Ok(())
}

// ── CLI process spawning (extracted from claude_stream.rs) ──

/// Spawn a Claude CLI process and return (Child, ChildStdin, ChildStdout, ChildStderr).
/// Sends the initial prompt via stdin for new sessions.
/// For remote sessions, wraps the CLI command in SSH.
#[allow(clippy::too_many_arguments)]
async fn spawn_cli_process(
    cwd: &str,
    prompt: &str,
    settings: &adapter::AdapterSettings,
    session_mode: &SessionMode,
    resume_session_id: Option<&str>,
    _is_new: bool,
    _initial_attachments: &[AttachmentData],
    remote_host: Option<&RemoteHost>,
    remote_cwd: Option<&str>,
    api_key: Option<&str>,
    auth_token: Option<&str>,
    base_url: Option<&str>,
    _run_id: &str,
    default_model: Option<&str>,
    extra_env: Option<&std::collections::HashMap<String, String>>,
) -> Result<
    (
        tokio::process::Child,
        tokio::process::ChildStdin,
        tokio::process::ChildStdout,
        tokio::process::ChildStderr,
    ),
    String,
> {
    // Build CLI args (shared between local and remote)
    let mut claude_args: Vec<String> = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];

    // Session mode args
    match session_mode {
        SessionMode::Resume | SessionMode::Continue => {
            let sid = resume_session_id.ok_or("session_id required for resume/continue")?;
            claude_args.push("--resume".into());
            claude_args.push(sid.into());
        }
        SessionMode::Fork => {
            return Err("Fork mode not supported in spawn_cli_process — use fork_oneshot()".into());
        }
        SessionMode::New => {}
    }

    // Settings flags
    let flag_args = adapter::build_settings_args(settings, false);
    claude_args.extend(flag_args.iter().cloned());
    if settings.include_partial_messages {
        claude_args.push("--include-partial-messages".into());
    }

    log::debug!(
        "[session] session_mode={:?}, resume_id={:?}, flag_args={:?}, remote={:?}",
        session_mode,
        resume_session_id,
        flag_args,
        remote_host.map(|r| &r.name),
    );

    let mut child = if let Some(remote) = remote_host {
        // SSH branch: wrap claude command in ssh
        let effective_remote_cwd = remote_cwd.unwrap_or(cwd);
        let remote_cmd = crate::agent::ssh::build_remote_claude_command(
            remote,
            effective_remote_cwd,
            &claude_args,
            api_key,
            auth_token,
            base_url,
            default_model,
            extra_env,
        );
        let mut ssh_cmd = crate::agent::ssh::build_ssh_command(remote, &remote_cmd);
        ssh_cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        log::debug!(
            "[session] spawning remote CLI via SSH: {}@{}, cwd={}",
            remote.user,
            remote.host,
            effective_remote_cwd
        );

        ssh_cmd.spawn().map_err(|e| {
            log::error!("[session] Failed to spawn ssh: {}", e);
            format!("Failed to spawn ssh: {}", e)
        })?
    } else {
        // Local branch: existing logic
        let claude_bin = claude_stream::resolve_claude_path();
        log::debug!("[session] resolved binary: {}", claude_bin);

        let mut cmd = tokio::process::Command::new(&claude_bin);
        for arg in &claude_args {
            cmd.arg(arg);
        }

        let path_env = claude_stream::augmented_path();
        log::debug!("[session] PATH: {}", path_env);
        log::debug!(
            "[session] cwd: {}, prompt: {:?}",
            cwd,
            truncate_str(prompt, 80)
        );
        cmd.current_dir(cwd)
            .env("PATH", &path_env)
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Pass API key to CLI when using API Key authentication mode (x-api-key header).
        // MUST remove AUTH_TOKEN to avoid inherited shell env vars taking priority.
        // Use env_remove (not empty string) — CLI may treat empty as "set but invalid".
        if let Some(key) = api_key {
            log::debug!("[session] setting ANTHROPIC_API_KEY env for local CLI");
            cmd.env("ANTHROPIC_API_KEY", key);
            cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        }

        // Pass auth token for third-party platforms using Bearer auth.
        // MUST remove API_KEY to avoid inherited shell env vars causing conflicts.
        if let Some(token) = auth_token {
            log::debug!("[session] setting ANTHROPIC_AUTH_TOKEN env for local CLI");
            cmd.env("ANTHROPIC_AUTH_TOKEN", token);
            cmd.env_remove("ANTHROPIC_API_KEY");
        }

        // Pass Base URL for third-party API endpoints
        if let Some(url) = base_url {
            log::debug!("[session] setting ANTHROPIC_BASE_URL={}", url);
            cmd.env("ANTHROPIC_BASE_URL", url);
        }

        // Pass default model for third-party platforms (low priority — --model flag overrides)
        if let Some(model) = default_model {
            log::debug!("[session] setting ANTHROPIC_MODEL={}", model);
            cmd.env("ANTHROPIC_MODEL", model);
            cmd.env("ANTHROPIC_DEFAULT_HAIKU_MODEL", model);
            cmd.env("ANTHROPIC_DEFAULT_SONNET_MODEL", model);
            cmd.env("ANTHROPIC_DEFAULT_OPUS_MODEL", model);
        }

        // Pass extra env vars for third-party platforms (e.g. API_TIMEOUT_MS for DeepSeek)
        if let Some(extra) = extra_env {
            for (k, v) in extra {
                log::debug!("[session] setting extra env {}={}", k, v);
                cmd.env(k, v);
            }
        }

        cmd.spawn().map_err(|e| {
            log::error!("[session] Failed to spawn claude: {}", e);
            format!("Failed to spawn claude: {}", e)
        })?
    };
    log::debug!("[session] child process spawned, pid={:?}", child.id());

    let stdin = child.stdin.take().ok_or("Failed to capture claude stdin")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture claude stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Failed to capture claude stderr")?;

    // Initial prompt is now sent via ActorCommand::SendMessage after actor spawn.
    // This ensures ALL user messages go through the Turn Transaction Engine.

    Ok((child, stdin, stdout, stderr))
}

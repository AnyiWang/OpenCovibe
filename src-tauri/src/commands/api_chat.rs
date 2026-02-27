use crate::api_client::executor::{run_api_agent, PermissionDecision, PermissionMap};
use crate::models::RunStatus;
use crate::storage;
use tauri::Emitter;

#[tauri::command]
pub async fn send_api_message(
    app: tauri::AppHandle,
    permission_map: tauri::State<'_, PermissionMap>,
    run_id: String,
    message: String,
    model: Option<String>,
) -> Result<(), String> {
    let run = storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;

    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("message is required".to_string());
    }

    let settings = storage::settings::get_user_settings();

    // Resolve API key from run's platform credential, fallback to global
    let api_key = if let Some(ref pid) = run.platform_id {
        settings
            .platform_credentials
            .iter()
            .find(|c| c.platform_id == *pid)
            .and_then(|c| c.api_key.clone())
            .filter(|k| !k.is_empty())
            .or_else(|| {
                settings
                    .anthropic_api_key
                    .as_ref()
                    .filter(|k| !k.is_empty())
                    .cloned()
            })
    } else {
        settings
            .anthropic_api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .cloned()
    }
    .ok_or_else(|| "No API key configured. Go to Settings to add one.".to_string())?;

    // Resolve base_url from run's platform credential, fallback to global
    let base_url = if let Some(ref pid) = run.platform_id {
        settings
            .platform_credentials
            .iter()
            .find(|c| c.platform_id == *pid)
            .and_then(|c| c.base_url.clone())
            .filter(|s| !s.is_empty())
            .or(settings.anthropic_base_url)
    } else {
        settings.anthropic_base_url
    };

    let agent_settings = storage::settings::get_agent_settings(&run.agent);
    let effective_model = model
        .filter(|m| !m.is_empty())
        .or(agent_settings.model)
        .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());

    let permission_mode = settings.permission_mode;
    let pm = permission_map.inner().clone();
    let app_clone = app.clone();
    let run_id_clone = run_id.clone();
    let cwd = run.cwd.clone();

    log::debug!(
        "[api_chat] starting API agent: run={}, model={}, cwd={}, base_url={:?}",
        run_id,
        effective_model,
        cwd,
        base_url
    );

    tokio::spawn(async move {
        if let Err(e) = run_api_agent(
            app_clone.clone(),
            pm,
            run_id_clone.clone(),
            api_key,
            effective_model,
            message,
            cwd,
            permission_mode,
            base_url,
        )
        .await
        {
            log::error!("[api_chat] agent error: {}", e);
            if let Err(e2) = storage::runs::update_status(
                &run_id_clone,
                RunStatus::Failed,
                Some(1),
                Some(e.clone()),
            ) {
                log::warn!("[api_chat] failed to update status to Failed: {}", e2);
            }
            let _ = app_clone.emit(
                "chat-done",
                crate::models::ChatDone {
                    ok: false,
                    code: 1,
                    error: Some(e.clone()),
                },
            );
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn resolve_permission(
    permission_map: tauri::State<'_, PermissionMap>,
    request_id: String,
    decision: String,
) -> Result<(), String> {
    let mut map = permission_map.lock().await;
    if let Some(sender) = map.remove(&request_id) {
        let d = if decision == "allow" {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny
        };
        let _ = sender.send(d);
        Ok(())
    } else {
        Err(format!("No pending permission request: {}", request_id))
    }
}

#[tauri::command]
pub async fn stop_api_agent(run_id: String) -> Result<(), String> {
    // For API mode, we just mark the run as stopped.
    // The agent loop will stop when it checks next.
    if let Err(e) = storage::runs::update_status(
        &run_id,
        RunStatus::Stopped,
        None,
        Some("Stopped by user".to_string()),
    ) {
        log::warn!("[api_chat] failed to update status to Stopped: {}", e);
    }
    Ok(())
}

use crate::agent::pty::PtyMap;
use crate::agent::spawn::build_agent_command;
use crate::models::RunStatus;
use crate::storage;

#[tauri::command]
pub async fn spawn_pty(
    app: tauri::AppHandle,
    pty_map: tauri::State<'_, PtyMap>,
    run_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    log::debug!(
        "[pty_cmd] spawn_pty: run_id={}, rows={}, cols={}",
        run_id,
        rows,
        cols
    );
    let run = storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;

    let agent_settings = storage::settings::get_agent_settings(&run.agent);
    let user_settings = storage::settings::get_user_settings();
    let adapter_settings =
        crate::agent::adapter::build_adapter_settings(&agent_settings, &user_settings, None);

    // Pass the run's initial prompt as CLI argument so claude starts with it
    let initial_prompt = run.prompt.clone();
    let (cmd, args) = build_agent_command(
        &run.agent,
        &initial_prompt,
        &adapter_settings,
        false, // interactive mode: don't add --print
    )?;

    // Log initial user message
    if let Err(e) = storage::events::append_event(
        &run_id,
        crate::models::RunEventType::User,
        serde_json::json!({
            "text": initial_prompt,
            "source": "ui_chat"
        }),
    ) {
        log::warn!("[pty_cmd] failed to log user event: {}", e);
    }

    // Update run status
    if let Err(e) = storage::runs::update_status(&run_id, RunStatus::Running, None, None) {
        log::warn!("[pty_cmd] failed to update status to Running: {}", e);
    }

    crate::agent::pty::spawn_pty_session(app, &pty_map, &run_id, &cmd, &args, &run.cwd, rows, cols)
}

#[tauri::command]
pub fn write_pty(
    pty_map: tauri::State<'_, PtyMap>,
    run_id: String,
    data: String,
) -> Result<(), String> {
    log::debug!(
        "[pty_cmd] write_pty: run_id={}, data_len={}",
        run_id,
        data.len()
    );
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|e| format!("Invalid base64: {}", e))?;
    crate::agent::pty::write_to_pty(&pty_map, &run_id, &bytes)
}

#[tauri::command]
pub fn resize_pty(
    pty_map: tauri::State<'_, PtyMap>,
    run_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    log::debug!(
        "[pty_cmd] resize_pty: run_id={}, rows={}, cols={}",
        run_id,
        rows,
        cols
    );
    crate::agent::pty::resize_pty(&pty_map, &run_id, rows, cols)
}

#[tauri::command]
pub fn close_pty(pty_map: tauri::State<'_, PtyMap>, run_id: String) -> Result<bool, String> {
    log::debug!("[pty_cmd] close_pty: run_id={}", run_id);
    crate::agent::pty::close_pty(&pty_map, &run_id)
}

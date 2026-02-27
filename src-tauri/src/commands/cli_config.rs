use crate::storage::cli_config;
use serde_json::Value;

#[tauri::command]
pub fn get_cli_config() -> Result<Value, String> {
    log::debug!("[cli_config] get_cli_config");
    Ok(cli_config::load_cli_config())
}

#[tauri::command]
pub fn get_project_cli_config(cwd: String) -> Result<Value, String> {
    log::debug!("[cli_config] get_project_cli_config cwd={}", cwd);
    Ok(cli_config::load_project_cli_config(&cwd))
}

#[tauri::command]
pub fn update_cli_config(patch: Value) -> Result<Value, String> {
    log::debug!("[cli_config] update_cli_config patch={}", patch);
    cli_config::update_cli_config(patch)
}

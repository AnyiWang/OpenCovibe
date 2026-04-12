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

// ── Codex config commands ──

/// Returns { config: {}, warning?: string }
#[tauri::command]
pub fn get_codex_config() -> Result<Value, String> {
    log::debug!("[cli_config] get_codex_config");
    let (config, warning) = cli_config::load_codex_config();
    let mut result = serde_json::Map::new();
    result.insert("config".to_string(), config);
    if let Some(w) = warning {
        result.insert("warning".to_string(), Value::String(w));
    }
    Ok(Value::Object(result))
}

#[tauri::command]
pub fn get_project_codex_config(cwd: String) -> Result<Value, String> {
    log::debug!("[cli_config] get_project_codex_config cwd={}", cwd);
    Ok(cli_config::load_project_codex_config(&cwd))
}

#[tauri::command]
pub fn update_codex_config(patch: Value) -> Result<Value, String> {
    log::debug!("[cli_config] update_codex_config patch={}", patch);
    cli_config::update_codex_config(patch)
}

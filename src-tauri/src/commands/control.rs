use crate::agent::codex_control::{self, CodexInfoCache};
use crate::agent::control::{self, CliInfoCache};
use crate::models::{CliInfo, CodexModelList};
use tauri::State;

#[tauri::command]
pub async fn get_cli_info(
    cache: State<'_, CliInfoCache>,
    force_refresh: Option<bool>,
) -> Result<CliInfo, String> {
    log::debug!(
        "[control] get_cli_info IPC, force={}",
        force_refresh.unwrap_or(false)
    );
    match control::get_cli_info(&cache, force_refresh.unwrap_or(false)).await {
        Ok(info) => Ok(info),
        Err(e) => {
            log::warn!(
                "[control] CLI info failed ({}): {}, using fallback",
                e.code,
                e.message
            );
            Ok(control::fallback_cli_info())
        }
    }
}

#[tauri::command]
pub async fn get_codex_models(
    cache: State<'_, CodexInfoCache>,
    force_refresh: Option<bool>,
) -> Result<CodexModelList, String> {
    log::debug!(
        "[control] get_codex_models IPC, force={}",
        force_refresh.unwrap_or(false)
    );
    match codex_control::get_codex_models(&cache, force_refresh.unwrap_or(false)).await {
        Ok(list) => Ok(list),
        Err(e) => {
            log::warn!(
                "[control] codex models failed ({}): {}, using fallback",
                e.code,
                e.message
            );
            Ok(codex_control::fallback_models())
        }
    }
}

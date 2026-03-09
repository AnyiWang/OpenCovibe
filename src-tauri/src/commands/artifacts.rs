use crate::models::RunArtifact;
use crate::storage;

#[tauri::command]
pub fn get_run_artifacts(id: String) -> Result<RunArtifact, String> {
    log::debug!("[artifacts] get_run_artifacts: id={}", id);
    storage::runs::get_run(&id).ok_or_else(|| format!("Run {} not found", id))?;
    Ok(storage::artifacts::get_artifact(&id))
}

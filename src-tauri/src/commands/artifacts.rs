use crate::models::RunArtifact;
use crate::storage;

#[tauri::command]
pub fn get_run_artifacts(id: String) -> RunArtifact {
    log::debug!("[artifacts] get_run_artifacts: id={}", id);
    storage::artifacts::get_artifact(&id)
}

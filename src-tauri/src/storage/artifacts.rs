use crate::models::{now_iso, RunArtifact};
use std::fs;

fn artifacts_path(run_id: &str) -> std::path::PathBuf {
    super::run_dir(run_id).join("artifacts.json")
}

pub fn get_artifact(run_id: &str) -> RunArtifact {
    let path = artifacts_path(run_id);
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<RunArtifact>(&content) {
                Ok(artifact) => return artifact,
                // Derived/regenerable data, so a fresh empty is safe — but surface
                // the corruption instead of swallowing it silently. (audit #9)
                Err(e) => log::warn!(
                    "[artifacts] corrupt artifacts.json for run {}, regenerating: {}",
                    run_id,
                    e
                ),
            }
        }
    }
    RunArtifact {
        task_id: run_id.to_string(),
        files_changed: vec![],
        diff_summary: String::new(),
        commands: vec![],
        cost_estimate: None,
        updated_at: now_iso(),
    }
}

pub fn save_artifact(artifact: &RunArtifact) -> Result<(), String> {
    let dir = super::run_dir(&artifact.task_id);
    super::ensure_dir(&dir).map_err(|e| e.to_string())?;
    let path = artifacts_path(&artifact.task_id);
    let json = serde_json::to_string_pretty(artifact).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

use crate::models::RunEvent;
use crate::storage;

#[tauri::command]
pub fn get_run_events(id: String, since_seq: Option<u64>) -> Result<Vec<RunEvent>, String> {
    log::debug!(
        "[events] get_run_events: id={}, since_seq={:?}",
        id,
        since_seq
    );
    storage::runs::get_run(&id).ok_or_else(|| format!("Run {} not found", id))?;
    Ok(storage::events::list_events(&id, since_seq.unwrap_or(0)))
}

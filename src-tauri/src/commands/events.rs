use crate::models::RunEvent;
use crate::storage;

#[tauri::command]
pub fn get_run_events(id: String, since_seq: Option<u64>) -> Vec<RunEvent> {
    log::debug!(
        "[events] get_run_events: id={}, since_seq={:?}",
        id,
        since_seq
    );
    storage::events::list_events(&id, since_seq.unwrap_or(0))
}

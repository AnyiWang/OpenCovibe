use crate::storage::{self, history};

#[tauri::command]
pub async fn get_history_summary(
    run_id: String,
    refresh: Option<bool>,
) -> Result<history::HistorySummary, String> {
    storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {run_id} not found"))?;
    let refresh = refresh.unwrap_or(false);
    log::debug!(
        "[history] get_summary: run_id={}, refresh={}",
        run_id,
        refresh
    );
    tokio::task::spawn_blocking(move || history::get_summary(&run_id, refresh))
        .await
        .map_err(|e| format!("history build task failed: {e}"))?
}

#[tauri::command]
pub async fn get_history_page(
    run_id: String,
    generation_id: Option<String>,
    before_cursor: Option<String>,
) -> Result<history::HistoryPage, String> {
    storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {run_id} not found"))?;
    log::debug!(
        "[history] get_page: run_id={}, before_cursor={:?}",
        run_id,
        before_cursor
    );
    tokio::task::spawn_blocking(move || {
        history::get_page(&run_id, generation_id.as_deref(), before_cursor.as_deref())
    })
    .await
    .map_err(|e| format!("history page task failed: {e}"))?
}

#[tauri::command]
pub async fn get_history_content_chunk(
    run_id: String,
    generation_id: String,
    content_id: String,
    offset: u64,
    max_bytes: usize,
) -> Result<history::ContentChunk, String> {
    storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {run_id} not found"))?;
    log::debug!(
        "[history] get_content_chunk: run_id={}, generation={}, content={}, offset={}, max_bytes={}",
        run_id,
        generation_id,
        content_id,
        offset,
        max_bytes
    );
    tokio::task::spawn_blocking(move || {
        history::get_content_chunk(&run_id, &generation_id, &content_id, offset, max_bytes)
    })
    .await
    .map_err(|e| format!("history content task failed: {e}"))?
}

#[tauri::command]
pub async fn get_subhistory_page(
    run_id: String,
    generation_id: String,
    sub_history_id: String,
    before_cursor: Option<String>,
) -> Result<history::SubHistoryPage, String> {
    storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {run_id} not found"))?;
    log::debug!(
        "[history] get_subhistory_page: run_id={}, generation={}, subhistory={}",
        run_id,
        generation_id,
        sub_history_id
    );
    tokio::task::spawn_blocking(move || {
        history::get_subhistory_page(
            &run_id,
            &generation_id,
            &sub_history_id,
            before_cursor.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("subhistory page task failed: {e}"))?
}

//! IPC commands for CLI session discovery, import, and sync.

use crate::storage::cli_sessions::{self, CliSessionSummary, ImportResult, SyncResult};
use crate::storage::events::EventWriter;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn discover_cli_sessions(cwd: String) -> Result<Vec<CliSessionSummary>, String> {
    let start = std::time::Instant::now();
    log::debug!("[cli_sync] discover_cli_sessions: cwd={}", cwd);

    let result = tokio::task::spawn_blocking(move || cli_sessions::discover_sessions(&cwd))
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?;

    log::debug!(
        "[cli_sync] discover_cli_sessions: done in {:?}",
        start.elapsed()
    );
    result
}

#[tauri::command]
pub async fn import_cli_session(
    session_id: String,
    cwd: String,
    event_writer: State<'_, Arc<EventWriter>>,
) -> Result<ImportResult, String> {
    let start = std::time::Instant::now();
    log::debug!(
        "[cli_sync] import_cli_session: session_id={}, cwd={}",
        session_id,
        cwd
    );

    let writer = event_writer.inner().clone();
    let result = tokio::task::spawn_blocking(move || {
        cli_sessions::import_session(&session_id, &cwd, writer)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {}", e))?;

    log::debug!(
        "[cli_sync] import_cli_session: done in {:?}",
        start.elapsed()
    );
    result
}

#[tauri::command]
pub async fn sync_cli_session(
    run_id: String,
    event_writer: State<'_, Arc<EventWriter>>,
) -> Result<SyncResult, String> {
    let start = std::time::Instant::now();
    log::debug!("[cli_sync] sync_cli_session: run_id={}", run_id);

    let writer = event_writer.inner().clone();
    let result = tokio::task::spawn_blocking(move || cli_sessions::sync_session(&run_id, writer))
        .await
        .map_err(|e| format!("spawn_blocking: {}", e))?;

    log::debug!("[cli_sync] sync_cli_session: done in {:?}", start.elapsed());
    result
}

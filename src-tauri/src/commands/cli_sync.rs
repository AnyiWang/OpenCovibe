//! IPC commands for CLI session discovery, import, and sync.
//!
//! Dispatches by `agent` parameter (defaulting to "claude" for backward
//! compatibility with older clients that don't pass the field).

use crate::storage::cli_sessions_common::{DiscoverResult, ImportResult, SyncResult};
use crate::storage::events::EventWriter;
use crate::storage::{cli_sessions, codex_sessions};
use std::sync::Arc;
use tauri::State;

fn agent_or_default(agent: Option<String>) -> String {
    agent.unwrap_or_else(|| "claude".to_string())
}

#[tauri::command]
pub async fn discover_cli_sessions(
    cwd: String,
    agent: Option<String>,
) -> Result<DiscoverResult, String> {
    let start = std::time::Instant::now();
    let agent = agent_or_default(agent);
    log::debug!(
        "[cli_sync] discover_cli_sessions: agent={}, cwd={}",
        agent,
        cwd
    );

    let result = tokio::task::spawn_blocking(move || match agent.as_str() {
        "codex" => codex_sessions::discover_sessions(&cwd),
        _ => cli_sessions::discover_sessions(&cwd),
    })
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
    agent: Option<String>,
    event_writer: State<'_, Arc<EventWriter>>,
) -> Result<ImportResult, String> {
    let start = std::time::Instant::now();
    let agent = agent_or_default(agent);
    log::debug!(
        "[cli_sync] import_cli_session: agent={}, session_id={}, cwd={}",
        agent,
        session_id,
        cwd
    );

    let writer = event_writer.inner().clone();
    let result = tokio::task::spawn_blocking(move || match agent.as_str() {
        "codex" => codex_sessions::import_session(&session_id, &cwd, writer),
        _ => cli_sessions::import_session(&session_id, &cwd, writer),
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

    // Dispatch from RunMeta.agent rather than a frontend parameter — the run's
    // origin agent is the authoritative source.
    let meta = crate::storage::runs::get_run(&run_id)
        .ok_or_else(|| format!("run {} not found", run_id))?;
    let agent = meta.agent.clone();

    let writer = event_writer.inner().clone();
    let result = tokio::task::spawn_blocking(move || match agent.as_str() {
        "codex" => codex_sessions::sync_session(&run_id, writer),
        _ => cli_sessions::sync_session(&run_id, writer),
    })
    .await
    .map_err(|e| format!("spawn_blocking: {}", e))?;

    log::debug!("[cli_sync] sync_cli_session: done in {:?}", start.elapsed());
    result
}

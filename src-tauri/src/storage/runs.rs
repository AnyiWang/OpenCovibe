use crate::models::{now_iso, RunMeta, RunStatus, TaskRun};
use std::fs;

#[allow(clippy::too_many_arguments)]
pub fn create_run(
    id: &str,
    prompt: &str,
    cwd: &str,
    agent: &str,
    status: RunStatus,
    model: Option<String>,
    parent_run_id: Option<String>,
    remote_host_name: Option<String>,
    remote_cwd: Option<String>,
    remote_host_snapshot: Option<crate::models::RemoteHost>,
    platform_id: Option<String>,
) -> Result<RunMeta, String> {
    log::debug!(
        "[storage/runs] create_run: id={}, agent={}, model={:?}, parent={:?}, remote={:?}, platform={:?}, prompt_len={}",
        id,
        agent,
        model,
        parent_run_id,
        remote_host_name,
        platform_id,
        prompt.len()
    );
    let dir = super::run_dir(id);
    super::ensure_dir(&dir).map_err(|e| e.to_string())?;

    let settings = super::settings::get_user_settings();

    // Use explicit platform_id if provided, otherwise fall back to global active
    let resolved_pid = platform_id.or_else(|| settings.active_platform_id.clone());

    // Resolve base_url: prefer credential's base_url for the platform, fallback to global
    let resolved_base_url = resolved_pid
        .as_ref()
        .and_then(|pid| {
            settings
                .platform_credentials
                .iter()
                .find(|c| c.platform_id == *pid)
        })
        .and_then(|c| c.base_url.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| settings.anthropic_base_url.clone());

    let meta = RunMeta {
        id: id.to_string(),
        prompt: prompt.to_string(),
        cwd: cwd.to_string(),
        agent: agent.to_string(),
        auth_mode: settings.auth_mode,
        status,
        started_at: now_iso(),
        ended_at: None,
        exit_code: None,
        error_message: None,
        session_id: None,
        result_subtype: None,
        model,
        parent_run_id,
        name: None,
        remote_host_name,
        remote_cwd,
        remote_host_snapshot,
        platform_id: resolved_pid,
        platform_base_url: resolved_base_url,
        source: None,
        cli_import_watermark: None,
        cli_session_path: None,
        cli_usage_incomplete: None,
    };

    save_meta(&meta)?;
    Ok(meta)
}

pub fn save_meta(meta: &RunMeta) -> Result<(), String> {
    let dir = super::run_dir(&meta.id);
    super::ensure_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("meta.json");
    let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

pub fn get_run(id: &str) -> Option<RunMeta> {
    let path = super::run_dir(id).join("meta.json");
    if !path.exists() {
        log::debug!("[storage/runs] get_run: id={} not found", id);
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn update_session_id(id: &str, session_id: &str) -> Result<(), String> {
    if session_id.is_empty() {
        log::debug!("[storage/runs] update_session_id: empty session_id, skipping");
        return Ok(());
    }
    let mut meta = get_run(id).ok_or_else(|| format!("Run {} not found", id))?;
    if meta.session_id.as_deref() == Some(session_id) {
        log::debug!("[storage/runs] update_session_id: unchanged, skipping");
        return Ok(());
    }
    log::debug!(
        "[storage/runs] update_session_id: id={}, old={:?}, new={}",
        id,
        meta.session_id,
        session_id
    );
    meta.session_id = Some(session_id.to_string());
    save_meta(&meta)
}

pub fn rename_run(id: &str, name: &str) -> Result<(), String> {
    log::debug!("[storage/runs] rename_run: id={}, name={}", id, name);
    let mut meta = get_run(id).ok_or_else(|| format!("Run {} not found", id))?;
    meta.name = if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    };
    save_meta(&meta)
}

pub fn update_run_model(id: &str, model: &str) -> Result<(), String> {
    log::debug!(
        "[storage/runs] update_run_model: id={}, model={}",
        id,
        model
    );
    let mut meta = get_run(id).ok_or_else(|| format!("Run {} not found", id))?;
    meta.model = Some(model.to_string());
    save_meta(&meta)
}

pub fn update_status(
    id: &str,
    status: RunStatus,
    exit_code: Option<i32>,
    error_message: Option<String>,
) -> Result<(), String> {
    log::debug!(
        "[storage/runs] update_status: id={}, status={:?}, exit_code={:?}",
        id,
        status,
        exit_code
    );
    let mut meta = get_run(id).ok_or_else(|| format!("Run {} not found", id))?;
    meta.status = status.clone();
    let is_terminal = matches!(
        status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Stopped
    );
    if is_terminal {
        meta.ended_at = Some(now_iso());
    } else {
        // Clear stale ended_at when re-activating (e.g. resume after orphan recovery)
        meta.ended_at = None;
    }
    meta.exit_code = exit_code;
    meta.error_message = error_message;
    save_meta(&meta)
}

/// Persist only error_message and result_subtype without changing status or ended_at.
/// Used in the event loop when a `result` error arrives — the process may still be running,
/// so we only save the error info. EOF cleanup later sets the terminal status.
pub fn persist_result_error(
    id: &str,
    error_message: Option<String>,
    result_subtype: Option<String>,
) -> Result<(), String> {
    log::debug!(
        "[storage/runs] persist_result_error: id={}, error={:?}, subtype={:?}",
        id,
        error_message,
        result_subtype
    );
    let mut meta = get_run(id).ok_or_else(|| format!("Run {} not found", id))?;
    meta.error_message = error_message;
    meta.result_subtype = result_subtype;
    save_meta(&meta)
}

pub fn list_runs() -> Vec<TaskRun> {
    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        return vec![];
    }

    let mut runs: Vec<TaskRun> = vec![];
    if let Ok(entries) = fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let meta_path = entry.path().join("meta.json");
            if !meta_path.exists() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<RunMeta>(&content) {
                    // Compute summary from events
                    let events_path = entry.path().join("events.jsonl");
                    let (last_activity, msg_count, last_preview) = summarize_events(&events_path);

                    // Skip runs with no events that aren't running/pending
                    if msg_count == 0
                        && !matches!(meta.status, RunStatus::Running | RunStatus::Pending)
                    {
                        // Still include if recent (within last hour)
                        if let Ok(started) = chrono::DateTime::parse_from_rfc3339(&meta.started_at)
                        {
                            let age = chrono::Utc::now().signed_duration_since(started);
                            if age.num_hours() > 1 {
                                continue;
                            }
                        }
                    }

                    runs.push(meta.to_task_run(last_activity, Some(msg_count), last_preview));
                }
            }
        }
    }

    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs
}

fn summarize_events(events_path: &std::path::Path) -> (Option<String>, u32, Option<String>) {
    if !events_path.exists() {
        return (None, 0, None);
    }
    let content = match fs::read_to_string(events_path) {
        Ok(c) => c,
        Err(_) => return (None, 0, None),
    };

    // Count non-empty lines for msg_count (cheap string scan — no JSON parsing)
    let total_lines = content.lines().filter(|l| !l.trim().is_empty()).count() as u32;

    // For last_activity + last_preview: only parse last few lines (most recent events)
    let mut last_ts: Option<String> = None;
    let mut msg_count: u32 = 0;
    let mut last_preview: Option<String> = None;

    // Collect last N non-empty lines and count messages only in those
    // For full msg_count, count user_message/message_complete across entire file cheaply
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Cheap substring check before full JSON parse for msg_count
        if line.contains("\"user_message\"")
            || line.contains("\"message_complete\"")
            || line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
        {
            msg_count += 1;
        }
    }

    // Parse only last 5 lines for timestamp + preview
    let last_lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail_start = if last_lines.len() > 5 {
        last_lines.len() - 5
    } else {
        0
    };
    for line in &last_lines[tail_start..] {
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
            if event.get("_bus").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(ts) = event.get("ts").and_then(|v| v.as_str()) {
                    last_ts = Some(ts.to_string());
                }
                if let Some(inner) = event.get("event") {
                    let inner_type = inner.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if inner_type == "user_message" || inner_type == "message_complete" {
                        if let Some(text) = inner.get("text").and_then(|t| t.as_str()) {
                            last_preview = Some(truncate_preview(text));
                        }
                    }
                }
            } else {
                if let Some(ts) = event.get("timestamp").and_then(|v| v.as_str()) {
                    last_ts = Some(ts.to_string());
                }
                let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if event_type == "user" || event_type == "assistant" {
                    if let Some(text) = event
                        .get("payload")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        last_preview = Some(truncate_preview(text));
                    }
                }
            }
        }
    }

    let _ = total_lines; // available for future use
    (last_ts, msg_count, last_preview)
}

fn truncate_preview(text: &str) -> String {
    if text.chars().count() > 100 {
        let end = text
            .char_indices()
            .nth(100)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        format!("{}...", &text[..end])
    } else {
        text.to_string()
    }
}

/// Return all run metadata (for stats aggregation)
pub fn list_all_run_metas() -> Vec<RunMeta> {
    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        return vec![];
    }
    let mut metas = vec![];
    if let Ok(entries) = fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if !meta_path.exists() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<RunMeta>(&content) {
                    metas.push(meta);
                }
            }
        }
    }
    metas
}

/// Reconcile any "running" runs that survived a crash,
/// and migrate old runs missing auth_mode.
pub fn reconcile_orphaned_runs() {
    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        return;
    }
    if let Ok(entries) = fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if !meta_path.exists() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&meta_path) {
                if let Ok(mut meta) = serde_json::from_str::<RunMeta>(&content) {
                    let mut dirty = false;

                    if meta.status == RunStatus::Running {
                        meta.status = RunStatus::Stopped;
                        meta.ended_at = Some(now_iso());
                        meta.error_message = Some("Recovered after app restart".to_string());
                        dirty = true;
                    }

                    // Pending = start_run created meta but start_session never completed.
                    // On restart these are orphans — mark Failed so they don't linger in nav.
                    if meta.status == RunStatus::Pending {
                        meta.status = RunStatus::Failed;
                        meta.ended_at = Some(now_iso());
                        meta.error_message = Some("Session never started".to_string());
                        dirty = true;
                        log::debug!(
                            "[storage/runs] reconcile: pending orphan {} -> failed",
                            meta.id
                        );
                    }

                    if dirty {
                        let _ = save_meta(&meta);
                    }
                }
            }
        }
    }
}

use crate::agent::adapter::ActorSessionMap;
use crate::models::{ExecutionPath, PromptFavorite, PromptSearchResult, RunStatus, TaskRun};
use crate::storage;
use std::collections::{HashMap, HashSet};

/// Validate that agent supports the requested execution path.
/// Both supported agents now accept either path (Claude: stream-json/--print;
/// Codex: app-server/exec), so this only rejects unknown agents.
fn validate_agent_path(agent: &str, _path: &ExecutionPath) -> Result<(), String> {
    match agent {
        "claude" => Ok(()), // Claude supports both session_actor and pipe_exec
        // Codex supports pipe_exec (legacy `codex exec`) and session_actor
        // (`codex app-server` bidirectional transport — interactive tools).
        "codex" => Ok(()),
        _ => Err(format!(
            "unknown agent '{}': supported agents are 'claude' and 'codex'",
            agent
        )),
    }
}

#[tauri::command]
pub async fn list_runs() -> Result<Vec<TaskRun>, String> {
    let runs = tokio::task::spawn_blocking(storage::runs::list_runs)
        .await
        .map_err(|e| format!("list_runs task failed: {}", e))?;
    log::debug!("[runs] list_runs: count={}", runs.len());
    Ok(runs)
}

#[tauri::command]
pub fn get_run(id: String) -> Result<TaskRun, String> {
    log::debug!("[runs] get_run: id={}", id);
    let meta = storage::runs::get_run(&id).ok_or_else(|| format!("Run {} not found", id))?;
    let events = storage::events::list_events(&id, 0);
    let mut msg_count: u32 = 0;
    let mut last_ts: Option<String> = None;
    let mut last_preview: Option<String> = None;
    for e in &events {
        last_ts = Some(e.timestamp.clone());
        let t = format!("{}", e.event_type);
        if t == "user" || t == "assistant" {
            msg_count += 1;
            if let Some(text) = e.payload.get("text").and_then(|v| v.as_str()) {
                let preview = if text.chars().count() > 100 {
                    let end: usize = text
                        .char_indices()
                        .nth(100)
                        .map(|(i, _)| i)
                        .unwrap_or(text.len());
                    format!("{}...", &text[..end])
                } else {
                    text.to_string()
                };
                last_preview = Some(preview);
            }
        }
    }
    Ok(meta.to_task_run(last_ts, Some(msg_count), last_preview))
}

#[tauri::command]
pub fn start_run(
    prompt: String,
    cwd: String,
    agent: String,
    model: Option<String>,
    remote_host_name: Option<String>,
    platform_id: Option<String>,
    execution_path: Option<String>,
) -> Result<TaskRun, String> {
    log::debug!(
        "[runs] start_run: agent={}, model={:?}, remote={:?}, platform={:?}, path={:?}, prompt_len={}, cwd={}",
        agent,
        model,
        remote_host_name,
        platform_id,
        execution_path,
        prompt.len(),
        cwd
    );

    // Resolve execution_path: explicit (must be valid) > agent-based default
    let path: ExecutionPath = match execution_path {
        Some(s) => serde_json::from_value(serde_json::Value::String(s.clone())).map_err(|_| {
            format!(
                "invalid execution_path '{}': expected 'session_actor' or 'pipe_exec'",
                s
            )
        })?,
        None => {
            if agent == "claude" {
                ExecutionPath::SessionActor
            } else if agent == "codex"
                && storage::settings::get_user_settings()
                    .codex_transport
                    .as_deref()
                    != Some("exec")
                && crate::commands::session::codex_appserver_supported()
            {
                // Codex DEFAULTS to the app-server (bidirectional session_actor) path so the
                // interactive tools (approvals, steer, fork/rewind/compact/goal, images, live
                // command output) work out of the box — most of the Codex feature surface
                // depends on it. Only an explicit "exec" setting opts out, OR an installed
                // Codex CLI too old for `codex app-server --enable …` (the probe) — in which
                // case we auto-fall back to the one-shot exec transport so the run still works.
                ExecutionPath::SessionActor
            } else {
                ExecutionPath::PipeExec
            }
        }
    };

    // Validate agent/path combination
    validate_agent_path(&agent, &path)?;

    // Snapshot remote host config at creation time (self-contained — survives renames/deletions).
    // Prefer the cwd argument as the remote path (user's just-picked folder); fall back to the
    // host's configured default only when cwd is empty/`/`. This means `meta.remote_cwd` reflects
    // what the user actually chose for this run.
    //
    // Why "/" is treated as empty: the frontend folder picker uses "/" as its placeholder for
    // "no path selected yet". Treating an explicit "/" as a chosen cwd would root the session at
    // the remote filesystem root, which is almost never the user's intent and breaks most Claude
    // Code skills (path resolution, project memory). If "/" is genuinely needed, configure it
    // via `RemoteHost.remote_cwd`.
    let (remote_cwd, remote_host_snapshot) = if let Some(ref name) = remote_host_name {
        let settings = storage::settings::get_user_settings();
        let host = settings
            .remote_hosts
            .iter()
            .find(|h| h.name == *name)
            .ok_or_else(|| format!("Remote host '{}' not found in settings", name))?;
        let effective = if !cwd.is_empty() && cwd != "/" {
            Some(cwd.clone())
        } else {
            host.remote_cwd.clone()
        };
        (effective, Some(host.clone()))
    } else {
        (None, None)
    };

    let id = uuid::Uuid::new_v4().to_string();
    let mut meta = storage::runs::create_run(
        &id,
        &prompt,
        &cwd,
        &agent,
        RunStatus::Pending,
        model,
        None,
        remote_host_name,
        remote_cwd,
        remote_host_snapshot,
        platform_id,
    )?;
    meta.execution_path = Some(path);
    storage::runs::save_meta(&meta)?;
    log::debug!("[runs] start_run: created id={}", id);
    Ok(meta.to_task_run(None, None, None))
}

#[tauri::command]
pub fn rename_run(id: String, name: String) -> Result<(), String> {
    log::debug!("[runs] rename_run: id={}, name={}", id, name);
    storage::runs::rename_run(&id, &name)
}

#[tauri::command]
pub fn soft_delete_runs(ids: Vec<String>) -> Result<u32, String> {
    log::debug!("[cmd/runs] soft_delete_runs: ids={:?}", ids);
    storage::runs::soft_delete_runs(&ids)
}

#[tauri::command]
pub fn update_run_model(id: String, model: String) -> Result<(), String> {
    log::debug!("[runs] update_run_model: id={}, model={}", id, model);
    storage::runs::update_run_model(&id, &model)
}

#[tauri::command]
pub async fn stop_run(
    id: String,
    sessions: tauri::State<'_, ActorSessionMap>,
    process_map: tauri::State<'_, crate::agent::stream::ProcessMap>,
) -> Result<bool, String> {
    log::debug!("[runs] stop_run: id={}", id);

    // Try actor session first (primary mode)
    let actor_stopped = super::session::stop_actor(&sessions, &id)
        .await
        .unwrap_or(false);

    if actor_stopped {
        log::debug!("[runs] stop_run: stopped actor session for id={}", id);
    } else {
        // Fall through to pipe mode (Codex)
        crate::agent::stream::stop_process(&process_map, &id).await;
    }

    // Update status regardless of which path stopped the process
    if let Err(e) = storage::runs::update_status(
        &id,
        RunStatus::Stopped,
        None,
        Some("Stopped by user".to_string()),
    ) {
        log::warn!("[runs] stop_run: failed to update status: {}", e);
    }
    Ok(true)
}

// ── Prompt search & favorites ──

#[tauri::command]
pub async fn search_prompts(
    query: String,
    limit: Option<usize>,
) -> Result<Vec<PromptSearchResult>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(vec![]);
    }
    log::debug!("[runs] search_prompts: query={}", query);

    tokio::task::spawn_blocking(move || {
        let entries = storage::prompt_index::build_or_update_index()?;

        // Case-insensitive substring filter
        let query_lower = query.to_lowercase();
        let matched: Vec<_> = entries
            .into_iter()
            .filter(|e| e.text.to_lowercase().contains(&query_lower))
            .collect();

        // Load RunMeta map
        let metas = storage::runs::list_all_run_metas();
        let meta_map: HashMap<String, _> = metas.into_iter().map(|m| (m.id.clone(), m)).collect();

        // Load favorites set
        let favs = storage::favorites::list_favorites();
        let fav_set: HashSet<(String, u64)> = favs.into_iter().map(|f| (f.run_id, f.seq)).collect();

        // Join and build results
        let mut results: Vec<PromptSearchResult> = matched
            .into_iter()
            .filter_map(|entry| {
                let meta = meta_map.get(&entry.run_id)?;
                Some(PromptSearchResult {
                    run_id: entry.run_id.clone(),
                    run_name: meta.name.clone(),
                    run_prompt: meta.prompt.clone(),
                    agent: meta.agent.clone(),
                    model: meta.model.clone(),
                    status: meta.status.clone(),
                    started_at: meta.started_at.clone(),
                    matched_text: entry.text,
                    matched_seq: entry.seq,
                    matched_ts: entry.ts,
                    matched_event_id: entry.event_id,
                    is_favorite: fav_set.contains(&(entry.run_id, entry.seq)),
                })
            })
            .collect();

        // Sort by matched_ts descending
        results.sort_by(|a, b| b.matched_ts.cmp(&a.matched_ts));

        // Apply limit
        let limit = limit.unwrap_or(100);
        results.truncate(limit);

        log::debug!("[runs] search_prompts: {} results", results.len());
        Ok(results)
    })
    .await
    .map_err(|e| format!("search task failed: {e}"))?
}

#[tauri::command]
pub fn add_prompt_favorite(
    run_id: String,
    seq: u64,
    text: String,
) -> Result<PromptFavorite, String> {
    storage::favorites::add_favorite(&run_id, seq, &text)
}

#[tauri::command]
pub fn remove_prompt_favorite(run_id: String, seq: u64) -> Result<(), String> {
    storage::favorites::remove_favorite(&run_id, seq)
}

#[tauri::command]
pub fn update_prompt_favorite_tags(
    run_id: String,
    seq: u64,
    tags: Vec<String>,
) -> Result<(), String> {
    storage::favorites::update_favorite_tags(&run_id, seq, tags)
}

#[tauri::command]
pub fn update_prompt_favorite_note(run_id: String, seq: u64, note: String) -> Result<(), String> {
    storage::favorites::update_favorite_note(&run_id, seq, &note)
}

#[tauri::command]
pub fn list_prompt_favorites() -> Result<Vec<PromptFavorite>, String> {
    Ok(storage::favorites::list_favorites())
}

#[tauri::command]
pub fn list_prompt_tags() -> Result<Vec<String>, String> {
    Ok(storage::favorites::list_all_tags())
}

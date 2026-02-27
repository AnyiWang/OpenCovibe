use crate::storage::teams;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

/// Tracks last-seen modification timestamps for dedup.
type TimestampMap = Arc<Mutex<HashMap<PathBuf, SystemTime>>>;

/// Start watching ~/.claude/teams/ and ~/.claude/tasks/ for changes.
/// Emits `team-update` and `task-update` Tauri events.
/// Respects the CancellationToken for graceful shutdown.
pub fn start_team_watcher(app: AppHandle, cancel: CancellationToken) {
    let timestamps: TimestampMap = Arc::new(Mutex::new(HashMap::new()));

    std::thread::spawn(move || {
        let teams_dir = teams::teams_dir();
        let tasks_dir = teams::tasks_dir();

        // Ensure directories exist (no-op if already present)
        let _ = std::fs::create_dir_all(&teams_dir);
        let _ = std::fs::create_dir_all(&tasks_dir);

        let (tx, rx) = std::sync::mpsc::channel();

        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                log::error!("[team_watcher] init failed: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&teams_dir, RecursiveMode::Recursive) {
            log::warn!("[team_watcher] failed to watch teams dir: {}", e);
        }
        if let Err(e) = watcher.watch(&tasks_dir, RecursiveMode::Recursive) {
            log::warn!("[team_watcher] failed to watch tasks dir: {}", e);
        }

        log::info!(
            "[team_watcher] started — watching {} and {}",
            teams_dir.display(),
            tasks_dir.display()
        );

        loop {
            if cancel.is_cancelled() {
                log::info!("[team_watcher] shutting down");
                break;
            }

            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {}
                        _ => continue,
                    }
                    for path in &event.paths {
                        process_team_file_change(&app, path, &teams_dir, &tasks_dir, &timestamps);
                    }
                }
                Ok(Err(e)) => log::warn!("[team_watcher] watch error: {}", e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    log::info!("[team_watcher] channel disconnected, stopping");
                    break;
                }
            }
        }
    });
}

/// Process a single file change event, determine team_name and change type, emit event.
fn process_team_file_change(
    app: &AppHandle,
    path: &Path,
    teams_dir: &Path,
    tasks_dir: &Path,
    timestamps: &TimestampMap,
) {
    // Skip non-JSON files
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "json" {
        return;
    }

    // Dedup by modification timestamp
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            let mut map = timestamps.lock().unwrap();
            if let Some(prev) = map.get(path) {
                if *prev == modified {
                    return; // Same modification time — skip duplicate
                }
            }
            map.insert(path.to_path_buf(), modified);

            // GC stale entries
            if map.len() > 200 {
                map.retain(|p, _| p.exists());
            }
        }
    }

    // Determine if this is a teams/ or tasks/ change by checking the path prefix
    if let Ok(rel) = path.strip_prefix(teams_dir) {
        // teams/{team_name}/config.json or teams/{team_name}/inboxes/{agent}.json
        let components: Vec<&str> = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        if components.is_empty() {
            return;
        }

        let team_name = components[0];
        let change = if components.len() >= 2 && components[1] == "inboxes" {
            "inbox"
        } else {
            "config"
        };

        log::debug!(
            "[team_watcher] team-update: team={}, change={}",
            team_name,
            change
        );
        let _ = app.emit(
            "team-update",
            serde_json::json!({
                "team_name": team_name,
                "change": change,
            }),
        );
    } else if let Ok(rel) = path.strip_prefix(tasks_dir) {
        // tasks/{team_name}/{id}.json
        let components: Vec<&str> = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        if components.len() < 2 {
            return;
        }

        let team_name = components[0];
        let file_name = components[1];
        let task_id = file_name.strip_suffix(".json").unwrap_or(file_name);

        // Skip lock/highwatermark files
        if task_id.starts_with('.') || task_id == "lock" || task_id == "highwatermark" {
            return;
        }

        log::debug!(
            "[team_watcher] task-update: team={}, task_id={}",
            team_name,
            task_id
        );
        let _ = app.emit(
            "task-update",
            serde_json::json!({
                "team_name": team_name,
                "task_id": task_id,
                "change": "updated",
            }),
        );
    }
}

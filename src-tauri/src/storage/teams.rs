use crate::models::{TeamConfig, TeamInboxMessage, TeamSummary, TeamTask};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

/// Root of Claude Code data: ~/.claude/
pub fn claude_home_dir() -> PathBuf {
    crate::storage::dirs_next()
        .expect("home dir")
        .join(".claude")
}

/// ~/.claude/teams/
pub fn teams_dir() -> PathBuf {
    claude_home_dir().join("teams")
}

/// ~/.claude/tasks/
pub fn tasks_dir() -> PathBuf {
    claude_home_dir().join("tasks")
}

/// Generic JSON file reader — returns None on read or parse errors.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("[teams] parse error {}: {}", path.display(), e);
                None
            }
        },
        Err(e) => {
            log::debug!("[teams] read error {}: {}", path.display(), e);
            None
        }
    }
}

/// List all teams by reading ~/.claude/teams/*/config.json.
pub fn list_teams() -> Vec<TeamSummary> {
    let dir = teams_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::debug!("[teams] cannot read teams dir {}: {}", dir.display(), e);
            return vec![];
        }
    };

    let mut teams = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let config_path = path.join("config.json");
        if let Some(config) = read_json::<TeamConfig>(&config_path) {
            let team_name = config.name.clone();
            let task_count = count_tasks(&team_name);
            teams.push(TeamSummary {
                name: config.name,
                description: config.description,
                member_count: config.members.len(),
                task_count,
                created_at: config.created_at,
            });
        }
    }

    // Sort by created_at descending (most recent first)
    teams.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    teams
}

/// Count non-internal tasks for a team.
fn count_tasks(team_name: &str) -> usize {
    let dir = tasks_dir().join(team_name);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".json")
                && !name.starts_with('.')
                && name != ".lock"
                && name != ".highwatermark"
        })
        .count()
}

/// Read a single team config.
pub fn get_team_config(name: &str) -> Option<TeamConfig> {
    let config_path = teams_dir().join(name).join("config.json");
    read_json::<TeamConfig>(&config_path)
}

/// List all tasks for a team, sorted by id (numeric).
/// Skips .lock, .highwatermark, non-JSON files, and tasks with metadata._internal == true.
pub fn list_team_tasks(name: &str) -> Vec<TeamTask> {
    let dir = tasks_dir().join(name);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::debug!("[teams] cannot read tasks dir {}: {}", dir.display(), e);
            return vec![];
        }
    };

    let mut tasks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = match path.file_name().and_then(|f| f.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip non-JSON, hidden files, lock/highwatermark
        if !file_name.ends_with(".json")
            || file_name.starts_with('.')
            || file_name == ".lock"
            || file_name == ".highwatermark"
        {
            continue;
        }

        if let Some(task) = read_json::<TeamTask>(&path) {
            // Skip _internal tasks
            if let Some(ref meta) = task.metadata {
                if meta.get("_internal").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
            }
            // Validate required fields
            if task.id.is_empty() || task.subject.is_empty() {
                log::warn!(
                    "[teams] skipping task with empty id/subject: {}",
                    path.display()
                );
                continue;
            }
            tasks.push(task);
        }
    }

    // Sort by id: numeric parse, fallback to string sort
    tasks.sort_by(|a, b| {
        let a_num = a.id.parse::<u64>();
        let b_num = b.id.parse::<u64>();
        match (a_num, b_num) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            _ => a.id.cmp(&b.id),
        }
    });

    tasks
}

/// Read a single task by team and id.
pub fn get_team_task(team: &str, id: &str) -> Option<TeamTask> {
    let path = tasks_dir().join(team).join(format!("{}.json", id));
    read_json::<TeamTask>(&path)
}

/// Delete a team by removing its directories from ~/.claude/teams/{name} and ~/.claude/tasks/{name}.
pub fn delete_team(name: &str) -> Result<(), String> {
    // Validate name to prevent path traversal
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(format!("Invalid team name: {}", name));
    }

    let team_dir = teams_dir().join(name);
    let task_dir = tasks_dir().join(name);

    let mut deleted_any = false;

    if team_dir.is_dir() {
        std::fs::remove_dir_all(&team_dir)
            .map_err(|e| format!("Failed to remove team dir {}: {}", team_dir.display(), e))?;
        log::debug!("[teams] deleted team dir: {}", team_dir.display());
        deleted_any = true;
    }

    if task_dir.is_dir() {
        std::fs::remove_dir_all(&task_dir)
            .map_err(|e| format!("Failed to remove task dir {}: {}", task_dir.display(), e))?;
        log::debug!("[teams] deleted task dir: {}", task_dir.display());
        deleted_any = true;
    }

    if !deleted_any {
        return Err(format!("Team '{}' not found", name));
    }

    log::debug!("[teams] delete_team completed: {}", name);
    Ok(())
}

/// Read the inbox for an agent in a team.
pub fn get_team_inbox(team: &str, agent: &str) -> Vec<TeamInboxMessage> {
    let path = teams_dir()
        .join(team)
        .join("inboxes")
        .join(format!("{}.json", agent));
    read_json::<Vec<TeamInboxMessage>>(&path).unwrap_or_default()
}

/// Read all inboxes for a team, merged and sorted by timestamp descending.
pub fn get_all_team_inboxes(name: &str) -> Vec<TeamInboxMessage> {
    let inboxes_dir = teams_dir().join(name).join("inboxes");
    let entries = match std::fs::read_dir(&inboxes_dir) {
        Ok(e) => e,
        Err(e) => {
            log::debug!(
                "[teams] cannot read inboxes dir {}: {}",
                inboxes_dir.display(),
                e
            );
            return vec![];
        }
    };

    let mut all_messages = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = match path.file_name().and_then(|f| f.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !file_name.ends_with(".json") {
            continue;
        }
        if let Some(messages) = read_json::<Vec<TeamInboxMessage>>(&path) {
            all_messages.extend(messages);
        }
    }

    // Sort by timestamp descending (most recent first)
    all_messages.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    log::debug!(
        "[teams] get_all_team_inboxes: {} messages for team '{}'",
        all_messages.len(),
        name
    );
    all_messages
}

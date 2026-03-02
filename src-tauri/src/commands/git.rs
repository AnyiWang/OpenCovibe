use serde::Serialize;
use std::process::Command;

#[derive(Serialize)]
pub struct GitFileStat {
    pub path: String,
    pub status: String,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Serialize)]
pub struct GitSummary {
    pub branch: String,
    pub files: Vec<GitFileStat>,
    pub total_files: u32,
    pub total_insertions: u32,
    pub total_deletions: u32,
}

#[tauri::command]
pub async fn get_git_summary(cwd: String) -> Result<GitSummary, String> {
    log::debug!("[git] get_git_summary: cwd={}", cwd);

    // Branch name
    let branch = Command::new("git")
        .current_dir(&cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Per-file numstat (staged + unstaged vs HEAD)
    let numstat_output = Command::new("git")
        .current_dir(&cwd)
        .args(["diff", "--numstat", "HEAD"])
        .output()
        .map_err(|e| format!("Failed to run git diff --numstat: {}", e))?;

    // Status for file status codes (M/A/D/R/?)
    let status_output = Command::new("git")
        .current_dir(&cwd)
        .args(["status", "--short"])
        .output()
        .map_err(|e| format!("Failed to run git status: {}", e))?;

    // Parse status codes into a map: path → status char
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    let mut status_map = std::collections::HashMap::new();
    for line in status_str.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let path = line[3..].trim();
        // Pick the most relevant status: index (X) or worktree (Y)
        let code = if xy.starts_with('?') {
            "?"
        } else if xy.starts_with('A') || xy.ends_with('A') {
            "A"
        } else if xy.starts_with('D') || xy.ends_with('D') {
            "D"
        } else if xy.starts_with('R') || xy.ends_with('R') {
            "R"
        } else {
            "M"
        };
        // Handle renames: "R  old -> new"
        let actual_path = if let Some(arrow) = path.find(" -> ") {
            &path[arrow + 4..]
        } else {
            path
        };
        status_map.insert(actual_path.to_string(), code.to_string());
    }

    // Parse numstat: "insertions\tdeletions\tpath"
    let numstat_str = String::from_utf8_lossy(&numstat_output.stdout);
    let mut files = Vec::new();
    let mut total_ins: u32 = 0;
    let mut total_del: u32 = 0;

    for line in numstat_str.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        // Binary files show "-" for insertions/deletions
        let ins = parts[0].parse::<u32>().unwrap_or(0);
        let del = parts[1].parse::<u32>().unwrap_or(0);
        let path = parts[2].to_string();
        let status = status_map
            .get(&path)
            .cloned()
            .unwrap_or_else(|| "M".to_string());
        total_ins += ins;
        total_del += del;
        files.push(GitFileStat {
            path,
            status,
            insertions: ins,
            deletions: del,
        });
    }

    // Also add untracked files from status (not in numstat)
    for (path, code) in &status_map {
        if code == "?" && !files.iter().any(|f| &f.path == path) {
            files.push(GitFileStat {
                path: path.clone(),
                status: "?".to_string(),
                insertions: 0,
                deletions: 0,
            });
        }
    }

    let total_files = files.len() as u32;

    Ok(GitSummary {
        branch,
        files,
        total_files,
        total_insertions: total_ins,
        total_deletions: total_del,
    })
}

#[tauri::command]
pub async fn get_git_diff(
    cwd: String,
    staged: bool,
    file: Option<String>,
) -> Result<String, String> {
    log::debug!(
        "[git] get_git_diff: cwd={}, staged={}, file={:?}",
        cwd,
        staged,
        file
    );
    let mut cmd = Command::new("git");
    cmd.current_dir(&cwd);
    cmd.arg("diff");
    if staged {
        cmd.arg("--cached");
    } else if file.is_some() {
        // Per-file diff: compare working tree against HEAD (staged + unstaged)
        cmd.arg("HEAD");
    }
    if let Some(ref f) = file {
        cmd.arg("--").arg(f);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git diff: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff failed: {}", stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
pub async fn get_git_status(cwd: String) -> Result<String, String> {
    log::debug!("[git] get_git_status: cwd={}", cwd);
    let output = Command::new("git")
        .current_dir(&cwd)
        .args(["status", "--short"])
        .output()
        .map_err(|e| format!("Failed to run git status: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git status failed: {}", stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

use std::fs;
use std::path::PathBuf;

/// Validate that a file path is within allowed directories.
///
/// Allowed directories:
/// - `~/.opencovibe/` (data dir)
/// - `~/.claude/` (Claude config dir)
/// - The global `working_directory` from user settings (if set)
/// - Any per-agent `working_directory` from agent settings
/// - The caller-provided `extra_allowed` directory (e.g. frontend project cwd)
fn validate_file_path(path: &str, extra_allowed: Option<&str>) -> Result<PathBuf, String> {
    let requested = PathBuf::from(path);

    // Defense-in-depth: reject raw traversal patterns
    if path.contains("..") {
        log::warn!("[files] path traversal rejected: {}", path);
        return Err("Path traversal not allowed".to_string());
    }

    // For existing files: canonicalize and check prefix
    // For new files: canonicalize parent and check prefix
    let canonical = if requested.exists() {
        std::fs::canonicalize(&requested)
    } else if let Some(parent) = requested.parent() {
        if parent.as_os_str().is_empty() || parent.exists() {
            if parent.as_os_str().is_empty() {
                // Relative path with no parent dir component — use cwd
                Ok(std::env::current_dir()
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join(requested.file_name().unwrap_or_default()))
            } else {
                std::fs::canonicalize(parent)
                    .map(|p| p.join(requested.file_name().unwrap_or_default()))
            }
        } else {
            return Err(format!("Parent directory does not exist: {}", path));
        }
    } else {
        return Err(format!("Invalid path: {}", path));
    }
    .map_err(|e| format!("Cannot resolve path: {}", e))?;

    let data_dir = crate::storage::data_dir();
    let home = crate::storage::home_dir().unwrap_or_default();
    let claude_dir = PathBuf::from(&home).join(".claude");

    // Allow: ~/.opencovibe/*, ~/.claude/*
    if canonical.starts_with(&data_dir) || canonical.starts_with(&claude_dir) {
        log::debug!("[files] path allowed (config dir): {}", canonical.display());
        return Ok(canonical);
    }

    // Allow: project cwd (if set in global user settings)
    let settings = crate::storage::settings::get_user_settings();
    if let Some(ref wd) = settings.working_directory {
        if let Ok(wd_canonical) = std::fs::canonicalize(wd) {
            if canonical.starts_with(&wd_canonical) {
                log::debug!(
                    "[files] path allowed (working dir): {}",
                    canonical.display()
                );
                return Ok(canonical);
            }
        }
    }

    // Allow: per-agent working directories
    let all_settings = crate::storage::settings::load();
    for agent_settings in all_settings.agents.values() {
        if let Some(ref wd) = agent_settings.working_directory {
            if let Ok(wd_canonical) = std::fs::canonicalize(wd) {
                if canonical.starts_with(&wd_canonical) {
                    log::debug!(
                        "[files] path allowed (agent working dir): {}",
                        canonical.display()
                    );
                    return Ok(canonical);
                }
            }
        }
    }

    // Allow: caller-provided directory (e.g. frontend project cwd)
    if let Some(extra) = extra_allowed {
        if let Ok(extra_canonical) = std::fs::canonicalize(extra) {
            if canonical.starts_with(&extra_canonical) {
                log::debug!("[files] path allowed (extra dir): {}", canonical.display());
                return Ok(canonical);
            }
        }
    }

    log::warn!(
        "[files] access denied: path '{}' is outside allowed directories",
        path
    );
    Err(format!(
        "Access denied: path '{}' is outside allowed directories",
        path
    ))
}

#[tauri::command]
pub fn read_text_file(path: String, cwd: Option<String>) -> Result<String, String> {
    log::debug!("[files] read_text_file: path={}, cwd={:?}", path, cwd);
    let validated = validate_file_path(&path, cwd.as_deref())?;
    fs::read_to_string(&validated)
        .map_err(|e| format!("Failed to read {}: {}", validated.display(), e))
}

const MAX_TASK_OUTPUT_BYTES: u64 = 512 * 1024; // 512KB

#[tauri::command]
pub fn read_task_output(path: String) -> Result<String, String> {
    log::debug!("[files] read_task_output: path={}", path);

    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| format!("Cannot resolve path '{}': {}", path, e))?;

    // Suffix check: must be .output
    if canonical.extension().and_then(|e| e.to_str()) != Some("output") {
        log::warn!(
            "[files] read_task_output denied (not .output): {}",
            canonical.display()
        );
        return Err("Access denied: not a task output file".into());
    }

    // Prefix check: must be in temp directory (PathBuf::starts_with is path-level, not string-level)
    let temp_dir =
        std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    #[cfg(target_os = "macos")]
    let extra_temp = Some(PathBuf::from("/private/tmp"));
    #[cfg(not(target_os = "macos"))]
    let extra_temp: Option<PathBuf> = None;
    if !canonical.starts_with(&temp_dir)
        && !extra_temp
            .as_ref()
            .is_some_and(|t| canonical.starts_with(t))
    {
        log::warn!(
            "[files] read_task_output denied (not in temp): {}",
            canonical.display()
        );
        return Err("Access denied: task output must be in temp directory".into());
    }

    // Size check + tail read
    let meta = fs::metadata(&canonical).map_err(|e| format!("Cannot stat: {}", e))?;
    let size = meta.len();

    use std::io::{Read, Seek, SeekFrom};
    if size <= MAX_TASK_OUTPUT_BYTES {
        log::debug!("[files] read_task_output: full read {}B", size);
        let bytes = fs::read(&canonical).map_err(|e| format!("Failed to read: {}", e))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        log::debug!("[files] read_task_output: tail read ({}B > max)", size);
        let mut file = fs::File::open(&canonical).map_err(|e| format!("Failed to open: {}", e))?;
        file.seek(SeekFrom::End(-(MAX_TASK_OUTPUT_BYTES as i64)))
            .map_err(|e| format!("Seek failed: {}", e))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("Read failed: {}", e))?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        // Skip to first complete line (seek may land mid-line)
        let trimmed = if let Some(nl) = text.find('\n') {
            &text[nl + 1..]
        } else {
            &text
        };
        Ok(format!(
            "... ({} bytes truncated)\n{}",
            size - MAX_TASK_OUTPUT_BYTES,
            trimmed
        ))
    }
}

#[tauri::command]
pub fn write_text_file(path: String, content: String, cwd: Option<String>) -> Result<(), String> {
    log::debug!(
        "[files] write_text_file: path={}, content_len={}, cwd={:?}",
        path,
        content.len(),
        cwd,
    );
    let validated = validate_file_path(&path, cwd.as_deref())?;
    if let Some(parent) = validated.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {}", e))?;
    }
    fs::write(&validated, content)
        .map_err(|e| format!("Failed to write {}: {}", validated.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_task_output_allows_output_in_temp() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_read_task_output.output");
        std::fs::write(&path, "hello from task").unwrap();
        let result = read_task_output(path.to_string_lossy().to_string());
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        assert_eq!(result.unwrap(), "hello from task");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_task_output_denies_non_temp_path() {
        // /etc/passwd renamed to .output — still outside temp dir
        let result = read_task_output("/etc/passwd.output".to_string());
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Could be "Cannot resolve" (doesn't exist) or "Access denied"
        assert!(
            err.contains("Cannot resolve") || err.contains("Access denied"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn read_task_output_denies_non_output_suffix() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_read_task_output.txt");
        std::fs::write(&path, "secret").unwrap();
        let result = read_task_output(path.to_string_lossy().to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a task output file"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_task_output_error_for_nonexistent() {
        let result = read_task_output("/tmp/definitely_does_not_exist_12345.output".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot resolve"));
    }
}

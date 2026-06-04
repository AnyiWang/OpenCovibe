use std::fs;
use std::path::PathBuf;

/// Canonicalize a path for `starts_with` comparison. If the path doesn't exist,
/// canonicalize the parent and re-append the final component. Falls back to
/// the path as-is if neither exist. Sufficient for `~/.opencovibe` / `~/.claude`
/// where the parent (`$HOME`) always exists.
fn canonicalize_for_prefix(path: &std::path::Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    if let Some(parent) = path.parent() {
        if let Ok(cp) = std::fs::canonicalize(parent) {
            if let Some(name) = path.file_name() {
                return cp.join(name);
            }
        }
    }
    path.to_path_buf()
}

/// Validate that a file path is within allowed directories.
///
/// Allowed directories:
/// - `~/.opencovibe/` (data dir)
/// - `~/.claude/` (Claude config dir)
/// - The global `working_directory` from user settings (if set)
/// - Any per-agent `working_directory` from agent settings
/// - The caller-provided `extra_allowed` directory (e.g. frontend project cwd)
pub(crate) fn validate_file_path(
    path: &str,
    extra_allowed: Option<&str>,
) -> Result<PathBuf, String> {
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
            // Parent doesn't exist — find nearest existing ancestor,
            // canonicalize it, then append non-existent suffix.
            // Allows write_text_file's create_dir_all to create missing dirs.
            let mut existing = parent.to_path_buf();
            while !existing.exists() {
                match existing.parent() {
                    Some(p) if !p.as_os_str().is_empty() => {
                        existing = p.to_path_buf();
                    }
                    _ => {
                        // Relative path with no remaining parent — use cwd as base
                        existing = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
                        break;
                    }
                }
            }
            let canonical_base = std::fs::canonicalize(&existing)
                .map_err(|e| format!("Cannot resolve path: {}", e))?;
            // Strip the existing ancestor prefix from requested, then join onto canonical base.
            // Works for both absolute (/a/b/c → existing=/a → suffix=b/c)
            // and relative (a/b/c → existing=a → suffix=b/c).
            // If existing is cwd (break branch above), strip_prefix fails → fall back to full join.
            if let Ok(suffix) = requested.strip_prefix(&existing) {
                Ok(canonical_base.join(suffix))
            } else {
                // existing is cwd itself (not a prefix of requested) → join full requested
                Ok(canonical_base.join(&requested))
            }
        }
    } else {
        return Err(format!("Invalid path: {}", path));
    }
    .map_err(|e| format!("Cannot resolve path: {}", e))?;

    let data_dir = crate::storage::data_dir();
    let home = crate::storage::home_dir().unwrap_or_default();
    let claude_dir = PathBuf::from(&home).join(".claude");

    // Canonicalize allowed directories for reliable comparison on Windows
    // (fs::canonicalize normalizes case; raw paths from home_dir() may differ)
    let data_dir_c = canonicalize_for_prefix(&data_dir);
    let claude_dir_c = canonicalize_for_prefix(&claude_dir);

    let codex_dir = PathBuf::from(&home).join(".codex");
    let codex_dir_c = canonicalize_for_prefix(&codex_dir);

    // Allow: ~/.opencovibe/*, ~/.claude/*, ~/.codex/*
    if canonical.starts_with(&data_dir_c)
        || canonical.starts_with(&claude_dir_c)
        || canonical.starts_with(&codex_dir_c)
    {
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

    // Allow: caller-provided directory (e.g. frontend project cwd) and its git root
    if let Some(extra) = extra_allowed {
        if let Ok(extra_canonical) = std::fs::canonicalize(extra) {
            if canonical.starts_with(&extra_canonical) {
                log::debug!("[files] path allowed (extra dir): {}", canonical.display());
                return Ok(canonical);
            }
            // Also allow ancestor agent-memory files (e.g. an AGENTS.md / CLAUDE.md at
            // the repo root when cwd is a subdir). Restricted to those filenames: an
            // unrestricted git-root allowance would let a caller write e.g.
            // `<repo>/.git/hooks/pre-commit` and gain code execution on the next commit.
            let is_agent_memory = canonical
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("AGENTS.md") || n.eq_ignore_ascii_case("CLAUDE.md"))
                .unwrap_or(false);
            if is_agent_memory {
                for ancestor in extra_canonical.ancestors().skip(1) {
                    if ancestor.join(".git").exists() {
                        if canonical.starts_with(ancestor) {
                            log::debug!(
                                "[files] path allowed (agent-memory file at git root): {}",
                                canonical.display()
                            );
                            return Ok(canonical);
                        }
                        break;
                    }
                }
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

/// Check whether `{cwd}/AGENTS.md` exists. Lightweight existence query for
/// the Codex `/init` slash command, which needs to skip rather than overwrite.
///
/// Deliberately narrow — no general `path` parameter — so this IPC can't be
/// abused as a filesystem probe. The filename is hardcoded; the only attacker
/// surface is `cwd`, which still must be sanitised (reject `..` traversal,
/// reject empty/relative cwds). Existence of an `AGENTS.md` file at an
/// arbitrary directory is low-sensitivity, but we still limit the API to
/// "directories the caller already pretends to know about".
///
/// Does NOT reuse `validate_file_path`: that resolves relative paths against
/// the process `current_dir()` rather than the supplied cwd.
#[tauri::command]
pub fn agents_md_exists(cwd: String) -> Result<bool, String> {
    log::debug!("[files] agents_md_exists: cwd={}", cwd);
    if cwd.is_empty() {
        return Err("cwd is required".to_string());
    }
    if cwd.contains("..") {
        return Err("Path traversal not allowed in cwd".to_string());
    }
    let cwd_path = std::path::Path::new(&cwd);
    if !cwd_path.is_absolute() {
        return Err("cwd must be an absolute path".to_string());
    }
    Ok(cwd_path.join("AGENTS.md").exists())
}

#[tauri::command]
pub fn read_text_file(path: String, cwd: Option<String>) -> Result<String, String> {
    log::debug!("[files] read_text_file: path={}, cwd={:?}", path, cwd);
    let validated = validate_file_path(&path, cwd.as_deref())?;
    fs::read_to_string(&validated)
        .map_err(|e| format!("Failed to read {}: {}", validated.display(), e))
}

/// Cheap file size lookup. Lets the frontend gate `read_text_file` for very large files
/// without paying the full read + IPC + JS string allocation cost first.
/// Returns size in bytes on success; returns Err on validation failure or stat failure
/// (caller should fall back to read_text_file in that case — Err does NOT mean size 0).
#[tauri::command]
pub fn stat_text_file(path: String, cwd: Option<String>) -> Result<u64, String> {
    log::debug!("[files] stat_text_file: path={}, cwd={:?}", path, cwd);
    let validated = validate_file_path(&path, cwd.as_deref())?;
    let size = fs::metadata(&validated)
        .map(|m| m.len())
        .map_err(|e| format!("Failed to stat {}: {}", validated.display(), e))?;
    log::debug!(
        "[files] stat_text_file: result path={} size={}",
        validated.display(),
        size
    );
    Ok(size)
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

    // Prefix check: must be in temp directory (PathBuf::starts_with is path-level, not string-level).
    // Both `canonical` and `temp_dir` are canonicalized, so short/long path differences on Windows
    // are already resolved. WSL paths (/tmp/...) are NOT supported — this is a native Windows app.
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

/// Recursively scan a directory for `.md` files, returning them as memory candidates.
pub(crate) fn scan_memory_md_files(
    dir: &std::path::Path,
    base: &std::path::Path,
    max_depth: usize,
    max_files: usize,
) -> Vec<crate::models::MemoryFileCandidate> {
    let mut files = Vec::new();
    scan_md_inner(dir, base, &mut files, 0, max_depth, max_files);
    // Sort: MEMORY.md first, then alphabetical
    files.sort_by(|a, b| {
        let a_is_index = a.label == "MEMORY.md";
        let b_is_index = b.label == "MEMORY.md";
        b_is_index.cmp(&a_is_index).then(a.label.cmp(&b.label))
    });
    files
}

fn scan_md_inner(
    dir: &std::path::Path,
    base: &std::path::Path,
    files: &mut Vec<crate::models::MemoryFileCandidate>,
    depth: usize,
    max_depth: usize,
    max_files: usize,
) {
    if depth > max_depth {
        log::debug!(
            "[files] memory scan: max depth {} reached at {}",
            max_depth,
            dir.display()
        );
        return;
    }
    if files.len() >= max_files {
        log::debug!(
            "[files] memory scan: max files {} reached, truncating",
            max_files
        );
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= max_files {
            break;
        }
        let p = entry.path();
        // Skip symlinks to avoid circular traversal
        if p.is_symlink() {
            log::debug!("[files] skipping symlink in memory scan: {}", p.display());
            continue;
        }
        if p.is_dir() {
            scan_md_inner(&p, base, files, depth + 1, max_depth, max_files);
        } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
            let label = p
                .strip_prefix(base)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });
            files.push(crate::models::MemoryFileCandidate {
                path: p.display().to_string(),
                label,
                scope: "memory".to_string(),
                exists: true,
            });
        }
    }
}

#[tauri::command]
pub fn list_memory_files(
    cwd: Option<String>,
) -> Result<Vec<crate::models::MemoryFileCandidate>, String> {
    let project_names = [
        "CLAUDE.md",
        ".claude/CLAUDE.md",
        "CLAUDE.local.md",
        ".claude/CLAUDE.local.md",
    ];
    let global_names = ["CLAUDE.md", "CLAUDE.local.md"];

    let mut files = Vec::new();

    // Global scope — only if home is available
    match crate::storage::home_dir() {
        Some(home) if !home.is_empty() => {
            let claude_dir = std::path::Path::new(&home).join(".claude");
            for name in &global_names {
                let p = claude_dir.join(name);
                files.push(crate::models::MemoryFileCandidate {
                    path: p.display().to_string(),
                    label: name.to_string(),
                    scope: "global".to_string(),
                    exists: p.exists(),
                });
            }

            // Codex global scope (~/.codex/)
            let codex_global_names = ["AGENTS.override.md", "AGENTS.md"];
            let codex_dir = std::path::Path::new(&home).join(".codex");
            for name in &codex_global_names {
                let p = codex_dir.join(name);
                files.push(crate::models::MemoryFileCandidate {
                    path: p.display().to_string(),
                    label: name.to_string(),
                    scope: "global".to_string(),
                    exists: p.exists(),
                });
            }
        }
        _ => {
            log::warn!(
                "[files] list_memory_files: home_dir unavailable, skipping global candidates"
            );
        }
    }

    // Project scope — Claude
    if let Some(ref cwd) = cwd {
        let cwd_path = std::path::Path::new(cwd);
        for name in &project_names {
            let p = cwd_path.join(name);
            files.push(crate::models::MemoryFileCandidate {
                path: p.display().to_string(),
                label: name.to_string(),
                scope: "project".to_string(),
                exists: p.exists(),
            });
        }
    }

    // Project scope — Codex (hierarchical: repo root → cwd)
    if let Some(ref cwd) = cwd {
        let cwd_path = std::path::Path::new(cwd);
        // Find project root by walking up to .git (matches Codex default project_root_markers)
        let project_root = cwd_path.ancestors().find(|a| a.join(".git").exists());
        // Collect dirs from root → cwd (inclusive)
        let search_dirs: Vec<&std::path::Path> = if let Some(root) = project_root {
            let mut dirs = vec![];
            let mut cursor = cwd_path;
            loop {
                dirs.push(cursor);
                if cursor == root {
                    break;
                }
                match cursor.parent() {
                    Some(p) => cursor = p,
                    None => break,
                }
            }
            dirs.reverse(); // root first
            dirs
        } else {
            vec![cwd_path]
        };
        let codex_project_names = ["AGENTS.override.md", "AGENTS.md"];
        let label_base = project_root.unwrap_or(cwd_path);
        for dir in &search_dirs {
            for name in &codex_project_names {
                let p = dir.join(name);
                // Label relative to project root, e.g. "packages/frontend/AGENTS.md"
                // Root-level files show as just "AGENTS.md"
                let label = p
                    .strip_prefix(label_base)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| name.to_string());
                files.push(crate::models::MemoryFileCandidate {
                    path: p.display().to_string(),
                    label,
                    scope: "project".to_string(),
                    exists: p.exists(),
                });
            }
        }
    }

    // Project auto-memory scope — scan ~/.claude/projects/{slug}/memory/*.md
    if let (Some(home), Some(ref cwd_val)) = (crate::storage::home_dir(), &cwd) {
        let slug = crate::storage::cli_sessions::encode_cwd(cwd_val);
        let memory_dir = std::path::Path::new(&home)
            .join(".claude")
            .join("projects")
            .join(&slug)
            .join("memory");
        if memory_dir.is_dir() {
            let memory_files = scan_memory_md_files(&memory_dir, &memory_dir, 3, 50);
            files.extend(memory_files);
        }
    }

    log::debug!(
        "[files] list_memory_files: {} candidates ({} exist)",
        files.len(),
        files.iter().filter(|f| f.exists).count()
    );
    Ok(files)
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
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent.output");
        let result = read_task_output(nonexistent.to_string_lossy().into_owned());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot resolve"));
    }

    #[test]
    fn write_creates_missing_parent_dirs() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("sub").join("deep").join("test.md");

        let result = write_text_file(
            target.to_string_lossy().to_string(),
            "hello".to_string(),
            Some(root.path().to_string_lossy().to_string()),
        );
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[test]
    fn write_creates_nested_relative_path_no_duplication() {
        let root = tempfile::tempdir().unwrap();
        // Create partial ancestor: root/sub exists, but root/sub/deep does not
        let sub = root.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let target = root.path().join("sub").join("deep").join("file.md");

        let result = write_text_file(
            target.to_string_lossy().to_string(),
            "no-dup".to_string(),
            Some(root.path().to_string_lossy().to_string()),
        );
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        // Verify it wrote to the correct path, not a duplicated one
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "no-dup");
        // Verify no duplicated directory was created
        assert!(!root.path().join("sub").join("sub").exists());
    }

    #[test]
    fn list_memory_files_returns_project_and_global_candidates() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("my-project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "# hello").unwrap();

        let result = list_memory_files(Some(cwd.to_string_lossy().to_string()));
        assert!(result.is_ok());
        let files = result.unwrap();

        let project_files: Vec<_> = files.iter().filter(|f| f.scope == "project").collect();
        // 4 Claude + 2 Codex (no .git so only cwd layer)
        assert_eq!(project_files.len(), 6);
        assert!(project_files[0].exists);
        assert_eq!(project_files[0].label, "CLAUDE.md");
        assert!(!project_files[1].exists);
    }

    #[test]
    fn list_memory_files_no_cwd_returns_only_global() {
        let result = list_memory_files(None);
        assert!(result.is_ok());
        let files = result.unwrap();
        assert!(files.iter().all(|f| f.scope == "global"));
    }

    #[test]
    fn scan_memory_md_files_basic() {
        let root = tempfile::tempdir().unwrap();
        let mem = root.path().join("memory");
        std::fs::create_dir_all(mem.join("plans")).unwrap();
        std::fs::write(mem.join("MEMORY.md"), "# index").unwrap();
        std::fs::write(mem.join("alpha.md"), "# alpha").unwrap();
        std::fs::write(mem.join("plans").join("feat.md"), "# feat").unwrap();
        std::fs::write(mem.join("notes.txt"), "ignored").unwrap();

        let files = scan_memory_md_files(&mem, &mem, 3, 50);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].label, "MEMORY.md");
        assert_eq!(files[1].label, "alpha.md");
        assert_eq!(files[2].label, "plans/feat.md");
        assert!(files.iter().all(|f| f.exists && f.scope == "memory"));
    }

    #[test]
    fn scan_memory_md_files_respects_max_depth() {
        let root = tempfile::tempdir().unwrap();
        let mem = root.path().join("memory");
        std::fs::create_dir_all(mem.join("d1").join("d2")).unwrap();
        std::fs::write(mem.join("a.md"), "a").unwrap();
        std::fs::write(mem.join("d1").join("b.md"), "b").unwrap();
        std::fs::write(mem.join("d1").join("d2").join("c.md"), "c").unwrap();

        let files = scan_memory_md_files(&mem, &mem, 1, 50);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn scan_memory_md_files_respects_max_files() {
        let root = tempfile::tempdir().unwrap();
        let mem = root.path().join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        for i in 0..10 {
            std::fs::write(mem.join(format!("file{}.md", i)), "x").unwrap();
        }
        let files = scan_memory_md_files(&mem, &mem, 3, 5);
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn canonicalize_for_prefix_reconstructs_via_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent_child");
        assert!(!nonexistent.exists());

        let result = canonicalize_for_prefix(&nonexistent);

        // Parent (tmp dir) should be canonicalized, with "nonexistent_child" appended
        let expected_parent = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(result, expected_parent.join("nonexistent_child"));
    }

    #[test]
    fn canonicalize_for_prefix_returns_canonical_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let result = canonicalize_for_prefix(tmp.path());
        let expected = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn validate_allows_codex_global_dir() {
        // ~/.codex/AGENTS.md should be allowed by the whitelist
        let home = crate::storage::home_dir().unwrap_or_default();
        let codex_dir = PathBuf::from(&home).join(".codex");
        std::fs::create_dir_all(&codex_dir).ok();
        let agents_path = codex_dir.join("AGENTS.md");
        std::fs::write(&agents_path, "# test").ok();

        let result = validate_file_path(&agents_path.to_string_lossy(), None);
        assert!(
            result.is_ok(),
            "expected Ok for ~/.codex/AGENTS.md, got: {:?}",
            result
        );

        // Clean up test file (leave dir — user may have real files)
        std::fs::remove_file(&agents_path).ok();
    }

    #[test]
    fn validate_allows_ancestor_agents_md_via_git_root() {
        // Create a fake repo: root/.git + root/packages/frontend/
        let root = tempfile::tempdir().unwrap();
        let git_dir = root.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let sub = root.path().join("packages").join("frontend");
        std::fs::create_dir_all(&sub).unwrap();

        // Write AGENTS.md at repo root (ancestor of cwd)
        let agents = root.path().join("AGENTS.md");
        std::fs::write(&agents, "# root agents").unwrap();

        // extra_allowed = sub (the cwd), but target is at repo root
        let result = validate_file_path(&agents.to_string_lossy(), Some(&sub.to_string_lossy()));
        assert!(
            result.is_ok(),
            "expected Ok for repo-root AGENTS.md with sub cwd, got: {:?}",
            result
        );
    }

    #[test]
    fn validate_denies_outside_git_root() {
        // Paths outside the git root should still be rejected
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // A file outside the repo entirely
        let outside = root.path().join("outside.md");
        std::fs::write(&outside, "nope").unwrap();

        let result = validate_file_path(&outside.to_string_lossy(), Some(&sub.to_string_lossy()));
        assert!(result.is_err(), "expected Err for path outside git root");
    }

    #[test]
    fn list_memory_files_hierarchical_codex_candidates_with_git() {
        // Create repo: root/.git, root/packages/frontend/ as cwd
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        let sub = root.path().join("packages").join("frontend");
        std::fs::create_dir_all(&sub).unwrap();

        // Write one AGENTS.md at root to verify exists flag
        std::fs::write(root.path().join("AGENTS.md"), "# root").unwrap();

        let result = list_memory_files(Some(sub.to_string_lossy().to_string()));
        assert!(result.is_ok());
        let files = result.unwrap();

        // Filter to Codex project candidates (contain "AGENTS")
        let codex: Vec<_> = files
            .iter()
            .filter(|f| f.scope == "project" && f.label.contains("AGENTS"))
            .collect();

        // 3 dirs (root, packages, packages/frontend) × 2 files = 6 candidates
        assert_eq!(codex.len(), 6, "codex candidates: {:?}", codex);

        // First pair is root level: override first, then base
        assert_eq!(codex[0].label, "AGENTS.override.md");
        assert!(!codex[0].exists);
        assert_eq!(codex[1].label, "AGENTS.md");
        assert!(codex[1].exists); // we wrote this one

        // Second pair is packages/
        assert_eq!(codex[2].label, "packages/AGENTS.override.md");
        assert!(!codex[2].exists);
        assert_eq!(codex[3].label, "packages/AGENTS.md");
        assert!(!codex[3].exists);

        // Third pair is packages/frontend/
        assert_eq!(codex[4].label, "packages/frontend/AGENTS.override.md");
        assert!(!codex[4].exists);
        assert_eq!(codex[5].label, "packages/frontend/AGENTS.md");
        assert!(!codex[5].exists);
    }

    #[cfg(unix)]
    #[test]
    fn scan_memory_md_files_skips_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let mem = root.path().join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("real.md"), "real").unwrap();
        std::os::unix::fs::symlink(&mem, mem.join("loop")).unwrap();

        let files = scan_memory_md_files(&mem, &mem, 3, 50);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].label, "real.md");
    }

    // ── path_exists ──

    // ── agents_md_exists ──

    #[test]
    fn agents_md_exists_returns_true_when_file_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "hi").unwrap();
        let r = agents_md_exists(tmp.path().to_string_lossy().to_string()).unwrap();
        assert!(r);
    }

    #[test]
    fn agents_md_exists_returns_false_when_file_absent() {
        // /init's primary path depends on this.
        let tmp = tempfile::tempdir().unwrap();
        let r = agents_md_exists(tmp.path().to_string_lossy().to_string()).unwrap();
        assert!(!r);
    }

    #[test]
    fn agents_md_exists_rejects_traversal_in_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd_with_traversal = format!("{}/..", tmp.path().to_string_lossy());
        assert!(agents_md_exists(cwd_with_traversal).is_err());
    }

    #[test]
    fn agents_md_exists_rejects_empty_cwd() {
        assert!(agents_md_exists(String::new()).is_err());
    }

    #[test]
    fn agents_md_exists_rejects_relative_cwd() {
        // Forces callers to pass canonical absolute paths and prevents
        // implicit fallback to the process cwd.
        assert!(agents_md_exists("relative/path".to_string()).is_err());
    }

    #[test]
    fn agents_md_exists_does_not_probe_other_files() {
        // The IPC must not be a general filesystem probe. Even with cwd="/"
        // (which an attacker could supply), the only path checked is
        // "/AGENTS.md" — never "/etc/passwd" or any other attacker-chosen
        // file. Verify by writing a non-AGENTS file in the temp dir and
        // confirming the command reports false.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("etc-passwd-stand-in"), "secret").unwrap();
        let r = agents_md_exists(tmp.path().to_string_lossy().to_string()).unwrap();
        assert!(!r, "command must not return true for non-AGENTS.md files");
    }
}

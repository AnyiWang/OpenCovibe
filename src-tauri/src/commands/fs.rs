use crate::models::{DirEntry, DirListing};
use base64::Engine;

const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "target",
    "__pycache__",
    ".next",
    ".svelte-kit",
    ".turbo",
];

#[tauri::command]
pub fn list_directory(path: String, show_hidden: Option<bool>) -> Result<DirListing, String> {
    let show_hidden = show_hidden.unwrap_or(false);
    log::debug!(
        "[fs] list_directory: path={}, show_hidden={}",
        path,
        show_hidden
    );
    let dir = std::path::Path::new(&path);
    if !dir.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    if !dir.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let mut entries: Vec<DirEntry> = vec![];
    let read_dir = std::fs::read_dir(dir).map_err(|e| e.to_string())?;

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files unless requested
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        // Always skip noise directories
        if metadata.is_dir() && EXCLUDED_DIRS.contains(&name.as_str()) {
            continue;
        }
        entries.push(DirEntry {
            name,
            is_dir: metadata.is_dir(),
            size: metadata.len(),
        });
    }

    entries.sort_by(|a, b| {
        // Directories first, then alphabetical
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(DirListing {
        path: path.to_string(),
        entries,
    })
}

#[tauri::command]
pub fn check_is_directory(path: String) -> bool {
    let result = std::path::Path::new(&path).is_dir();
    log::debug!("[fs] check_is_directory: path={path}, result={result}");
    result
}

/// Maximum file size for drag-drop (100 MB)
/// Business requirement: Prevent OOM on large binary files
const MAX_DRAG_FILE_SIZE: u64 = 100 * 1024 * 1024;

#[tauri::command]
pub fn read_file_base64(path: String) -> Result<(String, String), String> {
    let p = std::path::Path::new(&path);
    log::debug!("[fs] read_file_base64: path={path}");
    let meta = p
        .metadata()
        .map_err(|e| format!("Cannot stat {}: {}", path, e))?;

    // Enforce 100MB business limit
    if meta.len() > MAX_DRAG_FILE_SIZE {
        return Err(format!(
            "File too large ({} MB, max {} MB): {}",
            meta.len() / (1024 * 1024),
            MAX_DRAG_FILE_SIZE / (1024 * 1024),
            path
        ));
    }

    // Use mime_guess for comprehensive MIME type detection
    let mime = mime_guess_from_path(p);
    let bytes = std::fs::read(p).map_err(|e| format!("Failed to read {}: {}", path, e))?;

    // Use standard base64 library instead of manual implementation
    let base64 = base64::prelude::BASE64_STANDARD.encode(&bytes);
    log::debug!(
        "[fs] read_file_base64: done path={path}, mime={mime}, size={}",
        bytes.len()
    );
    Ok((base64, mime))
}

/// Detect MIME type from file path with Office format support.
///
/// Office formats are checked first (hardcoded table for accuracy),
/// then falls back to mime_guess library for all other formats.
fn mime_guess_from_path(path: &std::path::Path) -> String {
    // Office formats first — mime_guess is inaccurate for some of these
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if let Some(mime) = office_mime(ext) {
            return mime.into();
        }
    }
    // Fallback to mime_guess for non-Office formats
    mime_guess::from_path(path)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".into())
}

fn office_mime(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "xls" => Some("application/vnd.ms-excel"),
        "csv" => Some("text/csv"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "doc" => Some("application/msword"),
        "docm" => Some("application/vnd.ms-word.document.macroEnabled.12"),
        "dotx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.template"),
        "dotm" => Some("application/vnd.ms-word.template.macroEnabled.12"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "ppt" => Some("application/vnd.ms-powerpoint"),
        "pptm" => Some("application/vnd.ms-powerpoint.presentation.macroEnabled.12"),
        "potx" => Some("application/vnd.openxmlformats-officedocument.presentationml.template"),
        "potm" => Some("application/vnd.ms-powerpoint.template.macroEnabled.12"),
        _ => None,
    }
}

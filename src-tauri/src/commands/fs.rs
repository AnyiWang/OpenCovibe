use base64::Engine;
use crate::models::{DirEntry, DirListing};

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
    std::path::Path::new(&path).is_dir()
}

/// Maximum file size for drag-drop (100 MB)
/// Business requirement: Prevent OOM on large binary files
const MAX_DRAG_FILE_SIZE: u64 = 100 * 1024 * 1024;

#[tauri::command]
pub fn read_file_base64(path: String) -> Result<(String, String), String> {
    let p = std::path::Path::new(&path);
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
    Ok((base64, mime))
}

/// Detect MIME type from file path with Office format support
///
/// Uses mime_guess library for comprehensive type detection,
/// with manual fallback for Office formats to ensure accuracy.
fn mime_guess_from_path(path: &std::path::Path) -> String {
    // First try mime_guess library (covers most formats)
    if let Some(mime) = mime_guess::from_path(path).first() {
        let mime_str = mime.to_string();

        // mime_guess doesn't cover all Office formats accurately,
        // so we use explicit mappings for better precision
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                // Office formats (explicit for accuracy)
                "xlsx" => return "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
                "xls" => return "application/vnd.ms-excel".into(),
                "csv" => return "text/csv".into(),
                "docx" => return "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
                "doc" => return "application/msword".into(),
                "docm" => return "application/vnd.ms-word.document.macroEnabled.12".into(),
                "dotx" => return "application/vnd.openxmlformats-officedocument.wordprocessingml.template".into(),
                "dotm" => return "application/vnd.ms-word.template.macroEnabled.12".into(),
                "pptx" => return "application/vnd.openxmlformats-officedocument.presentationml.presentation".into(),
                "ppt" => return "application/vnd.ms-powerpoint".into(),
                "pptm" => return "application/vnd.ms-powerpoint.presentation.macroEnabled.12".into(),
                "potx" => return "application/vnd.openxmlformats-officedocument.presentationml.template".into(),
                "potm" => return "application/vnd.ms-powerpoint.template.macroEnabled.12".into(),
                // Fallback to mime_guess for non-Office formats
                _ => return mime_str,
            }
        }
        return mime_str;
    }

    // Ultimate fallback
    "application/octet-stream".into()
}

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

const MAX_DRAG_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100MB

#[tauri::command]
pub fn read_file_base64(path: String) -> Result<(String, String), String> {
    let p = std::path::Path::new(&path);
    let meta = p.metadata().map_err(|e| format!("Cannot stat {}: {}", path, e))?;
    if meta.len() > MAX_DRAG_FILE_SIZE {
        return Err(format!("File too large ({} MB, max 100 MB): {}", meta.len() / (1024 * 1024), path));
    }
    let mime = mime_from_ext(p.extension().and_then(|e| e.to_str()).unwrap_or(""));
    let bytes = std::fs::read(p).map_err(|e| format!("Failed to read {}: {}", path, e))?;
    let base64 = base64_encode(&bytes);
    Ok((base64, mime))
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn mime_from_ext(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "js" | "mjs" => "text/javascript",
        "ts" | "mts" => "text/typescript",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "yaml" | "yml" => "text/yaml",
        "toml" => "text/toml",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "sh" | "bash" | "zsh" => "text/x-shellscript",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

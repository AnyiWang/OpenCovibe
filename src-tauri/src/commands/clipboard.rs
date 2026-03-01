use crate::models::max_attachment_size;
use serde::Serialize;
use std::path::PathBuf;

/// Allowed file extensions for clipboard paste (whitelist).
const ALLOWED_CLIPBOARD_EXTENSIONS: &[&str] = &[
    // Images
    "png", "jpg", "jpeg", "webp", "gif", // Documents
    "pdf", // Text (aligned with frontend TEXT_EXTENSIONS)
    "txt", "md", "json", "ts", "tsx", "js", "jsx", "py", "rs", "svelte", "html", "css", "scss",
    "yaml", "yml", "toml", "xml", "sh", "bash", "sql", "go", "java", "c", "cpp", "h", "hpp", "rb",
    "php", "swift", "kt", "csv", "log", "env", "cfg", "ini", "conf", "vue", "astro", "prisma",
    "graphql",
];

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardFileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
}

#[derive(Debug, Serialize)]
pub struct ClipboardFileContent {
    pub content_base64: String,
    pub content_text: Option<String>,
}

/// Validate a clipboard file path: must exist, be a regular file, within size limit, allowed extension.
fn validate_clipboard_path(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err("file not found".into());
    }
    if !p.is_file() {
        return Err("not a regular file".into());
    }
    let meta = std::fs::metadata(&p).map_err(|e| format!("metadata error: {}", e))?;
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !ALLOWED_CLIPBOARD_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("unsupported extension: .{}", ext));
    }
    // Size check: images have no limit (CLI compresses), others 10MB
    let mime = mime_from_extension(&ext);
    let limit = max_attachment_size(mime);
    if limit < u64::MAX && meta.len() > limit {
        let limit_mb = limit / (1024 * 1024);
        return Err(format!("file too large (>{}MB)", limit_mb));
    }
    Ok(p)
}

/// Infer MIME type from file extension.
fn mime_from_extension(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "html" => "text/html",
        "css" => "text/css",
        "xml" => "text/xml",
        "csv" => "text/csv",
        "svg" => "image/svg+xml",
        // All other recognized text extensions
        "txt" | "md" | "ts" | "tsx" | "js" | "jsx" | "py" | "rs" | "svelte" | "scss" | "yaml"
        | "yml" | "toml" | "sh" | "bash" | "sql" | "go" | "java" | "c" | "cpp" | "h" | "hpp"
        | "rb" | "php" | "swift" | "kt" | "log" | "env" | "cfg" | "ini" | "conf" | "vue"
        | "astro" | "prisma" | "graphql" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Build ClipboardFileInfo entries from an iterator of file paths.
/// Shared logic for macOS and Linux clipboard reading.
#[cfg(not(windows))]
fn paths_to_clipboard_infos(paths: impl Iterator<Item = String>) -> Vec<ClipboardFileInfo> {
    let mut results = Vec::new();
    for path_str in paths {
        let p = PathBuf::from(&path_str);
        if !p.exists() || !p.is_file() {
            log::debug!("[clipboard] skipping {}: not a regular file", path_str);
            continue;
        }
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mime_type = mime_from_extension(&ext).to_string();

        log::debug!("[clipboard] file: {} ({}, {} bytes)", name, mime_type, size);
        results.push(ClipboardFileInfo {
            path: path_str,
            name,
            size,
            mime_type,
        });
    }
    results
}

/// Parse a text/uri-list payload into file paths.
/// Skips comment lines (starting with #) and non-file:// URIs.
#[cfg(any(target_os = "linux", test))]
fn parse_uri_list(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(path) = file_uri_to_path(line) {
            paths.push(path);
        }
    }
    paths
}

/// Convert a file:// URI to a local filesystem path.
/// Uses the `url` crate for safe percent-decoding.
#[cfg(any(target_os = "linux", test))]
fn file_uri_to_path(uri: &str) -> Option<String> {
    let parsed = url::Url::parse(uri).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    parsed
        .to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Read file paths from the native clipboard.
///
/// - macOS: uses osascript with AppleScriptObjC to access NSPasteboard file URLs.
/// - Linux: probes wl-paste → xclip → xsel for text/uri-list content.
/// - Other platforms: returns empty.
#[tauri::command]
pub fn get_clipboard_files() -> Result<Vec<ClipboardFileInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        log::debug!("[clipboard] reading native clipboard for file URLs");

        let script = r#"
use framework "AppKit"
set pb to current application's NSPasteboard's generalPasteboard()
set fileURLs to pb's readObjectsForClasses:{current application's NSURL} options:(missing value)
if fileURLs is missing value then return ""
set paths to {}
repeat with u in fileURLs
    set end of paths to (u's |path|() as text)
end repeat
set AppleScript's text item delimiters to linefeed
return paths as text
"#;

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("osascript failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("[clipboard] osascript error: {}", stderr);
            // Not an error — clipboard may simply not contain files
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw = stdout.trim();
        if raw.is_empty() {
            log::debug!("[clipboard] no file URLs in clipboard");
            return Ok(vec![]);
        }

        let results = paths_to_clipboard_infos(
            raw.lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string()),
        );

        log::debug!("[clipboard] returning {} files", results.len());
        Ok(results)
    }

    #[cfg(target_os = "linux")]
    {
        log::debug!("[clipboard] reading Linux clipboard for file URLs");

        // Probe clipboard tools in priority order: wl-paste (Wayland) → xclip → xsel (X11)
        let tools: &[(&str, &[&str])] = &[
            ("wl-paste", &["--type", "text/uri-list"]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "text/uri-list", "-o"],
            ),
            ("xsel", &["--clipboard", "--output"]),
        ];

        for (bin, args) in tools {
            if let Some(raw) = try_clipboard_tool(bin, args) {
                let parsed = parse_uri_list(&raw);
                if !parsed.is_empty() {
                    let results = paths_to_clipboard_infos(parsed.into_iter());
                    log::debug!("[clipboard] {} returned {} files", bin, results.len());
                    return Ok(results);
                }
                log::debug!(
                    "[clipboard] {} returned data but no file:// URIs, trying next tool",
                    bin
                );
            }
        }

        log::warn!(
            "[clipboard] no clipboard tool found or no file URIs (tried wl-paste, xclip, xsel)"
        );
        Ok(vec![])
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        log::debug!("[clipboard] unsupported platform, returning empty");
        Ok(vec![])
    }
}

/// Try running a clipboard tool, returning its stdout on success.
#[cfg(target_os = "linux")]
fn try_clipboard_tool(bin: &str, args: &[&str]) -> Option<String> {
    match std::process::Command::new(bin).args(args).output() {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).into_owned();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        }
        Ok(_) => None,  // command exists but clipboard has no matching content
        Err(_) => None, // command not found
    }
}

/// Read a single clipboard file's content (base64, optionally as text).
#[tauri::command]
pub fn read_clipboard_file(path: String, as_text: bool) -> Result<ClipboardFileContent, String> {
    log::debug!("[clipboard] reading file: {} (as_text={})", path, as_text);

    let p = validate_clipboard_path(&path)?;
    let bytes = std::fs::read(&p).map_err(|e| format!("read error: {}", e))?;

    use base64::Engine;
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let content_text = if as_text {
        match String::from_utf8(bytes) {
            Ok(s) => Some(s),
            Err(_) => {
                log::debug!("[clipboard] file is not valid UTF-8, returning base64 only");
                None
            }
        }
    } else {
        None
    };

    Ok(ClipboardFileContent {
        content_base64,
        content_text,
    })
}

/// Save file content to a temp directory, returning the filesystem path.
/// Used for >20MB PDFs from drag-and-drop/file picker: file is saved to temp,
/// then its path is injected into the prompt text so CLI handles via pdftoppm.
#[tauri::command]
pub fn save_temp_attachment(name: String, content_base64: String) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join("opencovibe-attachments");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("mkdir: {}", e))?;

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&content_base64)
        .map_err(|e| format!("base64 decode: {}", e))?;

    // Unique prefix to avoid collisions
    let unique_name = format!("{}_{}", &uuid::Uuid::new_v4().to_string()[..8], name);
    let path = tmp_dir.join(&unique_name);
    std::fs::write(&path, &bytes).map_err(|e| format!("write: {}", e))?;

    let result = path.to_string_lossy().to_string();
    log::debug!(
        "[clipboard] saved temp attachment: {} ({} bytes) → {}",
        name,
        bytes.len(),
        result
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_clipboard_path_existing_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.pdf");
        std::fs::write(&file_path, b"fake pdf content").unwrap();

        let result = validate_clipboard_path(file_path.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_clipboard_path_not_found() {
        let result = validate_clipboard_path("/nonexistent/path/to/file.pdf");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn validate_clipboard_path_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("malware.exe");
        std::fs::write(&file_path, b"bad stuff").unwrap();

        let result = validate_clipboard_path(file_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unsupported extension"));
    }

    #[test]
    fn validate_clipboard_path_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = validate_clipboard_path(dir.path().to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a regular file"));
    }

    #[test]
    fn validate_clipboard_path_text_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.txt");
        // Use set_len() for sparse file — no real allocation
        let f = std::fs::File::create(&file_path).unwrap();
        f.set_len(10 * 1024 * 1024 + 1).unwrap(); // just over 10MB
        drop(f);

        let result = validate_clipboard_path(file_path.to_str().unwrap());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("too large"));
        assert!(err.contains("10MB"));
    }

    #[test]
    fn validate_clipboard_path_image_no_size_limit() {
        let dir = tempfile::tempdir().unwrap();

        // Large images pass — CLI handles compression via sharp
        let big_path = dir.path().join("big.png");
        let big_data = vec![0u8; 15 * 1024 * 1024]; // 15MB image
        std::fs::write(&big_path, &big_data).unwrap();
        assert!(validate_clipboard_path(big_path.to_str().unwrap()).is_ok());
    }

    #[test]
    fn validate_clipboard_path_pdf_20mb_limit() {
        let dir = tempfile::tempdir().unwrap();

        // 15MB PDF → Ok (below 20MB limit)
        let ok_path = dir.path().join("ok.pdf");
        let f = std::fs::File::create(&ok_path).unwrap();
        f.set_len(15 * 1024 * 1024).unwrap(); // sparse file
        drop(f);
        assert!(validate_clipboard_path(ok_path.to_str().unwrap()).is_ok());

        // 20MB + 1 PDF → Err
        let big_path = dir.path().join("big.pdf");
        let f = std::fs::File::create(&big_path).unwrap();
        f.set_len(20 * 1024 * 1024 + 1).unwrap(); // sparse file
        drop(f);
        let result = validate_clipboard_path(big_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("20MB"));
    }

    #[test]
    fn mime_from_extension_known_types() {
        assert_eq!(mime_from_extension("png"), "image/png");
        assert_eq!(mime_from_extension("jpg"), "image/jpeg");
        assert_eq!(mime_from_extension("jpeg"), "image/jpeg");
        assert_eq!(mime_from_extension("pdf"), "application/pdf");
        assert_eq!(mime_from_extension("ts"), "text/plain");
        assert_eq!(mime_from_extension("py"), "text/plain");
        assert_eq!(mime_from_extension("html"), "text/html");
        assert_eq!(mime_from_extension("css"), "text/css");
        assert_eq!(mime_from_extension("json"), "application/json");
    }

    #[test]
    fn mime_from_extension_unknown() {
        assert_eq!(mime_from_extension("zip"), "application/octet-stream");
        assert_eq!(mime_from_extension("exe"), "application/octet-stream");
        assert_eq!(mime_from_extension(""), "application/octet-stream");
    }

    #[test]
    fn validate_clipboard_path_allowed_text_extensions() {
        let dir = tempfile::tempdir().unwrap();
        for ext in &["ts", "py", "rs", "go", "json", "md", "txt", "svelte"] {
            let file_path = dir.path().join(format!("test.{}", ext));
            std::fs::write(&file_path, "content").unwrap();
            let result = validate_clipboard_path(file_path.to_str().unwrap());
            assert!(result.is_ok(), "expected .{} to be allowed", ext);
        }
    }

    #[test]
    fn parse_file_uri_basic() {
        let result = file_uri_to_path("file:///home/user/doc.txt");
        assert_eq!(result, Some("/home/user/doc.txt".to_string()));
    }

    #[test]
    fn parse_file_uri_with_spaces() {
        let result = file_uri_to_path("file:///home/user/my%20file.txt");
        assert_eq!(result, Some("/home/user/my file.txt".to_string()));
    }

    #[test]
    fn parse_file_uri_ignores_non_file() {
        assert_eq!(file_uri_to_path("http://example.com/file.txt"), None);
        assert_eq!(file_uri_to_path("https://example.com/file.txt"), None);
        assert_eq!(file_uri_to_path("ftp://example.com/file.txt"), None);
    }

    #[test]
    fn parse_file_uri_ignores_comments() {
        let input = "# comment line\nfile:///home/user/doc.txt\n# another comment\n";
        let result = parse_uri_list(input);
        assert_eq!(result, vec!["/home/user/doc.txt".to_string()]);
    }

    #[test]
    fn parse_uri_list_multiple() {
        let input = "file:///home/user/a.txt\nfile:///home/user/b.pdf\n";
        let result = parse_uri_list(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "/home/user/a.txt");
        assert_eq!(result[1], "/home/user/b.pdf");
    }

    #[test]
    fn parse_uri_list_mixed() {
        let input = "# comment\nfile:///home/user/doc.txt\nhttps://example.com\n\n";
        let result = parse_uri_list(input);
        assert_eq!(result, vec!["/home/user/doc.txt".to_string()]);
    }

    #[test]
    fn validate_clipboard_path_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("Makefile");
        std::fs::write(&file_path, "all: build").unwrap();

        let result = validate_clipboard_path(file_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unsupported extension"));
    }
}

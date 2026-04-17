use crate::storage;
use std::path::Path;

#[tauri::command]
pub fn export_conversation(run_id: String) -> Result<String, String> {
    log::debug!("[export] export_conversation: run_id={}", run_id);
    storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;
    let events = storage::events::list_events(&run_id, 0);
    let mut md = String::new();
    md.push_str(&format!("# Conversation — {}\n\n", run_id));

    for event in events {
        let type_str = format!("{}", event.event_type);
        if type_str != "user" && type_str != "assistant" {
            continue;
        }
        let text = event
            .payload
            .get("text")
            .or_else(|| event.payload.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let role = if type_str == "user" {
            "User"
        } else {
            "Assistant"
        };
        md.push_str(&format!("## {}\n\n{}\n\n---\n\n", role, text));
    }

    Ok(md)
}

#[tauri::command]
pub async fn write_html_export(path: String, content: String) -> Result<(), String> {
    log::debug!(
        "[export] write_html_export: path={}, content_len={}",
        path,
        content.len()
    );

    let ext = Path::new(&path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html") | Some("htm") => {}
        _ => {
            log::error!(
                "[export] write_html_export rejected non-html path: {}",
                path
            );
            return Err("write_html_export: only .html/.htm paths allowed".into());
        }
    }

    tokio::fs::write(&path, content).await.map_err(|e| {
        log::error!("[export] write_html_export failed: {}", e);
        e.to_string()
    })
}

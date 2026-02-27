use crate::storage;

#[tauri::command]
pub fn export_conversation(run_id: String) -> Result<String, String> {
    log::debug!("[export] export_conversation: run_id={}", run_id);
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

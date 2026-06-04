use crate::agent::spawn::build_agent_command;
use crate::agent::stream::{run_agent, ProcessMap};
use crate::models::{
    max_attachment_size, Attachment, AttachmentMeta, BusEvent, ConversationRef, RunEventType,
    RunStatus,
};
use crate::storage;
use crate::web_server::broadcaster::BroadcastEmitter;
use std::fs;
use std::sync::Arc;
use tauri::Emitter;

fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let truncated = if cleaned.len() > 120 {
        &cleaned[..120]
    } else {
        &cleaned
    };
    if truncated.is_empty() {
        "attachment.bin".to_string()
    } else {
        truncated.to_string()
    }
}

fn extension_for_mime(mime: &str) -> &str {
    if mime.starts_with("image/png") {
        return ".png";
    }
    if mime.starts_with("image/jpeg") {
        return ".jpg";
    }
    if mime.starts_with("image/webp") {
        return ".webp";
    }
    if mime.starts_with("image/gif") {
        return ".gif";
    }
    if mime.starts_with("application/pdf") {
        return ".pdf";
    }
    if mime.starts_with("text/markdown") {
        return ".md";
    }
    if mime.starts_with("text/plain") {
        return ".txt";
    }
    if mime.contains("json") {
        return ".json";
    }
    ""
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn send_chat_message(
    app: tauri::AppHandle,
    process_map: tauri::State<'_, ProcessMap>,
    emitter: tauri::State<'_, Arc<BroadcastEmitter>>,
    run_id: String,
    message: String,
    attachments: Option<Vec<Attachment>>,
    model: Option<String>,
    client_uuid: Option<String>,
) -> Result<(), String> {
    log::debug!(
        "[chat] send_chat_message: run_id={}, msg_len={}, attachments={}, client_uuid={:?}",
        run_id,
        message.len(),
        attachments.as_ref().map_or(0, |a| a.len()),
        client_uuid
    );
    let run = storage::runs::get_run(&run_id).ok_or_else(|| format!("Run {} not found", run_id))?;

    // Validate execution path — send_chat_message is the pipe_exec path
    let exec_path = run.resolved_execution_path();
    if exec_path != crate::models::ExecutionPath::PipeExec {
        return Err(format!(
            "send_chat_message requires execution_path=pipe_exec, got {:?} for run {}",
            exec_path, run_id
        ));
    }

    // Resume validation: reject if conversation_ref exists but session persistence is disabled
    if run.conversation_ref.is_some() {
        let agent_settings = storage::settings::get_agent_settings(&run.agent);
        if agent_settings.no_session_persistence.unwrap_or(false) {
            return Err("Cannot resume: session persistence is disabled".to_string());
        }
    }

    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("message is required".to_string());
    }

    // Handle attachments
    let attachments = attachments.unwrap_or_default();
    let mut attachment_paths: Vec<(String, String, String, u64)> = vec![]; // (path, name, type, size)
    let mut attachment_metas: Vec<AttachmentMeta> = vec![];

    if !attachments.is_empty() {
        let upload_dir = std::env::temp_dir()
            .join("opencovibe-uploads")
            .join(&run_id);
        fs::create_dir_all(&upload_dir).map_err(|e| e.to_string())?;

        for att in attachments.iter().take(8) {
            if att.content_base64.is_empty() {
                continue;
            }
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&att.content_base64)
                .map_err(|e| e.to_string())?;
            if bytes.is_empty() {
                continue;
            }
            let limit = max_attachment_size(&att.mime_type) as usize;
            if bytes.len() > limit {
                log::warn!(
                    "[chat] skipping oversized attachment: {} ({} bytes > {} limit)",
                    att.name,
                    bytes.len(),
                    limit
                );
                continue;
            }

            let base = safe_filename(&att.name);
            let ext = extension_for_mime(&att.mime_type);
            let filename = format!(
                "{}-{}-{}{}",
                chrono::Utc::now().timestamp_millis(),
                &uuid::Uuid::new_v4().to_string()[..6],
                base,
                ext
            );
            let full_path = upload_dir.join(&filename);
            fs::write(&full_path, &bytes).map_err(|e| e.to_string())?;
            attachment_paths.push((
                full_path.to_string_lossy().to_string(),
                att.name.clone(),
                att.mime_type.clone(),
                att.size,
            ));
            attachment_metas.push(AttachmentMeta {
                name: att.name.clone(),
                mime_type: att.mime_type.clone(),
                size: att.size,
            });
        }
    }

    // Build prompt with attachments
    let attachment_text = if !attachment_paths.is_empty() {
        let files: Vec<String> = attachment_paths
            .iter()
            .map(|(path, name, mime, size)| {
                format!("- {} ({}, {} bytes) => {}", name, mime, size, path)
            })
            .collect();
        format!(
            "\n\nAttached files:\n{}\nUse these local file paths directly when needed.",
            files.join("\n")
        )
    } else {
        String::new()
    };
    let full_prompt = format!("{}{}", message, attachment_text);

    // Add user event (legacy events.jsonl)
    let att_json: Vec<serde_json::Value> = attachment_paths
        .iter()
        .map(|(path, name, mime, size)| {
            serde_json::json!({ "name": name, "type": mime, "size": size, "path": path })
        })
        .collect();

    if let Err(e) = storage::events::append_event(
        &run_id,
        RunEventType::User,
        serde_json::json!({
            "text": message,
            "source": "ui_chat",
            "attachments": att_json
        }),
    ) {
        log::warn!("[chat] failed to log user event: {}", e);
    }

    // Emit UserMessage bus event
    emitter.persist_and_emit(
        &run_id,
        &BusEvent::UserMessage {
            run_id: run_id.clone(),
            text: message.clone(),
            uuid: None,
            client_uuid: client_uuid.clone(),
            attachments: attachment_metas,
        },
    );

    // Pipe mode (Codex / Claude --print)
    log::debug!(
        "[chat] spawning pipe mode: run_id={}, agent={}",
        run_id,
        run.agent
    );
    // Update run status to running
    if let Err(e) = storage::runs::update_status(&run_id, RunStatus::Running, None, None) {
        log::warn!("[chat] failed to update status to Running: {}", e);
    }

    // Build unified adapter settings
    let agent_settings = storage::settings::get_agent_settings(&run.agent);
    let user_settings = storage::settings::get_user_settings();
    let adapter_settings =
        crate::agent::adapter::build_adapter_settings(&agent_settings, &user_settings, model);

    // Resolve resume thread_id from conversation_ref
    let resume_tid = run.conversation_ref.as_ref().and_then(|r| match r {
        ConversationRef::CodexThread(tid) => Some(tid.as_str()),
        _ => None,
    });

    // Image attachments → Codex --image (real vision input; the text breadcrumb above
    // still records all attachment paths). Non-image files Codex reads via tools.
    let image_paths: Vec<String> = attachment_paths
        .iter()
        .filter(|(_, _, mime, _)| mime.starts_with("image/"))
        .map(|(path, _, _, _)| path.clone())
        .collect();

    // Build command
    let (command, args) = build_agent_command(
        &run.agent,
        &full_prompt,
        &adapter_settings,
        true, // print mode
        resume_tid,
        &image_paths,
    )?;

    // Record the model Codex will actually use (from built command args)
    if run.agent == "codex" {
        let codex_model = args
            .iter()
            .position(|a| a == "--model")
            .and_then(|i| args.get(i + 1))
            .cloned();
        if let Some(ref m) = codex_model {
            if let Err(e) = storage::runs::update_run_model(&run_id, m) {
                log::warn!("[chat] failed to record codex model: {}", e);
            }
        }
    }

    // Codex third-party provider: supply the API key via the env var named by env_key
    // (the -c model_providers.*.env_key override was already added in build_agent_command).
    let mut extra_env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(p) = &adapter_settings.codex_provider {
        if let Some((k, v)) = crate::agent::spawn::codex_provider_env(p) {
            extra_env.insert(k, v);
        }
    }

    // Spawn agent in background
    let pm = process_map.inner().clone();
    let em = emitter.inner().clone();
    let app_clone = app.clone();
    let run_id_clone = run_id.clone();
    let agent_clone = run.agent.clone();
    let cwd = run.cwd.clone();

    tokio::spawn(async move {
        if let Err(e) = run_agent(
            app_clone.clone(),
            pm,
            run_id_clone.clone(),
            command,
            args,
            cwd,
            agent_clone,
            Some(em.clone()),
            extra_env,
        )
        .await
        {
            if let Err(e2) = storage::runs::update_status(
                &run_id_clone,
                RunStatus::Failed,
                Some(1),
                Some(e.clone()),
            ) {
                log::warn!("[chat] failed to update status to Failed: {}", e2);
            }
            // Emit RunState so timeline mode (which ignores chat-done) transitions phase
            em.persist_and_emit(
                &run_id_clone,
                &BusEvent::RunState {
                    run_id: run_id_clone.clone(),
                    state: "failed".to_string(),
                    exit_code: Some(1),
                    error: Some(e.clone()),
                },
            );
            let _ = app_clone.emit(
                "chat-done",
                crate::models::ChatDone {
                    ok: false,
                    code: 1,
                    error: None,
                },
            );
        }
    });

    Ok(())
}

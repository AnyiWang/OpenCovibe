use crate::agent::codex_parser::extract_codex_delta;
use crate::models::{ChatDelta, ChatDone, RunEventType};
use crate::storage;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub type ProcessMap = Arc<Mutex<HashMap<String, Child>>>;

pub fn new_process_map() -> ProcessMap {
    Arc::new(Mutex::new(HashMap::new()))
}

pub async fn run_agent(
    app: AppHandle,
    process_map: ProcessMap,
    run_id: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    agent: String,
) -> Result<(), String> {
    log::debug!(
        "[stream] run_agent: run_id={}, cmd={}, args={:?}, cwd={}, agent={}",
        run_id,
        command,
        args,
        cwd,
        agent
    );

    let emit_run_event = |rt: RunEventType, payload: serde_json::Value| {
        if let Err(e) = storage::events::append_event(&run_id, rt, payload) {
            log::warn!(
                "[stream] failed to append event for run_id={}: {}",
                run_id,
                e
            );
        }
    };

    // Log start
    emit_run_event(
        RunEventType::System,
        serde_json::json!({
            "message": format!("Started {} {}", command, args.join(" ")),
            "source": "ui_chat"
        }),
    );

    let mut child = Command::new(&command)
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OPENCOVIBE_TASK_ID", &run_id)
        .env("OPENCOVIBE_RUN_ID", &run_id)
        .env_remove("CLAUDECODE") // Allow running inside a Claude Code session
        .spawn()
        .map_err(|e| {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "Command \"{}\" not found. Is {} CLI installed and in your PATH?",
                    command, agent
                )
            } else {
                e.to_string()
            };
            log::error!("[stream] spawn failed: {}", msg);
            msg
        })?;

    let pid = child.id().unwrap_or(0);
    log::debug!("[stream] spawned process: run_id={}, pid={}", run_id, pid);

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Store child for stop_run
    {
        let mut map = process_map.lock().await;
        map.insert(run_id.clone(), child);
    }

    let run_id_out = run_id.clone();
    let run_id_err = run_id.clone();
    let app_out = app.clone();
    let agent_clone = agent.clone();

    // Stdout reader
    let stdout_handle = tokio::spawn(async move {
        let mut assistant_text = String::new();
        let is_codex = agent_clone == "codex";

        if is_codex {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Err(e) = storage::events::append_event(
                    &run_id_out,
                    RunEventType::Stdout,
                    serde_json::json!({ "text": line, "source": "ui_chat" }),
                ) {
                    log::warn!("[stream] stdout append failed: {}", e);
                }
                let _ = app_out.emit(
                    "run-event",
                    serde_json::json!({
                        "run_id": run_id_out,
                        "type": "stdout",
                        "text": line
                    }),
                );

                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    if let Some(delta) = extract_codex_delta(&payload) {
                        assistant_text.push_str(&delta);
                        let _ = app_out.emit("chat-delta", ChatDelta { text: delta });
                    } else {
                        let evt = payload
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        log::debug!("[codex] unhandled event: type={}", evt);
                    }
                }
            }
        } else {
            // Claude: stdout is the response text
            let mut reader = BufReader::new(stdout);
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        assistant_text.push_str(&text);
                        if let Err(e) = storage::events::append_event(
                            &run_id_out,
                            RunEventType::Stdout,
                            serde_json::json!({ "text": text, "source": "ui_chat" }),
                        ) {
                            log::warn!("[stream] stdout append failed: {}", e);
                        }
                        let _ = app_out.emit(
                            "run-event",
                            serde_json::json!({
                                "run_id": run_id_out,
                                "type": "stdout",
                                "text": text
                            }),
                        );
                        let _ = app_out.emit("chat-delta", ChatDelta { text });
                    }
                    Err(_) => break,
                }
            }
        }

        assistant_text
    });

    // Stderr reader
    let app_err = app.clone();
    let stderr_handle = tokio::spawn(async move {
        let mut stderr_text = String::new();
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            stderr_text.push_str(&line);
            stderr_text.push('\n');
            if let Err(e) = storage::events::append_event(
                &run_id_err,
                RunEventType::Stderr,
                serde_json::json!({ "text": line, "source": "ui_chat" }),
            ) {
                log::warn!("[stream] stderr append failed: {}", e);
            }
            let _ = app_err.emit(
                "run-event",
                serde_json::json!({
                    "run_id": run_id_err,
                    "type": "stderr",
                    "text": line
                }),
            );
        }
        stderr_text
    });

    // Wait for process
    let exit_code = {
        let mut map = process_map.lock().await;
        if let Some(mut child) = map.remove(&run_id) {
            match child.wait().await {
                Ok(status) => status.code().unwrap_or(1),
                Err(_) => 1,
            }
        } else {
            // Was killed by stop_run
            -1
        }
    };

    let assistant_text = stdout_handle.await.unwrap_or_default();
    let _stderr_text = stderr_handle.await.unwrap_or_default();

    // Save assistant event
    if !assistant_text.trim().is_empty() {
        emit_run_event(
            RunEventType::Assistant,
            serde_json::json!({ "text": assistant_text.trim(), "source": "ui_chat" }),
        );
    }

    log::debug!(
        "[stream] process exited: run_id={}, exit_code={}, output_len={}",
        run_id,
        exit_code,
        assistant_text.len()
    );

    // Update run status
    if exit_code == 0 {
        if let Err(e) = storage::runs::update_status(
            &run_id,
            crate::models::RunStatus::Completed,
            Some(0),
            None,
        ) {
            log::warn!("[stream] failed to update status to Completed: {}", e);
        }
    } else if exit_code == -1 {
        if let Err(e) = storage::runs::update_status(
            &run_id,
            crate::models::RunStatus::Stopped,
            None,
            Some("Stopped by user".to_string()),
        ) {
            log::warn!("[stream] failed to update status to Stopped: {}", e);
        }
    } else if let Err(e) = storage::runs::update_status(
        &run_id,
        crate::models::RunStatus::Failed,
        Some(exit_code),
        Some(format!("Exit code {}", exit_code)),
    ) {
        log::warn!("[stream] failed to update status to Failed: {}", e);
    }

    emit_run_event(
        RunEventType::System,
        serde_json::json!({ "message": format!("Process exited with code {}", exit_code), "source": "ui_chat" }),
    );

    let _ = app.emit(
        "chat-done",
        ChatDone {
            ok: exit_code == 0,
            code: exit_code,
            error: None,
        },
    );

    Ok(())
}

pub async fn stop_process(process_map: &ProcessMap, run_id: &str) -> bool {
    log::debug!("[stream] stop_process: run_id={}", run_id);
    let mut map = process_map.lock().await;
    if let Some(mut child) = map.remove(run_id) {
        let _ = child.kill().await;
        let _ = child.wait().await;
        log::debug!("[stream] stop_process: killed run_id={}", run_id);
        true
    } else {
        log::debug!("[stream] stop_process: no process for run_id={}", run_id);
        false
    }
}

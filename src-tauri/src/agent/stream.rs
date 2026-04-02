use crate::agent::pipe_parser::{CodexStdoutParser, PipeStdoutParser};
use crate::models::{BusEvent, ChatDelta, ChatDone, RunEventType};
use crate::process_ext::HideConsole;
use crate::storage;
use crate::web_server::broadcaster::BroadcastEmitter;
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
    emitter: Option<Arc<BroadcastEmitter>>,
) -> Result<(), String> {
    log::debug!(
        "[stream] run_agent: run_id={}, cmd={}, args={:?}, cwd={}, agent={}, has_emitter={}",
        run_id,
        command,
        args,
        cwd,
        agent,
        emitter.is_some()
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

    // Pre-compute process_seq BEFORE spawning the child process.
    // If this fails, we return Err without having spawned anything — no leak.
    let is_codex = agent == "codex";
    let process_seq: u32 = if is_codex && emitter.is_some() {
        storage::runs::next_codex_process_seq(&run_id)?
    } else {
        0
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
        .hide_console()
        .kill_on_drop(true)
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
    let emitter_out = emitter.clone();

    // Stdout reader
    let stdout_handle = tokio::spawn(async move {
        let mut assistant_text = String::new();

        if is_codex {
            let mut parser = CodexStdoutParser::new(process_seq);
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
                    // Capture thread_id as ConversationRef for Codex resume
                    let type_str = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if type_str == "thread.started" {
                        if let Some(tid) = payload.get("thread_id").and_then(|v| v.as_str()) {
                            log::debug!("[codex] captured thread_id={} as conversation_ref", tid);
                            let tid_str = tid.to_string();
                            let rid = run_id_out.clone();
                            if let Err(e) = crate::storage::runs::with_meta(&rid, |meta| {
                                meta.conversation_ref =
                                    Some(crate::models::ConversationRef::CodexThread(tid_str));
                                Ok(())
                            }) {
                                log::warn!("[codex] failed to persist conversation_ref: {}", e);
                            }
                        }
                    }

                    // Use PipeStdoutParser trait for structured event → BusEvent
                    let events = parser.parse_line(&run_id_out, &payload);
                    for ev in &events {
                        // Emit via BroadcastEmitter if available (Phase 2 structured path)
                        if let Some(ref em) = emitter_out {
                            em.persist_and_emit(&run_id_out, ev);
                        }
                        // Also emit chat-delta for backward compatibility (terminal view)
                        if let BusEvent::MessageComplete { text, .. } = ev {
                            assistant_text.push_str(text);
                            let _ = app_out.emit("chat-delta", ChatDelta { text: text.clone() });
                        }
                    }
                    if events.is_empty() && !type_str.is_empty() {
                        log::debug!("[codex] unhandled event: type={}", type_str);
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
    let emitter_err = emitter.clone();
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
            // Emit stderr as CommandOutput via bus
            if let Some(ref em) = emitter_err {
                em.persist_and_emit(
                    &run_id_err,
                    &BusEvent::CommandOutput {
                        run_id: run_id_err.clone(),
                        content: format!("[stderr] {}", line),
                    },
                );
            }
        }
        stderr_text
    });

    // Wait for stdout/stderr to close (= process exited or pipes broken).
    // This completes without holding the ProcessMap lock.
    let assistant_text = stdout_handle.await.unwrap_or_default();
    let _stderr_text = stderr_handle.await.unwrap_or_default();

    // Short lock: remove child from map, then wait() outside the lock.
    // If stop_process already removed+killed the child, we get None → exit_code -1.
    let removed_child = {
        let mut map = process_map.lock().await;
        map.remove(&run_id)
    };
    let exit_code = if let Some(mut child) = removed_child {
        match child.wait().await {
            Ok(status) => status.code().unwrap_or(1),
            Err(_) => 1,
        }
    } else {
        // Was killed by stop_run
        -1
    };

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

    // Emit RunState via bus
    if let Some(ref em) = emitter {
        // For resumable Codex runs (has conversation_ref), normal exit → "idle"
        // instead of "completed", so the UI shows the same state as Claude between turns.
        let has_conversation_ref = is_codex
            && storage::runs::get_run(&run_id)
                .map(|r| r.conversation_ref.is_some())
                .unwrap_or(false);
        let state = match exit_code {
            0 if has_conversation_ref => "idle",
            0 => "completed",
            -1 => "stopped",
            _ => "failed",
        };
        em.persist_and_emit(
            &run_id,
            &BusEvent::RunState {
                run_id: run_id.clone(),
                state: state.to_string(),
                exit_code: Some(exit_code),
                error: if exit_code != 0 && exit_code != -1 {
                    Some(format!("Exit code {}", exit_code))
                } else {
                    None
                },
            },
        );
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
    // Short lock: remove child, then kill+wait outside the lock.
    let removed = {
        let mut map = process_map.lock().await;
        map.remove(run_id)
    };
    if let Some(mut child) = removed {
        let _ = child.kill().await;
        let _ = child.wait().await;
        log::debug!("[stream] stop_process: killed run_id={}", run_id);
        true
    } else {
        log::debug!("[stream] stop_process: no process for run_id={}", run_id);
        false
    }
}

use crate::models::{PtyExit, PtyOutput, RunEventType};
use crate::storage;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn portable_pty::Child + Send>,
}

pub type PtyMap = Arc<std::sync::Mutex<HashMap<String, PtySession>>>;

pub fn new_pty_map() -> PtyMap {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_pty_session(
    app: AppHandle,
    pty_map: &PtyMap,
    run_id: &str,
    cmd: &str,
    args: &[String],
    cwd: &str,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    log::debug!(
        "[pty] spawn_pty_session: run_id={}, cmd={}, args={:?}, cwd={}, rows={}, cols={}",
        run_id,
        cmd,
        args,
        cwd,
        rows,
        cols
    );
    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open PTY: {}", e))?;

    let mut cmd_builder = CommandBuilder::new(cmd);
    cmd_builder.args(args);
    cmd_builder.cwd(cwd);
    cmd_builder.env("OPENCOVIBE_TASK_ID", run_id);
    cmd_builder.env("OPENCOVIBE_RUN_ID", run_id);
    cmd_builder.env("TERM", "xterm-256color");
    cmd_builder.env("CLAUDECODE", ""); // Clear to allow running inside a Claude Code session

    let child = pair
        .slave
        .spawn_command(cmd_builder)
        .map_err(|e| format!("Failed to spawn process in PTY: {}", e))?;

    log::debug!("[pty] spawned process in PTY: run_id={}", run_id);
    // Drop slave — we only need the master side
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get PTY writer: {}", e))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get PTY reader: {}", e))?;

    // Store session
    {
        let mut map = pty_map.lock().map_err(|e| e.to_string())?;
        map.insert(
            run_id.to_string(),
            PtySession {
                writer,
                master: pair.master,
                child,
            },
        );
    }

    // Log start
    if let Err(e) = storage::events::append_event(
        run_id,
        RunEventType::System,
        serde_json::json!({
            "message": format!("Started PTY session: {} {}", cmd, args.join(" ")),
            "source": "ui_chat"
        }),
    ) {
        log::warn!("[pty] failed to log PTY start event: {}", e);
    }

    log::debug!("[pty] starting read loop: run_id={}", run_id);
    // Reader thread: blocking read → mpsc → async emit
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    let run_id_reader = run_id.to_string();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log::debug!("[pty] read loop EOF: run_id={}", run_id_reader);
                    break;
                }
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    if tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::debug!("[pty] read loop error: run_id={}, err={}", run_id_reader, e);
                    break;
                }
            }
            // Tiny yield to prevent event flooding
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    // Async task: receive chunks, base64 encode, emit events, persist text
    let run_id_async = run_id.to_string();
    let pty_map_clone = pty_map.clone();
    tokio::spawn(async move {
        use base64::Engine;
        let mut text_buffer = String::new();
        let mut last_flush = std::time::Instant::now();

        while let Some(chunk) = rx.recv().await {
            // Emit raw PTY output as base64
            let b64 = base64::engine::general_purpose::STANDARD.encode(&chunk);
            let _ = app.emit(
                "pty-output",
                PtyOutput {
                    run_id: run_id_async.clone(),
                    data: b64,
                },
            );

            // Accumulate stripped text for persistence
            let stripped = strip_ansi_escapes::strip(&chunk);
            let text = String::from_utf8_lossy(&stripped);
            text_buffer.push_str(&text);

            // Flush text to events.jsonl every 2 seconds or on newlines
            let should_flush = last_flush.elapsed() >= std::time::Duration::from_secs(2)
                || text_buffer.contains('\n');

            if should_flush && !text_buffer.is_empty() {
                if let Err(e) = storage::events::append_event(
                    &run_id_async,
                    RunEventType::Assistant,
                    serde_json::json!({
                        "text": text_buffer.trim_end(),
                        "source": "pty"
                    }),
                ) {
                    log::warn!("[pty] failed to flush text buffer: {}", e);
                }
                text_buffer.clear();
                last_flush = std::time::Instant::now();
            }
        }

        // Flush remaining text
        if !text_buffer.trim().is_empty() {
            if let Err(e) = storage::events::append_event(
                &run_id_async,
                RunEventType::Assistant,
                serde_json::json!({
                    "text": text_buffer.trim_end(),
                    "source": "pty"
                }),
            ) {
                log::warn!("[pty] failed to flush final text buffer: {}", e);
            }
        }

        // Process has exited — get exit code
        let exit_code = {
            let mut map = pty_map_clone.lock().unwrap();
            if let Some(session) = map.get_mut(&run_id_async) {
                match session.child.wait() {
                    Ok(status) => {
                        if status.success() {
                            0
                        } else {
                            status.exit_code().try_into().unwrap_or(1)
                        }
                    }
                    Err(_) => 1,
                }
            } else {
                -1
            }
        };

        // Update run status
        if exit_code == 0 {
            if let Err(e) = storage::runs::update_status(
                &run_id_async,
                crate::models::RunStatus::Completed,
                Some(exit_code),
                None,
            ) {
                log::warn!("[pty] failed to update status to Completed: {}", e);
            }
        } else if exit_code == -1 {
            if let Err(e) = storage::runs::update_status(
                &run_id_async,
                crate::models::RunStatus::Stopped,
                None,
                Some("Stopped by user".to_string()),
            ) {
                log::warn!("[pty] failed to update status to Stopped: {}", e);
            }
        } else if let Err(e) = storage::runs::update_status(
            &run_id_async,
            crate::models::RunStatus::Failed,
            Some(exit_code),
            Some(format!("Exit code {}", exit_code)),
        ) {
            log::warn!("[pty] failed to update status to Failed: {}", e);
        }

        if let Err(e) = storage::events::append_event(
            &run_id_async,
            RunEventType::System,
            serde_json::json!({
                "message": format!("PTY process exited with code {}", exit_code),
                "source": "pty"
            }),
        ) {
            log::warn!("[pty] failed to log exit event: {}", e);
        }

        log::debug!(
            "[pty] process exited: run_id={}, exit_code={}",
            run_id_async,
            exit_code
        );

        let _ = app.emit(
            "pty-exit",
            PtyExit {
                run_id: run_id_async.clone(),
                exit_code,
            },
        );

        // Remove from map
        let _ = pty_map_clone.lock().unwrap().remove(&run_id_async);
    });

    Ok(())
}

pub fn write_to_pty(pty_map: &PtyMap, run_id: &str, data: &[u8]) -> Result<(), String> {
    log::trace!(
        "[pty] write_to_pty: run_id={}, data_len={}",
        run_id,
        data.len()
    );
    let mut map = pty_map.lock().map_err(|e| e.to_string())?;
    let session = map
        .get_mut(run_id)
        .ok_or_else(|| format!("No PTY session for run {}", run_id))?;
    session
        .writer
        .write_all(data)
        .map_err(|e| format!("Failed to write to PTY: {}", e))?;
    session
        .writer
        .flush()
        .map_err(|e| format!("Failed to flush PTY: {}", e))?;
    Ok(())
}

pub fn resize_pty(pty_map: &PtyMap, run_id: &str, rows: u16, cols: u16) -> Result<(), String> {
    log::debug!(
        "[pty] resize_pty: run_id={}, rows={}, cols={}",
        run_id,
        rows,
        cols
    );
    let map = pty_map.lock().map_err(|e| e.to_string())?;
    let session = map
        .get(run_id)
        .ok_or_else(|| format!("No PTY session for run {}", run_id))?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to resize PTY: {}", e))?;
    Ok(())
}

pub fn close_pty(pty_map: &PtyMap, run_id: &str) -> Result<bool, String> {
    log::debug!("[pty] close_pty: run_id={}", run_id);
    let mut map = pty_map.lock().map_err(|e| e.to_string())?;
    if let Some(mut session) = map.remove(run_id) {
        let _ = session.child.kill();
        log::debug!("[pty] close_pty: killed process for run_id={}", run_id);
        Ok(true)
    } else {
        log::debug!("[pty] close_pty: no session found for run_id={}", run_id);
        Ok(false)
    }
}

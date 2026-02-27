use crate::models::{now_iso, BusEvent, ModelUsageSummary, RawRunUsage, RunEvent, RunEventType};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

/// Event types the frontend reducer actually handles during replay.
/// "raw" events (CLI stream data) are 90%+ of the file but the frontend drops them,
/// so filtering here avoids serializing megabytes of unused data across IPC.
pub const REPLAY_TYPES: &[&str] = &[
    "session_init",
    "message_delta",
    "thinking_delta",
    "tool_input_delta",
    "message_complete",
    "user_message",
    "tool_start",
    "tool_end",
    "run_state",
    "usage_update",
    "permission_denied",
    "permission_prompt",
    "compact_boundary",
    "system_status",
    "auth_status",
    "hook_started",
    "hook_response",
    "control_cancelled",
    "task_notification",
    "tool_progress",
    "tool_use_summary",
    "command_output",
    "files_persisted",
    "hook_progress",
    "hook_callback",
];

/// Check if a BusEvent's serde tag is in REPLAY_TYPES.
pub fn is_replayable(event: &BusEvent) -> bool {
    let Ok(v) = serde_json::to_value(event) else {
        return false;
    };
    let Some(tag) = v.get("type").and_then(|t| t.as_str()) else {
        return false;
    };
    REPLAY_TYPES.contains(&tag)
}

fn events_path(run_id: &str) -> std::path::PathBuf {
    super::run_dir(run_id).join("events.jsonl")
}

pub fn next_seq(run_id: &str) -> u64 {
    let path = events_path(run_id);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return 1,
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file_len == 0 {
        return 1;
    }

    let mut reader = BufReader::new(file);
    if file_len > 4096 {
        let _ = reader.seek(SeekFrom::End(-4096));
    }

    // Use read_to_end + from_utf8_lossy to handle potential mid-character seek
    let mut buf = Vec::new();
    if reader.read_to_end(&mut buf).is_err() {
        return 1;
    }
    let tail = String::from_utf8_lossy(&buf);

    // Skip first (potentially partial) line if we seeked into the middle
    let lines_str = if file_len > 4096 {
        tail.split_once('\n').map(|(_, rest)| rest).unwrap_or(&tail)
    } else {
        &tail
    };

    let max_seq = lines_str
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("seq").and_then(|s| s.as_u64()))
        .max()
        .unwrap_or(0);
    max_seq + 1
}

pub fn append_event(
    run_id: &str,
    event_type: RunEventType,
    payload: serde_json::Value,
) -> Result<RunEvent, String> {
    log::trace!(
        "[storage/events] append_event: run_id={}, type={:?}",
        run_id,
        event_type
    );
    let dir = super::run_dir(run_id);
    super::ensure_dir(&dir).map_err(|e| e.to_string())?;

    let event = RunEvent {
        id: uuid::Uuid::new_v4().to_string()[..12].to_string(),
        task_id: run_id.to_string(),
        seq: next_seq(run_id),
        event_type,
        payload,
        timestamp: now_iso(),
    };

    let path = events_path(run_id);
    let line = serde_json::to_string(&event).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())?;

    Ok(event)
}

pub fn list_events(run_id: &str, since_seq: u64) -> Vec<RunEvent> {
    let path = events_path(run_id);
    if !path.exists() {
        return vec![];
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RunEvent>(l).ok())
        .filter(|e| e.seq > since_seq)
        .collect()
}

// ── Bus event persistence ──

use std::sync::{Arc, Mutex};

/// Atomic seq allocation + file write under per-run locks.
/// Each run_id gets its own Mutex so different runs never block each other.
/// The outer Mutex is only held briefly to get/create the per-run Arc.
pub struct EventWriter {
    inner: Mutex<HashMap<String, Arc<Mutex<u64>>>>, // run_id → Arc<Mutex<next_seq>>
}

impl Default for EventWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl EventWriter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Atomically assign seq + write to events.jsonl (both under the same per-run lock).
    /// Returns `Err` if any step fails (dir creation, serialization, file I/O).
    pub fn write_bus_event(&self, run_id: &str, event: &BusEvent) -> Result<(), String> {
        log::trace!("[storage/events] write_bus_event: run_id={}", run_id);

        // Get or create the per-run lock (brief global lock, then release)
        let run_lock = {
            let mut map = self.inner.lock().unwrap();
            // GC: drop entries whose per-run Arc has no other holders (session ended)
            if map.len() > 50 {
                map.retain(|_, v| Arc::strong_count(v) > 1);
            }
            map.entry(run_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(next_seq(run_id))))
                .clone()
        };
        // Global lock released here — other runs proceed in parallel

        // Per-run lock: seq allocation + file write are atomic
        let mut seq_guard = run_lock.lock().unwrap();
        let current = *seq_guard;
        *seq_guard = current + 1;

        let dir = super::run_dir(run_id);
        super::ensure_dir(&dir).map_err(|e| format!("ensure_dir failed: {}", e))?;

        let envelope = serde_json::json!({
            "_bus": true,
            "seq": current,
            "ts": now_iso(),
            "event": event,
        });
        let path = events_path(run_id);
        let line =
            serde_json::to_string(&envelope).map_err(|e| format!("serialize failed: {}", e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open {} failed: {}", path.display(), e))?;
        writeln!(file, "{}", line)
            .map_err(|e| format!("write to {} failed: {}", path.display(), e))?;

        Ok(())
    }

    /// Like `write_bus_event` but uses a caller-supplied timestamp and returns the assigned seq.
    pub fn write_bus_event_with_ts(
        &self,
        run_id: &str,
        event: &BusEvent,
        ts: &str,
    ) -> Result<u64, String> {
        log::trace!(
            "[storage/events] write_bus_event_with_ts: run_id={}, ts={}",
            run_id,
            ts
        );

        let run_lock = {
            let mut map = self.inner.lock().unwrap();
            if map.len() > 50 {
                map.retain(|_, v| Arc::strong_count(v) > 1);
            }
            map.entry(run_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(next_seq(run_id))))
                .clone()
        };

        let mut seq_guard = run_lock.lock().unwrap();
        let current = *seq_guard;
        *seq_guard = current + 1;

        let dir = super::run_dir(run_id);
        super::ensure_dir(&dir).map_err(|e| format!("ensure_dir failed: {}", e))?;

        let envelope = serde_json::json!({
            "_bus": true,
            "seq": current,
            "ts": ts,
            "event": event,
        });
        let path = events_path(run_id);
        let line =
            serde_json::to_string(&envelope).map_err(|e| format!("serialize failed: {}", e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open {} failed: {}", path.display(), e))?;
        writeln!(file, "{}", line)
            .map_err(|e| format!("write to {} failed: {}", path.display(), e))?;

        Ok(current)
    }
}

/// Thin wrapper for backward compatibility — delegates to EventWriter.
/// Returns `Err` if persistence failed.
pub fn persist_bus_event(
    writer: &EventWriter,
    run_id: &str,
    event: &BusEvent,
) -> Result<(), String> {
    writer.write_bus_event(run_id, event)
}

/// Copy content bus events from one run's events.jsonl to another.
/// Used by fork to preserve conversation history in the new run.
/// Lifecycle events (session_init, run_state, usage_update, permission_denied, raw)
/// are excluded — they belong to the parent session, not the fork.
/// Copied events get their `run_id` rewritten to `to_run_id` and `seq` renumbered
/// from 1 so the fork run's events.jsonl is fully self-consistent.
pub fn copy_bus_events(from_run_id: &str, to_run_id: &str) -> Result<(), String> {
    let src = events_path(from_run_id);
    if !src.exists() {
        log::debug!(
            "[storage/events] copy_bus_events: source {} has no events",
            from_run_id
        );
        return Ok(());
    }
    let dst_dir = super::run_dir(to_run_id);
    super::ensure_dir(&dst_dir).map_err(|e| format!("ensure_dir failed: {}", e))?;
    let dst = events_path(to_run_id);

    let content =
        fs::read_to_string(&src).map_err(|e| format!("read source events failed: {}", e))?;

    // Content event types to copy (conversation history).
    const CONTENT_TYPES: &[&str] = &[
        "message_delta",
        "message_complete",
        "tool_start",
        "tool_end",
        "user_message",
    ];

    let mut out = String::new();
    let mut copied = 0u64;
    let mut skipped = 0u64;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut envelope) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        // Only process bus events
        if envelope.get("_bus").and_then(|b| b.as_bool()) != Some(true) {
            continue;
        }

        // Check inner event type
        let event_type = envelope
            .get("event")
            .and_then(|e| e.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if CONTENT_TYPES.contains(&event_type.as_str()) {
            // Rewrite run_id in inner event to the fork run
            if let Some(event) = envelope.get_mut("event").and_then(|e| e.as_object_mut()) {
                event.insert(
                    "run_id".to_string(),
                    serde_json::Value::String(to_run_id.to_string()),
                );
            }
            // Renumber seq sequentially
            copied += 1;
            envelope["seq"] = serde_json::Value::Number(copied.into());

            let serialized =
                serde_json::to_string(&envelope).map_err(|e| format!("serialize failed: {}", e))?;
            out.push_str(&serialized);
            out.push('\n');
        } else {
            skipped += 1;
        }
    }

    fs::write(&dst, &out).map_err(|e| format!("write fork events failed: {}", e))?;
    log::debug!(
        "[storage/events] copy_bus_events: {} → {} (copied {} content events, skipped {} lifecycle, new max_seq={})",
        from_run_id, to_run_id, copied, skipped, copied
    );
    Ok(())
}

/// Extract usage summary from a run's events.jsonl by scanning for usage_update events.
/// Uses "simpler v1" approach: peak-detection for cost (handles session restarts),
/// last usage_update for tokens and model_usage, sum for duration_ms.
pub fn extract_run_usage(run_id: &str) -> Option<RawRunUsage> {
    let path = events_path(run_id);
    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path).ok()?;

    let mut total_cost: f64 = 0.0;
    let mut prev_cost: f64 = 0.0;
    let mut peak_cost: f64 = 0.0;
    let mut total_duration_ms: u64 = 0;
    let mut found_any = false;

    // "Simpler v1": take values from the last usage_update event
    let mut last_input: u64 = 0;
    let mut last_output: u64 = 0;
    let mut last_cache_read: u64 = 0;
    let mut last_cache_write: u64 = 0;
    let mut last_num_turns: u64 = 0;
    let mut last_model_usage: HashMap<String, ModelUsageSummary> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Cheap pre-filter: skip ~99.6% of lines without JSON parsing
        if !line.contains("\"usage_update\"") {
            continue;
        }

        let Ok(envelope) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if envelope.get("_bus").and_then(|b| b.as_bool()) != Some(true) {
            continue;
        }
        let Some(event) = envelope.get("event") else {
            continue;
        };
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if event_type != "usage_update" {
            continue;
        }

        found_any = true;
        let cost = event
            .get("total_cost_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        // Detect session restart: cost decreased → new session segment
        if cost < prev_cost * 0.9 && prev_cost > 0.0 {
            total_cost += peak_cost;
            peak_cost = 0.0;
        }
        if cost > peak_cost {
            peak_cost = cost;
        }
        prev_cost = cost;

        // Overwrite with latest values (cumulative within session)
        last_input = event
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(last_input);
        last_output = event
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(last_output);
        last_cache_read = event
            .get("cache_read_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(last_cache_read);
        last_cache_write = event
            .get("cache_write_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(last_cache_write);
        last_num_turns = event
            .get("num_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(last_num_turns);

        // Sum duration_ms across turns (per-turn value, not cumulative)
        if let Some(d) = event.get("duration_ms").and_then(|v| v.as_u64()) {
            total_duration_ms += d;
        }

        // Take last model_usage map
        if let Some(mu) = event.get("model_usage").and_then(|v| v.as_object()) {
            last_model_usage.clear();
            for (model, entry) in mu {
                last_model_usage.insert(
                    model.clone(),
                    ModelUsageSummary {
                        input_tokens: entry
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        output_tokens: entry
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_read_tokens: entry
                            .get("cache_read_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_write_tokens: entry
                            .get("cache_write_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cost_usd: entry
                            .get("cost_usd")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                    },
                );
            }
        }
    }

    if !found_any {
        return None;
    }

    // Add final segment's peak cost
    total_cost += peak_cost;

    log::debug!(
        "[storage/events] extract_run_usage: run_id={}, cost={:.6}, tokens={}+{}, turns={}, models={}",
        run_id,
        total_cost,
        last_input,
        last_output,
        last_num_turns,
        last_model_usage.len()
    );

    Some(RawRunUsage {
        total_cost_usd: total_cost,
        input_tokens: last_input,
        output_tokens: last_output,
        cache_read_tokens: last_cache_read,
        cache_write_tokens: last_cache_write,
        duration_ms: total_duration_ms,
        num_turns: last_num_turns,
        model_usage: last_model_usage,
    })
}

/// Count user_message events in events.jsonl for resume baseline.
/// Returns (total_user_messages, normal_user_messages).
///
/// Compat: handles both wrapped `{"event": {"type": "user_message", ...}, ...}`
/// and direct `{"type": "user_message", ...}` JSONL formats.
/// Unparseable lines are skipped (debug-level count logged).
pub fn count_user_messages(run_id: &str) -> (u32, u32) {
    let path = events_path(run_id);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };

    let mut total: u32 = 0;
    let mut normal: u32 = 0;
    let mut skipped: u32 = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Fast pre-filter: skip lines that can't contain user_message
        if !line.contains("\"user_message\"") {
            continue;
        }
        let parsed = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        // Compat: wrapped format takes .event, direct format takes self
        let event = parsed.get("event").unwrap_or(&parsed);
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type == "user_message" {
            total += 1;
            let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if !text.trim_start().starts_with('/') {
                normal += 1;
            }
        }
    }

    if skipped > 0 {
        log::debug!(
            "[events] count_user_messages: skipped {} unparseable lines",
            skipped
        );
    }

    (total, normal)
}

pub fn list_bus_events(run_id: &str, since_seq: Option<u64>) -> Vec<serde_json::Value> {
    log::debug!(
        "[storage/events] list_bus_events: run_id={}, since_seq={:?}",
        run_id,
        since_seq
    );
    let path = events_path(run_id);
    if !path.exists() {
        return vec![];
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let min_seq = since_seq.unwrap_or(0);

    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            // Only process bus events
            if v.get("_bus")?.as_bool()? {
                let seq = v.get("seq")?.as_u64()?;
                if seq > min_seq {
                    let event = v.get("event")?;
                    // Skip event types the frontend doesn't use (raw, stream_event, etc.)
                    let etype = event.get("type")?.as_str()?;
                    if !REPLAY_TYPES.contains(&etype) {
                        return None;
                    }
                    let mut event = event.clone();
                    // Inject envelope timestamp into event so frontend can display it
                    if let (Some(ts), Some(obj)) = (v.get("ts"), event.as_object_mut()) {
                        obj.insert("ts".to_string(), ts.clone());
                    }
                    return Some(event);
                }
            }
            None
        })
        .collect()
}

use crate::models::{now_iso, BusEvent, ModelUsageSummary, RawRunUsage, RunEvent, RunEventType};
use once_cell::sync::Lazy;
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
    "elicitation_prompt",
    "rate_limit_event",
    "codex_hook_run",
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
    let file_len = match fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(_) => return 1,
    };
    if file_len == 0 {
        return 1;
    }

    // Fast path: scan only the last 4 KiB — recent (highest) seqs are at the end.
    if let Some(max) = max_seq_in_tail(&path, file_len) {
        return max + 1;
    }

    // Fallback: the tail window held no parseable seq line — e.g. the last event
    // line is itself larger than 4 KiB, so after dropping the partial first line
    // nothing parses. Seeding 1 here would collide with existing seqs, so do a
    // full scan to seed correctly. (audit #7: oversized-line seed reset)
    if let Ok(content) = fs::read_to_string(&path) {
        if let Some(max) = scan_max_seq(&content) {
            return max + 1;
        }
    }
    1
}

/// Max `seq` over a JSONL string's parseable lines (None if none parse).
fn scan_max_seq(content: &str) -> Option<u64> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("seq").and_then(|s| s.as_u64()))
        .max()
}

/// Max `seq` from the last 4 KiB of `path`. Returns None when the window contains
/// no complete line (too small to hold the final event), signalling the caller to
/// fall back to a full scan instead of trusting a bogus 0 seed.
fn max_seq_in_tail(path: &std::path::Path, file_len: u64) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    if file_len > 4096 {
        reader.seek(SeekFrom::End(-4096)).ok()?;
    }
    // read_to_end + from_utf8_lossy tolerates a mid-character seek.
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).ok()?;
    let tail = String::from_utf8_lossy(&buf);
    // Drop the first (partial) line when we seeked into the middle. If there is no
    // newline at all, the whole window is one partial line → "" → None (full scan).
    let lines_str = if file_len > 4096 {
        tail.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &tail
    };
    scan_max_seq(lines_str)
}

/// Append a raw run-event (stdout/stderr/etc.) to events.jsonl.
///
/// Delegates to the process-wide [`EventWriter`] singleton so that seq allocation
/// and the file write happen under the SAME per-run lock as bus events. Previously
/// this computed seq via an unlocked file read, so concurrent writers (e.g. Codex
/// stdout + stderr tasks, or a bus-event write interleaving) could collide on seq
/// or interleave partial lines. (audit #1: append_event seq race)
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
    EVENT_WRITER.write_run_event(run_id, event_type, payload)
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

    /// Atomically assign seq + append a raw [`RunEvent`] (stdout/stderr/etc.) under
    /// the same per-run lock and seq counter as bus events, so the two write paths
    /// can't collide on seq or interleave partial lines into events.jsonl.
    pub fn write_run_event(
        &self,
        run_id: &str,
        event_type: RunEventType,
        payload: serde_json::Value,
    ) -> Result<RunEvent, String> {
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
        super::ensure_dir(&dir).map_err(|e| e.to_string())?;

        let event = RunEvent {
            id: uuid::Uuid::new_v4().to_string()[..12].to_string(),
            task_id: run_id.to_string(),
            seq: current,
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
}

/// Process-wide singleton EventWriter. Both bus events and raw run-events (via
/// `append_event`) write through this instance so all writes to a given run's
/// events.jsonl share one per-run lock + one monotonic seq source.
static EVENT_WRITER: Lazy<Arc<EventWriter>> = Lazy::new(|| Arc::new(EventWriter::new()));

/// Returns the process-wide [`EventWriter`] singleton. Register this as the Tauri
/// managed state so command handlers and `append_event` share the same locks/seq.
pub fn global_writer() -> Arc<EventWriter> {
    EVENT_WRITER.clone()
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

/// Extract aggregated usage from bus-events for a single run.
///
/// Three modes:
/// - CLI imports (source=cli_import): per-turn cost+tokens, sum all
/// - Codex (agent=codex): per-turn tokens, sum all; cost estimated in stats.rs
/// - Claude native sessions: cumulative cost (peak-detect), cumulative tokens (take-last)
pub fn extract_run_usage(run_id: &str) -> Option<RawRunUsage> {
    let path = events_path(run_id);
    if !path.exists() {
        return None;
    }

    // Run-scoped detection: parse meta.json once for source + agent
    let (is_per_turn_cost, is_codex) = {
        let meta_path = super::run_dir(run_id).join("meta.json");
        let meta_val = meta_path
            .exists()
            .then(|| {
                fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            })
            .flatten();
        let source = meta_val
            .as_ref()
            .and_then(|v| v.get("source").and_then(|s| s.as_str()).map(String::from));
        let agent = meta_val
            .as_ref()
            .and_then(|v| v.get("agent").and_then(|s| s.as_str()).map(String::from));
        (
            source == Some("cli_import".to_string()),
            agent == Some("codex".to_string()),
        )
    };
    // Codex turn.completed.usage is per-turn (same as CLI imports)
    let sum_usage = is_per_turn_cost || is_codex;

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

        if sum_usage {
            // CLI imports + Codex: per-turn cost, sum directly
            total_cost += cost;
        } else {
            // Native Claude session: cumulative cost, peak-detect
            if cost < prev_cost * 0.9 && prev_cost > 0.0 {
                total_cost += peak_cost;
                peak_cost = 0.0;
            }
            if cost > peak_cost {
                peak_cost = cost;
            }
            prev_cost = cost;
        }

        // Tokens: for per-turn (CLI imports + Codex), sum; for cumulative, take last
        if sum_usage {
            last_input += event
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            last_output += event
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            last_cache_read += event
                .get("cache_read_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            last_cache_write += event
                .get("cache_write_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        } else {
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
        }

        // num_turns: Claude sends num_turns, Codex sends turn_index (1-based)
        let event_num_turns = event.get("num_turns").and_then(|v| v.as_u64());
        let event_turn_index = event.get("turn_index").and_then(|v| v.as_u64());
        if let Some(nt) = event_num_turns {
            last_num_turns = nt;
        } else if let Some(ti) = event_turn_index {
            // Codex: turn_index is 1-based counter, use as num_turns
            if ti > last_num_turns {
                last_num_turns = ti;
            }
        }

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

    // Add final segment's peak cost (only for cumulative mode)
    if !sum_usage {
        total_cost += peak_cost;
    }

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
                    if let Some(obj) = event.as_object_mut() {
                        // Inject envelope timestamp into event so frontend can display it
                        if let Some(ts) = v.get("ts") {
                            obj.insert("ts".to_string(), ts.clone());
                        }
                        // Inject _seq so frontend can track checkpoint for WS subscribe
                        obj.insert("_seq".to_string(), serde_json::Value::Number(seq.into()));
                    }
                    return Some(event);
                }
            }
            None
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{max_seq_in_tail, scan_max_seq};
    use std::io::Write as _;

    #[test]
    fn scan_max_seq_picks_highest_and_ignores_junk() {
        assert_eq!(
            scan_max_seq("{\"seq\":1}\n{\"seq\":5}\n{\"seq\":3}\n"),
            Some(5)
        );
        assert_eq!(scan_max_seq(""), None);
        assert_eq!(scan_max_seq("not json\n\n"), None);
    }

    #[test]
    fn max_seq_in_tail_small_file_reads_directly() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "{}", serde_json::json!({"seq": 7})).unwrap();
        f.flush().unwrap();
        let len = f.as_file().metadata().unwrap().len();
        assert_eq!(max_seq_in_tail(f.path(), len), Some(7));
    }

    #[test]
    fn max_seq_in_tail_returns_none_when_last_line_exceeds_window() {
        // audit #7: a final event line larger than the 4 KiB tail window leaves no
        // newline in the window, so the tail scan must report None (not a bogus 0)
        // to let next_seq fall back to a full scan instead of reseeding seq to 1.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "{}", serde_json::json!({"seq": 1})).unwrap();
        let big = "x".repeat(8192);
        writeln!(f, "{}", serde_json::json!({"seq": 2, "blob": big})).unwrap();
        f.flush().unwrap();
        let len = f.as_file().metadata().unwrap().len();
        assert!(len > 4096);
        assert_eq!(max_seq_in_tail(f.path(), len), None);
        // The full-scan fallback path still recovers the true max.
        let content = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(scan_max_seq(&content), Some(2));
    }
}

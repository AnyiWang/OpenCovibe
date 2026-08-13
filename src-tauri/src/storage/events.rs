use crate::models::{now_iso, BusEvent, ModelUsageSummary, RawRunUsage, RunEvent, RunEventType};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};

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
    "interaction_response_started",
    "interaction_response_failed",
    "interaction_resolved",
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
    // Replayed so a loaded run (e.g. the /agents Active scope) can label Codex sub-agent nodes
    // with their resolved nickname/role instead of a raw thread id.
    "codex_agent_info",
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

const BUS_EVENT_PAGE_ENTRY_LIMIT: usize = 100;
const BUS_EVENT_PAGE_BYTE_LIMIT: usize = 1024 * 1024;
const BUS_EVENT_LINE_LIMIT: usize = 2 * 1024 * 1024;
pub const HISTORY_PROJECTION_REQUIRED: &str = "HISTORY_PROJECTION_REQUIRED";

fn history_projection_required(detail: impl std::fmt::Display) -> String {
    format!("{HISTORY_PROJECTION_REQUIRED}: {detail}")
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusEventPage {
    pub events: Vec<serde_json::Value>,
    pub last_seq: u64,
    pub has_more: bool,
    pub next_offset: u64,
}

/// Atomic seq allocation + file write under per-run locks.
/// Each run_id gets its own Mutex so different runs never block each other.
/// The outer Mutex is only held briefly to get/create the per-run Arc.
pub struct EventWriter {
    inner: Mutex<HashMap<String, Arc<Mutex<RunWriterState>>>>,
}

struct RunWriterState {
    next_seq: u64,
    tail_repaired: bool,
}

pub(crate) struct EventLogSnapshot {
    pub file: fs::File,
    pub metadata: fs::Metadata,
}

fn repair_incomplete_tail(path: &std::path::Path) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        super::ensure_dir(parent).map_err(|e| {
            format!(
                "create event log directory {} failed: {e}",
                parent.display()
            )
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("open {} failed: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Ok(0);
    }
    file.seek(SeekFrom::End(-1)).map_err(|e| e.to_string())?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last).map_err(|e| e.to_string())?;
    if last[0] == b'\n' {
        return Ok(len);
    }

    let mut cursor = len;
    let mut buffer = vec![0u8; 64 * 1024];
    let line_start = loop {
        let start = cursor.saturating_sub(buffer.len() as u64);
        let count = (cursor - start) as usize;
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut buffer[..count])
            .map_err(|e| e.to_string())?;
        if let Some(index) = buffer[..count].iter().rposition(|byte| *byte == b'\n') {
            break start + index as u64 + 1;
        }
        if start == 0 {
            break 0;
        }
        cursor = start;
    };
    file.seek(SeekFrom::Start(line_start))
        .map_err(|e| e.to_string())?;
    let mut deserializer =
        serde_json::Deserializer::from_reader(BufReader::new((&mut file).take(len - line_start)));
    let complete_json =
        <serde::de::IgnoredAny as serde::Deserialize>::deserialize(&mut deserializer)
            .and_then(|_| deserializer.end())
            .is_ok();
    if complete_json {
        file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| {
            format!(
                "commit final event newline in {} failed: {e}",
                path.display()
            )
        })?;
        log::debug!(
            "[storage/events] normalized complete final event: path={}",
            path.display()
        );
        return Ok(len + 1);
    }

    file.set_len(line_start).map_err(|e| {
        format!(
            "truncate incomplete event tail in {} failed: {e}",
            path.display()
        )
    })?;
    log::warn!(
        "[storage/events] repaired incomplete tail: path={}, removed_bytes={}",
        path.display(),
        len - line_start
    );
    Ok(line_start)
}

fn capture_event_log_snapshot(path: &std::path::Path) -> Result<EventLogSnapshot, String> {
    repair_incomplete_tail(path)?;
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("open {} failed: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    Ok(EventLogSnapshot { file, metadata })
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

    fn run_lock(&self, run_id: &str) -> Arc<Mutex<RunWriterState>> {
        let mut map = self.inner.lock().unwrap();
        if map.len() > 50 {
            map.retain(|_, value| Arc::strong_count(value) > 1);
        }
        map.entry(run_id.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(RunWriterState {
                    next_seq: next_seq(run_id),
                    tail_repaired: false,
                }))
            })
            .clone()
    }

    fn ensure_tail_repaired(run_id: &str, state: &mut RunWriterState) -> Result<(), String> {
        if state.tail_repaired {
            return Ok(());
        }
        repair_incomplete_tail(&events_path(run_id))?;
        // Repair can remove the only occurrence of the highest sequence when it was part of the
        // interrupted record, so seed again from the committed file before allocating a value.
        state.next_seq = next_seq(run_id);
        state.tail_repaired = true;
        Ok(())
    }

    /// Capture a fixed prefix that ends after a complete JSONL record. Appends may resume as soon
    /// as this method returns; the cloned file handle and length keep the history scan bounded.
    pub(crate) fn snapshot(&self, run_id: &str) -> Result<EventLogSnapshot, String> {
        let run_lock = self.run_lock(run_id);
        let mut state = run_lock.lock().unwrap();
        let dir = super::run_dir(run_id);
        super::ensure_dir(&dir).map_err(|e| format!("ensure_dir failed: {e}"))?;
        let path = events_path(run_id);
        Self::ensure_tail_repaired(run_id, &mut state)?;
        capture_event_log_snapshot(&path)
    }

    /// Atomically assign seq + write to events.jsonl (both under the same per-run lock).
    /// Returns `Err` if any step fails (dir creation, serialization, file I/O).
    pub fn write_bus_event(&self, run_id: &str, event: &BusEvent) -> Result<(), String> {
        log::trace!("[storage/events] write_bus_event: run_id={}", run_id);

        // Get or create the per-run lock (brief global lock, then release)
        let run_lock = self.run_lock(run_id);
        // Global lock released here — other runs proceed in parallel

        // Per-run lock: seq allocation + file write are atomic
        let mut state = run_lock.lock().unwrap();
        Self::ensure_tail_repaired(run_id, &mut state)?;
        let current = state.next_seq;

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
        if let Err(error) = writeln!(file, "{}", line) {
            state.tail_repaired = false;
            return Err(format!("write to {} failed: {}", path.display(), error));
        }
        state.next_seq = current + 1;

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

        let run_lock = self.run_lock(run_id);

        let mut state = run_lock.lock().unwrap();
        Self::ensure_tail_repaired(run_id, &mut state)?;
        let current = state.next_seq;

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
        if let Err(error) = writeln!(file, "{}", line) {
            state.tail_repaired = false;
            return Err(format!("write to {} failed: {}", path.display(), error));
        }
        state.next_seq = current + 1;

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
        let run_lock = self.run_lock(run_id);

        let mut state = run_lock.lock().unwrap();
        Self::ensure_tail_repaired(run_id, &mut state)?;
        let current = state.next_seq;

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
        if let Err(error) = writeln!(file, "{}", line) {
            state.tail_repaired = false;
            return Err(error.to_string());
        }
        state.next_seq = current + 1;

        Ok(event)
    }

    /// Copy a fork's replayable conversation from a committed source prefix while holding the
    /// target run lock for publication. The source actor may continue appending after snapshot.
    pub fn copy_bus_events(&self, from_run_id: &str, to_run_id: &str) -> Result<(), String> {
        let snapshot = self.snapshot(from_run_id)?;
        let source = BufReader::new(snapshot.file.take(snapshot.metadata.len()));

        let target_lock = self.run_lock(to_run_id);
        let mut target_state = target_lock.lock().unwrap();
        target_state.tail_repaired = false;
        let dst_dir = super::run_dir(to_run_id);
        super::ensure_dir(&dst_dir).map_err(|e| format!("ensure_dir failed: {e}"))?;
        let dst = events_path(to_run_id);
        let target =
            fs::File::create(&dst).map_err(|e| format!("create fork events failed: {e}"))?;
        let (copied, skipped) = copy_bus_events_from_reader(source, target, to_run_id)?;
        target_state.next_seq = copied + 1;
        target_state.tail_repaired = true;
        log::debug!(
            "[storage/events] copy_bus_events: {} → {} (copied {} content events, skipped {} lifecycle, new max_seq={})",
            from_run_id,
            to_run_id,
            copied,
            skipped,
            copied
        );
        Ok(())
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
    EVENT_WRITER.copy_bus_events(from_run_id, to_run_id)
}

fn copy_bus_events_from_reader<R: BufRead, W: Write>(
    source: R,
    target: W,
    to_run_id: &str,
) -> Result<(u64, u64), String> {
    // Content event types to copy (conversation history).
    const CONTENT_TYPES: &[&str] = &[
        "message_delta",
        "message_complete",
        "tool_start",
        "tool_end",
        "user_message",
    ];

    let mut output = std::io::BufWriter::new(target);
    let mut copied = 0u64;
    let mut skipped = 0u64;

    for line in source.lines() {
        let line = line.map_err(|e| format!("read source events failed: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut envelope) = serde_json::from_str::<serde_json::Value>(&line) else {
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

            serde_json::to_writer(&mut output, &envelope)
                .map_err(|e| format!("serialize failed: {e}"))?;
            output
                .write_all(b"\n")
                .map_err(|e| format!("write fork events failed: {e}"))?;
        } else {
            skipped += 1;
        }
    }

    output
        .flush()
        .map_err(|e| format!("write fork events failed: {e}"))?;
    Ok((copied, skipped))
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
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(_) => return vec![],
    };

    let min_seq = since_seq.unwrap_or(0);
    let reader = BufReader::new(file);
    reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(&line).ok()?;
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

/// Return a replay page whose encoded event payload stays bounded. The caller advances with
/// `last_seq`; this avoids ever materializing an arbitrarily large catch-up array in Rust or JS.
pub fn list_bus_events_page(
    run_id: &str,
    since_seq: u64,
    offset: Option<u64>,
) -> Result<BusEventPage, String> {
    log::debug!(
        "[storage/events] list_bus_events_page: run_id={}, since_seq={}",
        run_id,
        since_seq
    );
    let path = events_path(run_id);
    if !path.exists() {
        return Ok(BusEventPage {
            events: vec![],
            last_seq: since_seq,
            has_more: false,
            next_offset: offset.unwrap_or(0),
        });
    }
    let mut file = fs::File::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let start_offset = offset.unwrap_or(0);
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if start_offset > length {
        return Err(history_projection_required(
            "bus event offset is outside the current event log",
        ));
    }
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|e| e.to_string())?;
    let page = list_bus_events_page_from_reader(BufReader::new(file), since_seq, start_offset)?;
    log::debug!(
        "[storage/events] list_bus_events_page complete: run_id={}, events={}, last_seq={}, has_more={}",
        run_id,
        page.events.len(),
        page.last_seq,
        page.has_more
    );
    Ok(page)
}

fn list_bus_events_page_from_reader<R: BufRead>(
    mut reader: R,
    since_seq: u64,
    start_offset: u64,
) -> Result<BusEventPage, String> {
    let mut events = Vec::new();
    let mut encoded_bytes = 2usize;
    let mut last_seq = since_seq;
    let mut has_more = false;
    let mut consumed_bytes = 0u64;

    loop {
        let line_start_offset = consumed_bytes;
        let mut line = Vec::with_capacity(16 * 1024);
        let mut oversized = false;
        // Keep only a bounded prefix after the line crosses the hard limit. The envelope seq is
        // serialized before the event payload, so this is enough to decide whether an old large
        // event is already behind the caller's watermark without retaining the whole line.
        let mut prefix = Vec::with_capacity(64 * 1024);
        let mut sequence_scan = Vec::with_capacity(128);
        let mut oversized_seq = None;
        loop {
            let available = reader
                .fill_buf()
                .map_err(|e| format!("read bus event page: {e}"))?;
            if available.is_empty() {
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let data_len = newline.unwrap_or(available.len());
            if prefix.len() < 64 * 1024 {
                let take = (64 * 1024 - prefix.len()).min(data_len);
                prefix.extend_from_slice(&available[..take]);
            }
            sequence_scan.extend_from_slice(&available[..data_len]);
            if let Some(seq) = envelope_seq_from_bytes(&sequence_scan) {
                oversized_seq = Some(seq);
            }
            if sequence_scan.len() > 128 {
                sequence_scan.drain(..sequence_scan.len() - 128);
            }
            if !oversized && line.len().saturating_add(data_len) <= BUS_EVENT_LINE_LIMIT {
                line.extend_from_slice(&available[..data_len]);
            } else {
                oversized = true;
                line.clear();
            }
            let consumed = data_len + usize::from(newline.is_some());
            reader.consume(consumed);
            consumed_bytes += consumed as u64;
            if newline.is_some() {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        if oversized {
            if oversized_seq
                .or_else(|| envelope_seq_from_bytes(&prefix))
                .is_some_and(|seq| seq <= since_seq)
            {
                continue;
            }
            return Err(history_projection_required(
                "bus event exceeds bounded catch-up line limit",
            ));
        }
        let line = std::str::from_utf8(&line)
            .map_err(|e| format!("invalid UTF-8 in bus event page: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if envelope.get("_bus").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let Some(seq) = envelope.get("seq").and_then(|v| v.as_u64()) else {
            continue;
        };
        if seq <= since_seq {
            continue;
        }
        let Some(raw_event) = envelope.get("event") else {
            continue;
        };
        let Some(event_type) = raw_event.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if !REPLAY_TYPES.contains(&event_type) {
            continue;
        }
        let mut event = raw_event.clone();
        if let Some(object) = event.as_object_mut() {
            if let Some(ts) = envelope.get("ts") {
                object.insert("ts".to_string(), ts.clone());
            }
            object.insert("_seq".to_string(), serde_json::Value::Number(seq.into()));
        }
        let event_bytes = serde_json::to_vec(&event).map_err(|e| e.to_string())?.len();
        if event_bytes.saturating_add(2) > BUS_EVENT_PAGE_BYTE_LIMIT {
            return Err(history_projection_required(format!(
                "bus event {seq} exceeds catch-up page limit: {event_bytes} bytes"
            )));
        }
        if !events.is_empty()
            && (events.len() >= BUS_EVENT_PAGE_ENTRY_LIMIT
                || encoded_bytes.saturating_add(event_bytes + 1) > BUS_EVENT_PAGE_BYTE_LIMIT)
        {
            has_more = true;
            consumed_bytes = line_start_offset;
            break;
        }
        encoded_bytes += event_bytes + 1;
        last_seq = seq;
        events.push(event);
    }

    Ok(BusEventPage {
        events,
        last_seq,
        has_more,
        next_offset: start_offset + consumed_bytes,
    })
}

fn envelope_seq_from_bytes(prefix: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(prefix).ok()?;
    let marker = "\"seq\":";
    let start = text.find(marker)? + marker.len();
    let digits: String = text[start..]
        .chars()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{
        copy_bus_events_from_reader, list_bus_events_page_from_reader, max_seq_in_tail,
        repair_incomplete_tail, scan_max_seq, BUS_EVENT_PAGE_ENTRY_LIMIT,
    };
    use std::io::{BufReader, Read as _, Write as _};

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

    #[test]
    fn incomplete_tail_is_truncated_to_last_committed_newline() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{{\"seq\":1}}\n{{\"seq\":2").unwrap();
        file.flush().unwrap();

        let committed = repair_incomplete_tail(file.path()).unwrap();
        assert_eq!(committed, 10);
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "{\"seq\":1}\n"
        );
    }

    #[test]
    fn repair_creates_missing_run_directory_before_first_write() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("new-run").join("events.jsonl");

        assert_eq!(repair_incomplete_tail(&path).unwrap(), 0);
        assert!(path.is_file());
    }

    #[test]
    fn complete_tail_is_preserved() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{{\"seq\":1}}\n{{\"seq\":2}}\n").unwrap();
        file.flush().unwrap();
        let before = std::fs::read(file.path()).unwrap();

        assert_eq!(
            repair_incomplete_tail(file.path()).unwrap(),
            before.len() as u64
        );
        assert_eq!(std::fs::read(file.path()).unwrap(), before);
    }

    #[test]
    fn complete_legacy_tail_without_newline_is_committed_not_deleted() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{{\"seq\":1}}").unwrap();
        file.flush().unwrap();

        assert_eq!(repair_incomplete_tail(file.path()).unwrap(), 10);
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "{\"seq\":1}\n"
        );
    }

    #[test]
    fn snapshot_prefix_never_ends_inside_an_appended_record() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{{\"seq\":1}}\n{{\"seq\":2").unwrap();
        file.flush().unwrap();

        let snapshot = super::capture_event_log_snapshot(file.path()).unwrap();
        assert_eq!(snapshot.metadata.len(), 10);
        let mut body = String::new();
        BufReader::new(snapshot.file)
            .take(snapshot.metadata.len())
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "{\"seq\":1}\n");
    }

    #[test]
    fn fork_copy_reads_only_committed_snapshot_prefix() {
        let committed = serde_json::json!({
            "_bus": true,
            "seq": 1,
            "event": {"type":"user_message","run_id":"source","text":"hello"}
        })
        .to_string();
        let partial = r#"{"_bus":true,"seq":2,"event":{"type":"message_complete""#;
        let body = format!("{committed}\n{partial}");
        let committed_len = committed.len() + 1;
        let mut output = Vec::new();

        let (copied, skipped) = copy_bus_events_from_reader(
            std::io::Cursor::new(body).take(committed_len as u64),
            &mut output,
            "fork",
        )
        .unwrap();

        assert_eq!((copied, skipped), (1, 0));
        let line: serde_json::Value =
            serde_json::from_slice(output.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(line["seq"], 1);
        assert_eq!(line["event"]["run_id"], "fork");
    }

    #[test]
    fn bus_event_catchup_is_paged_and_advances_monotonically() {
        let mut data = Vec::new();
        for seq in 1..=(BUS_EVENT_PAGE_ENTRY_LIMIT as u64 + 5) {
            writeln!(
                data,
                "{}",
                serde_json::json!({
                    "_bus": true,
                    "seq": seq,
                    "ts": "t",
                    "event": {"type": "user_message", "run_id": "run-test", "text": seq.to_string()}
                })
            )
            .unwrap();
        }

        let first = list_bus_events_page_from_reader(std::io::Cursor::new(&data), 0, 0).unwrap();
        assert_eq!(first.events.len(), BUS_EVENT_PAGE_ENTRY_LIMIT);
        assert_eq!(first.last_seq, BUS_EVENT_PAGE_ENTRY_LIMIT as u64);
        assert!(first.has_more);
        let second = list_bus_events_page_from_reader(
            std::io::Cursor::new(&data[first.next_offset as usize..]),
            first.last_seq,
            first.next_offset,
        )
        .unwrap();
        assert_eq!(second.events.len(), 5);
        assert_eq!(second.last_seq, BUS_EVENT_PAGE_ENTRY_LIMIT as u64 + 5);
        assert!(!second.has_more);
    }

    #[test]
    fn bus_event_catchup_rejects_oversized_line_without_buffering_it() {
        let data = format!("{}\n", "x".repeat(super::BUS_EVENT_LINE_LIMIT + 1));
        let error = list_bus_events_page_from_reader(std::io::Cursor::new(data), 0, 0).unwrap_err();
        assert!(error.contains(super::HISTORY_PROJECTION_REQUIRED));
        assert!(error.contains("bounded catch-up line limit"));
    }

    #[test]
    fn bus_event_catchup_requests_projection_for_event_larger_than_page() {
        let envelope = serde_json::json!({
            "_bus": true,
            "seq": 1,
            "ts": "t",
            "event": {
                "type": "message_complete",
                "run_id": "run-test",
                "message_id": "large-message",
                "text": "x".repeat(super::BUS_EVENT_PAGE_BYTE_LIMIT + 1024)
            }
        });
        let data = format!("{envelope}\n");
        assert!(data.len() < super::BUS_EVENT_LINE_LIMIT);

        let error = list_bus_events_page_from_reader(std::io::Cursor::new(data), 0, 0).unwrap_err();
        assert!(error.contains(super::HISTORY_PROJECTION_REQUIRED));
        assert!(error.contains("exceeds catch-up page limit"));
    }

    #[test]
    fn oversized_line_before_watermark_is_skipped_but_new_one_requests_rebuild() {
        let oversized = serde_json::json!({
            "_bus": true,
            "seq": 7,
            "ts": "t",
            "event": {
                "type": "message_complete",
                "run_id": "run-test",
                "text": "x".repeat(super::BUS_EVENT_LINE_LIMIT + 1)
            }
        })
        .to_string();
        let next = serde_json::json!({
            "_bus": true,
            "seq": 8,
            "ts": "t",
            "event": {"type": "user_message", "run_id": "run-test", "text": "next"}
        });
        let data = format!("{oversized}\n{next}\n");

        let page = list_bus_events_page_from_reader(std::io::Cursor::new(&data), 7, 0).unwrap();
        assert_eq!(page.last_seq, 8);
        assert_eq!(page.events.len(), 1);

        let error =
            list_bus_events_page_from_reader(std::io::Cursor::new(&data), 6, 0).unwrap_err();
        assert!(error.contains(super::HISTORY_PROJECTION_REQUIRED));
        assert!(error.contains("bounded catch-up line limit"));
    }
}

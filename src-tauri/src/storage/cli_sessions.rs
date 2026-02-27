//! CLI session discovery, normalization, import, and incremental sync.
//!
//! Reads Claude CLI transcript files (~/.claude/projects/*/*.jsonl) and converts
//! them into OpenCovibe run format (~/.opencovibe/runs/{run-id}/).

use crate::agent::claude_protocol::{validate_bus_event, ProtocolState};
use crate::models::{BusEvent, ImportWatermark, RunMeta, RunSource, RunStatus};
use crate::storage::events::{is_replayable, EventWriter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// ── Types ────────────────────────────────────────────────────────────

/// CLI session summary (discovery phase output).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionSummary {
    pub session_id: String,
    pub cwd: String,
    pub first_prompt: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub message_count: u32,
    pub model: Option<String>,
    pub cli_version: Option<String>,
    pub file_size: u64,
    pub file_path: String,
    pub has_subagents: bool,
    pub already_imported: bool,
    pub existing_run_id: Option<String>,
}

/// Import result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub run_id: String,
    pub session_id: String,
    pub events_imported: u64,
    pub events_skipped: u64,
    pub usage_incomplete: bool,
    pub skipped_subtypes: HashMap<String, u64>,
}

/// Incremental sync result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub new_events: u64,
    pub new_watermark: ImportWatermark,
    pub usage_incomplete: bool,
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Encode cwd for Claude CLI directory naming: '/' → '-'.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.replace('/', "-")
}

fn claude_projects_dir() -> Option<PathBuf> {
    super::dirs_next().map(|h| h.join(".claude").join("projects"))
}

/// Validate that a path is within ~/.claude/projects/ (path traversal guard).
fn validate_cli_path(path: &Path) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("canonicalize failed: {}", e))?;
    let projects_dir = claude_projects_dir().ok_or("cannot determine home dir")?;
    // projects_dir may not exist yet — canonicalize parent check
    if let Ok(canonical_projects) = projects_dir.canonicalize() {
        if !canonical.starts_with(&canonical_projects) {
            return Err(format!(
                "path {:?} is outside ~/.claude/projects/",
                canonical
            ));
        }
    }
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("file is not .jsonl".to_string());
    }
    Ok(())
}

/// SHA-256 hash of a string, returning first 12 hex chars.
fn sha256_short(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    result[..6]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// Generate a source_key for a transcript line.
fn line_key(raw: &Value, byte_offset: u64, raw_trim: &str) -> String {
    let hash = sha256_short(raw_trim);
    let etype = raw
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    // uuid is most reliable
    if let Some(uuid) = raw.get("uuid").and_then(|v| v.as_str()) {
        return uuid.to_string();
    }
    // timestamp + type + hash as next best
    if let Some(ts) = raw.get("timestamp").and_then(|v| v.as_str()) {
        return format!("v1:{}:{}:{}", ts, etype, hash);
    }
    // byte offset fallback
    format!("v1:{}:{}:{}", byte_offset, etype, hash)
}

/// Generate an event-level key from line_key + event type + index.
fn event_key(lk: &str, event_type: &str, n: usize) -> String {
    format!("v1:{}#{}#{}", lk, event_type, n)
}

/// Get the serde tag of a BusEvent.
fn bus_event_tag(event: &BusEvent) -> String {
    // Use serde to get the "type" tag
    if let Ok(v) = serde_json::to_value(event) {
        if let Some(t) = v.get("type").and_then(|v| v.as_str()) {
            return t.to_string();
        }
    }
    "unknown".to_string()
}

/// Extract timestamp from a raw JSON value (CLI transcript line).
fn extract_timestamp(raw: &Value) -> Option<String> {
    raw.get("timestamp")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Path to import-index.jsonl for a run.
fn import_index_path(run_id: &str) -> PathBuf {
    super::run_dir(run_id).join("import-index.jsonl")
}

/// Load source_key set from an import-index file for dedup.
fn load_import_skip_set(index_path: &Path) -> HashSet<String> {
    let mut skip_set = HashSet::new();
    if let Ok(content) = fs::read_to_string(index_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                if let Some(key) = val.get("source_key").and_then(|v| v.as_str()) {
                    skip_set.insert(key.to_string());
                }
            }
        }
    }
    skip_set
}

// ── Schema Normalization ──────────────────────────────────────────

/// Convert a CLI transcript line to stream-json format for map_event().
pub fn normalize_transcript_line(raw: &Value) -> Option<Value> {
    let event_type = raw.get("type")?.as_str()?;
    match event_type {
        "queue-operation" | "file-history-snapshot" => None,

        "progress" => {
            let data = raw.get("data")?;
            let mut out = serde_json::Map::new();

            // type → "system", data.type → subtype
            out.insert("type".into(), json!("system"));
            if let Some(sub) = data.get("type").and_then(|v| v.as_str()) {
                out.insert("subtype".into(), json!(sub));
            }

            // camelCase→snake_case field promotion from data
            let renames = &[
                ("hookEvent", "hook_event"),
                ("hookId", "hook_id"),
                ("hookName", "hook_name"),
                ("outcome", "outcome"),
                ("stdout", "stdout"),
                ("stderr", "stderr"),
                ("exitCode", "exit_code"),
                ("command", "command"),
                ("output", "output"),
                ("exitStatus", "exit_status"),
            ];
            for (src, dst) in renames {
                if let Some(v) = data.get(*src) {
                    out.insert((*dst).into(), v.clone());
                }
            }

            // Pass through unmapped data fields
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    if k == "type" || renames.iter().any(|(s, _)| s == k) {
                        continue;
                    }
                    out.insert(k.clone(), v.clone());
                }
            }

            // Top-level camelCase→snake_case
            let top_renames: &[(&str, &str)] = &[
                ("toolUseID", "hook_id"),
                ("parentToolUseID", "parent_tool_use_id"),
                ("sessionId", "session_id"),
            ];
            for (src, dst) in top_renames {
                if !out.contains_key(*dst) {
                    if let Some(v) = raw.get(*src) {
                        out.insert((*dst).into(), v.clone());
                    }
                }
            }

            // Preserve top-level uuid/timestamp
            if let Some(u) = raw.get("uuid") {
                out.insert("uuid".into(), u.clone());
            }
            if let Some(t) = raw.get("timestamp") {
                out.insert("timestamp".into(), t.clone());
            }

            Some(Value::Object(out))
        }

        "user" | "assistant" | "system" | "result" => {
            let mut out = raw.as_object()?.clone();
            // Top-level camelCase→snake_case
            let top_renames: &[(&str, &str)] = &[
                ("parentToolUseID", "parent_tool_use_id"),
                ("sessionId", "session_id"),
                ("toolUseResult", "tool_use_result"),
            ];
            for (src, dst) in top_renames {
                if let Some(v) = out.remove(*src) {
                    out.insert((*dst).into(), v);
                }
            }
            Some(Value::Object(out))
        }

        // Unknown types: pass through
        _ => Some(raw.clone()),
    }
}

// ── TranscriptImporter ──────────────────────────────────────────────

/// Shared line processing for import and sync.
struct TranscriptImporter {
    run_id: String,
    protocol: ProtocolState,
    event_writer: std::sync::Arc<EventWriter>,
    turn_counter: u32,
    pending_usage: Option<Value>, // Current turn's assistant.message.usage candidate
    has_usage_update_this_turn: bool, // Whether current turn already has a UsageUpdate
    pending_model: Option<String>, // Model from last assistant message
    skipped_subtypes: HashMap<String, u64>,
    events_imported: u64,
    events_skipped: u64,
    usage_incomplete: bool,
    last_user_is_command: bool,      // Last user line was a slash command
    known_usage_turns: HashSet<u64>, // Turns that already have usage_update in events.jsonl
}

impl TranscriptImporter {
    fn new(run_id: String, writer: std::sync::Arc<EventWriter>) -> Self {
        Self {
            run_id,
            protocol: ProtocolState::new(false),
            event_writer: writer,
            turn_counter: 0,
            pending_usage: None,
            has_usage_update_this_turn: false,
            pending_model: None,
            skipped_subtypes: HashMap::new(),
            events_imported: 0,
            events_skipped: 0,
            usage_incomplete: false,
            last_user_is_command: false,
            known_usage_turns: HashSet::new(),
        }
    }

    /// Check if a user line is a real user prompt (not a command/metadata).
    fn is_real_user_prompt(normalized: &Value) -> bool {
        let message = normalized.get("message").unwrap_or(normalized);
        if let Some(is_meta) = normalized.get("isMeta").and_then(|v| v.as_bool()) {
            if is_meta {
                return false;
            }
        }
        if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
            if text.starts_with("<local-command-stdout>") {
                return false;
            }
            if text.contains("<local-command-caveat>") {
                return false;
            }
            if text.contains("<command-name>") {
                return false;
            }
            return true;
        }
        false
    }

    /// Produce a candidate UsageUpdate for the current turn (if needed).
    /// Returns None if already covered by map_event, known_usage_turns, or no data.
    fn flush_turn_usage(&mut self) -> Option<BusEvent> {
        if self.has_usage_update_this_turn {
            return None;
        }
        if self.known_usage_turns.contains(&(self.turn_counter as u64)) {
            log::debug!(
                "[cli_sessions] usage skip (known): turn={}",
                self.turn_counter
            );
            return None;
        }
        if let Some(ref usage) = self.pending_usage {
            let input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64());
            let cache_write = usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64());

            let model = self.pending_model.as_deref().unwrap_or("unknown");
            let cost = crate::pricing::estimate_cost(
                model,
                input_tokens,
                output_tokens,
                cache_read.unwrap_or(0),
                cache_write.unwrap_or(0),
            );

            log::debug!(
                "[cli_sessions] usage synthesized: turn={}, cost={:.6}",
                self.turn_counter,
                cost
            );

            Some(BusEvent::UsageUpdate {
                run_id: self.run_id.clone(),
                input_tokens,
                output_tokens,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                total_cost_usd: cost,
                turn_index: Some(self.turn_counter),
                model_usage: None,
                duration_api_ms: None,
                duration_ms: None,
                num_turns: None,
                stop_reason: None,
                service_tier: None,
                speed: None,
                web_fetch_requests: None,
                cache_creation_5m: None,
                cache_creation_1h: None,
            })
        } else {
            self.usage_incomplete = true;
            log::debug!(
                "[cli_sessions] usage incomplete: turn={}",
                self.turn_counter
            );
            None
        }
    }

    /// Process a single transcript line — two-phase: produce candidates, then write/skip.
    fn process_line(
        &mut self,
        raw_line: &str,
        raw_json: &Value,
        byte_offset: u64,
        index_writer: &mut BufWriter<File>,
        skip_set: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        let raw_trim = raw_line.trim();
        let lk = line_key(raw_json, byte_offset, raw_trim);

        let normalized = match normalize_transcript_line(raw_json) {
            Some(n) => n,
            None => {
                log::trace!("[cli_sessions] normalize: skipped (queue-op/file-history)");
                return Ok(());
            }
        };

        let norm_type = normalized
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ts = extract_timestamp(raw_json)
            .or_else(|| extract_timestamp(&normalized))
            .unwrap_or_default();

        // Track event counts per type for event_key generation
        let mut event_counts: HashMap<String, usize> = HashMap::new();

        // ── Phase 1: Produce candidate events ──

        let mut candidates: Vec<BusEvent> = Vec::new();
        // Handle user messages — synthesize UserMessage
        if norm_type == "user" {
            // Reset command flag on every user line (fix: sticky flag)
            self.last_user_is_command = false;

            if Self::is_real_user_prompt(&normalized) {
                // Flush usage candidate for previous turn (goes through unified write pipeline)
                if self.turn_counter > 0 {
                    if let Some(usage_ev) = self.flush_turn_usage() {
                        candidates.push(usage_ev);
                    }
                }
                self.turn_counter += 1;
                self.pending_usage = None;
                self.has_usage_update_this_turn = false;
                self.pending_model = None;

                let message = normalized.get("message").unwrap_or(&normalized);
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    candidates.push(BusEvent::UserMessage {
                        run_id: self.run_id.clone(),
                        text: text.to_string(),
                    });
                }
            } else {
                // Check if this is a slash command (for command_output filtering)
                let message = normalized.get("message").unwrap_or(&normalized);
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    self.last_user_is_command = text.contains("<command-name>");
                }
            }
        }

        // Track assistant message usage for synthesis
        if norm_type == "assistant" {
            let message = normalized.get("message").unwrap_or(&normalized);
            if let Some(usage) = message.get("usage") {
                self.pending_usage = Some(usage.clone());
            }
            if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                self.pending_model = Some(model.to_string());
            }
        }

        // Run through map_event
        let mapped = self.protocol.map_event(&self.run_id, &normalized);

        // Check for UsageUpdate and validate before extending candidates
        for ev in mapped {
            if matches!(&ev, BusEvent::UsageUpdate { .. }) {
                self.has_usage_update_this_turn = true;
            }
            if let Some(warn) = validate_bus_event(&ev) {
                log::debug!(
                    "[cli_sessions] invalid event dropped: {}.{}: {}",
                    warn.event_type,
                    warn.field,
                    warn.detail
                );
                self.protocol.stats.invalid_tool_count += 1;
                continue;
            }
            candidates.push(ev);
        }

        // ── Phase 2: Filter and write ──

        for event in candidates {
            let tag = bus_event_tag(&event);

            // Replayable filter
            if !is_replayable(&event) {
                self.events_skipped += 1;
                *self.skipped_subtypes.entry(tag.clone()).or_insert(0) += 1;
                continue;
            }

            // command_output content filter
            if let BusEvent::CommandOutput { ref content, .. } = event {
                if content.contains("## Context Usage")
                    || content.contains("## Session Cost")
                    || self.last_user_is_command
                {
                    self.events_skipped += 1;
                    *self
                        .skipped_subtypes
                        .entry("command_output_filtered".to_string())
                        .or_insert(0) += 1;
                    continue;
                }
            }

            let n = event_counts.entry(tag.clone()).or_insert(0);
            let ek = event_key(&lk, &tag, *n);
            *n += 1;

            // Skip-set check (reconcile mode)
            if let Some(ss) = skip_set {
                if ss.contains(&ek) {
                    continue;
                }
            }

            // Write event
            let seq = self
                .event_writer
                .write_bus_event_with_ts(&self.run_id, &event, &ts)?;

            // Write index entry
            writeln!(
                index_writer,
                "{}",
                json!({"source_key": ek, "imported_seq": seq})
            )
            .map_err(|e| format!("write index: {}", e))?;

            self.events_imported += 1;
        }

        Ok(())
    }

    /// Warmup mode: update ProtocolState and turn tracking without writing events.
    fn warmup_line(&mut self, raw_json: &Value) -> Result<(), String> {
        let normalized = match normalize_transcript_line(raw_json) {
            Some(n) => n,
            None => return Ok(()),
        };

        let norm_type = normalized
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Track turns and command flag (must mirror process_line logic)
        if norm_type == "user" {
            self.last_user_is_command = false;
            if Self::is_real_user_prompt(&normalized) {
                // Check previous turn's usage completeness before advancing
                if self.turn_counter > 0
                    && !self.has_usage_update_this_turn
                    && self.pending_usage.is_none()
                {
                    self.usage_incomplete = true;
                }
                self.turn_counter += 1;
                self.pending_usage = None;
                self.has_usage_update_this_turn = false;
                self.pending_model = None;
            } else {
                let message = normalized.get("message").unwrap_or(&normalized);
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    self.last_user_is_command = text.contains("<command-name>");
                }
            }
        }

        // Track assistant usage
        if norm_type == "assistant" {
            let message = normalized.get("message").unwrap_or(&normalized);
            if let Some(usage) = message.get("usage") {
                self.pending_usage = Some(usage.clone());
            }
            if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                self.pending_model = Some(model.to_string());
            }
        }

        // Run map_event for state tracking
        let mapped = self.protocol.map_event(&self.run_id, &normalized);
        for ev in &mapped {
            if matches!(ev, BusEvent::UsageUpdate { .. }) {
                self.has_usage_update_this_turn = true;
            }
            if let Some(warn) = validate_bus_event(ev) {
                log::debug!(
                    "[cli_sessions] invalid event dropped (v2): {}.{}: {}",
                    warn.event_type,
                    warn.field,
                    warn.detail
                );
                self.protocol.stats.invalid_tool_count += 1;
            }
        }

        Ok(())
    }

    /// Finalize — flush usage for the last turn via unified write pipeline.
    fn finalize(
        &mut self,
        ts: &str,
        index_writer: &mut BufWriter<File>,
        skip_set: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        if self.turn_counter > 0 {
            if let Some(event) = self.flush_turn_usage() {
                let lk = format!("v1:finalize:{}", self.turn_counter);
                let tag = bus_event_tag(&event);
                let ek = event_key(&lk, &tag, 0);

                // Skip-set check (reconcile mode)
                if let Some(ss) = skip_set {
                    if ss.contains(&ek) {
                        return Ok(());
                    }
                }

                let seq = self
                    .event_writer
                    .write_bus_event_with_ts(&self.run_id, &event, ts)?;
                writeln!(
                    index_writer,
                    "{}",
                    json!({"source_key": ek, "imported_seq": seq})
                )
                .map_err(|e| format!("write index: {}", e))?;
                self.events_imported += 1;
            }
        }
        Ok(())
    }
}

/// Scan existing events.jsonl for usage_update turn indices.
/// Used by sync/reconcile to avoid re-synthesizing usage for known turns.
fn load_known_usage_turns(run_id: &str) -> HashSet<u64> {
    let events_path = super::run_dir(run_id).join("events.jsonl");
    let mut turns = HashSet::new();
    let Ok(content) = fs::read_to_string(&events_path) else {
        return turns;
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Quick substring check before parsing
        if !trimmed.contains("\"usage_update\"") {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            let event = val.get("event").unwrap_or(&val);
            if event.get("type").and_then(|v| v.as_str()) == Some("usage_update") {
                if let Some(ti) = event.get("turn_index").and_then(|v| v.as_u64()) {
                    turns.insert(ti);
                }
            }
        }
    }
    log::debug!(
        "[cli_sessions] loaded {} known usage turns for run {}",
        turns.len(),
        run_id
    );
    turns
}

// ── Discovery ────────────────────────────────────────────────────────

/// Discover CLI sessions for a given working directory.
pub fn discover_sessions(target_cwd: &str) -> Result<Vec<CliSessionSummary>, String> {
    let start = std::time::Instant::now();
    let projects_dir = claude_projects_dir().ok_or("cannot determine home dir")?;

    if !projects_dir.exists() {
        log::debug!("[cli_sessions] discover: ~/.claude/projects/ does not exist");
        return Ok(vec![]);
    }

    // Collect all JSONL files with metadata
    let mut candidates: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();

    // Quick path: try encoded cwd directory first
    let encoded = encode_cwd(target_cwd);
    let quick_dir = projects_dir.join(&encoded);
    if quick_dir.is_dir() {
        collect_jsonl_files(&quick_dir, &mut candidates);
    }

    // Fallback: scan all project directories
    if candidates.is_empty() {
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_jsonl_files(&path, &mut candidates);
                }
            }
        }
    }

    // Sort by mtime descending, take top 50
    candidates.sort_by(|a, b| b.2.cmp(&a.2));
    candidates.truncate(50);

    log::debug!(
        "[cli_sessions] discover: {} candidate files for cwd={}",
        candidates.len(),
        target_cwd
    );

    // Cross-reference existing imports
    let imported_sessions = build_imported_index();

    // Extract summaries
    let mut results: Vec<CliSessionSummary> = Vec::new();
    for (path, size, _mtime) in &candidates {
        match extract_summary(path, *size, target_cwd, &imported_sessions) {
            Ok(Some(summary)) => results.push(summary),
            Ok(None) => {
                log::trace!("[cli_sessions] discover: skipped {:?} (cwd mismatch)", path);
            }
            Err(e) => {
                log::trace!("[cli_sessions] discover: error reading {:?}: {}", path, e);
            }
        }
    }

    // Sort by last_activity_at descending
    results.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));

    log::debug!(
        "[cli_sessions] discover: {} sessions found in {:?}",
        results.len(),
        start.elapsed()
    );

    Ok(results)
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<(PathBuf, u64, std::time::SystemTime)>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(meta) = path.metadata() {
                    out.push((
                        path,
                        meta.len(),
                        meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    ));
                }
            }
        }
    }
}

fn build_imported_index() -> HashMap<(String, String), String> {
    // Map (session_id, cwd) → run_id
    let mut index = HashMap::new();
    let runs_dir = super::runs_dir();
    if let Ok(entries) = fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if let Ok(content) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<RunMeta>(&content) {
                    if meta.source == Some(RunSource::CliImport) {
                        if let Some(ref sid) = meta.session_id {
                            index.insert((sid.clone(), meta.cwd.clone()), meta.id.clone());
                        }
                    }
                }
            }
        }
    }
    index
}

fn extract_summary(
    path: &Path,
    size: u64,
    target_cwd: &str,
    imported: &HashMap<(String, String), String>,
) -> Result<Option<CliSessionSummary>, String> {
    let file = File::open(path).map_err(|e| format!("open: {}", e))?;
    let reader = BufReader::new(&file);

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let mut cwd: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cli_version: Option<String> = None;
    let mut has_subagents = false;
    let mut message_count: u32 = 0;
    let mut last_ts: Option<String> = None;

    // Read first 20 lines for summary extraction
    let mut head_lines = 0;
    let mut head_bytes: u64 = 0; // Track bytes consumed by head for tail overlap check
    for line_result in reader.lines() {
        let line = line_result.map_err(|e| format!("read: {}", e))?;
        head_lines += 1;
        if head_lines > 20 {
            break;
        }
        head_bytes += (line.len() as u64) + 1; // +1 for newline

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Cheap substring matching for message_count
        if trimmed.contains("\"type\":\"user\"") || trimmed.contains("\"type\":\"assistant\"") {
            message_count += 1;
        }

        // Check for subagents
        if trimmed.contains("\"parentToolUseID\"") || trimmed.contains("\"parent_tool_use_id\"") {
            has_subagents = true;
        }

        let Ok(json_val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        // Extract cwd from any line that has it
        if cwd.is_none() {
            if let Some(c) = json_val.get("cwd").and_then(|v| v.as_str()) {
                cwd = Some(c.to_string());
            }
        }

        // Extract timestamp
        if let Some(ts) = json_val.get("timestamp").and_then(|v| v.as_str()) {
            if started_at.is_none() {
                started_at = Some(ts.to_string());
            }
            last_ts = Some(ts.to_string());
        }

        // Extract first user prompt
        if first_prompt.is_none() && json_val.get("type").and_then(|v| v.as_str()) == Some("user") {
            let message = json_val.get("message").unwrap_or(&json_val);
            if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                if !text.starts_with("<local-command-stdout>") && !text.contains("<command-name>") {
                    let truncated = if text.len() > 200 {
                        let end = text.floor_char_boundary(200);
                        format!("{}...", &text[..end])
                    } else {
                        text.to_string()
                    };
                    first_prompt = Some(truncated);
                }
            }
        }

        // Extract model from system/init progress events
        if json_val.get("type").and_then(|v| v.as_str()) == Some("progress") {
            if let Some(data) = json_val.get("data") {
                if data.get("type").and_then(|v| v.as_str()) == Some("init") {
                    if let Some(m) = data.get("model").and_then(|v| v.as_str()) {
                        model = Some(m.to_string());
                    }
                    if let Some(ver) = data.get("claude_code_version").and_then(|v| v.as_str()) {
                        cli_version = Some(ver.to_string());
                    }
                }
            }
        }

        // Also check direct system/init
        if json_val.get("type").and_then(|v| v.as_str()) == Some("system")
            && json_val.get("subtype").and_then(|v| v.as_str()) == Some("init")
        {
            if model.is_none() {
                if let Some(m) = json_val.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
                }
            }
            if cli_version.is_none() {
                if let Some(ver) = json_val.get("claude_code_version").and_then(|v| v.as_str()) {
                    cli_version = Some(ver.to_string());
                }
            }
            if cwd.is_none() {
                if let Some(c) = json_val.get("cwd").and_then(|v| v.as_str()) {
                    cwd = Some(c.to_string());
                }
            }
        }
    }

    // Fallback: if no cwd found in first 20 lines, check if file is in exact encoded dir
    if cwd.is_none() {
        let encoded = encode_cwd(target_cwd);
        if let Some(parent) = path.parent() {
            if parent.file_name().and_then(|s| s.to_str()) == Some(&encoded) {
                // File is in the exact encoded dir — scan deeper for cwd
                let file2 = File::open(path).map_err(|e| format!("open: {}", e))?;
                let reader2 = BufReader::new(file2);
                for (i, line_result) in reader2.lines().enumerate() {
                    if i >= 100 {
                        break;
                    }
                    if i < 20 {
                        continue; // Already scanned
                    }
                    let line = line_result.map_err(|e| format!("read: {}", e))?;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(json_val) = serde_json::from_str::<Value>(trimmed) {
                        if let Some(c) = json_val.get("cwd").and_then(|v| v.as_str()) {
                            cwd = Some(c.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }

    // Read tail for last timestamp (and remaining message count for large files)
    let file_for_tail = File::open(path).map_err(|e| format!("open: {}", e))?;
    let mut tail_reader = BufReader::new(file_for_tail);
    let tail_offset = size.saturating_sub(8192);
    // Only count messages from tail if tail starts beyond what head already scanned
    let count_messages_in_tail = tail_offset >= head_bytes;
    if tail_offset > 0 {
        tail_reader
            .seek(SeekFrom::Start(tail_offset))
            .map_err(|e| format!("seek: {}", e))?;
        // Skip partial first line
        let mut skip = String::new();
        let _ = tail_reader.read_line(&mut skip);
    }
    for line_result in tail_reader.lines() {
        let line = line_result.map_err(|e| format!("read: {}", e))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if count_messages_in_tail
            && (trimmed.contains("\"type\":\"user\"") || trimmed.contains("\"type\":\"assistant\""))
        {
            message_count += 1;
        }
        if let Ok(json_val) = serde_json::from_str::<Value>(trimmed) {
            if let Some(ts) = json_val.get("timestamp").and_then(|v| v.as_str()) {
                last_ts = Some(ts.to_string());
            }
        }
    }

    // If cwd doesn't match target, skip
    let matched_cwd = match &cwd {
        Some(c) if c == target_cwd => c.clone(),
        _ => return Ok(None),
    };

    let key = (session_id.clone(), matched_cwd.clone());
    let (already_imported, existing_run_id) = if let Some(rid) = imported.get(&key) {
        (true, Some(rid.clone()))
    } else {
        (false, None)
    };

    Ok(Some(CliSessionSummary {
        session_id,
        cwd: matched_cwd,
        first_prompt: first_prompt.unwrap_or_default(),
        started_at: started_at.unwrap_or_default(),
        last_activity_at: last_ts.unwrap_or_default(),
        message_count,
        model,
        cli_version,
        file_size: size,
        file_path: path.to_string_lossy().to_string(),
        has_subagents,
        already_imported,
        existing_run_id,
    }))
}

// ── Import ──────────────────────────────────────────────────────────

/// Import a CLI session as a new run.
pub fn import_session(
    session_id: &str,
    cwd: &str,
    event_writer: std::sync::Arc<EventWriter>,
) -> Result<ImportResult, String> {
    let start = std::time::Instant::now();
    log::debug!(
        "[cli_sessions] import: session_id={}, cwd={}",
        session_id,
        cwd
    );

    // 1. Dedup check
    let imported = build_imported_index();
    let key = (session_id.to_string(), cwd.to_string());
    if let Some(existing_run_id) = imported.get(&key) {
        return Err(format!(
            "session already imported as run {}",
            existing_run_id
        ));
    }

    // 2. Locate CLI JSONL file
    let cli_path = find_cli_session_path(session_id, cwd)?;
    validate_cli_path(&cli_path)?;

    // Verify file stem matches session_id
    let stem = cli_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem != session_id {
        return Err(format!(
            "file stem {:?} doesn't match session_id {:?}",
            stem, session_id
        ));
    }

    let file_meta = fs::metadata(&cli_path).map_err(|e| format!("stat: {}", e))?;
    let file_size = file_meta.len();

    // 3. First pass — raw scan for metadata
    let file = File::open(&cli_path).map_err(|e| format!("open: {}", e))?;
    let reader = BufReader::new(file);

    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;
    let mut has_result = false;
    let mut result_is_error = false;
    let mut first_prompt = String::new();
    let mut model: Option<String> = None;

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| format!("read: {}", e))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(json_val) = serde_json::from_str::<Value>(trimmed) {
            if let Some(ts) = json_val.get("timestamp").and_then(|v| v.as_str()) {
                if first_ts.is_none() {
                    first_ts = Some(ts.to_string());
                }
                last_ts = Some(ts.to_string());
            }

            let etype = json_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if etype == "result" {
                has_result = true;
                if let Some(sub) = json_val.get("subtype").and_then(|v| v.as_str()) {
                    if sub.starts_with("error") {
                        result_is_error = true;
                    }
                }
            }

            // Extract first prompt
            if first_prompt.is_empty() && etype == "user" {
                let message = json_val.get("message").unwrap_or(&json_val);
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    if !text.starts_with("<local-command-stdout>")
                        && !text.contains("<command-name>")
                    {
                        first_prompt = if text.len() > 200 {
                            let end = text.floor_char_boundary(200);
                            format!("{}...", &text[..end])
                        } else {
                            text.to_string()
                        };
                    }
                }
            }

            // Extract model
            if model.is_none() {
                if etype == "progress" {
                    if let Some(data) = json_val.get("data") {
                        if data.get("type").and_then(|v| v.as_str()) == Some("init") {
                            model = data.get("model").and_then(|v| v.as_str()).map(String::from);
                        }
                    }
                } else if etype == "system"
                    && json_val.get("subtype").and_then(|v| v.as_str()) == Some("init")
                {
                    model = json_val
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
        }
    }

    // 4. Create run
    let run_id = uuid::Uuid::new_v4().to_string();
    let status = if has_result && result_is_error {
        RunStatus::Failed
    } else {
        RunStatus::Stopped
    };

    let started_at = first_ts.clone().unwrap_or_else(crate::models::now_iso);
    let ended_at = last_ts.clone();

    #[cfg(unix)]
    let mtime_ns = {
        use std::os::unix::fs::MetadataExt;
        (file_meta.mtime() as u128) * 1_000_000_000 + (file_meta.mtime_nsec() as u128)
    };
    #[cfg(not(unix))]
    let mtime_ns = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let meta = RunMeta {
        id: run_id.clone(),
        prompt: first_prompt,
        cwd: cwd.to_string(),
        agent: "claude".to_string(),
        auth_mode: "cli".to_string(),
        status,
        started_at,
        ended_at,
        exit_code: None,
        error_message: None,
        session_id: Some(session_id.to_string()),
        result_subtype: None,
        model,
        parent_run_id: None,
        name: None,
        remote_host_name: None,
        remote_cwd: None,
        remote_host_snapshot: None,
        platform_id: None,
        platform_base_url: None,
        source: Some(RunSource::CliImport),
        cli_import_watermark: Some(ImportWatermark {
            offset: file_size,
            mtime_ns,
            file_size,
            last_uuid: None,
        }),
        cli_session_path: Some(cli_path.to_string_lossy().to_string()),
        cli_usage_incomplete: None, // Set after import
    };

    let run_dir = super::run_dir(&run_id);
    super::ensure_dir(&run_dir).map_err(|e| format!("ensure_dir: {}", e))?;

    // 5. Second pass — event conversion + index writing
    // Wrapped in closure so any `?` failure triggers cleanup in the match below.
    let import_result = (|| -> Result<TranscriptImporter, String> {
        let mut importer = TranscriptImporter::new(run_id.clone(), event_writer.clone());

        let index_path = import_index_path(&run_id);
        let index_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .map_err(|e| format!("open index: {}", e))?;
        let mut index_writer = BufWriter::new(index_file);

        let file2 = File::open(&cli_path).map_err(|e| format!("open: {}", e))?;
        let reader2 = BufReader::new(file2);
        let mut byte_offset: u64 = 0;

        for line_result in reader2.lines() {
            let line = line_result.map_err(|e| format!("read: {}", e))?;
            let current_offset = byte_offset;
            byte_offset += (line.len() as u64) + 1;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Ok(json_val) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };

            importer.process_line(&line, &json_val, current_offset, &mut index_writer, None)?;
        }

        // Finalize (flush last turn usage)
        let final_ts = last_ts.clone().unwrap_or_default();
        importer.finalize(&final_ts, &mut index_writer, None)?;
        index_writer
            .flush()
            .map_err(|e| format!("flush index: {}", e))?;

        Ok(importer)
    })();

    // On failure: clean up run_dir and propagate error
    let importer = match import_result {
        Ok(v) => v,
        Err(e) => {
            log::error!("[cli_sessions] import failed, cleaning up run_dir: {}", e);
            let _ = fs::remove_dir_all(&run_dir);
            return Err(e);
        }
    };

    // 6. Save meta atomically (only on success)
    let mut meta = meta;
    meta.cli_usage_incomplete = if importer.usage_incomplete {
        Some(true)
    } else {
        None
    };
    meta.cli_import_watermark = Some(ImportWatermark {
        offset: file_size,
        mtime_ns,
        file_size,
        last_uuid: None,
    });
    super::runs::save_meta(&meta)?;

    let elapsed = start.elapsed();
    log::debug!(
        "[cli_sessions] import: done in {:?}, events_imported={}, events_skipped={}, usage_incomplete={}",
        elapsed,
        importer.events_imported,
        importer.events_skipped,
        importer.usage_incomplete
    );

    Ok(ImportResult {
        run_id,
        session_id: session_id.to_string(),
        events_imported: importer.events_imported,
        events_skipped: importer.events_skipped,
        usage_incomplete: importer.usage_incomplete,
        skipped_subtypes: importer.skipped_subtypes,
    })
}

fn find_cli_session_path(session_id: &str, cwd: &str) -> Result<PathBuf, String> {
    let projects_dir = claude_projects_dir().ok_or("cannot determine home dir")?;
    let filename = format!("{}.jsonl", session_id);

    // Quick path: encoded cwd directory
    let encoded = encode_cwd(cwd);
    let quick_path = projects_dir.join(&encoded).join(&filename);
    if quick_path.exists() {
        return Ok(quick_path);
    }

    // Fallback: scan all project directories
    if let Ok(entries) = fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let candidate = entry.path().join(&filename);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(format!("CLI session file not found: {}", filename))
}

// ── Sync ──────────────────────────────────────────────────────────

/// Incremental sync — import new events since last watermark.
pub fn sync_session(
    run_id: &str,
    event_writer: std::sync::Arc<EventWriter>,
) -> Result<SyncResult, String> {
    let start = std::time::Instant::now();
    log::debug!("[cli_sessions] sync: run_id={}", run_id);

    // 1. Read RunMeta
    let meta = super::runs::get_run(run_id).ok_or_else(|| format!("run {} not found", run_id))?;
    let watermark = meta
        .cli_import_watermark
        .ok_or("no cli_import_watermark in RunMeta")?;
    let cli_path_str = meta
        .cli_session_path
        .ok_or("no cli_session_path in RunMeta")?;
    let cli_path = PathBuf::from(&cli_path_str);
    validate_cli_path(&cli_path)?;

    // Verify stem matches session_id
    if let Some(ref sid) = meta.session_id {
        let stem = cli_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem != sid {
            return Err(format!(
                "file stem {:?} doesn't match session_id {:?}",
                stem, sid
            ));
        }
    }

    let file_meta = fs::metadata(&cli_path).map_err(|e| format!("stat: {}", e))?;
    let current_size = file_meta.len();

    #[cfg(unix)]
    let current_mtime_ns = {
        use std::os::unix::fs::MetadataExt;
        (file_meta.mtime() as u128) * 1_000_000_000 + (file_meta.mtime_nsec() as u128)
    };
    #[cfg(not(unix))]
    let current_mtime_ns = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // 2. Determine sync strategy
    let file_identity_ok =
        current_size >= watermark.offset && current_mtime_ns >= watermark.mtime_ns;

    if !file_identity_ok {
        // Check if import-index exists for reconcile
        let index_path = import_index_path(run_id);
        if index_path.exists() {
            // Branch B: Reconcile
            log::debug!("[cli_sessions] sync: reconcile mode (file identity mismatch)");
            return sync_reconcile(run_id, &cli_path, event_writer);
        } else {
            // Branch C: Cannot reconcile
            log::debug!("[cli_sessions] sync: cannot reconcile (no import-index, file mismatch)");
            return Err("reconcile_index_missing".to_string());
        }
    }

    // Branch A: Normal append
    log::debug!(
        "[cli_sessions] sync: append mode, offset={}, file_size={}",
        watermark.offset,
        current_size
    );

    if current_size == watermark.offset {
        // No new data
        return Ok(SyncResult {
            new_events: 0,
            new_watermark: ImportWatermark {
                offset: current_size,
                mtime_ns: current_mtime_ns,
                file_size: current_size,
                last_uuid: watermark.last_uuid,
            },
            usage_incomplete: meta.cli_usage_incomplete.unwrap_or(false),
        });
    }

    let mut importer = TranscriptImporter::new(run_id.to_string(), event_writer.clone());
    importer.known_usage_turns = load_known_usage_turns(run_id);

    // Warmup: scan from beginning to watermark.offset
    let file = File::open(&cli_path).map_err(|e| format!("open: {}", e))?;
    let mut reader = BufReader::new(file);
    let mut byte_offset: u64 = 0;
    let mut warmup_lines = 0u64;

    while byte_offset < watermark.offset {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|e| format!("read: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        byte_offset += bytes_read as u64;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(json_val) = serde_json::from_str::<Value>(trimmed) {
            importer.warmup_line(&json_val)?;
            warmup_lines += 1;
        }
    }

    log::debug!(
        "[cli_sessions] sync: warmup done, {} lines, turn_counter={}",
        warmup_lines,
        importer.turn_counter
    );

    // Load existing import-index for dedup (guards against watermark drift
    // caused by CLI context compaction rewriting the JSONL file)
    let index_path = import_index_path(run_id);
    let skip_set = load_import_skip_set(&index_path);
    log::debug!(
        "[cli_sessions] sync: loaded {} existing keys for dedup",
        skip_set.len()
    );

    let index_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)
        .map_err(|e| format!("open index: {}", e))?;
    let mut index_writer = BufWriter::new(index_file);

    let mut last_ts = String::new();

    loop {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|e| format!("read: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        let current_offset = byte_offset;
        byte_offset += bytes_read as u64;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(json_val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = extract_timestamp(&json_val) {
            last_ts = ts;
        }

        importer.process_line(
            &line,
            &json_val,
            current_offset,
            &mut index_writer,
            Some(&skip_set),
        )?;
    }

    // Finalize
    importer.finalize(&last_ts, &mut index_writer, Some(&skip_set))?;
    index_writer
        .flush()
        .map_err(|e| format!("flush index: {}", e))?;

    // Update watermark
    let new_watermark = ImportWatermark {
        offset: byte_offset,
        mtime_ns: current_mtime_ns,
        file_size: current_size,
        last_uuid: None,
    };

    // Update RunMeta
    let mut updated_meta =
        super::runs::get_run(run_id).ok_or_else(|| format!("run {} not found", run_id))?;
    updated_meta.cli_import_watermark = Some(new_watermark.clone());
    // Warmup now tracks full usage completeness — use importer's authoritative state
    updated_meta.cli_usage_incomplete = if importer.usage_incomplete {
        Some(true)
    } else {
        None
    };
    super::runs::save_meta(&updated_meta)?;

    let elapsed = start.elapsed();
    log::debug!(
        "[cli_sessions] sync: done in {:?}, new_events={}",
        elapsed,
        importer.events_imported
    );

    Ok(SyncResult {
        new_events: importer.events_imported,
        new_watermark,
        usage_incomplete: importer.usage_incomplete,
    })
}

/// Reconcile sync — full re-scan with dedup via import-index.
fn sync_reconcile(
    run_id: &str,
    cli_path: &Path,
    event_writer: std::sync::Arc<EventWriter>,
) -> Result<SyncResult, String> {
    let start = std::time::Instant::now();
    log::debug!("[cli_sessions] reconcile: run_id={}", run_id);

    // Load existing import-index for dedup
    let index_path = import_index_path(run_id);
    let skip_set = load_import_skip_set(&index_path);
    log::debug!(
        "[cli_sessions] reconcile: loaded {} existing keys",
        skip_set.len()
    );

    let mut importer = TranscriptImporter::new(run_id.to_string(), event_writer.clone());
    importer.known_usage_turns = load_known_usage_turns(run_id);

    let index_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)
        .map_err(|e| format!("open index: {}", e))?;
    let mut index_writer = BufWriter::new(index_file);

    let file = File::open(cli_path).map_err(|e| format!("open: {}", e))?;
    let reader = BufReader::new(file);
    let mut byte_offset: u64 = 0;
    let mut last_ts = String::new();

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| format!("read: {}", e))?;
        let current_offset = byte_offset;
        byte_offset += (line.len() as u64) + 1;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(json_val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = extract_timestamp(&json_val) {
            last_ts = ts;
        }

        importer.process_line(
            &line,
            &json_val,
            current_offset,
            &mut index_writer,
            Some(&skip_set),
        )?;
    }

    importer.finalize(&last_ts, &mut index_writer, Some(&skip_set))?;
    index_writer
        .flush()
        .map_err(|e| format!("flush index: {}", e))?;

    // Rebuild watermark
    let file_meta = fs::metadata(cli_path).map_err(|e| format!("stat: {}", e))?;
    #[cfg(unix)]
    let mtime_ns = {
        use std::os::unix::fs::MetadataExt;
        (file_meta.mtime() as u128) * 1_000_000_000 + (file_meta.mtime_nsec() as u128)
    };
    #[cfg(not(unix))]
    let mtime_ns = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let new_watermark = ImportWatermark {
        offset: byte_offset,
        mtime_ns,
        file_size: file_meta.len(),
        last_uuid: None,
    };

    // Update RunMeta
    let mut meta =
        super::runs::get_run(run_id).ok_or_else(|| format!("run {} not found", run_id))?;
    meta.cli_import_watermark = Some(new_watermark.clone());
    meta.cli_usage_incomplete = if importer.usage_incomplete {
        Some(true)
    } else {
        None
    };
    super::runs::save_meta(&meta)?;

    log::debug!(
        "[cli_sessions] reconcile: done in {:?}, new_events={}",
        start.elapsed(),
        importer.events_imported
    );

    Ok(SyncResult {
        new_events: importer.events_imported,
        new_watermark,
        usage_incomplete: importer.usage_incomplete,
    })
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_cwd() {
        assert_eq!(encode_cwd("/Users/alice/project"), "-Users-alice-project");
        assert_eq!(encode_cwd("/"), "-");
        assert_eq!(encode_cwd("relative"), "relative");
    }

    #[test]
    fn test_normalize_queue_operation() {
        let raw = json!({"type": "queue-operation", "data": {}});
        assert!(normalize_transcript_line(&raw).is_none());
    }

    #[test]
    fn test_normalize_file_history_snapshot() {
        let raw = json!({"type": "file-history-snapshot", "data": {}});
        assert!(normalize_transcript_line(&raw).is_none());
    }

    #[test]
    fn test_normalize_progress_to_system() {
        let raw = json!({
            "type": "progress",
            "data": {
                "type": "init",
                "model": "claude-opus-4-6",
                "cwd": "/test"
            },
            "uuid": "abc-123",
            "timestamp": "2026-02-24T12:00:00Z"
        });
        let normalized = normalize_transcript_line(&raw).unwrap();
        assert_eq!(normalized.get("type").unwrap().as_str().unwrap(), "system");
        assert_eq!(normalized.get("subtype").unwrap().as_str().unwrap(), "init");
        assert_eq!(
            normalized.get("model").unwrap().as_str().unwrap(),
            "claude-opus-4-6"
        );
        assert!(normalized.get("uuid").is_some());
        assert!(normalized.get("timestamp").is_some());
    }

    #[test]
    fn test_normalize_progress_hook_started() {
        let raw = json!({
            "type": "progress",
            "data": {
                "type": "hook_started",
                "hookEvent": "PreToolUse",
                "hookId": "h1",
                "hookName": "my-hook"
            }
        });
        let normalized = normalize_transcript_line(&raw).unwrap();
        assert_eq!(normalized.get("type").unwrap().as_str().unwrap(), "system");
        assert_eq!(
            normalized.get("subtype").unwrap().as_str().unwrap(),
            "hook_started"
        );
        assert_eq!(
            normalized.get("hook_event").unwrap().as_str().unwrap(),
            "PreToolUse"
        );
        assert_eq!(normalized.get("hook_id").unwrap().as_str().unwrap(), "h1");
        assert_eq!(
            normalized.get("hook_name").unwrap().as_str().unwrap(),
            "my-hook"
        );
    }

    #[test]
    fn test_normalize_assistant_parent_tool_use_id() {
        let raw = json!({
            "type": "assistant",
            "parentToolUseID": "tool-123",
            "message": {
                "id": "msg-1",
                "content": [{"type": "text", "text": "hello"}]
            }
        });
        let normalized = normalize_transcript_line(&raw).unwrap();
        assert_eq!(
            normalized
                .get("parent_tool_use_id")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool-123"
        );
        assert!(normalized.get("parentToolUseID").is_none());
    }

    #[test]
    fn test_normalize_progress_session_id() {
        let raw = json!({
            "type": "progress",
            "sessionId": "ses-abc",
            "data": {
                "type": "init",
                "cwd": "/test"
            }
        });
        let normalized = normalize_transcript_line(&raw).unwrap();
        assert_eq!(
            normalized.get("session_id").unwrap().as_str().unwrap(),
            "ses-abc"
        );
    }

    #[test]
    fn test_normalize_unknown_type_passthrough() {
        let raw = json!({"type": "unknown_event", "data": 42});
        let normalized = normalize_transcript_line(&raw).unwrap();
        assert_eq!(
            normalized.get("type").unwrap().as_str().unwrap(),
            "unknown_event"
        );
    }

    #[test]
    fn test_normalize_user_event_tool_use_result() {
        let raw = json!({
            "type": "user",
            "toolUseResult": {
                "filePath": "src/main.rs",
                "structuredPatch": [{"oldStart": 551, "oldLines": 6, "newStart": 551, "newLines": 5, "lines": [" a", "-b", "+c"]}],
                "originalFile": "full file content here"
            },
            "parentToolUseID": "tu_abc"
        });
        let normalized = normalize_transcript_line(&raw).unwrap();
        // toolUseResult → tool_use_result
        let tur = normalized
            .get("tool_use_result")
            .expect("tool_use_result missing");
        assert!(tur.get("structuredPatch").is_some());
        assert_eq!(
            tur.get("structuredPatch").unwrap()[0]
                .get("oldStart")
                .unwrap()
                .as_i64()
                .unwrap(),
            551
        );
        // parentToolUseID → parent_tool_use_id
        assert_eq!(
            normalized
                .get("parent_tool_use_id")
                .unwrap()
                .as_str()
                .unwrap(),
            "tu_abc"
        );
    }

    #[test]
    fn test_is_real_user_prompt() {
        // Real prompt
        let real = json!({
            "type": "user",
            "message": {"content": "Fix the login bug"}
        });
        assert!(TranscriptImporter::is_real_user_prompt(&real));

        // Command output
        let cmd = json!({
            "type": "user",
            "message": {"content": "<local-command-stdout>output</local-command-stdout>"}
        });
        assert!(!TranscriptImporter::is_real_user_prompt(&cmd));

        // Meta
        let meta = json!({
            "type": "user",
            "isMeta": true,
            "message": {"content": "something"}
        });
        assert!(!TranscriptImporter::is_real_user_prompt(&meta));

        // Slash command
        let slash = json!({
            "type": "user",
            "message": {"content": "<command-name>/cost</command-name>"}
        });
        assert!(!TranscriptImporter::is_real_user_prompt(&slash));

        // Array content (not a real prompt)
        let array = json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result"}]}
        });
        assert!(!TranscriptImporter::is_real_user_prompt(&array));
    }

    #[test]
    fn test_source_key_uuid_priority() {
        let raw = json!({
            "type": "user",
            "uuid": "abc-def-123",
            "timestamp": "2026-01-01T00:00:00Z"
        });
        let key = line_key(&raw, 100, "raw line");
        assert_eq!(key, "abc-def-123");
    }

    #[test]
    fn test_source_key_timestamp_fallback() {
        let raw = json!({
            "type": "user",
            "timestamp": "2026-01-01T00:00:00Z"
        });
        let key = line_key(&raw, 100, "raw line");
        assert!(key.starts_with("v1:2026-01-01T00:00:00Z:user:"));
    }

    #[test]
    fn test_source_key_offset_fallback() {
        let raw = json!({"type": "user"});
        let key = line_key(&raw, 42, "raw line");
        assert!(key.starts_with("v1:42:user:"));
    }

    #[test]
    fn test_event_key_format() {
        let ek = event_key("abc-def", "tool_end", 1);
        assert_eq!(ek, "v1:abc-def#tool_end#1");
    }

    #[test]
    fn test_is_replayable() {
        let replayable = BusEvent::UserMessage {
            run_id: "r".into(),
            text: "hi".into(),
        };
        assert!(is_replayable(&replayable));

        let not_replayable = BusEvent::Raw {
            run_id: "r".into(),
            source: "test".into(),
            data: json!({}),
        };
        assert!(!is_replayable(&not_replayable));
    }
}

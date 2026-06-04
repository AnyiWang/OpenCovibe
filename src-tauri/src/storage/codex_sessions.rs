//! Codex rollout discovery, import, and incremental sync.
//!
//! Reads Codex CLI rollout files (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`)
//! and converts them into OpenCovibe run format. Unlike Claude (single JSONL
//! per session), a single Codex thread can produce multiple rollout files
//! across `codex exec resume` invocations — discovery groups by thread_id, and
//! import processes every rollout file belonging to a thread in mtime order.
//!
//! Source-of-truth: `codex-rs/protocol/src/protocol.rs` (RolloutLine, EventMsg)
//! and `codex-rs/protocol/src/models.rs` (ResponseItem).

use crate::models::{
    BusEvent, CodexImportedRollout, ConversationRef, ExecutionPath, RunMeta, RunSource, RunStatus,
};
use crate::storage::cli_sessions::{
    build_imported_index, build_imported_index_cached, invalidate_imported_cache,
};
use crate::storage::cli_sessions_common::{
    cache_key, event_key, load_import_skip_set, scan_cache_path, sha256_short, CachedFile,
    CliSessionSummary, DiscoverResult, DiskScanCache, ImportResult, SyncResult,
};
use crate::storage::events::{is_replayable, EventWriter};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// ── Constants ───────────────────────────────────────────────────────

const MAX_DISCOVER_CANDIDATES: usize = 500;
const SCAN_HEAD_LINES_FOR_FIRST_PROMPT: usize = 100;
const SUMMARY_CACHE_VERSION: u32 = 1;

fn summary_cache_path() -> PathBuf {
    scan_cache_path("codex-summary-scan-cache.json")
}

/// Cacheable per-rollout-file scan used to build discovery summaries. Aggregating
/// these across a thread's files (mtime asc) reproduces the old single-pass
/// `build_summary` result, but unchanged files are skipped via the disk cache.
#[derive(Clone, Default, Serialize, Deserialize)]
struct SummaryFileScan {
    /// Count of `event_msg/task_complete` records in this file.
    task_complete_count: u32,
    /// Last `timestamp` field seen in the file (for last_activity_at).
    last_ts: Option<String>,
    /// First `user_message` text within the head window (truncated for the prompt preview).
    first_user_message: Option<String>,
    /// Last `turn_context.payload.model` seen in the file (real model name).
    last_model: Option<String>,
}

// ── Paths ────────────────────────────────────────────────────────────

fn codex_sessions_dir() -> Option<PathBuf> {
    super::dirs_next().map(|h| h.join(".codex").join("sessions"))
}

/// Validate path is within `~/.codex/sessions/` (path traversal guard).
fn validate_codex_path(path: &Path) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("canonicalize failed: {}", e))?;
    let root = codex_sessions_dir().ok_or("cannot determine home dir")?;
    if let Ok(canonical_root) = root.canonicalize() {
        if !canonical.starts_with(&canonical_root) {
            return Err(format!(
                "path {:?} is outside ~/.codex/sessions/",
                canonical
            ));
        }
    }
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("file is not .jsonl".to_string());
    }
    Ok(())
}

fn import_index_path(run_id: &str) -> PathBuf {
    super::run_dir(run_id).join("import-index.jsonl")
}

// ── Rollout walk + metadata read ────────────────────────────────────

#[derive(Debug, Clone)]
struct RolloutFileInfo {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    mtime_ns: u128,
    meta: RolloutMeta,
}

/// Bits of session_meta payload we care about.
#[derive(Debug, Clone, Default)]
struct RolloutMeta {
    thread_id: String,
    timestamp: String,
    cwd: String,
    cli_version: Option<String>,
    model_provider: Option<String>,
}

fn mtime_ns(meta: &fs::Metadata) -> u128 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.mtime() as u128) * 1_000_000_000 + (meta.mtime_nsec() as u128)
    }
    #[cfg(not(unix))]
    {
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }
}

/// Walk `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and collect file metadata.
fn collect_rollout_files(root: &Path) -> Vec<(PathBuf, u64, SystemTime, u128)> {
    let mut out = Vec::new();
    let Ok(years) = fs::read_dir(root) else {
        return out;
    };
    for y in years.flatten() {
        if !y.path().is_dir() {
            continue;
        }
        let Ok(months) = fs::read_dir(y.path()) else {
            continue;
        };
        for m in months.flatten() {
            if !m.path().is_dir() {
                continue;
            }
            let Ok(days) = fs::read_dir(m.path()) else {
                continue;
            };
            for d in days.flatten() {
                if !d.path().is_dir() {
                    continue;
                }
                let Ok(files) = fs::read_dir(d.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let p = f.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Some(stem) = p.file_name().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if !stem.starts_with("rollout-") {
                        continue;
                    }
                    let Ok(md) = p.metadata() else {
                        continue;
                    };
                    let modified = md.modified().unwrap_or(std::time::UNIX_EPOCH);
                    out.push((p, md.len(), modified, mtime_ns(&md)));
                }
            }
        }
    }
    out
}

/// Read the first non-empty line of a rollout, expecting `session_meta`.
fn read_first_session_meta(path: &Path) -> Result<RolloutMeta, String> {
    let file = File::open(path).map_err(|e| format!("open: {}", e))?;
    for line in BufReader::new(file).lines().take(5) {
        let line = line.map_err(|e| format!("read: {}", e))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let json: Value =
            serde_json::from_str(trimmed).map_err(|e| format!("parse first line: {}", e))?;
        if json.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
            return Err(format!(
                "expected session_meta as first record, got {:?}",
                json.get("type")
            ));
        }
        let payload = json.get("payload").ok_or("session_meta missing payload")?;
        let thread_id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("session_meta missing id")?
            .to_string();
        let timestamp = payload
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| json.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""))
            .to_string();
        let cwd = payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cli_version = payload
            .get("cli_version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let model_provider = payload
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        return Ok(RolloutMeta {
            thread_id,
            timestamp,
            cwd,
            cli_version,
            model_provider,
        });
    }
    Err("empty rollout file".to_string())
}

// ── Discovery ────────────────────────────────────────────────────────

/// Discover Codex sessions grouped by thread_id.
///
/// **Hard contract**: this function may truncate to `MAX_DISCOVER_CANDIDATES`
/// rollout files (mtime desc). Import does a fresh full walk via
/// `find_rollouts_for_thread`, so old rollouts beyond truncation are still
/// importable once the user selects a thread.
pub fn discover_sessions(target_cwd: &str) -> Result<DiscoverResult, String> {
    let root = match codex_sessions_dir() {
        Some(r) => r,
        None => {
            return Ok(DiscoverResult {
                sessions: Vec::new(),
                total: 0,
                truncated: false,
            })
        }
    };
    let imported = build_imported_index_cached(Duration::from_secs(30));
    discover_sessions_in_root(&root, target_cwd, &imported, &summary_cache_path())
}

/// Testable inner: discovery against an explicit root + imported-index. The
/// summary scan cache lives at `cache_path` (a parameter so tests give each run
/// its own file — no shared global cache state).
fn discover_sessions_in_root(
    root: &Path,
    target_cwd: &str,
    imported: &crate::storage::cli_sessions::ImportedIndex,
    cache_path: &Path,
) -> Result<DiscoverResult, String> {
    let start = std::time::Instant::now();
    if !root.exists() {
        return Ok(DiscoverResult {
            sessions: Vec::new(),
            total: 0,
            truncated: false,
        });
    }

    let mut files = collect_rollout_files(root);
    files.sort_by_key(|x| std::cmp::Reverse(x.2));
    let total_candidates = files.len();
    let truncated = files.len() > MAX_DISCOVER_CANDIDATES;
    if truncated {
        files.truncate(MAX_DISCOVER_CANDIDATES);
    }

    log::debug!(
        "[codex_sessions] discover: {} candidate files (total={}, truncated={})",
        files.len(),
        total_candidates,
        truncated
    );

    let parsed: Vec<RolloutFileInfo> = files
        .par_iter()
        .filter_map(|(path, size, mtime, mtime_ns_val)| {
            let meta = read_first_session_meta(path).ok()?;
            Some(RolloutFileInfo {
                path: path.clone(),
                size: *size,
                mtime: *mtime,
                mtime_ns: *mtime_ns_val,
                meta,
            })
        })
        .collect();

    // Scan each rollout's body once, reusing unchanged files from the disk cache
    // (key = path + mtime_ns + size). This is the expensive part of discovery —
    // it used to be a single-threaded per-thread re-parse on every call.
    let scans = scan_summary_files(&parsed, cache_path);

    // Group by thread_id
    let mut groups: HashMap<String, Vec<RolloutFileInfo>> = HashMap::new();
    for fi in parsed {
        groups
            .entry(fi.meta.thread_id.clone())
            .or_default()
            .push(fi);
    }

    let show_all = target_cwd.is_empty() || target_cwd == "/";

    let mut summaries: Vec<CliSessionSummary> = groups
        .into_iter()
        .filter_map(|(thread_id, mut files)| {
            files.sort_by_key(|f| f.mtime); // asc
            let latest = files.last()?;
            if !show_all && latest.meta.cwd != target_cwd {
                return None;
            }
            build_summary(&thread_id, &files, &scans, imported)
        })
        .collect();

    summaries.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));

    log::debug!(
        "[codex_sessions] discover: {} threads in {:?}",
        summaries.len(),
        start.elapsed()
    );

    let total = summaries.len();
    Ok(DiscoverResult {
        sessions: summaries,
        total: if truncated { total_candidates } else { total },
        truncated,
    })
}

/// Scan one rollout's body into a cacheable `SummaryFileScan`.
///
/// Mirrors the per-file pass the old `build_summary` did inline: counts
/// `task_complete`, tracks the last timestamp, the first head-window
/// `user_message`, and the last `turn_context` model.
fn scan_summary_file(path: &Path) -> SummaryFileScan {
    let mut scan = SummaryFileScan::default();
    let Ok(file) = File::open(path) else {
        return scan;
    };
    let mut head_seen = 0usize;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if let Some(ts) = json.get("timestamp").and_then(|v| v.as_str()) {
            scan.last_ts = Some(ts.to_string());
        }
        let outer_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if outer_type == "event_msg" {
            let inner = json
                .get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if inner == "task_complete" {
                scan.task_complete_count = scan.task_complete_count.saturating_add(1);
            }
            if scan.first_user_message.is_none()
                && inner == "user_message"
                && head_seen < SCAN_HEAD_LINES_FOR_FIRST_PROMPT
            {
                if let Some(text) = json
                    .get("payload")
                    .and_then(|p| p.get("message"))
                    .and_then(|v| v.as_str())
                {
                    scan.first_user_message = Some(truncate_prompt(text));
                }
            }
        } else if outer_type == "turn_context" {
            if let Some(m) = json
                .get("payload")
                .and_then(|p| p.get("model"))
                .and_then(|v| v.as_str())
            {
                scan.last_model = Some(m.to_string());
            }
        }
        head_seen += 1;
    }
    scan
}

/// Scan all discovered rollout files into a `path → SummaryFileScan` map, reusing
/// unchanged files from the disk cache at `cache_path` and re-scanning only
/// new/modified ones in parallel. The refreshed cache is written back
/// (best-effort). `cache_path` is a parameter (not a global) so tests can point
/// each invocation at its own file with no shared mutable state.
fn scan_summary_files(
    files: &[RolloutFileInfo],
    cache_path: &Path,
) -> HashMap<String, SummaryFileScan> {
    let mut old_cache = DiskScanCache::<SummaryFileScan>::read(cache_path, SUMMARY_CACHE_VERSION)
        .unwrap_or_else(|| {
            crate::storage::cli_sessions_common::empty_scan_cache(SUMMARY_CACHE_VERSION)
        });

    // Split into cache hits (reused as-is) and misses (need a fresh scan). Pull
    // hits out of the old cache first so the parallel scan only touches misses.
    let mut result: HashMap<String, SummaryFileScan> = HashMap::new();
    let mut misses: Vec<&RolloutFileInfo> = Vec::new();
    for f in files {
        let key = cache_key(&f.path);
        match old_cache.take_if_fresh(&key, f.mtime_ns, f.size) {
            Some(data) => {
                result.insert(key, data);
            }
            None => misses.push(f),
        }
    }

    let scanned: Vec<(String, SummaryFileScan)> = misses
        .par_iter()
        .map(|f| (cache_key(&f.path), scan_summary_file(&f.path)))
        .collect();
    result.extend(scanned);

    // Rebuild the manifest from exactly the files we just discovered so stale
    // entries (deleted rollouts) are dropped, then persist.
    let manifest: HashMap<String, CachedFile<SummaryFileScan>> = files
        .iter()
        .filter_map(|f| {
            let key = cache_key(&f.path);
            result.get(&key).map(|data| {
                (
                    key,
                    CachedFile {
                        mtime_ns: f.mtime_ns,
                        size: f.size,
                        data: data.clone(),
                    },
                )
            })
        })
        .collect();
    DiskScanCache {
        version: SUMMARY_CACHE_VERSION,
        manifest,
    }
    .write(cache_path);

    result
}

fn build_summary(
    thread_id: &str,
    files_asc: &[RolloutFileInfo],
    scans: &HashMap<String, SummaryFileScan>,
    imported: &crate::storage::cli_sessions::ImportedIndex,
) -> Option<CliSessionSummary> {
    let earliest = files_asc.first()?;
    let latest = files_asc.last()?;

    let mut total_size: u64 = 0;
    let mut message_count: u32 = 0;
    let mut last_activity_ts: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    // Real model name (e.g. "gpt-5.5") lives in turn_context.payload.model, NOT in
    // session_meta (which only carries model_provider). Latest occurrence wins, to match
    // the latest-rollout-authoritative rule used for cwd/cli_version/provider below.
    let mut model: Option<String> = None;

    // Merge per-file scans (files in mtime-asc order) to reproduce the old
    // single-pass result: sum task_complete; first head-window prompt wins;
    // latest non-empty last_ts and model win.
    for f in files_asc {
        total_size = total_size.saturating_add(f.size);
        let Some(scan) = scans.get(&cache_key(&f.path)) else {
            continue;
        };
        message_count = message_count.saturating_add(scan.task_complete_count);
        if scan.last_ts.is_some() {
            last_activity_ts = scan.last_ts.clone();
        }
        if first_prompt.is_none() {
            first_prompt = scan.first_user_message.clone();
        }
        if scan.last_model.is_some() {
            model = scan.last_model.clone();
        }
    }

    let imported_key = ("codex".to_string(), thread_id.to_string(), String::new());
    let (already_imported, existing_run_id) = match imported.get(&imported_key) {
        Some(rid) => (true, Some(rid.clone())),
        None => (false, None),
    };

    Some(CliSessionSummary {
        agent: "codex".to_string(),
        session_id: thread_id.to_string(),
        cwd: latest.meta.cwd.clone(),
        first_prompt: first_prompt.unwrap_or_default(),
        started_at: earliest.meta.timestamp.clone(),
        last_activity_at: last_activity_ts.unwrap_or_else(|| earliest.meta.timestamp.clone()),
        message_count,
        model: model.or_else(|| latest.meta.model_provider.clone()),
        cli_version: latest.meta.cli_version.clone(),
        file_size: total_size,
        file_path: latest.path.to_string_lossy().to_string(),
        rollout_paths: files_asc
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect(),
        has_subagents: false, // Codex CollabAgent is per-rollout; not surfaced in summary v1
        already_imported,
        existing_run_id,
    })
}

fn truncate_prompt(text: &str) -> String {
    if text.len() <= 200 {
        return text.to_string();
    }
    let mut end = 200;
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

// ── Find rollouts for a thread (full walk) ──────────────────────────

/// Walk `~/.codex/sessions/` fully and return all rollout files whose first
/// session_meta line matches `thread_id`. **Not subject to discovery's 500-file
/// truncation.**
fn find_rollouts_for_thread(thread_id: &str) -> Result<Vec<RolloutFileInfo>, String> {
    let root = codex_sessions_dir().ok_or("cannot determine home dir")?;
    find_rollouts_for_thread_in(&root, thread_id)
}

fn find_rollouts_for_thread_in(
    root: &Path,
    thread_id: &str,
) -> Result<Vec<RolloutFileInfo>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let files = collect_rollout_files(root);
    let matches: Vec<RolloutFileInfo> = files
        .par_iter()
        .filter_map(|(path, size, mtime, mtime_ns_val)| {
            let meta = read_first_session_meta(path).ok()?;
            if meta.thread_id != thread_id {
                return None;
            }
            Some(RolloutFileInfo {
                path: path.clone(),
                size: *size,
                mtime: *mtime,
                mtime_ns: *mtime_ns_val,
                meta,
            })
        })
        .collect();
    Ok(matches)
}

// ── Importer state ──────────────────────────────────────────────────

struct PendingCall {
    tool_use_id: String,
    tool_name: String,
}

struct CodexRolloutImporter {
    run_id: String,
    writer: Arc<EventWriter>,
    turn_counter: u32,
    saw_task_complete_in_turn: bool,
    pending_tool_calls: HashMap<String, PendingCall>,
    ended_call_ids: HashSet<String>,
    seen_message_keys: HashSet<String>,
    pending_token_count: Option<Value>,
    pending_model: Option<String>,
    last_seen_ts: Option<String>,
    events_imported: u64,
    events_skipped: u64,
    skipped_subtypes: HashMap<String, u64>,
    usage_incomplete: bool,
}

impl CodexRolloutImporter {
    fn new(run_id: String, writer: Arc<EventWriter>) -> Self {
        Self {
            run_id,
            writer,
            turn_counter: 0,
            saw_task_complete_in_turn: false,
            pending_tool_calls: HashMap::new(),
            ended_call_ids: HashSet::new(),
            seen_message_keys: HashSet::new(),
            pending_token_count: None,
            pending_model: None,
            last_seen_ts: None,
            events_imported: 0,
            events_skipped: 0,
            skipped_subtypes: HashMap::new(),
            usage_incomplete: false,
        }
    }

    fn scoped_tool_use_id(&self, call_id: &str) -> String {
        let run_short: String = self.run_id.chars().take(8).collect();
        format!(
            "codex-import-{}-{}-{}",
            run_short, self.turn_counter, call_id
        )
    }

    fn message_key(&self, role: &str, content: &str) -> String {
        format!("{}:{}:{}", self.turn_counter, role, sha256_short(content))
    }

    /// Build source_key including file basename hash to prevent cross-file collisions.
    fn build_source_key(
        file_basename_sha8: &str,
        ts: &str,
        outer: &str,
        inner: &str,
        raw_line: &str,
    ) -> String {
        format!(
            "v1:{}:{}:{}:{}:{}",
            file_basename_sha8,
            ts,
            outer,
            inner,
            sha256_short(raw_line)
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_event(
        &mut self,
        event: BusEvent,
        source_key: &str,
        ts: &str,
        source_file: &str,
        event_counts: &mut HashMap<String, usize>,
        index_writer: &mut BufWriter<File>,
        skip_set: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        let tag = bus_event_tag(&event);

        if !is_replayable(&event) {
            self.events_skipped += 1;
            *self.skipped_subtypes.entry(tag).or_insert(0) += 1;
            return Ok(());
        }

        let n = event_counts.entry(tag.clone()).or_insert(0);
        let ek = event_key(source_key, &tag, *n);
        *n += 1;

        if let Some(ss) = skip_set {
            if ss.contains(&ek) {
                return Ok(());
            }
        }

        let seq = self
            .writer
            .write_bus_event_with_ts(&self.run_id, &event, ts)?;
        writeln!(
            index_writer,
            "{}",
            json!({
                "source_key": ek,
                "imported_seq": seq,
                "source_file": source_file,
            })
        )
        .map_err(|e| format!("write index: {}", e))?;
        self.events_imported += 1;
        Ok(())
    }

    /// Emit a ToolEnd; if there's no matching ToolStart, synthesize one first.
    /// Always inserts call_id into `ended_call_ids` so later fallback paths
    /// (`function_call_output`) skip duplicates.
    fn emit_tool_end(
        &mut self,
        call_id: &str,
        fallback_tool_name: &str,
        fallback_input: Value,
        output: Value,
        status: &str,
    ) -> Vec<BusEvent> {
        if self.ended_call_ids.contains(call_id) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let (tool_use_id, tool_name) = match self.pending_tool_calls.get(call_id) {
            Some(pending) => (pending.tool_use_id.clone(), pending.tool_name.clone()),
            None => {
                // Orphan recovery: synthesize a ToolStart for the same line.
                let tool_use_id = self.scoped_tool_use_id(call_id);
                let tool_name = fallback_tool_name.to_string();
                out.push(BusEvent::ToolStart {
                    run_id: self.run_id.clone(),
                    tool_use_id: tool_use_id.clone(),
                    tool_name: tool_name.clone(),
                    input: fallback_input,
                    parent_tool_use_id: None,
                });
                self.pending_tool_calls.insert(
                    call_id.to_string(),
                    PendingCall {
                        tool_use_id: tool_use_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                );
                (tool_use_id, tool_name)
            }
        };
        out.push(BusEvent::ToolEnd {
            run_id: self.run_id.clone(),
            tool_use_id,
            tool_name,
            output,
            status: status.to_string(),
            duration_ms: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        });
        self.ended_call_ids.insert(call_id.to_string());
        out
    }

    fn flush_pending_token_count(&mut self) -> Option<BusEvent> {
        let tc = self.pending_token_count.take()?;
        // Codex v0.130: payload.info.last_token_usage.{input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens}
        // Treat JSON null info as absent (some token_count events have info:null) — otherwise
        // `get("info")?` would yield Some(Null) and produce an all-zero UsageUpdate.
        let info = tc.get("info").filter(|v| !v.is_null())?;
        let last = info.get("last_token_usage").unwrap_or(info);
        let input = last
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = last
            .get("cached_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = last
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let reasoning_out = last
            .get("reasoning_output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_total = output.saturating_add(reasoning_out);
        Some(BusEvent::UsageUpdate {
            run_id: self.run_id.clone(),
            input_tokens: input,
            output_tokens: output_total,
            cache_read_tokens: if cached > 0 { Some(cached) } else { None },
            cache_write_tokens: None,
            total_cost_usd: 0.0,
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
    }

    fn process_line(
        &mut self,
        raw_line: &str,
        source_file: &Path,
        index_writer: &mut BufWriter<File>,
        skip_set: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let Ok(json): std::result::Result<Value, _> = serde_json::from_str(trimmed) else {
            return Ok(());
        };

        let outer_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = json.get("payload");
        let inner_type = payload
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ts = json
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !ts.is_empty() {
            self.last_seen_ts = Some(ts.clone());
        }

        let file_basename = source_file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let file_sha8 = sha256_short(file_basename);
        let source_key = Self::build_source_key(&file_sha8, &ts, outer_type, inner_type, trimmed);
        let source_file_str = source_file.to_string_lossy().to_string();

        let candidates: Vec<BusEvent> = match outer_type {
            "session_meta" => Vec::new(),
            "turn_context" => {
                if let Some(p) = payload {
                    if let Some(m) = p.get("model").and_then(|v| v.as_str()) {
                        self.pending_model = Some(m.to_string());
                    }
                }
                Vec::new()
            }
            "event_msg" => self.handle_event_msg(payload, inner_type),
            "response_item" => self.handle_response_item(payload, inner_type),
            "compacted" => self.handle_top_level_compacted(payload),
            _ => {
                *self
                    .skipped_subtypes
                    .entry("codex_unknown_outer".to_string())
                    .or_insert(0) += 1;
                vec![BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: "codex_unknown".to_string(),
                    data: json.clone(),
                }]
            }
        };

        let mut event_counts: HashMap<String, usize> = HashMap::new();
        for event in candidates {
            self.record_event(
                event,
                &source_key,
                &ts,
                &source_file_str,
                &mut event_counts,
                index_writer,
                skip_set,
            )?;
        }
        Ok(())
    }

    /// Map a top-level `compacted` rollout record to BusEvents. Codex doesn't
    /// surface pre-compaction token counts, and the summary `message` field is
    /// left for Phase 2; v1 emits a bare divider matching Claude's
    /// `CompactBoundary` rendering.
    ///
    /// Pure-ish (takes `&self` for run_id only) — testable without touching disk.
    fn handle_top_level_compacted(&self, _payload: Option<&Value>) -> Vec<BusEvent> {
        vec![BusEvent::CompactBoundary {
            run_id: self.run_id.clone(),
            trigger: "codex_auto".to_string(),
            pre_tokens: None,
        }]
    }

    fn handle_event_msg(&mut self, payload: Option<&Value>, inner: &str) -> Vec<BusEvent> {
        let Some(p) = payload else {
            return Vec::new();
        };
        match inner {
            "task_started" => {
                self.turn_counter += 1;
                self.saw_task_complete_in_turn = false;
                self.pending_token_count = None;
                Vec::new()
            }
            "user_message" => {
                // Fallback turn increment if no preceding task_started (after task_complete).
                if self.saw_task_complete_in_turn {
                    self.turn_counter += 1;
                    self.saw_task_complete_in_turn = false;
                    self.pending_token_count = None;
                }
                let Some(text) = p.get("message").and_then(|v| v.as_str()) else {
                    return Vec::new();
                };
                let key = self.message_key("user", text);
                if self.seen_message_keys.contains(&key) {
                    return Vec::new();
                }
                self.seen_message_keys.insert(key);
                vec![BusEvent::UserMessage {
                    run_id: self.run_id.clone(),
                    text: text.to_string(),
                    uuid: None,
                    client_uuid: None,
                    attachments: Vec::new(),
                }]
            }
            "agent_message" => {
                let Some(text) = p.get("message").and_then(|v| v.as_str()) else {
                    return Vec::new();
                };
                let key = self.message_key("assistant", text);
                if self.seen_message_keys.contains(&key) {
                    return Vec::new();
                }
                self.seen_message_keys.insert(key);
                let model = self.pending_model.clone();
                vec![
                    BusEvent::MessageDelta {
                        run_id: self.run_id.clone(),
                        text: text.to_string(),
                        parent_tool_use_id: None,
                    },
                    BusEvent::MessageComplete {
                        run_id: self.run_id.clone(),
                        message_id: format!("codex-import-msg-{}", self.turn_counter),
                        text: text.to_string(),
                        parent_tool_use_id: None,
                        model,
                        stop_reason: None,
                        message_usage: None,
                    },
                ]
            }
            "agent_reasoning" => {
                let text = p
                    .get("text")
                    .and_then(|v| v.as_str())
                    .or_else(|| p.get("summary").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                vec![BusEvent::ThinkingDelta {
                    run_id: self.run_id.clone(),
                    text: text.to_string(),
                    parent_tool_use_id: None,
                }]
            }
            "exec_command_end" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                let command = p.get("command").cloned().unwrap_or(Value::Null);
                let stdout = p.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
                let stderr = p.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
                let exit_code = p.get("exit_code").and_then(|v| v.as_i64());
                let output = json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code,
                });
                let status = match exit_code {
                    Some(0) => "success",
                    Some(_) => "failure",
                    None => "unknown",
                };
                self.emit_tool_end(
                    call_id,
                    "Bash",
                    json!({ "command": command }),
                    output,
                    status,
                )
            }
            "patch_apply_end" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                let success = p.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                let changes = p.get("changes").cloned().unwrap_or(Value::Null);
                let stdout = p.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
                let stderr = p.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
                let status = if success { "success" } else { "failure" };
                self.emit_tool_end(
                    call_id,
                    "Edit",
                    json!({ "changes": changes }),
                    json!({ "success": success, "stdout": stdout, "stderr": stderr, "changes": changes }),
                    status,
                )
            }
            "mcp_tool_call_end" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                let result = p.get("result").cloned();
                let error = p.get("error").cloned();
                let status = if error.is_some() {
                    "failure"
                } else {
                    "success"
                };
                let output = json!({ "result": result, "error": error });
                self.emit_tool_end(call_id, "MCP", Value::Null, output, status)
            }
            "web_search_end" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                let query = p.get("query").cloned().unwrap_or(Value::Null);
                let action = p.get("action").cloned().unwrap_or(Value::Null);
                self.emit_tool_end(
                    call_id,
                    "WebSearch",
                    json!({ "query": query }),
                    json!({ "query": query, "action": action }),
                    "success",
                )
            }
            "token_count" => {
                self.pending_token_count = Some(p.clone());
                Vec::new()
            }
            "task_complete" => {
                self.saw_task_complete_in_turn = true;
                let usage_event = self.flush_pending_token_count();
                match usage_event {
                    Some(ev) => vec![ev],
                    None => {
                        self.usage_incomplete = true;
                        Vec::new()
                    }
                }
            }
            "turn_aborted" => vec![BusEvent::RunState {
                run_id: self.run_id.clone(),
                state: "stopped".to_string(),
                exit_code: None,
                error: p
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            }],
            "context_compacted" => vec![BusEvent::CompactBoundary {
                run_id: self.run_id.clone(),
                trigger: "codex_auto".to_string(),
                // Codex doesn't surface pre-compaction token count.
                pre_tokens: None,
            }],
            "entered_review_mode" | "exited_review_mode" | "item_completed" => {
                vec![BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: format!("codex_{}", inner),
                    data: p.clone(),
                }]
            }
            _ => {
                *self
                    .skipped_subtypes
                    .entry(format!("event_msg.{}", inner))
                    .or_insert(0) += 1;
                vec![BusEvent::Raw {
                    run_id: self.run_id.clone(),
                    source: format!("codex_event_msg_unknown_{}", inner),
                    data: p.clone(),
                }]
            }
        }
    }

    fn handle_response_item(&mut self, payload: Option<&Value>, inner: &str) -> Vec<BusEvent> {
        let Some(p) = payload else {
            return Vec::new();
        };
        match inner {
            "function_call" | "custom_tool_call" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                if self.pending_tool_calls.contains_key(call_id)
                    || self.ended_call_ids.contains(call_id)
                {
                    return Vec::new();
                }
                let raw_name = p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tool_name = map_tool_name(&raw_name);
                let arguments = if inner == "custom_tool_call" {
                    p.get("input").cloned().unwrap_or(Value::Null)
                } else {
                    // function_call.arguments is a JSON string
                    p.get("arguments")
                        .and_then(|v| v.as_str())
                        .map(|s| {
                            serde_json::from_str::<Value>(s)
                                .unwrap_or_else(|_| Value::String(s.to_string()))
                        })
                        .unwrap_or(Value::Null)
                };
                let tool_use_id = self.scoped_tool_use_id(call_id);
                self.pending_tool_calls.insert(
                    call_id.to_string(),
                    PendingCall {
                        tool_use_id: tool_use_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                );
                vec![BusEvent::ToolStart {
                    run_id: self.run_id.clone(),
                    tool_use_id,
                    tool_name,
                    input: arguments,
                    parent_tool_use_id: None,
                }]
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if call_id.is_empty() {
                    return Vec::new();
                }
                if self.ended_call_ids.contains(call_id) {
                    *self
                        .skipped_subtypes
                        .entry("function_call_output_duplicate".to_string())
                        .or_insert(0) += 1;
                    return Vec::new();
                }
                let output = p.get("output").cloned().unwrap_or(Value::Null);
                self.emit_tool_end(call_id, "function", Value::Null, output, "success")
            }
            "message" => {
                let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let content = extract_message_text(p);
                if role.is_empty() || content.is_empty() {
                    return Vec::new();
                }
                // User messages are emitted via event_msg/user_message (the clean prompt).
                // response_item user blocks ALSO carry injected context (AGENTS.md, environment
                // preamble) that would render as spurious user bubbles — skip them here, and
                // don't touch seen_message_keys so the event_msg path still emits the real prompt.
                if role == "user" {
                    return Vec::new();
                }
                let key = self.message_key(role, &content);
                if self.seen_message_keys.contains(&key) {
                    return Vec::new();
                }
                self.seen_message_keys.insert(key);
                match role {
                    "assistant" => vec![BusEvent::MessageComplete {
                        run_id: self.run_id.clone(),
                        message_id: format!("codex-import-msg-fallback-{}", self.turn_counter),
                        text: content,
                        parent_tool_use_id: None,
                        model: self.pending_model.clone(),
                        stop_reason: None,
                        message_usage: None,
                    }],
                    _ => Vec::new(),
                }
            }
            "reasoning" | "web_search_call" => Vec::new(), // covered by event_msg variants
            _ => {
                *self
                    .skipped_subtypes
                    .entry(format!("response_item.{}", inner))
                    .or_insert(0) += 1;
                Vec::new()
            }
        }
    }
}

fn map_tool_name(raw: &str) -> String {
    match raw {
        "shell" | "container.exec" | "local_shell" => "Bash",
        "apply_patch" => "Edit",
        "update_plan" => "TodoWrite",
        _ => raw,
    }
    .to_string()
}

fn extract_message_text(payload: &Value) -> String {
    let content = match payload.get("content") {
        Some(c) => c,
        None => return String::new(),
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        return out;
    }
    String::new()
}

fn bus_event_tag(event: &BusEvent) -> String {
    if let Ok(v) = serde_json::to_value(event) {
        if let Some(t) = v.get("type").and_then(|v| v.as_str()) {
            return t.to_string();
        }
    }
    "unknown".to_string()
}

// ── Import ──────────────────────────────────────────────────────────

pub fn import_session(
    thread_id: &str,
    _cwd_filter: &str,
    writer: Arc<EventWriter>,
) -> Result<ImportResult, String> {
    let start = std::time::Instant::now();
    log::debug!("[codex_sessions] import: thread_id={}", thread_id);

    let imported = build_imported_index();
    let dedup_key = ("codex".to_string(), thread_id.to_string(), String::new());
    if let Some(existing) = imported.get(&dedup_key) {
        return Err(format!(
            "thread {} already imported as run {}",
            thread_id, existing
        ));
    }

    let mut rollouts = find_rollouts_for_thread(thread_id)?;
    if rollouts.is_empty() {
        return Err(format!("no rollouts found for thread {}", thread_id));
    }
    rollouts.sort_by_key(|r| r.mtime);

    for r in &rollouts {
        validate_codex_path(&r.path)?;
    }

    let earliest_meta = &rollouts.first().unwrap().meta;
    let latest_meta = &rollouts.last().unwrap().meta;
    let actual_cwd = latest_meta.cwd.clone();

    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = super::run_dir(&run_id);
    super::ensure_dir(&run_dir).map_err(|e| format!("ensure_dir: {}", e))?;

    // First-pass scan to derive first_prompt. Codex rollouts always end in a
    // terminal state, so status is uniformly Stopped — rollout files don't carry
    // enough signal to distinguish error from cancellation.
    let mut first_prompt = String::new();
    'outer: for r in &rollouts {
        if let Ok(file) = File::open(&r.path) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(json): std::result::Result<Value, _> = serde_json::from_str(trimmed) else {
                    continue;
                };
                let outer = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let inner = json
                    .get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if outer == "event_msg" && inner == "user_message" {
                    if let Some(text) = json
                        .get("payload")
                        .and_then(|p| p.get("message"))
                        .and_then(|v| v.as_str())
                    {
                        first_prompt = truncate_prompt(text);
                        break 'outer;
                    }
                }
            }
        }
    }
    let status = RunStatus::Stopped;

    let meta = RunMeta {
        id: run_id.clone(),
        prompt: first_prompt,
        cwd: actual_cwd,
        agent: "codex".to_string(),
        auth_mode: "cli".to_string(),
        status,
        started_at: earliest_meta.timestamp.clone(),
        ended_at: rollouts.last().and_then(|r| {
            r.mtime.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            })
        }),
        exit_code: None,
        error_message: None,
        session_id: Some(thread_id.to_string()),
        result_subtype: None,
        model: latest_meta.model_provider.clone(),
        parent_run_id: None,
        name: None,
        remote_host_name: None,
        remote_cwd: None,
        remote_host_snapshot: None,
        platform_id: None,
        platform_base_url: None,
        source: Some(RunSource::CliImport),
        cli_import_watermark: None,
        cli_session_path: Some(rollouts.last().unwrap().path.to_string_lossy().to_string()),
        cli_usage_incomplete: None,
        deleted_at: None,
        no_session_persistence: false,
        execution_path: Some(ExecutionPath::PipeExec),
        conversation_ref: Some(ConversationRef::CodexThread(thread_id.to_string())),
        codex_process_seq: Some(0),
        codex_imported_rollouts: None,
    };

    let import_result =
        (|| -> Result<(CodexRolloutImporter, Vec<CodexImportedRollout>), String> {
            let mut importer = CodexRolloutImporter::new(run_id.clone(), writer.clone());
            let mut index_writer = BufWriter::new(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(import_index_path(&run_id))
                    .map_err(|e| format!("open index: {}", e))?,
            );
            let mut imported_rollouts = Vec::new();
            for r in &rollouts {
                let file = File::open(&r.path).map_err(|e| format!("open rollout: {}", e))?;
                for line in BufReader::new(file).lines() {
                    let line = line.map_err(|e| format!("read rollout: {}", e))?;
                    importer.process_line(&line, &r.path, &mut index_writer, None)?;
                }
                imported_rollouts.push(CodexImportedRollout {
                    path: r.path.to_string_lossy().to_string(),
                    size: r.size,
                    mtime_ns: r.mtime_ns.to_string(),
                    last_event_ts: importer.last_seen_ts.clone(),
                });
            }
            index_writer
                .flush()
                .map_err(|e| format!("flush index: {}", e))?;
            Ok((importer, imported_rollouts))
        })();

    let (importer, imported_rollouts) = match import_result {
        Ok(v) => v,
        Err(e) => {
            log::error!("[codex_sessions] import failed, cleaning up run_dir: {}", e);
            let _ = fs::remove_dir_all(&run_dir);
            return Err(e);
        }
    };

    let mut meta = meta;
    // Prefer the real model from turn_context (captured during import) over the
    // session_meta model_provider ("openai") set above. Latest turn_context wins.
    if let Some(m) = &importer.pending_model {
        meta.model = Some(m.clone());
    }
    meta.codex_imported_rollouts = Some(imported_rollouts);
    meta.cli_usage_incomplete = if importer.usage_incomplete {
        Some(true)
    } else {
        None
    };
    super::runs::save_meta(&meta)?;
    invalidate_imported_cache();

    log::debug!(
        "[codex_sessions] import: done in {:?}, events={}, skipped={}, usage_incomplete={}",
        start.elapsed(),
        importer.events_imported,
        importer.events_skipped,
        importer.usage_incomplete
    );

    Ok(ImportResult {
        run_id,
        session_id: thread_id.to_string(),
        events_imported: importer.events_imported,
        events_skipped: importer.events_skipped,
        usage_incomplete: importer.usage_incomplete,
        skipped_subtypes: importer.skipped_subtypes,
    })
}

// ── Sync ────────────────────────────────────────────────────────────

pub fn sync_session(run_id: &str, writer: Arc<EventWriter>) -> Result<SyncResult, String> {
    let start = std::time::Instant::now();
    let meta = super::runs::get_run(run_id).ok_or_else(|| format!("run {} not found", run_id))?;
    if meta.agent != "codex" {
        return Err(format!(
            "run {} is not a Codex run (agent={})",
            run_id, meta.agent
        ));
    }
    let thread_id = meta
        .session_id
        .as_ref()
        .ok_or_else(|| format!("run {} has no session_id", run_id))?;

    let already: HashSet<String> = meta
        .codex_imported_rollouts
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let prior_usage_incomplete = meta.cli_usage_incomplete.unwrap_or(false);

    let current = find_rollouts_for_thread(thread_id)?;
    let mut new_files: Vec<RolloutFileInfo> = current
        .into_iter()
        .filter(|r| !already.contains(&r.path.to_string_lossy().to_string()))
        .collect();
    new_files.sort_by_key(|r| r.mtime);

    if new_files.is_empty() {
        return Ok(SyncResult {
            new_events: 0,
            new_watermark: None,
            new_rollouts: Vec::new(),
            usage_incomplete: prior_usage_incomplete,
        });
    }

    for f in &new_files {
        validate_codex_path(&f.path)?;
    }

    // Crash-recovery dedup: if a prior sync wrote events + import-index entries
    // but failed to update RunMeta.codex_imported_rollouts, the rollout file is
    // still listed as "new" here. Load the existing import-index source_key set
    // so we skip events that were already written.
    let skip_set = load_import_skip_set(&import_index_path(run_id));
    log::debug!(
        "[codex_sessions] sync: loaded {} existing import-index keys",
        skip_set.len()
    );

    // Warmup turn_counter from existing events.jsonl so newly imported turns
    // don't collide with previously imported turn indices. Other state
    // (pending_tool_calls / seen_message_keys / ended_call_ids) is per-rollout
    // and naturally resets — Codex rollouts are self-contained between
    // task_started/task_complete.
    let warmup_turn = read_warmup_turn_count(run_id)?;

    let mut importer = CodexRolloutImporter::new(run_id.to_string(), writer);
    importer.turn_counter = warmup_turn;

    let mut index_writer = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(import_index_path(run_id))
            .map_err(|e| format!("open index: {}", e))?,
    );

    let mut imported_new = Vec::new();
    let events_before = importer.events_imported;

    for r in &new_files {
        let file = File::open(&r.path).map_err(|e| format!("open rollout: {}", e))?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| format!("read rollout: {}", e))?;
            importer.process_line(&line, &r.path, &mut index_writer, Some(&skip_set))?;
        }
        imported_new.push(CodexImportedRollout {
            path: r.path.to_string_lossy().to_string(),
            size: r.size,
            mtime_ns: r.mtime_ns.to_string(),
            last_event_ts: importer.last_seen_ts.clone(),
        });
    }
    index_writer
        .flush()
        .map_err(|e| format!("flush index: {}", e))?;

    // Update meta: extend codex_imported_rollouts.
    let imported_new_clone = imported_new.clone();
    let new_paths: Vec<String> = imported_new.iter().map(|r| r.path.clone()).collect();
    super::runs::with_meta(run_id, |m| {
        let mut all = m.codex_imported_rollouts.take().unwrap_or_default();
        all.extend(imported_new_clone.iter().cloned());
        m.codex_imported_rollouts = Some(all);
        if importer.usage_incomplete {
            m.cli_usage_incomplete = Some(true);
        }
        Ok(())
    })?;

    log::debug!(
        "[codex_sessions] sync: done in {:?}, events={}, new_files={}",
        start.elapsed(),
        importer.events_imported - events_before,
        new_paths.len()
    );

    // Preserve historical usage_incomplete: if either the prior import or this
    // sync left usage incomplete, the run as a whole is still incomplete.
    let combined_usage_incomplete = prior_usage_incomplete || importer.usage_incomplete;

    Ok(SyncResult {
        new_events: importer.events_imported - events_before,
        new_watermark: None,
        new_rollouts: new_paths,
        usage_incomplete: combined_usage_incomplete,
    })
}

/// Count distinct user_message events in events.jsonl as a turn-count warmup.
/// This is a coarse approximation — exact turn counting would require replaying
/// `task_started` events, but for collision avoidance "approximate count" is
/// sufficient because new rollouts always emit new task_started which advances
/// the counter further.
fn read_warmup_turn_count(run_id: &str) -> Result<u32, String> {
    let events_path = super::run_dir(run_id).join("events.jsonl");
    let Ok(content) = fs::read_to_string(&events_path) else {
        return Ok(0);
    };
    let mut n: u32 = 0;
    for line in content.lines() {
        if !line.contains("\"user_message\"") {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(line) {
            let event = val.get("event").unwrap_or(&val);
            if event.get("type").and_then(|v| v.as_str()) == Some("user_message") {
                n = n.saturating_add(1);
            }
        }
    }
    Ok(n)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A summary-scan cache path inside a test's own temp dir. Each test owns its
    /// own file, so there is zero shared mutable cache state across parallel tests
    /// (no process-global env var, no shared OnceLock).
    fn test_cache_path(tmp: &std::path::Path) -> std::path::PathBuf {
        tmp.join("summary-scan-cache.json")
    }

    fn writer() -> Arc<EventWriter> {
        Arc::new(EventWriter::new())
    }

    fn importer() -> CodexRolloutImporter {
        CodexRolloutImporter::new("test-run-abcdef12".to_string(), writer())
    }

    #[test]
    fn response_item_user_message_skipped() {
        let mut imp = importer();
        // role=user → skipped (real prompt comes via event_msg/user_message)
        let user = imp.handle_response_item(
            Some(&json!({"type": "message", "role": "user",
                         "content": [{"type": "input_text", "text": "# AGENTS.md ..."}]})),
            "message",
        );
        assert!(user.is_empty());
        // role=assistant → still emits MessageComplete
        let asst = imp.handle_response_item(
            Some(&json!({"type": "message", "role": "assistant",
                         "content": [{"type": "output_text", "text": "hi"}]})),
            "message",
        );
        assert_eq!(asst.len(), 1);
        assert!(matches!(asst[0], BusEvent::MessageComplete { .. }));
    }

    #[test]
    fn token_count_null_info_yields_no_usage() {
        let mut imp = importer();
        imp.pending_token_count = Some(json!({ "info": null }));
        assert!(imp.flush_pending_token_count().is_none());
    }

    #[test]
    fn importer_captures_turn_context_model() {
        let mut imp = importer();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut iw = BufWriter::new(tmp.reopen().unwrap());
        let line = json!({"type": "turn_context", "payload": {"model": "gpt-5.5"}}).to_string();
        imp.process_line(
            &line,
            std::path::Path::new("/tmp/rollout-x.jsonl"),
            &mut iw,
            None,
        )
        .unwrap();
        assert_eq!(imp.pending_model.as_deref(), Some("gpt-5.5"));
    }

    // ── Per-variant event_msg mapping ──

    #[test]
    fn user_message_increments_turn_and_emits() {
        let mut imp = importer();
        // Simulate task_started first (normal flow)
        let _ = imp.handle_event_msg(Some(&json!({"type": "task_started"})), "task_started");
        assert_eq!(imp.turn_counter, 1);
        let events = imp.handle_event_msg(
            Some(&json!({"type": "user_message", "message": "hi"})),
            "user_message",
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BusEvent::UserMessage { .. }));
    }

    #[test]
    fn user_message_after_task_complete_fallback_increments_turn() {
        let mut imp = importer();
        let _ = imp.handle_event_msg(Some(&json!({"type": "task_started"})), "task_started");
        // Emit task_complete without a token_count → saw_task_complete_in_turn = true
        let _ = imp.handle_event_msg(Some(&json!({"type": "task_complete"})), "task_complete");
        assert_eq!(imp.turn_counter, 1);
        assert!(imp.saw_task_complete_in_turn);
        // Next user_message without intervening task_started → fallback bumps turn
        let _ = imp.handle_event_msg(
            Some(&json!({"type": "user_message", "message": "next"})),
            "user_message",
        );
        assert_eq!(imp.turn_counter, 2);
    }

    #[test]
    fn agent_message_emits_delta_and_complete() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let events = imp.handle_event_msg(
            Some(&json!({"type": "agent_message", "message": "ok"})),
            "agent_message",
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], BusEvent::MessageDelta { .. }));
        assert!(matches!(events[1], BusEvent::MessageComplete { .. }));
    }

    #[test]
    fn agent_reasoning_emits_thinking_delta() {
        let mut imp = importer();
        let events = imp.handle_event_msg(
            Some(&json!({"type": "agent_reasoning", "text": "thinking..."})),
            "agent_reasoning",
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BusEvent::ThinkingDelta { .. }));
    }

    #[test]
    fn event_msg_context_compacted_emits_compact_boundary() {
        let mut imp = importer();
        let events = imp.handle_event_msg(
            Some(&json!({"type": "context_compacted"})),
            "context_compacted",
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::CompactBoundary {
                trigger,
                pre_tokens,
                ..
            } => {
                assert_eq!(trigger, "codex_auto");
                assert!(pre_tokens.is_none());
            }
            other => panic!("expected CompactBoundary, got {:?}", other),
        }
    }

    #[test]
    fn top_level_compacted_emits_compact_boundary() {
        // Top-level rollout `compacted` records carry summary text in `message`,
        // but v1 ignores it — assert the divider fires regardless.
        let imp = importer();
        let payload = json!({
            "message": "Summary of older turns…",
            "replacement_history": []
        });
        let events = imp.handle_top_level_compacted(Some(&payload));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BusEvent::CompactBoundary { .. }));
    }

    #[test]
    fn top_level_compacted_without_payload_still_emits_divider() {
        let imp = importer();
        let events = imp.handle_top_level_compacted(None);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BusEvent::CompactBoundary { .. }));
    }

    #[test]
    fn turn_aborted_emits_run_state_stopped() {
        let mut imp = importer();
        let events = imp.handle_event_msg(
            Some(&json!({"type": "turn_aborted", "reason": "user_cancel"})),
            "turn_aborted",
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::RunState { state, error, .. } => {
                assert_eq!(state, "stopped");
                assert_eq!(error.as_deref(), Some("user_cancel"));
            }
            _ => panic!("expected RunState"),
        }
    }

    // ── Tool call pairing ──

    #[test]
    fn function_call_then_exec_command_end_pairs() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let starts = imp.handle_response_item(
            Some(&json!({
                "type": "function_call",
                "name": "shell",
                "call_id": "c1",
                "arguments": "{\"command\":[\"echo\",\"hi\"]}"
            })),
            "function_call",
        );
        assert_eq!(starts.len(), 1);
        match &starts[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "Bash"),
            _ => panic!("expected ToolStart"),
        }

        let ends = imp.handle_event_msg(
            Some(&json!({
                "type": "exec_command_end",
                "call_id": "c1",
                "exit_code": 0,
                "stdout": "hi\n",
                "stderr": ""
            })),
            "exec_command_end",
        );
        assert_eq!(ends.len(), 1);
        match &ends[0] {
            BusEvent::ToolEnd {
                status, tool_name, ..
            } => {
                assert_eq!(status, "success");
                assert_eq!(tool_name, "Bash");
            }
            _ => panic!("expected ToolEnd"),
        }
        assert!(imp.ended_call_ids.contains("c1"));
    }

    #[test]
    fn function_call_output_after_exec_end_is_skipped() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let _starts = imp.handle_response_item(
            Some(&json!({"type": "function_call", "name": "shell", "call_id": "c1", "arguments": "{}"})),
            "function_call",
        );
        let _ends = imp.handle_event_msg(
            Some(&json!({"type": "exec_command_end", "call_id": "c1", "exit_code": 0, "stdout": "", "stderr": ""})),
            "exec_command_end",
        );
        // Now the late function_call_output should be skipped (already ended)
        let late = imp.handle_response_item(
            Some(&json!({"type": "function_call_output", "call_id": "c1", "output": "ignored"})),
            "function_call_output",
        );
        assert!(late.is_empty());
        assert!(imp
            .skipped_subtypes
            .contains_key("function_call_output_duplicate"));
    }

    #[test]
    fn web_search_end_orphan_recovery_synthesizes_start() {
        let mut imp = importer();
        imp.turn_counter = 1;
        // No prior function_call — emit orphan web_search_end
        let events = imp.handle_event_msg(
            Some(&json!({
                "type": "web_search_end",
                "call_id": "ws-1",
                "query": "rust async",
                "action": {"type":"search","query":"rust async"}
            })),
            "web_search_end",
        );
        assert_eq!(events.len(), 2, "expected synthesized ToolStart + ToolEnd");
        match &events[0] {
            BusEvent::ToolStart { tool_name, .. } => assert_eq!(tool_name, "WebSearch"),
            _ => panic!("expected ToolStart"),
        }
        match &events[1] {
            BusEvent::ToolEnd { tool_name, .. } => assert_eq!(tool_name, "WebSearch"),
            _ => panic!("expected ToolEnd"),
        }
        assert!(imp.ended_call_ids.contains("ws-1"));
    }

    // ── Message dedup ──

    #[test]
    fn response_item_message_dedup_same_turn() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let _ = imp.handle_event_msg(
            Some(&json!({"type": "agent_message", "message": "ok"})),
            "agent_message",
        );
        // Same content same turn from response_item.message → skipped
        let late = imp.handle_response_item(
            Some(&json!({"type": "message", "role": "assistant", "content": "ok"})),
            "message",
        );
        assert!(late.is_empty(), "expected dedup");
    }

    #[test]
    fn response_item_message_kept_cross_turn() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let _ = imp.handle_event_msg(
            Some(&json!({"type": "agent_message", "message": "ok"})),
            "agent_message",
        );
        // Advance turn
        imp.turn_counter = 2;
        // Same content but different turn → must NOT be deduped
        let events = imp.handle_response_item(
            Some(&json!({"type": "message", "role": "assistant", "content": "ok"})),
            "message",
        );
        assert_eq!(events.len(), 1, "cross-turn same content should be kept");
    }

    // ── Token count / usage ──

    #[test]
    fn token_count_flushed_on_task_complete_with_reasoning_merged() {
        let mut imp = importer();
        let _ = imp.handle_event_msg(Some(&json!({"type": "task_started"})), "task_started");
        let _ = imp.handle_event_msg(
            Some(&json!({
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 20,
                        "output_tokens": 50,
                        "reasoning_output_tokens": 30
                    }
                }
            })),
            "token_count",
        );
        let events = imp.handle_event_msg(Some(&json!({"type": "task_complete"})), "task_complete");
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 80, "reasoning_output merged into output");
                assert_eq!(*cache_read_tokens, Some(20));
            }
            _ => panic!("expected UsageUpdate"),
        }
    }

    #[test]
    fn task_complete_without_token_count_marks_usage_incomplete() {
        let mut imp = importer();
        let _ = imp.handle_event_msg(Some(&json!({"type": "task_started"})), "task_started");
        let events = imp.handle_event_msg(Some(&json!({"type": "task_complete"})), "task_complete");
        assert!(events.is_empty());
        assert!(imp.usage_incomplete);
    }

    // ── Source/event key construction ──

    #[test]
    fn source_key_includes_file_hash() {
        let sk1 = CodexRolloutImporter::build_source_key(
            "aaaaaaaa",
            "ts1",
            "event_msg",
            "user_message",
            "line",
        );
        let sk2 = CodexRolloutImporter::build_source_key(
            "bbbbbbbb",
            "ts1",
            "event_msg",
            "user_message",
            "line",
        );
        assert_ne!(sk1, sk2, "file hash should make keys distinct across files");
    }

    #[test]
    fn event_key_distinguishes_tool_start_and_tool_end() {
        let sk = "v1:abc:ts:event_msg:web_search_end:hash";
        let start_key = event_key(sk, "tool_start", 0);
        let end_key = event_key(sk, "tool_end", 0);
        assert_ne!(start_key, end_key);
    }

    // ── Custom tool call ──

    #[test]
    fn custom_tool_call_paired_with_custom_output() {
        let mut imp = importer();
        imp.turn_counter = 1;
        let starts = imp.handle_response_item(
            Some(&json!({
                "type": "custom_tool_call",
                "name": "weather",
                "call_id": "ct-1",
                "input": "Paris"
            })),
            "custom_tool_call",
        );
        assert_eq!(starts.len(), 1);
        let ends = imp.handle_response_item(
            Some(&json!({
                "type": "custom_tool_call_output",
                "call_id": "ct-1",
                "output": "Cloudy 18C"
            })),
            "custom_tool_call_output",
        );
        assert_eq!(ends.len(), 1);
    }

    // ── Tool name mapping ──

    #[test]
    fn tool_name_mapping() {
        assert_eq!(map_tool_name("shell"), "Bash");
        assert_eq!(map_tool_name("apply_patch"), "Edit");
        assert_eq!(map_tool_name("update_plan"), "TodoWrite");
        assert_eq!(map_tool_name("container.exec"), "Bash");
        assert_eq!(map_tool_name("custom_thing"), "custom_thing");
    }

    // ── Reasoning text extraction ──

    #[test]
    fn extract_message_text_handles_string_and_array() {
        assert_eq!(extract_message_text(&json!({"content": "hello"})), "hello");
        assert_eq!(
            extract_message_text(&json!({"content": [{"text": "a"}, {"text": "b"}]})),
            "a\nb"
        );
        assert_eq!(extract_message_text(&json!({"content": null})), "");
    }

    // ── Path validation ──

    #[test]
    fn validate_codex_path_rejects_non_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("rollout.txt");
        std::fs::write(&bad, "x").unwrap();
        assert!(validate_codex_path(&bad).is_err());
    }

    // ── File-system level tests (discovery + full-walk) ──

    /// Write a minimal rollout-*.jsonl under `root/YYYY/MM/DD/`.
    /// `extra_lines` may be empty; the session_meta line is always added first.
    fn write_rollout(
        root: &std::path::Path,
        year: &str,
        month: &str,
        day: &str,
        filename: &str,
        thread_id: &str,
        cwd: &str,
        extra_lines: &[&str],
    ) -> std::path::PathBuf {
        let dir = root.join(year).join(month).join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(filename);
        let mut content = format!(
            "{{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{}\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"{}\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.130.0\"}}}}\n",
            thread_id, cwd
        );
        for line in extra_lines {
            content.push_str(line);
            content.push('\n');
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn empty_imported() -> crate::storage::cli_sessions::ImportedIndex {
        std::collections::HashMap::new()
    }

    #[test]
    fn discover_empty_root_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // Don't create the sessions/ subdir at all — fully nonexistent root path
        let nonexistent = tmp.path().join("does-not-exist");
        let result = discover_sessions_in_root(
            &nonexistent,
            "/",
            &empty_imported(),
            &test_cache_path(tmp.path()),
        )
        .unwrap();
        assert_eq!(result.sessions.len(), 0);
        assert!(!result.truncated);
    }

    #[test]
    fn discover_groups_two_files_into_one_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Same thread_id, two files (simulating original + resume)
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-2026-02-10T00-00-00-abc.jsonl",
            "thread-1",
            "/tmp/proj",
            &[
                r#"{"timestamp":"2026-02-10T00:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );
        write_rollout(
            root,
            "2026",
            "02",
            "11",
            "rollout-2026-02-11T00-00-00-def.jsonl",
            "thread-1",
            "/tmp/proj",
            &[
                r#"{"timestamp":"2026-02-11T00:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );
        // Different thread, one file
        write_rollout(
            root,
            "2026",
            "02",
            "12",
            "rollout-2026-02-12T00-00-00-ghi.jsonl",
            "thread-2",
            "/tmp/proj",
            &[
                r#"{"timestamp":"2026-02-12T00:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );

        let result =
            discover_sessions_in_root(root, "/", &empty_imported(), &test_cache_path(root))
                .unwrap();
        assert_eq!(result.sessions.len(), 2, "two distinct threads");
        let t1 = result
            .sessions
            .iter()
            .find(|s| s.session_id == "thread-1")
            .unwrap();
        assert_eq!(t1.rollout_paths.len(), 2, "thread-1 has 2 rollouts");
        assert_eq!(t1.message_count, 2, "two task_complete events accumulated");
        let t2 = result
            .sessions
            .iter()
            .find(|s| s.session_id == "thread-2")
            .unwrap();
        assert_eq!(t2.rollout_paths.len(), 1);
    }

    #[test]
    fn discover_summary_model_latest_turn_context_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two turn_context lines with different models — the latest must win.
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-2026-02-10T00-00-00-mdl.jsonl",
            "thread-model",
            "/tmp/proj",
            &[
                r#"{"timestamp":"2026-02-10T00:01:00Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
                r#"{"timestamp":"2026-02-10T00:02:00Z","type":"turn_context","payload":{"model":"gpt-6"}}"#,
                r#"{"timestamp":"2026-02-10T00:03:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );
        let result =
            discover_sessions_in_root(root, "/", &empty_imported(), &test_cache_path(root))
                .unwrap();
        let s = result
            .sessions
            .iter()
            .find(|s| s.session_id == "thread-model")
            .unwrap();
        assert_eq!(s.model.as_deref(), Some("gpt-6"));
    }

    #[test]
    fn discover_summary_falls_back_to_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No turn_context → fall back to session_meta.model_provider. write_rollout's
        // session_meta omits model_provider, so write a custom rollout inline.
        let dir = root.join("2026").join("02").join("10");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-02-10T00-00-00-prov.jsonl");
        let content = concat!(
            r#"{"timestamp":"2026-02-10T00:00:00Z","type":"session_meta","payload":{"id":"thread-prov","timestamp":"2026-02-10T00:00:00Z","cwd":"/tmp/proj","model_provider":"openai"}}"#,
            "\n",
            r#"{"timestamp":"2026-02-10T00:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            "\n",
        );
        std::fs::write(&path, content).unwrap();
        let result =
            discover_sessions_in_root(root, "/", &empty_imported(), &test_cache_path(root))
                .unwrap();
        let s = result
            .sessions
            .iter()
            .find(|s| s.session_id == "thread-prov")
            .unwrap();
        assert_eq!(s.model.as_deref(), Some("openai"));
    }

    #[test]
    fn discover_truncates_to_500_but_full_walk_finds_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Write 510 files: 5 belong to "old-thread" (will be the oldest mtime,
        // pushed out of the discovery window). Remaining 505 are unique threads.
        // We write old-thread first so its mtime is earliest.
        for i in 0..5 {
            write_rollout(
                root,
                "2026",
                "01",
                "01",
                &format!("rollout-old-{:03}.jsonl", i),
                "old-thread",
                "/tmp/old",
                &[],
            );
            // Sleep to differentiate mtimes (filesystem coarseness varies).
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for i in 0..505 {
            write_rollout(
                root,
                "2026",
                "06",
                "01",
                &format!("rollout-new-{:03}.jsonl", i),
                &format!("thread-{}", i),
                "/tmp/new",
                &[],
            );
        }

        // Discovery truncates to 500 (mtime desc keeps the 505 "new" files;
        // first 500 of them are kept; "old" files are evicted).
        let result =
            discover_sessions_in_root(root, "/", &empty_imported(), &test_cache_path(root))
                .unwrap();
        assert!(result.truncated);
        assert_eq!(result.total, 510, "truncated total = candidate file count");
        assert!(
            !result.sessions.iter().any(|s| s.session_id == "old-thread"),
            "old-thread evicted from discovery window"
        );

        // Full walk still finds all 5 old rollouts — this is the hard contract.
        let old_rollouts = find_rollouts_for_thread_in(root, "old-thread").unwrap();
        assert_eq!(
            old_rollouts.len(),
            5,
            "import-time full walk finds rollouts beyond discovery truncate"
        );
    }

    #[test]
    fn discover_filters_by_target_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-a.jsonl",
            "thread-a",
            "/proj-a",
            &[],
        );
        write_rollout(
            root,
            "2026",
            "02",
            "11",
            "rollout-b.jsonl",
            "thread-b",
            "/proj-b",
            &[],
        );
        let result =
            discover_sessions_in_root(root, "/proj-a", &empty_imported(), &test_cache_path(root))
                .unwrap();
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].session_id, "thread-a");
    }

    #[test]
    fn discover_cross_cwd_same_thread_uses_latest_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Same thread resumed in two different cwds.
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-a.jsonl",
            "thread-x",
            "/proj-old",
            &[],
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        write_rollout(
            root,
            "2026",
            "02",
            "11",
            "rollout-b.jsonl",
            "thread-x",
            "/proj-new",
            &[],
        );
        let result =
            discover_sessions_in_root(root, "/", &empty_imported(), &test_cache_path(root))
                .unwrap();
        assert_eq!(result.sessions.len(), 1, "single thread despite two cwds");
        assert_eq!(
            result.sessions[0].cwd, "/proj-new",
            "summary.cwd takes the latest rollout's cwd"
        );
        assert_eq!(result.sessions[0].rollout_paths.len(), 2);
    }

    #[test]
    fn find_rollouts_for_thread_ignores_other_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-a.jsonl",
            "match",
            "/p",
            &[],
        );
        write_rollout(
            root,
            "2026",
            "02",
            "11",
            "rollout-b.jsonl",
            "match",
            "/p",
            &[],
        );
        write_rollout(
            root,
            "2026",
            "02",
            "12",
            "rollout-c.jsonl",
            "other",
            "/p",
            &[],
        );
        let matches = find_rollouts_for_thread_in(root, "match").unwrap();
        assert_eq!(matches.len(), 2);
        let no_match = find_rollouts_for_thread_in(root, "absent").unwrap();
        assert_eq!(no_match.len(), 0);
    }

    // ── Already-imported dedup index ──

    // ── Sync crash-recovery: skip_set prevents duplicate event writes ──

    #[test]
    fn skip_set_blocks_duplicate_event_record() {
        // Scenario: a prior sync wrote an event + index entry but crashed before
        // updating RunMeta.codex_imported_rollouts. The next sync reloads the
        // import-index as a skip_set; re-processing the same rollout line must
        // NOT increment events_imported and must NOT touch the EventWriter.
        let mut imp = importer();
        let source_key = "v1:filehash:ts:event_msg:user_message:linehash";
        let expected_ek = event_key(source_key, "user_message", 0);

        let mut skip = std::collections::HashSet::new();
        skip.insert(expected_ek.clone());

        let event = BusEvent::UserMessage {
            run_id: imp.run_id.clone(),
            text: "hi".to_string(),
            uuid: None,
            client_uuid: None,
            attachments: Vec::new(),
        };

        // Use a discardable temp index_writer; the skip check returns before any write.
        let tmp = tempfile::tempdir().unwrap();
        let idx_path = tmp.path().join("index.jsonl");
        let mut index_writer = std::io::BufWriter::new(std::fs::File::create(&idx_path).unwrap());
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        imp.record_event(
            event,
            source_key,
            "ts",
            "/fake/rollout.jsonl",
            &mut counts,
            &mut index_writer,
            Some(&skip),
        )
        .unwrap();

        assert_eq!(
            imp.events_imported, 0,
            "event present in skip_set must be dropped"
        );
        index_writer.into_inner().unwrap().sync_all().unwrap();
        let idx_content = std::fs::read_to_string(&idx_path).unwrap();
        assert!(
            idx_content.is_empty(),
            "no index entry written for skipped event"
        );
    }

    #[test]
    fn load_import_skip_set_round_trip_through_process_line() {
        // End-to-end: process_line a user_message → import-index records its
        // source_key. Reload skip_set from that index, re-process the same
        // line → no new event is recorded.
        use std::io::Write;

        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#;
        let path = std::path::PathBuf::from("/fake/sessions/2026/01/01/rollout-test.jsonl");

        // First, derive the expected source_key + event_key without running
        // process_line (avoids EventWriter disk writes during test setup).
        let file_sha8 = sha256_short("rollout-test.jsonl");
        let source_key = CodexRolloutImporter::build_source_key(
            &file_sha8,
            "2026-01-01T00:00:00Z",
            "event_msg",
            "user_message",
            line,
        );
        let expected_ek = event_key(&source_key, "user_message", 0);

        // Seed an import-index file with that key.
        let tmp = tempfile::tempdir().unwrap();
        let idx_path = tmp.path().join("import-index.jsonl");
        {
            let mut f = std::fs::File::create(&idx_path).unwrap();
            writeln!(
                f,
                r#"{{"source_key":"{}","imported_seq":1,"source_file":"/fake/rollout-test.jsonl"}}"#,
                expected_ek
            )
            .unwrap();
        }

        let skip_set = load_import_skip_set(&idx_path);
        assert_eq!(skip_set.len(), 1);
        assert!(skip_set.contains(&expected_ek));

        // Now process the line; the importer should consult skip_set and drop the event.
        let mut imp = importer();
        let mut idx_writer = std::io::BufWriter::new(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&idx_path)
                .unwrap(),
        );
        imp.process_line(line, &path, &mut idx_writer, Some(&skip_set))
            .unwrap();
        assert_eq!(
            imp.events_imported, 0,
            "re-processing a known line must not write events"
        );
    }

    #[test]
    fn discover_marks_already_imported_when_index_has_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_rollout(
            root,
            "2026",
            "02",
            "10",
            "rollout-a.jsonl",
            "thread-z",
            "/p",
            &[],
        );

        let mut imported: crate::storage::cli_sessions::ImportedIndex =
            std::collections::HashMap::new();
        // Codex dedup key: (codex, thread_id, "")
        imported.insert(
            ("codex".to_string(), "thread-z".to_string(), String::new()),
            "run-existing".to_string(),
        );

        let result =
            discover_sessions_in_root(root, "/", &imported, &test_cache_path(root)).unwrap();
        assert_eq!(result.sessions.len(), 1);
        assert!(result.sessions[0].already_imported);
        assert_eq!(
            result.sessions[0].existing_run_id.as_deref(),
            Some("run-existing")
        );
    }

    // ── Summary scan cache: hit + invalidation ──

    /// Build a `RolloutFileInfo` from an on-disk path, reading current metadata.
    fn rollout_info(path: &std::path::Path) -> RolloutFileInfo {
        let md = std::fs::metadata(path).unwrap();
        let mtime = md.modified().unwrap();
        RolloutFileInfo {
            path: path.to_path_buf(),
            size: md.len(),
            mtime,
            mtime_ns: mtime_ns(&md),
            meta: RolloutMeta::default(),
        }
    }

    #[test]
    fn summary_scan_cache_serves_hit_and_invalidates_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        // This test's own cache file — no shared global state with other tests.
        let cache = test_cache_path(tmp.path());
        let path = write_rollout(
            tmp.path(),
            "2026",
            "03",
            "01",
            "rollout-cache.jsonl",
            "thread-cache",
            "/p",
            &[
                r#"{"timestamp":"2026-03-01T00:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );

        // First pass: scans the file, count = 1, and populates the disk cache.
        let info = rollout_info(&path);
        let first = scan_summary_files(std::slice::from_ref(&info), &cache);
        assert_eq!(first[&cache_key(&path)].task_complete_count, 1);

        // Append a second task_complete on disk, but call with the STALE
        // (mtime_ns,size). The cache key still matches → cached scan reused,
        // so the new line is NOT seen (count stays 1). Proves a cache hit.
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str(
            "{\"timestamp\":\"2026-03-01T00:02:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n",
        );
        std::fs::write(&path, &content).unwrap();
        let stale_hit = scan_summary_files(std::slice::from_ref(&info), &cache);
        assert_eq!(
            stale_hit[&cache_key(&path)].task_complete_count,
            1,
            "unchanged (mtime,size) key must reuse cached scan"
        );

        // Now pass the real updated metadata → key differs → re-scan picks up
        // both task_complete lines. Proves invalidation on file change.
        let updated = rollout_info(&path);
        assert!(
            updated.size != info.size || updated.mtime_ns != info.mtime_ns,
            "appended bytes should change size/mtime"
        );
        let refreshed = scan_summary_files(std::slice::from_ref(&updated), &cache);
        assert_eq!(
            refreshed[&cache_key(&path)].task_complete_count,
            2,
            "changed file must be re-scanned"
        );
    }
}

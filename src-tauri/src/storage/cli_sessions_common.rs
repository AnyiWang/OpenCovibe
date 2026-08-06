//! Shared types and helpers for CLI session import (Claude + Codex).
//!
//! `cli_sessions.rs` (Claude) and `codex_sessions.rs` (Codex) both import CLI
//! transcripts into OpenCovibe runs. Types and lightweight helpers live here so
//! the two implementations stay in sync.

use crate::models::ImportWatermark;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Shared types ────────────────────────────────────────────────────

/// CLI session summary (discovery phase output).
///
/// `agent` distinguishes Claude vs Codex sources. `rolloutPaths` is Codex-only —
/// the list of all rollout files belonging to a thread (Claude is always single-file).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionSummary {
    pub agent: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollout_paths: Vec<String>,
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

/// Discovery result with truncation metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    pub sessions: Vec<CliSessionSummary>,
    pub total: usize,
    pub truncated: bool,
}

/// Incremental sync result.
///
/// `new_watermark` is Claude-only (offset-based append). Codex returns `None`
/// and reports newly imported rollout files via `new_rollouts` instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub new_events: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_watermark: Option<ImportWatermark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_rollouts: Vec<String>,
    pub usage_incomplete: bool,
}

// ── Shared helpers ──────────────────────────────────────────────────

/// Encode cwd for Claude CLI directory naming: '/' and '\' → '-'.
pub fn encode_cwd(cwd: &str) -> String {
    cwd.replace(['/', '\\'], "-")
}

/// SHA-256 hash of a string, returning first 12 hex chars.
pub fn sha256_short(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    result[..6].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate an event-level key from line_key + event type + index.
pub fn event_key(lk: &str, event_type: &str, n: usize) -> String {
    format!("v1:{}#{}#{}", lk, event_type, n)
}

// ── Generic per-file scan cache ─────────────────────────────────────

/// Disk-backed scan cache keyed by file path + (mtime_ns, size). A cached entry
/// is reused only when both the modification time and size match, so any edit to
/// a rollout file invalidates its entry. `T` is the per-file scan result.
///
/// Shared by `codex_usage` (token aggregation) and `codex_sessions` (discovery
/// summaries) so unchanged rollout files are never re-parsed.
#[derive(Serialize, Deserialize)]
pub struct DiskScanCache<T> {
    pub version: u32,
    /// path → cached scan of that file.
    pub manifest: HashMap<String, CachedFile<T>>,
}

#[derive(Serialize, Deserialize)]
pub struct CachedFile<T> {
    pub mtime_ns: u128,
    pub size: u64,
    pub data: T,
}

impl<T> DiskScanCache<T>
where
    T: Serialize + DeserializeOwned,
{
    /// Read and validate a cache file; `None` on missing/corrupt/version mismatch.
    pub fn read(path: &Path, version: u32) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let cache: Self = serde_json::from_str(&raw).ok()?;
        if cache.version != version {
            return None;
        }
        Some(cache)
    }

    /// Atomically write the cache (write tmp + rename). Best-effort: errors ignored.
    pub fn write(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = crate::storage::ensure_dir(parent);
        }
        let Ok(json) = serde_json::to_string(self) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Take the cached scan for `key` if its (mtime_ns, size) still match, removing
    /// it from the old manifest. `None` means the file is new or changed and must
    /// be re-scanned.
    pub fn take_if_fresh(&mut self, key: &str, mtime_ns: u128, size: u64) -> Option<T> {
        match self.manifest.remove(key) {
            Some(cf) if cf.mtime_ns == mtime_ns && cf.size == size => Some(cf.data),
            _ => None,
        }
    }
}

/// Convenience: a fresh empty manifest of the given version.
pub fn empty_scan_cache<T>(version: u32) -> DiskScanCache<T> {
    DiskScanCache {
        version,
        manifest: HashMap::new(),
    }
}

/// Stable string key for a path, used as the cache manifest key.
pub fn cache_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Where per-scan caches live (under the app data dir).
pub fn scan_cache_path(filename: &str) -> PathBuf {
    crate::storage::data_dir().join(filename)
}

/// Load `source_key` set from an import-index file for crash-recovery dedup.
/// Returns empty set if the file does not exist or is unreadable.
pub fn load_import_skip_set(index_path: &std::path::Path) -> std::collections::HashSet<String> {
    let mut skip_set = std::collections::HashSet::new();
    let Ok(content) = std::fs::read_to_string(index_path) else {
        return skip_set;
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(key) = val.get("source_key").and_then(|v| v.as_str()) {
                skip_set.insert(key.to_string());
            }
        }
    }
    skip_set
}

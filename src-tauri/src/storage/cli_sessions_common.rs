//! Shared types and helpers for CLI session import (Claude + Codex).
//!
//! `cli_sessions.rs` (Claude) and `codex_sessions.rs` (Codex) both import CLI
//! transcripts into OpenCovibe runs. Types and lightweight helpers live here so
//! the two implementations stay in sync.

use crate::models::ImportWatermark;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

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

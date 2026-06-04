//! Read Codex global usage by scanning `~/.codex/sessions` rollout JSONL files.
//!
//! Parallel to `claude_usage.rs` (which scans `~/.claude/projects`). Codex sessions are
//! stored as `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. Per-turn token usage comes
//! from `token_count` events; the model from `turn_context`. Cost is estimated via the
//! `pricing` module — Codex (unlike Claude Code) does not emit cost itself.
//!
//! Counting rule: a `token_count` event carries both `info.last_token_usage` (this turn's
//! delta) and `info.total_token_usage` (cumulative). We sum `last_token_usage` ONLY —
//! summing the cumulative figure too would double-count.
//!
//! Token→pricing mapping mirrors the App-scope path (`pipe_parser::map_turn_completed`):
//! input = input_tokens (raw, includes cached), cache_read = cached_input_tokens,
//! output = output_tokens + reasoning_output_tokens, cache_write = 0.

use crate::models::{DailyAggregate, ModelAggregate, UsageOverview};
use crate::pricing;
use crate::storage::cli_sessions_common::{cache_key, scan_cache_path, CachedFile, DiskScanCache};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DISK_CACHE_VERSION: u32 = 1;

#[derive(Clone, Default, Serialize, Deserialize)]
struct TokenCounts {
    input: u64,
    output: u64,
    cache_read: u64,
}

/// Per-file scan result: date → model → tokens, plus the set of active dates.
#[derive(Clone, Default, Serialize, Deserialize)]
struct FileData {
    /// date (YYYY-MM-DD) → model → TokenCounts
    daily: HashMap<String, HashMap<String, TokenCounts>>,
    /// dates that had at least one session turn (for activity/streaks)
    dates: Vec<String>,
}

fn sessions_dir() -> Option<PathBuf> {
    let home = super::home_dir()?;
    Some(PathBuf::from(home).join(".codex").join("sessions"))
}

/// Recursively list all `rollout-*.jsonl` files under `~/.codex/sessions/`.
fn list_rollout_files(dir: &Path) -> Vec<(PathBuf, u128, u64)> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("rollout-"))
                    .unwrap_or(false)
            {
                let meta = match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let mtime_ns = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                result.push((path, mtime_ns, meta.len()));
            }
        }
    }
    result
}

/// Scan one rollout file into per-date/model token counts. Tracks the current model from
/// `turn_context` events; attributes each `token_count` (last_token_usage delta) to it.
fn scan_single_rollout(path: &Path) -> FileData {
    let mut data = FileData::default();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return data,
    };

    let mut current_model: Option<String> = None;
    let mut date_set: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // Track the session model.
        if line_type == "turn_context" {
            if let Some(m) = v
                .get("payload")
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
            {
                current_model = Some(m.to_string());
            }
            continue;
        }

        // token_count carries this turn's usage delta.
        if line_type == "event_msg" {
            let payload = match v.get("payload") {
                Some(p) => p,
                None => continue,
            };
            if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
                continue;
            }
            let last = match payload.get("info").and_then(|i| i.get("last_token_usage")) {
                Some(l) => l,
                None => continue,
            };
            let input = last
                .get("input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let cached = last
                .get("cached_input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let output = last
                .get("output_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let reasoning = last
                .get("reasoning_output_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            if input == 0 && output == 0 && reasoning == 0 {
                continue;
            }

            // Date from the event timestamp (fall back to skipping if absent).
            let date = match v.get("timestamp").and_then(|t| t.as_str()) {
                Some(ts) if ts.len() >= 10 => ts[..10].to_string(),
                _ => continue,
            };
            // Unknown model → label it so it still shows in the table (at $0 cost).
            let model = current_model
                .clone()
                .unwrap_or_else(|| "codex-unknown".to_string());

            let tc = data
                .daily
                .entry(date.clone())
                .or_default()
                .entry(model)
                .or_default();
            tc.input += input;
            tc.cache_read += cached;
            tc.output += output + reasoning;
            date_set.insert(date);
        }
    }

    data.dates = date_set.into_iter().collect();
    data
}

fn disk_cache_path() -> PathBuf {
    scan_cache_path("codex-usage-scan-cache.json")
}

/// Read aggregated global Codex usage. `days` filters the daily window (None = all time).
pub fn read_global_codex_usage(days: Option<u32>) -> Result<UsageOverview, String> {
    let dir = match sessions_dir() {
        Some(d) => d,
        None => return Ok(empty_overview()),
    };
    let files = list_rollout_files(&dir);
    log::debug!("[codex_usage] {} rollout files", files.len());

    let mut old_cache = DiskScanCache::<FileData>::read(&disk_cache_path(), DISK_CACHE_VERSION)
        .unwrap_or_else(|| {
            crate::storage::cli_sessions_common::empty_scan_cache(DISK_CACHE_VERSION)
        });
    let mut new_manifest: HashMap<String, CachedFile<FileData>> = HashMap::new();

    // date → model → TokenCounts (merged across all files)
    let mut merged: HashMap<String, HashMap<String, TokenCounts>> = HashMap::new();
    let mut all_dates: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (path, mtime_ns, size) in files {
        let key = cache_key(&path);
        // Reuse cached scan if unchanged.
        let data = old_cache
            .take_if_fresh(&key, mtime_ns, size)
            .unwrap_or_else(|| scan_single_rollout(&path));
        for (date, models) in &data.daily {
            let day = merged.entry(date.clone()).or_default();
            for (model, tc) in models {
                let agg = day.entry(model.clone()).or_default();
                agg.input += tc.input;
                agg.output += tc.output;
                agg.cache_read += tc.cache_read;
            }
        }
        for d in &data.dates {
            all_dates.insert(d.clone());
        }
        new_manifest.insert(
            key,
            CachedFile {
                mtime_ns,
                size,
                data,
            },
        );
    }

    DiskScanCache {
        version: DISK_CACHE_VERSION,
        manifest: new_manifest,
    }
    .write(&disk_cache_path());

    Ok(build_overview(merged, all_dates, days))
}

fn empty_overview() -> UsageOverview {
    UsageOverview {
        total_cost_usd: 0.0,
        total_tokens: 0,
        total_runs: 0,
        avg_cost_per_run: 0.0,
        by_model: Vec::new(),
        daily: Vec::new(),
        runs: Vec::new(),
        scan_mode: Some("codex".to_string()),
        active_days: 0,
        current_streak: 0,
        longest_streak: 0,
    }
}

fn build_overview(
    merged: HashMap<String, HashMap<String, TokenCounts>>,
    all_dates: std::collections::HashSet<String>,
    days: Option<u32>,
) -> UsageOverview {
    let cutoff = days.and_then(|d| {
        let now = chrono::Utc::now().date_naive();
        now.checked_sub_signed(chrono::Duration::days(d.saturating_sub(1) as i64))
            .map(|nd| nd.format("%Y-%m-%d").to_string())
    });

    let mut daily: Vec<DailyAggregate> = Vec::new();
    let mut model_totals: HashMap<String, ModelAggregate> = HashMap::new();
    let mut total_cost = 0.0f64;
    let mut total_tokens = 0u64;

    let mut dates: Vec<&String> = merged.keys().collect();
    dates.sort();

    for date in dates {
        if let Some(ref c) = cutoff {
            if date < c {
                continue;
            }
        }
        let models = &merged[date];
        let mut day_cost = 0.0f64;
        let mut day_in = 0u64;
        let mut day_out = 0u64;
        for (model, tc) in models {
            // Mirror App-scope Codex cost: try_estimate_cost (None → $0 for unknown models).
            let cost = pricing::try_estimate_cost(model, tc.input, tc.output, tc.cache_read, 0)
                .unwrap_or(0.0);
            day_cost += cost;
            day_in += tc.input;
            day_out += tc.output;

            let agg = model_totals.entry(model.clone()).or_insert(ModelAggregate {
                model: model.clone(),
                runs: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
                pct: 0.0,
            });
            agg.input_tokens += tc.input;
            agg.output_tokens += tc.output;
            agg.cache_read_tokens += tc.cache_read;
            agg.cost_usd += cost;
        }
        total_cost += day_cost;
        total_tokens += day_in + day_out;
        daily.push(DailyAggregate {
            date: date.clone(),
            cost_usd: day_cost,
            runs: 0,
            input_tokens: day_in,
            output_tokens: day_out,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: None,
        });
    }

    let mut by_model: Vec<ModelAggregate> = model_totals.into_values().collect();
    for m in &mut by_model {
        m.pct = if total_cost > 0.0 {
            m.cost_usd / total_cost * 100.0
        } else {
            0.0
        };
    }
    by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Activity/streaks computed over ALL dates (not date-filtered), matching claude_usage.
    let activity_daily: Vec<DailyAggregate> = all_dates
        .iter()
        .map(|d| DailyAggregate {
            date: d.clone(),
            cost_usd: 0.0,
            runs: 1,
            input_tokens: 0,
            output_tokens: 0,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: None,
        })
        .collect();
    let anchor = chrono::Utc::now().date_naive();
    let (active_days, current_streak, longest_streak) =
        super::claude_usage::compute_streaks(&activity_daily, anchor);

    let total_runs = all_dates.len() as u32; // sessions ~ active days; approximate
    UsageOverview {
        total_cost_usd: total_cost,
        total_tokens,
        total_runs,
        avg_cost_per_run: 0.0,
        by_model,
        daily,
        runs: Vec::new(),
        scan_mode: Some("codex".to_string()),
        active_days,
        current_streak,
        longest_streak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_last_token_usage_not_cumulative() {
        // Two token_count events: last deltas 100/200 in, cumulative would be 100 then 300.
        // Correct total input = 300 (sum of last), NOT 400 (last + cumulative).
        let dir = std::env::temp_dir().join(format!("codex_usage_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("rollout-test.jsonl");
        let lines = [
            r#"{"timestamp":"2026-06-01T10:00:00Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            r#"{"timestamp":"2026-06-01T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":0},"total_token_usage":{"input_tokens":100}}}}"#,
            r#"{"timestamp":"2026-06-01T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200,"cached_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":5},"total_token_usage":{"input_tokens":300}}}}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();

        let fd = scan_single_rollout(&path);
        let tc = &fd.daily["2026-06-01"]["gpt-5.4"];
        assert_eq!(tc.input, 300, "should sum last_token_usage deltas only");
        assert_eq!(tc.output, 35, "output = 10 + (20+5 reasoning)");
        assert_eq!(tc.cache_read, 40);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_model_zero_cost_still_listed() {
        let merged: HashMap<String, HashMap<String, TokenCounts>> = HashMap::from([(
            "2026-06-01".to_string(),
            HashMap::from([(
                "gpt-oss-mystery".to_string(),
                TokenCounts {
                    input: 1000,
                    output: 500,
                    cache_read: 0,
                },
            )]),
        )]);
        let ov = build_overview(merged, std::collections::HashSet::new(), None);
        assert_eq!(ov.by_model.len(), 1);
        assert_eq!(
            ov.by_model[0].cost_usd, 0.0,
            "unknown model → $0, no Sonnet fallback"
        );
        assert_eq!(ov.total_tokens, 1500);
    }
}

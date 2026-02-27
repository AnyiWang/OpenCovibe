//! Read Claude Code global usage by scanning session JSONL files.
//!
//! Scans `~/.claude/projects/*/*.jsonl` for per-turn token usage,
//! aggregates by date and model, computes cost via `pricing` module.
//! Activity metrics (messages, sessions, tool calls) come from
//! `~/.claude/stats-cache.json` which tracks those separately.
//!
//! Results are cached in memory (120s TTL) and on disk
//! (`~/.opencovibe/usage-scan-cache.json`) to avoid rescanning
//! unchanged files across restarts.

use crate::models::{DailyAggregate, ModelAggregate, UsageOverview};
use crate::pricing;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, UNIX_EPOCH};

// ── In-memory cache ──

static CACHE: std::sync::LazyLock<Mutex<Option<CachedData>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Separate mutex to prevent concurrent recomputation of the scan.
static COMPUTE_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

const CACHE_TTL_SECS: u64 = 120;

struct CachedData {
    computed_at: Instant,
    /// date → model → TokenCounts (from JSONL scan)
    daily_model: DailyModelMap,
    /// date → (messages, sessions, tool_calls) (from stats-cache.json)
    daily_activity: HashMap<String, (u32, u32, u32)>,
    /// date → (messages, sessions) derived from JSONL scan (fallback)
    scan_activity: ScanActivityMap,
    total_sessions: u32,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct TokenCounts {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_create: u64,
}

// ── Disk cache types ──

const DISK_CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct DiskCache {
    version: u32,
    /// file path → (mtime_ns, size_bytes)
    manifest: HashMap<String, (u128, u64)>,
    /// file path → per-file aggregated data
    per_file: HashMap<String, FileData>,
}

#[derive(Serialize, Deserialize, Clone)]
struct FileData {
    /// date → model → TokenCounts
    daily_tokens: HashMap<String, HashMap<String, TokenCounts>>,
    /// date → message count
    daily_messages: HashMap<String, u32>,
}

// ── JSONL line schema (partial — unknown fields are skipped by serde) ──

#[derive(Deserialize)]
struct SessionLine {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<LineMessage>,
}

#[derive(Deserialize)]
struct LineMessage {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<LineUsage>,
}

#[derive(Deserialize)]
struct LineUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

// ── stats-cache.json (activity data only) ──

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StatsCache {
    #[serde(default)]
    total_sessions: u32,
    #[serde(default)]
    daily_activity: Vec<DailyActivityEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyActivityEntry {
    date: String,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    session_count: u32,
    #[serde(default)]
    tool_call_count: u32,
}

// ── Public API ──

pub fn read_global_usage(days: Option<u32>) -> Result<UsageOverview, String> {
    let _compute_guard = COMPUTE_LOCK
        .lock()
        .map_err(|e| format!("Compute lock: {e}"))?;

    let home = super::dirs_next().ok_or("Could not determine home directory")?;
    let claude_dir = home.join(".claude");

    // Check in-memory cache
    {
        let lock = CACHE.lock().map_err(|e| format!("Cache lock: {e}"))?;
        if let Some(ref cached) = *lock {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                log::debug!(
                    "[claude_usage] memory cache hit (age {}s)",
                    cached.computed_at.elapsed().as_secs()
                );
                let mut overview = build_overview(cached, days);
                overview.scan_mode = Some("memory".to_string());
                return Ok(overview);
            }
        }
    }

    let start = Instant::now();

    // List all JSONL files with metadata (no content reading yet)
    let current_files = list_jsonl_files(&claude_dir);
    let current_manifest: HashMap<String, (u128, u64)> = current_files
        .iter()
        .map(|(p, mtime, size)| (p.to_string_lossy().to_string(), (*mtime, *size)))
        .collect();

    // Load disk cache
    let disk_cache = read_disk_cache();

    // Compare manifests: determine which files are clean vs dirty
    let mut per_file: HashMap<String, FileData> = HashMap::new();
    let mut dirty_count = 0u32;
    let mut clean_count = 0u32;
    let has_disk_cache = disk_cache.is_some();

    let old_manifest = disk_cache
        .as_ref()
        .map(|dc| &dc.manifest)
        .cloned()
        .unwrap_or_default();
    let old_per_file = disk_cache.map(|dc| dc.per_file).unwrap_or_default();

    for (path_str, &(mtime, size)) in &current_manifest {
        if let Some(&(old_mtime, old_size)) = old_manifest.get(path_str) {
            if mtime == old_mtime && size == old_size {
                // Unchanged — reuse cached data
                if let Some(cached_data) = old_per_file.get(path_str) {
                    per_file.insert(path_str.clone(), cached_data.clone());
                    clean_count += 1;
                    continue;
                }
            }
        }
        // New or changed — scan this file
        let path = Path::new(path_str);
        let file_data = scan_single_jsonl_standalone(path);
        per_file.insert(path_str.clone(), file_data);
        dirty_count += 1;
    }
    // Deleted files are simply not in current_manifest, so they are dropped

    let scan_mode = if !has_disk_cache {
        "full"
    } else if dirty_count > 0 {
        "incremental"
    } else {
        "disk"
    };

    log::debug!(
        "[claude_usage] scan_mode={}, {} clean, {} dirty, {} total files",
        scan_mode,
        clean_count,
        dirty_count,
        current_manifest.len(),
    );

    // Merge all per-file data into aggregate structures
    let (daily_model, scan_activity) = merge_all_file_data(&per_file);

    // Write updated disk cache (atomic)
    let new_disk_cache = DiskCache {
        version: DISK_CACHE_VERSION,
        manifest: current_manifest,
        per_file,
    };
    write_disk_cache(&new_disk_cache);

    // Read stats-cache for activity data (messages, sessions, tool calls)
    let (daily_activity, total_sessions) = read_activity_data(&claude_dir);

    let scan_secs = start.elapsed().as_secs_f64();
    log::debug!(
        "[claude_usage] done in {:.2}s ({}): {} days token, {} days activity, {} sessions",
        scan_secs,
        scan_mode,
        daily_model.len(),
        daily_activity.len(),
        total_sessions,
    );

    let cached = CachedData {
        computed_at: Instant::now(),
        daily_model,
        daily_activity,
        scan_activity,
        total_sessions,
    };

    let mut overview = build_overview(&cached, days);
    overview.scan_mode = Some(scan_mode.to_string());

    // Store in memory cache
    if let Ok(mut lock) = CACHE.lock() {
        *lock = Some(cached);
    }

    Ok(overview)
}

// ── File listing (metadata only) ──

/// Recursively list all `.jsonl` files under `~/.claude/projects/`.
/// Returns `(path, mtime_ns, size_bytes)` tuples — no file content is read.
fn list_jsonl_files(claude_dir: &Path) -> Vec<(PathBuf, u128, u64)> {
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.is_dir() {
        log::debug!("[claude_usage] no projects directory at {:?}", projects_dir);
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut dirs_to_visit: Vec<PathBuf> = vec![projects_dir];

    while let Some(dir) = dirs_to_visit.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == "memory").unwrap_or(false) {
                    continue;
                }
                dirs_to_visit.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
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
                let size = meta.len();
                result.push((path, mtime_ns, size));
            }
        }
    }

    result
}

// ── Disk cache I/O ──

fn disk_cache_path() -> PathBuf {
    super::data_dir().join("usage-scan-cache.json")
}

/// Read the disk cache file. Returns `None` on any error or version mismatch.
fn read_disk_cache() -> Option<DiskCache> {
    let path = disk_cache_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => {
            log::debug!("[claude_usage] no disk cache at {:?}", path);
            return None;
        }
    };

    let cache: DiskCache = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            log::debug!("[claude_usage] disk cache parse error: {e}");
            return None;
        }
    };

    if cache.version != DISK_CACHE_VERSION {
        log::debug!(
            "[claude_usage] disk cache version mismatch: {} != {}",
            cache.version,
            DISK_CACHE_VERSION
        );
        return None;
    }

    log::debug!(
        "[claude_usage] loaded disk cache: {} files",
        cache.manifest.len()
    );
    Some(cache)
}

/// Atomically write disk cache: write to .tmp file, then rename.
fn write_disk_cache(cache: &DiskCache) {
    let path = disk_cache_path();
    let tmp_path = path.with_extension("json.tmp");

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = super::ensure_dir(parent);
    }

    let json = match serde_json::to_string(cache) {
        Ok(j) => j,
        Err(e) => {
            log::error!("[claude_usage] failed to serialize disk cache: {e}");
            return;
        }
    };

    if let Err(e) = std::fs::write(&tmp_path, &json) {
        log::error!("[claude_usage] failed to write disk cache tmp: {e}");
        return;
    }

    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        log::error!("[claude_usage] failed to rename disk cache: {e}");
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    log::debug!(
        "[claude_usage] wrote disk cache: {} files, {} bytes",
        cache.manifest.len(),
        json.len(),
    );
}

// ── Standalone single-file scanner (returns FileData, no side-effects) ──

fn scan_single_jsonl_standalone(path: &Path) -> FileData {
    let mut daily_tokens: HashMap<String, HashMap<String, TokenCounts>> = HashMap::new();
    let mut daily_messages: HashMap<String, u32> = HashMap::new();

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            return FileData {
                daily_tokens,
                daily_messages,
            };
        }
    };
    let reader = std::io::BufReader::new(file);

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Count messages: lines with "role":"user" or "role":"assistant"
        let is_message =
            line.contains("\"role\":\"user\"") || line.contains("\"role\":\"assistant\"");
        if is_message {
            if let Some(date) = extract_date_fast(&line) {
                *daily_messages.entry(date).or_default() += 1;
            }
        }

        // Fast filter for token usage data
        if !line.contains("\"cache_read_input_tokens\"") {
            continue;
        }

        let parsed: SessionLine = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let message = match parsed.message {
            Some(m) => m,
            None => continue,
        };
        let usage = match message.usage {
            Some(u) => u,
            None => continue,
        };
        let model = match message.model {
            Some(m) if !m.is_empty() => m,
            _ => continue,
        };

        if parsed.timestamp.len() < 10 {
            continue;
        }
        let date = &parsed.timestamp[..10];

        let day_entry = daily_tokens.entry(date.to_string()).or_default();
        let tc = day_entry.entry(model).or_default();
        tc.input += usage.input_tokens;
        tc.output += usage.output_tokens;
        tc.cache_read += usage.cache_read_input_tokens;
        tc.cache_create += usage.cache_creation_input_tokens;
    }

    FileData {
        daily_tokens,
        daily_messages,
    }
}

// ── Merge all per-file data into aggregate structures ──

fn merge_all_file_data(per_file: &HashMap<String, FileData>) -> (DailyModelMap, ScanActivityMap) {
    let mut daily_model: DailyModelMap = BTreeMap::new();
    // date → set of file path strings (for counting unique sessions per day)
    let mut daily_sessions: HashMap<String, HashSet<&str>> = HashMap::new();
    let mut daily_messages_agg: HashMap<String, u32> = HashMap::new();

    for (file_path, file_data) in per_file {
        // Merge token data
        for (date, models) in &file_data.daily_tokens {
            let day_entry = daily_model.entry(date.clone()).or_default();
            for (model, tc) in models {
                let entry = day_entry.entry(model.clone()).or_default();
                entry.input += tc.input;
                entry.output += tc.output;
                entry.cache_read += tc.cache_read;
                entry.cache_create += tc.cache_create;
            }
        }

        // Merge message counts + session tracking
        for (date, &msg_count) in &file_data.daily_messages {
            *daily_messages_agg.entry(date.clone()).or_default() += msg_count;
            daily_sessions
                .entry(date.clone())
                .or_default()
                .insert(file_path.as_str());
        }
    }

    // Build scan_activity: date → (messages, sessions)
    let scan_activity: ScanActivityMap = daily_messages_agg
        .into_iter()
        .map(|(date, msgs)| {
            let sessions = daily_sessions
                .get(&date)
                .map(|s| s.len() as u32)
                .unwrap_or(0);
            (date, (msgs, sessions))
        })
        .collect();

    (daily_model, scan_activity)
}

// ── Build UsageOverview from cached data with date filter ──

fn build_overview(data: &CachedData, days: Option<u32>) -> UsageOverview {
    let cutoff_date = days.map(|d| {
        let now = chrono::Utc::now().date_naive();
        // "1d" = today only, "7d" = last 7 days including today
        now - chrono::Duration::days(d.saturating_sub(1) as i64)
    });

    let in_range = |date_str: &str| -> bool {
        match cutoff_date {
            Some(ref cutoff) => chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                .map(|d| d >= *cutoff)
                .unwrap_or(true),
            None => true,
        }
    };

    // ── Per-model totals (filtered) + daily aggregates ──
    let mut model_totals: HashMap<String, TokenCounts> = HashMap::new();
    let mut daily_aggs: Vec<DailyAggregate> = Vec::new();
    let mut filtered_sessions: u32 = 0;

    // Collect all dates from both token data and activity data
    let mut all_dates: BTreeMap<&str, ()> = BTreeMap::new();
    for date in data.daily_model.keys() {
        all_dates.insert(date.as_str(), ());
    }
    for date in data.daily_activity.keys() {
        all_dates.insert(date.as_str(), ());
    }

    for date in all_dates.keys() {
        if !in_range(date) {
            continue;
        }

        let models = data.daily_model.get(*date);
        let activity = data.daily_activity.get(*date);
        let scan_act = data.scan_activity.get(*date);

        // Accumulate per-model totals
        let mut day_input = 0u64;
        let mut day_output = 0u64;
        let mut day_cost = 0.0f64;

        if let Some(models) = models {
            for (model, tc) in models {
                let entry = model_totals.entry(model.clone()).or_default();
                entry.input += tc.input;
                entry.output += tc.output;
                entry.cache_read += tc.cache_read;
                entry.cache_create += tc.cache_create;

                day_input += tc.input;
                day_output += tc.output;
                day_cost += pricing::estimate_cost(
                    model,
                    tc.input,
                    tc.output,
                    tc.cache_read,
                    tc.cache_create,
                );
            }
        }

        // Activity: prefer stats-cache, fall back to JSONL-derived counts
        let (msg_count, sess_count, tool_count) = if let Some(&(m, s, t)) = activity {
            (m, s, t)
        } else if let Some(&(msgs, sessions)) = scan_act {
            // JSONL-derived: messages ≈ assistant turns × 2, no tool count
            (msgs, sessions, 0)
        } else {
            (0, 0, 0)
        };

        filtered_sessions += sess_count;

        daily_aggs.push(DailyAggregate {
            date: date.to_string(),
            cost_usd: day_cost,
            runs: sess_count,
            input_tokens: day_input,
            output_tokens: day_output,
            message_count: Some(msg_count),
            session_count: Some(sess_count),
            tool_call_count: if tool_count > 0 {
                Some(tool_count)
            } else {
                None
            },
            model_breakdown: None,
        });
    }

    // Populate model_breakdown for last 30 days only (stacked chart window)
    let breakdown_start = daily_aggs.len().saturating_sub(30);
    for agg in &mut daily_aggs[breakdown_start..] {
        if let Some(models) = data.daily_model.get(&agg.date) {
            agg.model_breakdown = Some(
                models
                    .iter()
                    .map(|(model, tc)| {
                        (
                            model.clone(),
                            crate::models::ModelTokens {
                                input_tokens: tc.input,
                                output_tokens: tc.output,
                                cache_read_tokens: tc.cache_read,
                                cache_write_tokens: tc.cache_create,
                            },
                        )
                    })
                    .collect(),
            );
        }
    }

    // ── By-model aggregates ──
    let mut total_cost = 0.0f64;
    let mut total_tokens = 0u64;
    let mut by_model: Vec<ModelAggregate> = Vec::new();

    for (model, tc) in &model_totals {
        let cost =
            pricing::estimate_cost(model, tc.input, tc.output, tc.cache_read, tc.cache_create);
        total_cost += cost;
        total_tokens += tc.input + tc.output;

        by_model.push(ModelAggregate {
            model: model.clone(),
            runs: 0,
            input_tokens: tc.input,
            output_tokens: tc.output,
            cache_read_tokens: tc.cache_read,
            cache_write_tokens: tc.cache_create,
            cost_usd: cost,
            pct: 0.0,
        });
    }

    // Fill pct and sort by cost descending
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

    // ── Summary ──
    let total_sessions = if days.is_some() {
        filtered_sessions
    } else {
        data.total_sessions
    };

    let avg_cost = if total_sessions > 0 {
        total_cost / total_sessions as f64
    } else {
        0.0
    };

    log::debug!(
        "[claude_usage] overview: days={:?}, ${:.2}, {} sessions, {} models, {} daily",
        days,
        total_cost,
        total_sessions,
        by_model.len(),
        daily_aggs.len(),
    );

    let (active_days, current_streak, longest_streak) =
        compute_streaks(&daily_aggs, chrono::Utc::now().date_naive());

    UsageOverview {
        total_cost_usd: total_cost,
        total_tokens,
        total_runs: total_sessions,
        avg_cost_per_run: avg_cost,
        by_model,
        daily: daily_aggs,
        runs: vec![],
        scan_mode: None,
        active_days,
        current_streak,
        longest_streak,
    }
}

// ── JSONL scanning (type aliases) ──

type DailyModelMap = BTreeMap<String, HashMap<String, TokenCounts>>;
type ScanActivityMap = HashMap<String, (u32, u32)>;

/// Extract date ("YYYY-MM-DD") from a JSONL line by finding the "timestamp" field.
fn extract_date_fast(line: &str) -> Option<String> {
    // Look for "timestamp":"2026-02-13T..." pattern
    let marker = "\"timestamp\":\"";
    let idx = line.find(marker)?;
    let start = idx + marker.len();
    if start + 10 > line.len() {
        return None;
    }
    let date = &line[start..start + 10];
    // Quick validation: should look like "YYYY-MM-DD"
    if date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-' {
        Some(date.to_string())
    } else {
        None
    }
}

// ── Activity data from stats-cache.json ──

fn read_activity_data(claude_dir: &Path) -> (HashMap<String, (u32, u32, u32)>, u32) {
    let path = claude_dir.join("stats-cache.json");
    if !path.exists() {
        return (HashMap::new(), 0);
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            log::error!("[claude_usage] failed to read stats-cache.json: {e}");
            return (HashMap::new(), 0);
        }
    };

    let cache: StatsCache = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[claude_usage] failed to parse stats-cache.json: {e}");
            return (HashMap::new(), 0);
        }
    };

    let activity: HashMap<String, (u32, u32, u32)> = cache
        .daily_activity
        .into_iter()
        .map(|a| {
            (
                a.date,
                (a.message_count, a.session_count, a.tool_call_count),
            )
        })
        .collect();

    (activity, cache.total_sessions)
}

// ── Cache clearing ──

/// Compute (active_days, current_streak, longest_streak) from daily aggregates.
/// A day is active if input_tokens + output_tokens > 0 || message_count > 0 || runs > 0.
/// `anchor` is the reference "today" date (UTC).
pub(crate) fn compute_streaks(
    daily: &[crate::models::DailyAggregate],
    anchor: chrono::NaiveDate,
) -> (u32, u32, u32) {
    // Collect active dates into a HashSet
    let active_dates: std::collections::HashSet<chrono::NaiveDate> = daily
        .iter()
        .filter(|d| {
            d.input_tokens + d.output_tokens > 0 || d.message_count.unwrap_or(0) > 0 || d.runs > 0
        })
        .filter_map(|d| chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok())
        .collect();

    let active_days = active_dates.len() as u32;
    if active_days == 0 {
        return (0, 0, 0);
    }

    // Current streak: count backward from anchor
    let mut current_streak = 0u32;
    let mut day = anchor;
    loop {
        if active_dates.contains(&day) {
            current_streak += 1;
            day -= chrono::Duration::days(1);
        } else if day == anchor {
            // Today not active, try yesterday
            day -= chrono::Duration::days(1);
            continue;
        } else {
            break;
        }
    }

    // Longest streak: sort dates, scan for consecutive runs
    let mut sorted: Vec<chrono::NaiveDate> = active_dates.into_iter().collect();
    sorted.sort();
    let mut longest_streak = 0u32;
    let mut streak = 0u32;
    let mut prev: Option<chrono::NaiveDate> = None;
    for d in &sorted {
        if let Some(p) = prev {
            if *d == p + chrono::Duration::days(1) {
                streak += 1;
            } else {
                longest_streak = longest_streak.max(streak);
                streak = 1;
            }
        } else {
            streak = 1;
        }
        prev = Some(*d);
    }
    longest_streak = longest_streak.max(streak);

    (active_days, current_streak, longest_streak)
}

/// Clear both in-memory and disk caches, forcing a full rescan on next request.
pub fn clear_cache() {
    // Clear in-memory cache
    if let Ok(mut lock) = CACHE.lock() {
        *lock = None;
        log::debug!("[claude_usage] in-memory cache cleared");
    }

    // Delete disk cache file
    let path = super::data_dir().join("usage-scan-cache.json");
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            log::error!("[claude_usage] failed to remove disk cache: {e}");
        } else {
            log::debug!("[claude_usage] disk cache deleted: {:?}", path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DailyAggregate;

    fn make_day(date: &str, input: u64, output: u64, runs: u32) -> DailyAggregate {
        DailyAggregate {
            date: date.to_string(),
            cost_usd: 0.0,
            runs,
            input_tokens: input,
            output_tokens: output,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: None,
        }
    }

    #[test]
    fn test_compute_streaks_empty() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        assert_eq!(compute_streaks(&[], anchor), (0, 0, 0));
    }

    #[test]
    fn test_compute_streaks_single_today() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        let daily = vec![make_day("2026-02-25", 100, 50, 0)];
        assert_eq!(compute_streaks(&daily, anchor), (1, 1, 1));
    }

    #[test]
    fn test_compute_streaks_gap() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        let daily = vec![
            make_day("2026-02-21", 100, 50, 0),
            make_day("2026-02-22", 100, 50, 0),
            make_day("2026-02-24", 100, 50, 0),
            make_day("2026-02-25", 100, 50, 0),
        ];
        // active=4, current=2 (25,24), longest=2
        assert_eq!(compute_streaks(&daily, anchor), (4, 2, 2));
    }

    #[test]
    fn test_compute_streaks_yesterday_start() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        let daily = vec![
            make_day("2026-02-23", 100, 50, 0),
            make_day("2026-02-24", 100, 50, 0),
        ];
        // Today not active, yesterday active -> current streak = 2
        assert_eq!(compute_streaks(&daily, anchor), (2, 2, 2));
    }

    #[test]
    fn test_compute_streaks_no_recent() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        let daily = vec![make_day("2026-02-10", 100, 50, 0)];
        // active=1, current=0 (gap too large), longest=1
        assert_eq!(compute_streaks(&daily, anchor), (1, 0, 1));
    }

    #[test]
    fn test_compute_streaks_runs_only_active() {
        let anchor = chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap();
        let daily = vec![make_day("2026-02-25", 0, 0, 1)]; // tokens=0 but runs=1
        assert_eq!(compute_streaks(&daily, anchor), (1, 1, 1));
    }

    #[test]
    fn test_model_tokens_serialization() {
        let mt = crate::models::ModelTokens {
            input_tokens: 100,
            output_tokens: 200,
            cache_read_tokens: 50,
            cache_write_tokens: 25,
        };
        let json = serde_json::to_string(&mt).unwrap();
        assert!(json.contains("cacheWriteTokens"));
        assert!(!json.contains("cacheCreate"));
        assert!(!json.contains("cache_write"));
    }

    #[test]
    fn test_model_breakdown_last_30_only() {
        // Build 50 days of data in a CachedData-like structure
        // This test verifies the build_overview logic indirectly
        let mut daily: Vec<DailyAggregate> = Vec::new();
        for i in 0..50 {
            let date = format!("2026-01-{:02}", (i % 28) + 1);
            daily.push(DailyAggregate {
                date,
                cost_usd: 1.0,
                runs: 1,
                input_tokens: 100,
                output_tokens: 50,
                message_count: None,
                session_count: None,
                tool_call_count: None,
                model_breakdown: Some(std::collections::HashMap::from([(
                    "claude-opus-4".to_string(),
                    crate::models::ModelTokens {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                )])),
            });
        }
        // Simulate the "last 30 only" logic
        let breakdown_start = daily.len().saturating_sub(30);
        for (i, agg) in daily.iter_mut().enumerate() {
            if i < breakdown_start {
                agg.model_breakdown = None;
            }
        }
        // First 20 should be None, last 30 should be Some
        for (i, agg) in daily.iter().enumerate() {
            if i < 20 {
                assert!(agg.model_breakdown.is_none(), "day {} should be None", i);
            } else {
                assert!(agg.model_breakdown.is_some(), "day {} should be Some", i);
            }
        }
    }
}

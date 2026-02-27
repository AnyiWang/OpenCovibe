//! Prompt index — scans events.jsonl files to extract searchable messages
//! (user prompts + assistant responses).
//!
//! Index file:    `~/.opencovibe/prompt-index.jsonl`
//! Manifest file: `~/.opencovibe/prompt-index-manifest.json`
//!
//! Uses in-memory cache with 120s TTL (same pattern as `claude_usage.rs`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

// ── Types ──

#[derive(Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    pub run_id: String,
    pub seq: u64,
    pub ts: String,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    /// run_id → (mtime_ns, file_size)
    runs: HashMap<String, (u128, u64)>,
}

// ── Cache ──

struct CachedIndex {
    computed_at: Instant,
    entries: Vec<PromptEntry>,
}

static CACHE: std::sync::LazyLock<Mutex<Option<CachedIndex>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

static COMPUTE_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

const CACHE_TTL_SECS: u64 = 120;

// ── File paths ──

fn index_path() -> std::path::PathBuf {
    super::data_dir().join("prompt-index.jsonl")
}

fn manifest_path() -> std::path::PathBuf {
    super::data_dir().join("prompt-index-manifest.json")
}

/// Atomically write content to `path` (write .tmp → set 0o600 → rename).
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("write tmp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

fn file_fingerprint(path: &Path) -> Option<(u128, u64)> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, meta.len()))
}

// ── Scanning ──

/// Max text length to index per entry (avoid huge tool outputs bloating the index).
const MAX_TEXT_LEN: usize = 500;

/// Extract searchable messages from a single events.jsonl file.
///
/// Indexes: user_message, message_complete (assistant text).
fn scan_events_file(run_id: &str, events_path: &Path) -> Vec<PromptEntry> {
    let file = match fs::File::open(events_path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let reader = std::io::BufReader::new(file);
    let mut entries = vec![];
    let mut seq: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        // Substring pre-filter: only parse lines that might match
        let is_user_bus = line.contains("\"user_message\"");
        let is_assistant_bus = line.contains("\"message_complete\"");
        let is_legacy = line.contains("\"type\":\"user\"") && !line.contains("\"_bus\"");

        if !is_user_bus && !is_assistant_bus && !is_legacy {
            continue;
        }

        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Bus format: {"_bus":true,"seq":N,"ts":"...","event":{"type":"...","text":"..."}}
        if event.get("_bus").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(inner) = event.get("event") {
                let inner_type = inner.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if inner_type == "user_message" || inner_type == "message_complete" {
                    if let Some(text) = inner.get("text").and_then(|t| t.as_str()) {
                        if text.is_empty() {
                            continue;
                        }
                        seq += 1;
                        let ts = event
                            .get("ts")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let truncated = if text.len() > MAX_TEXT_LEN {
                            format!("{}…", &text[..text.floor_char_boundary(MAX_TEXT_LEN)])
                        } else {
                            text.to_string()
                        };
                        entries.push(PromptEntry {
                            run_id: run_id.to_string(),
                            seq,
                            ts,
                            text: truncated,
                        });
                    }
                }
            }
        }
        // Legacy format: {"type":"user","payload":{"text":"..."},"timestamp":"..."}
        else if event.get("type").and_then(|v| v.as_str()) == Some("user") {
            if let Some(text) = event
                .get("payload")
                .and_then(|p| p.get("text"))
                .and_then(|t| t.as_str())
            {
                if text.is_empty() {
                    continue;
                }
                seq += 1;
                let ts = event
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let truncated = if text.len() > MAX_TEXT_LEN {
                    format!("{}…", &text[..text.floor_char_boundary(MAX_TEXT_LEN)])
                } else {
                    text.to_string()
                };
                entries.push(PromptEntry {
                    run_id: run_id.to_string(),
                    seq,
                    ts,
                    text: truncated,
                });
            }
        }
    }

    entries
}

/// Build or incrementally update the prompt index.
pub fn build_or_update_index() -> Result<Vec<PromptEntry>, String> {
    // Fast path: check cache TTL
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                log::debug!(
                    "[prompt_index] cache hit ({} entries)",
                    cached.entries.len()
                );
                return Ok(cached.entries.clone());
            }
        }
    }

    // Acquire compute lock (prevents concurrent rebuilds)
    let _lock = COMPUTE_LOCK.lock().unwrap();

    // Double-check cache after acquiring lock
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                return Ok(cached.entries.clone());
            }
        }
    }

    log::debug!("[prompt_index] rebuilding index");
    let start = Instant::now();

    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        log::debug!("[prompt_index] no runs dir, returning empty");
        let entries = vec![];
        update_cache(entries.clone());
        return Ok(entries);
    }

    // Load existing manifest
    let mut manifest = load_manifest();
    let mut all_entries: Vec<PromptEntry> = vec![];

    // Load existing index entries (to reuse unchanged runs)
    let existing_entries = load_index_file();
    let mut existing_by_run: HashMap<String, Vec<PromptEntry>> = HashMap::new();
    for entry in existing_entries {
        existing_by_run
            .entry(entry.run_id.clone())
            .or_default()
            .push(entry);
    }

    // Collect current run IDs
    let mut current_run_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Ok(dir_entries) = fs::read_dir(&runs_dir) {
        for entry in dir_entries.flatten() {
            let run_id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let events_path = entry.path().join("events.jsonl");
            if !events_path.exists() {
                continue;
            }

            current_run_ids.insert(run_id.clone());

            let fingerprint = file_fingerprint(&events_path);
            let cached_fp = manifest.runs.get(&run_id).cloned();

            if fingerprint == cached_fp && existing_by_run.contains_key(&run_id) {
                // Unchanged — reuse cached entries
                if let Some(entries) = existing_by_run.remove(&run_id) {
                    all_entries.extend(entries);
                }
            } else {
                // Changed or new — rescan
                log::debug!("[prompt_index] scanning run: {}", run_id);
                let entries = scan_events_file(&run_id, &events_path);
                all_entries.extend(entries);

                // Update manifest
                if let Some(fp) = fingerprint {
                    manifest.runs.insert(run_id, fp);
                }
            }
        }
    }

    // Remove deleted runs from manifest
    manifest.runs.retain(|id, _| current_run_ids.contains(id));

    // Write index + manifest atomically
    let index_content: String = all_entries
        .iter()
        .filter_map(|e| serde_json::to_string(e).ok())
        .collect::<Vec<_>>()
        .join("\n");

    super::ensure_dir(super::data_dir().as_path()).map_err(|e| e.to_string())?;
    write_atomic(&index_path(), &index_content)?;

    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    write_atomic(&manifest_path(), &manifest_json)?;

    let elapsed = start.elapsed();
    log::debug!(
        "[prompt_index] index built: {} entries in {:?}",
        all_entries.len(),
        elapsed
    );

    update_cache(all_entries.clone());
    Ok(all_entries)
}

/// Current manifest version.  Bump this when the scanning logic changes
/// so that existing on-disk manifests are treated as stale and all runs
/// are re-scanned.
const MANIFEST_VERSION: u32 = 2;

fn load_manifest() -> Manifest {
    let path = manifest_path();
    if !path.exists() {
        return Manifest {
            version: MANIFEST_VERSION,
            runs: HashMap::new(),
        };
    }
    match fs::read_to_string(&path) {
        Ok(content) => {
            let m: Manifest = serde_json::from_str(&content).unwrap_or(Manifest {
                version: MANIFEST_VERSION,
                runs: HashMap::new(),
            });
            // Version mismatch → force full rescan
            if m.version != MANIFEST_VERSION {
                log::debug!(
                    "[prompt_index] manifest version {} != {}, forcing full rescan",
                    m.version,
                    MANIFEST_VERSION
                );
                return Manifest {
                    version: MANIFEST_VERSION,
                    runs: HashMap::new(),
                };
            }
            m
        }
        Err(_) => Manifest {
            version: MANIFEST_VERSION,
            runs: HashMap::new(),
        },
    }
}

fn load_index_file() -> Vec<PromptEntry> {
    let path = index_path();
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
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn update_cache(entries: Vec<PromptEntry>) {
    let mut cache = CACHE.lock().unwrap();
    *cache = Some(CachedIndex {
        computed_at: Instant::now(),
        entries,
    });
}

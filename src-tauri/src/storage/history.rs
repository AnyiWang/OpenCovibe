//! Bounded, rebuildable history projection for large runs.
//!
//! `events.jsonl` remains the source of truth. This module builds immutable generations whose
//! pages are small enough to cross Tauri/WebSocket IPC without materializing the complete run.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;

const FORMAT_VERSION: u32 = 1;
const BUILDER_VERSION: u32 = 9;
const PAGE_ENTRY_LIMIT: usize = 100;
const PAGE_BYTE_LIMIT: usize = 1024 * 1024;
const ENTRY_BYTE_LIMIT: usize = 256 * 1024;
const CONTENT_INLINE_LIMIT: usize = 64 * 1024;
const CONTENT_CHUNK_LIMIT: usize = 256 * 1024;
const PARSE_LINE_LIMIT: usize = 2 * 1024 * 1024;
const PREVIEW_HEAD_BYTES: usize = 24 * 1024;
const PREVIEW_TAIL_BYTES: usize = 8 * 1024;
const MAX_OPEN_TOOLS: usize = 256;
const MAX_SUBHISTORIES: usize = 64;
const MAX_PENDING_INTERACTIONS: usize = 64;
const MAX_JSON_NESTING_DEPTH: usize = 512;
const MAX_JSON_SCALAR_BYTES: usize = 64 * 1024;

static BUILD_LOCKS: Lazy<Vec<Mutex<()>>> = Lazy::new(|| (0..64).map(|_| Mutex::new(())).collect());
static GLOBAL_BUILD_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static PROCESS_RETENTION_ID: Lazy<String> = Lazy::new(|| uuid::Uuid::new_v4().simple().to_string());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryContent {
    pub preview: String,
    pub byte_length: u64,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
// Pages cap this enum at 100 entries; keeping the tagged IPC shape flat avoids wrapper-only
// allocation and matching complexity while the in-memory upper bound remains below one page.
#[allow(clippy::large_enum_variant)]
pub enum HistoryEntry {
    User {
        id: String,
        #[serde(rename = "anchorId")]
        anchor_id: String,
        ts: String,
        content: HistoryContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "cliUuid")]
        cli_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Value>,
        #[serde(rename = "firstSeq")]
        first_seq: u64,
        #[serde(rename = "lastSeq")]
        last_seq: u64,
    },
    Assistant {
        id: String,
        #[serde(rename = "anchorId")]
        anchor_id: String,
        ts: String,
        content: HistoryContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<HistoryContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(rename = "firstSeq")]
        first_seq: u64,
        #[serde(rename = "lastSeq")]
        last_seq: u64,
    },
    Tool {
        id: String,
        #[serde(rename = "anchorId")]
        anchor_id: String,
        ts: String,
        #[serde(rename = "toolUseId")]
        tool_use_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        status: String,
        input: Value,
        output: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "toolUseResult")]
        tool_use_result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "inputContent")]
        input_content: Option<HistoryContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "outputContent")]
        output_content: Option<HistoryContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "resultContent")]
        result_content: Option<HistoryContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "durationMs")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "subHistoryId")]
        sub_history_id: Option<String>,
        #[serde(rename = "firstSeq")]
        first_seq: u64,
        #[serde(rename = "lastSeq")]
        last_seq: u64,
    },
    CommandOutput {
        id: String,
        #[serde(rename = "anchorId")]
        anchor_id: String,
        ts: String,
        content: HistoryContent,
        #[serde(rename = "firstSeq")]
        first_seq: u64,
        #[serde(rename = "lastSeq")]
        last_seq: u64,
    },
    Placeholder {
        id: String,
        #[serde(rename = "anchorId")]
        anchor_id: String,
        ts: String,
        content: HistoryContent,
        #[serde(rename = "firstSeq")]
        first_seq: u64,
        #[serde(rename = "lastSeq")]
        last_seq: u64,
    },
}

impl HistoryEntry {
    fn last_seq(&self) -> u64 {
        match self {
            Self::User { last_seq, .. }
            | Self::Assistant { last_seq, .. }
            | Self::Tool { last_seq, .. }
            | Self::CommandOutput { last_seq, .. }
            | Self::Placeholder { last_seq, .. } => *last_seq,
        }
    }

    fn first_seq(&self) -> u64 {
        match self {
            Self::User { first_seq, .. }
            | Self::Assistant { first_seq, .. }
            | Self::Tool { first_seq, .. }
            | Self::CommandOutput { first_seq, .. }
            | Self::Placeholder { first_seq, .. } => *first_seq,
        }
    }
}

fn flush_entries_page(
    page: &mut Vec<HistoryEntry>,
    page_bytes: &mut usize,
    page_count: &mut u64,
    pages_dir: &Path,
) -> Result<(), String> {
    if page.is_empty() {
        return Ok(());
    }
    *page_count += 1;
    let path = pages_dir.join(format!("{:08}.json", *page_count));
    let body = serde_json::to_vec(page).map_err(|e| e.to_string())?;
    if body.len() > PAGE_BYTE_LIMIT {
        return Err(format!("history page exceeds hard limit: {}", body.len()));
    }
    fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    page.clear();
    *page_bytes = 2;
    Ok(())
}

fn update_tool_page<F>(
    pages_dir: &Path,
    page_number: u64,
    tool_use_id: &str,
    update: F,
) -> Result<(), String>
where
    F: FnOnce(&mut HistoryEntry) -> Result<(), String>,
{
    let path = pages_dir.join(format!("{page_number:08}.json"));
    let body = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if body.len() > PAGE_BYTE_LIMIT {
        return Err(format!("history page exceeds hard limit: {}", body.len()));
    }
    let mut entries: Vec<HistoryEntry> =
        serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let entry = entries
        .iter_mut()
        .find(|entry| matches!(entry, HistoryEntry::Tool { tool_use_id: id, .. } if id == tool_use_id))
        .ok_or_else(|| format!("history tool page is missing tool {tool_use_id}"))?;
    update(entry)?;
    let updated = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
    if updated.len() > PAGE_BYTE_LIMIT {
        return Err(format!(
            "history page exceeds hard limit: {}",
            updated.len()
        ));
    }
    let temporary = pages_dir.join(format!("{page_number:08}.tmp"));
    fs::write(&temporary, updated).map_err(|e| format!("write {}: {e}", temporary.display()))?;
    replace_file(&temporary, &path).map_err(|e| format!("replace {}: {e}", path.display()))
}

impl SubHistoryState {
    fn new(
        root: &Path,
        parent_tool_use_id: &str,
        run_id: &str,
        generation_id: &str,
        blobs_dir: &Path,
    ) -> Result<Self, String> {
        let id = sanitize_id(parent_tool_use_id);
        let dir = root.join(&id);
        let pages_dir = dir.join("pages");
        let spools_dir = dir.join("spools");
        super::ensure_dir(&pages_dir).map_err(|e| e.to_string())?;
        super::ensure_dir(&spools_dir).map_err(|e| e.to_string())?;
        Ok(Self {
            id,
            run_id: run_id.to_string(),
            generation_id: generation_id.to_string(),
            blobs_dir: blobs_dir.to_path_buf(),
            pages_dir,
            pending_message: SpoolText::new(spools_dir.join("message.bin")),
            pending_thinking: SpoolText::new(spools_dir.join("thinking.bin")),
            spools_dir,
            page: Vec::new(),
            page_bytes: 2,
            page_count: 0,
            last_seq: 0,
            pending_message_seq: None,
            pending_thinking_seq: None,
            open_tools: HashMap::new(),
        })
    }

    fn push(&mut self, entry: HistoryEntry) -> Result<(), String> {
        let bytes = serde_json::to_vec(&entry).map_err(|e| e.to_string())?.len();
        if bytes > ENTRY_BYTE_LIMIT {
            return Err("subhistory entry exceeds hard limit".to_string());
        }
        if !self.page.is_empty()
            && (self.page.len() >= PAGE_ENTRY_LIMIT
                || self.page_bytes.saturating_add(bytes + 1) > PAGE_BYTE_LIMIT)
        {
            flush_entries_page(
                &mut self.page,
                &mut self.page_bytes,
                &mut self.page_count,
                &self.pages_dir,
            )?;
        }
        self.page_bytes += bytes + 1;
        self.last_seq = self.last_seq.max(entry.last_seq());
        self.page.push(entry);
        Ok(())
    }

    fn reset_pending_streams(&mut self) -> Result<(), String> {
        self.pending_message.discard()?;
        self.pending_thinking.discard()?;
        self.pending_message_seq = None;
        self.pending_thinking_seq = None;
        Ok(())
    }

    fn settle_interrupted_work(&mut self, last_seq: u64, tool_error: &str) -> Result<(), String> {
        if !self.pending_message.is_empty() || !self.pending_thinking.is_empty() {
            let seq = match (self.pending_message_seq, self.pending_thinking_seq) {
                (Some(message), Some(thinking)) => message.min(thinking),
                (Some(seq), None) | (None, Some(seq)) => seq,
                (None, None) => last_seq,
            };
            let id = format!("sub-assistant-incomplete-{seq}");
            let content = if self.pending_message.is_empty() {
                HistoryContent {
                    preview: String::new(),
                    byte_length: 0,
                    truncated: false,
                    content_id: None,
                    encoding: "text".to_string(),
                }
            } else {
                content_from_spool_at(
                    &self.run_id,
                    &self.generation_id,
                    &self.blobs_dir,
                    &format!("sub:{}:{id}:text", self.id),
                    std::mem::replace(
                        &mut self.pending_message,
                        SpoolText::new(self.spools_dir.join("message-next.bin")),
                    ),
                )?
            };
            let thinking = if self.pending_thinking.is_empty() {
                None
            } else {
                Some(content_from_spool_at(
                    &self.run_id,
                    &self.generation_id,
                    &self.blobs_dir,
                    &format!("sub:{}:{id}:thinking", self.id),
                    std::mem::replace(
                        &mut self.pending_thinking,
                        SpoolText::new(self.spools_dir.join("thinking-next.bin")),
                    ),
                )?)
            };
            self.pending_message_seq = None;
            self.pending_thinking_seq = None;
            self.push(HistoryEntry::Assistant {
                id: id.clone(),
                anchor_id: id,
                ts: String::new(),
                content,
                thinking,
                model: None,
                first_seq: seq,
                last_seq,
            })?;
        }
        for (tool_use_id, open) in std::mem::take(&mut self.open_tools) {
            update_tool_page(&self.pages_dir, open.page_number, &tool_use_id, |entry| {
                if let HistoryEntry::Tool {
                    status,
                    output,
                    last_seq: entry_last_seq,
                    ..
                } = entry
                {
                    *status = "error".to_string();
                    *output = json!({"error":tool_error});
                    *entry_last_seq = last_seq;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.settle_interrupted_work(self.last_seq, "Session ended before tool result")?;
        flush_entries_page(
            &mut self.page,
            &mut self.page_bytes,
            &mut self.page_count,
            &self.pages_dir,
        )?;
        fs::write(
            self.pages_dir.parent().unwrap().join("manifest.json"),
            serde_json::to_vec(&json!({"pageCount": self.page_count}))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySummary {
    pub run_id: String,
    pub generation_id: String,
    pub page_count: u64,
    pub total_entries: u64,
    pub total_turns: u64,
    pub last_seq: u64,
    pub source_size: u64,
    pub source_mtime_ns: u128,
    pub latest_cursor: Option<String>,
    #[serde(default)]
    pub state_events: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub run_id: String,
    pub generation_id: String,
    pub entries: Vec<HistoryEntry>,
    pub page_cursor: String,
    pub previous_cursor: Option<String>,
    pub has_more: bool,
    pub first_seq: u64,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentChunk {
    pub run_id: String,
    pub generation_id: String,
    pub content_id: String,
    pub offset: u64,
    pub next_offset: u64,
    pub total_bytes: u64,
    pub eof: bool,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubHistoryPage {
    pub run_id: String,
    pub generation_id: String,
    pub sub_history_id: String,
    pub entries: Vec<HistoryEntry>,
    pub page_cursor: String,
    pub previous_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentPointer {
    generation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryManifest {
    format_version: u32,
    builder_version: u32,
    generation_id: String,
    source_size: u64,
    source_mtime_ns: u128,
    source_prefix_hash: String,
    last_seq: u64,
    page_count: u64,
    total_entries: u64,
    total_turns: u64,
    complete: bool,
}

#[derive(Debug)]
struct OpenTool {
    page_number: u64,
    input_stream: Option<SpoolText>,
    output_stream: Option<SpoolText>,
}

#[derive(Debug, Clone)]
enum OpenToolScope {
    Main,
    Subhistory(String),
}

#[derive(Debug)]
struct SubHistoryState {
    id: String,
    run_id: String,
    generation_id: String,
    blobs_dir: PathBuf,
    pages_dir: PathBuf,
    spools_dir: PathBuf,
    page: Vec<HistoryEntry>,
    page_bytes: usize,
    page_count: u64,
    last_seq: u64,
    pending_message: SpoolText,
    pending_message_seq: Option<u64>,
    pending_thinking_seq: Option<u64>,
    pending_thinking: SpoolText,
    open_tools: HashMap<String, OpenTool>,
}

#[derive(Debug)]
struct SpoolText {
    inline: String,
    file: Option<File>,
    path: PathBuf,
    byte_length: u64,
}

impl SpoolText {
    fn new(path: PathBuf) -> Self {
        Self {
            inline: String::new(),
            file: None,
            path,
            byte_length: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.byte_length == 0
    }

    fn append(&mut self, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Ok(());
        }
        if self.file.is_none()
            && self.inline.len().saturating_add(value.len()) <= CONTENT_INLINE_LIMIT
        {
            self.inline.push_str(value);
            self.byte_length += value.len() as u64;
            return Ok(());
        }
        if self.file.is_none() {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&self.path)
                .map_err(|e| format!("create history spool: {e}"))?;
            file.write_all(self.inline.as_bytes())
                .map_err(|e| format!("write history spool: {e}"))?;
            self.inline.clear();
            self.file = Some(file);
        }
        self.file
            .as_mut()
            .expect("spool file initialized")
            .write_all(value.as_bytes())
            .map_err(|e| format!("append history spool: {e}"))?;
        self.byte_length += value.len() as u64;
        Ok(())
    }

    fn discard(&mut self) -> Result<(), String> {
        let path = self.path.clone();
        *self = Self::new(path.clone());
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("remove history spool: {e}"));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct BuildState {
    run_id: String,
    generation_id: String,
    generation_dir: PathBuf,
    pages_dir: PathBuf,
    blobs_dir: PathBuf,
    spools_dir: PathBuf,
    page: Vec<HistoryEntry>,
    page_bytes: usize,
    page_count: u64,
    total_entries: u64,
    total_turns: u64,
    last_seq: u64,
    state_events: HashMap<String, Value>,
    pending_interactions: HashMap<String, Value>,
    pending_user_inputs: HashMap<String, u64>,
    pending_message: SpoolText,
    pending_message_seq: Option<u64>,
    pending_thinking_seq: Option<u64>,
    pending_thinking: SpoolText,
    seen_message_ids: HashSet<String>,
    seen_tool_ids: HashSet<String>,
    open_tools: HashMap<String, OpenTool>,
    subhistories: HashMap<String, SubHistoryState>,
}

fn history_root(run_id: &str) -> PathBuf {
    super::run_dir(run_id).join("history-v1")
}

fn source_path(run_id: &str) -> PathBuf {
    super::run_dir(run_id).join("events.jsonl")
}

fn modified_ns(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn hash_prefix(path: &Path, source_size: u64) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    hash_file_prefix(&file, source_size)
}

fn hash_file_prefix(file: &File, source_size: u64) -> Result<String, String> {
    let mut file = file.try_clone().map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut remaining = source_size;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let take = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..take])
            .map_err(|e| format!("hash event prefix: {e}"))?;
        digest.update(&buffer[..take]);
        remaining -= take as u64;
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn cursor(generation_id: &str, page: u64) -> String {
    format!("{generation_id}:{page}")
}

fn parse_cursor(raw: &str) -> Result<(&str, u64), String> {
    let (generation, page) = raw
        .split_once(':')
        .ok_or_else(|| "invalid history cursor".to_string())?;
    if !is_hex_id(generation, 32) {
        return Err("invalid history cursor generation".to_string());
    }
    let page = page
        .parse::<u64>()
        .map_err(|_| "invalid history cursor page".to_string())?;
    if page == 0 {
        return Err("invalid history cursor page".to_string());
    }
    Ok((generation, page))
}

fn is_hex_id(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
enum ScannedLine {
    Eof,
    Inline {
        data: Vec<u8>,
        source_bytes: u64,
    },
    External {
        path: PathBuf,
        byte_length: u64,
        source_bytes: u64,
    },
}

#[derive(Default)]
struct SemanticBoundaryTracker {
    streaming_scopes: HashSet<String>,
    open_tools: HashSet<(String, String)>,
}

fn normalized_parent_tool_use_id(event: &Value) -> Option<&str> {
    event
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

impl SemanticBoundaryTracker {
    fn observe(&mut self, envelope: &Value) {
        if envelope.get("_bus").and_then(Value::as_bool) != Some(true) {
            return;
        }
        let Some(event) = envelope.get("event") else {
            return;
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let scope = normalized_parent_tool_use_id(event)
            .unwrap_or("__main")
            .to_string();
        if kind == "user_message" && scope == "__main" {
            self.streaming_scopes.clear();
            self.open_tools.clear();
            return;
        }
        if kind == "run_state"
            && matches!(
                event.get("state").and_then(Value::as_str),
                Some("spawning" | "running" | "idle" | "completed" | "failed" | "stopped")
            )
        {
            self.streaming_scopes.clear();
            self.open_tools.clear();
            return;
        }
        match kind {
            "message_delta" | "thinking_delta" => {
                self.streaming_scopes.insert(scope);
            }
            "message_complete" => {
                self.streaming_scopes.remove(&scope);
            }
            "tool_start" => {
                if let Some(tool_use_id) = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    self.open_tools.insert((scope, tool_use_id.to_string()));
                }
            }
            "tool_end" => {
                if let Some(tool_use_id) = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let exact = (scope, tool_use_id.to_string());
                    if !self.open_tools.remove(&exact) {
                        // Some upstream tool_end events omit parent_tool_use_id. Tool IDs are
                        // globally unique within a run, so match the frontend's recursive fallback.
                        self.open_tools.retain(|(_, id)| id != tool_use_id);
                    }
                }
            }
            _ => {}
        }
    }

    fn is_closed(&self) -> bool {
        self.streaming_scopes.is_empty() && self.open_tools.is_empty()
    }
}

fn safe_projection_size(file: &File, source_size: u64, spools_dir: &Path) -> Result<u64, String> {
    let mut reader = BufReader::new(
        file.try_clone()
            .map_err(|e| e.to_string())?
            .take(source_size),
    );
    let mut tracker = SemanticBoundaryTracker::default();
    let mut offset = 0u64;
    let mut safe_offset = 0u64;
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let external_path = spools_dir.join(format!("boundary-line-{line_no}.bin"));
        let (value, source_bytes) = match read_bounded_line(&mut reader, &external_path)? {
            ScannedLine::Eof => break,
            ScannedLine::Inline { data, source_bytes } => {
                let value = std::str::from_utf8(&data)
                    .ok()
                    .and_then(|line| serde_json::from_str::<Value>(line).ok());
                (value, source_bytes)
            }
            ScannedLine::External {
                path, source_bytes, ..
            } => {
                let metadata = oversized_metadata(&path).ok().map(|event| {
                    let seq = event.get("seq").cloned().unwrap_or(Value::Null);
                    json!({"_bus":true,"seq":seq,"event":event})
                });
                fs::remove_file(path).map_err(|e| e.to_string())?;
                (metadata, source_bytes)
            }
        };
        offset = offset.saturating_add(source_bytes);
        if let Some(value) = value.as_ref() {
            tracker.observe(value);
        }
        if tracker.is_closed() {
            safe_offset = offset;
        }
    }
    Ok(safe_offset)
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    external_path: &Path,
) -> Result<ScannedLine, String> {
    let mut inline = Vec::with_capacity(16 * 1024);
    let mut external: Option<File> = None;
    let mut byte_length = 0u64;
    loop {
        let available = reader
            .fill_buf()
            .map_err(|e| format!("read history line: {e}"))?;
        if available.is_empty() {
            return if byte_length == 0 {
                Ok(ScannedLine::Eof)
            } else if let Some(mut file) = external {
                file.flush().map_err(|e| e.to_string())?;
                Ok(ScannedLine::External {
                    path: external_path.to_path_buf(),
                    byte_length,
                    source_bytes: byte_length,
                })
            } else {
                Ok(ScannedLine::Inline {
                    data: inline,
                    source_bytes: byte_length,
                })
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let data_len = newline.unwrap_or(available.len());
        let data = &available[..data_len];
        if external.is_none() && inline.len().saturating_add(data.len()) > PARSE_LINE_LIMIT {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(external_path)
                .map_err(|e| format!("create oversized history event: {e}"))?;
            file.write_all(&inline).map_err(|e| e.to_string())?;
            inline.clear();
            external = Some(file);
        }
        if let Some(file) = external.as_mut() {
            file.write_all(data).map_err(|e| e.to_string())?;
        } else {
            inline.extend_from_slice(data);
        }
        byte_length += data.len() as u64;
        let consumed = data_len + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            if let Some(mut file) = external {
                file.flush().map_err(|e| e.to_string())?;
                return Ok(ScannedLine::External {
                    path: external_path.to_path_buf(),
                    byte_length,
                    source_bytes: byte_length + 1,
                });
            }
            if inline.last() == Some(&b'\r') {
                inline.pop();
            }
            return Ok(ScannedLine::Inline {
                data: inline,
                source_bytes: byte_length + 1,
            });
        }
    }
}

fn safe_utf8_prefix(value: &str, max: usize) -> &str {
    &value[..value.floor_char_boundary(max.min(value.len()))]
}

fn safe_utf8_suffix(value: &str, max: usize) -> &str {
    let start = value.len().saturating_sub(max);
    &value[value.ceil_char_boundary(start)..]
}

fn oversized_metadata(path: &Path) -> Result<Value, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut scanner = JsonMetadataScanner::new(file);
    let mut metadata = serde_json::Map::new();
    scanner.scan_envelope(&mut metadata)?;
    Ok(Value::Object(metadata))
}

struct JsonMetadataScanner<R: Read> {
    reader: BufReader<R>,
    pushed: Option<u8>,
}

impl<R: Read> JsonMetadataScanner<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pushed: None,
        }
    }

    fn byte(&mut self) -> Result<Option<u8>, String> {
        if let Some(byte) = self.pushed.take() {
            return Ok(Some(byte));
        }
        let available = self.reader.fill_buf().map_err(|e| e.to_string())?;
        let byte = available.first().copied();
        if byte.is_some() {
            self.reader.consume(1);
        }
        Ok(byte)
    }

    fn non_whitespace(&mut self) -> Result<Option<u8>, String> {
        loop {
            match self.byte()? {
                Some(byte) if byte.is_ascii_whitespace() => {}
                other => return Ok(other),
            }
        }
    }

    fn unread(&mut self, byte: u8) -> Result<(), String> {
        if self.pushed.replace(byte).is_some() {
            return Err("oversized JSON scanner pushback overflow".to_string());
        }
        Ok(())
    }

    fn string(&mut self, label: &str) -> Result<String, String> {
        let mut encoded = vec![b'"'];
        let mut escaped = false;
        loop {
            let byte = self
                .byte()?
                .ok_or_else(|| format!("unterminated oversized JSON {label}"))?;
            encoded.push(byte);
            if encoded.len() > MAX_JSON_SCALAR_BYTES {
                return Err(format!(
                    "oversized JSON {label} exceeds hard limit: {MAX_JSON_SCALAR_BYTES}"
                ));
            }
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return serde_json::from_slice(&encoded)
                    .map_err(|e| format!("invalid oversized JSON {label}: {e}"));
            }
        }
    }

    fn skip_string(&mut self) -> Result<(), String> {
        let mut escaped = false;
        loop {
            let byte = self
                .byte()?
                .ok_or_else(|| "unterminated oversized JSON string".to_string())?;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(());
            }
        }
    }

    fn scalar(&mut self, first: u8) -> Result<Value, String> {
        let mut encoded = vec![first];
        loop {
            match self.byte()? {
                Some(byte) if matches!(byte, b',' | b'}' | b']') => {
                    self.unread(byte)?;
                    break;
                }
                Some(byte) => {
                    encoded.push(byte);
                    if encoded.len() > MAX_JSON_SCALAR_BYTES {
                        return Err(format!(
                            "oversized JSON scalar exceeds hard limit: {MAX_JSON_SCALAR_BYTES}"
                        ));
                    }
                }
                None => break,
            }
        }
        while encoded.last().is_some_and(u8::is_ascii_whitespace) {
            encoded.pop();
        }
        serde_json::from_slice(&encoded).map_err(|e| format!("invalid oversized JSON scalar: {e}"))
    }

    fn skip_value(&mut self, first: u8) -> Result<(), String> {
        if first == b'"' {
            return self.skip_string();
        }
        if !matches!(first, b'{' | b'[') {
            self.scalar(first)?;
            return Ok(());
        }
        let mut stack = vec![if first == b'{' { b'}' } else { b']' }];
        let mut in_string = false;
        let mut escaped = false;
        while let Some(byte) = self.byte()? {
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    if stack.len() >= MAX_JSON_NESTING_DEPTH {
                        return Err(format!(
                            "oversized JSON metadata nesting exceeds hard limit: {MAX_JSON_NESTING_DEPTH}"
                        ));
                    }
                    stack.push(if byte == b'{' { b'}' } else { b']' });
                }
                b'}' | b']' => {
                    if stack.pop() != Some(byte) {
                        return Err("mismatched oversized JSON metadata delimiter".to_string());
                    }
                    if stack.is_empty() {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err("unterminated oversized JSON metadata value".to_string())
    }

    fn string_or_skip(&mut self, first: u8, label: &str) -> Result<Option<String>, String> {
        if first == b'"' {
            return self.string(label).map(Some);
        }
        self.skip_value(first)?;
        Ok(None)
    }

    fn bounded_value(&mut self, first: u8, label: &str) -> Result<Value, String> {
        if !matches!(first, b'{' | b'[') {
            return self.scalar(first);
        }
        let mut encoded = vec![first];
        let mut stack = vec![if first == b'{' { b'}' } else { b']' }];
        let mut in_string = false;
        let mut escaped = false;
        while let Some(byte) = self.byte()? {
            encoded.push(byte);
            if encoded.len() > ENTRY_BYTE_LIMIT {
                return Err(format!(
                    "oversized JSON {label} exceeds hard limit: {ENTRY_BYTE_LIMIT}"
                ));
            }
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    if stack.len() >= MAX_JSON_NESTING_DEPTH {
                        return Err(format!(
                            "oversized JSON metadata nesting exceeds hard limit: {MAX_JSON_NESTING_DEPTH}"
                        ));
                    }
                    stack.push(if byte == b'{' { b'}' } else { b']' });
                }
                b'}' | b']' => {
                    if stack.pop() != Some(byte) {
                        return Err("mismatched oversized JSON metadata delimiter".to_string());
                    }
                    if stack.is_empty() {
                        return serde_json::from_slice(&encoded)
                            .map_err(|e| format!("invalid oversized JSON {label}: {e}"));
                    }
                }
                _ => {}
            }
        }
        Err(format!("unterminated oversized JSON {label}"))
    }

    fn scan_event(
        &mut self,
        first: u8,
        metadata: &mut serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        if first != b'{' {
            return self.skip_value(first);
        }
        loop {
            let first = self
                .non_whitespace()?
                .ok_or_else(|| "unterminated oversized event object".to_string())?;
            if first == b'}' {
                return Ok(());
            }
            if first != b'"' {
                return Err("invalid oversized event metadata key".to_string());
            }
            let key = self.string("event metadata key")?;
            if self.non_whitespace()? != Some(b':') {
                return Err("invalid oversized event metadata separator".to_string());
            }
            let value_first = self
                .non_whitespace()?
                .ok_or_else(|| "missing oversized event metadata value".to_string())?;
            if matches!(
                key.as_str(),
                "type"
                    | "message_id"
                    | "tool_use_id"
                    | "tool_name"
                    | "status"
                    | "parent_tool_use_id"
                    | "uuid"
                    | "client_uuid"
                    | "model"
            ) {
                if let Some(value) = self.string_or_skip(value_first, "event metadata string")? {
                    metadata.insert(key, Value::String(value));
                }
            } else if key == "duration_ms" {
                if matches!(value_first, b'{' | b'[') {
                    self.skip_value(value_first)?;
                } else {
                    let value = self.scalar(value_first)?;
                    if let Some(duration_ms) = value.as_u64() {
                        metadata.insert(key, Value::Number(duration_ms.into()));
                    }
                }
            } else if key == "attachments" && value_first == b'[' {
                let value = self.bounded_value(value_first, "event attachments")?;
                metadata.insert(key, value);
            } else {
                self.skip_value(value_first)?;
            }
            match self.non_whitespace()? {
                Some(b',') => {}
                Some(b'}') => return Ok(()),
                _ => return Err("invalid oversized event metadata delimiter".to_string()),
            }
        }
    }

    fn scan_envelope(
        &mut self,
        metadata: &mut serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        if self.non_whitespace()? != Some(b'{') {
            return Err("oversized event envelope must be an object".to_string());
        }
        loop {
            let first = self
                .non_whitespace()?
                .ok_or_else(|| "unterminated oversized event envelope".to_string())?;
            if first == b'}' {
                return Ok(());
            }
            if first != b'"' {
                return Err("invalid oversized event envelope key".to_string());
            }
            let key = self.string("envelope key")?;
            if self.non_whitespace()? != Some(b':') {
                return Err("invalid oversized event envelope separator".to_string());
            }
            let value_first = self
                .non_whitespace()?
                .ok_or_else(|| "missing oversized event envelope value".to_string())?;
            match key.as_str() {
                "seq" => {
                    let value = self.scalar(value_first)?;
                    if let Some(seq) = value.as_u64() {
                        metadata.insert("seq".to_string(), Value::Number(seq.into()));
                    }
                }
                "ts" => {
                    if let Some(value) = self.string_or_skip(value_first, "timestamp")? {
                        metadata.insert("ts".to_string(), Value::String(value));
                    }
                }
                "event" => self.scan_event(value_first, metadata)?,
                _ => self.skip_value(value_first)?,
            }
            match self.non_whitespace()? {
                Some(b',') => {}
                Some(b'}') => return Ok(()),
                _ => return Err("invalid oversized event envelope delimiter".to_string()),
            }
        }
    }
}

fn extract_json_string_field(
    path: &Path,
    key: &str,
    spool_path: PathBuf,
) -> Result<Option<SpoolText>, String> {
    let mut bytes = BufReader::new(File::open(path).map_err(|e| e.to_string())?).bytes();
    let mut candidate = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut stack = Vec::new();
    #[allow(clippy::while_let_on_iterator)]
    while let Some(byte) = bytes.next() {
        let byte = byte.map_err(|e| e.to_string())?;
        if in_string {
            if escaped {
                escaped = false;
                if candidate.len() < 128 {
                    candidate.push(byte);
                }
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                if stack.len() == 2 && candidate == key.as_bytes() {
                    let mut separator = bytes
                        .by_ref()
                        .map(|value| value.map_err(|e| e.to_string()))
                        .find(|value| {
                            value.as_ref().is_err()
                                || !value.as_ref().unwrap().is_ascii_whitespace()
                        })
                        .transpose()?;
                    if separator == Some(b':') {
                        separator = bytes
                            .by_ref()
                            .map(|value| value.map_err(|e| e.to_string()))
                            .find(|value| {
                                value.as_ref().is_err()
                                    || !value.as_ref().unwrap().is_ascii_whitespace()
                            })
                            .transpose()?;
                    }
                    if separator == Some(b'"') {
                        return capture_json_string(&mut bytes, spool_path).map(Some);
                    }
                }
                candidate.clear();
            } else if candidate.len() < 128 {
                candidate.push(byte);
            }
        } else if byte == b'"' {
            candidate.clear();
            in_string = true;
        } else if matches!(byte, b'{' | b'[') {
            if stack.len() >= MAX_JSON_NESTING_DEPTH {
                return Err(format!(
                    "oversized JSON envelope nesting exceeds hard limit: {}",
                    MAX_JSON_NESTING_DEPTH
                ));
            }
            stack.push(if byte == b'{' { b'}' } else { b']' });
        } else if matches!(byte, b'}' | b']') && stack.pop() != Some(byte) {
            return Err("mismatched oversized JSON envelope delimiter".to_string());
        }
    }
    Ok(None)
}

fn extract_json_value_field(
    path: &Path,
    key: &str,
    spool_path: PathBuf,
) -> Result<Option<SpoolText>, String> {
    let mut bytes = BufReader::new(File::open(path).map_err(|e| e.to_string())?).bytes();
    let mut candidate = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut stack = Vec::new();
    #[allow(clippy::while_let_on_iterator)]
    while let Some(byte) = bytes.next() {
        let byte = byte.map_err(|e| e.to_string())?;
        if in_string {
            if escaped {
                escaped = false;
                if candidate.len() < 128 {
                    candidate.push(byte);
                }
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                if stack.len() == 2 && candidate == key.as_bytes() {
                    let separator = bytes
                        .by_ref()
                        .map(|value| value.map_err(|e| e.to_string()))
                        .find(|value| {
                            value.as_ref().is_err()
                                || !value.as_ref().unwrap().is_ascii_whitespace()
                        })
                        .transpose()?;
                    if separator == Some(b':') {
                        let first = bytes
                            .by_ref()
                            .map(|value| value.map_err(|e| e.to_string()))
                            .find(|value| {
                                value.as_ref().is_err()
                                    || !value.as_ref().unwrap().is_ascii_whitespace()
                            })
                            .transpose()?;
                        if let Some(first) = first {
                            return capture_json_value(&mut bytes, first, spool_path).map(Some);
                        }
                    }
                }
                candidate.clear();
            } else if candidate.len() < 128 {
                candidate.push(byte);
            }
        } else if byte == b'"' {
            candidate.clear();
            in_string = true;
        } else if matches!(byte, b'{' | b'[') {
            if stack.len() >= MAX_JSON_NESTING_DEPTH {
                return Err(format!(
                    "oversized JSON envelope nesting exceeds hard limit: {}",
                    MAX_JSON_NESTING_DEPTH
                ));
            }
            stack.push(if byte == b'{' { b'}' } else { b']' });
        } else if matches!(byte, b'}' | b']') && stack.pop() != Some(byte) {
            return Err("mismatched oversized JSON envelope delimiter".to_string());
        }
    }
    Ok(None)
}

fn capture_json_value<I>(bytes: &mut I, first: u8, spool_path: PathBuf) -> Result<SpoolText, String>
where
    I: Iterator<Item = std::io::Result<u8>>,
{
    let mut spool = SpoolText::new(spool_path);
    let mut output = Vec::with_capacity(16 * 1024);
    output.push(first);
    let mut stack = match first {
        b'{' => vec![b'}'],
        b'[' => vec![b']'],
        _ => Vec::new(),
    };
    let mut in_string = first == b'"';
    let mut escaped = false;
    let scalar = stack.is_empty() && !in_string;
    let mut scalar_bytes = usize::from(scalar);
    for byte in bytes.by_ref() {
        let byte = byte.map_err(|e| e.to_string())?;
        if in_string {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                if stack.is_empty() {
                    flush_utf8_bytes(&mut spool, &mut output, true)?;
                    return Ok(spool);
                }
            }
        } else if byte == b'"' {
            in_string = true;
            output.push(byte);
        } else if matches!(byte, b'{' | b'[') {
            if stack.len() >= MAX_JSON_NESTING_DEPTH {
                return Err(format!(
                    "oversized JSON value nesting exceeds hard limit: {}",
                    MAX_JSON_NESTING_DEPTH
                ));
            }
            stack.push(if byte == b'{' { b'}' } else { b']' });
            output.push(byte);
        } else if matches!(byte, b'}' | b']') {
            if stack.is_empty() {
                if scalar {
                    flush_utf8_bytes(&mut spool, &mut output, true)?;
                    return Ok(spool);
                }
                return Err("unbalanced oversized JSON value".to_string());
            }
            if stack.pop() != Some(byte) {
                return Err("mismatched oversized JSON value delimiter".to_string());
            }
            output.push(byte);
            if stack.is_empty() {
                flush_utf8_bytes(&mut spool, &mut output, true)?;
                return Ok(spool);
            }
        } else if scalar && matches!(byte, b',' | b'}' | b']') {
            while output.last().is_some_and(u8::is_ascii_whitespace) {
                output.pop();
            }
            validate_json_scalar(&output)?;
            flush_utf8_bytes(&mut spool, &mut output, true)?;
            return Ok(spool);
        } else {
            output.push(byte);
            if scalar {
                scalar_bytes += 1;
                if scalar_bytes > MAX_JSON_SCALAR_BYTES {
                    return Err(format!(
                        "oversized JSON scalar exceeds hard limit: {}",
                        MAX_JSON_SCALAR_BYTES
                    ));
                }
            }
        }
        if !scalar && output.len() >= 16 * 1024 {
            flush_utf8_bytes(&mut spool, &mut output, false)?;
        }
    }
    if scalar {
        while output.last().is_some_and(u8::is_ascii_whitespace) {
            output.pop();
        }
        validate_json_scalar(&output)?;
        flush_utf8_bytes(&mut spool, &mut output, true)?;
        return Ok(spool);
    }
    Err("unterminated oversized JSON value".to_string())
}

fn validate_json_scalar(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| format!("invalid UTF-8 in oversized JSON scalar: {e}"))?;
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid oversized JSON scalar: {e}"))?;
    if value.is_array() || value.is_object() || value.is_string() {
        return Err("invalid oversized JSON scalar".to_string());
    }
    Ok(())
}

fn capture_json_string<I>(bytes: &mut I, spool_path: PathBuf) -> Result<SpoolText, String>
where
    I: Iterator<Item = std::io::Result<u8>>,
{
    let mut spool = SpoolText::new(spool_path);
    let mut output = Vec::with_capacity(16 * 1024);
    let mut escaped = false;
    let mut unicode: Option<String> = None;
    let mut high_surrogate = None;
    for byte in bytes.by_ref() {
        let byte = byte.map_err(|e| e.to_string())?;
        if let Some(digits) = unicode.as_mut() {
            digits.push(byte as char);
            if digits.len() == 4 {
                let value = u16::from_str_radix(digits, 16)
                    .map_err(|e| format!("decode oversized JSON string: {e}"))?;
                append_json_utf16_unit(&mut output, &mut high_surrogate, value)?;
                unicode = None;
                escaped = false;
            }
        } else if escaped {
            if high_surrogate.is_some() && byte != b'u' {
                return Err("unpaired high surrogate in oversized JSON string".to_string());
            }
            match byte {
                b'"' | b'\\' | b'/' => output.push(byte),
                b'b' => output.push(8),
                b'f' => output.push(12),
                b'n' => output.push(b'\n'),
                b'r' => output.push(b'\r'),
                b't' => output.push(b'\t'),
                b'u' => unicode = Some(String::with_capacity(4)),
                _ => return Err("invalid escape in oversized JSON string".to_string()),
            }
            if byte != b'u' {
                escaped = false;
            }
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            if high_surrogate.is_some() {
                return Err("unpaired high surrogate in oversized JSON string".to_string());
            }
            flush_utf8_bytes(&mut spool, &mut output, true)?;
            return Ok(spool);
        } else {
            if high_surrogate.is_some() {
                return Err("unpaired high surrogate in oversized JSON string".to_string());
            }
            output.push(byte);
        }
        if output.len() >= 16 * 1024 {
            flush_utf8_bytes(&mut spool, &mut output, false)?;
        }
    }
    Err("unterminated oversized JSON string".to_string())
}

fn append_json_utf16_unit(
    output: &mut Vec<u8>,
    high_surrogate: &mut Option<u16>,
    value: u16,
) -> Result<(), String> {
    if (0xD800..=0xDBFF).contains(&value) {
        if high_surrogate.replace(value).is_some() {
            return Err("consecutive high surrogates in oversized JSON string".to_string());
        }
        return Ok(());
    }
    let scalar = if (0xDC00..=0xDFFF).contains(&value) {
        let high = high_surrogate
            .take()
            .ok_or_else(|| "unpaired low surrogate in oversized JSON string".to_string())?;
        0x10000 + (((high as u32 - 0xD800) << 10) | (value as u32 - 0xDC00))
    } else {
        if high_surrogate.is_some() {
            return Err("unpaired high surrogate in oversized JSON string".to_string());
        }
        value as u32
    };
    let ch = char::from_u32(scalar)
        .ok_or_else(|| "invalid Unicode scalar in oversized JSON string".to_string())?;
    let mut encoded = [0u8; 4];
    output.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
    Ok(())
}

fn flush_utf8_bytes(
    spool: &mut SpoolText,
    output: &mut Vec<u8>,
    final_chunk: bool,
) -> Result<(), String> {
    let valid = match std::str::from_utf8(output) {
        Ok(_) => output.len(),
        Err(error) if !final_chunk && error.error_len().is_none() => error.valid_up_to(),
        Err(error) => return Err(format!("invalid UTF-8 in oversized JSON string: {error}")),
    };
    if valid > 0 {
        let text = std::str::from_utf8(&output[..valid]).map_err(|e| e.to_string())?;
        spool.append(text)?;
        output.drain(..valid);
    }
    Ok(())
}

fn preview_file(path: &Path, byte_length: u64) -> Result<String, String> {
    use std::io::{Seek, SeekFrom};

    let mut file = File::open(path).map_err(|e| format!("open history content preview: {e}"))?;
    let mut head_bytes = vec![0u8; PREVIEW_HEAD_BYTES.saturating_add(4)];
    let head_len = file
        .read(&mut head_bytes)
        .map_err(|e| format!("read history content preview: {e}"))?;
    head_bytes.truncate(head_len);
    let head_text = String::from_utf8_lossy(&head_bytes);
    let head = safe_utf8_prefix(&head_text, PREVIEW_HEAD_BYTES).to_string();

    let tail_offset = byte_length.saturating_sub((PREVIEW_TAIL_BYTES + 4) as u64);
    file.seek(SeekFrom::Start(tail_offset))
        .map_err(|e| format!("seek history content preview: {e}"))?;
    let mut tail_bytes = Vec::with_capacity((byte_length - tail_offset) as usize);
    file.read_to_end(&mut tail_bytes)
        .map_err(|e| format!("read history content preview: {e}"))?;
    let tail_text = String::from_utf8_lossy(&tail_bytes);
    let tail = safe_utf8_suffix(&tail_text, PREVIEW_TAIL_BYTES).to_string();
    let omitted = byte_length.saturating_sub((head.len() + tail.len()) as u64);
    Ok(format!("{head}\n… {omitted} bytes omitted …\n{tail}"))
}

fn sanitize_id(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn content_from_text_at(
    run_id: &str,
    generation_id: &str,
    blobs_dir: &Path,
    key: &str,
    value: &str,
) -> Result<HistoryContent, String> {
    if value.len() <= CONTENT_INLINE_LIMIT {
        return Ok(HistoryContent {
            preview: value.to_string(),
            byte_length: value.len() as u64,
            truncated: false,
            content_id: None,
            encoding: "text".to_string(),
        });
    }
    let content_id = sanitize_id(&format!("{run_id}:{generation_id}:{key}"));
    fs::write(
        blobs_dir.join(format!("{content_id}.bin")),
        value.as_bytes(),
    )
    .map_err(|e| format!("write history content: {e}"))?;
    let head = safe_utf8_prefix(value, PREVIEW_HEAD_BYTES);
    let tail_start = value
        .ceil_char_boundary(value.len().saturating_sub(PREVIEW_TAIL_BYTES))
        .max(head.len());
    Ok(HistoryContent {
        preview: format!(
            "{}\n… {} bytes omitted …\n{}",
            head,
            tail_start.saturating_sub(head.len()),
            &value[tail_start..]
        ),
        byte_length: value.len() as u64,
        truncated: true,
        content_id: Some(content_id),
        encoding: "text".to_string(),
    })
}

fn content_from_spool_at(
    run_id: &str,
    generation_id: &str,
    blobs_dir: &Path,
    key: &str,
    mut spool: SpoolText,
) -> Result<HistoryContent, String> {
    if spool.file.is_none() {
        return content_from_text_at(run_id, generation_id, blobs_dir, key, &spool.inline);
    }
    if let Some(mut file) = spool.file.take() {
        file.flush()
            .map_err(|e| format!("flush history spool: {e}"))?;
        file.sync_data()
            .map_err(|e| format!("sync history spool: {e}"))?;
    }
    let content_id = sanitize_id(&format!("{run_id}:{generation_id}:{key}"));
    let blob_path = blobs_dir.join(format!("{content_id}.bin"));
    fs::rename(&spool.path, &blob_path).map_err(|e| format!("publish history spool: {e}"))?;
    Ok(HistoryContent {
        preview: preview_file(&blob_path, spool.byte_length)?,
        byte_length: spool.byte_length,
        truncated: true,
        content_id: Some(content_id),
        encoding: "text".to_string(),
    })
}

fn publish_external_content(
    run_id: &str,
    generation_id: &str,
    blobs_dir: &Path,
    key: &str,
    path: &Path,
    byte_length: u64,
) -> Result<HistoryContent, String> {
    let content_id = sanitize_id(&format!("{run_id}:{generation_id}:{key}"));
    let blob_path = blobs_dir.join(format!("{content_id}.bin"));
    fs::rename(path, &blob_path).map_err(|e| format!("publish history content: {e}"))?;
    Ok(HistoryContent {
        preview: preview_file(&blob_path, byte_length)?,
        byte_length,
        truncated: true,
        content_id: Some(content_id),
        encoding: "json".to_string(),
    })
}

fn bounded_json_at(
    run_id: &str,
    generation_id: &str,
    blobs_dir: &Path,
    key: &str,
    value: &Value,
) -> Result<(Value, Option<HistoryContent>), String> {
    let encoded = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if encoded.len() <= CONTENT_INLINE_LIMIT {
        return Ok((value.clone(), None));
    }
    let content_id = sanitize_id(&format!("{run_id}:{generation_id}:{key}"));
    fs::write(blobs_dir.join(format!("{content_id}.bin")), &encoded)
        .map_err(|e| format!("write history json content: {e}"))?;
    let text = String::from_utf8_lossy(&encoded);
    let preview = safe_utf8_prefix(&text, PREVIEW_HEAD_BYTES).to_string();
    Ok((
        json!({"_truncated": true, "preview": preview}),
        Some(HistoryContent {
            preview,
            byte_length: encoded.len() as u64,
            truncated: true,
            content_id: Some(content_id),
            encoding: "json".to_string(),
        }),
    ))
}

fn bounded_json_spool_at(
    run_id: &str,
    generation_id: &str,
    blobs_dir: &Path,
    key: &str,
    spool: SpoolText,
) -> Result<(Value, Option<HistoryContent>), String> {
    if spool.file.is_none() {
        let value = serde_json::from_str(&spool.inline)
            .map_err(|e| format!("parse extracted oversized JSON value: {e}"))?;
        return Ok((value, None));
    }
    let mut content = content_from_spool_at(run_id, generation_id, blobs_dir, key, spool)?;
    content.encoding = "json".to_string();
    Ok((
        json!({"_truncated":true,"preview":content.preview}),
        Some(content),
    ))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|e| e.to_string())
    }
}

impl BuildState {
    fn open_tool_scope(&self, tool_use_id: &str) -> Option<OpenToolScope> {
        if self.open_tools.contains_key(tool_use_id) {
            return Some(OpenToolScope::Main);
        }
        self.subhistories
            .iter()
            .find_map(|(parent_id, subhistory)| {
                subhistory
                    .open_tools
                    .contains_key(tool_use_id)
                    .then(|| OpenToolScope::Subhistory(parent_id.clone()))
            })
    }

    fn open_tool_page(&self, scope: &OpenToolScope, tool_use_id: &str) -> Option<u64> {
        match scope {
            OpenToolScope::Main => self.open_tools.get(tool_use_id),
            OpenToolScope::Subhistory(parent_id) => self
                .subhistories
                .get(parent_id)
                .and_then(|subhistory| subhistory.open_tools.get(tool_use_id)),
        }
        .map(|open| open.page_number)
    }

    fn new(run_id: &str, generation_id: &str, generation_dir: PathBuf) -> Result<Self, String> {
        let pages_dir = generation_dir.join("pages");
        let blobs_dir = generation_dir.join("blobs");
        let spools_dir = generation_dir.join("spools");
        super::ensure_dir(&pages_dir).map_err(|e| e.to_string())?;
        super::ensure_dir(&blobs_dir).map_err(|e| e.to_string())?;
        super::ensure_dir(&spools_dir).map_err(|e| e.to_string())?;
        Ok(Self {
            run_id: run_id.to_string(),
            generation_id: generation_id.to_string(),
            generation_dir,
            pages_dir,
            blobs_dir,
            pending_message: SpoolText::new(spools_dir.join("assistant-message.bin")),
            pending_thinking: SpoolText::new(spools_dir.join("assistant-thinking.bin")),
            spools_dir,
            page: Vec::new(),
            page_bytes: 2,
            page_count: 0,
            total_entries: 0,
            total_turns: 0,
            last_seq: 0,
            state_events: HashMap::new(),
            pending_interactions: HashMap::new(),
            pending_user_inputs: HashMap::new(),
            pending_message_seq: None,
            pending_thinking_seq: None,
            seen_message_ids: HashSet::new(),
            seen_tool_ids: HashSet::new(),
            open_tools: HashMap::new(),
            subhistories: HashMap::new(),
        })
    }

    fn reset_pending_streams(&mut self) -> Result<(), String> {
        self.pending_message.discard()?;
        self.pending_thinking.discard()?;
        self.pending_message_seq = None;
        self.pending_thinking_seq = None;
        Ok(())
    }

    fn settle_interrupted_work(&mut self, last_seq: u64, tool_error: &str) -> Result<(), String> {
        if !self.pending_message.is_empty() || !self.pending_thinking.is_empty() {
            let seq = match (self.pending_message_seq, self.pending_thinking_seq) {
                (Some(message), Some(thinking)) => message.min(thinking),
                (Some(seq), None) | (None, Some(seq)) => seq,
                (None, None) => last_seq,
            };
            log::debug!(
                "[history] settle interrupted assistant: run_id={}, first_seq={}, last_seq={}",
                self.run_id,
                seq,
                last_seq
            );
            let id = format!("assistant-incomplete-{seq}");
            let content = if self.pending_message.is_empty() {
                HistoryContent {
                    preview: String::new(),
                    byte_length: 0,
                    truncated: false,
                    content_id: None,
                    encoding: "text".to_string(),
                }
            } else {
                let pending = std::mem::replace(
                    &mut self.pending_message,
                    SpoolText::new(self.spools_dir.join("assistant-message-next.bin")),
                );
                self.content_from_spool(&format!("{id}:text"), pending)?
            };
            let thinking = if self.pending_thinking.is_empty() {
                None
            } else {
                let pending = std::mem::replace(
                    &mut self.pending_thinking,
                    SpoolText::new(self.spools_dir.join("assistant-thinking-next.bin")),
                );
                Some(self.content_from_spool(&format!("{id}:thinking"), pending)?)
            };
            self.pending_message_seq = None;
            self.pending_thinking_seq = None;
            self.push(HistoryEntry::Assistant {
                id: id.clone(),
                anchor_id: id,
                ts: String::new(),
                content,
                thinking,
                model: None,
                first_seq: seq,
                last_seq,
            })?;
        }
        if !self.open_tools.is_empty() {
            log::debug!(
                "[history] settle interrupted tools: run_id={}, count={}, last_seq={}",
                self.run_id,
                self.open_tools.len(),
                last_seq
            );
        }
        for (tool_use_id, open) in std::mem::take(&mut self.open_tools) {
            update_tool_page(&self.pages_dir, open.page_number, &tool_use_id, |entry| {
                if let HistoryEntry::Tool {
                    status,
                    output,
                    last_seq: entry_last_seq,
                    ..
                } = entry
                {
                    *status = "error".to_string();
                    *output = json!({"error":tool_error});
                    *entry_last_seq = last_seq;
                }
                Ok(())
            })?;
        }
        for subhistory in self.subhistories.values_mut() {
            subhistory.settle_interrupted_work(last_seq, tool_error)?;
        }
        Ok(())
    }

    fn content_from_text(&self, key: &str, value: &str) -> Result<HistoryContent, String> {
        content_from_text_at(
            &self.run_id,
            &self.generation_id,
            &self.blobs_dir,
            key,
            value,
        )
    }

    fn content_from_spool(&self, key: &str, spool: SpoolText) -> Result<HistoryContent, String> {
        content_from_spool_at(
            &self.run_id,
            &self.generation_id,
            &self.blobs_dir,
            key,
            spool,
        )
    }

    fn bounded_json(
        &self,
        key: &str,
        value: &Value,
    ) -> Result<(Value, Option<HistoryContent>), String> {
        bounded_json_at(
            &self.run_id,
            &self.generation_id,
            &self.blobs_dir,
            key,
            value,
        )
    }

    fn push(&mut self, mut entry: HistoryEntry) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&entry).map_err(|e| e.to_string())?;
        if encoded.len() > ENTRY_BYTE_LIMIT {
            let first_seq = entry.first_seq();
            let last_seq = entry.last_seq();
            let id = format!("oversized-entry-{first_seq}-{last_seq}");
            let content_id = sanitize_id(&format!("{}:{}:{}", self.run_id, self.generation_id, id));
            fs::write(self.blobs_dir.join(format!("{content_id}.bin")), &encoded)
                .map_err(|e| format!("write oversized history entry: {e}"))?;
            let text = String::from_utf8_lossy(&encoded);
            entry = HistoryEntry::Placeholder {
                id: id.clone(),
                anchor_id: id,
                ts: String::new(),
                content: HistoryContent {
                    preview: safe_utf8_prefix(&text, PREVIEW_HEAD_BYTES).to_string(),
                    byte_length: encoded.len() as u64,
                    truncated: true,
                    content_id: Some(content_id),
                    encoding: "json".to_string(),
                },
                first_seq,
                last_seq,
            };
            encoded = serde_json::to_vec(&entry).map_err(|e| e.to_string())?;
            log::warn!(
                "[history] entry projected as placeholder: run_id={}, first_seq={}, last_seq={}, bytes={}",
                self.run_id,
                first_seq,
                last_seq,
                encoded.len()
            );
        }
        let entry_bytes = encoded.len();
        if !self.page.is_empty()
            && (self.page.len() >= PAGE_ENTRY_LIMIT
                || self.page_bytes.saturating_add(entry_bytes + 1) > PAGE_BYTE_LIMIT)
        {
            self.flush_page()?;
        }
        self.page_bytes += entry_bytes + 1;
        self.last_seq = self.last_seq.max(entry.last_seq());
        self.page.push(entry);
        self.total_entries += 1;
        Ok(())
    }

    fn is_duplicate_identity(&self, kind: &str, event: &Value) -> bool {
        match kind {
            "message_complete" => event
                .get("message_id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && self.seen_message_ids.contains(id)),
            "tool_start" => event
                .get("tool_use_id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && self.seen_tool_ids.contains(id)),
            _ => false,
        }
    }

    fn record_identity(&mut self, kind: &str, event: &Value) -> bool {
        match kind {
            "message_complete" => event
                .get("message_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .is_none_or(|id| self.seen_message_ids.insert(id.to_string())),
            "tool_start" => event
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .is_none_or(|id| self.seen_tool_ids.insert(id.to_string())),
            _ => true,
        }
    }

    fn flush_page(&mut self) -> Result<(), String> {
        if self.page.is_empty() {
            return Ok(());
        }
        self.page_count += 1;
        let path = self.pages_dir.join(format!("{:08}.json", self.page_count));
        let body = serde_json::to_vec(&self.page).map_err(|e| e.to_string())?;
        if body.len() > PAGE_BYTE_LIMIT {
            return Err(format!("history page exceeds hard limit: {}", body.len()));
        }
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
        self.page.clear();
        self.page_bytes = 2;
        Ok(())
    }

    fn handle_event(&mut self, envelope: &Value) -> Result<(), String> {
        if envelope.get("_bus").and_then(Value::as_bool) != Some(true) {
            return self.handle_legacy_event(envelope);
        }
        let seq = envelope.get("seq").and_then(Value::as_u64).unwrap_or(0);
        let ts = envelope
            .get("ts")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(event) = envelope.get("event") else {
            return Ok(());
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        self.last_seq = self.last_seq.max(seq);
        let closes_previous_turn = (kind == "user_message"
            && normalized_parent_tool_use_id(event).is_none())
            || (kind == "run_state"
                && matches!(
                    event.get("state").and_then(Value::as_str),
                    Some("spawning" | "running" | "idle" | "completed" | "failed" | "stopped")
                ));
        if closes_previous_turn {
            self.settle_interrupted_work(seq, "Turn ended before tool result")?;
        }
        // The live reducer treats upstream assistant/tool IDs as idempotency keys. Preserve that
        // contract across page boundaries so projection entries remain valid Svelte keyed-list
        // identities even when CLI import appends an overlapping event range.
        if !self.record_identity(kind, event) {
            if kind == "message_complete" {
                if let Some(parent_tool_use_id) = normalized_parent_tool_use_id(event) {
                    if let Some(subhistory) = self.subhistories.get_mut(parent_tool_use_id) {
                        subhistory.reset_pending_streams()?;
                    }
                } else {
                    self.reset_pending_streams()?;
                }
            }
            log::debug!(
                "[history] duplicate event omitted: run_id={}, type={}, seq={}",
                self.run_id,
                kind,
                seq
            );
            return Ok(());
        }
        if let Some(parent_tool_use_id) = normalized_parent_tool_use_id(event) {
            return self.handle_subhistory_event(parent_tool_use_id, seq, ts, event);
        }
        if kind == "tool_end" {
            let tool_use_id = event
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(OpenToolScope::Subhistory(parent_tool_use_id)) =
                self.open_tool_scope(tool_use_id)
            {
                log::debug!(
                    "[history] route tool_end with missing parent: run_id={}, tool_use_id={}, parent_tool_use_id={}",
                    self.run_id,
                    tool_use_id,
                    parent_tool_use_id
                );
                return self.handle_subhistory_event(&parent_tool_use_id, seq, ts, event);
            }
        }
        match kind {
            "user_message" => {
                self.total_turns += 1;
                let text = event.get("text").and_then(Value::as_str).unwrap_or("");
                let cli_uuid = event
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let anchor_id = cli_uuid
                    .clone()
                    .or_else(|| {
                        event
                            .get("client_uuid")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("user-{seq}"));
                // A CLI UUID is a rewind anchor, not a render identity: imported overlap can
                // legitimately repeat it. Sequence-derived IDs retain both user events while
                // keeping the paged timeline's keyed-list identity globally unique.
                let id = format!("user-{seq}");
                let content = self.content_from_text(&format!("user:{id}"), text)?;
                self.push(HistoryEntry::User {
                    anchor_id,
                    id,
                    ts,
                    content,
                    cli_uuid,
                    attachments: event
                        .get("attachments")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    first_seq: seq,
                    last_seq: seq,
                })?;
            }
            "message_delta" if normalized_parent_tool_use_id(event).is_none() => {
                if self.pending_message_seq.is_none() {
                    self.pending_message_seq = Some(seq);
                }
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    self.pending_message.append(text)?;
                }
            }
            "thinking_delta" if normalized_parent_tool_use_id(event).is_none() => {
                self.pending_thinking_seq.get_or_insert(seq);
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    self.pending_thinking.append(text)?;
                }
            }
            "message_complete" if normalized_parent_tool_use_id(event).is_none() => {
                let id = event
                    .get("message_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("assistant-{seq}"));
                let content = if let Some(text) = event.get("text").and_then(Value::as_str) {
                    self.content_from_text(&format!("assistant:{id}:text"), text)?
                } else {
                    let pending = std::mem::replace(
                        &mut self.pending_message,
                        SpoolText::new(self.spools_dir.join("assistant-message.bin")),
                    );
                    self.content_from_spool(&format!("assistant:{id}:text"), pending)?
                };
                let thinking = if self.pending_thinking.is_empty() {
                    None
                } else {
                    let pending = std::mem::replace(
                        &mut self.pending_thinking,
                        SpoolText::new(self.spools_dir.join("assistant-thinking.bin")),
                    );
                    Some(self.content_from_spool(&format!("assistant:{id}:thinking"), pending)?)
                };
                self.push(HistoryEntry::Assistant {
                    anchor_id: id.clone(),
                    id,
                    ts,
                    content,
                    thinking,
                    model: event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    first_seq: match (self.pending_message_seq, self.pending_thinking_seq) {
                        (Some(message), Some(thinking)) => message.min(thinking),
                        (Some(first), None) | (None, Some(first)) => first,
                        (None, None) => seq,
                    },
                    last_seq: seq,
                })?;
                self.reset_pending_streams()?;
            }
            "tool_start" if normalized_parent_tool_use_id(event).is_none() => {
                let tool_use_id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if tool_use_id.is_empty() {
                    return Ok(());
                }
                let input_raw = event.get("input").cloned().unwrap_or(Value::Null);
                let (input, input_content) =
                    self.bounded_json(&format!("tool:{tool_use_id}:input"), &input_raw)?;
                let entry = HistoryEntry::Tool {
                    id: tool_use_id.clone(),
                    anchor_id: tool_use_id.clone(),
                    ts,
                    tool_use_id: tool_use_id.clone(),
                    tool_name: event
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    status: "running".to_string(),
                    input,
                    output: Value::Null,
                    tool_use_result: None,
                    input_content,
                    output_content: None,
                    result_content: None,
                    duration_ms: None,
                    sub_history_id: None,
                    first_seq: seq,
                    last_seq: seq,
                };
                if self.open_tools.len() >= MAX_OPEN_TOOLS {
                    return Err(format!(
                        "history open tool limit exceeded: {}",
                        MAX_OPEN_TOOLS
                    ));
                }
                let input_stream = event
                    .get("input")
                    .filter(|value| !value.is_null())
                    .is_none()
                    .then(|| {
                        SpoolText::new(
                            self.spools_dir
                                .join(format!("tool-input-{tool_use_id}.bin")),
                        )
                    });
                // A tool owns its page so completion can atomically patch a bounded record while
                // preserving tool-start order even when tools finish out of order.
                self.flush_page()?;
                self.push(entry)?;
                self.flush_page()?;
                self.open_tools.insert(
                    tool_use_id,
                    OpenTool {
                        page_number: self.page_count,
                        input_stream,
                        output_stream: None,
                    },
                );
            }
            "tool_input_delta" if normalized_parent_tool_use_id(event).is_none() => {
                let tool_use_id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(open) = self.open_tools.get_mut(tool_use_id) {
                    if open.input_stream.is_none() {
                        open.input_stream = Some(SpoolText::new(
                            self.spools_dir
                                .join(format!("tool-input-{tool_use_id}.bin")),
                        ));
                    }
                    if let Some(partial) = event.get("partial_json").and_then(Value::as_str) {
                        open.input_stream.as_mut().unwrap().append(partial)?;
                    }
                }
            }
            "tool_output_delta" if normalized_parent_tool_use_id(event).is_none() => {
                let tool_use_id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(open) = self.open_tools.get_mut(tool_use_id) {
                    if open.output_stream.is_none() {
                        open.output_stream = Some(SpoolText::new(
                            self.spools_dir
                                .join(format!("tool-output-{tool_use_id}.bin")),
                        ));
                    }
                    if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                        open.output_stream.as_mut().unwrap().append(delta)?;
                    }
                }
            }
            "tool_end" if normalized_parent_tool_use_id(event).is_none() => {
                let tool_use_id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(mut open) = self.open_tools.remove(tool_use_id) {
                    let mut streamed_input = None;
                    if let Some(input_stream) = open.input_stream.take() {
                        if !input_stream.is_empty() {
                            let parsed = if input_stream.file.is_none() {
                                serde_json::from_str::<Value>(&input_stream.inline).ok()
                            } else {
                                None
                            };
                            let mut input_content = self.content_from_spool(
                                &format!("tool:{tool_use_id}:input-stream"),
                                input_stream,
                            )?;
                            input_content.encoding = "json".to_string();
                            let input = parsed.unwrap_or_else(
                                || json!({"_truncated": true, "preview": input_content.preview}),
                            );
                            streamed_input =
                                Some((input, input_content.truncated.then_some(input_content)));
                        }
                    }
                    let streamed_output =
                        open.output_stream.take().filter(|spool| !spool.is_empty());
                    let output_raw = event.get("output").cloned().unwrap_or(Value::Null);
                    let result_raw = event.get("tool_use_result").cloned();
                    let (output, output_content) = if output_raw.is_null() {
                        if let Some(output_stream) = streamed_output {
                            let content = self.content_from_spool(
                                &format!("tool:{tool_use_id}:output-stream"),
                                output_stream,
                            )?;
                            (
                                Value::String(content.preview.clone()),
                                content.truncated.then_some(content),
                            )
                        } else {
                            (Value::Null, None)
                        }
                    } else {
                        self.bounded_json(&format!("tool:{tool_use_id}:output"), &output_raw)?
                    };
                    let (tool_use_result, result_content) = if let Some(result) = result_raw {
                        let (bounded, content) =
                            self.bounded_json(&format!("tool:{tool_use_id}:result"), &result)?;
                        (Some(bounded), content)
                    } else {
                        (None, None)
                    };
                    update_tool_page(&self.pages_dir, open.page_number, tool_use_id, |entry| {
                        if let HistoryEntry::Tool {
                            status,
                            input,
                            output: old_output,
                            tool_use_result: old_result,
                            input_content,
                            output_content: old_output_content,
                            result_content: old_result_content,
                            duration_ms,
                            last_seq,
                            tool_name,
                            ..
                        } = entry
                        {
                            if let Some((stream_input, stream_content)) = streamed_input {
                                *input = stream_input;
                                *input_content = stream_content;
                            }
                            *status = event
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("success")
                                .to_string();
                            *old_output = output;
                            *old_result = tool_use_result;
                            *old_output_content = output_content;
                            *old_result_content = result_content;
                            *duration_ms = event.get("duration_ms").and_then(Value::as_u64);
                            *last_seq = seq;
                            if let Some(name) = event.get("tool_name").and_then(Value::as_str) {
                                if !name.is_empty() {
                                    *tool_name = name.to_string();
                                }
                            }
                        }
                        Ok(())
                    })?;
                    if event.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion")
                        && event.get("status").and_then(Value::as_str) == Some("error")
                    {
                        if self.pending_user_inputs.len() >= MAX_PENDING_INTERACTIONS
                            && !self.pending_user_inputs.contains_key(tool_use_id)
                        {
                            return Err(format!(
                                "history pending user input limit exceeded: {}",
                                MAX_PENDING_INTERACTIONS
                            ));
                        }
                        self.pending_user_inputs
                            .insert(tool_use_id.to_string(), open.page_number);
                    }
                }
            }
            "command_output" => {
                let text = event.get("content").and_then(Value::as_str).unwrap_or("");
                let id = format!("command-{seq}");
                let content = self.content_from_text(&format!("command:{seq}"), text)?;
                self.push(HistoryEntry::CommandOutput {
                    anchor_id: id.clone(),
                    id,
                    ts,
                    content,
                    first_seq: seq,
                    last_seq: seq,
                })?;
            }
            "session_init" | "run_state" | "usage_update" | "compact_boundary"
            | "system_status" | "auth_status" | "rate_limit_event" => {
                let mut bounded = event.clone();
                bounded["_seq"] = Value::Number(seq.into());
                bounded["ts"] = Value::String(ts);
                let encoded = serde_json::to_vec(&bounded).map_err(|e| e.to_string())?;
                if encoded.len() <= CONTENT_INLINE_LIMIT {
                    self.state_events.insert(kind.to_string(), bounded);
                } else {
                    log::warn!(
                        "[history] state event omitted from summary: run_id={}, type={}, bytes={}",
                        self.run_id,
                        kind,
                        encoded.len()
                    );
                }
            }
            "permission_prompt" | "elicitation_prompt" | "hook_callback" => {
                if kind == "hook_callback"
                    && event.get("hook_event").and_then(Value::as_str) != Some("PreToolUse")
                {
                    return Ok(());
                }
                let request_id = event
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !request_id.is_empty() {
                    let mut bounded = event.clone();
                    bounded["_seq"] = Value::Number(seq.into());
                    bounded["ts"] = Value::String(ts);
                    let encoded = serde_json::to_vec(&bounded).map_err(|e| e.to_string())?;
                    if encoded.len() <= CONTENT_INLINE_LIMIT {
                        if self.pending_interactions.len() >= MAX_PENDING_INTERACTIONS
                            && !self.pending_interactions.contains_key(request_id)
                        {
                            return Err(format!(
                                "history pending interaction limit exceeded: {}",
                                MAX_PENDING_INTERACTIONS
                            ));
                        }
                        self.pending_interactions
                            .insert(request_id.to_string(), bounded);
                    } else {
                        return Err(format!(
                            "pending interaction exceeds hard limit: {}",
                            encoded.len()
                        ));
                    }
                }
            }
            // A started response is intentionally non-retryable on recovery: the process can die
            // after writing to CLI stdin but before recording the resolved/failure terminal fact.
            "control_cancelled"
            | "interaction_response_started"
            | "interaction_response_failed"
            | "interaction_resolved" => {
                if let Some(request_id) = event.get("request_id").and_then(Value::as_str) {
                    let lifecycle_key = format!("interaction:{request_id}");
                    if (kind == "interaction_response_started"
                        || kind == "interaction_response_failed")
                        && self.pending_interactions.contains_key(request_id)
                    {
                        let mut bounded = event.clone();
                        bounded["_seq"] = Value::Number(seq.into());
                        bounded["ts"] = Value::String(ts.clone());
                        let encoded = serde_json::to_vec(&bounded).map_err(|e| e.to_string())?;
                        if encoded.len() > CONTENT_INLINE_LIMIT {
                            return Err(format!(
                                "interaction lifecycle event exceeds hard limit: {}",
                                encoded.len()
                            ));
                        }
                        // Keep the original prompt in pending_interactions so summary replay can
                        // render a visible non-retryable terminal card before applying this fact.
                        self.state_events.insert(lifecycle_key, bounded);
                    } else {
                        self.pending_interactions.remove(request_id);
                        self.state_events.remove(&lifecycle_key);
                    }
                    let page_number = if kind == "interaction_response_started" {
                        self.pending_user_inputs.get(request_id).copied()
                    } else {
                        self.pending_user_inputs.remove(request_id)
                    };
                    if let Some(page_number) = page_number {
                        let target_status = match kind {
                            "interaction_response_started" => "response_pending",
                            "interaction_response_failed" => "response_failed",
                            "interaction_resolved" => "success",
                            _ => "error",
                        };
                        update_tool_page(&self.pages_dir, page_number, request_id, |entry| {
                            if let HistoryEntry::Tool { status, .. } = entry {
                                *status = target_status.to_string();
                            }
                            Ok(())
                        })?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_oversized_event(
        &mut self,
        path: PathBuf,
        byte_length: u64,
        line_no: usize,
    ) -> Result<(), String> {
        let metadata = oversized_metadata(&path)?;
        let seq = metadata
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or(self.last_seq);
        let ts = metadata
            .get("ts")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let kind = metadata
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let parent = normalized_parent_tool_use_id(&metadata).map(str::to_string);
        let mut event = metadata;
        event["run_id"] = Value::String(self.run_id.clone());
        let envelope = json!({"_bus":true,"seq":seq,"ts":ts,"event":event});
        let duplicate_identity = self.is_duplicate_identity(kind.as_str(), &envelope["event"]);
        if duplicate_identity {
            // Let the normal handler advance last_seq and emit the omission log, but do not
            // extract multi-megabyte content into a spool that no projected entry will own.
            self.handle_event(&envelope)?;
            fs::remove_file(path).map_err(|e| e.to_string())?;
            return Ok(());
        }

        match kind.as_str() {
            "message_complete" | "user_message" | "command_output" => {
                let field = if kind == "command_output" {
                    "content"
                } else {
                    "text"
                };
                let text = extract_json_string_field(
                    &path,
                    field,
                    self.spools_dir
                        .join(format!("oversized-text-{line_no}.bin")),
                )?
                .ok_or_else(|| format!("oversized {kind} is missing {field}"))?;
                self.handle_event(&envelope)?;
                let key = format!("oversized:{kind}:{seq}:text");
                let content = self.content_from_spool(&key, text)?;
                let target = if let Some(parent_id) = parent.as_deref() {
                    self.subhistories
                        .get_mut(parent_id)
                        .and_then(|sub| sub.page.last_mut())
                } else {
                    self.page.last_mut()
                };
                match target {
                    Some(HistoryEntry::Assistant { content: old, .. })
                    | Some(HistoryEntry::User { content: old, .. })
                    | Some(HistoryEntry::CommandOutput { content: old, .. }) => *old = content,
                    _ => {
                        return Err(format!(
                            "oversized {kind} did not create its semantic entry"
                        ))
                    }
                }
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            "tool_start" | "tool_end" => {
                let tool_use_id = envelope["event"]
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("oversized {kind} is missing tool_use_id"))?
                    .to_string();
                let input_value = if kind == "tool_start" {
                    extract_json_value_field(
                        &path,
                        "input",
                        self.spools_dir
                            .join(format!("oversized-tool-input-{line_no}.bin")),
                    )?
                } else {
                    None
                };
                let output_value = if kind == "tool_end" {
                    extract_json_value_field(
                        &path,
                        "output",
                        self.spools_dir
                            .join(format!("oversized-tool-output-{line_no}.bin")),
                    )?
                } else {
                    None
                };
                let result_value = if kind == "tool_end" {
                    extract_json_value_field(
                        &path,
                        "tool_use_result",
                        self.spools_dir
                            .join(format!("oversized-tool-result-{line_no}.bin")),
                    )?
                } else {
                    None
                };
                let scope_before = if kind == "tool_end" {
                    self.open_tool_scope(&tool_use_id)
                } else {
                    None
                };
                let page_before = scope_before
                    .as_ref()
                    .and_then(|scope| self.open_tool_page(scope, &tool_use_id));
                self.handle_event(&envelope)?;
                let scope = scope_before.or_else(|| self.open_tool_scope(&tool_use_id));
                let page_number = page_before.or_else(|| {
                    scope
                        .as_ref()
                        .and_then(|owner| self.open_tool_page(owner, &tool_use_id))
                });
                let page_number = page_number
                    .ok_or_else(|| format!("oversized {kind} has no matching tool entry"))?;
                let bound_value = |key: &str, spool: SpoolText| {
                    bounded_json_spool_at(
                        &self.run_id,
                        &self.generation_id,
                        &self.blobs_dir,
                        key,
                        spool,
                    )
                };
                let extracted_input = input_value
                    .map(|spool| {
                        bound_value(&format!("oversized:input:{tool_use_id}:{seq}"), spool)
                    })
                    .transpose()?;
                let extracted_output = output_value
                    .map(|spool| {
                        bound_value(&format!("oversized:output:{tool_use_id}:{seq}"), spool)
                    })
                    .transpose()?;
                let extracted_result = result_value
                    .map(|spool| {
                        bound_value(&format!("oversized:result:{tool_use_id}:{seq}"), spool)
                    })
                    .transpose()?;
                let pages_dir = match scope.as_ref() {
                    Some(OpenToolScope::Subhistory(parent_id)) => self
                        .subhistories
                        .get(parent_id)
                        .map(|subhistory| subhistory.pages_dir.as_path())
                        .ok_or_else(|| format!("missing subhistory owner: {parent_id}"))?,
                    _ => self.pages_dir.as_path(),
                };
                update_tool_page(pages_dir, page_number, &tool_use_id, |entry| {
                    if let HistoryEntry::Tool {
                        input,
                        output,
                        input_content,
                        output_content,
                        result_content: old_result_content,
                        tool_use_result,
                        ..
                    } = entry
                    {
                        if kind == "tool_start" {
                            if let Some((value, content)) = extracted_input {
                                *input = value;
                                *input_content = content;
                            }
                        } else {
                            if let Some((value, content)) = extracted_output {
                                *output = value;
                                *output_content = content;
                            }
                            if let Some((value, content)) = extracted_result {
                                *tool_use_result = Some(value);
                                *old_result_content = content;
                            }
                        }
                    }
                    Ok(())
                })?;
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            _ => {
                let id = format!("oversized-line-{line_no}");
                let content = publish_external_content(
                    &self.run_id,
                    &self.generation_id,
                    &self.blobs_dir,
                    &id,
                    &path,
                    byte_length,
                )?;
                self.push(HistoryEntry::Placeholder {
                    id: id.clone(),
                    anchor_id: id,
                    ts,
                    content,
                    first_seq: seq,
                    last_seq: seq,
                })?;
            }
        }
        self.last_seq = self.last_seq.max(seq);
        log::warn!(
            "[history] oversized event projected semantically: run_id={}, line={}, type={}, bytes={}",
            self.run_id,
            line_no,
            kind,
            byte_length
        );
        Ok(())
    }

    fn handle_subhistory_event(
        &mut self,
        parent_tool_use_id: &str,
        seq: u64,
        ts: String,
        event: &Value,
    ) -> Result<(), String> {
        let run_id = self.run_id.clone();
        let generation_id = self.generation_id.clone();
        let blobs_dir = self.blobs_dir.clone();
        let subhistories_root = self.generation_dir.join("subhistories");
        super::ensure_dir(&subhistories_root).map_err(|e| e.to_string())?;
        if !self.subhistories.contains_key(parent_tool_use_id)
            && self.subhistories.len() >= MAX_SUBHISTORIES
        {
            return Err(format!(
                "history subhistory limit exceeded: {}",
                MAX_SUBHISTORIES
            ));
        }
        let child_history_id = sanitize_id(parent_tool_use_id);
        // Nested sub-agent events name their immediate child tool as the parent. Link that tool to
        // its independently paged history before borrowing/creating the target subhistory.
        for owner in self.subhistories.values_mut() {
            if let Some(open) = owner.open_tools.get(parent_tool_use_id) {
                update_tool_page(
                    &owner.pages_dir,
                    open.page_number,
                    parent_tool_use_id,
                    |entry| {
                        if let HistoryEntry::Tool { sub_history_id, .. } = entry {
                            *sub_history_id = Some(child_history_id.clone());
                        }
                        Ok(())
                    },
                )?;
                break;
            }
        }
        let sub = if let Some(sub) = self.subhistories.get_mut(parent_tool_use_id) {
            sub
        } else {
            let sub = SubHistoryState::new(
                &subhistories_root,
                parent_tool_use_id,
                &run_id,
                &generation_id,
                &blobs_dir,
            )?;
            self.subhistories
                .insert(parent_tool_use_id.to_string(), sub);
            self.subhistories.get_mut(parent_tool_use_id).unwrap()
        };
        if let Some(open) = self.open_tools.get(parent_tool_use_id) {
            update_tool_page(
                &self.pages_dir,
                open.page_number,
                parent_tool_use_id,
                |entry| {
                    if let HistoryEntry::Tool { sub_history_id, .. } = entry {
                        *sub_history_id = Some(sub.id.clone());
                    }
                    Ok(())
                },
            )?;
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_delta" => {
                sub.pending_message_seq.get_or_insert(seq);
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    sub.pending_message.append(text)?;
                }
            }
            "thinking_delta" => {
                sub.pending_thinking_seq.get_or_insert(seq);
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    sub.pending_thinking.append(text)?;
                }
            }
            "message_complete" => {
                let id = event
                    .get("message_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("sub-assistant-{seq}"));
                let content = if let Some(text) = event.get("text").and_then(Value::as_str) {
                    content_from_text_at(
                        &run_id,
                        &generation_id,
                        &blobs_dir,
                        &format!("sub:{parent_tool_use_id}:{id}:text"),
                        text,
                    )?
                } else {
                    let pending = std::mem::replace(
                        &mut sub.pending_message,
                        SpoolText::new(sub.spools_dir.join("message-next.bin")),
                    );
                    content_from_spool_at(
                        &run_id,
                        &generation_id,
                        &blobs_dir,
                        &format!("sub:{parent_tool_use_id}:{id}:text"),
                        pending,
                    )?
                };
                let thinking = if sub.pending_thinking.is_empty() {
                    None
                } else {
                    let pending = std::mem::replace(
                        &mut sub.pending_thinking,
                        SpoolText::new(sub.spools_dir.join("thinking-next.bin")),
                    );
                    Some(content_from_spool_at(
                        &run_id,
                        &generation_id,
                        &blobs_dir,
                        &format!("sub:{parent_tool_use_id}:{id}:thinking"),
                        pending,
                    )?)
                };
                let first_seq = sub.pending_message_seq.take().unwrap_or(seq);
                sub.pending_thinking_seq = None;
                sub.push(HistoryEntry::Assistant {
                    id: id.clone(),
                    anchor_id: id,
                    ts,
                    content,
                    thinking,
                    model: event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    first_seq,
                    last_seq: seq,
                })?;
                sub.reset_pending_streams()?;
            }
            "tool_start" => {
                let id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    return Ok(());
                }
                let input_raw = event.get("input").cloned().unwrap_or(Value::Null);
                let (input, input_content) = bounded_json_at(
                    &run_id,
                    &generation_id,
                    &blobs_dir,
                    &format!("sub:{parent_tool_use_id}:{id}:input"),
                    &input_raw,
                )?;
                if sub.open_tools.len() >= MAX_OPEN_TOOLS {
                    return Err(format!(
                        "subhistory open tool limit exceeded: {}",
                        MAX_OPEN_TOOLS
                    ));
                }
                let input_stream = event
                    .get("input")
                    .filter(|value| !value.is_null())
                    .is_none()
                    .then(|| SpoolText::new(sub.spools_dir.join(format!("tool-input-{id}.bin"))));
                flush_entries_page(
                    &mut sub.page,
                    &mut sub.page_bytes,
                    &mut sub.page_count,
                    &sub.pages_dir,
                )?;
                sub.push(HistoryEntry::Tool {
                    id: id.clone(),
                    anchor_id: id.clone(),
                    ts,
                    tool_use_id: id.clone(),
                    tool_name: event
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    status: "running".to_string(),
                    input,
                    output: Value::Null,
                    tool_use_result: None,
                    input_content,
                    output_content: None,
                    result_content: None,
                    duration_ms: None,
                    sub_history_id: None,
                    first_seq: seq,
                    last_seq: seq,
                })?;
                flush_entries_page(
                    &mut sub.page,
                    &mut sub.page_bytes,
                    &mut sub.page_count,
                    &sub.pages_dir,
                )?;
                sub.open_tools.insert(
                    id.clone(),
                    OpenTool {
                        page_number: sub.page_count,
                        input_stream,
                        output_stream: None,
                    },
                );
            }
            "tool_input_delta" => {
                let id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(open) = sub.open_tools.get_mut(id) {
                    if open.input_stream.is_none() {
                        open.input_stream = Some(SpoolText::new(
                            sub.spools_dir.join(format!("tool-input-{id}.bin")),
                        ));
                    }
                    if let Some(partial) = event.get("partial_json").and_then(Value::as_str) {
                        open.input_stream.as_mut().unwrap().append(partial)?;
                    }
                }
            }
            "tool_output_delta" => {
                let id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(open) = sub.open_tools.get_mut(id) {
                    if open.output_stream.is_none() {
                        open.output_stream = Some(SpoolText::new(
                            sub.spools_dir.join(format!("tool-output-{id}.bin")),
                        ));
                    }
                    if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                        open.output_stream.as_mut().unwrap().append(delta)?;
                    }
                }
            }
            "tool_end" => {
                let id = event
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(mut open) = sub.open_tools.remove(id) {
                    let mut streamed_input = None;
                    if let Some(input_stream) = open.input_stream.take() {
                        if !input_stream.is_empty() {
                            let parsed = if input_stream.file.is_none() {
                                serde_json::from_str::<Value>(&input_stream.inline).ok()
                            } else {
                                None
                            };
                            let mut input_content = content_from_spool_at(
                                &run_id,
                                &generation_id,
                                &blobs_dir,
                                &format!("sub:{parent_tool_use_id}:{id}:input-stream"),
                                input_stream,
                            )?;
                            input_content.encoding = "json".to_string();
                            let input = parsed.unwrap_or_else(
                                || json!({"_truncated": true, "preview": input_content.preview}),
                            );
                            streamed_input =
                                Some((input, input_content.truncated.then_some(input_content)));
                        }
                    }
                    let streamed_output =
                        open.output_stream.take().filter(|spool| !spool.is_empty());
                    let output_raw = event.get("output").cloned().unwrap_or(Value::Null);
                    let (bounded_output, output_content) = if output_raw.is_null() {
                        if let Some(output_stream) = streamed_output {
                            let content = content_from_spool_at(
                                &run_id,
                                &generation_id,
                                &blobs_dir,
                                &format!("sub:{parent_tool_use_id}:{id}:output-stream"),
                                output_stream,
                            )?;
                            (
                                Value::String(content.preview.clone()),
                                content.truncated.then_some(content),
                            )
                        } else {
                            (Value::Null, None)
                        }
                    } else {
                        bounded_json_at(
                            &run_id,
                            &generation_id,
                            &blobs_dir,
                            &format!("sub:{parent_tool_use_id}:{id}:output"),
                            &output_raw,
                        )?
                    };
                    let (bounded_result, result_content) =
                        if let Some(result) = event.get("tool_use_result") {
                            let (bounded, content) = bounded_json_at(
                                &run_id,
                                &generation_id,
                                &blobs_dir,
                                &format!("sub:{parent_tool_use_id}:{id}:result"),
                                result,
                            )?;
                            (Some(bounded), content)
                        } else {
                            (None, None)
                        };
                    update_tool_page(&sub.pages_dir, open.page_number, id, |entry| {
                        if let HistoryEntry::Tool {
                            status,
                            input,
                            output,
                            tool_use_result,
                            input_content,
                            output_content: old_output_content,
                            result_content: old_result_content,
                            duration_ms,
                            last_seq,
                            ..
                        } = entry
                        {
                            if let Some((stream_input, stream_content)) = streamed_input {
                                *input = stream_input;
                                *input_content = stream_content;
                            }
                            *status = event
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("success")
                                .to_string();
                            *output = bounded_output;
                            *tool_use_result = bounded_result;
                            *old_output_content = output_content;
                            *old_result_content = result_content;
                            *duration_ms = event.get("duration_ms").and_then(Value::as_u64);
                            *last_seq = seq;
                        }
                        Ok(())
                    })?;
                }
            }
            "user_message" => {
                let id = format!("sub-user-{seq}");
                let content = content_from_text_at(
                    &run_id,
                    &generation_id,
                    &blobs_dir,
                    &format!("sub:{parent_tool_use_id}:{id}"),
                    event.get("text").and_then(Value::as_str).unwrap_or(""),
                )?;
                sub.push(HistoryEntry::User {
                    id: id.clone(),
                    anchor_id: id,
                    ts,
                    content,
                    cli_uuid: None,
                    attachments: vec![],
                    first_seq: seq,
                    last_seq: seq,
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_legacy_event(&mut self, event: &Value) -> Result<(), String> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let seq = event.get("seq").and_then(Value::as_u64).unwrap_or_else(|| {
            self.last_seq += 1;
            self.last_seq
        });
        let ts = event
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let text = event
            .get("payload")
            .and_then(|p| p.get("text").or_else(|| p.get("message")))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !matches!(kind, "user" | "assistant" | "stdout" | "stderr") || text.is_empty() {
            return Ok(());
        }
        let id = format!("legacy-{kind}-{seq}");
        let content = self.content_from_text(&id, text)?;
        let entry = if kind == "user" {
            self.total_turns += 1;
            HistoryEntry::User {
                id: id.clone(),
                anchor_id: id,
                ts,
                content,
                cli_uuid: None,
                attachments: vec![],
                first_seq: seq,
                last_seq: seq,
            }
        } else if kind == "assistant" {
            HistoryEntry::Assistant {
                id: id.clone(),
                anchor_id: id,
                ts,
                content,
                thinking: None,
                model: None,
                first_seq: seq,
                last_seq: seq,
            }
        } else {
            HistoryEntry::CommandOutput {
                id: id.clone(),
                anchor_id: id,
                ts,
                content,
                first_seq: seq,
                last_seq: seq,
            }
        };
        self.push(entry)
    }

    fn finish(
        mut self,
        source_size: u64,
        source_mtime_ns: u128,
        source_prefix_hash: String,
    ) -> Result<HistoryManifest, String> {
        self.settle_interrupted_work(self.last_seq, "Session ended before tool result")?;
        let subhistories = std::mem::take(&mut self.subhistories);
        for subhistory in subhistories.into_values() {
            subhistory.finish()?;
        }
        self.flush_page()?;
        let mut state_events: Vec<_> = self.state_events.into_values().collect();
        state_events.sort_by_key(|event| event.get("_seq").and_then(Value::as_u64).unwrap_or(0));
        let mut pending_interactions: Vec<_> = self.pending_interactions.into_values().collect();
        pending_interactions
            .sort_by_key(|event| event.get("_seq").and_then(Value::as_u64).unwrap_or(0));
        state_events.extend(pending_interactions);
        // Lifecycle facts must replay after their prompt. Sorting the two collections separately
        // and appending prompts last would resurrect a retry button for an uncertain response.
        state_events.sort_by_key(|event| event.get("_seq").and_then(Value::as_u64).unwrap_or(0));
        let summary = HistorySummary {
            run_id: self.run_id.clone(),
            generation_id: self.generation_id.clone(),
            page_count: self.page_count,
            total_entries: self.total_entries,
            total_turns: self.total_turns,
            last_seq: self.last_seq,
            source_size,
            source_mtime_ns,
            latest_cursor: (self.page_count > 0)
                .then(|| cursor(&self.generation_id, self.page_count)),
            state_events,
        };
        let summary_body = serde_json::to_vec(&summary).map_err(|e| e.to_string())?;
        if summary_body.len() > CONTENT_CHUNK_LIMIT {
            return Err(format!(
                "history summary exceeds hard limit: {}",
                summary_body.len()
            ));
        }
        fs::write(self.generation_dir.join("summary.json"), summary_body)
            .map_err(|e| e.to_string())?;
        Ok(HistoryManifest {
            format_version: FORMAT_VERSION,
            builder_version: BUILDER_VERSION,
            generation_id: self.generation_id,
            source_size,
            source_mtime_ns,
            source_prefix_hash,
            last_seq: self.last_seq,
            page_count: self.page_count,
            total_entries: self.total_entries,
            total_turns: self.total_turns,
            complete: true,
        })
    }
}

fn current_generation(run_id: &str) -> Result<Option<String>, String> {
    let path = history_root(run_id).join("current.json");
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read(&path).map_err(|e| e.to_string())?;
    let pointer: CurrentPointer = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(Some(pointer.generation_id))
}

fn retain_generation(run_id: &str, generation_id: &str) -> Result<(), String> {
    let path = history_root(run_id)
        .join("generations")
        .join(generation_id)
        .join(".retain");
    fs::write(&path, PROCESS_RETENTION_ID.as_bytes())
        .map_err(|e| format!("retain history generation: {e}"))
}

fn retained_in_this_process(path: &Path) -> bool {
    fs::read_to_string(path.join(".retain"))
        .is_ok_and(|value| value == PROCESS_RETENTION_ID.as_str())
}

fn should_remove_generation(path: &Path, name: &str, current: Option<&str>) -> bool {
    name.ends_with(".build")
        || (is_hex_id(name, 32) && current != Some(name) && !retained_in_this_process(path))
}

fn load_manifest(run_id: &str, generation_id: &str) -> Result<HistoryManifest, String> {
    if !is_hex_id(generation_id, 32) {
        return Err("invalid history generation identifier".to_string());
    }
    let path = history_root(run_id)
        .join("generations")
        .join(generation_id)
        .join("manifest.json");
    let body = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

fn manifest_matches(run_id: &str, manifest: &HistoryManifest) -> bool {
    if manifest.format_version != FORMAT_VERSION
        || manifest.builder_version != BUILDER_VERSION
        || !manifest.complete
    {
        return false;
    }
    let allow_append = !run_is_terminal(run_id);
    manifest_matches_source(&source_path(run_id), manifest, allow_append)
}

fn run_is_terminal(run_id: &str) -> bool {
    super::runs::get_run(run_id).is_some_and(|run| {
        matches!(
            run.status,
            crate::models::RunStatus::Completed
                | crate::models::RunStatus::Failed
                | crate::models::RunStatus::Stopped
        )
    })
}

fn manifest_matches_source(source: &Path, manifest: &HistoryManifest, allow_append: bool) -> bool {
    let Ok(meta) = source.metadata() else {
        return manifest.source_size == 0;
    };
    if meta.len() < manifest.source_size || (!allow_append && meta.len() != manifest.source_size) {
        return false;
    }
    // Appends after publication are consumed by bounded catch-up from `source_size`; rebuilding
    // here would defeat that handoff and retain a new immutable generation on every run load.
    // For an unchanged-length source, mtime still detects same-size replacement even when the
    // boundary windows happen to match.
    if meta.len() == manifest.source_size && modified_ns(&meta) != manifest.source_mtime_ns {
        return false;
    }
    hash_prefix(source, manifest.source_size).ok().as_deref()
        == Some(manifest.source_prefix_hash.as_str())
}

fn build_generation(run_id: &str) -> Result<String, String> {
    let _global_guard = GLOBAL_BUILD_LOCK.lock().map_err(|e| e.to_string())?;
    let previous_generation = current_generation(run_id).ok().flatten();
    let root = history_root(run_id);
    let generations = root.join("generations");
    super::ensure_dir(&generations).map_err(|e| e.to_string())?;
    let generation_id = uuid::Uuid::new_v4().simple().to_string();
    let build_dir = generations.join(format!("{generation_id}.build"));
    super::ensure_dir(&build_dir).map_err(|e| e.to_string())?;
    log::debug!(
        "[history] build start: run_id={}, generation={}",
        run_id,
        generation_id
    );
    let mut state = BuildState::new(run_id, &generation_id, build_dir.clone())?;
    // The writer repairs an interrupted tail and captures metadata while holding the same
    // per-run lock used by append. The lock is released before scanning, but this fixed prefix
    // can only end after a complete JSONL record; later appends are delivered by catch-up.
    let snapshot = super::events::global_writer().snapshot(run_id)?;
    let source_meta = snapshot.metadata;
    let source_mtime_ns = modified_ns(&source_meta);
    let terminal = run_is_terminal(run_id);
    // Terminal runs have no frontend catch-up phase, so their interrupted tail must remain
    // visible. Active runs defer an open semantic tail and replay it from the safe offset.
    let projection_size = if terminal {
        source_meta.len()
    } else {
        safe_projection_size(&snapshot.file, source_meta.len(), &state.spools_dir)?
    };
    let source_prefix_hash = hash_file_prefix(&snapshot.file, projection_size)?;
    let mut source_file = snapshot.file;
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    if projection_size < source_meta.len() {
        log::debug!(
            "[history] active tail deferred to catch-up: run_id={}, source_size={}, projection_size={}",
            run_id,
            source_meta.len(),
            projection_size
        );
    }
    let mut reader = BufReader::new(source_file.take(projection_size));
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let oversized_path = state
            .spools_dir
            .join(format!("oversized-line-{line_no}.bin"));
        let line = match read_bounded_line(&mut reader, &oversized_path)? {
            ScannedLine::Eof => break,
            ScannedLine::Inline { data, .. } => data,
            ScannedLine::External {
                path, byte_length, ..
            } => {
                state.handle_oversized_event(path, byte_length, line_no)?;
                continue;
            }
        };
        let line = match std::str::from_utf8(&line) {
            Ok(line) => line,
            Err(e) => {
                log::warn!(
                    "[history] invalid utf-8 skipped: run_id={}, line={}, error={}",
                    run_id,
                    line_no,
                    e
                );
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => state.handle_event(&value)?,
            Err(e) => log::warn!(
                "[history] invalid json skipped: run_id={}, line={}, error={}",
                run_id,
                line_no,
                e
            ),
        }
    }
    let manifest = state.finish(projection_size, source_mtime_ns, source_prefix_hash)?;
    fs::write(
        build_dir.join("manifest.json"),
        serde_json::to_vec(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let final_dir = generations.join(&generation_id);
    fs::rename(&build_dir, &final_dir).map_err(|e| format!("publish generation: {e}"))?;
    let pointer_tmp = root.join(format!("current.{}.tmp", generation_id));
    fs::write(
        &pointer_tmp,
        serde_json::to_vec(&CurrentPointer {
            generation_id: generation_id.clone(),
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let pointer = root.join("current.json");
    // The old pointer must remain readable until replacement succeeds. Deleting it first would
    // create a crash window with no published generation, especially on Windows.
    replace_file(&pointer_tmp, &pointer).map_err(|e| format!("publish current pointer: {e}"))?;
    if let Some(previous) = previous_generation {
        retain_generation(run_id, &previous)?;
    }
    retain_generation(run_id, &generation_id)?;
    log::debug!(
        "[history] build complete: run_id={}, generation={}, pages={}, entries={}",
        run_id,
        generation_id,
        manifest.page_count,
        manifest.total_entries
    );
    cleanup_history(run_id)?;
    Ok(generation_id)
}

fn ensure_history_with_refresh(run_id: &str, refresh: bool) -> Result<String, String> {
    let digest = Sha256::digest(run_id.as_bytes());
    let shard = digest[0] as usize % BUILD_LOCKS.len();
    let _guard = BUILD_LOCKS[shard].lock().map_err(|e| e.to_string())?;
    cleanup_history(run_id)?;
    if !refresh {
        if let Ok(Some(generation)) = current_generation(run_id) {
            if let Ok(manifest) = load_manifest(run_id, &generation) {
                if manifest_matches(run_id, &manifest) {
                    log::debug!(
                        "[history] generation hit: run_id={}, generation={}",
                        run_id,
                        generation
                    );
                    retain_generation(run_id, &generation)?;
                    return Ok(generation);
                }
            }
        }
    } else {
        log::debug!("[history] forced rebuild: run_id={}", run_id);
    }
    build_generation(run_id)
}

pub fn ensure_history(run_id: &str) -> Result<String, String> {
    ensure_history_with_refresh(run_id, false)
}

fn cleanup_history(run_id: &str) -> Result<(), String> {
    let generations = history_root(run_id).join("generations");
    if !generations.exists() {
        return Ok(());
    }
    let current = current_generation(run_id).ok().flatten();
    for entry in fs::read_dir(&generations).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if should_remove_generation(&entry.path(), &name, current.as_deref()) {
            fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn get_summary(run_id: &str, refresh: bool) -> Result<HistorySummary, String> {
    let generation = ensure_history_with_refresh(run_id, refresh)?;
    read_summary(run_id, &generation)
        .or_else(|_| build_generation(run_id).and_then(|rebuilt| read_summary(run_id, &rebuilt)))
}

fn read_summary(run_id: &str, generation: &str) -> Result<HistorySummary, String> {
    let path = history_root(run_id)
        .join("generations")
        .join(generation)
        .join("summary.json");
    let body = fs::read(&path).map_err(|e| e.to_string())?;
    if body.len() > CONTENT_CHUNK_LIMIT {
        return Err("history summary exceeds hard limit".to_string());
    }
    let summary: HistorySummary = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if summary.page_count > 0 {
        let latest = history_root(run_id)
            .join("generations")
            .join(generation)
            .join("pages")
            .join(format!("{:08}.json", summary.page_count));
        let page = fs::read(&latest).map_err(|e| e.to_string())?;
        if page.len() > PAGE_BYTE_LIMIT {
            return Err("history page exceeds hard limit".to_string());
        }
        serde_json::from_slice::<Vec<HistoryEntry>>(&page).map_err(|e| e.to_string())?;
    }
    Ok(summary)
}

pub fn get_page(
    run_id: &str,
    generation_id: Option<&str>,
    before_cursor: Option<&str>,
) -> Result<HistoryPage, String> {
    let (generation, page) = if let Some(raw) = before_cursor {
        let (generation, page) = parse_cursor(raw)?;
        if generation_id.is_some_and(|expected| expected != generation) {
            return Err("history cursor generation mismatch".to_string());
        }
        (
            generation.to_string(),
            page.checked_sub(1)
                .ok_or_else(|| "history cursor has no previous page".to_string())?,
        )
    } else {
        let generation = generation_id
            .map(str::to_string)
            .map(Ok)
            .unwrap_or_else(|| ensure_history(run_id))?;
        let manifest = load_manifest(run_id, &generation)?;
        let page = manifest.page_count;
        (generation, page)
    };
    let manifest = load_manifest(run_id, &generation)?;
    retain_generation(run_id, &generation)?;
    if page == 0 || page > manifest.page_count {
        return Err("history page not found".to_string());
    }
    let path = history_root(run_id)
        .join("generations")
        .join(&generation)
        .join("pages")
        .join(format!("{page:08}.json"));
    let body = fs::read(&path).map_err(|e| e.to_string())?;
    if body.len() > PAGE_BYTE_LIMIT {
        return Err("history page exceeds hard limit".to_string());
    }
    let entries: Vec<HistoryEntry> = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let first_seq = entries.first().map(HistoryEntry::first_seq).unwrap_or(0);
    let last_seq = entries.last().map(HistoryEntry::last_seq).unwrap_or(0);
    Ok(HistoryPage {
        run_id: run_id.to_string(),
        generation_id: generation.clone(),
        entries,
        page_cursor: cursor(&generation, page),
        previous_cursor: (page > 1).then(|| cursor(&generation, page)),
        has_more: page > 1,
        first_seq,
        last_seq,
    })
}

pub fn get_content_chunk(
    run_id: &str,
    generation_id: &str,
    content_id: &str,
    offset: u64,
    max_bytes: usize,
) -> Result<ContentChunk, String> {
    if !is_hex_id(generation_id, 32) || !is_hex_id(content_id, 64) {
        return Err("invalid history content identifier".to_string());
    }
    retain_generation(run_id, generation_id)?;
    let max_bytes = max_bytes.min(CONTENT_CHUNK_LIMIT);
    if max_bytes == 0 {
        return Err("max_bytes must be positive".to_string());
    }
    let path = history_root(run_id)
        .join("generations")
        .join(generation_id)
        .join("blobs")
        .join(format!("{content_id}.bin"));
    let mut file = File::open(&path).map_err(|e| format!("open history content: {e}"))?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    if offset > total {
        return Err("history content offset out of range".to_string());
    }
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut data = vec![0u8; max_bytes.min((total - offset) as usize)];
    file.read_exact(&mut data).map_err(|e| e.to_string())?;
    let next_offset = offset + data.len() as u64;
    use base64::Engine;
    Ok(ContentChunk {
        run_id: run_id.to_string(),
        generation_id: generation_id.to_string(),
        content_id: content_id.to_string(),
        offset,
        next_offset,
        total_bytes: total,
        eof: next_offset >= total,
        data_base64: base64::engine::general_purpose::STANDARD.encode(data),
    })
}

pub fn get_subhistory_page(
    run_id: &str,
    generation_id: &str,
    sub_history_id: &str,
    before_cursor: Option<&str>,
) -> Result<SubHistoryPage, String> {
    if !is_hex_id(generation_id, 32) || !is_hex_id(sub_history_id, 64) {
        return Err("invalid subhistory identifier".to_string());
    }
    retain_generation(run_id, generation_id)?;
    let root = history_root(run_id)
        .join("generations")
        .join(generation_id)
        .join("subhistories")
        .join(sub_history_id);
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let page_count = manifest
        .get("pageCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let page = if let Some(raw) = before_cursor {
        let (cursor_generation, cursor_page) = parse_cursor(raw)?;
        if cursor_generation != generation_id {
            return Err("subhistory cursor generation mismatch".to_string());
        }
        cursor_page.saturating_sub(1)
    } else {
        page_count
    };
    if page == 0 || page > page_count {
        return Err("subhistory page not found".to_string());
    }
    let body =
        fs::read(root.join("pages").join(format!("{page:08}.json"))).map_err(|e| e.to_string())?;
    if body.len() > PAGE_BYTE_LIMIT {
        return Err("subhistory page exceeds hard limit".to_string());
    }
    Ok(SubHistoryPage {
        run_id: run_id.to_string(),
        generation_id: generation_id.to_string(),
        sub_history_id: sub_history_id.to_string(),
        entries: serde_json::from_slice(&body).map_err(|e| e.to_string())?,
        page_cursor: cursor(generation_id, page),
        previous_cursor: (page > 1).then(|| cursor(generation_id, page)),
        has_more: page > 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn state(temp: &TempDir) -> BuildState {
        BuildState::new("run-test", "gen-test", temp.path().join("gen")).unwrap()
    }

    fn page_entries(pages_dir: &Path, page: u64) -> Vec<HistoryEntry> {
        serde_json::from_slice(&fs::read(pages_dir.join(format!("{page:08}.json"))).unwrap())
            .unwrap()
    }

    #[test]
    fn manifest_accepts_append_only_source_prefix() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        fs::write(&source, b"{\"seq\":1}\n").unwrap();
        let metadata = source.metadata().unwrap();
        let source_size = metadata.len();
        let manifest = HistoryManifest {
            format_version: FORMAT_VERSION,
            builder_version: BUILDER_VERSION,
            generation_id: "0123456789abcdef0123456789abcdef".to_string(),
            source_size,
            source_mtime_ns: modified_ns(&metadata),
            source_prefix_hash: hash_prefix(&source, source_size).unwrap(),
            last_seq: 1,
            page_count: 0,
            total_entries: 0,
            total_turns: 0,
            complete: true,
        };

        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(b"{\"seq\":2}\n")
            .unwrap();

        assert!(manifest_matches_source(&source, &manifest, true));
        assert!(!manifest_matches_source(&source, &manifest, false));

        let mut replacement = fs::read(&source).unwrap();
        replacement[1] = b'X';
        fs::write(&source, replacement).unwrap();
        assert!(!manifest_matches_source(&source, &manifest, true));
    }

    #[test]
    fn manifest_rejects_middle_prefix_mutation_after_append() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        let original = vec![b'a'; 24 * 1024];
        fs::write(&source, &original).unwrap();
        let metadata = source.metadata().unwrap();
        let source_size = metadata.len();
        let manifest = HistoryManifest {
            format_version: FORMAT_VERSION,
            builder_version: BUILDER_VERSION,
            generation_id: "0123456789abcdef0123456789abcdef".to_string(),
            source_size,
            source_mtime_ns: modified_ns(&metadata),
            source_prefix_hash: hash_prefix(&source, source_size).unwrap(),
            last_seq: 1,
            page_count: 0,
            total_entries: 0,
            total_turns: 0,
            complete: true,
        };

        let mut replacement = original;
        replacement[12 * 1024] = b'b';
        replacement.extend_from_slice(b"append");
        fs::write(&source, replacement).unwrap();

        assert!(!manifest_matches_source(&source, &manifest, true));
    }

    #[test]
    fn semantic_boundary_defers_open_main_and_subhistory_work() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        let events = [
            json!({"_bus":true,"seq":1,"event":{"type":"user_message","text":"question"}}),
            json!({"_bus":true,"seq":2,"event":{"type":"message_delta","text":"partial"}}),
            json!({"_bus":true,"seq":3,"event":{"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash"}}),
            json!({"_bus":true,"seq":4,"event":{"type":"message_delta","parent_tool_use_id":"parent-tool","text":"child partial"}}),
            json!({"_bus":true,"seq":5,"event":{"type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash"}}),
        ];
        let mut body = Vec::new();
        let mut safe_size = 0u64;
        for (index, event) in events.iter().enumerate() {
            serde_json::to_writer(&mut body, event).unwrap();
            body.push(b'\n');
            if index == 0 {
                safe_size = body.len() as u64;
            }
        }
        fs::write(&source, &body).unwrap();
        let file = File::open(&source).unwrap();

        assert_eq!(
            safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
            safe_size
        );
    }

    #[test]
    fn semantic_boundary_advances_after_main_and_subhistory_completion() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        let events = [
            json!({"_bus":true,"seq":1,"event":{"type":"message_delta","text":"partial"}}),
            json!({"_bus":true,"seq":2,"event":{"type":"message_complete","message_id":"main-message","text":"done"}}),
            json!({"_bus":true,"seq":3,"event":{"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash"}}),
            json!({"_bus":true,"seq":4,"event":{"type":"tool_end","tool_use_id":"main-tool","status":"success"}}),
            json!({"_bus":true,"seq":5,"event":{"type":"message_delta","parent_tool_use_id":"parent-tool","text":"partial"}}),
            json!({"_bus":true,"seq":6,"event":{"type":"message_complete","parent_tool_use_id":"parent-tool","message_id":"sub-message","text":"done"}}),
            json!({"_bus":true,"seq":7,"event":{"type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash"}}),
            json!({"_bus":true,"seq":8,"event":{"type":"tool_end","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","status":"success"}}),
        ];
        let mut body = Vec::new();
        for event in events {
            serde_json::to_writer(&mut body, &event).unwrap();
            body.push(b'\n');
        }
        fs::write(&source, &body).unwrap();
        let file = File::open(&source).unwrap();

        assert_eq!(
            safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
            body.len() as u64
        );
    }

    #[test]
    fn semantic_boundary_handles_each_open_work_kind_independently() {
        let cases = [
            (
                "main-assistant",
                json!({"_bus":true,"seq":1,"event":{"type":"message_delta","text":"partial"}}),
                json!({"_bus":true,"seq":2,"event":{"type":"message_complete","message_id":"main-message","text":"done"}}),
            ),
            (
                "main-tool",
                json!({"_bus":true,"seq":1,"event":{"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash"}}),
                json!({"_bus":true,"seq":2,"event":{"type":"tool_end","tool_use_id":"main-tool","status":"success"}}),
            ),
            (
                "subhistory-assistant",
                json!({"_bus":true,"seq":1,"event":{"type":"message_delta","parent_tool_use_id":"parent-tool","text":"partial"}}),
                json!({"_bus":true,"seq":2,"event":{"type":"message_complete","parent_tool_use_id":"parent-tool","message_id":"sub-message","text":"done"}}),
            ),
            (
                "subhistory-tool",
                json!({"_bus":true,"seq":1,"event":{"type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash"}}),
                json!({"_bus":true,"seq":2,"event":{"type":"tool_end","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","status":"success"}}),
            ),
        ];

        for (name, started, completed) in cases {
            let temp = TempDir::new().unwrap();
            let source = temp.path().join(format!("{name}.jsonl"));
            let mut body = serde_json::to_vec(&started).unwrap();
            body.push(b'\n');
            fs::write(&source, &body).unwrap();
            let file = File::open(&source).unwrap();
            assert_eq!(
                safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
                0,
                "{name} must remain in catch-up while open"
            );

            serde_json::to_writer(&mut body, &completed).unwrap();
            body.push(b'\n');
            fs::write(&source, &body).unwrap();
            let file = File::open(&source).unwrap();
            assert_eq!(
                safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
                body.len() as u64,
                "{name} must become publishable after completion"
            );
        }
    }

    #[test]
    fn main_history_normalizes_absent_null_and_empty_parent_scope() {
        let parent_representations = [
            ("absent", None),
            ("null", Some(Value::Null)),
            ("empty", Some(Value::String(String::new()))),
        ];

        for (name, parent) in parent_representations {
            let temp = TempDir::new().unwrap();
            let source = temp.path().join(format!("parent-{name}.jsonl"));
            let mut events = vec![
                json!({"type":"message_delta","text":"partial"}),
                json!({"type":"message_complete","message_id":"main-message","text":"done"}),
                json!({"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash","input":{"command":"pwd"}}),
                json!({"type":"tool_end","tool_use_id":"main-tool","status":"success","output":{"stdout":"/tmp"}}),
            ];
            if let Some(parent) = parent {
                for event in &mut events {
                    event
                        .as_object_mut()
                        .unwrap()
                        .insert("parent_tool_use_id".to_string(), parent.clone());
                }
            }

            let mut body = Vec::new();
            for (index, event) in events.iter().enumerate() {
                serde_json::to_writer(
                    &mut body,
                    &json!({"_bus":true,"seq":index + 1,"ts":"t","event":event}),
                )
                .unwrap();
                body.push(b'\n');
            }
            fs::write(&source, &body).unwrap();
            let file = File::open(&source).unwrap();
            assert_eq!(
                safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
                body.len() as u64,
                "{name} parent scope must be publishable after completion"
            );

            let mut state = state(&temp);
            for (index, event) in events.into_iter().enumerate() {
                state
                    .handle_event(&json!({"_bus":true,"seq":index + 1,"ts":"t","event":event}))
                    .unwrap();
            }
            state.flush_page().unwrap();
            let mut entries = Vec::new();
            for page in 1..=state.page_count {
                entries.extend(page_entries(&state.pages_dir, page));
            }

            assert!(
                matches!(
                    entries.iter().find(|entry| matches!(entry, HistoryEntry::Assistant { id, .. } if id == "main-message")),
                    Some(HistoryEntry::Assistant { content, last_seq: 2, .. }) if content.preview == "done"
                ),
                "{name} parent scope must project the assistant into main history"
            );
            assert!(
                matches!(
                    entries.iter().find(|entry| matches!(entry, HistoryEntry::Tool { tool_use_id, .. } if tool_use_id == "main-tool")),
                    Some(HistoryEntry::Tool { status, output, last_seq: 4, .. })
                        if status == "success" && output["stdout"] == "/tmp"
                ),
                "{name} parent scope must project the tool into main history"
            );
            assert!(state.subhistories.is_empty());
        }
    }

    #[test]
    fn semantic_boundary_closes_subhistory_tool_when_end_omits_parent() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        let events = [
            json!({"_bus":true,"seq":1,"event":{"type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash"}}),
            json!({"_bus":true,"seq":2,"event":{"type":"tool_end","tool_use_id":"child-tool","status":"success"}}),
        ];
        let mut body = Vec::new();
        for event in events {
            serde_json::to_writer(&mut body, &event).unwrap();
            body.push(b'\n');
        }
        fs::write(&source, &body).unwrap();
        let file = File::open(&source).unwrap();

        assert_eq!(
            safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
            body.len() as u64
        );
    }

    #[test]
    fn builder_closes_subhistory_tool_when_end_omits_parent() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash","input":{"command":"pwd"}}),
            ),
            (
                2,
                json!({"type":"tool_end","tool_use_id":"child-tool","tool_name":"Bash","status":"success","output":{"stdout":"/tmp"}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        let subhistory = state.subhistories.get("parent-tool").unwrap();
        let entries = page_entries(&subhistory.pages_dir, 1);
        assert!(matches!(&entries[0], HistoryEntry::Tool {
            status,
            output,
            last_seq: 2,
            ..
        } if status == "success" && output["stdout"] == "/tmp"));
        assert!(subhistory.open_tools.is_empty());
    }

    #[test]
    fn semantic_boundary_advances_after_interrupted_turn_and_later_turns() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("events.jsonl");
        let events = [
            json!({"_bus":true,"seq":1,"event":{"type":"user_message","text":"first"}}),
            json!({"_bus":true,"seq":2,"event":{"type":"thinking_delta","text":"partial thought"}}),
            json!({"_bus":true,"seq":3,"event":{"type":"tool_start","tool_use_id":"abandoned","tool_name":"Bash"}}),
            json!({"_bus":true,"seq":4,"event":{"type":"run_state","state":"idle"}}),
            json!({"_bus":true,"seq":5,"event":{"type":"user_message","text":"second"}}),
            json!({"_bus":true,"seq":6,"event":{"type":"message_complete","message_id":"second-answer","text":"done"}}),
            json!({"_bus":true,"seq":7,"event":{"type":"run_state","state":"idle"}}),
        ];
        let mut body = Vec::new();
        for event in events {
            serde_json::to_writer(&mut body, &event).unwrap();
            body.push(b'\n');
        }
        fs::write(&source, &body).unwrap();
        let file = File::open(&source).unwrap();

        assert_eq!(
            safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
            body.len() as u64
        );
    }

    #[test]
    fn builder_settles_interrupted_main_and_subhistory_work_at_turn_boundary() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let events = [
            json!({"type":"user_message","text":"question"}),
            json!({"type":"message_delta","text":"partial answer"}),
            json!({"type":"thinking_delta","text":"partial thought"}),
            json!({"type":"tool_start","tool_use_id":"parent","tool_name":"Agent","input":{}}),
            json!({"type":"message_delta","parent_tool_use_id":"parent","text":"child partial"}),
            json!({"type":"tool_start","parent_tool_use_id":"parent","tool_use_id":"child","tool_name":"Bash","input":{}}),
            json!({"type":"run_state","state":"stopped"}),
            json!({"type":"user_message","text":"next turn"}),
            json!({"type":"message_complete","message_id":"next-answer","text":"clean answer"}),
            json!({"type":"run_state","state":"idle"}),
        ];
        for (index, event) in events.into_iter().enumerate() {
            state
                .handle_event(&json!({"_bus":true,"seq":index + 1,"ts":"t","event":event}))
                .unwrap();
        }

        let mut main_entries = Vec::new();
        for page in 1..=state.page_count {
            main_entries.extend(page_entries(&state.pages_dir, page));
        }
        main_entries.extend(state.page.clone());
        assert!(matches!(
            main_entries.iter().find(|entry| matches!(entry, HistoryEntry::Assistant { id, .. } if id == "assistant-incomplete-2")),
            Some(HistoryEntry::Assistant { content, thinking: Some(thinking), last_seq: 7, .. })
                if content.preview == "partial answer" && thinking.preview == "partial thought"
        ));
        assert!(matches!(
            main_entries.iter().find(|entry| matches!(entry, HistoryEntry::Tool { tool_use_id, .. } if tool_use_id == "parent")),
            Some(HistoryEntry::Tool { status, output, last_seq: 7, .. })
                if status == "error" && output["error"] == "Turn ended before tool result"
        ));
        assert!(matches!(
            main_entries.last(),
            Some(HistoryEntry::Assistant { id, thinking: None, .. }) if id == "next-answer"
        ));

        let sub = state.subhistories.get("parent").unwrap();
        let mut sub_entries = Vec::new();
        for page in 1..=sub.page_count {
            sub_entries.extend(page_entries(&sub.pages_dir, page));
        }
        sub_entries.extend(sub.page.clone());
        assert!(matches!(
            sub_entries.iter().find(|entry| matches!(entry, HistoryEntry::Assistant { id, .. } if id == "sub-assistant-incomplete-5")),
            Some(HistoryEntry::Assistant { content, last_seq: 7, .. }) if content.preview == "child partial"
        ));
        assert!(matches!(
            sub_entries.iter().find(|entry| matches!(entry, HistoryEntry::Tool { tool_use_id, .. } if tool_use_id == "child")),
            Some(HistoryEntry::Tool { status, last_seq: 7, .. }) if status == "error"
        ));
    }

    #[test]
    fn content_is_inline_below_limit_and_chunked_above_limit() {
        let temp = TempDir::new().unwrap();
        let state = state(&temp);
        let small = state.content_from_text("small", "hello").unwrap();
        assert!(!small.truncated);
        assert_eq!(small.preview, "hello");

        let large = "界".repeat(CONTENT_INLINE_LIMIT);
        let projected = state.content_from_text("large", &large).unwrap();
        assert!(projected.truncated);
        assert!(projected.content_id.is_some());
        assert!(std::str::from_utf8(projected.preview.as_bytes()).is_ok());
    }

    #[test]
    fn page_hard_limit_and_cursor_order_are_stable() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for seq in 1..=205 {
            let id = format!("u-{seq}");
            state
                .push(HistoryEntry::User {
                    id: id.clone(),
                    anchor_id: id,
                    ts: String::new(),
                    content: HistoryContent {
                        preview: "x".to_string(),
                        byte_length: 1,
                        truncated: false,
                        content_id: None,
                        encoding: "text".to_string(),
                    },
                    cli_uuid: None,
                    attachments: vec![],
                    first_seq: seq,
                    last_seq: seq,
                })
                .unwrap();
        }
        state.flush_page().unwrap();
        assert_eq!(state.page_count, 3);
        for page in 1..=3 {
            let len = fs::metadata(state.pages_dir.join(format!("{page:08}.json")))
                .unwrap()
                .len();
            assert!(len <= PAGE_BYTE_LIMIT as u64);
        }
        let generation = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_cursor(&format!("{generation}:3")).unwrap(),
            (generation, 3)
        );
        assert!(parse_cursor("../bad:1").is_err());
    }

    #[test]
    fn deltas_fold_into_final_assistant_entry() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"message_delta","run_id":"run-test","text":"hel"}),
            ),
            (
                2,
                json!({"type":"message_delta","run_id":"run-test","text":"lo"}),
            ),
            (
                3,
                json!({"type":"thinking_delta","run_id":"run-test","text":"hmm"}),
            ),
            (
                4,
                json!({"type":"message_complete","run_id":"run-test","message_id":"m1","text":"hello"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        match &state.page[0] {
            HistoryEntry::Assistant {
                content,
                thinking,
                first_seq,
                last_seq,
                ..
            } => {
                assert_eq!(content.preview, "hello");
                assert_eq!(thinking.as_ref().unwrap().preview, "hmm");
                assert_eq!((*first_seq, *last_seq), (1, 4));
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn imported_overlap_preserves_unique_render_ids_and_reducer_dedup_semantics() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"user_message","uuid":"same-user","text":"hello"}),
            ),
            (
                2,
                json!({"type":"message_complete","message_id":"same-assistant","text":"first"}),
            ),
            (
                3,
                json!({"type":"tool_start","tool_use_id":"same-tool","tool_name":"Read","input":{"path":"/first"}}),
            ),
            (
                4,
                json!({"type":"tool_end","tool_use_id":"same-tool","status":"success","output":{"ok":true}}),
            ),
            (
                5,
                json!({"type":"command_output","content":"force the first identities onto disk"}),
            ),
            (
                101,
                json!({"type":"user_message","uuid":"same-user","text":"hello"}),
            ),
            (
                102,
                json!({"type":"message_complete","message_id":"same-assistant","text":"duplicate"}),
            ),
            (
                103,
                json!({"type":"tool_start","tool_use_id":"same-tool","tool_name":"Read","input":{"path":"/duplicate"}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
            if seq == 5 {
                state.flush_page().unwrap();
            }
        }
        state.flush_page().unwrap();

        let mut entries = Vec::new();
        for page in 1..=state.page_count {
            entries.extend(page_entries(&state.pages_dir, page));
        }
        let ids: Vec<&str> = entries
            .iter()
            .map(|entry| match entry {
                HistoryEntry::User { id, .. }
                | HistoryEntry::Assistant { id, .. }
                | HistoryEntry::Tool { id, .. }
                | HistoryEntry::CommandOutput { id, .. }
                | HistoryEntry::Placeholder { id, .. } => id.as_str(),
            })
            .collect();
        assert_eq!(ids.len(), ids.iter().copied().collect::<HashSet<_>>().len());
        assert_eq!(
            ids,
            vec![
                "user-1",
                "same-assistant",
                "same-tool",
                "command-5",
                "user-101"
            ]
        );
        assert_eq!(state.total_turns, 2);
        assert!(matches!(&entries[0], HistoryEntry::User {
            anchor_id,
            cli_uuid: Some(cli_uuid),
            ..
        } if anchor_id == "same-user" && cli_uuid == "same-user"));
        assert!(
            matches!(&entries[1], HistoryEntry::Assistant { content, .. }
            if content.preview == "first")
        );
        assert!(
            matches!(&entries[2], HistoryEntry::Tool { input, status, .. }
            if input["path"] == "/first" && status == "success")
        );
    }

    #[test]
    fn duplicate_main_completion_discards_streamed_overlap() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let large = "x".repeat(CONTENT_INLINE_LIMIT + 1);
        for (seq, event) in [
            (
                1,
                json!({"type":"message_complete","message_id":"same-assistant","text":"first"}),
            ),
            (2, json!({"type":"message_delta","text":large})),
            (
                3,
                json!({"type":"thinking_delta","text":"y".repeat(CONTENT_INLINE_LIMIT + 1)}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let message_spool = state.pending_message.path.clone();
        let thinking_spool = state.pending_thinking.path.clone();
        assert!(message_spool.exists());
        assert!(thinking_spool.exists());

        state
            .handle_event(&json!({
                "_bus":true,
                "seq":4,
                "ts":"t",
                "event":{"type":"message_complete","message_id":"same-assistant","text":"duplicate"}
            }))
            .unwrap();
        state
            .settle_interrupted_work(5, "Turn ended before tool result")
            .unwrap();

        assert_eq!(state.page.len(), 1);
        assert!(
            matches!(&state.page[0], HistoryEntry::Assistant { content, .. } if content.preview == "first")
        );
        assert!(state.pending_message.is_empty());
        assert!(state.pending_thinking.is_empty());
        assert!(!message_spool.exists());
        assert!(!thinking_spool.exists());
    }

    #[test]
    fn duplicate_subhistory_completion_discards_streamed_overlap() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let large = "x".repeat(CONTENT_INLINE_LIMIT + 1);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","tool_use_id":"parent","tool_name":"Task","input":{}}),
            ),
            (
                2,
                json!({"type":"message_complete","parent_tool_use_id":"parent","message_id":"same-child","text":"first"}),
            ),
            (
                3,
                json!({"type":"message_delta","parent_tool_use_id":"parent","text":large}),
            ),
            (
                4,
                json!({"type":"thinking_delta","parent_tool_use_id":"parent","text":"y".repeat(CONTENT_INLINE_LIMIT + 1)}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let subhistory = state.subhistories.get("parent").unwrap();
        let message_spool = subhistory.pending_message.path.clone();
        let thinking_spool = subhistory.pending_thinking.path.clone();
        assert!(message_spool.exists());
        assert!(thinking_spool.exists());

        state
            .handle_event(&json!({
                "_bus":true,
                "seq":5,
                "ts":"t",
                "event":{"type":"message_complete","parent_tool_use_id":"parent","message_id":"same-child","text":"duplicate"}
            }))
            .unwrap();
        let subhistory = state.subhistories.get_mut("parent").unwrap();
        subhistory
            .settle_interrupted_work(6, "Turn ended before tool result")
            .unwrap();

        assert_eq!(subhistory.page.len(), 1);
        assert!(
            matches!(&subhistory.page[0], HistoryEntry::Assistant { content, .. } if content.preview == "first")
        );
        assert!(subhistory.pending_message.is_empty());
        assert!(subhistory.pending_thinking.is_empty());
        assert!(!message_spool.exists());
        assert!(!thinking_spool.exists());
    }

    #[test]
    fn tool_result_updates_single_entry_and_externalizes_large_output() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .handle_event(&json!({"_bus":true,"seq":1,"ts":"t","event":{
                "type":"tool_start","run_id":"run-test","tool_use_id":"tool-1","tool_name":"Bash","input":{"cmd":"x"}
            }}))
            .unwrap();
        let output = "z".repeat(CONTENT_INLINE_LIMIT + 1);
        state
            .handle_event(&json!({"_bus":true,"seq":2,"ts":"t","event":{
                "type":"tool_end","run_id":"run-test","tool_use_id":"tool-1","tool_name":"Bash","status":"success","output":{"text":output}
            }}))
            .unwrap();
        let entries = page_entries(&state.pages_dir, 1);
        match &entries[0] {
            HistoryEntry::Tool {
                status,
                output_content,
                last_seq,
                ..
            } => {
                assert_eq!(status, "success");
                assert!(output_content.as_ref().unwrap().truncated);
                assert_eq!(*last_seq, 2);
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn open_tool_survives_page_flush_before_tool_end() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .handle_event(&json!({"_bus":true,"seq":1,"ts":"t","event":{
                "type":"tool_start","run_id":"run-test","tool_use_id":"tool-1","tool_name":"Bash","input":{"cmd":"x"}
            }}))
            .unwrap();
        for seq in 2..=(PAGE_ENTRY_LIMIT as u64 + 2) {
            let id = format!("user-{seq}");
            state
                .push(HistoryEntry::User {
                    id: id.clone(),
                    anchor_id: id,
                    ts: String::new(),
                    content: HistoryContent {
                        preview: "x".to_string(),
                        byte_length: 1,
                        truncated: false,
                        content_id: None,
                        encoding: "text".to_string(),
                    },
                    cli_uuid: None,
                    attachments: vec![],
                    first_seq: seq,
                    last_seq: seq,
                })
                .unwrap();
        }
        state
            .handle_event(&json!({"_bus":true,"seq":200,"ts":"t","event":{
                "type":"tool_end","run_id":"run-test","tool_use_id":"tool-1","status":"success","output":{"text":"done"}
            }}))
            .unwrap();
        let entries = page_entries(&state.pages_dir, 1);
        match &entries[0] {
            HistoryEntry::Tool {
                status,
                output,
                last_seq,
                ..
            } => {
                assert_eq!(status, "success");
                assert_eq!(output, &json!({"text":"done"}));
                assert_eq!(*last_seq, 200);
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn tools_remain_in_start_order_when_they_finish_out_of_order() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, id) in [(1, "first"), (2, "second")] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":{
                    "type":"tool_start","tool_use_id":id,"tool_name":"Bash","input":{}
                }}))
                .unwrap();
        }
        for (seq, id) in [(3, "second"), (4, "first")] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":{
                    "type":"tool_end","tool_use_id":id,"status":"success","output":{}
                }}))
                .unwrap();
        }

        let first = page_entries(&state.pages_dir, 1);
        let second = page_entries(&state.pages_dir, 2);
        assert!(
            matches!(&first[0], HistoryEntry::Tool { tool_use_id, .. } if tool_use_id == "first")
        );
        assert!(
            matches!(&second[0], HistoryEntry::Tool { tool_use_id, .. } if tool_use_id == "second")
        );
    }

    #[test]
    fn claude_streamed_tool_input_is_reconstructed() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","tool_use_id":"tool-1","tool_name":"Bash","input":null}),
            ),
            (
                2,
                json!({"type":"tool_input_delta","tool_use_id":"tool-1","partial_json":"{\"cmd\":"}),
            ),
            (
                3,
                json!({"type":"tool_input_delta","tool_use_id":"tool-1","partial_json":"\"echo hi\"}"}),
            ),
            (
                4,
                json!({"type":"tool_end","tool_use_id":"tool-1","status":"success","output":{}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        let entries = page_entries(&state.pages_dir, 1);
        assert!(
            matches!(&entries[0], HistoryEntry::Tool { input, input_content: None, .. } if input == &json!({"cmd":"echo hi"}))
        );
    }

    #[test]
    fn pending_interactions_are_retained_and_resolved_by_request_id() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"permission_prompt","request_id":"permission-1","tool_name":"Bash"}),
            ),
            (
                2,
                json!({"type":"elicitation_prompt","request_id":"elicitation-1","message":"choose"}),
            ),
            (
                3,
                json!({"type":"control_cancelled","request_id":"elicitation-1"}),
            ),
            (
                4,
                json!({"type":"interaction_resolved","request_id":"permission-1"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        assert!(!state.pending_interactions.contains_key("permission-1"));
        assert!(!state.pending_interactions.contains_key("elicitation-1"));
    }

    #[test]
    fn uncertain_interaction_keeps_prompt_and_lifecycle_in_replay_order() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"elicitation_prompt","request_id":"elicitation-1","mcp_server_name":"server","message":"choose"}),
            ),
            (
                2,
                json!({"type":"interaction_response_started","request_id":"elicitation-1","interaction_kind":"elicitation"}),
            ),
            (
                3,
                json!({"type":"interaction_response_failed","request_id":"elicitation-1","interaction_kind":"elicitation","error":"flush failed"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        assert!(state.pending_interactions.contains_key("elicitation-1"));
        let lifecycle = state.state_events.get("interaction:elicitation-1").unwrap();
        assert_eq!(lifecycle["type"], "interaction_response_failed");
        assert_eq!(lifecycle["_seq"], 3);
    }

    #[test]
    fn summary_replays_uncertain_lifecycle_after_its_prompt() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"hook_callback","request_id":"hook-1","hook_event":"PreToolUse","hook_id":"h","data":{}}),
            ),
            (
                2,
                json!({"type":"interaction_response_started","request_id":"hook-1","interaction_kind":"hook"}),
            ),
            (
                3,
                json!({"type":"interaction_response_failed","request_id":"hook-1","interaction_kind":"hook","error":"flush failed"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let source = temp.path().join("events.jsonl");
        fs::write(&source, b"{}\n").unwrap();
        let metadata = source.metadata().unwrap();
        state
            .finish(
                metadata.len(),
                modified_ns(&metadata),
                hash_prefix(&source, metadata.len()).unwrap(),
            )
            .unwrap();
        let summary: HistorySummary = serde_json::from_slice(
            &fs::read(temp.path().join("gen").join("summary.json")).unwrap(),
        )
        .unwrap();

        assert_eq!(summary.state_events.len(), 2);
        assert_eq!(summary.state_events[0]["type"], "hook_callback");
        assert_eq!(
            summary.state_events[1]["type"],
            "interaction_response_failed"
        );
    }

    #[test]
    fn answered_user_input_is_not_restored_as_pending() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","tool_use_id":"question-1","tool_name":"AskUserQuestion","input":{"questions":[]}}),
            ),
            (
                2,
                json!({"type":"tool_end","tool_use_id":"question-1","tool_name":"AskUserQuestion","status":"error","output":{}}),
            ),
            (
                3,
                json!({"type":"interaction_response_started","request_id":"question-1","interaction_kind":"user_input"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        let entries = page_entries(&state.pages_dir, 1);
        assert!(matches!(
            &entries[0],
            HistoryEntry::Tool { status, .. } if status == "response_pending"
        ));
    }

    #[test]
    fn failed_user_input_response_is_terminal_error() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","tool_use_id":"question-1","tool_name":"AskUserQuestion","input":{"questions":[]}}),
            ),
            (
                2,
                json!({"type":"tool_end","tool_use_id":"question-1","tool_name":"AskUserQuestion","status":"error","output":{}}),
            ),
            (
                3,
                json!({"type":"interaction_response_started","request_id":"question-1","interaction_kind":"user_input"}),
            ),
            (
                4,
                json!({"type":"interaction_response_failed","request_id":"question-1","interaction_kind":"user_input","error":"closed"}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }

        assert!(!state.pending_user_inputs.contains_key("question-1"));
        let entries = page_entries(&state.pages_dir, 1);
        assert!(matches!(
            &entries[0],
            HistoryEntry::Tool { status, .. } if status == "response_failed"
        ));
    }

    #[test]
    fn oversized_json_string_combines_utf16_surrogate_pairs() {
        let temp = TempDir::new().unwrap();
        let input = b"\\uD83D\\uDE00\"".iter().copied().map(Ok);
        let spool =
            capture_json_string(&mut input.into_iter(), temp.path().join("emoji.bin")).unwrap();
        assert_eq!(spool.inline, "😀");
    }

    #[test]
    fn oversized_json_scanners_reject_excessive_nesting() {
        let temp = TempDir::new().unwrap();
        let nested_value = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_NESTING_DEPTH + 1),
            "]".repeat(MAX_JSON_NESTING_DEPTH + 1)
        );
        let mut value_bytes = nested_value.bytes().skip(1).map(Ok);
        let value_error =
            capture_json_value(&mut value_bytes, b'[', temp.path().join("nested-value.bin"))
                .unwrap_err();
        assert!(value_error.contains("nesting exceeds hard limit"));

        let envelope = format!(
            "{}{{\"input\":0}}{}",
            "[".repeat(MAX_JSON_NESTING_DEPTH + 1),
            "]".repeat(MAX_JSON_NESTING_DEPTH + 1)
        );
        let envelope_path = temp.path().join("nested-envelope.json");
        fs::write(&envelope_path, envelope).unwrap();
        let envelope_error = extract_json_value_field(
            &envelope_path,
            "input",
            temp.path().join("nested-field.bin"),
        )
        .unwrap_err();
        assert!(envelope_error.contains("nesting exceeds hard limit"));
    }

    #[test]
    fn oversized_json_scanners_reject_mismatched_envelope_delimiters() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("mismatched-envelope.json");
        fs::write(&path, r#"{{"event":{"input":[]}]}"#).unwrap();
        let error =
            extract_json_value_field(&path, "input", temp.path().join("value.bin")).unwrap_err();
        assert!(error.contains("mismatched oversized JSON envelope delimiter"));
    }

    #[test]
    fn oversized_json_scalar_rejects_invalid_or_unbounded_values() {
        let temp = TempDir::new().unwrap();
        let mut invalid = b"oops,".iter().copied().map(Ok);
        let error =
            capture_json_value(&mut invalid, b'n', temp.path().join("invalid.bin")).unwrap_err();
        assert!(error.contains("invalid oversized JSON scalar"));

        let mut huge = (0..=MAX_JSON_SCALAR_BYTES).map(|_| Ok(b'1'));
        let error = capture_json_value(&mut huge, b'1', temp.path().join("huge.bin")).unwrap_err();
        assert!(error.contains("scalar exceeds hard limit"));
    }

    #[test]
    fn resource_limits_fail_explicitly() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for index in 0..MAX_OPEN_TOOLS {
            state
                .handle_event(&json!({"_bus":true,"seq":index + 1,"ts":"t","event":{
                    "type":"tool_start","tool_use_id":format!("tool-{index}"),"tool_name":"Bash","input":{}
                }}))
                .unwrap();
        }
        let error = state
            .handle_event(
                &json!({"_bus":true,"seq":MAX_OPEN_TOOLS + 1,"ts":"t","event":{
                    "type":"tool_start","tool_use_id":"overflow","tool_name":"Bash","input":{}
                }}),
            )
            .unwrap_err();
        assert!(error.contains("open tool limit"));
    }

    #[test]
    fn streaming_text_spools_after_inline_limit_without_data_loss() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let part = "界".repeat(CONTENT_INLINE_LIMIT / 3);
        for seq in 1..=4 {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":{
                    "type":"message_delta","run_id":"run-test","text":part
                }}))
                .unwrap();
        }
        assert!(state.pending_message.file.is_some());
        assert!(state.pending_message.inline.is_empty());
        let expected_bytes = part.len() as u64 * 4;
        state
            .handle_event(&json!({"_bus":true,"seq":5,"ts":"t","event":{
                "type":"message_complete","run_id":"run-test","message_id":"m1"
            }}))
            .unwrap();
        match state.page.last().unwrap() {
            HistoryEntry::Assistant { content, .. } => {
                assert!(content.truncated);
                assert_eq!(content.byte_length, expected_bytes);
                let blob = fs::read(
                    state
                        .blobs_dir
                        .join(format!("{}.bin", content.content_id.as_ref().unwrap())),
                )
                .unwrap();
                assert_eq!(blob.len() as u64, expected_bytes);
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn oversized_entry_falls_back_to_bounded_placeholder() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .push(HistoryEntry::User {
                id: "u1".to_string(),
                anchor_id: "u1".to_string(),
                ts: String::new(),
                content: HistoryContent {
                    preview: "x".to_string(),
                    byte_length: 1,
                    truncated: false,
                    content_id: None,
                    encoding: "text".to_string(),
                },
                cli_uuid: None,
                attachments: vec![json!({"contentBase64":"x".repeat(ENTRY_BYTE_LIMIT)})],
                first_seq: 1,
                last_seq: 1,
            })
            .unwrap();
        assert!(matches!(state.page[0], HistoryEntry::Placeholder { .. }));
        assert!(serde_json::to_vec(&state.page[0]).unwrap().len() <= ENTRY_BYTE_LIMIT);
    }

    #[test]
    fn bounded_line_reader_spools_without_growing_inline_buffer() {
        let temp = TempDir::new().unwrap();
        let data = vec![b'x'; PARSE_LINE_LIMIT + 1024];
        let mut reader = BufReader::with_capacity(4096, data.as_slice());
        let path = temp.path().join("oversized.bin");
        match read_bounded_line(&mut reader, &path).unwrap() {
            ScannedLine::External {
                path, byte_length, ..
            } => {
                assert_eq!(byte_length, data.len() as u64);
                assert_eq!(fs::metadata(path).unwrap().len(), data.len() as u64);
            }
            other => panic!("unexpected line: {other:?}"),
        }
    }

    #[test]
    fn oversized_line_prefix_recovers_envelope_sequence() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("oversized-envelope.json");
        fs::write(
            &path,
            format!(
                "{{\"_bus\":true,\"seq\":987654,\"event\":{{\"text\":\"{}\"}}}}",
                "x".repeat(128 * 1024)
            ),
        )
        .unwrap();
        assert_eq!(oversized_metadata(&path).unwrap()["seq"], 987654);
    }

    #[test]
    fn oversized_metadata_ignores_nested_decoys_and_normalizes_parent_scope() {
        let cases = [
            ("absent", "", None),
            ("null", r#","parent_tool_use_id":null"#, None),
            ("empty", r#","parent_tool_use_id":"""#, None),
            (
                "subhistory",
                r#","parent_tool_use_id":"actual-parent""#,
                Some("actual-parent"),
            ),
        ];
        for (name, parent_field, expected_parent) in cases {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join(format!("metadata-{name}.json"));
            let line = format!(
                r#"{{"_bus":true,"seq":42,"event":{{"input":{{"seq":999,"type":"message_complete","status":"error","parent_tool_use_id":"decoy"}},"type":"tool_start","tool_use_id":"real-tool","tool_name":"Bash"{parent_field}}},"ts":"real-ts"}}"#
            );
            fs::write(&path, line).unwrap();

            let metadata = oversized_metadata(&path).unwrap();
            assert_eq!(metadata["seq"], 42, "{name}");
            assert_eq!(metadata["type"], "tool_start", "{name}");
            assert_eq!(metadata["tool_use_id"], "real-tool", "{name}");
            assert_eq!(metadata["ts"], "real-ts", "{name}");
            assert_eq!(
                normalized_parent_tool_use_id(&metadata),
                expected_parent,
                "{name}"
            );
            assert!(metadata.get("status").is_none(), "{name}");
        }
    }

    #[test]
    fn oversized_tool_events_ignore_nested_metadata_decoys() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let start_path = state.spools_dir.join("oversized-decoy-start.bin");
        let start = format!(
            r#"{{"_bus":true,"seq":1,"ts":"t","event":{{"input":{{"seq":999,"type":"message_complete","parent_tool_use_id":"decoy","payload":"{}"}},"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash","parent_tool_use_id":""}}}}"#,
            "x".repeat(PARSE_LINE_LIMIT)
        );
        fs::write(&start_path, &start).unwrap();
        state
            .handle_oversized_event(start_path, start.len() as u64, 1)
            .unwrap();
        assert!(state.open_tools.contains_key("main-tool"));
        assert!(state.subhistories.is_empty());

        let end_path = state.spools_dir.join("oversized-decoy-end.bin");
        let end = format!(
            r#"{{"_bus":true,"seq":2,"ts":"t","event":{{"output":{{"seq":888,"type":"tool_start","status":"error","parent_tool_use_id":"decoy","stdout":"{}"}},"type":"tool_end","tool_use_id":"main-tool","tool_name":"Bash","status":"success","parent_tool_use_id":""}}}}"#,
            "x".repeat(PARSE_LINE_LIMIT)
        );
        fs::write(&end_path, &end).unwrap();
        state
            .handle_oversized_event(end_path, end.len() as u64, 2)
            .unwrap();

        let entries = page_entries(&state.pages_dir, 1);
        assert!(matches!(&entries[0], HistoryEntry::Tool {
            status,
            last_seq: 2,
            output_content: Some(content),
            ..
        } if status == "success" && content.truncated));
        assert!(state.open_tools.is_empty());
    }

    #[test]
    fn oversized_nested_metadata_decoys_do_not_block_safe_projection() {
        let temp = TempDir::new().unwrap();
        let start = format!(
            r#"{{"_bus":true,"seq":1,"ts":"t","event":{{"input":{{"seq":999,"type":"message_complete","status":"error","parent_tool_use_id":"decoy","payload":"{}"}},"type":"tool_start","tool_use_id":"main-tool","tool_name":"Bash"}}}}"#,
            "x".repeat(PARSE_LINE_LIMIT)
        );
        let end = format!(
            r#"{{"_bus":true,"seq":2,"ts":"t","event":{{"output":{{"seq":888,"type":"tool_start","status":"error","parent_tool_use_id":"decoy","stdout":"{}"}},"type":"tool_end","tool_use_id":"main-tool","tool_name":"Bash","status":"success"}}}}"#,
            "x".repeat(PARSE_LINE_LIMIT)
        );
        let body = format!("{start}\n{end}\n");
        let path = temp.path().join("events.jsonl");
        fs::write(&path, &body).unwrap();
        let file = File::open(path).unwrap();

        assert_eq!(
            safe_projection_size(&file, body.len() as u64, temp.path()).unwrap(),
            body.len() as u64
        );
    }

    #[test]
    fn oversized_message_complete_keeps_assistant_semantics() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let path = state.spools_dir.join("oversized-message.bin");
        let expected = "large answer ".repeat(PARSE_LINE_LIMIT / 6);
        let line = json!({
            "_bus":true,
            "seq":42,
            "ts":"t",
            "event":{
                "type":"message_complete",
                "run_id":"run-test",
                "message_id":"message-42",
                "model":"claude-sonnet-4-5",
                "text":expected
            }
        })
        .to_string();
        fs::write(&path, &line).unwrap();

        state
            .handle_oversized_event(path, line.len() as u64, 1)
            .unwrap();

        assert_eq!(state.page.len(), 1);
        match &state.page[0] {
            HistoryEntry::Assistant {
                id,
                content,
                model,
                first_seq,
                last_seq,
                ..
            } => {
                assert_eq!(id, "message-42");
                assert!(content.truncated);
                assert_eq!(content.byte_length, expected.len() as u64);
                assert_eq!(model.as_deref(), Some("claude-sonnet-4-5"));
                assert_eq!((*first_seq, *last_seq), (42, 42));
            }
            other => panic!("unexpected entry: {other:?}"),
        }
        assert!(state.pending_message.is_empty());
    }

    #[test]
    fn oversized_user_message_keeps_identifiers_and_attachments() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let path = state.spools_dir.join("oversized-user.bin");
        let expected = "large prompt ".repeat(PARSE_LINE_LIMIT / 6);
        let attachments = json!([{
            "name":"screenshot.png",
            "mime_type":"image/png",
            "size":12345
        }]);
        let line = json!({
            "_bus":true,
            "seq":43,
            "ts":"t",
            "event":{
                "type":"user_message",
                "run_id":"run-test",
                "uuid":"cli-user-43",
                "client_uuid":"client-user-43",
                "attachments":attachments,
                "text":expected
            }
        })
        .to_string();
        fs::write(&path, &line).unwrap();

        state
            .handle_oversized_event(path, line.len() as u64, 1)
            .unwrap();

        assert!(matches!(&state.page[0], HistoryEntry::User {
            id,
            cli_uuid: Some(cli_uuid),
            attachments: actual_attachments,
            content,
            ..
        } if id == "user-43"
            && cli_uuid == "cli-user-43"
            && actual_attachments == attachments.as_array().unwrap()
            && content.byte_length == expected.len() as u64));

        let client_path = state.spools_dir.join("oversized-client-user.bin");
        let client_line = json!({
            "_bus":true,
            "seq":44,
            "ts":"t",
            "event":{
                "type":"user_message",
                "run_id":"run-test",
                "client_uuid":"client-only-44",
                "attachments":[],
                "text":expected
            }
        })
        .to_string();
        fs::write(&client_path, &client_line).unwrap();
        state
            .handle_oversized_event(client_path, client_line.len() as u64, 2)
            .unwrap();
        assert!(
            matches!(&state.page[1], HistoryEntry::User { id, cli_uuid: None, .. }
            if id == "user-44")
        );
    }

    #[test]
    fn oversized_tool_end_updates_existing_tool_without_placeholder() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .handle_event(&json!({"_bus":true,"seq":1,"ts":"t","event":{
                "type":"tool_start","tool_use_id":"tool-1","tool_name":"Bash","input":{}
            }}))
            .unwrap();
        let path = state.spools_dir.join("oversized-tool.bin");
        let line = json!({
            "_bus":true,
            "seq":2,
            "ts":"t",
            "event":{
                "type":"tool_end",
                "run_id":"run-test",
                "tool_use_id":"tool-1",
                "tool_name":"Bash",
                "status":"success",
                "duration_ms":9876,
                "output":{"stdout":"x".repeat(PARSE_LINE_LIMIT)},
                "tool_use_result":{"exitCode":0,"detail":"result-only"}
            }
        })
        .to_string();
        fs::write(&path, &line).unwrap();

        state
            .handle_oversized_event(path, line.len() as u64, 1)
            .unwrap();

        let entries = page_entries(&state.pages_dir, 1);
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], HistoryEntry::Tool {
            tool_name,
            status,
            output_content: Some(content),
            tool_use_result: Some(result),
            result_content: None,
            duration_ms: Some(9876),
            last_seq: 2,
            ..
        } if tool_name == "Bash"
            && status == "success"
            && content.truncated
            && result["detail"] == "result-only"
            && !content.preview.contains("tool_use_id")));
        assert!(state.open_tools.is_empty());
    }

    #[test]
    fn oversized_tool_end_without_parent_updates_subhistory_tool() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .handle_event(&json!({"_bus":true,"seq":1,"ts":"t","event":{
                "type":"tool_start","parent_tool_use_id":"parent-tool","tool_use_id":"child-tool","tool_name":"Bash","input":{}
            }}))
            .unwrap();
        let path = state.spools_dir.join("oversized-sub-tool.bin");
        let line = json!({
            "_bus":true,
            "seq":2,
            "ts":"t",
            "event":{
                "type":"tool_end",
                "run_id":"run-test",
                "tool_use_id":"child-tool",
                "tool_name":"Bash",
                "status":"success",
                "output":{"stdout":"x".repeat(PARSE_LINE_LIMIT)}
            }
        })
        .to_string();
        fs::write(&path, &line).unwrap();

        state
            .handle_oversized_event(path, line.len() as u64, 1)
            .unwrap();

        let subhistory = state.subhistories.get("parent-tool").unwrap();
        let entries = page_entries(&subhistory.pages_dir, 1);
        assert!(matches!(&entries[0], HistoryEntry::Tool {
            status,
            output_content: Some(content),
            last_seq: 2,
            ..
        } if status == "success" && content.truncated));
        assert!(subhistory.open_tools.is_empty());
    }

    #[test]
    fn history_identifiers_reject_path_like_values() {
        assert!(parse_cursor("../bad:1").is_err());
        assert!(parse_cursor("0123456789abcdef0123456789abcdef:1").is_ok());
        assert!(!is_hex_id("../bad", 32));
        assert!(!is_hex_id("g123456789abcdef0123456789abcdef", 32));
    }

    #[test]
    fn cleanup_keeps_current_and_process_retained_generations_only() {
        let temp = TempDir::new().unwrap();
        let current = "0123456789abcdef0123456789abcdef";
        let retained = "1123456789abcdef0123456789abcdef";
        let stale = "2123456789abcdef0123456789abcdef";
        let retained_dir = temp.path().join(retained);
        fs::create_dir(&retained_dir).unwrap();
        fs::write(
            retained_dir.join(".retain"),
            PROCESS_RETENTION_ID.as_bytes(),
        )
        .unwrap();

        assert!(!should_remove_generation(
            &temp.path().join(current),
            current,
            Some(current)
        ));
        assert!(!should_remove_generation(
            &retained_dir,
            retained,
            Some(current)
        ));
        assert!(should_remove_generation(
            &temp.path().join(stale),
            stale,
            Some(current)
        ));
        assert!(should_remove_generation(
            &temp.path().join("dead.build"),
            "dead.build",
            Some(current)
        ));
    }

    #[test]
    fn subagent_events_are_projected_into_independent_pages() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","run_id":"run-test","tool_use_id":"parent-1","tool_name":"Task","input":{"prompt":"work"}}),
            ),
            (
                2,
                json!({"type":"message_delta","run_id":"run-test","parent_tool_use_id":"parent-1","text":"child "}),
            ),
            (
                3,
                json!({"type":"message_complete","run_id":"run-test","parent_tool_use_id":"parent-1","message_id":"child-message","text":"child answer"}),
            ),
            (
                4,
                json!({"type":"tool_end","run_id":"run-test","tool_use_id":"parent-1","tool_name":"Task","status":"success","output":{"result":"done"}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let entries = page_entries(&state.pages_dir, 1);
        match &entries[0] {
            HistoryEntry::Tool { sub_history_id, .. } => {
                assert_eq!(
                    sub_history_id.as_deref(),
                    Some(sanitize_id("parent-1").as_str())
                );
            }
            other => panic!("unexpected parent entry: {other:?}"),
        }
        let sub = state.subhistories.remove("parent-1").unwrap();
        assert_eq!(sub.page.len(), 1);
        assert!(matches!(sub.page[0], HistoryEntry::Assistant { .. }));
        assert!(sub.pending_message.is_empty());
        assert!(sub.pending_thinking.is_empty());
        sub.finish().unwrap();
        let page = fs::read(
            state
                .generation_dir
                .join("subhistories")
                .join(sanitize_id("parent-1"))
                .join("pages/00000001.json"),
        )
        .unwrap();
        assert!(page.len() <= PAGE_BYTE_LIMIT);
        let entries: Vec<HistoryEntry> = serde_json::from_slice(&page).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn subhistory_externalizes_large_tool_fields() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        let large = "x".repeat(CONTENT_INLINE_LIMIT + 1024);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","parent_tool_use_id":"parent","tool_use_id":"child","tool_name":"Bash","input":{"command":large}}),
            ),
            (
                2,
                json!({"type":"tool_end","parent_tool_use_id":"parent","tool_use_id":"child","status":"success","output":{"stdout":large},"tool_use_result":{"data":large}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let sub = state.subhistories.get("parent").unwrap();
        let entries = page_entries(&sub.pages_dir, 1);
        match &entries[0] {
            HistoryEntry::Tool {
                input_content,
                output_content,
                result_content,
                ..
            } => {
                assert!(input_content.as_ref().is_some_and(|v| v.truncated));
                assert!(output_content.as_ref().is_some_and(|v| v.truncated));
                assert!(result_content.as_ref().is_some_and(|v| v.truncated));
                assert!(serde_json::to_vec(&entries[0]).unwrap().len() <= ENTRY_BYTE_LIMIT);
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn subhistory_preserves_unfinished_stream_at_eof() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        state
            .handle_event(&json!({
                "_bus":true,"seq":7,"ts":"t",
                "event":{"type":"message_delta","parent_tool_use_id":"parent","text":"partial answer"}
            }))
            .unwrap();
        let sub = state.subhistories.remove("parent").unwrap();
        let sub_dir = sub.pages_dir.parent().unwrap().to_path_buf();
        sub.finish().unwrap();
        let body = fs::read(sub_dir.join("pages/00000001.json")).unwrap();
        let entries: Vec<HistoryEntry> = serde_json::from_slice(&body).unwrap();
        match &entries[0] {
            HistoryEntry::Assistant {
                content, first_seq, ..
            } => {
                assert_eq!(content.preview, "partial answer");
                assert_eq!(*first_seq, 7);
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[test]
    fn nested_subhistory_is_linked_from_child_tool() {
        let temp = TempDir::new().unwrap();
        let mut state = state(&temp);
        for (seq, event) in [
            (
                1,
                json!({"type":"tool_start","parent_tool_use_id":"root","tool_use_id":"child","tool_name":"Task","input":{}}),
            ),
            (
                2,
                json!({"type":"message_delta","parent_tool_use_id":"child","text":"nested"}),
            ),
            (
                3,
                json!({"type":"tool_end","parent_tool_use_id":"root","tool_use_id":"child","status":"success","output":{}}),
            ),
        ] {
            state
                .handle_event(&json!({"_bus":true,"seq":seq,"ts":"t","event":event}))
                .unwrap();
        }
        let root = state.subhistories.get("root").unwrap();
        let entries = page_entries(&root.pages_dir, 1);
        match &entries[0] {
            HistoryEntry::Tool { sub_history_id, .. } => {
                assert_eq!(
                    sub_history_id.as_deref(),
                    Some(sanitize_id("child").as_str())
                );
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }
}

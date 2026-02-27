use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunStatus::Pending => write!(f, "pending"),
            RunStatus::Running => write!(f, "running"),
            RunStatus::Completed => write!(f, "completed"),
            RunStatus::Failed => write!(f, "failed"),
            RunStatus::Stopped => write!(f, "stopped"),
        }
    }
}

/// Run source — how this run was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunSource {
    Native,    // app-created run
    CliImport, // imported from CLI transcript
}

/// Import watermark for incremental CLI session sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWatermark {
    pub offset: u64,
    pub mtime_ns: u128,
    pub file_size: u64,
    pub last_uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunEventType {
    System,
    Stdout,
    Stderr,
    Command,
    User,
    Assistant,
}

impl std::fmt::Display for RunEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunEventType::System => write!(f, "system"),
            RunEventType::Stdout => write!(f, "stdout"),
            RunEventType::Stderr => write!(f, "stderr"),
            RunEventType::Command => write!(f, "command"),
            RunEventType::User => write!(f, "user"),
            RunEventType::Assistant => write!(f, "assistant"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: String,
    pub prompt: String,
    pub cwd: String,
    pub agent: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    pub status: RunStatus,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_subtype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The run_id this session was forked from (None if not a fork).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// User-assigned display name (None = use prompt as label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Remote host name (if this run is on a remote machine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_host_name: Option<String>,
    /// Snapshot of remote working directory at run creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<String>,
    /// Snapshot of active_platform_id at run creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_id: Option<String>,
    /// Snapshot of anthropic_base_url at run creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_base_url: Option<String>,
    /// Run source (native or cli_import).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RunSource>,
    /// CLI import watermark for incremental sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_import_watermark: Option<ImportWatermark>,
    /// Absolute path to CLI session JSONL file (read-only reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_path: Option<String>,
    /// True when CLI import couldn't reconstruct complete usage data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_usage_incomplete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub id: String,
    pub task_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub event_type: RunEventType,
    pub payload: serde_json::Value,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub task_id: String,
    pub files_changed: Vec<String>,
    pub diff_summary: String,
    pub commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_estimate: Option<f64>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub default_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    pub allowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    pub provider_mode: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_base_url: Option<String>,
    /// Which env var to inject: "ANTHROPIC_API_KEY" or "ANTHROPIC_AUTH_TOKEN".
    /// Set by the selected platform preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_env_var: Option<String>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub keybinding_overrides: Vec<KeyBindingOverride>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_hosts: Vec<RemoteHost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform_credentials: Vec<PlatformCredential>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_platform_id: Option<String>,
    #[serde(default)]
    pub onboarding_completed: bool,
    pub updated_at: String,
}

fn default_auth_mode() -> String {
    "cli".to_string()
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteHost {
    pub name: String,
    pub host: String,
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_claude_path: Option<String>,
    #[serde(default)]
    pub forward_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTestResult {
    pub ssh_ok: bool,
    pub cli_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_permission_mode() -> String {
    "auto_read".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformCredential {
    pub platform_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_env: Option<HashMap<String, String>>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            default_agent: "claude".to_string(),
            default_model: None,
            allowed_tools: vec![],
            working_directory: None,
            provider_mode: "local".to_string(),
            auth_mode: "cli".to_string(),
            anthropic_api_key: None,
            anthropic_base_url: None,
            auth_env_var: None,
            permission_mode: "auto_read".to_string(),
            max_budget_usd: None,
            fallback_model: None,
            keybinding_overrides: vec![],
            remote_hosts: vec![],
            platform_credentials: vec![],
            active_platform_id: None,
            onboarding_completed: false,
            updated_at: now_iso(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub allowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_partial_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_debug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_session_persistence: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub betas: Option<Vec<String>>,
    /// Custom agent definitions JSON string (passed to --agents flag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_json: Option<String>,
    pub updated_at: String,
}

impl AgentSettings {
    pub fn default_for(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            model: None,
            allowed_tools: vec![],
            working_directory: None,
            plan_mode: None,
            disallowed_tools: None,
            append_system_prompt: None,
            max_budget_usd: None,
            fallback_model: None,
            system_prompt: None,
            tool_set: None,
            add_dirs: None,
            json_schema: None,
            include_partial_messages: None,
            cli_debug: None,
            no_session_persistence: None,
            max_turns: None,
            effort: None,
            betas: None,
            agents_json: None,
            updated_at: now_iso(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllSettings {
    pub user: UserSettings,
    pub agents: std::collections::HashMap<String, AgentSettings>,
}

impl Default for AllSettings {
    fn default() -> Self {
        let mut agents = std::collections::HashMap::new();
        agents.insert("claude".to_string(), AgentSettings::default_for("claude"));
        agents.insert("codex".to_string(), AgentSettings::default_for("codex"));
        Self {
            user: UserSettings::default(),
            agents,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub id: String,
    pub prompt: String,
    pub cwd: String,
    pub agent: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    pub status: RunStatus,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_subtype: Option<String>,
    /// The model used in this run (updated on hot-switch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The run_id this session was forked from (None if not a fork).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// User-assigned display name (None = use prompt as label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Remote host name (references UserSettings.remote_hosts by name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_host_name: Option<String>,
    /// Snapshot of remote_cwd at run creation time (stable — not affected by later settings changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<String>,
    /// Full snapshot of RemoteHost config at run creation time.
    /// Used to restore remote sessions even if the host is renamed/deleted from settings.
    /// Falls back to name-based lookup for old runs that don't have this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_host_snapshot: Option<RemoteHost>,
    /// Snapshot of active_platform_id at run creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_id: Option<String>,
    /// Snapshot of anthropic_base_url at run creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_base_url: Option<String>,
    /// Run source (native or cli_import).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RunSource>,
    /// CLI import watermark for incremental sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_import_watermark: Option<ImportWatermark>,
    /// Absolute path to CLI session JSONL file (read-only reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_path: Option<String>,
    /// True when CLI import couldn't reconstruct complete usage data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_usage_incomplete: Option<bool>,
}

impl RunMeta {
    pub fn to_task_run(
        &self,
        last_activity_at: Option<String>,
        message_count: Option<u32>,
        last_message_preview: Option<String>,
    ) -> TaskRun {
        TaskRun {
            id: self.id.clone(),
            prompt: self.prompt.clone(),
            cwd: self.cwd.clone(),
            agent: self.agent.clone(),
            auth_mode: self.auth_mode.clone(),
            status: self.status.clone(),
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
            exit_code: self.exit_code,
            error_message: self.error_message.clone(),
            last_activity_at,
            message_count,
            last_message_preview,
            session_id: self.session_id.clone(),
            result_subtype: self.result_subtype.clone(),
            model: self.model.clone(),
            parent_run_id: self.parent_run_id.clone(),
            name: self.name.clone(),
            remote_host_name: self.remote_host_name.clone(),
            remote_cwd: self.remote_cwd.clone(),
            platform_id: self.platform_id.clone(),
            platform_base_url: self.platform_base_url.clone(),
            source: self.source.clone(),
            cli_import_watermark: self.cli_import_watermark.clone(),
            cli_session_path: self.cli_session_path.clone(),
            cli_usage_incomplete: self.cli_usage_incomplete,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDelta {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDone {
    pub ok: bool,
    pub code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    #[serde(rename = "type")]
    pub mime_type: String,
    pub size: u64,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCheckResult {
    pub agent: String,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliDistTags {
    pub latest: Option<String>,
    pub stable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInitStatus {
    pub cwd: String,
    pub has_claude_md: bool,
}

// ── Diagnostics report (run_diagnostics command) ──

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub cli: CliDiagnostics,
    pub auth: AuthDiagnostics,
    pub project: ProjectDiagnostics,
    pub configs: ConfigDiagnostics,
    pub services: ServicesDiagnostics,
    pub system: SystemDiagnostics,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliDiagnostics {
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub latest: Option<String>,
    pub stable: Option<String>,
    pub auto_update_channel: Option<String>,
    pub ripgrep_available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthDiagnostics {
    pub has_oauth: bool,
    pub oauth_account: Option<String>,
    pub has_api_key: bool,
    pub api_key_hint: Option<String>,
    pub api_key_source: Option<String>,
    pub app_has_credentials: bool,
    pub app_platform_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDiagnostics {
    pub cwd: String,
    pub has_claude_md: bool,
    pub claude_md_files: Vec<ClaudeMdInfo>,
    pub skipped_project_scope: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeMdInfo {
    pub path: String,
    pub size_chars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDiagnostics {
    pub settings_issues: Vec<ConfigIssue>,
    pub keybinding_issues: Vec<ConfigIssue>,
    pub mcp_issues: Vec<ConfigIssue>,
    pub env_var_issues: Vec<ConfigIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigIssue {
    pub scope: String,
    pub file: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServicesDiagnostics {
    pub community_registry: Option<bool>,
    pub mcp_registry: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemDiagnostics {
    pub sandbox_available: Option<bool>,
    pub lock_files: Vec<String>,
}

/// Raw usage data extracted from a run's events.jsonl (no RunMeta fields).
#[derive(Debug, Clone, Default)]
pub struct RawRunUsage {
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub duration_ms: u64,
    pub num_turns: u64,
    pub model_usage: HashMap<String, ModelUsageSummary>,
}

/// Per-run usage summary (RunMeta + usage data), returned by IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunUsageSummary {
    pub run_id: String,
    pub name: String,
    pub agent: String,
    pub model: Option<String>,
    pub status: RunStatus,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub duration_ms: u64,
    pub num_turns: u64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_usage: HashMap<String, ModelUsageSummary>,
}

/// Per-model token and cost summary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// Aggregated usage overview across all runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOverview {
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub total_runs: u32,
    pub avg_cost_per_run: f64,
    pub by_model: Vec<ModelAggregate>,
    pub daily: Vec<DailyAggregate>,
    pub runs: Vec<RunUsageSummary>,
    /// How the data was produced: "memory", "disk", "incremental", "full".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_mode: Option<String>,
    /// Number of days with activity.
    #[serde(default)]
    pub active_days: u32,
    /// Current consecutive active days (including today).
    #[serde(default)]
    pub current_streak: u32,
    /// Longest consecutive active days ever.
    #[serde(default)]
    pub longest_streak: u32,
}

/// Per-model aggregate stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAggregate {
    pub model: String,
    pub runs: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub pct: f64,
}

/// Daily aggregate stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyAggregate {
    pub date: String,
    pub cost_usd: f64,
    pub runs: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Message count (Global mode — from Claude Code stats-cache).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
    /// Session count (Global mode — from Claude Code stats-cache).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_count: Option<u32>,
    /// Tool call count (Global mode — from Claude Code stats-cache).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_count: Option<u32>,
    /// Per-model token breakdown (populated for last 30 daily entries only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_breakdown: Option<std::collections::HashMap<String, ModelTokens>>,
}

/// Per-model token counts for a single day (stacked chart).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyOutput {
    pub run_id: String,
    pub data: String, // base64 encoded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyExit {
    pub run_id: String,
    pub exit_code: i32,
}

// API mode types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub run_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub run_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub output: serde_json::Value,
    pub status: String, // "success" | "error" | "denied"
}

// ── CLI Control Protocol types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliModelInfo {
    pub value: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        default,
        rename = "supportsEffort",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_effort: Option<bool>,
    #[serde(
        default,
        rename = "supportedEffortLevels",
        skip_serializing_if = "Option::is_none"
    )]
    pub supported_effort_levels: Option<Vec<String>>,
    #[serde(
        default,
        rename = "supportsAdaptiveThinking",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_adaptive_thinking: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliAccount {
    #[serde(default, rename = "tokenSource")]
    pub token_source: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliInfo {
    pub models: Vec<CliModelInfo>,
    pub commands: Vec<CliCommand>,
    #[serde(default)]
    pub available_output_styles: Vec<String>,
    pub account: Option<CliAccount>,
    /// The model currently selected in Claude Code (from ~/.claude/settings.json)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_model: Option<String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliInfoError {
    pub code: String,
    pub message: String,
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ── Attachment limits ──
// Images: no app-side limit — CLI compresses via sharp (→ ≤3.75MB + ≤2000px).
pub const MAX_TEXT_SIZE: u64 = 10 * 1024 * 1024; // 10MB — text files
pub const MAX_PDF_BINARY_SIZE: u64 = 20 * 1024 * 1024; // 20MB — PDF binary inline (CLI dj6)
pub const ALLOWED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];
pub const ALLOWED_DOC_TYPES: &[&str] = &["application/pdf"];

/// Max size for attachment by MIME type. Images: no limit, PDF: 20MB, text: 10MB.
pub fn max_attachment_size(mime: &str) -> u64 {
    if ALLOWED_IMAGE_TYPES.iter().any(|t| mime.starts_with(t)) {
        u64::MAX // CLI handles compression
    } else if ALLOWED_DOC_TYPES.contains(&mime) {
        MAX_PDF_BINARY_SIZE // 20MB for PDF (CLI dj6)
    } else {
        MAX_TEXT_SIZE // 10MB for text
    }
}

// ── Per-model usage breakdown ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageEntry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub web_search_requests: u64,
    pub cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Maximum output tokens for this model (e.g. 32000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

// ── MCP server info ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Event Bus types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BusEvent {
    SessionInit {
        run_id: String,
        session_id: Option<String>,
        model: Option<String>,
        tools: Vec<String>,
        cwd: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        slash_commands: Vec<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mcp_servers: Vec<McpServerInfo>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "permissionMode"
        )]
        permission_mode: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "apiKeySource"
        )]
        api_key_source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_code_version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_style: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        agents: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        plugins: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fast_mode_state: Option<String>,
    },
    MessageDelta {
        run_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    MessageComplete {
        run_id: String,
        message_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        /// Actual model used for this message (e.g. "claude-opus-4-6").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Stop reason (v2.1.41: usually null; future: "end_turn", "tool_use").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        /// Per-message token usage (raw JSON — result event has aggregated totals).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_usage: Option<Value>,
    },
    ToolStart {
        run_id: String,
        tool_use_id: String,
        tool_name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    ToolEnd {
        run_id: String,
        tool_use_id: String,
        tool_name: String,
        output: Value,
        status: String,
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        /// Structured tool result metadata from CLI verbose mode (e.g. file info for Read)
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_result: Option<Value>,
    },
    UserMessage {
        run_id: String,
        text: String,
    },
    RunState {
        run_id: String,
        state: String,
        exit_code: Option<i32>,
        error: Option<String>,
    },
    UsageUpdate {
        run_id: String,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        total_cost_usd: f64,
        /// Backend-authoritative turn index (1-based). Injected by session_actor for user turns.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_usage: Option<HashMap<String, ModelUsageEntry>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_api_ms: Option<u64>,
        /// Total duration including hooks/overhead (from result event).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Number of turns in this session (from result event).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        num_turns: Option<u64>,
        /// Stop reason from result event (v2.1.41: usually null).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        /// Service tier (e.g. "standard").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service_tier: Option<String>,
        /// Speed tier (e.g. "standard").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed: Option<String>,
        /// Web fetch request count (from usage.server_tool_use).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        web_fetch_requests: Option<u64>,
        /// 5-minute ephemeral cache creation tokens.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_creation_5m: Option<u64>,
        /// 1-hour ephemeral cache creation tokens.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_creation_1h: Option<u64>,
    },
    Raw {
        run_id: String,
        source: String,
        data: Value,
    },
    PermissionDenied {
        run_id: String,
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
    },
    /// Thinking/reasoning text delta (from extended thinking).
    ThinkingDelta {
        run_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// Partial JSON input for a tool being invoked (real-time streaming).
    ToolInputDelta {
        run_id: String,
        tool_use_id: String,
        partial_json: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// Inline permission prompt from `--permission-prompt-tool stdio`.
    /// CLI is waiting for a control_response with allow/deny.
    PermissionPrompt {
        run_id: String,
        request_id: String,
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
        decision_reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggestions: Vec<Value>,
    },
    /// Context compaction boundary — CLI auto-compressed the conversation context.
    CompactBoundary {
        run_id: String,
        trigger: String,
        pre_tokens: Option<u64>,
    },
    /// System status change (e.g. "compacting").
    SystemStatus {
        run_id: String,
        /// CLI status string, e.g. "compacting", null for cleared
        status: Option<String>,
        data: Value,
    },
    /// Hook execution started.
    HookStarted {
        run_id: String,
        hook_event: String,
        hook_id: String,
        data: Value,
        /// Hook name (e.g. "SessionStart:startup").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_name: Option<String>,
    },
    /// Hook execution progress.
    HookProgress {
        run_id: String,
        hook_id: String,
        data: Value,
    },
    /// Hook execution completed with result.
    HookResponse {
        run_id: String,
        hook_id: String,
        hook_event: String,
        outcome: String,
        data: Value,
        /// Hook name (e.g. "SessionStart:startup").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_name: Option<String>,
        /// Hook stdout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout: Option<String>,
        /// Hook stderr.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
        /// Hook process exit code.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
    /// Background task notification (file indexing, MCP init, etc.).
    TaskNotification {
        run_id: String,
        task_id: String,
        status: String,
        data: Value,
    },
    /// Files persisted notification.
    FilesPersisted {
        run_id: String,
        files: Value,
        data: Value,
    },
    /// Tool progress update (real-time elapsed time).
    /// Top-level event type "tool_progress" (not a content_block_delta subtype).
    ToolProgress {
        run_id: String,
        tool_use_id: String,
        elapsed_time_seconds: Option<f64>,
        data: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// Tool use summary — top-level event type "tool_use_summary".
    ToolUseSummary {
        run_id: String,
        tool_use_id: String,
        summary: String,
        preceding_tool_use_ids: Vec<String>,
        data: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// Authentication status update.
    AuthStatus {
        run_id: String,
        is_authenticating: bool,
        output: Vec<String>,
        data: Value,
    },
    /// Hook callback control_request — CLI requests hook execution/approval.
    /// Analogous to PermissionPrompt (needs a control_response).
    HookCallback {
        run_id: String,
        request_id: String,
        hook_event: String,
        hook_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hook_name: Option<String>,
        data: Value,
    },
    /// CLI cancelled a pending control_request (e.g. cancelled permission prompt).
    ControlCancelled {
        run_id: String,
        request_id: String,
    },
    /// Output from a CLI slash command (e.g. /context, /cost).
    /// Extracted from `<local-command-stdout>` tags in user messages.
    CommandOutput {
        run_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    New,
    Resume,
    Continue,
    Fork,
}

// ── Agent Team Mode types ──
// Read from ~/.claude/teams/ and ~/.claude/tasks/ (Claude Code team collaboration)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: u64,
    #[serde(rename = "leadAgentId", default)]
    pub lead_agent_id: String,
    #[serde(rename = "leadSessionId", default)]
    pub lead_session_id: String,
    #[serde(default)]
    pub members: Vec<TeamMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub name: String,
    #[serde(rename = "agentType", default)]
    pub agent_type: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub color: String,
    #[serde(rename = "planModeRequired", default)]
    pub plan_mode_required: bool,
    #[serde(rename = "joinedAt", default)]
    pub joined_at: u64,
    #[serde(rename = "tmuxPaneId", default)]
    pub tmux_pane_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(rename = "backendType", default)]
    pub backend_type: String,
    /// The prompt given to spawned teammates (not present on leader)
    #[serde(default)]
    pub prompt: String,
    /// Runtime active status (set by setMemberActive in Claude Code SDK)
    #[serde(rename = "isActive", default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamInboxMessage {
    #[serde(default)]
    pub from: String,
    pub text: String,
    #[serde(default)]
    pub summary: String,
    pub timestamp: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTask {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "activeForm", default)]
    pub active_form: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(rename = "blockedBy", default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSummary {
    pub name: String,
    pub description: String,
    pub member_count: usize,
    pub task_count: usize,
    pub created_at: u64,
}

// ── Plugin types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePlugin {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<PluginAuthor>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Raw source — string for local ("./plugins/name"), object for external
    #[serde(default)]
    pub source: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(default, rename = "lspServers")]
    pub lsp_servers: Option<serde_json::Value>,
    // ── Fields enriched by our code (not from marketplace.json) ──
    #[serde(default)]
    pub marketplace_name: Option<String>,
    #[serde(default)]
    pub install_count: Option<u64>,
    /// Components discovered by scanning plugin subdirectories
    #[serde(default)]
    pub components: PluginComponents,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginComponents {
    pub skills: Vec<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    pub hooks: bool,
    pub mcp_servers: Vec<String>,
    pub lsp_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceInfo {
    pub name: String,
    pub source: serde_json::Value,
    pub install_location: String,
    pub last_updated: Option<String>,
    pub plugin_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandaloneSkill {
    pub name: String,
    pub description: String,
    pub path: String,
    /// "user" or "project"
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    #[serde(default, alias = "id")]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub marketplace: Option<String>,
    #[serde(default, rename = "pluginId")]
    pub plugin_id: Option<String>,
    /// Catch-all for unknown fields
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOperationResult {
    pub success: bool,
    pub message: String,
}

// ── Community skill types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommunitySkillResult {
    pub id: String,
    pub name: String,
    pub installs: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommunitySkillDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installs: u64,
    pub source: String,
    pub content: Option<String>,
    pub raw_url: Option<String>,
    pub skills_sh_url: Option<String>,
    pub github_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub available: bool,
    pub reason: Option<String>,
}

// ── MCP Registry API response types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRegistrySearchResult {
    pub servers: Vec<McpRegistryServer>,
    pub next_cursor: Option<String>,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryServer {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub packages: Vec<McpRegistryPackage>,
    #[serde(default)]
    pub remotes: Vec<McpRegistryRemote>,
    #[serde(default)]
    pub repository: Option<McpRegistryRepository>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryPackage {
    #[serde(default)]
    pub registry_type: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub environment_variables: Vec<McpRegistryEnvVar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryRemote {
    #[serde(rename = "type", default)]
    pub remote_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<McpRegistryHeader>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryEnvVar {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_required: Option<bool>,
    #[serde(default)]
    pub is_secret: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryHeader {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub is_required: Option<bool>,
    #[serde(default)]
    pub is_secret: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryRepository {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

// ── Configured MCP server ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredMcpServer {
    pub name: String,
    pub server_type: String,
    pub scope: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub env_keys: Vec<String>,
    #[serde(default)]
    pub header_keys: Vec<String>,
}

// ── Keybinding types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingOverride {
    pub command: String,
    pub key: String,
}

// ── Onboarding types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKeyInfo {
    pub key_path: String,
    pub key_path_expanded: String,
    pub pub_key_path: String,
    pub key_type: String,
    pub exists: bool,
    pub pub_exists: bool,
    pub ssh_copy_id_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCheckResult {
    pub has_oauth: bool,
    pub has_api_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_account: Option<String>,
}

/// Overview of all three authentication sources (configuration state only — no effective_source inference).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthOverview {
    /// User-configured mode: "cli" or "api"
    pub auth_mode: String,
    /// CLI Login (OAuth) available via `claude auth status`
    pub cli_login_available: bool,
    /// CLI Login account email (if logged in)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_login_account: Option<String>,
    /// CLI API Key detected from settings/env/shell config
    pub cli_has_api_key: bool,
    /// Hint of the CLI API key (last 4 chars)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_api_key_hint: Option<String>,
    /// Source of CLI API key: "settings", "env", "shell_config", or None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_api_key_source: Option<String>,
    /// App has platform credentials configured
    pub app_has_credentials: bool,
    /// Active platform ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_platform_id: Option<String>,
    /// Active platform display name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_platform_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallMethod {
    pub id: String,
    pub name: String,
    pub command: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

// ── Prompt search & favorites ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSearchResult {
    pub run_id: String,
    pub run_name: Option<String>,
    pub run_prompt: String,
    pub agent: String,
    pub model: Option<String>,
    pub status: RunStatus,
    pub started_at: String,
    pub matched_text: String,
    pub matched_seq: u64,
    pub matched_ts: String,
    pub is_favorite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFavorite {
    pub run_id: String,
    pub seq: u64,
    pub text: String,
    pub tags: Vec<String>,
    pub note: String,
    pub created_at: String,
}

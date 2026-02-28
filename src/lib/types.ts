export type RunStatus = "pending" | "running" | "completed" | "failed" | "stopped";

export type RunEventType = "system" | "stdout" | "stderr" | "command" | "user" | "assistant";

export interface TaskRun {
  id: string;
  prompt: string;
  cwd: string;
  agent: string;
  auth_mode: string;
  status: RunStatus;
  started_at: string;
  ended_at?: string;
  exit_code?: number;
  error_message?: string;
  last_activity_at?: string;
  message_count?: number;
  last_message_preview?: string;
  session_id?: string;
  result_subtype?: string;
  /** Model used in this run (persisted on hot-switch). */
  model?: string;
  /** The run_id this session was forked from. */
  parent_run_id?: string;
  /** User-assigned display name. */
  name?: string;
  /** Remote host name (if running on a remote machine). */
  remote_host_name?: string;
  /** Snapshot of remote working directory at run creation. */
  remote_cwd?: string;
  /** Snapshot of active_platform_id at run creation time. */
  platform_id?: string;
  /** Snapshot of anthropic_base_url at run creation time. */
  platform_base_url?: string;
  /** Run source (native or cli_import). */
  source?: "native" | "cli_import";
  /** CLI import watermark for incremental sync. */
  cli_import_watermark?: ImportWatermark;
  /** Absolute path to CLI session JSONL file. */
  cli_session_path?: string;
  /** True when CLI import couldn't reconstruct complete usage data. */
  cli_usage_incomplete?: boolean;
}

export interface ImportWatermark {
  offset: number;
  mtimeNs: number;
  fileSize: number;
  lastUuid?: string;
}

export interface CliSessionSummary {
  sessionId: string;
  cwd: string;
  firstPrompt: string;
  startedAt: string;
  lastActivityAt: string;
  messageCount: number;
  model?: string;
  cliVersion?: string;
  fileSize: number;
  filePath: string;
  hasSubagents: boolean;
  alreadyImported: boolean;
  existingRunId?: string;
}

export interface ImportResult {
  runId: string;
  sessionId: string;
  eventsImported: number;
  eventsSkipped: number;
  usageIncomplete: boolean;
  skippedSubtypes: Record<string, number>;
}

export interface SyncResult {
  newEvents: number;
  newWatermark: ImportWatermark;
  usageIncomplete: boolean;
}

export interface RunEvent {
  id: string;
  task_id: string;
  seq: number;
  type: RunEventType;
  payload: Record<string, unknown>;
  timestamp: string;
}

export interface RunArtifact {
  task_id: string;
  files_changed: string[];
  diff_summary: string;
  commands: string[];
  cost_estimate?: number;
  updated_at: string;
}

export interface UserSettings {
  default_agent: string;
  default_model?: string;
  allowed_tools: string[];
  working_directory?: string;
  provider_mode: string;
  auth_mode: string;
  anthropic_api_key?: string;
  anthropic_base_url?: string;
  /** "ANTHROPIC_API_KEY" or "ANTHROPIC_AUTH_TOKEN" — set by platform preset */
  auth_env_var?: string;
  permission_mode: string;
  max_budget_usd?: number;
  fallback_model?: string;
  keybinding_overrides: KeyBindingOverride[];
  remote_hosts?: RemoteHost[];
  platform_credentials?: PlatformCredential[];
  active_platform_id?: string;
  onboarding_completed: boolean;
  updated_at: string;
}

// ── Remote SSH types ──

export interface RemoteHost {
  name: string;
  host: string;
  user: string;
  port: number;
  key_path?: string;
  remote_cwd?: string;
  remote_claude_path?: string;
  forward_api_key: boolean;
}

export interface RemoteTestResult {
  ssh_ok: boolean;
  cli_found: boolean;
  cli_version?: string;
  cli_path?: string;
  error?: string;
}

// ── Keybinding types ──

export interface KeyBinding {
  command: string;
  label: string;
  key: string;
  context: "global" | "chat" | "prompt" | "cli";
  editable: boolean;
  source: "app" | "cli";
  /** If true, this binding is also registered as an OS-level global shortcut. */
  osGlobal?: boolean;
}

export interface ScreenshotPayload {
  contentBase64: string;
  mediaType: string;
  filename: string;
}

export interface KeyBindingOverride {
  command: string;
  key: string;
}

export interface HookEvent {
  run_id: string;
  hook_type: string;
  tool_name?: string;
  tool_input?: Record<string, unknown>;
  tool_output?: Record<string, unknown>;
  status?: string;
  usage?: TokenUsage;
  timestamp: string;
  source?: string;
  reason?: string;
  error?: string;
  message?: string;
  title?: string;
  notification_type?: string;
  agent_id?: string;
  agent_type?: string;
  trigger?: string;
  task_id?: string;
  task_subject?: string;
  model?: string;
  session_id?: string;
}

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  cost: number;
}

export interface AgentSettings {
  agent: string;
  model?: string;
  allowed_tools: string[];
  working_directory?: string;
  plan_mode?: boolean;
  disallowed_tools?: string[];
  append_system_prompt?: string;
  max_budget_usd?: number;
  fallback_model?: string;
  system_prompt?: string;
  tool_set?: string;
  add_dirs?: string[];
  json_schema?: unknown;
  include_partial_messages?: boolean;
  cli_debug?: string;
  no_session_persistence?: boolean;
  max_turns?: number;
  effort?: string;
  betas?: string[];
  agents_json?: string;
  updated_at: string;
}

export type SessionMode = "new" | "resume" | "continue" | "fork";

export interface DirEntry {
  name: string;
  is_dir: boolean;
  size: number;
}

export interface DirListing {
  path: string;
  entries: DirEntry[];
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: string;
}

export interface Attachment {
  name: string;
  type: string;
  size: number;
  contentBase64: string;
}

export interface CliCheckResult {
  agent: string;
  found: boolean;
  path?: string;
  version?: string;
}

export interface ProjectInitStatus {
  cwd: string;
  has_claude_md: boolean;
}

export interface CliDistTags {
  latest?: string;
  stable?: string;
}

export interface ModelUsageSummary {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  costUsd: number;
}

export interface RunUsageSummary {
  runId: string;
  name: string;
  agent: string;
  model?: string;
  status: RunStatus;
  startedAt: string;
  endedAt?: string;
  totalCostUsd: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  durationMs: number;
  numTurns: number;
  modelUsage?: Record<string, ModelUsageSummary>;
}

export interface ModelAggregate {
  model: string;
  runs: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  costUsd: number;
  pct: number;
}

export interface DailyAggregate {
  date: string;
  costUsd: number;
  runs: number;
  inputTokens: number;
  outputTokens: number;
  /** Message count (Global mode — from Claude Code stats-cache). */
  messageCount?: number;
  /** Session count (Global mode — from Claude Code stats-cache). */
  sessionCount?: number;
  /** Tool call count (Global mode — from Claude Code stats-cache). */
  toolCallCount?: number;
  /** Per-model token breakdown (populated for last 30 daily entries only). */
  modelBreakdown?: Record<string, ModelTokens>;
}

export interface ModelTokens {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
}

export interface UsageOverview {
  totalCostUsd: number;
  totalTokens: number;
  totalRuns: number;
  avgCostPerRun: number;
  byModel: ModelAggregate[];
  daily: DailyAggregate[];
  runs: RunUsageSummary[];
  /** How the data was produced: "memory", "disk", "incremental", "full". */
  scanMode?: string;
  /** Number of days with activity. */
  activeDays: number;
  /** Current consecutive active days. */
  currentStreak: number;
  /** Longest consecutive active days ever. */
  longestStreak: number;
}

// ── Git types ──

export interface GitFileStat {
  path: string;
  status: string; // "M", "A", "D", "R", "?"
  insertions: number;
  deletions: number;
}

export interface GitSummary {
  branch: string;
  files: GitFileStat[];
  total_files: number;
  total_insertions: number;
  total_deletions: number;
}

// ── CLI Control Protocol types ──

export interface CliModelInfo {
  value: string;
  displayName: string;
  description: string;
  supportsEffort?: boolean;
  supportedEffortLevels?: string[];
  supportsAdaptiveThinking?: boolean;
}

export interface CliCommand {
  name: string;
  description: string;
  aliases?: string[];
  [key: string]: unknown;
}

export interface CliAccount {
  tokenSource: string;
  [key: string]: unknown;
}

export interface CliInfo {
  models: CliModelInfo[];
  commands: CliCommand[];
  available_output_styles: string[];
  account?: CliAccount;
  /** The model currently selected in Claude Code (from ~/.claude/settings.json) */
  current_model?: string;
  fetched_at: string;
}

// ── Per-model usage breakdown ──

export interface ModelUsageEntry {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  web_search_requests: number;
  cost_usd: number;
  context_window?: number;
  maxOutputTokens?: number;
}

// ── MCP server info ──

export interface McpServerInfo {
  name: string;
  status: string;
  server_type?: string;
  error?: string;
}

// ── Diagnostics report (run_diagnostics command) ──

export interface DiagnosticsReport {
  cli: CliDiagnostics;
  auth: AuthDiagnostics;
  project: ProjectDiagnostics;
  configs: ConfigDiagnostics;
  services: ServicesDiagnostics;
  system: SystemDiagnostics;
}

export interface CliDiagnostics {
  found: boolean;
  version: string | null;
  path: string | null;
  latest: string | null;
  stable: string | null;
  auto_update_channel: string | null;
  ripgrep_available: boolean;
}

export interface AuthDiagnostics {
  has_oauth: boolean;
  oauth_account: string | null;
  has_api_key: boolean;
  api_key_hint: string | null;
  api_key_source: string | null;
  app_has_credentials: boolean;
  app_platform_name: string | null;
}

export interface ProjectDiagnostics {
  cwd: string;
  has_claude_md: boolean;
  claude_md_files: ClaudeMdInfo[];
  skipped_project_scope: boolean;
}

export interface ClaudeMdInfo {
  path: string;
  size_chars: number;
}

export interface ConfigDiagnostics {
  settings_issues: ConfigIssue[];
  keybinding_issues: ConfigIssue[];
  mcp_issues: ConfigIssue[];
  env_var_issues: ConfigIssue[];
}

export interface ConfigIssue {
  scope: string;
  file: string;
  severity: string;
  message: string;
}

export interface ServicesDiagnostics {
  community_registry: boolean | null;
  mcp_registry: boolean | null;
}

export interface SystemDiagnostics {
  sandbox_available: boolean | null;
  lock_files: string[];
}

// ── Permission suggestion ──

export interface PermissionSuggestion {
  type: string;
  rules?: string[];
  behavior?: string;
  mode?: string;
  directories?: string[];
  destination?: string;
  /** additionalContext hook data */
  message?: unknown;
}

// ── Event Bus types ──

// ── Team types (mirror Rust models.rs) ──

export interface TeamConfig {
  name: string;
  description: string;
  /** camelCase from serde rename */
  createdAt: number;
  leadAgentId: string;
  leadSessionId: string;
  members: TeamMember[];
}

export interface TeamMember {
  agentId: string;
  name: string;
  agentType: string;
  model: string;
  color: string;
  planModeRequired: boolean;
  joinedAt: number;
  tmuxPaneId: string;
  cwd: string;
  subscriptions: string[];
  backendType: string;
  /** The prompt given to spawned teammates (empty for leader) */
  prompt: string;
  /** Runtime active status from Claude Code SDK */
  isActive: boolean;
}

export interface TeamInboxMessage {
  from: string;
  text: string;
  summary: string;
  timestamp: string;
  color: string;
  read: boolean;
}

export interface TeamTask {
  id: string;
  subject: string;
  description: string;
  activeForm: string;
  owner: string;
  status: string;
  blocks: string[];
  blockedBy: string[];
  metadata?: unknown;
}

export interface TeamSummary {
  name: string;
  description: string;
  /** snake_case — internal type, not deserialized from Claude Code files */
  member_count: number;
  task_count: number;
  created_at: number;
}

// ── Plugin types ──

export interface PluginAuthor {
  name: string;
  email?: string;
}

export interface PluginComponents {
  skills: string[];
  commands: string[];
  agents: string[];
  hooks: boolean;
  mcp_servers: string[];
  lsp_servers: string[];
}

export interface MarketplacePlugin {
  name: string;
  description: string;
  version?: string;
  author?: PluginAuthor;
  category?: string;
  homepage?: string;
  source?: unknown;
  tags: string[];
  strict?: boolean;
  lsp_servers?: unknown;
  marketplace_name?: string;
  install_count?: number;
  components: PluginComponents;
}

export interface MarketplaceInfo {
  name: string;
  source: unknown;
  install_location: string;
  last_updated?: string;
  plugin_count: number;
}

export interface StandaloneSkill {
  name: string;
  description: string;
  path: string;
  scope?: string;
}

export interface InstalledPlugin {
  name: string;
  description: string;
  version?: string;
  scope?: string;
  enabled?: boolean;
  marketplace?: string;
  pluginId?: string;
  [key: string]: unknown;
}

export interface PluginOperationResult {
  success: boolean;
  message: string;
}

export interface CommunitySkillResult {
  id: string;
  name: string;
  installs: number;
  source: string;
}

export interface CommunitySkillDetail {
  id: string;
  name: string;
  description: string;
  installs: number;
  source: string;
  content: string | null;
  raw_url: string | null;
  skills_sh_url: string | null;
  github_url: string | null;
}

export interface ProviderHealth {
  available: boolean;
  reason: string | null;
}

// ── Auto-context tracking ──

export interface ContextSnapshot {
  runId: string;
  turnIndex: number;
  ts: string;
  data: import("$lib/utils/context-parser").ContextData;
}

// ── MCP Registry types ──

export interface McpRegistrySearchResult {
  servers: McpRegistryServer[];
  nextCursor: string | null;
  count: number;
}

export interface McpRegistryServer {
  name: string;
  description: string;
  title?: string;
  version: string;
  packages: McpRegistryPackage[];
  remotes: McpRegistryRemote[];
  repository?: McpRegistryRepository;
}

export interface McpRegistryPackage {
  registryType: string;
  identifier: string;
  version?: string;
  environmentVariables: McpRegistryEnvVar[];
}

export interface McpRegistryRemote {
  type: string;
  url: string;
  headers: McpRegistryHeader[];
}

export interface McpRegistryEnvVar {
  name: string;
  description?: string;
  isRequired?: boolean;
  isSecret?: boolean;
}

export interface McpRegistryHeader {
  name: string;
  description?: string;
  value?: string;
  isRequired?: boolean;
  isSecret?: boolean;
}

export interface McpRegistryRepository {
  url?: string;
  source?: string;
}

export interface ConfiguredMcpServer {
  name: string;
  server_type: string;
  scope: string;
  command?: string;
  args: string[];
  url?: string;
  env_keys: string[];
  header_keys: string[];
}

// ── Sidebar panel types ──

export interface FileEntry {
  path: string;
  action: "read" | "write" | "edit" | "persisted";
  toolUseId?: string; // only top-level tools can be scrolled to
  status?: string;
}

export interface SessionInfoData {
  sessionId?: string;
  runId?: string;
  runName?: string;
  cwd: string;
  numTurns: number;
  status: RunStatus;
  startedAt: string | null;
  endedAt: string | null;
  lastTurnDurationMs: number;
  tokensEstimated: boolean;
  model: string;
  agent: string;
  cliVersion: string;
  permissionMode: string;
  fastModeState: string;
  cost: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  contextWindow: number;
  contextUtilization: number;
  compactCount: number;
  microcompactCount: number;
  mcpServers: McpServerInfo[];
  remoteHostName?: string | null;
  platformId?: string | null;
  cliUsageIncomplete?: boolean;
  runSource?: string;
  authSourceLabel?: string;
  platformName?: string;
  cliUpdateAvailable?: string;
}

export type BusEvent =
  | {
      type: "session_init";
      run_id: string;
      session_id?: string;
      model?: string;
      tools: string[];
      cwd: string;
      slash_commands?: CliCommand[];
      mcp_servers?: McpServerInfo[];
      permissionMode?: string;
      apiKeySource?: string;
      claude_code_version?: string;
      output_style?: string;
      agents?: string[];
      skills?: string[];
      plugins?: unknown[];
      fast_mode_state?: string;
    }
  | { type: "message_delta"; run_id: string; text: string; parent_tool_use_id?: string }
  | {
      type: "message_complete";
      run_id: string;
      message_id: string;
      text: string;
      parent_tool_use_id?: string;
      model?: string;
      stop_reason?: string | null;
      message_usage?: Record<string, unknown>;
    }
  | {
      type: "tool_start";
      run_id: string;
      tool_use_id: string;
      tool_name: string;
      input: Record<string, unknown>;
      parent_tool_use_id?: string;
    }
  | {
      type: "tool_end";
      run_id: string;
      tool_use_id: string;
      tool_name: string;
      output: Record<string, unknown>;
      status: string;
      duration_ms?: number;
      parent_tool_use_id?: string;
      /** Structured tool result metadata from CLI verbose mode */
      tool_use_result?: Record<string, unknown>;
    }
  | { type: "user_message"; run_id: string; text: string }
  | { type: "run_state"; run_id: string; state: string; exit_code?: number; error?: string }
  | {
      type: "usage_update";
      run_id: string;
      input_tokens: number;
      output_tokens: number;
      cache_read_tokens?: number;
      cache_write_tokens?: number;
      total_cost_usd: number;
      /** Backend-authoritative turn index (1-based). Present for user turns. */
      turn_index?: number;
      model_usage?: Record<string, ModelUsageEntry>;
      duration_api_ms?: number;
      duration_ms?: number;
      num_turns?: number;
      stop_reason?: string | null;
      service_tier?: string;
      speed?: string;
      web_fetch_requests?: number;
      cache_creation_5m?: number;
      cache_creation_1h?: number;
    }
  | { type: "raw"; run_id: string; source: string; data: Record<string, unknown> }
  | { type: "thinking_delta"; run_id: string; text: string; parent_tool_use_id?: string }
  | {
      type: "tool_input_delta";
      run_id: string;
      tool_use_id: string;
      partial_json: string;
      parent_tool_use_id?: string;
    }
  | {
      type: "permission_denied";
      run_id: string;
      tool_name: string;
      tool_use_id: string;
      tool_input: Record<string, unknown>;
    }
  | {
      type: "permission_prompt";
      run_id: string;
      request_id: string;
      tool_name: string;
      tool_use_id: string;
      tool_input: Record<string, unknown>;
      decision_reason: string;
      parent_tool_use_id?: string;
      suggestions?: PermissionSuggestion[];
    }
  | { type: "compact_boundary"; run_id: string; trigger: string; pre_tokens?: number }
  | { type: "system_status"; run_id: string; status?: string; data: Record<string, unknown> }
  | {
      type: "hook_started";
      run_id: string;
      hook_event: string;
      hook_id: string;
      data: Record<string, unknown>;
      hook_name?: string;
    }
  | { type: "hook_progress"; run_id: string; hook_id: string; data: Record<string, unknown> }
  | {
      type: "hook_response";
      run_id: string;
      hook_id: string;
      hook_event: string;
      outcome: string;
      data: Record<string, unknown>;
      hook_name?: string;
      stdout?: string;
      stderr?: string;
      exit_code?: number;
    }
  | {
      type: "task_notification";
      run_id: string;
      task_id: string;
      status: string;
      data: Record<string, unknown>;
    }
  | { type: "files_persisted"; run_id: string; files: unknown; data: Record<string, unknown> }
  | {
      type: "tool_progress";
      run_id: string;
      tool_use_id: string;
      elapsed_time_seconds?: number;
      data: Record<string, unknown>;
      parent_tool_use_id?: string;
    }
  | {
      type: "tool_use_summary";
      run_id: string;
      tool_use_id: string;
      summary: string;
      preceding_tool_use_ids: string[];
      data: Record<string, unknown>;
      parent_tool_use_id?: string;
    }
  | {
      type: "auth_status";
      run_id: string;
      is_authenticating: boolean;
      output: string[];
      data: Record<string, unknown>;
    }
  | {
      type: "hook_callback";
      run_id: string;
      request_id: string;
      hook_event: string;
      hook_id: string;
      hook_name?: string;
      data: Record<string, unknown>;
    }
  | { type: "control_cancelled"; run_id: string; request_id: string }
  | { type: "command_output"; run_id: string; content: string };

export interface BusToolItem {
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  output?: Record<string, unknown>;
  status:
    | "running"
    | "success"
    | "error"
    | "denied"
    | "ask_pending"
    | "permission_denied"
    | "permission_prompt";
  /** For permission_prompt status: the control_request ID needed to respond. */
  permission_request_id?: string;
  duration_ms?: number;
  /** Real-time elapsed time from tool_progress (seconds, float). */
  elapsed_time_seconds?: number;
  /** Summary text from tool_use_summary. */
  summary?: string;
  /** Permission update suggestions from CLI. */
  suggestions?: PermissionSuggestion[];
  /** Structured tool result metadata from CLI verbose mode (e.g. file info for Read). */
  tool_use_result?: Record<string, unknown>;
}

export type TimelineEntry =
  | { kind: "user"; id: string; content: string; ts: string; attachments?: Attachment[] }
  | {
      kind: "assistant";
      id: string;
      content: string;
      ts: string;
      thinkingText?: string;
      model?: string;
    }
  | { kind: "tool"; id: string; tool: BusToolItem; ts: string; subTimeline?: TimelineEntry[] }
  | { kind: "separator"; id: string; content: string; ts: string }
  | { kind: "command_output"; id: string; content: string; ts: string };

// ── App Updates ──

export interface UpdateInfo {
  hasUpdate: boolean;
  latestVersion: string;
  currentVersion: string;
  downloadUrl: string;
}

// ── Changelog ──

export interface ChangelogEntry {
  version: string;
  date: string;
  changes: string[];
}

// ── Hook config types (mirrors ~/.claude/settings.json hooks) ──

export type HookEventType =
  | "PreToolUse"
  | "PostToolUse"
  | "Notification"
  | "Stop"
  | "SubagentStop"
  | "SubagentTool";

export interface HookHandler {
  type: "command" | "prompt";
  command?: string;
  prompt?: string;
  timeout?: number;
  async?: boolean;
  statusMessage?: string;
  model?: string;
  once?: boolean;
}

export interface HookMatcherGroup {
  matcher?: string;
  hooks: HookHandler[];
  [key: string]: unknown;
}

export type HooksConfig = Record<string, HookMatcherGroup[]>;

// ── CLI Config types ──

export interface CliConfigSettingDef {
  key: string;
  label: string;
  description: string;
  group: "behavior" | "appearance" | "advanced";
  type: "boolean" | "enum" | "string";
  default: unknown;
  options?: { value: string; label: string }[];
}

// ── Onboarding types ──

export interface SshKeyInfo {
  key_path: string;
  key_path_expanded: string;
  pub_key_path: string;
  key_type: string;
  exists: boolean;
  pub_exists: boolean;
  ssh_copy_id_available: boolean;
}

export interface AuthCheckResult {
  has_oauth: boolean;
  has_api_key: boolean;
  oauth_account?: string;
}

/** Overview of all authentication sources (configuration state, no runtime inference). */
export interface AuthOverview {
  auth_mode: string;
  cli_login_available: boolean;
  cli_login_account?: string;
  cli_has_api_key: boolean;
  cli_api_key_hint?: string;
  /** Source of CLI API key: "settings" | "env" | "shell_config" */
  cli_api_key_source?: string;
  app_has_credentials: boolean;
  app_platform_id?: string;
  app_platform_name?: string;
}

export interface InstallMethod {
  id: string;
  name: string;
  command: string;
  available: boolean;
  unavailable_reason?: string;
}

// ── Prompt search & favorites ──

export interface PromptSearchResult {
  runId: string;
  runName?: string;
  runPrompt: string;
  agent: string;
  model?: string;
  status: RunStatus;
  startedAt: string;
  matchedText: string;
  matchedSeq: number;
  matchedTs: string;
  isFavorite: boolean;
}

export interface PromptFavorite {
  runId: string;
  seq: number;
  text: string;
  tags: string[];
  note: string;
  createdAt: string;
}

export interface PlatformPreset {
  id: string;
  name: string;
  base_url: string;
  auth_env_var: "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN";
  description: string;
  key_placeholder: string;
  category: "provider" | "proxy" | "local" | "custom";
  models?: string[];
  extra_env?: Record<string, string>;
  docs_url?: string;
}

/** Snapshot of PromptInput state for stash/restore. */
export interface PromptInputSnapshot {
  text: string;
  attachments: Array<{
    id: string;
    name: string;
    type: string;
    size: number;
    contentBase64?: string;
    filePath?: string;
  }>;
  pastedBlocks: Array<{
    id: string;
    text: string;
    lineCount: number;
    charCount: number;
    preview: string;
  }>;
}

export interface PlatformCredential {
  platform_id: string;
  api_key?: string;
  base_url?: string;
  auth_env_var?: string;
  name?: string;
  models?: string[];
  extra_env?: Record<string, string>;
}

export interface AgentDefinitionSummary {
  file_name: string;
  name: string;
  description: string;
  model?: string;
  source: string;
  scope: "user" | "project" | "plugin";
  tools?: string[];
  disallowed_tools?: string[];
  permission_mode?: string;
  max_turns?: number;
  background?: boolean;
  isolation?: string;
  readonly: boolean;
  raw_content?: string;
}

import { getTransport } from "./transport";
import { dbg, dbgWarn, redactSensitive } from "./utils/debug";

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return getTransport().invoke<T>(cmd, args);
}
import type {
  TaskRun,
  RunEvent,
  RunArtifact,
  UserSettings,
  AgentSettings,
  DirListing,
  Attachment,
  CliCheckResult,
  ProjectInitStatus,
  CliDistTags,
  UsageOverview,
  BusEvent,
  CliInfo,
  SessionMode,
  TeamSummary,
  TeamConfig,
  TeamTask,
  TeamInboxMessage,
  MarketplacePlugin,
  MarketplaceInfo,
  StandaloneSkill,
  InstalledPlugin,
  PluginOperationResult,
  GitSummary,
  ConfiguredMcpServer,
  McpRegistrySearchResult,
  ProviderHealth,
  ChangelogEntry,
  RemoteTestResult,
  SshKeyInfo,
  PromptSearchResult,
  PromptFavorite,
  SyncResult,
  DiagnosticsReport,
  AgentDefinitionSummary,
  RunSearchFilters,
  RunSearchResponse,
} from "./types";

// Runs
export async function listRuns(): Promise<TaskRun[]> {
  dbg("api", "listRuns");
  try {
    const runs = await invoke<TaskRun[]>("list_runs");
    dbg("api", "listRuns →", runs.length);
    return runs;
  } catch (e) {
    dbgWarn("api", "listRuns error", e);
    throw e;
  }
}

export async function getRun(id: string): Promise<TaskRun> {
  dbg("api", "getRun", id);
  return invoke<TaskRun>("get_run", { id });
}

export async function startRun(
  prompt: string,
  cwd: string,
  agent: string,
  model?: string,
  remoteHostName?: string,
  platformId?: string,
): Promise<TaskRun> {
  dbg("api", "startRun", { prompt: prompt.slice(0, 80), agent, cwd, remoteHostName, platformId });
  const result = await invoke<TaskRun>("start_run", {
    prompt,
    cwd,
    agent,
    model,
    remoteHostName: remoteHostName ?? null,
    platformId: platformId ?? null,
  });
  dbg("api", "startRun →", result.id);
  return result;
}

export async function stopRun(id: string): Promise<boolean> {
  dbg("api", "stopRun", id);
  return invoke<boolean>("stop_run", { id });
}

export async function renameRun(id: string, name: string): Promise<void> {
  dbg("api", "renameRun", { id, name });
  return invoke<void>("rename_run", { id, name });
}

export async function updateRunModel(id: string, model: string): Promise<void> {
  dbg("api", "updateRunModel", { id, model });
  return invoke<void>("update_run_model", { id, model });
}

export async function softDeleteRuns(ids: string[]): Promise<number> {
  dbg("api", "softDeleteRuns", { ids });
  return invoke<number>("soft_delete_runs", { ids });
}

// Prompt search & favorites

export async function searchPrompts(query: string, limit?: number): Promise<PromptSearchResult[]> {
  dbg("api", "searchPrompts", { query, limit });
  return invoke<PromptSearchResult[]>("search_prompts", { query, limit });
}

export async function addPromptFavorite(
  runId: string,
  seq: number,
  text: string,
): Promise<PromptFavorite> {
  dbg("api", "addPromptFavorite", { runId, seq });
  const result = await invoke<PromptFavorite>("add_prompt_favorite", { runId, seq, text });
  window.dispatchEvent(new Event("ocv:favorites-changed"));
  return result;
}

export async function removePromptFavorite(runId: string, seq: number): Promise<void> {
  dbg("api", "removePromptFavorite", { runId, seq });
  await invoke<void>("remove_prompt_favorite", { runId, seq });
  window.dispatchEvent(new Event("ocv:favorites-changed"));
}

export async function updatePromptFavoriteTags(
  runId: string,
  seq: number,
  tags: string[],
): Promise<void> {
  dbg("api", "updatePromptFavoriteTags", { runId, seq, tags });
  await invoke<void>("update_prompt_favorite_tags", { runId, seq, tags });
  window.dispatchEvent(new Event("ocv:favorites-changed"));
}

export async function updatePromptFavoriteNote(
  runId: string,
  seq: number,
  note: string,
): Promise<void> {
  dbg("api", "updatePromptFavoriteNote", { runId, seq, note });
  await invoke<void>("update_prompt_favorite_note", { runId, seq, note });
  window.dispatchEvent(new Event("ocv:favorites-changed"));
}

export async function listPromptFavorites(): Promise<PromptFavorite[]> {
  dbg("api", "listPromptFavorites");
  return invoke<PromptFavorite[]>("list_prompt_favorites");
}

export async function listPromptTags(): Promise<string[]> {
  dbg("api", "listPromptTags");
  return invoke<string[]>("list_prompt_tags");
}

// Run search (History)

export async function searchRuns(filters: RunSearchFilters): Promise<RunSearchResponse> {
  dbg("api", "searchRuns", filters);
  return invoke<RunSearchResponse>("search_runs", { filters });
}

export async function getRunFiles(runId: string): Promise<string[]> {
  dbg("api", "getRunFiles", { runId });
  return invoke<string[]>("get_run_files", { runId });
}

// Chat
export async function sendChatMessage(
  runId: string,
  message: string,
  attachments?: Attachment[],
  model?: string,
): Promise<void> {
  dbg("api", "sendChatMessage", {
    runId,
    msgLen: message.length,
    attachments: attachments?.length ?? 0,
  });
  return invoke("send_chat_message", { runId, message, attachments, model });
}

// CLI sync
export async function syncCliSession(runId: string): Promise<SyncResult> {
  dbg("api", "syncCliSession", { runId });
  return invoke<SyncResult>("sync_cli_session", { runId });
}

// Events
export async function getRunEvents(id: string, sinceSeq?: number): Promise<RunEvent[]> {
  dbg("api", "getRunEvents", { id, sinceSeq });
  return invoke<RunEvent[]>("get_run_events", { id, sinceSeq });
}

// Artifacts
export async function getRunArtifacts(id: string): Promise<RunArtifact> {
  dbg("api", "getRunArtifacts", id);
  return invoke<RunArtifact>("get_run_artifacts", { id });
}

// Settings
export async function getUserSettings(): Promise<UserSettings> {
  dbg("api", "getUserSettings");
  return invoke<UserSettings>("get_user_settings");
}

export async function updateUserSettings(patch: Partial<UserSettings>): Promise<UserSettings> {
  dbg("api", "updateUserSettings");
  return invoke<UserSettings>("update_user_settings", { patch });
}

export async function getAgentSettings(agent: string): Promise<AgentSettings> {
  dbg("api", "getAgentSettings", agent);
  return invoke<AgentSettings>("get_agent_settings", { agent });
}

export async function updateAgentSettings(
  agent: string,
  patch: Partial<AgentSettings>,
): Promise<AgentSettings> {
  dbg("api", "updateAgentSettings", agent);
  return invoke<AgentSettings>("update_agent_settings", { agent, patch });
}

// Filesystem
export async function listDirectory(path: string, showHidden?: boolean): Promise<DirListing> {
  dbg("api", "listDirectory", path, { showHidden });
  return invoke<DirListing>("list_directory", { path, showHidden });
}

export async function checkIsDirectory(path: string): Promise<boolean> {
  return invoke<boolean>("check_is_directory", { path });
}

export async function readFileBase64(path: string, cwd?: string): Promise<[string, string]> {
  return invoke<[string, string]>("read_file_base64", { path, cwd: cwd ?? null });
}

// Git
export async function getGitSummary(cwd: string): Promise<GitSummary> {
  dbg("api", "getGitSummary", cwd);
  return invoke<GitSummary>("get_git_summary", { cwd });
}

export async function getGitBranch(cwd: string): Promise<string> {
  dbg("api", "getGitBranch", cwd);
  return invoke<string>("get_git_branch", { cwd });
}

export async function getGitDiff(cwd: string, staged: boolean, file?: string): Promise<string> {
  dbg("api", "getGitDiff", { cwd, staged, file });
  return invoke<string>("get_git_diff", { cwd, staged, file: file ?? null });
}

export async function getGitStatus(cwd: string): Promise<string> {
  dbg("api", "getGitStatus", cwd);
  return invoke<string>("get_git_status", { cwd });
}

// Export
export async function exportConversation(runId: string): Promise<string> {
  dbg("api", "exportConversation", runId);
  return invoke<string>("export_conversation", { runId });
}

// Memory file candidates
export async function listMemoryFiles(
  cwd?: string,
): Promise<import("./types").MemoryFileCandidate[]> {
  dbg("api", "listMemoryFiles", { cwd });
  return invoke<import("./types").MemoryFileCandidate[]>("list_memory_files", { cwd: cwd ?? null });
}

// Files
export async function readTextFile(path: string, cwd?: string): Promise<string> {
  dbg("api", "readTextFile", path, { cwd });
  return invoke<string>("read_text_file", { path, cwd: cwd ?? null });
}

export async function writeTextFile(path: string, content: string, cwd?: string): Promise<void> {
  dbg("api", "writeTextFile", path, { cwd });
  return invoke("write_text_file", { path, content, cwd: cwd ?? null });
}

// Task output
export async function readTaskOutput(path: string): Promise<string> {
  dbg("api", "readTaskOutput", path);
  return invoke<string>("read_task_output", { path });
}

// Stats
export async function getUsageOverview(days?: number): Promise<UsageOverview> {
  dbg("api", "getUsageOverview", { days });
  return invoke<UsageOverview>("get_usage_overview", { days: days ?? null });
}

export async function getGlobalUsageOverview(days?: number): Promise<UsageOverview> {
  dbg("api", "getGlobalUsageOverview", { days });
  return invoke<UsageOverview>("get_global_usage_overview", { days: days ?? null });
}

export async function clearUsageCache(): Promise<void> {
  dbg("api", "clearUsageCache");
  return invoke<void>("clear_usage_cache");
}

export async function getHeatmapDaily(
  scope: "app" | "global",
): Promise<import("./types").DailyAggregate[]> {
  dbg("api", "getHeatmapDaily", { scope });
  return invoke<import("./types").DailyAggregate[]>("get_heatmap_daily", { scope });
}

// Diagnostics
export async function checkAgentCli(agent: string): Promise<CliCheckResult> {
  dbg("api", "checkAgentCli", agent);
  return invoke<CliCheckResult>("check_agent_cli", { agent });
}

export async function checkProjectInit(cwd: string): Promise<ProjectInitStatus> {
  dbg("api", "checkProjectInit", cwd);
  return invoke<ProjectInitStatus>("check_project_init", { cwd });
}

export async function getCliDistTags(): Promise<CliDistTags> {
  dbg("api", "getCliDistTags");
  return invoke<CliDistTags>("get_cli_dist_tags");
}

export async function checkSshKey(): Promise<SshKeyInfo> {
  dbg("api", "checkSshKey");
  return invoke<SshKeyInfo>("check_ssh_key");
}

export async function generateSshKey(): Promise<SshKeyInfo> {
  dbg("api", "generateSshKey");
  return invoke<SshKeyInfo>("generate_ssh_key");
}

export async function detectLocalProxy(
  proxyId: string,
  baseUrl: string,
): Promise<import("./types").LocalProxyStatus> {
  dbg("api", "detectLocalProxy", { proxyId, baseUrl });
  return invoke<import("./types").LocalProxyStatus>("detect_local_proxy", { proxyId, baseUrl });
}

export async function testApiConnectivity(
  apiKey: string,
  baseUrl: string,
  authEnvVar: string,
  model: string,
): Promise<import("./types").ApiTestResult> {
  dbg("api", "testApiConnectivity", { baseUrl, authEnvVar, model });
  return invoke<import("./types").ApiTestResult>("test_api_connectivity", {
    apiKey,
    baseUrl,
    authEnvVar,
    model,
  });
}

export async function runDiagnostics(cwd: string): Promise<DiagnosticsReport> {
  dbg("api", "runDiagnostics", { cwd });
  return invoke<DiagnosticsReport>("run_diagnostics", { cwd });
}

export async function testRemoteHost(
  host: string,
  user: string,
  port?: number,
  keyPath?: string,
  remoteClaudePath?: string,
): Promise<RemoteTestResult> {
  dbg("api", "testRemoteHost", { host, user, port });
  return invoke<RemoteTestResult>("test_remote_host", {
    host,
    user,
    port: port ?? null,
    keyPath: keyPath ?? null,
    remoteClaudePath: remoteClaudePath ?? null,
  });
}

// PTY
export async function spawnPty(runId: string, rows: number, cols: number): Promise<void> {
  dbg("api", "spawnPty", { runId, rows, cols });
  return invoke("spawn_pty", { runId, rows, cols });
}

export async function writePty(runId: string, data: string): Promise<void> {
  dbg("api", "writePty", { runId, dataLen: data.length });
  return invoke("write_pty", { runId, data });
}

export async function resizePty(runId: string, rows: number, cols: number): Promise<void> {
  dbg("api", "resizePty", { runId, rows, cols });
  return invoke("resize_pty", { runId, rows, cols });
}

// CLI Control Protocol
export async function getCliInfo(forceRefresh?: boolean): Promise<CliInfo> {
  dbg("api", "getCliInfo", { forceRefresh });
  try {
    const info = await invoke<CliInfo>("get_cli_info", { forceRefresh });
    dbg("api", "getCliInfo →", { models: info.models.length });
    return info;
  } catch (e) {
    dbgWarn("api", "getCliInfo error", e);
    throw e;
  }
}

// Session (event bus)
export async function startSession(
  runId: string,
  mode?: SessionMode,
  sessionId?: string,
  initialMessage?: string,
  attachments?: Array<{ content_base64: string; media_type: string; filename: string }>,
  platformId?: string,
): Promise<void> {
  dbg("api", "startSession", {
    runId,
    mode,
    sessionId,
    hasMessage: !!initialMessage,
    attachments: attachments?.length ?? 0,
    platformId,
  });
  return invoke("start_session", {
    runId,
    mode,
    sessionId,
    initialMessage,
    attachments: attachments ?? null,
    platformId: platformId ?? null,
  });
}

export async function sendSessionMessage(
  runId: string,
  message: string,
  attachments?: Array<{ content_base64: string; media_type: string; filename: string }>,
): Promise<void> {
  dbg("api", "sendSessionMessage", {
    runId,
    msgLen: message.length,
    attachments: attachments?.length ?? 0,
  });
  return invoke("send_session_message", {
    runId,
    message,
    attachments: attachments ?? null,
  });
}

export async function sendSessionControl(
  runId: string,
  subtype: string,
  params?: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  dbg("api", "sendSessionControl", { runId, subtype, params });
  try {
    const result = await invoke<Record<string, unknown>>("send_session_control", {
      runId,
      subtype,
      params: params ?? null,
    });
    dbg("api", "sendSessionControl →", result);
    return result;
  } catch (e) {
    dbgWarn("api", "sendSessionControl error", e);
    throw e;
  }
}

export async function stopSession(runId: string): Promise<void> {
  dbg("api", "stopSession", runId);
  return invoke("stop_session", { runId });
}

export interface LoadRunDataResult {
  run: TaskRun;
  busEvents: BusEvent[];
}

export async function loadRunData(id: string, syncCli = false): Promise<LoadRunDataResult> {
  dbg("api", "loadRunData", { id, syncCli });
  return invoke<LoadRunDataResult>("load_run_data", { id, syncCli });
}

export async function getBusEvents(id: string, sinceSeq?: number): Promise<BusEvent[]> {
  dbg("api", "getBusEvents", { id, sinceSeq });
  return invoke<BusEvent[]>("get_bus_events", { id, sinceSeq });
}

export async function getToolResult(
  runId: string,
  toolUseId: string,
): Promise<Record<string, unknown> | null> {
  dbg("api", "getToolResult", { runId, toolUseId });
  return invoke<Record<string, unknown> | null>("get_tool_result", { runId, toolUseId });
}

export async function forkSession(runId: string): Promise<string> {
  dbg("api", "forkSession", { runId });
  return invoke<string>("fork_session", { runId });
}

export async function sideQuestion(runId: string, question: string): Promise<string> {
  dbg("api", "sideQuestion", { runId, question: question.slice(0, 50) });
  return invoke<string>("side_question", { runId, question });
}

export async function approveSessionTool(runId: string, toolName: string): Promise<void> {
  dbg("api", "approveSessionTool", { runId, toolName });
  return invoke("approve_session_tool", { runId, toolName });
}

export async function respondPermission(
  runId: string,
  requestId: string,
  behavior: string,
  updatedPermissions?: import("./types").PermissionSuggestion[],
  updatedInput?: Record<string, unknown>,
  denyMessage?: string,
  interrupt?: boolean,
): Promise<void> {
  dbg("api", "respondPermission", {
    runId,
    requestId,
    behavior,
    updatedPermissions,
    updatedInput,
    denyMessage,
    interrupt,
  });
  return invoke("respond_permission", {
    runId,
    requestId,
    behavior,
    updatedPermissions: updatedPermissions ?? null,
    updatedInput: updatedInput ?? null,
    denyMessage: denyMessage ?? null,
    interrupt: interrupt ?? null,
  });
}

export async function respondHookCallback(
  runId: string,
  requestId: string,
  decision: "allow" | "deny",
): Promise<void> {
  dbg("api", "respondHookCallback", { runId, requestId, decision });
  return invoke("respond_hook_callback", { runId, requestId, decision });
}

// ── Typed control request wrappers ──

export async function setSessionModel(runId: string, model: string) {
  return sendSessionControl(runId, "set_model", { model });
}

export async function interruptSession(runId: string) {
  return sendSessionControl(runId, "interrupt");
}

export async function setPermissionMode(runId: string, mode: string) {
  return sendSessionControl(runId, "set_permission_mode", { mode });
}

export async function setMaxThinkingTokens(runId: string, tokens: number) {
  return sendSessionControl(runId, "set_max_thinking_tokens", { max_thinking_tokens: tokens });
}

export async function getMcpStatus(runId: string) {
  return sendSessionControl(runId, "mcp_status");
}

export async function setMcpServers(runId: string, servers: Record<string, unknown>) {
  return sendSessionControl(runId, "mcp_set_servers", { servers });
}

export async function reconnectMcpServer(runId: string, serverName: string) {
  return sendSessionControl(runId, "mcp_reconnect", { serverName });
}

export async function toggleMcpServer(runId: string, serverName: string, enabled: boolean) {
  return sendSessionControl(runId, "mcp_toggle", { serverName, enabled });
}

export async function broadcastMcpToggle(serverName: string, enabled: boolean): Promise<number> {
  return invoke<number>("broadcast_mcp_toggle", { serverName, enabled });
}

export async function getDisabledMcpServers(): Promise<string[]> {
  return invoke<string[]>("get_disabled_mcp_servers");
}

export async function toggleMcpServerConfig(
  serverName: string,
  enabled: boolean,
  scope: string,
  cwd?: string,
): Promise<{ success: boolean; message: string }> {
  dbg("api", "toggleMcpServerConfig", { serverName, enabled, scope, cwd });
  return invoke("toggle_mcp_server_config", {
    name: serverName,
    enabled,
    scope,
    cwd: cwd ?? null,
  });
}

export async function rewindFiles(
  runId: string,
  opts: { userMessageId: string; dryRun?: boolean; files?: string[] },
) {
  return sendSessionControl(runId, "rewind_files", {
    user_message_id: opts.userMessageId,
    ...(opts.dryRun ? { dry_run: true } : {}),
    ...(opts.files ? { files: opts.files } : {}),
  });
}

export async function cancelControlRequest(runId: string, requestId: string) {
  dbg("api", "cancelControlRequest", { runId, requestId });
  return invoke("cancel_control_request", { runId, requestId });
}

export async function respondElicitation(
  runId: string,
  requestId: string,
  action: "accept" | "decline" | "cancel",
  content?: Record<string, unknown>,
): Promise<void> {
  dbg("api", "respondElicitation", { runId, requestId, action });
  return invoke("respond_elicitation", {
    runId,
    requestId,
    action,
    content: content ?? null,
  });
}

// ── Teams ──

export async function listTeams(): Promise<TeamSummary[]> {
  dbg("api", "listTeams");
  return invoke<TeamSummary[]>("list_teams");
}

export async function getTeamConfig(name: string): Promise<TeamConfig> {
  dbg("api", "getTeamConfig", name);
  return invoke<TeamConfig>("get_team_config", { name });
}

export async function listTeamTasks(teamName: string): Promise<TeamTask[]> {
  dbg("api", "listTeamTasks", teamName);
  return invoke<TeamTask[]>("list_team_tasks", { teamName });
}

export async function getTeamTask(teamName: string, taskId: string): Promise<TeamTask> {
  dbg("api", "getTeamTask", { teamName, taskId });
  return invoke<TeamTask>("get_team_task", { teamName, taskId });
}

export async function getTeamInbox(
  teamName: string,
  agentName: string,
): Promise<TeamInboxMessage[]> {
  dbg("api", "getTeamInbox", { teamName, agentName });
  return invoke<TeamInboxMessage[]>("get_team_inbox", { teamName, agentName });
}

export async function getAllTeamInboxes(name: string): Promise<TeamInboxMessage[]> {
  dbg("api", "getAllTeamInboxes", name);
  return invoke<TeamInboxMessage[]>("get_all_team_inboxes", { name });
}

export async function deleteTeam(name: string): Promise<void> {
  dbg("api", "deleteTeam", name);
  return invoke<void>("delete_team", { name });
}

// ── Clipboard ──

export interface ClipboardFileInfo {
  path: string;
  name: string;
  size: number;
  mime_type: string;
}

export interface ClipboardFileContent {
  content_base64: string;
  content_text: string | null;
}

export async function getClipboardFiles(): Promise<ClipboardFileInfo[]> {
  dbg("api", "getClipboardFiles");
  return invoke<ClipboardFileInfo[]>("get_clipboard_files");
}

export async function readClipboardFile(
  path: string,
  asText: boolean,
): Promise<ClipboardFileContent> {
  dbg("api", "readClipboardFile", { path, asText });
  return invoke<ClipboardFileContent>("read_clipboard_file", { path, asText });
}

/** Save file to temp directory, return filesystem path. For >20MB PDFs from drag-drop/file picker. */
export async function saveTempAttachment(name: string, contentBase64: string): Promise<string> {
  dbg("api", "saveTempAttachment", { name, len: contentBase64.length });
  return invoke<string>("save_temp_attachment", { name, contentBase64 });
}

// ── Plugins ──

export async function listMarketplaces(): Promise<MarketplaceInfo[]> {
  dbg("api", "listMarketplaces");
  return invoke<MarketplaceInfo[]>("list_marketplaces");
}

export async function listMarketplacePlugins(): Promise<MarketplacePlugin[]> {
  dbg("api", "listMarketplacePlugins");
  return invoke<MarketplacePlugin[]>("list_marketplace_plugins");
}

export async function listProjectCommands(cwd?: string): Promise<import("./types").CliCommand[]> {
  dbg("api", "listProjectCommands", { cwd });
  return invoke<import("./types").CliCommand[]>("list_project_commands", { cwd: cwd ?? null });
}

export async function listStandaloneSkills(cwd?: string): Promise<StandaloneSkill[]> {
  dbg("api", "listStandaloneSkills", { cwd });
  return invoke<StandaloneSkill[]>("list_standalone_skills", { cwd: cwd ?? null });
}

export async function getSkillContent(path: string, cwd?: string): Promise<string> {
  dbg("api", "getSkillContent", path);
  return invoke<string>("get_skill_content", { path, cwd: cwd ?? "" });
}

export async function createSkill(
  name: string,
  description: string,
  content: string,
  scope: string,
  cwd?: string,
): Promise<StandaloneSkill> {
  dbg("api", "createSkill", { name, scope, cwd });
  return invoke<StandaloneSkill>("create_skill", {
    name,
    description,
    content,
    scope,
    cwd: cwd ?? null,
  });
}

export async function updateSkill(path: string, content: string, cwd?: string): Promise<void> {
  dbg("api", "updateSkill", { path, cwd });
  return invoke<void>("update_skill", { path, content, cwd: cwd ?? null });
}

export async function deleteSkill(path: string, cwd?: string): Promise<void> {
  dbg("api", "deleteSkill", { path, cwd });
  return invoke<void>("delete_skill", { path, cwd: cwd ?? null });
}

export async function listInstalledPlugins(): Promise<InstalledPlugin[]> {
  dbg("api", "listInstalledPlugins");
  return invoke<InstalledPlugin[]>("list_installed_plugins");
}

export async function installPlugin(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "installPlugin", { name, scope, cwd });
  return invoke<PluginOperationResult>("install_plugin", { name, scope, cwd });
}

export async function uninstallPlugin(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "uninstallPlugin", { name, scope, cwd });
  return invoke<PluginOperationResult>("uninstall_plugin", { name, scope, cwd });
}

export async function enablePlugin(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "enablePlugin", { name, scope, cwd });
  return invoke<PluginOperationResult>("enable_plugin", { name, scope, cwd });
}

export async function disablePlugin(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "disablePlugin", { name, scope, cwd });
  return invoke<PluginOperationResult>("disable_plugin", { name, scope, cwd });
}

export async function updatePlugin(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "updatePlugin", { name, scope, cwd });
  return invoke<PluginOperationResult>("update_plugin", { name, scope, cwd });
}

export async function addMarketplace(source: string): Promise<PluginOperationResult> {
  dbg("api", "addMarketplace", { source });
  return invoke<PluginOperationResult>("add_marketplace", { source });
}

export async function removeMarketplace(name: string): Promise<PluginOperationResult> {
  dbg("api", "removeMarketplace", { name });
  return invoke<PluginOperationResult>("remove_marketplace", { name });
}

export async function updateMarketplace(name?: string): Promise<PluginOperationResult> {
  dbg("api", "updateMarketplace", { name });
  return invoke<PluginOperationResult>("update_marketplace", { name: name ?? null });
}

// ── Community Skills ──

export async function checkCommunityHealth(): Promise<import("./types").ProviderHealth> {
  dbg("api", "checkCommunityHealth");
  return invoke<import("./types").ProviderHealth>("check_community_health");
}

export async function searchCommunitySkills(
  query: string,
  limit?: number,
): Promise<import("./types").CommunitySkillResult[]> {
  dbg("api", "searchCommunitySkills", { query, limit });
  return invoke<import("./types").CommunitySkillResult[]>("search_community_skills", {
    query,
    limit: limit ?? null,
  });
}

export async function getCommunitySkillDetail(
  source: string,
  skillId: string,
): Promise<import("./types").CommunitySkillDetail> {
  dbg("api", "getCommunitySkillDetail", { source, skillId });
  return invoke<import("./types").CommunitySkillDetail>("get_community_skill_detail", {
    source,
    skillId,
  });
}

export async function installCommunitySkill(
  source: string,
  skillId: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "installCommunitySkill", { source, skillId, scope });
  return invoke<PluginOperationResult>("install_community_skill", {
    source,
    skillId,
    scope,
    cwd: cwd ?? null,
  });
}

// ── MCP Registry ──

export async function listConfiguredMcpServers(cwd?: string): Promise<ConfiguredMcpServer[]> {
  dbg("api", "listConfiguredMcpServers", { cwd });
  return invoke<ConfiguredMcpServer[]>("list_configured_mcp_servers", { cwd: cwd ?? null });
}

export async function addMcpServer(
  name: string,
  transport: string,
  scope: string,
  cwd?: string,
  configJson?: string,
  url?: string,
  envVars?: Record<string, string>,
  headers?: Record<string, string>,
): Promise<PluginOperationResult> {
  dbg("api", "addMcpServer", { name, transport, scope });
  return invoke<PluginOperationResult>("add_mcp_server", {
    name,
    transport,
    scope,
    cwd: cwd ?? null,
    configJson: configJson ?? null,
    url: url ?? null,
    envVars: envVars ?? null,
    headers: headers ?? null,
  });
}

export async function removeMcpServer(
  name: string,
  scope: string,
  cwd?: string,
): Promise<PluginOperationResult> {
  dbg("api", "removeMcpServer", { name, scope, cwd });
  return invoke<PluginOperationResult>("remove_mcp_server", {
    name,
    scope,
    cwd: cwd ?? null,
  });
}

export async function checkMcpRegistryHealth(): Promise<ProviderHealth> {
  dbg("api", "checkMcpRegistryHealth");
  return invoke<ProviderHealth>("check_mcp_registry_health");
}

export async function searchMcpRegistry(
  query: string,
  limit?: number,
  cursor?: string,
): Promise<McpRegistrySearchResult> {
  dbg("api", "searchMcpRegistry", { query, limit, cursor });
  return invoke<McpRegistrySearchResult>("search_mcp_registry", {
    query,
    limit: limit ?? null,
    cursor: cursor ?? null,
  });
}

// ── CLI Permissions ──

export interface CliPermissions {
  user: { allow: string[]; deny: string[] };
  project: { allow: string[]; deny: string[] };
  projectError?: string | null;
}

export async function getCliPermissions(cwd?: string): Promise<CliPermissions> {
  dbg("api", "getCliPermissions", { cwd });
  return invoke<CliPermissions>("get_cli_permissions", { cwd: cwd ?? null });
}

export async function updateCliPermissions(
  scope: "user" | "project",
  category: "allow" | "deny",
  rules: string[],
  cwd?: string,
): Promise<void> {
  dbg("api", "updateCliPermissions", { scope, category, count: rules.length });
  return invoke<void>("update_cli_permissions", {
    scope,
    category,
    rules,
    cwd: cwd ?? null,
  });
}

// ── CLI Config ──

export async function getCliConfig(): Promise<Record<string, unknown>> {
  dbg("api", "getCliConfig");
  return invoke<Record<string, unknown>>("get_cli_config");
}

export async function getProjectCliConfig(cwd: string): Promise<Record<string, unknown>> {
  dbg("api", "getProjectCliConfig", { cwd });
  return invoke<Record<string, unknown>>("get_project_cli_config", { cwd });
}

export async function updateCliConfig(
  patch: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  dbg("api", "updateCliConfig", { patch: redactSensitive(patch) });
  return invoke<Record<string, unknown>>("update_cli_config", { patch });
}

// ── App Updates ──

export async function checkForUpdates(): Promise<import("./types").UpdateInfo> {
  dbg("api", "checkForUpdates");
  return invoke<import("./types").UpdateInfo>("check_for_updates");
}

// ── Changelog ──

export async function getChangelog(): Promise<ChangelogEntry[]> {
  dbg("api", "getChangelog");
  return invoke<ChangelogEntry[]>("get_changelog");
}

// ── Onboarding ──

export async function checkAuthStatus(): Promise<import("./types").AuthCheckResult> {
  dbg("api", "checkAuthStatus");
  return invoke<import("./types").AuthCheckResult>("check_auth_status");
}

export async function detectInstallMethods(): Promise<import("./types").InstallMethod[]> {
  dbg("api", "detectInstallMethods");
  return invoke<import("./types").InstallMethod[]>("detect_install_methods");
}

export async function runClaudeLogin(): Promise<boolean> {
  dbg("api", "runClaudeLogin");
  return invoke<boolean>("run_claude_login");
}

export async function getAuthOverview(): Promise<import("./types").AuthOverview> {
  dbg("api", "getAuthOverview");
  return invoke<import("./types").AuthOverview>("get_auth_overview");
}

export async function setCliApiKey(key: string): Promise<void> {
  dbg("api", "setCliApiKey");
  return invoke<void>("set_cli_api_key", { key });
}

export async function removeCliApiKey(): Promise<void> {
  dbg("api", "removeCliApiKey");
  return invoke<void>("remove_cli_api_key");
}

// ── Screenshot ──

export async function captureScreenshot(): Promise<void> {
  dbg("api", "captureScreenshot");
  return invoke<void>("capture_screenshot");
}

export async function updateScreenshotHotkey(hotkey: string | null): Promise<void> {
  dbg("api", "updateScreenshotHotkey", { hotkey });
  return invoke<void>("update_screenshot_hotkey", { hotkey });
}

// ── Web Server ──

export async function getWebServerToken(): Promise<string | null> {
  dbg("api", "getWebServerToken");
  return invoke<string | null>("get_web_server_token");
}

export async function getWebServerStatus(): Promise<{
  enabled: boolean;
  running: boolean;
  port: number;
  bind: string;
  warning?: string;
}> {
  dbg("api", "getWebServerStatus");
  return invoke<{
    enabled: boolean;
    running: boolean;
    port: number;
    bind: string;
    warning?: string;
  }>("get_web_server_status");
}

export async function regenerateWebServerToken(): Promise<string> {
  dbg("api", "regenerateWebServerToken");
  return invoke<string>("regenerate_web_server_token");
}

export interface WebServerConfig {
  enabled: boolean;
  port: number;
  bind: string;
  allowed_origins: string[] | null;
  tunnel_url: string | null;
}

export interface RestartResult {
  started: boolean;
  config_saved: boolean;
}

export async function restartWebServer(config: WebServerConfig): Promise<RestartResult> {
  dbg("api", "restartWebServer", { enabled: config.enabled, port: config.port });
  return invoke<RestartResult>("restart_web_server", { config });
}

export async function getLocalIp(preferV6: boolean): Promise<string | null> {
  dbg("api", "getLocalIp", { preferV6 });
  return invoke<string | null>("get_local_ip", { preferV6 });
}

// ── Agents ──

export async function listAgents(cwd?: string): Promise<AgentDefinitionSummary[]> {
  dbg("api", "listAgents", { cwd });
  return invoke<AgentDefinitionSummary[]>("list_agents", { cwd: cwd ?? null });
}

export async function readAgentFile(
  scope: "user" | "project",
  fileName: string,
  cwd?: string,
): Promise<string> {
  dbg("api", "readAgentFile", { scope, fileName });
  return invoke<string>("read_agent_file", {
    scope,
    fileName,
    cwd: cwd ?? null,
  });
}

export async function createAgentFile(
  scope: "user" | "project",
  fileName: string,
  content: string,
  cwd?: string,
): Promise<void> {
  dbg("api", "createAgentFile", { scope, fileName });
  return invoke<void>("create_agent_file", {
    scope,
    fileName,
    content,
    cwd: cwd ?? null,
  });
}

export async function updateAgentFile(
  scope: "user" | "project",
  fileName: string,
  content: string,
  cwd?: string,
): Promise<void> {
  dbg("api", "updateAgentFile", { scope, fileName });
  return invoke<void>("update_agent_file", {
    scope,
    fileName,
    content,
    cwd: cwd ?? null,
  });
}

export async function deleteAgentFile(
  scope: "user" | "project",
  fileName: string,
  cwd?: string,
): Promise<void> {
  dbg("api", "deleteAgentFile", { scope, fileName });
  return invoke<void>("delete_agent_file", {
    scope,
    fileName,
    cwd: cwd ?? null,
  });
}

// ── Preview ──

export async function openPreviewWindow(url: string, instanceId: string): Promise<void> {
  dbg("api", "openPreviewWindow", { url, instanceId });
  return invoke("open_preview_window", { url, instanceId });
}

export async function closePreviewWindow(): Promise<void> {
  dbg("api", "closePreviewWindow");
  return invoke("close_preview_window");
}

// ── Ralph Loop ──

export async function startRalphLoop(
  runId: string,
  prompt: string,
  maxIterations: number,
  completionPromise: string | null,
): Promise<void> {
  dbg("api", "startRalphLoop", { runId, maxIterations, completionPromise });
  return invoke<void>("start_ralph_loop", {
    runId,
    prompt,
    maxIterations,
    completionPromise,
  });
}

export async function cancelRalphLoop(
  runId: string,
): Promise<{ iteration: number; immediate: boolean }> {
  dbg("api", "cancelRalphLoop", { runId });
  return invoke<{ iteration: number; immediate: boolean }>("cancel_ralph_loop", { runId });
}

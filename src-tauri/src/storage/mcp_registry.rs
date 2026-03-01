use crate::agent::claude_stream::{augmented_path, resolve_claude_path};
use crate::models::{
    ConfiguredMcpServer, McpRegistrySearchResult, McpRegistryServer, PluginOperationResult,
    ProviderHealth,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

// ── Constants ──

const REGISTRY_BASE: &str = "https://registry.modelcontextprotocol.io/v0";
const CACHE_TTL: Duration = Duration::from_secs(120);
const HEALTH_TTL: Duration = Duration::from_secs(300);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CACHE_ENTRIES: usize = 100;

// Patterns to redact in args
const SENSITIVE_PATTERNS: &[&str] = &["token", "key", "secret", "bearer", "password", "auth"];

// ── HTTP client (reuse across requests) ──

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("OpenCovibe/0.1")
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(2)
        .build()
        .unwrap_or_default()
});

// ── Search cache: key → (timestamp, result) ──

type SearchCache = HashMap<String, (Instant, McpRegistrySearchResult)>;
static SEARCH_CACHE: LazyLock<Mutex<SearchCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Health cache ──

static HEALTH_CACHE: LazyLock<Mutex<Option<(Instant, ProviderHealth)>>> =
    LazyLock::new(|| Mutex::new(None));

// ── Install mutex (serialize add/remove) ──

static INSTALL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ── Intermediate deserialization structs ──
// The registry API wraps each server entry in a `server` object with optional `_meta`.

#[derive(Deserialize)]
struct RegistryApiResponse {
    #[serde(default)]
    servers: Vec<RegistryApiEntry>,
    #[serde(default)]
    metadata: Option<RegistryApiMetadata>,
}

#[derive(Deserialize)]
struct RegistryApiEntry {
    #[serde(default)]
    server: serde_json::Value,
    #[serde(default, rename = "_meta")]
    meta: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryApiMetadata {
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    count: Option<u32>,
}

// ── Public API ──

pub async fn health_check() -> ProviderHealth {
    // Check cache first
    {
        let cache = HEALTH_CACHE.lock().await;
        if let Some((ts, ref health)) = *cache {
            if ts.elapsed() < HEALTH_TTL {
                log::debug!(
                    "[mcp_registry] health_check: cached result={}",
                    health.available
                );
                return health.clone();
            }
        }
    }

    log::debug!("[mcp_registry] health_check: fetching from registry");
    let url = format!("{}/servers?search=test&limit=1", REGISTRY_BASE);
    let result = CLIENT.get(&url).send().await;

    let health = match result {
        Ok(resp) if resp.status().is_success() => ProviderHealth {
            available: true,
            reason: None,
        },
        Ok(resp) => ProviderHealth {
            available: false,
            reason: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => ProviderHealth {
            available: false,
            reason: Some(format!("{e}")),
        },
    };

    log::debug!(
        "[mcp_registry] health_check: available={}, reason={:?}",
        health.available,
        health.reason
    );

    let mut cache = HEALTH_CACHE.lock().await;
    *cache = Some((Instant::now(), health.clone()));
    health
}

pub async fn search(
    query: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<McpRegistrySearchResult, String> {
    let cache_key = format!(
        "{}:{}:{}",
        query.to_lowercase(),
        limit,
        cursor.unwrap_or("")
    );

    // Check cache
    {
        let cache = SEARCH_CACHE.lock().await;
        if let Some((ts, ref results)) = cache.get(&cache_key) {
            if ts.elapsed() < CACHE_TTL {
                log::debug!(
                    "[mcp_registry] search: cache hit for '{}', {} servers",
                    query,
                    results.servers.len()
                );
                return Ok(results.clone());
            }
        }
    }

    log::debug!(
        "[mcp_registry] search: query='{}', limit={}, cursor={:?}",
        query,
        limit,
        cursor
    );

    let mut url = format!("{}/servers?search={}&limit={}", REGISTRY_BASE, query, limit);
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", c));
    }

    let resp = CLIENT
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Search request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Registry API returned HTTP {}", resp.status()));
    }

    let body: RegistryApiResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse registry response: {e}"))?;

    // Lenient deserialization: parse each entry individually, skip failures
    let mut servers = Vec::new();
    for entry in &body.servers {
        // Only include latest versions
        if let Some(ref meta) = entry.meta {
            if let Some(is_latest) = meta.get("isLatest") {
                if is_latest == &serde_json::Value::Bool(false) {
                    continue;
                }
            }
        }

        match serde_json::from_value::<McpRegistryServer>(entry.server.clone()) {
            Ok(s) => servers.push(s),
            Err(e) => {
                log::debug!("[mcp_registry] skipping entry: parse error: {}", e);
            }
        }
    }

    // Deduplicate by name — keep the first occurrence (registry returns latest-first)
    let mut seen_names = std::collections::HashSet::new();
    servers.retain(|s| seen_names.insert(s.name.clone()));

    let next_cursor = body.metadata.as_ref().and_then(|m| m.next_cursor.clone());
    let count = body
        .metadata
        .as_ref()
        .and_then(|m| m.count)
        .unwrap_or(servers.len() as u32);

    let result = McpRegistrySearchResult {
        servers,
        next_cursor,
        count,
    };

    log::debug!(
        "[mcp_registry] search: '{}' returned {} servers, next_cursor={:?}",
        query,
        result.servers.len(),
        result.next_cursor
    );

    // Store in cache (with eviction)
    {
        let mut cache = SEARCH_CACHE.lock().await;
        if cache.len() >= MAX_CACHE_ENTRIES {
            let now = Instant::now();
            cache.retain(|_, (ts, _)| now.duration_since(*ts) < CACHE_TTL);
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(cache_key, (Instant::now(), result.clone()));
    }

    Ok(result)
}

/// List configured MCP servers from all config file locations.
///
/// Reads from:
/// - `~/.claude.json` → `projects[cwd].mcpServers` → scope="local"
/// - `~/.claude.json` → top-level `mcpServers` → scope="user" (CLI primary location)
/// - `~/.claude/settings.json` → `mcpServers` → scope="user" (fallback)
/// - `{cwd}/.mcp.json` → `mcpServers` → scope="project"
pub fn list_configured(cwd: Option<&str>) -> Vec<ConfiguredMcpServer> {
    let mut servers = Vec::new();
    let home = match crate::storage::dirs_next() {
        Some(h) => h,
        None => {
            log::warn!("[mcp_registry] could not determine home directory");
            return servers;
        }
    };

    // 1. ~/.claude.json → projects[cwd].mcpServers (scope="local")
    if let Some(cwd_str) = cwd {
        if !cwd_str.is_empty() {
            let claude_json = home.join(".claude.json");
            if let Ok(content) = std::fs::read_to_string(&claude_json) {
                if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(project_servers) = root
                        .get("projects")
                        .and_then(|p| p.get(cwd_str))
                        .and_then(|p| p.get("mcpServers"))
                        .and_then(|v| v.as_object())
                    {
                        for (name, config) in project_servers {
                            servers.push(parse_mcp_entry(name, config, "local"));
                        }
                        log::debug!(
                            "[mcp_registry] local servers from ~/.claude.json: {}",
                            project_servers.len()
                        );
                    }
                }
            }
        }
    }

    // 2a. ~/.claude.json → top-level mcpServers (scope="user")
    //     CLI stores user-scope servers here via `claude mcp add --scope user`
    {
        let claude_json = home.join(".claude.json");
        if let Ok(content) = std::fs::read_to_string(&claude_json) {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(mcp_servers) = root.get("mcpServers").and_then(|v| v.as_object()) {
                    for (name, config) in mcp_servers {
                        servers.push(parse_mcp_entry(name, config, "user"));
                    }
                    log::debug!(
                        "[mcp_registry] user servers from ~/.claude.json: {}",
                        mcp_servers.len()
                    );
                }
            }
        }
    }

    // 2b. ~/.claude/settings.json → mcpServers (scope="user")
    let settings_path = home.join(".claude").join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(mcp_servers) = root.get("mcpServers").and_then(|v| v.as_object()) {
                for (name, config) in mcp_servers {
                    // Avoid duplicates if same name already found in ~/.claude.json
                    if !servers.iter().any(|s| s.name == *name && s.scope == "user") {
                        servers.push(parse_mcp_entry(name, config, "user"));
                    }
                }
                log::debug!(
                    "[mcp_registry] user servers from settings.json: {}",
                    mcp_servers.len()
                );
            }
        }
    }

    // 3. {cwd}/.mcp.json → mcpServers (scope="project")
    if let Some(cwd_str) = cwd {
        if !cwd_str.is_empty() {
            let mcp_json = std::path::PathBuf::from(cwd_str).join(".mcp.json");
            if let Ok(content) = std::fs::read_to_string(&mcp_json) {
                if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                    // Support both flat format and wrapped { mcpServers: {...} }
                    let mcp_obj = root
                        .get("mcpServers")
                        .and_then(|v| v.as_object())
                        .or_else(|| root.as_object());

                    if let Some(entries) = mcp_obj {
                        // Skip if entries look like the wrapper itself (has "mcpServers" key only)
                        let is_wrapper = entries.len() == 1 && entries.contains_key("mcpServers");
                        if !is_wrapper {
                            for (name, config) in entries {
                                servers.push(parse_mcp_entry(name, config, "project"));
                            }
                            log::debug!(
                                "[mcp_registry] project servers from .mcp.json: {}",
                                entries.len()
                            );
                        }
                    }
                }
            }
        }
    }

    log::debug!(
        "[mcp_registry] list_configured: {} total servers",
        servers.len()
    );
    servers
}

/// Parse a single MCP server entry from JSON config into ConfiguredMcpServer.
fn parse_mcp_entry(name: &str, config: &serde_json::Value, scope: &str) -> ConfiguredMcpServer {
    let server_type = config
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("stdio")
        .to_string();

    let command = config
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let args = config
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| {
                    let s = v.as_str().unwrap_or("").to_string();
                    redact_sensitive_arg(&s)
                })
                .collect()
        })
        .unwrap_or_default();

    let url = config
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Only expose env keys, not values
    let env_keys = config
        .get("env")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    // Only expose header names
    let header_keys = config
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    ConfiguredMcpServer {
        name: name.to_string(),
        server_type,
        scope: scope.to_string(),
        command,
        args,
        url,
        env_keys,
        header_keys,
    }
}

/// Redact arg values that match sensitive patterns.
fn redact_sensitive_arg(arg: &str) -> String {
    let lower = arg.to_lowercase();
    for pattern in SENSITIVE_PATTERNS {
        if lower.contains(pattern) {
            return "***".to_string();
        }
    }
    arg.to_string()
}

/// Add an MCP server via Claude CLI.
#[allow(clippy::too_many_arguments)]
pub async fn add_server(
    name: &str,
    transport: &str,
    scope: &str,
    cwd: Option<&str>,
    config_json: Option<&str>,
    url: Option<&str>,
    env_vars: Option<&HashMap<String, String>>,
    headers: Option<&HashMap<String, String>>,
) -> Result<PluginOperationResult, String> {
    validate_name(name)?;
    validate_scope(scope)?;

    // scope=local or project requires cwd
    if (scope == "local" || scope == "project") && cwd.map(|s| s.is_empty()).unwrap_or(true) {
        return Err(format!(
            "Scope '{}' requires a working directory (cwd)",
            scope
        ));
    }

    // CLI only accepts [a-zA-Z0-9_-] in names — derive local name from registry format
    // e.g. "ai.kubit/mcp-server" → "mcp-server", "com.letta/memory-mcp" → "memory-mcp"
    let local_name = to_cli_name(name);

    let _lock = INSTALL_LOCK.lock().await;

    let claude_bin = resolve_claude_path();
    let path_env = augmented_path();

    let mut cmd = Command::new(&claude_bin);

    match transport {
        "stdio" | "sse" => {
            // Use add-json: `claude mcp add-json --scope {scope} {name} '{json}'`
            let json_str = match config_json {
                Some(j) => j.to_string(),
                None => {
                    return Err("config_json is required for stdio/sse transport".to_string());
                }
            };
            cmd.args(["mcp", "add-json", "--scope", scope, &local_name, &json_str]);
        }
        "http" => {
            // Use add: `claude mcp add --transport http --scope {scope} [-H "K: V"]... {name} {url}`
            let server_url = match url {
                Some(u) if !u.is_empty() => u,
                _ => return Err("url is required for http transport".to_string()),
            };
            cmd.args(["mcp", "add", "--transport", "http", "--scope", scope]);
            // Add headers
            if let Some(hdrs) = headers {
                for (k, v) in hdrs {
                    cmd.args(["-H", &format!("{}: {}", k, v)]);
                }
            }
            cmd.args([&local_name, server_url]);
        }
        _ => {
            return Err(format!("Unsupported transport: {}", transport));
        }
    }

    // Set env vars for stdio servers
    if let Some(env) = env_vars {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    cmd.env("PATH", &path_env)
        .env_remove("CLAUDECODE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Set cwd for local/project scope
    if let Some(cwd_str) = cwd {
        if !cwd_str.is_empty() {
            cmd.current_dir(cwd_str);
        }
    }

    log::debug!(
        "[mcp_registry] add_server: name={} → local_name={}, transport={}, scope={}",
        name,
        local_name,
        transport,
        scope
    );

    let child = cmd.spawn().map_err(|e| {
        log::error!("[mcp_registry] failed to spawn claude: {}", e);
        format!("Failed to spawn claude: {e}")
    })?;

    let result = timeout(CMD_TIMEOUT, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let success = output.status.success();

            log::debug!(
                "[mcp_registry] add_server completed: success={}, stdout_len={}, stderr_len={}",
                success,
                stdout.len(),
                stderr.len()
            );
            if !success {
                log::debug!(
                    "[mcp_registry] add stderr: {}",
                    &stderr[..stderr.len().min(500)]
                );
            }

            Ok(PluginOperationResult {
                success,
                message: if success {
                    let msg = stdout.trim().to_string();
                    if msg.is_empty() {
                        format!("Added MCP server '{}'", name)
                    } else {
                        msg
                    }
                } else {
                    stderr.trim().to_string()
                },
            })
        }
        Ok(Err(e)) => {
            log::error!("[mcp_registry] process error: {}", e);
            Err(format!("Process error: {e}"))
        }
        Err(_) => {
            log::error!(
                "[mcp_registry] command timed out after {}s",
                CMD_TIMEOUT.as_secs()
            );
            Err(format!(
                "Command timed out after {}s",
                CMD_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Remove an MCP server via Claude CLI.
pub async fn remove_server(
    name: &str,
    scope: &str,
    cwd: Option<&str>,
) -> Result<PluginOperationResult, String> {
    validate_name(name)?;
    validate_scope(scope)?;

    // scope=local or project requires cwd
    if (scope == "local" || scope == "project") && cwd.map(|s| s.is_empty()).unwrap_or(true) {
        return Err(format!(
            "Scope '{}' requires a working directory (cwd)",
            scope
        ));
    }

    let _lock = INSTALL_LOCK.lock().await;

    let claude_bin = resolve_claude_path();
    let path_env = augmented_path();

    let mut cmd = Command::new(&claude_bin);
    cmd.args(["mcp", "remove", "--scope", scope, name]);
    cmd.env("PATH", &path_env)
        .env_remove("CLAUDECODE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Set cwd for local/project scope
    if let Some(cwd_str) = cwd {
        if !cwd_str.is_empty() {
            cmd.current_dir(cwd_str);
        }
    }

    log::debug!(
        "[mcp_registry] remove_server: name={}, scope={}",
        name,
        scope
    );

    let child = cmd.spawn().map_err(|e| {
        log::error!("[mcp_registry] failed to spawn claude: {}", e);
        format!("Failed to spawn claude: {e}")
    })?;

    let result = timeout(CMD_TIMEOUT, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let success = output.status.success();

            log::debug!(
                "[mcp_registry] remove_server completed: success={}, stdout_len={}, stderr_len={}",
                success,
                stdout.len(),
                stderr.len()
            );

            Ok(PluginOperationResult {
                success,
                message: if success {
                    let msg = stdout.trim().to_string();
                    if msg.is_empty() {
                        format!("Removed MCP server '{}'", name)
                    } else {
                        msg
                    }
                } else {
                    stderr.trim().to_string()
                },
            })
        }
        Ok(Err(e)) => {
            log::error!("[mcp_registry] process error: {}", e);
            Err(format!("Process error: {e}"))
        }
        Err(_) => {
            log::error!(
                "[mcp_registry] command timed out after {}s",
                CMD_TIMEOUT.as_secs()
            );
            Err(format!(
                "Command timed out after {}s",
                CMD_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Toggle an MCP server's disabled state by modifying the config file directly.
/// Claude CLI does not support toggle via the stream-json control protocol,
/// so we set/remove `"disabled": true` in the config JSON.
pub fn toggle_server_config(
    name: &str,
    enabled: bool,
    scope: &str,
    cwd: Option<&str>,
) -> Result<PluginOperationResult, String> {
    let home = crate::storage::dirs_next()
        .ok_or_else(|| "Could not determine home directory".to_string())?;

    // Determine which config file and JSON path to modify
    let (config_path, json_path) = match scope {
        "local" => {
            let cwd_str = cwd
                .filter(|s| !s.is_empty())
                .ok_or("Local scope requires a working directory")?;
            (home.join(".claude.json"), Some(cwd_str.to_string()))
        }
        "user" => (home.join(".claude.json"), None),
        "project" => {
            let cwd_str = cwd
                .filter(|s| !s.is_empty())
                .ok_or("Project scope requires a working directory")?;
            (
                std::path::PathBuf::from(cwd_str).join(".mcp.json"),
                None,
            )
        }
        _ => return Err(format!("Unknown scope: {}", scope)),
    };

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;

    // Navigate to the correct mcpServers object
    let servers = if let Some(ref cwd_str) = json_path {
        // local scope: projects[cwd].mcpServers
        root.pointer_mut(&format!("/projects/{}/mcpServers", cwd_str.replace('~', "~0").replace('/', "~1")))
    } else if scope == "project" {
        // project scope: mcpServers in .mcp.json (may be top-level or nested)
        if root.get("mcpServers").is_some() {
            root.get_mut("mcpServers")
        } else {
            Some(&mut root)
        }
    } else {
        // user scope: top-level mcpServers
        root.get_mut("mcpServers")
    };

    let servers = servers
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| format!("mcpServers not found in {}", config_path.display()))?;

    let server = servers
        .get_mut(name)
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| format!("MCP server '{}' not found", name))?;

    if enabled {
        server.remove("disabled");
    } else {
        server.insert("disabled".to_string(), serde_json::Value::Bool(true));
    }

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&config_path, output)
        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;

    let action = if enabled { "Enabled" } else { "Disabled" };
    log::debug!(
        "[mcp_registry] toggle_server_config: {} '{}' in {}",
        action,
        name,
        config_path.display()
    );

    Ok(PluginOperationResult {
        success: true,
        message: format!("{} MCP server '{}'", action, name),
    })
}

// ── Validators ──

/// Convert a registry name to a CLI-friendly local name.
/// Registry uses reverse-domain format: "ai.kubit/mcp-server" → "mcp-server"
/// Falls back to replacing dots with hyphens if no slash present.
fn to_cli_name(name: &str) -> String {
    // Take the part after the last '/'
    let base = name.rsplit('/').next().unwrap_or(name);
    // Replace any remaining dots with hyphens, filter to [a-zA-Z0-9_-]
    let slug: String = base
        .chars()
        .map(|c| if c == '.' { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if slug.is_empty() {
        // Fallback: sanitize the whole name
        name.chars()
            .map(|c| if c == '.' || c == '/' { '-' } else { c })
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    } else {
        slug
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Server name cannot be empty".into());
    }
    if name.len() > 128 {
        return Err("Server name too long (max 128 characters)".into());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("Server name contains invalid characters".into());
    }
    Ok(())
}

fn validate_scope(scope: &str) -> Result<(), String> {
    match scope {
        "local" | "user" | "project" => Ok(()),
        _ => Err(format!(
            "Invalid scope: {scope}. Must be \"local\", \"user\", or \"project\""
        )),
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_name() {
        assert!(validate_name("my-server").is_ok());
        assert!(validate_name("test_server_123").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("test\x00").is_err());
        assert!(validate_name(&"a".repeat(129)).is_err());
    }

    #[test]
    fn test_validate_scope() {
        assert!(validate_scope("local").is_ok());
        assert!(validate_scope("user").is_ok());
        assert!(validate_scope("project").is_ok());
        assert!(validate_scope("global").is_err());
        assert!(validate_scope("").is_err());
    }

    #[test]
    fn test_redact_sensitive_arg() {
        assert_eq!(redact_sensitive_arg("hello"), "hello");
        assert_eq!(redact_sensitive_arg("my-api-token-123"), "***");
        assert_eq!(redact_sensitive_arg("GITHUB_KEY"), "***");
        assert_eq!(redact_sensitive_arg("Bearer xyz"), "***");
        assert_eq!(redact_sensitive_arg("some-secret-value"), "***");
        assert_eq!(redact_sensitive_arg("password=abc"), "***");
        assert_eq!(redact_sensitive_arg("normal-arg"), "normal-arg");
    }

    #[test]
    fn test_parse_mcp_entry_stdio() {
        let config = serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"],
            "env": {
                "NODE_ENV": "production",
                "API_TOKEN": "secret123"
            }
        });

        let entry = parse_mcp_entry("filesystem", &config, "user");
        assert_eq!(entry.name, "filesystem");
        assert_eq!(entry.server_type, "stdio");
        assert_eq!(entry.scope, "user");
        assert_eq!(entry.command, Some("npx".to_string()));
        assert_eq!(entry.args.len(), 3);
        assert_eq!(entry.env_keys.len(), 2);
        assert!(entry.env_keys.contains(&"NODE_ENV".to_string()));
        assert!(entry.env_keys.contains(&"API_TOKEN".to_string()));
    }

    #[test]
    fn test_parse_mcp_entry_http() {
        let config = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": {
                "Authorization": "Bearer xyz",
                "X-Custom": "val"
            }
        });

        let entry = parse_mcp_entry("remote-server", &config, "project");
        assert_eq!(entry.name, "remote-server");
        assert_eq!(entry.server_type, "http");
        assert_eq!(entry.scope, "project");
        assert_eq!(entry.url, Some("https://example.com/mcp".to_string()));
        assert_eq!(entry.header_keys.len(), 2);
        assert!(entry.command.is_none());
    }

    #[test]
    fn test_parse_mcp_entry_default_type() {
        let config = serde_json::json!({
            "command": "my-server"
        });

        let entry = parse_mcp_entry("test", &config, "local");
        assert_eq!(entry.server_type, "stdio"); // default
    }

    #[test]
    fn test_redact_args() {
        let config = serde_json::json!({
            "type": "stdio",
            "command": "server",
            "args": ["--port", "8080", "--api-token", "secret123"]
        });

        let entry = parse_mcp_entry("test", &config, "user");
        assert_eq!(entry.args[0], "--port");
        assert_eq!(entry.args[1], "8080");
        assert_eq!(entry.args[2], "***"); // contains "token"
        assert_eq!(entry.args[3], "***"); // the actual secret value doesn't match but "secret123" contains "secret"
    }
}

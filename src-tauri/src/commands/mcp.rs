use crate::models::{
    ConfiguredMcpServer, McpRegistrySearchResult, PluginOperationResult, ProviderHealth,
};
use std::collections::HashMap;

#[tauri::command]
pub fn list_configured_mcp_servers(
    cwd: Option<String>,
) -> Result<Vec<ConfiguredMcpServer>, String> {
    log::debug!("[mcp] list_configured_mcp_servers: cwd={:?}", cwd);
    Ok(crate::storage::mcp_registry::list_configured(
        cwd.as_deref(),
    ))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_mcp_server(
    name: String,
    transport: String,
    scope: String,
    cwd: Option<String>,
    config_json: Option<String>,
    url: Option<String>,
    env_vars: Option<HashMap<String, String>>,
    headers: Option<HashMap<String, String>>,
) -> Result<PluginOperationResult, String> {
    log::debug!(
        "[mcp] add_mcp_server: name={}, transport={}, scope={}, cwd={:?}",
        name,
        transport,
        scope,
        cwd
    );
    crate::storage::mcp_registry::add_server(
        &name,
        &transport,
        &scope,
        cwd.as_deref(),
        config_json.as_deref(),
        url.as_deref(),
        env_vars.as_ref(),
        headers.as_ref(),
    )
    .await
}

#[tauri::command]
pub async fn remove_mcp_server(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    log::debug!(
        "[mcp] remove_mcp_server: name={}, scope={}, cwd={:?}",
        name,
        scope,
        cwd
    );
    crate::storage::mcp_registry::remove_server(&name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub fn toggle_mcp_server_config(
    name: String,
    enabled: bool,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    log::debug!(
        "[mcp] toggle_mcp_server_config: name={}, enabled={}, scope={}, cwd={:?}",
        name,
        enabled,
        scope,
        cwd
    );
    crate::storage::mcp_registry::toggle_server_config(&name, enabled, &scope, cwd.as_deref())
}

#[tauri::command]
pub async fn check_mcp_registry_health() -> Result<ProviderHealth, String> {
    log::debug!("[mcp] check_mcp_registry_health");
    Ok(crate::storage::mcp_registry::health_check().await)
}

#[tauri::command]
pub async fn search_mcp_registry(
    query: String,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<McpRegistrySearchResult, String> {
    log::debug!(
        "[mcp] search_mcp_registry: query={}, limit={:?}, cursor={:?}",
        query,
        limit,
        cursor
    );
    let q = query.trim();
    if q.len() < 2 {
        return Err("Query must be at least 2 characters".into());
    }
    if q.len() > 200 {
        return Err("Query too long (max 200 characters)".into());
    }
    crate::storage::mcp_registry::search(q, limit.unwrap_or(30), cursor.as_deref()).await
}

use crate::models::{
    CliCommand, CommunitySkillDetail, CommunitySkillResult, InstalledPlugin, MarketplaceInfo,
    MarketplacePlugin, PluginOperationResult, ProviderHealth, StandaloneSkill,
};

/// Validate and resolve cwd for plugin commands.
/// Returns `Some(cwd)` when scope is project/local (cwd required),
/// `None` for user/managed scope (cwd not needed).
fn validate_plugin_cwd<'a>(scope: &str, cwd: Option<&'a str>) -> Result<Option<&'a str>, String> {
    if scope == "project" || scope == "local" {
        match cwd {
            Some(dir) if !dir.is_empty() => {
                if !std::path::Path::new(dir).is_dir() {
                    return Err(format!("Working directory does not exist: {}", dir));
                }
                Ok(Some(dir))
            }
            _ => Err(format!(
                "Scope '{}' requires a working directory (cwd)",
                scope
            )),
        }
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn list_marketplaces() -> Result<Vec<MarketplaceInfo>, String> {
    log::debug!("[plugins] list_marketplaces");
    Ok(crate::storage::plugins::list_marketplaces())
}

#[tauri::command]
pub fn list_marketplace_plugins() -> Result<Vec<MarketplacePlugin>, String> {
    log::debug!("[plugins] list_marketplace_plugins");
    Ok(crate::storage::plugins::list_marketplace_plugins())
}

#[tauri::command]
pub fn list_project_commands(cwd: Option<String>) -> Result<Vec<CliCommand>, String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!("[plugins] list_project_commands: cwd={}", cwd);
    Ok(crate::storage::plugins::list_project_commands(&cwd))
}

#[tauri::command]
pub fn list_standalone_skills(cwd: Option<String>) -> Result<Vec<StandaloneSkill>, String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!("[plugins] list_standalone_skills: cwd={}", cwd);
    Ok(crate::storage::plugins::list_standalone_skills(&cwd))
}

#[tauri::command]
pub fn get_skill_content(path: String, cwd: Option<String>) -> Result<String, String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!("[plugins] get_skill_content: path={}, cwd={}", path, cwd);
    crate::storage::plugins::read_skill_content(&path, &cwd)
}

#[tauri::command]
pub fn create_skill(
    name: String,
    description: String,
    content: String,
    scope: String,
    cwd: Option<String>,
) -> Result<StandaloneSkill, String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!(
        "[plugins] create_skill: name={}, scope={}, cwd={}",
        name,
        scope,
        cwd
    );
    crate::storage::plugins::create_skill(&name, &description, &content, &scope, &cwd)
}

#[tauri::command]
pub fn update_skill(path: String, content: String, cwd: Option<String>) -> Result<(), String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!("[plugins] update_skill: path={}, cwd={}", path, cwd);
    crate::storage::plugins::update_skill_content(&path, &content, &cwd)
}

#[tauri::command]
pub fn delete_skill(path: String, cwd: Option<String>) -> Result<(), String> {
    let cwd = cwd.unwrap_or_default();
    log::debug!("[plugins] delete_skill: path={}, cwd={}", path, cwd);
    crate::storage::plugins::delete_skill(&path, &cwd)
}

// ── L2: Plugin lifecycle commands ──

#[tauri::command]
pub async fn list_installed_plugins() -> Result<Vec<InstalledPlugin>, String> {
    log::debug!("[plugins] list_installed_plugins");
    crate::storage::plugins::list_installed_plugins_cli().await
}

/// Shared helper for install/uninstall/enable/disable/update — all have identical structure.
async fn plugin_lifecycle_op(
    verb: &str,
    name: &str,
    scope: &str,
    cwd: Option<&str>,
) -> Result<PluginOperationResult, String> {
    log::debug!(
        "[plugins] {}: name={}, scope={}, cwd={:?}",
        verb,
        name,
        scope,
        cwd
    );
    crate::storage::plugins::validate_plugin_name(name)?;
    crate::storage::plugins::validate_scope(scope)?;
    let effective_cwd = validate_plugin_cwd(scope, cwd)?;
    let result =
        crate::storage::plugins::run_plugin_command(&[verb, name, "--scope", scope], effective_cwd)
            .await?;
    Ok(PluginOperationResult {
        success: result.success,
        message: if result.success {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
    })
}

#[tauri::command]
pub async fn install_plugin(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    plugin_lifecycle_op("install", &name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub async fn uninstall_plugin(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    plugin_lifecycle_op("uninstall", &name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub async fn enable_plugin(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    plugin_lifecycle_op("enable", &name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub async fn disable_plugin(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    plugin_lifecycle_op("disable", &name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub async fn update_plugin(
    name: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    plugin_lifecycle_op("update", &name, &scope, cwd.as_deref()).await
}

#[tauri::command]
pub async fn add_marketplace(source: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] add_marketplace: source={}", source);
    crate::storage::plugins::validate_marketplace_source(&source)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["marketplace", "add", &source], None).await?;

    Ok(PluginOperationResult {
        success: result.success,
        message: if result.success {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
    })
}

#[tauri::command]
pub async fn remove_marketplace(name: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] remove_marketplace: name={}", name);
    crate::storage::plugins::validate_plugin_name(&name)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["marketplace", "remove", &name], None)
            .await?;

    Ok(PluginOperationResult {
        success: result.success,
        message: if result.success {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
    })
}

#[tauri::command]
pub async fn update_marketplace(name: Option<String>) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] update_marketplace: name={:?}", name);
    if let Some(ref n) = name {
        crate::storage::plugins::validate_plugin_name(n)?;
    }

    let args: Vec<&str> = match &name {
        Some(n) => vec!["marketplace", "update", n.as_str()],
        None => vec!["marketplace", "update"],
    };

    let result = crate::storage::plugins::run_plugin_command(&args, None).await?;

    Ok(PluginOperationResult {
        success: result.success,
        message: if result.success {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        },
    })
}

// ── Community skills (HTTP API) ──

#[tauri::command]
pub async fn check_community_health() -> Result<ProviderHealth, String> {
    log::debug!("[community] health_check");
    Ok(crate::storage::community_skills::health_check().await)
}

#[tauri::command]
pub async fn search_community_skills(
    query: String,
    limit: Option<u32>,
) -> Result<Vec<CommunitySkillResult>, String> {
    log::debug!("[community] search: query={}, limit={:?}", query, limit);
    crate::storage::community_skills::validate_query(&query)?;
    crate::storage::community_skills::search(&query, limit.unwrap_or(20)).await
}

#[tauri::command]
pub async fn get_community_skill_detail(
    source: String,
    skill_id: String,
) -> Result<CommunitySkillDetail, String> {
    log::debug!(
        "[community] detail: source={}, skill_id={}",
        source,
        skill_id
    );
    crate::storage::community_skills::validate_skill_id(&skill_id)?;
    crate::storage::community_skills::get_detail(&source, &skill_id).await
}

#[tauri::command]
pub async fn install_community_skill(
    source: String,
    skill_id: String,
    scope: String,
    cwd: Option<String>,
) -> Result<PluginOperationResult, String> {
    log::debug!(
        "[community] install: source={}, skill_id={}, scope={}",
        source,
        skill_id,
        scope
    );
    crate::storage::community_skills::validate_skill_id(&skill_id)?;
    crate::storage::community_skills::install_skill(&source, &skill_id, &scope, cwd.as_deref())
        .await
}

// ── Codex plugins commands ──

/// Map one entry of `codex plugin list --json`'s `installed[]` to InstalledPlugin.
fn codex_json_plugin(v: &serde_json::Value) -> Option<InstalledPlugin> {
    let name = v.get("name").and_then(|x| x.as_str())?.to_string();
    Some(InstalledPlugin {
        name,
        description: String::new(),
        version: v.get("version").and_then(|x| x.as_str()).map(String::from),
        scope: v
            .get("source")
            .and_then(|s| s.get("source"))
            .and_then(|x| x.as_str())
            .map(String::from),
        enabled: v.get("enabled").and_then(|x| x.as_bool()),
        marketplace: v
            .get("marketplaceName")
            .and_then(|x| x.as_str())
            .map(String::from),
        plugin_id: v.get("pluginId").and_then(|x| x.as_str()).map(String::from),
        agent: Some("codex".to_string()),
        ..Default::default()
    })
}

/// Authoritative installed-plugin list via `codex plugin list --json` (handles `.tmp` plugins +
/// install/auth policy that the cache-dir walk misses). Returns None on any failure so the caller
/// falls back to the filesystem scan. 12s timeout.
async fn list_codex_plugins_via_cli() -> Option<Vec<InstalledPlugin>> {
    let path = crate::agent::claude_stream::which_binary("codex")?;
    let aug_path = crate::agent::claude_stream::augmented_path();
    use crate::process_ext::HideConsole;
    use tokio::process::Command as TokioCommand;
    let mut cmd = TokioCommand::new(&path);
    cmd.args(["plugin", "list", "--json"])
        .env("PATH", &aug_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .hide_console()
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(12), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let installed = parsed.get("installed")?.as_array()?;
    let plugins: Vec<InstalledPlugin> = installed.iter().filter_map(codex_json_plugin).collect();
    log::debug!(
        "[plugins] list_codex_installed_plugins via CLI: {} plugin(s)",
        plugins.len()
    );
    Some(plugins)
}

#[tauri::command]
pub async fn list_codex_installed_plugins() -> Result<Vec<InstalledPlugin>, String> {
    log::debug!("[plugins] list_codex_installed_plugins");
    // Prefer the authoritative CLI list; fall back to the filesystem cache-dir scan when the CLI
    // is absent / errors / times out (keeps the panel working offline or on older codex).
    if let Some(plugins) = list_codex_plugins_via_cli().await {
        return Ok(plugins);
    }
    log::debug!("[plugins] list_codex_installed_plugins: CLI path unavailable, using dir scan");
    Ok(crate::storage::plugins::list_codex_installed_plugins())
}

#[tauri::command]
pub fn toggle_codex_plugin(plugin_id: String, enabled: bool) -> Result<(), String> {
    log::debug!("[plugins] toggle_codex_plugin: {}={}", plugin_id, enabled);
    crate::storage::plugins::toggle_codex_plugin(&plugin_id, enabled)
}

// ── Codex skills commands ──

#[tauri::command]
pub fn list_codex_skills(cwd: Option<String>) -> Result<Vec<StandaloneSkill>, String> {
    log::debug!("[plugins] list_codex_skills: cwd={:?}", cwd);
    Ok(crate::storage::plugins::list_codex_skills(cwd.as_deref()))
}

#[tauri::command]
pub fn create_codex_skill(
    name: String,
    description: String,
    content: String,
    scope: String,
    cwd: Option<String>,
) -> Result<StandaloneSkill, String> {
    log::debug!(
        "[plugins] create_codex_skill: name={}, scope={}",
        name,
        scope
    );
    crate::storage::plugins::create_codex_skill(
        &name,
        &description,
        &content,
        &scope,
        cwd.as_deref(),
    )
}

#[tauri::command]
pub fn delete_codex_skill(path: String, cwd: Option<String>) -> Result<(), String> {
    log::debug!("[plugins] delete_codex_skill: path={}", path);
    crate::storage::plugins::delete_codex_skill(&path, cwd.as_deref())
}

#[tauri::command]
pub fn toggle_codex_skill(
    skill_path: String,
    enabled: bool,
    cwd: Option<String>,
) -> Result<(), String> {
    log::debug!(
        "[plugins] toggle_codex_skill: path={}, enabled={}",
        skill_path,
        enabled
    );
    crate::storage::plugins::toggle_codex_skill(&skill_path, enabled, cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_json_plugin_maps_cli_entry() {
        // Real `codex plugin list --json` installed[] entry shape.
        let v = serde_json::json!({
            "pluginId": "github@openai-curated",
            "name": "github",
            "marketplaceName": "openai-curated",
            "version": "2abb1c44",
            "installed": true,
            "enabled": true,
            "source": { "source": "local", "path": "/x/.codex/.tmp/plugins/plugins/github" },
            "installPolicy": "AVAILABLE",
            "authPolicy": "ON_INSTALL"
        });
        let p = codex_json_plugin(&v).expect("maps");
        assert_eq!(p.name, "github");
        assert_eq!(p.plugin_id.as_deref(), Some("github@openai-curated"));
        assert_eq!(p.marketplace.as_deref(), Some("openai-curated"));
        assert_eq!(p.version.as_deref(), Some("2abb1c44"));
        assert_eq!(p.enabled, Some(true));
        assert_eq!(p.scope.as_deref(), Some("local"));
        assert_eq!(p.agent.as_deref(), Some("codex"));
        // Missing name → None (skipped).
        assert!(codex_json_plugin(&serde_json::json!({ "version": "x" })).is_none());
    }

    #[test]
    fn validate_plugin_cwd_requires_cwd_for_project_scope() {
        let result = validate_plugin_cwd("project", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a working directory"));
    }

    #[test]
    fn validate_plugin_cwd_requires_cwd_for_local_scope() {
        let result = validate_plugin_cwd("local", Some(""));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a working directory"));
    }

    #[test]
    fn validate_plugin_cwd_rejects_nonexistent_dir() {
        let result = validate_plugin_cwd("project", Some("/nonexistent_dir_12345"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn validate_plugin_cwd_user_scope_ignores_cwd() {
        let result = validate_plugin_cwd("user", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn validate_plugin_cwd_project_with_valid_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap();
        let result = validate_plugin_cwd("project", Some(dir));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(dir));
    }
}

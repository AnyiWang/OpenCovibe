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

#[cfg(test)]
mod tests {
    use super::*;

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

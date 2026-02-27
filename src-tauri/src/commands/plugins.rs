use crate::models::{
    CommunitySkillDetail, CommunitySkillResult, InstalledPlugin, MarketplaceInfo,
    MarketplacePlugin, PluginOperationResult, ProviderHealth, StandaloneSkill,
};

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

#[tauri::command]
pub async fn install_plugin(name: String, scope: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] install_plugin: name={}, scope={}", name, scope);
    crate::storage::plugins::validate_plugin_name(&name)?;
    crate::storage::plugins::validate_scope(&scope)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["install", &name, "--scope", &scope]).await?;

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
pub async fn uninstall_plugin(
    name: String,
    scope: String,
) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] uninstall_plugin: name={}, scope={}", name, scope);
    crate::storage::plugins::validate_plugin_name(&name)?;
    crate::storage::plugins::validate_scope(&scope)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["uninstall", &name, "--scope", &scope])
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
pub async fn enable_plugin(name: String, scope: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] enable_plugin: name={}, scope={}", name, scope);
    crate::storage::plugins::validate_plugin_name(&name)?;
    crate::storage::plugins::validate_scope(&scope)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["enable", &name, "--scope", &scope]).await?;

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
pub async fn disable_plugin(name: String, scope: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] disable_plugin: name={}, scope={}", name, scope);
    crate::storage::plugins::validate_plugin_name(&name)?;
    crate::storage::plugins::validate_scope(&scope)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["disable", &name, "--scope", &scope]).await?;

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
pub async fn update_plugin(name: String, scope: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] update_plugin: name={}, scope={}", name, scope);
    crate::storage::plugins::validate_plugin_name(&name)?;
    crate::storage::plugins::validate_scope(&scope)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["update", &name, "--scope", &scope]).await?;

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
pub async fn add_marketplace(source: String) -> Result<PluginOperationResult, String> {
    log::debug!("[plugins] add_marketplace: source={}", source);
    crate::storage::plugins::validate_marketplace_source(&source)?;

    let result =
        crate::storage::plugins::run_plugin_command(&["marketplace", "add", &source]).await?;

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
        crate::storage::plugins::run_plugin_command(&["marketplace", "remove", &name]).await?;

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

    let result = crate::storage::plugins::run_plugin_command(&args).await?;

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

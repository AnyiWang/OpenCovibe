use crate::models::{AgentSettings, AllSettings, UserSettings};
use std::fs;
use std::path::PathBuf;

fn settings_path() -> PathBuf {
    super::data_dir().join("settings.json")
}

pub fn load() -> AllSettings {
    let path = settings_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(mut settings) => {
                    log::debug!("[storage/settings] loaded settings from {}", path.display());
                    // Run one-time migrations on platform credentials
                    if migrate_platform_credentials(&mut settings) {
                        log::info!("[storage/settings] migrated platform credentials, saving");
                        let _ = save(&settings);
                    }
                    return settings;
                }
                Err(e) => {
                    log::warn!("[storage/settings] failed to parse settings: {}", e);
                }
            },
            Err(e) => {
                log::warn!("[storage/settings] failed to read settings: {}", e);
            }
        }
    }
    log::debug!("[storage/settings] using default settings");
    let defaults = AllSettings::default();
    let _ = save(&defaults);
    defaults
}

/// Known provider defaults for migration.
/// Must match the values in platform-presets.ts.
struct ProviderDefaults {
    base_url: Option<&'static str>,
    models: Option<Vec<String>>,
    extra_env: Option<std::collections::HashMap<String, String>>,
}

fn known_provider_defaults(pid: &str) -> Option<ProviderDefaults> {
    use std::collections::HashMap;
    match pid {
        "deepseek" => Some(ProviderDefaults {
            base_url: Some("https://api.deepseek.com/anthropic"),
            models: Some(vec!["deepseek-chat".to_string()]),
            extra_env: Some(HashMap::from([(
                "API_TIMEOUT_MS".to_string(),
                "600000".to_string(),
            )])),
        }),
        "kimi" => Some(ProviderDefaults {
            base_url: Some("https://api.moonshot.cn/anthropic"),
            models: Some(vec!["kimi-k2.5".to_string(), "kimi-k2".to_string()]),
            extra_env: None,
        }),
        "kimi-coding" => Some(ProviderDefaults {
            base_url: Some("https://api.kimi.com/coding/"),
            models: None,
            extra_env: None,
        }),
        "zhipu" => Some(ProviderDefaults {
            base_url: Some("https://open.bigmodel.cn/api/anthropic"),
            models: Some(vec![
                "glm-4.7".to_string(),
                "glm-4.5-air".to_string(),
                "glm-4.5-flash".to_string(),
            ]),
            extra_env: None,
        }),
        "bailian" => Some(ProviderDefaults {
            base_url: Some("https://dashscope.aliyuncs.com/apps/anthropic"),
            models: Some(vec![
                "qwen3-max".to_string(),
                "qwen3.5-plus".to_string(),
                "qwen-plus".to_string(),
                "qwen-flash".to_string(),
            ]),
            extra_env: None,
        }),
        "doubao" => Some(ProviderDefaults {
            base_url: Some("https://ark.cn-beijing.volces.com/api/coding"),
            models: Some(vec!["doubao-seed-code-preview-latest".to_string()]),
            extra_env: None,
        }),
        "minimax" => Some(ProviderDefaults {
            base_url: Some("https://api.minimax.io/anthropic"),
            models: Some(vec![
                "MiniMax-M2.5".to_string(),
                "MiniMax-M2.5-highspeed".to_string(),
            ]),
            extra_env: None,
        }),
        "minimax-cn" => Some(ProviderDefaults {
            base_url: Some("https://api.minimaxi.com/anthropic"),
            models: Some(vec![
                "MiniMax-M2.5".to_string(),
                "MiniMax-M2.5-highspeed".to_string(),
            ]),
            extra_env: None,
        }),
        "mimo" => Some(ProviderDefaults {
            base_url: Some("https://api.xiaomimimo.com/anthropic"),
            models: Some(vec!["mimo-v2-flash".to_string()]),
            extra_env: None,
        }),
        "openrouter" => Some(ProviderDefaults {
            base_url: Some("https://openrouter.ai/api"),
            models: None,
            extra_env: None,
        }),
        "aihubmix" => Some(ProviderDefaults {
            base_url: Some("https://aihubmix.com"),
            models: None,
            extra_env: None,
        }),
        "vercel" => Some(ProviderDefaults {
            base_url: Some("https://ai-gateway.vercel.sh"),
            models: None,
            extra_env: None,
        }),
        "ollama" => Some(ProviderDefaults {
            base_url: Some("http://localhost:11434"),
            models: None,
            extra_env: None,
        }),
        _ => None,
    }
}

/// Migrate stale platform credential data. Returns true if any changes were made.
///
/// Fixes:
/// - Incorrect auth_env_var for providers that need ANTHROPIC_API_KEY (x-api-key header)
/// - Old "minimax" credentials using minimaxi.com → rename to "minimax-cn" preset
/// - Missing models/extra_env on existing credentials (needed for ANTHROPIC_MODEL injection)
fn migrate_platform_credentials(settings: &mut AllSettings) -> bool {
    let auth_fixes: &[(&str, &str)] = &[
        ("deepseek", "ANTHROPIC_API_KEY"),
        ("zhipu", "ANTHROPIC_API_KEY"),
        ("doubao", "ANTHROPIC_API_KEY"),
        ("minimax", "ANTHROPIC_API_KEY"),
        ("minimax-cn", "ANTHROPIC_API_KEY"),
        ("mimo", "ANTHROPIC_API_KEY"),
    ];
    let mut changed = false;

    for cred in &mut settings.user.platform_credentials {
        // Fix auth_env_var
        for &(pid, correct) in auth_fixes {
            if cred.platform_id == pid && cred.auth_env_var.as_deref() != Some(correct) {
                log::info!(
                    "[storage/settings] migrating auth_env_var for '{}': {:?} → {}",
                    pid,
                    cred.auth_env_var,
                    correct
                );
                cred.auth_env_var = Some(correct.to_string());
                changed = true;
            }
        }

        // Migrate old "minimax" credentials that used minimaxi.com → "minimax-cn"
        if cred.platform_id == "minimax" {
            if let Some(ref url) = cred.base_url {
                if url.contains("api.minimaxi.com") {
                    log::info!(
                        "[storage/settings] migrating minimax credential with minimaxi.com to minimax-cn"
                    );
                    cred.platform_id = "minimax-cn".to_string();
                    changed = true;
                }
            }
        }

        // Populate base_url, models, and extra_env from known provider defaults if missing.
        // base_url is CRITICAL — without it, ANTHROPIC_BASE_URL is not set and
        // requests go to Anthropic's default endpoint instead of the third-party provider.
        if let Some(defaults) = known_provider_defaults(&cred.platform_id) {
            if cred.base_url.is_none() {
                if let Some(url) = defaults.base_url {
                    log::info!(
                        "[storage/settings] migrating base_url for '{}': {}",
                        cred.platform_id,
                        url
                    );
                    cred.base_url = Some(url.to_string());
                    changed = true;
                }
            }
            if cred.models.is_none() {
                if let Some(models) = defaults.models {
                    log::info!(
                        "[storage/settings] migrating models for '{}': {:?}",
                        cred.platform_id,
                        models
                    );
                    cred.models = Some(models);
                    changed = true;
                }
            }
            if cred.extra_env.is_none() {
                if let Some(extra) = defaults.extra_env {
                    log::info!(
                        "[storage/settings] migrating extra_env for '{}': {:?}",
                        cred.platform_id,
                        extra
                    );
                    cred.extra_env = Some(extra);
                    changed = true;
                }
            }
        }
    }

    // If active_platform_id was "minimax" but was migrated to "minimax-cn", update it
    if settings.user.active_platform_id.as_deref() == Some("minimax") {
        // Check if the minimax credential was migrated to minimax-cn
        let has_minimax_cn = settings
            .user
            .platform_credentials
            .iter()
            .any(|c| c.platform_id == "minimax-cn");
        let has_minimax = settings
            .user
            .platform_credentials
            .iter()
            .any(|c| c.platform_id == "minimax");
        if has_minimax_cn && !has_minimax {
            log::info!(
                "[storage/settings] migrating active_platform_id from minimax to minimax-cn"
            );
            settings.user.active_platform_id = Some("minimax-cn".to_string());
            changed = true;
        }
    }

    // Also fix the global auth_env_var if it was set by one of these providers
    // (only if active_platform_id matches a provider that needs fixing)
    if let Some(ref pid) = settings.user.active_platform_id {
        for &(fix_pid, correct) in auth_fixes {
            if pid == fix_pid && settings.user.auth_env_var.as_deref() != Some(correct) {
                log::info!(
                    "[storage/settings] migrating global auth_env_var for active platform '{}': {:?} → {}",
                    pid,
                    settings.user.auth_env_var,
                    correct
                );
                settings.user.auth_env_var = Some(correct.to_string());
                changed = true;
            }
        }
    }

    changed
}

pub fn save(settings: &AllSettings) -> Result<(), String> {
    log::debug!("[storage/settings] saving settings");
    let path = settings_path();
    super::ensure_dir(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, &json).map_err(|e| e.to_string())?;

    // Restrict file permissions — settings may contain API keys
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            log::warn!(
                "[storage/settings] failed to set permissions on settings.json: {}",
                e
            );
        }
    }

    Ok(())
}

pub fn get_user_settings() -> UserSettings {
    load().user
}

pub fn update_user_settings(patch: serde_json::Value) -> Result<UserSettings, String> {
    let mut all = load();
    if let Some(agent) = patch.get("default_agent").and_then(|v| v.as_str()) {
        all.user.default_agent = agent.to_string();
    }
    if let Some(model) = patch.get("default_model") {
        all.user.default_model = model.as_str().map(|s| s.to_string());
    }
    if let Some(tools) = patch.get("allowed_tools").and_then(|v| v.as_array()) {
        all.user.allowed_tools = tools
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(wd) = patch.get("working_directory") {
        all.user.working_directory = wd.as_str().map(|s| s.to_string());
    }
    if let Some(mode) = patch.get("provider_mode").and_then(|v| v.as_str()) {
        all.user.provider_mode = mode.to_string();
    }
    if let Some(mode) = patch.get("auth_mode").and_then(|v| v.as_str()) {
        all.user.auth_mode = mode.to_string();
    }
    if let Some(key) = patch.get("anthropic_api_key") {
        all.user.anthropic_api_key = key
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }
    if let Some(url) = patch.get("anthropic_base_url") {
        all.user.anthropic_base_url = url
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }
    if let Some(v) = patch.get("auth_env_var") {
        all.user.auth_env_var = v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
    }
    if let Some(mode) = patch.get("permission_mode").and_then(|v| v.as_str()) {
        all.user.permission_mode = mode.to_string();
    }
    if let Some(v) = patch.get("max_budget_usd") {
        all.user.max_budget_usd = if v.is_null() { None } else { v.as_f64() };
    }
    if let Some(v) = patch.get("fallback_model") {
        all.user.fallback_model = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("keybinding_overrides") {
        if v.is_null() {
            all.user.keybinding_overrides = vec![];
        } else {
            all.user.keybinding_overrides = serde_json::from_value(v.clone())
                .map_err(|e| format!("Invalid keybinding_overrides: {}", e))?;
        }
    }
    if let Some(v) = patch.get("remote_hosts") {
        if v.is_null() {
            all.user.remote_hosts = vec![];
        } else {
            all.user.remote_hosts = serde_json::from_value(v.clone())
                .map_err(|e| format!("Invalid remote_hosts: {}", e))?;
        }
    }
    if let Some(v) = patch.get("platform_credentials") {
        if v.is_null() {
            all.user.platform_credentials = vec![];
        } else {
            all.user.platform_credentials = serde_json::from_value(v.clone())
                .map_err(|e| format!("Invalid platform_credentials: {}", e))?;
        }
    }
    if let Some(v) = patch.get("active_platform_id") {
        all.user.active_platform_id = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("onboarding_completed") {
        all.user.onboarding_completed = v.as_bool().unwrap_or(false);
    }
    all.user.updated_at = crate::models::now_iso();
    save(&all)?;
    Ok(all.user)
}

pub fn get_agent_settings(agent: &str) -> AgentSettings {
    log::debug!("[storage/settings] get_agent_settings: agent={}", agent);
    let all = load();
    all.agents
        .get(agent)
        .cloned()
        .unwrap_or_else(|| AgentSettings::default_for(agent))
}

pub fn update_agent_settings(
    agent: &str,
    patch: serde_json::Value,
) -> Result<AgentSettings, String> {
    let mut all = load();
    let mut settings = all
        .agents
        .get(agent)
        .cloned()
        .unwrap_or_else(|| AgentSettings::default_for(agent));

    if let Some(model) = patch.get("model") {
        settings.model = model.as_str().map(|s| s.to_string());
    }
    if let Some(tools) = patch.get("allowed_tools").and_then(|v| v.as_array()) {
        settings.allowed_tools = tools
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(wd) = patch.get("working_directory") {
        settings.working_directory = wd.as_str().map(|s| s.to_string());
    }
    if let Some(v) = patch.get("plan_mode") {
        settings.plan_mode = if v.is_null() { None } else { v.as_bool() };
    }
    if let Some(v) = patch.get("disallowed_tools") {
        settings.disallowed_tools = if v.is_null() {
            None
        } else {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        };
    }
    if let Some(v) = patch.get("append_system_prompt") {
        settings.append_system_prompt = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("max_budget_usd") {
        settings.max_budget_usd = if v.is_null() { None } else { v.as_f64() };
    }
    if let Some(v) = patch.get("fallback_model") {
        settings.fallback_model = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("system_prompt") {
        settings.system_prompt = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("tool_set") {
        settings.tool_set = if v.is_null() {
            None
        } else {
            v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("add_dirs") {
        settings.add_dirs = if v.is_null() {
            None
        } else {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
        };
    }
    if let Some(v) = patch.get("json_schema") {
        settings.json_schema = if v.is_null() { None } else { Some(v.clone()) };
    }
    if let Some(v) = patch.get("include_partial_messages") {
        settings.include_partial_messages = if v.is_null() { None } else { v.as_bool() };
    }
    if let Some(v) = patch.get("cli_debug") {
        settings.cli_debug = if v.is_null() {
            None
        } else {
            // Allow empty string (means "--debug" with no filter)
            v.as_str().map(|s| s.to_string())
        };
    }
    if let Some(v) = patch.get("no_session_persistence") {
        settings.no_session_persistence = if v.is_null() { None } else { v.as_bool() };
    }
    settings.updated_at = crate::models::now_iso();
    all.agents.insert(agent.to_string(), settings.clone());
    save(&all)?;
    Ok(settings)
}

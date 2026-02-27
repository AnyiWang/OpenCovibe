use crate::storage::teams::claude_home_dir;
use serde_json::Value;
use std::path::PathBuf;

/// Path to the user-level CLI settings file: ~/.claude/settings.json
fn cli_config_path() -> PathBuf {
    claude_home_dir().join("settings.json")
}

/// Load user-level CLI config (~/.claude/settings.json).
/// Returns `{}` if the file doesn't exist or is invalid.
pub fn load_cli_config() -> Value {
    let path = cli_config_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) if v.is_object() => {
                log::debug!("[cli_config] loaded {} keys", v.as_object().unwrap().len());
                v
            }
            Ok(_) => {
                log::warn!("[cli_config] not an object, returning {{}}");
                Value::Object(serde_json::Map::new())
            }
            Err(e) => {
                log::warn!("[cli_config] parse error: {}", e);
                Value::Object(serde_json::Map::new())
            }
        },
        Err(e) => {
            log::debug!("[cli_config] read error (expected if first run): {}", e);
            Value::Object(serde_json::Map::new())
        }
    }
}

/// Load project-level CLI config ({cwd}/.claude/settings.json).
/// Read-only — used for override indicator display.
pub fn load_project_cli_config(cwd: &str) -> Value {
    let path = PathBuf::from(cwd).join(".claude").join("settings.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) if v.is_object() => {
                log::debug!(
                    "[cli_config] project config loaded {} keys from {}",
                    v.as_object().unwrap().len(),
                    path.display()
                );
                v
            }
            Ok(_) => Value::Object(serde_json::Map::new()),
            Err(e) => {
                log::warn!("[cli_config] project parse error {}: {}", path.display(), e);
                Value::Object(serde_json::Map::new())
            }
        },
        Err(e) => {
            log::debug!("[cli_config] project read: {}: {}", path.display(), e);
            Value::Object(serde_json::Map::new())
        }
    }
}

/// Apply a shallow merge patch to the user-level CLI config.
/// - Only top-level keys in `patch` are written.
/// - `null` values delete the key (restore CLI default).
/// - All other existing keys are preserved (hooks, env, enabledPlugins, etc.).
/// - File permissions are set to 0o600 on unix.
pub fn update_cli_config(patch: Value) -> Result<Value, String> {
    let patch_obj = patch
        .as_object()
        .ok_or_else(|| "patch must be a JSON object".to_string())?;

    let mut config = load_cli_config();
    let map = config
        .as_object_mut()
        .expect("load_cli_config always returns object");

    const SENSITIVE_KEYS: &[&str] = &["apiKey", "primaryApiKey"];

    for (key, value) in patch_obj {
        if value.is_null() {
            log::debug!("[cli_config] deleting key: {}", key);
            map.remove(key);
        } else {
            if SENSITIVE_KEYS.contains(&key.as_str()) {
                log::debug!("[cli_config] setting key: {} = ***", key);
            } else {
                log::debug!("[cli_config] setting key: {} = {}", key, value);
            }
            map.insert(key.clone(), value.clone());
        }
    }

    // Write with pretty formatting
    let path = cli_config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let content =
        serde_json::to_string_pretty(&config).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&path, &content).map_err(|e| format!("Failed to write: {}", e))?;

    // Set file permissions to 0600 (user read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    log::debug!(
        "[cli_config] updated {} keys total",
        config.as_object().unwrap().len()
    );
    Ok(config)
}

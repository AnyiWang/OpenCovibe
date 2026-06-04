use crate::storage::teams::claude_home_dir;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Path to the user-level CLI settings file: ~/.claude/settings.json
fn cli_config_path() -> PathBuf {
    claude_home_dir().join("settings.json")
}

// ── Codex config support ──

/// Resolve CODEX_HOME directory.
/// Mirrors upstream: codex-rs/utils/home-dir/src/lib.rs:12
/// - If $CODEX_HOME is set and non-empty, validate it exists and is a directory.
/// - Otherwise fall back to ~/.codex
pub fn codex_home_dir() -> Result<PathBuf, String> {
    match std::env::var("CODEX_HOME").ok().filter(|v| !v.is_empty()) {
        Some(val) => {
            let path = PathBuf::from(&val);
            let meta = std::fs::metadata(&path)
                .map_err(|_| format!("CODEX_HOME={}: path does not exist", val))?;
            if !meta.is_dir() {
                return Err(format!("CODEX_HOME={}: not a directory", val));
            }
            log::debug!("[codex_config] CODEX_HOME env hit: {}", val);
            std::fs::canonicalize(&path)
                .map_err(|e| format!("CODEX_HOME={}: canonicalize failed: {}", val, e))
        }
        None => {
            let home = crate::storage::home_dir()
                .ok_or_else(|| "home directory not available".to_string())?;
            log::debug!("[codex_config] CODEX_HOME fallback: ~/.codex");
            Ok(PathBuf::from(home).join(".codex"))
        }
    }
}

/// Path to the user-level Codex config: $CODEX_HOME/config.toml
pub fn codex_config_path() -> Result<PathBuf, String> {
    codex_home_dir().map(|d| d.join("config.toml"))
}

/// Load user-level Codex config ($CODEX_HOME/config.toml).
/// Returns (config_as_json, optional_warning).
/// Warning scenarios: CODEX_HOME invalid, read permission error, TOML parse error.
/// File not found (first-run) → empty config, no warning.
pub fn load_codex_config() -> (Value, Option<String>) {
    let path = match codex_config_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[codex_config] codex_home_dir error: {}", e);
            return (Value::Object(serde_json::Map::new()), Some(e));
        }
    };

    match std::fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<toml::Value>(&s) {
            Ok(tv) => {
                let jv = toml_value_to_json(&tv);
                let count = jv.as_object().map_or(0, |m| m.len());
                log::debug!(
                    "[codex_config] loaded {} keys from {}",
                    count,
                    path.display()
                );
                (jv, None)
            }
            Err(e) => {
                let msg = format!("TOML parse error: {}", e);
                log::warn!("[codex_config] {}: {}", path.display(), msg);
                (Value::Object(serde_json::Map::new()), Some(msg))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::debug!(
                "[codex_config] file not found (first run): {}",
                path.display()
            );
            (Value::Object(serde_json::Map::new()), None)
        }
        Err(e) => {
            let msg = format!("Read error: {}", e);
            log::warn!("[codex_config] {}: {}", path.display(), msg);
            (Value::Object(serde_json::Map::new()), Some(msg))
        }
    }
}

/// Load project-level Codex config — ancestor .codex/config.toml chain.
/// Walks from project root to cwd, merging .codex/config.toml at each level.
/// Project root is found by .git directory. Does NOT read {cwd}/config.toml.
///
/// Divergence from upstream: does not read project_root_markers config
/// (upstream default: .git), does not handle trust context.
pub fn load_project_codex_config(cwd: &str) -> Value {
    let cwd_path = PathBuf::from(cwd);

    // Find project root by walking up to find .git
    let project_root = {
        let mut dir = cwd_path.as_path();
        loop {
            if dir.join(".git").exists() {
                break Some(dir.to_path_buf());
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break None,
            }
        }
    };

    let project_root = match project_root {
        Some(r) => r,
        None => {
            log::debug!(
                "[codex_config] no .git found from {}, no project config",
                cwd
            );
            return Value::Object(serde_json::Map::new());
        }
    };

    let mut merged = serde_json::Map::new();
    let mut layers = 0u32;

    // Walk from project_root towards cwd, reading .codex/config.toml at each ancestor.
    // Stop BEFORE reaching cwd — don't read cwd's own .codex/config.toml.
    let mut current = project_root.clone();
    loop {
        // Stop before reading cwd's layer
        if current == cwd_path {
            break;
        }

        let config_path = current.join(".codex").join("config.toml");
        if config_path.is_file() {
            if let Ok(s) = std::fs::read_to_string(&config_path) {
                if let Ok(tv) = toml::from_str::<toml::Value>(&s) {
                    let jv = toml_value_to_json(&tv);
                    if let Some(obj) = jv.as_object() {
                        for (k, v) in obj {
                            merged.insert(k.clone(), v.clone());
                        }
                        layers += 1;
                    }
                }
            }
        }

        // Advance towards cwd
        let relative = match cwd_path.strip_prefix(&current) {
            Ok(r) => r,
            Err(_) => break,
        };
        match relative.components().next() {
            Some(component) => current = current.join(component),
            None => break,
        }
    }

    log::debug!(
        "[codex_config] project config: scanned {} layers, {} keys merged",
        layers,
        merged.len()
    );
    Value::Object(merged)
}

/// Toggle a single Codex feature flag durably: `[features].<name> = enabled` (or remove the key
/// when `enabled` is None). A nested write — unlike `update_codex_config`'s top-level shallow merge,
/// this preserves the other keys in the `[features]` table instead of replacing the whole table.
/// Takes effect on the next spawned session. Returns the full reloaded config.
pub fn set_codex_feature(name: &str, enabled: Option<bool>) -> Result<Value, String> {
    if name.is_empty() {
        return Err("feature name must not be empty".to_string());
    }
    let config_path = codex_config_path()?;
    let mut doc: toml_edit::DocumentMut = match std::fs::read_to_string(&config_path) {
        Ok(s) => s
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("TOML parse error: {}", e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(format!("Read error: {}", e)),
    };

    // Ensure `[features]` is a STANDARD table. `doc["features"][k] = v` would create an INLINE
    // table (`features = { … }`), which reads worse and — crucially — cannot be mutated via
    // `as_table_mut()` (it's an Item::Value), so the removal branch would silently no-op. Normalize
    // any existing inline/missing form to a standard table before editing.
    let features = doc
        .entry("features")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(inline) = features.as_inline_table().cloned() {
        let mut table = toml_edit::Table::new();
        for (k, v) in inline.iter() {
            table.insert(k, toml_edit::Item::Value(v.clone()));
        }
        *features = toml_edit::Item::Table(table);
    }
    let table = features
        .as_table_mut()
        .ok_or_else(|| "config key `features` is not a table".to_string())?;
    match enabled {
        Some(value) => {
            log::debug!("[codex_config] set feature {} = {}", name, value);
            table[name] = toml_edit::value(value);
        }
        None => {
            log::debug!("[codex_config] clearing feature override {}", name);
            table.remove(name);
        }
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }
    std::fs::write(&config_path, doc.to_string()).map_err(|e| format!("Failed to write: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    let (config, _) = load_codex_config();
    Ok(config)
}

/// Apply a shallow merge patch to the user-level Codex config.
/// Uses toml_edit to preserve comments and formatting.
/// - null values delete the key.
/// - Creates parent directory if needed.
/// - Sets file permissions to 0o600 on unix.
pub fn update_codex_config(patch: Value) -> Result<Value, String> {
    let patch_obj = patch
        .as_object()
        .ok_or_else(|| "patch must be a JSON object".to_string())?;

    let config_path = codex_config_path()?;

    // Read existing file as toml_edit document (preserves comments)
    let mut doc: toml_edit::DocumentMut = match std::fs::read_to_string(&config_path) {
        Ok(s) => s
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("TOML parse error: {}", e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(format!("Read error: {}", e)),
    };

    for (key, value) in patch_obj {
        if value.is_null() {
            log::debug!("[codex_config] deleting key: {}", key);
            doc.remove(key);
        } else {
            log::debug!("[codex_config] setting key: {} = {}", key, value);
            doc[key] = json_to_toml_item(value);
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let content = doc.to_string();
    std::fs::write(&config_path, &content).map_err(|e| format!("Failed to write: {}", e))?;

    // Set file permissions to 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    // Re-read and return the full config
    let (config, _) = load_codex_config();
    log::debug!(
        "[codex_config] updated, {} keys total",
        config.as_object().map_or(0, |m| m.len())
    );
    Ok(config)
}

/// Convert a TOML value to a serde_json::Value.
fn toml_value_to_json(tv: &toml::Value) -> Value {
    match tv {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_value_to_json).collect()),
        toml::Value::Table(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, v) in tbl {
                map.insert(k.clone(), toml_value_to_json(v));
            }
            Value::Object(map)
        }
    }
}

/// Convert a serde_json::Value to a toml_edit::Item for writing.
fn json_to_toml_item(jv: &Value) -> toml_edit::Item {
    match jv {
        Value::String(s) => toml_edit::value(s.as_str()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml_edit::value(i)
            } else if let Some(f) = n.as_f64() {
                toml_edit::value(f)
            } else {
                toml_edit::value(n.to_string())
            }
        }
        Value::Bool(b) => toml_edit::value(*b),
        Value::Array(arr) => {
            let mut a = toml_edit::Array::new();
            for item in arr {
                match item {
                    Value::String(s) => a.push(s.as_str()),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            a.push(i);
                        } else if let Some(f) = n.as_f64() {
                            a.push(f);
                        }
                    }
                    Value::Bool(b) => a.push(*b),
                    _ => {} // skip nested complex types
                }
            }
            toml_edit::value(a)
        }
        Value::Object(obj) => {
            let mut tbl = toml_edit::Table::new();
            for (k, v) in obj {
                tbl[k] = json_to_toml_item(v);
            }
            toml_edit::Item::Table(tbl)
        }
        Value::Null => toml_edit::Item::None,
    }
}

// ── Codex hooks support ──

/// Path to the Codex hooks file: $CODEX_HOME/hooks.json
pub fn codex_hooks_path() -> Result<PathBuf, String> {
    codex_home_dir().map(|d| d.join("hooks.json"))
}

/// Load Codex hooks ($CODEX_HOME/hooks.json).
/// Returns (hooks_object, optional_warning).
/// File not found → ({}, None). Parse error → ({}, Some(warning)).
pub fn load_codex_hooks() -> (Value, Option<String>) {
    let path = match codex_hooks_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[codex_hooks] codex_home_dir error: {}", e);
            return (Value::Object(serde_json::Map::new()), Some(e));
        }
    };

    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) => match v.get("hooks") {
                Some(h) if h.is_object() => {
                    log::debug!("[codex_hooks] loaded from {}", path.display());
                    (h.clone(), None)
                }
                Some(_) => {
                    let msg = "hooks field is not an object".to_string();
                    log::warn!("[codex_hooks] {}: {}", path.display(), msg);
                    (Value::Object(serde_json::Map::new()), Some(msg))
                }
                None => {
                    log::debug!("[codex_hooks] no hooks field in {}", path.display());
                    (Value::Object(serde_json::Map::new()), None)
                }
            },
            Err(e) => {
                let msg = format!("JSON parse error: {}", e);
                log::warn!("[codex_hooks] {}: {}", path.display(), msg);
                (Value::Object(serde_json::Map::new()), Some(msg))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::debug!(
                "[codex_hooks] file not found (first run): {}",
                path.display()
            );
            (Value::Object(serde_json::Map::new()), None)
        }
        Err(e) => {
            let msg = format!("Read error: {}", e);
            log::warn!("[codex_hooks] {}: {}", path.display(), msg);
            (Value::Object(serde_json::Map::new()), Some(msg))
        }
    }
}

/// Replace the `hooks` field in $CODEX_HOME/hooks.json.
/// - `hooks` must be a JSON object.
/// - If the file exists but is malformed, returns Err (refuses to overwrite).
/// - Creates parent directory and sets 0o600 permissions.
pub fn update_codex_hooks(hooks: Value) -> Result<Value, String> {
    if !hooks.is_object() {
        return Err("hooks must be a JSON object".to_string());
    }

    let path = codex_hooks_path()?;

    // Read existing file to preserve other top-level keys
    let mut doc: Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str::<Value>(&s).map_err(|e| {
            format!(
                "hooks.json parse error (fix manually or delete file): {}",
                e
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            json!({})
        }
        Err(e) => return Err(format!("Read error: {}", e)),
    };

    // Ensure doc is an object
    if !doc.is_object() {
        doc = json!({});
    }

    doc["hooks"] = hooks.clone();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let content =
        serde_json::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&path, &content).map_err(|e| format!("Failed to write: {}", e))?;

    // Set file permissions to 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    log::debug!("[codex_hooks] updated hooks.json");
    Ok(hooks)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // CODEX_HOME is process-global; serialize the env-mutating tests so cargo's parallel
    // runner can't interleave a set/remove from another test in this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run `f` with CODEX_HOME pointed at a fresh temp dir, restoring the prior value after.
    fn with_codex_home<F: FnOnce(&std::path::Path)>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let orig = std::env::var("CODEX_HOME").ok();
        std::env::set_var("CODEX_HOME", dir.path());
        f(dir.path());
        match orig {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }

    #[test]
    fn set_codex_feature_nested_write_preserves_siblings() {
        with_codex_home(|home| {
            // First write creates [features].hooks = true.
            set_codex_feature("hooks", Some(true)).unwrap();
            // Second write must NOT clobber the [features] table — this is the whole point of
            // the nested write vs update_codex_config's top-level shallow merge.
            let cfg = set_codex_feature("plugins", Some(true)).unwrap();
            assert_eq!(cfg["features"]["hooks"], json!(true));
            assert_eq!(cfg["features"]["plugins"], json!(true));

            // Verify it actually landed on disk as a single nested table, not two clobbering writes.
            // Must be a STANDARD `[features]` table — an inline `features = {…}` can't be mutated
            // via as_table_mut(), which is exactly what broke the removal path before the fix.
            let on_disk = std::fs::read_to_string(home.join("config.toml")).unwrap();
            assert!(
                on_disk.contains("[features]"),
                "expected standard table: {on_disk}"
            );
            assert!(
                !on_disk.contains("features = {"),
                "must not be inline: {on_disk}"
            );
            assert!(on_disk.contains("hooks = true"), "disk: {on_disk}");
            assert!(on_disk.contains("plugins = true"), "disk: {on_disk}");

            // enabled=None removes only the named key; the sibling survives.
            let cleared = set_codex_feature("hooks", None).unwrap();
            assert!(
                cleared["features"].get("hooks").is_none(),
                "hooks should be removed: {cleared}"
            );
            assert_eq!(cleared["features"]["plugins"], json!(true));
        });
    }

    #[test]
    fn set_codex_feature_can_disable() {
        with_codex_home(|_home| {
            // Some(false) writes an explicit false (an override), distinct from None (remove).
            let cfg = set_codex_feature("hooks", Some(false)).unwrap();
            assert_eq!(cfg["features"]["hooks"], json!(false));
        });
    }

    #[test]
    fn set_codex_feature_rejects_empty_name() {
        // No CODEX_HOME needed — the guard rejects before any path resolution.
        assert!(set_codex_feature("", Some(true)).is_err());
    }
}

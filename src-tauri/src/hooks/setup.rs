use std::fs;
use std::path::{Path, PathBuf};

/// Production entry: resolve real paths and dispatch to the testable pure function.
pub fn cleanup_hook_bridge() {
    let bridge_path = crate::storage::data_dir().join("hook-bridge.mjs");
    let settings_path = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".claude").join("settings.json"));

    match settings_path {
        Ok(sp) => cleanup_hook_bridge_at(&sp, &bridge_path),
        Err(_) => {
            log::warn!(
                "[hooks/setup] HOME/USERPROFILE not set, cannot locate ~/.claude/settings.json. \
                 Skipping hook-bridge cleanup. To fix manually: edit ~/.claude/settings.json \
                 and remove all hook entries containing 'hook-bridge.mjs', \
                 then delete ~/.opencovibe/hook-bridge.mjs"
            );
        }
    }
}

/// Path-injected pure function — testable with tempdir.
///
/// 1. Reads settings.json, removes any hook array entries containing "hook-bridge.mjs".
///    If an array becomes empty after filtering, the key is removed.
///    If the hooks object becomes empty, the "hooks" key is removed.
/// 2. Deletes the hook-bridge.mjs script file (only if settings cleanup succeeded or
///    settings didn't exist / had no bridge entries).
pub fn cleanup_hook_bridge_at(settings_path: &Path, bridge_path: &Path) {
    let mut can_delete_script = true;

    // ── Step 1: Clean settings.json ──
    if settings_path.exists() {
        match fs::read_to_string(settings_path) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(mut settings) => {
                    let changed = strip_bridge_entries(&mut settings);
                    if changed {
                        let json = serde_json::to_string_pretty(&settings)
                            .unwrap_or_else(|_| content.clone());
                        if let Err(e) = write_settings_file(settings_path, &json) {
                            can_delete_script = false;
                            log::warn!(
                                "[hooks/setup] failed to write back {}: {}. \
                                 Keeping hook-bridge.mjs to avoid dangling reference.",
                                settings_path.display(),
                                e
                            );
                        } else {
                            log::debug!(
                                "[hooks/setup] cleaned hook-bridge entries from {}",
                                settings_path.display()
                            );
                        }
                    }
                    // No change → settings already clean, can_delete_script stays true
                }
                Err(e) => {
                    can_delete_script = false;
                    log::warn!(
                        "[hooks/setup] failed to parse {}: {}. \
                         Keeping hook-bridge.mjs to avoid dangling reference.",
                        settings_path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                can_delete_script = false;
                log::warn!(
                    "[hooks/setup] failed to read {}: {}. \
                     Keeping hook-bridge.mjs to avoid dangling reference.",
                    settings_path.display(),
                    e
                );
            }
        }
    }
    // settings doesn't exist → can_delete_script stays true (no reference to worry about)

    // ── Step 2: Delete the bridge script ──
    if can_delete_script && bridge_path.exists() {
        if let Err(e) = fs::remove_file(bridge_path) {
            log::warn!(
                "[hooks/setup] failed to delete {}: {}",
                bridge_path.display(),
                e
            );
        } else {
            log::debug!(
                "[hooks/setup] deleted hook-bridge script at {}",
                bridge_path.display()
            );
        }
    }
}

/// Remove all hook array entries whose JSON serialization contains "hook-bridge.mjs".
/// Returns true if any entries were removed (i.e. settings needs to be written back).
fn strip_bridge_entries(settings: &mut serde_json::Value) -> bool {
    let hooks = match settings.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        Some(h) => h,
        None => return false,
    };

    let mut changed = false;
    let mut empty_keys: Vec<String> = Vec::new();

    for (key, value) in hooks.iter_mut() {
        if let Some(arr) = value.as_array_mut() {
            let before = arr.len();
            arr.retain(|entry| {
                !serde_json::to_string(entry)
                    .unwrap_or_default()
                    .contains("hook-bridge.mjs")
            });
            if arr.len() != before {
                changed = true;
            }
            if arr.is_empty() {
                empty_keys.push(key.clone());
            }
        }
    }

    for key in &empty_keys {
        hooks.remove(key);
        changed = true;
    }

    // If hooks object is now empty, remove it entirely
    if hooks.is_empty() {
        settings.as_object_mut().map(|obj| obj.remove("hooks"));
        // changed is already true if we removed entries
    }

    changed
}

/// Write settings.json.
fn write_settings_file(path: &Path, content: &str) -> std::io::Result<()> {
    fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create settings.json with given hooks config
    fn write_settings(dir: &Path, hooks_json: &str) -> PathBuf {
        let settings_path = dir.join("settings.json");
        let content = format!(r#"{{"hooks": {}}}"#, hooks_json);
        fs::write(&settings_path, content).unwrap();
        settings_path
    }

    /// Helper: create a dummy hook-bridge.mjs file
    fn write_bridge(dir: &Path) -> PathBuf {
        let bridge_path = dir.join("hook-bridge.mjs");
        fs::write(&bridge_path, "// bridge script").unwrap();
        bridge_path
    }

    // ── Happy path tests ──

    #[test]
    fn removes_bridge_entries_and_deletes_script() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = write_settings(
            tmp.path(),
            r#"{"PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "node /path/hook-bridge.mjs PreToolUse"}]}]}"#,
        );
        let bridge_path = write_bridge(tmp.path());

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        // Bridge entry removed from settings
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(
            content.get("hooks").is_none(),
            "hooks key should be removed when empty"
        );

        // Script deleted
        assert!(!bridge_path.exists(), "hook-bridge.mjs should be deleted");
    }

    #[test]
    fn preserves_user_custom_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = write_settings(
            tmp.path(),
            r#"{
                "PreToolUse": [
                    {"matcher": "*", "hooks": [{"type": "command", "command": "node /path/hook-bridge.mjs PreToolUse"}]},
                    {"matcher": "Write", "hooks": [{"type": "command", "command": "my-custom-hook.sh"}]}
                ],
                "Stop": [
                    {"matcher": "*", "hooks": [{"type": "command", "command": "my-stop-hook.sh"}]}
                ]
            }"#,
        );
        let bridge_path = write_bridge(tmp.path());

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        let hooks = content.get("hooks").expect("hooks should still exist");

        // Bridge entry removed, custom entry preserved
        let pre = hooks.get("PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert!(serde_json::to_string(&pre[0])
            .unwrap()
            .contains("my-custom-hook.sh"));

        // User-defined Stop hook untouched
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();
        assert_eq!(stop.len(), 1);

        // Script deleted (settings cleaned successfully)
        assert!(!bridge_path.exists());
    }

    #[test]
    fn empty_array_removes_key_empty_hooks_removes_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        // All entries are bridge entries → arrays become empty → keys removed → hooks removed
        let settings_path = write_settings(
            tmp.path(),
            r#"{
                "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "node /x/hook-bridge.mjs PreToolUse"}]}],
                "PostToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "node /x/hook-bridge.mjs PostToolUse"}]}]
            }"#,
        );
        let bridge_path = write_bridge(tmp.path());

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(
            content.get("hooks").is_none(),
            "hooks key should be removed entirely"
        );
        assert!(!bridge_path.exists());
    }

    #[test]
    fn settings_not_exist_still_deletes_script() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("nonexistent").join("settings.json");
        let bridge_path = write_bridge(tmp.path());

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        assert!(
            !bridge_path.exists(),
            "script should be deleted when settings doesn't exist"
        );
    }

    // ── Failure branch tests ──

    #[test]
    fn parse_failure_keeps_script() {
        let tmp = tempfile::tempdir().unwrap();
        let settings_path = tmp.path().join("settings.json");
        fs::write(&settings_path, "not valid json!!!").unwrap();
        let bridge_path = write_bridge(tmp.path());

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        // Settings unchanged
        assert_eq!(
            fs::read_to_string(&settings_path).unwrap(),
            "not valid json!!!"
        );
        // Script preserved
        assert!(
            bridge_path.exists(),
            "script should be kept when settings can't be parsed"
        );
    }

    #[test]
    fn write_failure_keeps_script() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        // Put settings in a subdirectory so we can make it read-only
        let sub = tmp.path().join("readonly");
        fs::create_dir_all(&sub).unwrap();
        let settings_path = write_settings(
            &sub,
            r#"{"PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "node /x/hook-bridge.mjs PreToolUse"}]}]}"#,
        );
        let original = fs::read_to_string(&settings_path).unwrap();
        let bridge_path = write_bridge(tmp.path());

        // Make the settings file read-only to trigger write failure
        fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o444)).unwrap();

        cleanup_hook_bridge_at(&settings_path, &bridge_path);

        // Restore permissions for cleanup
        fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o644)).unwrap();

        // Settings unchanged (write failed)
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
        // Script preserved
        assert!(
            bridge_path.exists(),
            "script should be kept when settings write fails"
        );
    }
}

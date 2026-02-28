use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;

/// Screenshot capture core logic (shared by global hotkey callback and IPC command).
#[cfg(target_os = "macos")]
fn do_capture(app: &AppHandle) {
    let path = std::env::temp_dir()
        .join(format!(
            "ocv-screenshot-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ))
        .to_string_lossy()
        .into_owned();
    log::debug!("[screenshot] starting capture: {}", path);

    let status = std::process::Command::new("screencapture")
        .args(["-i", &path])
        .status();

    match status {
        Ok(s) if s.success() && std::path::Path::new(&path).exists() => {
            log::debug!("[screenshot] capture succeeded, reading file");
            match std::fs::read(&path) {
                Ok(data) => {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    log::debug!(
                        "[screenshot] encoded {}KB, emitting event",
                        data.len() / 1024
                    );

                    // Show + focus window before emitting (Rust-side, not dependent on frontend listener)
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }

                    let filename =
                        format!("screenshot-{}.png", chrono::Local::now().format("%H%M%S"));
                    let _ = app.emit(
                        "screenshot-taken",
                        serde_json::json!({
                            "contentBase64": b64,
                            "mediaType": "image/png",
                            "filename": filename,
                        }),
                    );
                }
                Err(e) => log::warn!("[screenshot] read failed: {}", e),
            }
            let _ = std::fs::remove_file(&path);
        }
        Ok(_) => log::debug!("[screenshot] cancelled by user (ESC)"),
        Err(e) => log::warn!("[screenshot] screencapture failed: {}", e),
    }
}

/// Called by the global shortcut handler (from lib.rs with_handler callback).
pub fn handle_global_shortcut(app: &AppHandle) {
    log::debug!("[screenshot] global hotkey pressed");
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        std::thread::spawn(move || do_capture(&app));
    }
}

/// IPC: manually trigger screenshot (frontend button / non-macOS detection entry).
#[tauri::command]
pub async fn capture_screenshot(app: AppHandle) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[screenshot] not supported on this platform");
        return Err("Screenshot capture is only supported on macOS".into());
    }

    #[cfg(target_os = "macos")]
    {
        // screencapture blocks waiting for user selection, must spawn thread
        std::thread::spawn(move || do_capture(&app));
        Ok(())
    }
}

/// IPC: register/update/unregister global screenshot hotkey.
#[tauri::command]
pub fn update_screenshot_hotkey(app: AppHandle, hotkey: Option<String>) -> Result<(), String> {
    log::debug!("[screenshot] update_hotkey: {:?}", hotkey);
    unregister_all_screenshot_hotkeys(&app);
    if let Some(ref key) = hotkey {
        let tauri_key = convert_to_tauri_shortcut(key);
        register_screenshot_hotkey(&app, &tauri_key)?;
    }
    Ok(())
}

/// Called in setup(): read settings and register initial hotkey.
pub fn init_screenshot_hotkey(app: &AppHandle) {
    #[cfg(not(target_os = "macos"))]
    {
        log::debug!("[screenshot] skipping init (not macOS)");
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let settings = crate::storage::settings::get_user_settings();
        // SYNC: default "Cmd+Ctrl+S" also in src/lib/stores/keybindings.svelte.ts APP_DEFAULTS
        let resolved_key = settings
            .keybinding_overrides
            .iter()
            .find(|o| o.command == "app:screenshot")
            .map(|o| o.key.clone())
            .unwrap_or_else(|| "Cmd+Ctrl+S".to_string());

        if resolved_key.is_empty() || resolved_key == "disabled" {
            log::debug!("[screenshot] hotkey empty/disabled, skipping");
            return;
        }

        let tauri_key = convert_to_tauri_shortcut(&resolved_key);
        if let Err(e) = register_screenshot_hotkey(app, &tauri_key) {
            log::warn!("[screenshot] init hotkey failed: {}", e);
        }
    }
}

/// Convert app key format "Cmd+Ctrl+S" → Tauri shortcut format "super+ctrl+s".
fn convert_to_tauri_shortcut(key: &str) -> String {
    key.split('+')
        .map(|part| match part {
            "Cmd" => "super",
            "Ctrl" => "ctrl",
            "Alt" => "alt",
            "Shift" => "shift",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Register a global shortcut for screenshot capture.
/// Uses `register()` (not `on_shortcut`) — events dispatch to the global `with_handler` in lib.rs.
fn register_screenshot_hotkey(app: &AppHandle, shortcut_str: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    log::debug!("[screenshot] registering hotkey: {}", shortcut_str);

    let shortcut: tauri_plugin_global_shortcut::Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("invalid shortcut '{}': {}", shortcut_str, e))?;

    app.global_shortcut()
        .register(shortcut)
        .map_err(|e| format!("failed to register shortcut: {}", e))?;

    log::debug!("[screenshot] hotkey registered successfully");
    Ok(())
}

/// Unregister all screenshot global shortcuts.
fn unregister_all_screenshot_hotkeys(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    log::debug!("[screenshot] unregistering all hotkeys");
    let _ = app.global_shortcut().unregister_all();
}

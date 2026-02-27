//! Desktop notification helper — sends macOS notifications when the window is
//! hidden or unfocused (e.g. minimized to tray). Rate-limited to avoid spam.

use std::sync::atomic::{AtomicU64, Ordering};
use tauri::Manager;

static LAST_NOTIFY_MS: AtomicU64 = AtomicU64::new(0);
const NOTIFY_COOLDOWN_MS: u64 = 5000;

/// Send a macOS notification if the main window is hidden or unfocused.
/// Rate-limited to at most 1 notification per 5 seconds.
pub fn notify_if_background(app: &tauri::AppHandle, title: &str, body: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_NOTIFY_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < NOTIFY_COOLDOWN_MS {
        log::trace!("[notify] rate-limited, skipping: {}", title);
        return;
    }

    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(true);
        let focused = window.is_focused().unwrap_or(true);
        if !visible || !focused {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title(title).body(body).show();
            LAST_NOTIFY_MS.store(now, Ordering::Relaxed);
            log::debug!("[notify] sent: {} — {}", title, body);
        }
    }
}

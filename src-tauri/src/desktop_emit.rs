//! Desktop emission facade — lets the runtime core run with or without a Tauri desktop.
//!
//! The agent core, storage layer, and embedded web server are all Tauri-free; the only
//! thing they need from the desktop is pushing UI events into the webview. Historically
//! that was a hard `tauri::AppHandle` threaded through the broadcaster, the turn engine,
//! and the team watcher — which made it impossible to start the web server without
//! building a Tauri app (and therefore without initializing GTK on Linux).
//!
//! `DesktopEmit` makes "is there a desktop?" an explicit, single-point decision:
//! events are forwarded to the webview in desktop mode and dropped in `--headless` mode.
//! Persistence (`EventWriter`) and WebSocket fan-out (`EventBroadcaster`) are unaffected —
//! they never went through Tauri.

use serde::Serialize;

/// Where UI events should be delivered, if anywhere.
#[derive(Clone)]
pub enum DesktopEmit {
    /// Desktop mode — emit into the Tauri webview.
    Tauri(tauri::AppHandle),
    /// Headless server mode — no webview exists, so UI events are dropped.
    /// WebSocket clients still receive everything via `EventBroadcaster`.
    Headless,
}

impl DesktopEmit {
    /// Emit a Tauri event, or no-op when there is no desktop.
    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        if let Self::Tauri(app) = self {
            use tauri::Emitter;
            let _ = app.emit(event, payload);
        }
    }

    /// The underlying `AppHandle`, when running with a desktop.
    ///
    /// Callers that need desktop-only capabilities (window focus checks, native
    /// notifications) must handle `None` by skipping the feature.
    pub fn app(&self) -> Option<&tauri::AppHandle> {
        match self {
            Self::Tauri(app) => Some(app),
            Self::Headless => None,
        }
    }
}

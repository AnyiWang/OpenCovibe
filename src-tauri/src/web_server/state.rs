use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::agent::adapter::ActorSessionMap;
use crate::agent::control::CliInfoCache;
use crate::agent::spawn_locks::SpawnLocks;
use crate::agent::stream::ProcessMap;
use crate::desktop_emit::DesktopEmit;
use crate::storage::events::EventWriter;
use crate::web_server::broadcaster::{BroadcastEmitter, EventBroadcaster};
use crate::{
    EffectiveWebBind, EffectiveWebPort, SharedLiveToken, SharedTokenVersion, WebServerCancel,
    WebServerGeneration, WebServerHandle, WebServerLock, WebServerWarning, WsShutdownSender,
};
use tokio_util::sync::CancellationToken;

/// Session entry for cookie-based HTTP authentication
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub token_version: u64,
}

/// Aggregated application state shared between Tauri IPC and axum web server.
/// All fields are Arc-wrapped for cheap cloning.
#[derive(Clone)]
pub struct AppState {
    pub process_map: ProcessMap,
    pub sessions: ActorSessionMap,
    pub writer: Arc<EventWriter>,
    pub spawn_locks: SpawnLocks,
    pub cancel_token: CancellationToken,
    pub cli_info_cache: CliInfoCache,
    pub emitter: Arc<BroadcastEmitter>,
    pub broadcaster: EventBroadcaster,

    /// Authentication token for web server access (hot-swappable for rotation)
    pub token: Arc<tokio::sync::RwLock<String>>,
    /// Token version — incremented on token rotation to invalidate sessions
    pub token_version: Arc<std::sync::atomic::AtomicU64>,
    /// WS shutdown broadcast — token rotation triggers disconnect of all WS clients
    pub ws_shutdown: Arc<tokio::sync::broadcast::Sender<()>>,
    /// HTTP session store (cookie → session entry)
    pub http_sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
    /// Effective port after binding (may differ from config if port was busy)
    pub effective_port: Arc<std::sync::atomic::AtomicU16>,
    /// Bind address
    pub bind_addr: Arc<String>,
    /// Allowed origins for CORS
    pub allowed_origins: Option<Vec<String>>,
}

/// The set of process-wide handles the web server lifecycle needs.
///
/// In desktop mode these live in Tauri's managed-state container; in headless mode
/// there is no Tauri app, so they are created directly. Both paths converge here so
/// `web_server` never has to know which container it came from — this is what lets
/// the server start without building a Tauri app (and therefore without GTK).
#[derive(Clone)]
pub struct CoreState {
    pub process_map: ProcessMap,
    pub sessions: ActorSessionMap,
    pub spawn_locks: SpawnLocks,
    pub cli_info_cache: CliInfoCache,
    pub writer: Arc<EventWriter>,
    pub cancel_token: CancellationToken,
    pub broadcaster: EventBroadcaster,
    pub emitter: Arc<BroadcastEmitter>,
    pub token: SharedLiveToken,
    pub token_version: SharedTokenVersion,
    pub ws_shutdown: WsShutdownSender,
    pub effective_port: EffectiveWebPort,
    pub ws_lock: WebServerLock,
    /// Web-server-specific cancel token (distinct from the app-wide `cancel_token`,
    /// because restarting the server must not cancel running sessions).
    pub ws_cancel: WebServerCancel,
    pub ws_handle: WebServerHandle,
    pub generation: WebServerGeneration,
    pub effective_bind: EffectiveWebBind,
    pub warning: WebServerWarning,
}

impl CoreState {
    /// Adopt the handles already registered with a live Tauri app (desktop mode).
    /// Mirrors the `.manage(...)` list in `lib.rs`.
    pub fn from_app(app: &tauri::AppHandle) -> Self {
        use tauri::Manager;
        Self {
            process_map: app.state::<ProcessMap>().inner().clone(),
            sessions: app.state::<ActorSessionMap>().inner().clone(),
            spawn_locks: app.state::<SpawnLocks>().inner().clone(),
            cli_info_cache: app.state::<CliInfoCache>().inner().clone(),
            writer: app.state::<Arc<EventWriter>>().inner().clone(),
            cancel_token: app.state::<CancellationToken>().inner().clone(),
            broadcaster: app
                .try_state::<EventBroadcaster>()
                .map(|s| s.inner().clone())
                .unwrap_or_default(),
            emitter: app.state::<Arc<BroadcastEmitter>>().inner().clone(),
            token: app.state::<SharedLiveToken>().inner().clone(),
            token_version: app.state::<SharedTokenVersion>().inner().clone(),
            ws_shutdown: app.state::<WsShutdownSender>().inner().clone(),
            effective_port: app.state::<EffectiveWebPort>().inner().clone(),
            ws_lock: app.state::<WebServerLock>().inner().clone(),
            ws_cancel: app.state::<WebServerCancel>().inner().clone(),
            ws_handle: app.state::<WebServerHandle>().inner().clone(),
            generation: app.state::<WebServerGeneration>().inner().clone(),
            effective_bind: app.state::<EffectiveWebBind>().inner().clone(),
            warning: app.state::<WebServerWarning>().inner().clone(),
        }
    }

    /// Build a fresh, Tauri-free state set for `--headless` server mode.
    ///
    /// `token` is supplied by the caller (env var or generated + persisted) since
    /// headless installs have no desktop UI to display it in.
    pub fn headless(token: String) -> Self {
        use std::sync::atomic::{AtomicU16, AtomicU64};

        let writer = crate::storage::events::global_writer();
        let broadcaster = EventBroadcaster::new();
        let emitter = Arc::new(BroadcastEmitter::new(
            writer.clone(),
            DesktopEmit::Headless,
            broadcaster.clone(),
        ));
        let (ws_shutdown, _) = tokio::sync::broadcast::channel::<()>(1);

        Self {
            process_map: crate::agent::stream::new_process_map(),
            sessions: crate::agent::adapter::new_actor_session_map(),
            spawn_locks: SpawnLocks::new(),
            cli_info_cache: CliInfoCache::new(),
            writer,
            cancel_token: CancellationToken::new(),
            broadcaster,
            emitter,
            token: Arc::new(tokio::sync::RwLock::new(token)),
            token_version: Arc::new(AtomicU64::new(0)),
            ws_shutdown: Arc::new(ws_shutdown),
            effective_port: Arc::new(AtomicU16::new(0)),
            ws_lock: Arc::new(tokio::sync::Mutex::new(())),
            ws_cancel: Arc::new(tokio::sync::Mutex::new(CancellationToken::new())),
            ws_handle: Arc::new(tokio::sync::Mutex::new(None)),
            generation: WebServerGeneration(Arc::new(AtomicU64::new(0))),
            effective_bind: EffectiveWebBind(Arc::new(tokio::sync::RwLock::new(String::new()))),
            warning: WebServerWarning(Arc::new(tokio::sync::RwLock::new(None))),
        }
    }
}

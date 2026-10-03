pub mod agent;
pub mod commands;
pub mod desktop_emit;
pub mod hooks;
pub mod models;
pub mod pricing;
pub mod process_ext;
pub mod storage;
pub mod web_server;

use agent::adapter::new_actor_session_map;
use agent::codex_control::CodexInfoCache;
use agent::control::CliInfoCache;
use agent::spawn_locks::SpawnLocks;
use agent::stream::new_process_map;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use storage::events::EventWriter;
use tauri::tray::TrayIconEvent;
use tauri::Manager;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Effective web server port (may differ from configured port if busy)
pub type EffectiveWebPort = Arc<AtomicU16>;
/// Web-server-specific cancel token for restart support
pub type WebServerCancel = Arc<tokio::sync::Mutex<CancellationToken>>;
/// Token version — shared between IPC and web server for rotation detection
pub type SharedTokenVersion = Arc<AtomicU64>;
/// WS shutdown broadcast — token rotation triggers disconnect of all WS clients
pub type WsShutdownSender = Arc<broadcast::Sender<()>>;
/// Live token — hot-swappable via RwLock for immediate login/logout on rotation
pub type SharedLiveToken = Arc<tokio::sync::RwLock<String>>;
/// Mutex to serialize web server start/stop operations
pub type WebServerLock = Arc<tokio::sync::Mutex<()>>;
/// JoinHandle for the serve task — await during stop to ensure port release
pub type WebServerHandle = Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>;
/// Generation counter — each spawn_server increments; stale tasks check before cleanup.
/// Newtype to avoid Tauri manage() collision with SharedTokenVersion (both Arc<AtomicU64>).
#[derive(Clone)]
pub struct WebServerGeneration(pub Arc<AtomicU64>);
/// Effective bind address — reflects actual running state (not settings).
/// Newtype to avoid Tauri manage() collision with SharedLiveToken (both Arc<RwLock<String>>).
#[derive(Clone)]
pub struct EffectiveWebBind(pub Arc<tokio::sync::RwLock<String>>);
/// Startup warning — populated when origins are degraded or other non-fatal startup issues.
#[derive(Clone)]
pub struct WebServerWarning(pub Arc<tokio::sync::RwLock<Option<String>>>);

/// One-shot gate to prevent concurrent shutdown tasks.
/// CAS ensures only the first caller proceeds; subsequent quit/close events are no-ops.
pub struct ShutdownGate(AtomicBool);

impl Default for ShutdownGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownGate {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    /// Returns `true` if this call entered the gate (first caller wins).
    pub fn try_enter(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

pub fn run() {
    init_logging();

    match parse_cli_args(std::env::args().skip(1)) {
        Ok(CliMode::Help) => print!("{}", USAGE),
        Ok(CliMode::Headless(config)) => run_headless(config),
        Ok(CliMode::Desktop) => run_desktop(),
        Err(message) => {
            eprintln!("error: {message}\n\n{}", USAGE);
            std::process::exit(2);
        }
    }
}

/// Initialize logging — our crate at debug level by default.
/// Override with RUST_LOG env var, e.g. RUST_LOG=warn cargo tauri dev
fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("opencovibe_desktop_lib=debug,warn"),
    )
    .format_timestamp_millis()
    .init();
}

/// Desktop mode: full Tauri app with a window, tray, and the embedded web server.
///
/// NOTE: this path calls `tauri::Builder::build()`, which on Linux unconditionally
/// initializes GTK. It therefore cannot run without a display — use `--headless`.
fn run_desktop() {
    log::info!("OpenCovibe Desktop starting");

    // All storage modules use process-local writer locks. Hold the OS lock before any startup
    // reconciliation touches data so a second backend cannot allocate duplicate event sequences
    // or clean a history generation that this process is serving.
    let data_dir_lock = storage::DataDirLock::acquire().unwrap_or_else(|error| {
        log::error!("[app] data writer lock failed: {error}");
        panic!("{error}");
    });

    // Set up Windows Job Object so child processes are killed on crash/force-quit.
    // No-op on non-Windows.
    process_ext::setup_job_kill_on_close();

    // Reconcile orphaned runs on startup
    storage::runs::reconcile_orphaned_runs();

    // Clean up legacy hook-bridge (removed: was redundant with stream-json mode)
    hooks::setup::cleanup_hook_bridge();

    // Global cancellation token — shared with all session actors for graceful shutdown
    let cancel_token = CancellationToken::new();
    let cancel_for_exit = cancel_token.clone();

    // Shared flag: true if system tray was successfully created
    let tray_ok = Arc::new(AtomicBool::new(false));
    let tray_ok_for_event = tray_ok.clone();

    // Web server shared state
    let ws_shutdown_sender: WsShutdownSender = Arc::new(broadcast::channel::<()>(1).0);
    let shared_token_version: SharedTokenVersion = Arc::new(AtomicU64::new(0));
    let shared_live_token: SharedLiveToken = {
        use rand::Rng;
        let token: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        log::debug!("[app] ephemeral web token generated (masked)");
        Arc::new(tokio::sync::RwLock::new(token))
    };
    let effective_web_port: EffectiveWebPort = Arc::new(AtomicU16::new(0));
    let ws_cancel: WebServerCancel = Arc::new(tokio::sync::Mutex::new(CancellationToken::new()));
    let ws_lock: WebServerLock = Arc::new(tokio::sync::Mutex::new(()));
    let ws_handle: WebServerHandle = Arc::new(tokio::sync::Mutex::new(None));
    let ws_generation = WebServerGeneration(Arc::new(AtomicU64::new(0)));
    let ws_effective_bind = EffectiveWebBind(Arc::new(tokio::sync::RwLock::new(String::new())));
    let ws_warning = WebServerWarning(Arc::new(tokio::sync::RwLock::new(None)));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .manage(new_process_map())
        .manage(new_actor_session_map())
        .manage(CliInfoCache::new())
        .manage(CodexInfoCache::new())
        // Managed writer = the process-wide singleton, so the free-function
        // append_event path shares its per-run locks + seq source (audit #1).
        .manage(crate::storage::events::global_writer())
        .manage(SpawnLocks::new())
        .manage(ShutdownGate::new())
        .manage(data_dir_lock)
        .manage(cancel_token)
        .manage(ws_shutdown_sender)
        .manage(shared_token_version)
        .manage(shared_live_token)
        .manage(effective_web_port)
        .manage(ws_cancel)
        .manage(ws_lock)
        .manage(ws_handle)
        .manage(ws_generation)
        .manage(ws_effective_bind)
        .manage(ws_warning)
        // NOTE: Currently ~60 IPC commands. If approaching 80+, consider grouping
        // into Tauri command modules or using a single dispatch command with typed payloads.
        .invoke_handler(tauri::generate_handler![
            commands::runs::list_runs,
            commands::runs::get_run,
            commands::runs::start_run,
            commands::runs::stop_run,
            commands::runs::update_run_model,
            commands::runs::rename_run,
            commands::runs::soft_delete_runs,
            commands::runs::search_prompts,
            commands::history::search_runs,
            commands::history::get_run_files,
            commands::runs::add_prompt_favorite,
            commands::runs::remove_prompt_favorite,
            commands::runs::update_prompt_favorite_tags,
            commands::runs::update_prompt_favorite_note,
            commands::runs::list_prompt_favorites,
            commands::runs::list_prompt_tags,
            commands::chat::send_chat_message,
            commands::events::get_run_events,
            commands::artifacts::get_run_artifacts,
            commands::settings::get_user_settings,
            commands::settings::update_user_settings,
            commands::settings::get_agent_settings,
            commands::settings::update_agent_settings,
            commands::fs::list_directory,
            commands::fs::check_is_directory,
            commands::fs::read_file_base64,
            commands::remote_fs::list_remote_directory,
            commands::remote_fs::resolve_remote_home,
            commands::git::get_git_summary,
            commands::git::get_git_branch,
            commands::git::get_git_diff,
            commands::git::get_git_status,
            commands::export::export_conversation,
            commands::export::write_html_export,
            commands::files::agents_md_exists,
            commands::files::read_text_file,
            commands::files::stat_text_file,
            commands::files::write_text_file,
            commands::files::read_task_output,
            commands::files::list_memory_files,
            commands::stats::get_usage_overview,
            commands::stats::get_global_usage_overview,
            commands::stats::clear_usage_cache,
            commands::stats::get_heatmap_daily,
            commands::stats::get_changelog,
            commands::diagnostics::check_agent_cli,
            commands::diagnostics::check_codex_auth,
            commands::diagnostics::run_codex_doctor,
            commands::diagnostics::test_remote_host,
            commands::diagnostics::get_cli_dist_tags,
            commands::diagnostics::check_project_init,
            commands::diagnostics::check_ssh_key,
            commands::diagnostics::generate_ssh_key,
            commands::diagnostics::run_diagnostics,
            commands::diagnostics::detect_local_proxy,
            commands::diagnostics::test_api_connectivity,
            commands::session::start_session,
            commands::session::send_session_message,
            commands::session::stop_session,
            commands::session::send_session_control,
            commands::session::broadcast_mcp_toggle,
            commands::session::get_bus_events,
            commands::session::get_bus_events_page,
            commands::history_pages::get_history_summary,
            commands::history_pages::get_history_page,
            commands::history_pages::get_history_content_chunk,
            commands::history_pages::get_subhistory_page,
            commands::session::fork_session,
            commands::session::side_question,
            commands::session::start_ralph_loop,
            commands::session::cancel_ralph_loop,
            commands::session::approve_session_tool,
            commands::session::cancel_control_request,
            commands::session::respond_permission,
            commands::session::respond_hook_callback,
            commands::session::respond_elicitation,
            commands::session::respond_user_input,
            commands::control::get_cli_info,
            commands::control::get_codex_models,
            commands::teams::list_teams,
            commands::teams::get_team_config,
            commands::teams::list_team_tasks,
            commands::teams::get_team_task,
            commands::teams::get_team_inbox,
            commands::teams::get_all_team_inboxes,
            commands::teams::delete_team,
            commands::plugins::list_marketplaces,
            commands::plugins::list_marketplace_plugins,
            commands::plugins::list_standalone_skills,
            commands::plugins::list_project_commands,
            commands::plugins::get_skill_content,
            commands::plugins::list_installed_plugins,
            commands::plugins::install_plugin,
            commands::plugins::uninstall_plugin,
            commands::plugins::enable_plugin,
            commands::plugins::disable_plugin,
            commands::plugins::update_plugin,
            commands::plugins::add_marketplace,
            commands::plugins::remove_marketplace,
            commands::plugins::update_marketplace,
            commands::plugins::create_skill,
            commands::plugins::update_skill,
            commands::plugins::delete_skill,
            commands::plugins::list_codex_skills,
            commands::plugins::create_codex_skill,
            commands::plugins::delete_codex_skill,
            commands::plugins::toggle_codex_skill,
            commands::plugins::list_codex_installed_plugins,
            commands::plugins::toggle_codex_plugin,
            commands::plugins::check_community_health,
            commands::plugins::search_community_skills,
            commands::plugins::get_community_skill_detail,
            commands::plugins::install_community_skill,
            commands::agents::list_agents,
            commands::agents::read_agent_file,
            commands::agents::create_agent_file,
            commands::agents::update_agent_file,
            commands::agents::delete_agent_file,
            commands::agents::list_codex_agents,
            commands::clipboard::get_clipboard_files,
            commands::clipboard::read_clipboard_file,
            commands::clipboard::save_temp_attachment,
            commands::mcp::list_configured_mcp_servers,
            commands::mcp::add_mcp_server,
            commands::mcp::remove_mcp_server,
            commands::mcp::toggle_mcp_server_config,
            commands::mcp::get_disabled_mcp_servers,
            commands::mcp::check_mcp_registry_health,
            commands::mcp::search_mcp_registry,
            commands::mcp::list_codex_mcp_servers,
            commands::mcp::add_codex_mcp_server,
            commands::mcp::remove_codex_mcp_server,
            commands::cli_config::get_cli_config,
            commands::cli_config::get_project_cli_config,
            commands::cli_config::update_cli_config,
            commands::cli_config::get_codex_config,
            commands::cli_config::get_project_codex_config,
            commands::cli_config::update_codex_config,
            commands::cli_config::set_codex_feature,
            commands::cli_config::get_codex_hooks,
            commands::cli_config::update_codex_hooks,
            commands::cli_settings::get_cli_permissions,
            commands::cli_settings::update_cli_permissions,
            commands::onboarding::check_auth_status,
            commands::onboarding::detect_install_methods,
            commands::onboarding::run_claude_login,
            commands::onboarding::run_codex_login,
            commands::onboarding::run_codex_logout,
            commands::onboarding::get_auth_overview,
            commands::onboarding::set_cli_api_key,
            commands::onboarding::remove_cli_api_key,
            commands::screenshot::capture_screenshot,
            commands::screenshot::update_screenshot_hotkey,
            commands::cli_sync::discover_cli_sessions,
            commands::cli_sync::import_cli_session,
            commands::cli_sync::sync_cli_session,
            commands::updates::check_for_updates,
            commands::web_server::get_web_server_status,
            commands::web_server::get_web_server_token,
            commands::web_server::regenerate_web_server_token,
            commands::web_server::restart_web_server,
            commands::web_server::get_local_ip,
            commands::preview::open_preview_window,
            commands::preview::close_preview_window,
        ])
        .setup(move |app| {
            // Recover the user's real shell PATH off the hot path, so CLI detection works
            // when the app is launched from Finder/Dock (which provides only a minimal PATH).
            // Spawning a shell can take a moment; do it on a background thread so startup
            // isn't blocked and the cache is warm before the user reaches onboarding.
            std::thread::spawn(crate::agent::claude_stream::prime_path_cache);

            // Set up broadcast emitter (requires AppHandle, so must be in setup)
            let broadcaster = web_server::broadcaster::EventBroadcaster::new();
            let writer = app.state::<Arc<EventWriter>>().inner().clone();
            let emitter = Arc::new(web_server::broadcaster::BroadcastEmitter::new(
                writer,
                desktop_emit::DesktopEmit::Tauri(app.handle().clone()),
                broadcaster.clone(),
            ));
            app.manage(broadcaster);
            app.manage(emitter);

            // Start web server (non-blocking, spawns async task)
            let core = web_server::state::CoreState::from_app(app.handle());
            tauri::async_runtime::spawn(async move {
                match web_server::start_server(&core, web_server::StartOptions::from_settings())
                    .await
                {
                    Ok(true) => log::debug!("[app] web server started"),
                    Ok(false) => log::debug!("[app] web server disabled"),
                    Err(e) => log::error!("[app] web server failed to start: {}", e),
                }
            });

            // Start team file watcher for ~/.claude/teams/ and ~/.claude/tasks/
            let cancel = app.state::<CancellationToken>().inner().clone();
            hooks::team_watcher::start_team_watcher(
                desktop_emit::DesktopEmit::Tauri(app.handle().clone()),
                cancel,
            );

            // System tray — hide-to-tray on close, left-click to show
            // Non-fatal: if tray library is unavailable (e.g. some Linux desktops),
            // the app still works but window close = quit instead of hide-to-tray.
            match setup_tray(app) {
                Ok(_) => {
                    tray_ok.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    log::warn!("[app] tray unavailable: {e}, window close = quit");
                }
            }

            // Global shortcut plugin — must be registered inside setup() with a handler
            // so the event dispatch loop is properly initialized
            {
                use tauri_plugin_global_shortcut::ShortcutState;
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(|app, _shortcut, event| {
                            if event.state == ShortcutState::Pressed {
                                commands::screenshot::handle_global_shortcut(app);
                            }
                        })
                        .build(),
                )?;
            }

            // Register screenshot hotkey from settings (must come after plugin init)
            commands::screenshot::init_screenshot_hotkey(app.handle());

            Ok(())
        })
        .on_window_event(move |window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // Only intercept close for the main window
                    if window.label() != "main" {
                        return;
                    }
                    api.prevent_close(); // always prevent default close
                    if tray_ok_for_event.load(Ordering::Relaxed) {
                        // Hide to tray instead of quitting
                        let _ = window.hide();
                        log::debug!("[app] window hidden to tray");
                    } else {
                        // No tray — graceful shutdown
                        log::debug!("[app] tray unavailable, starting graceful shutdown");
                        let app = window.app_handle().clone();
                        if let Some(gate) = app.try_state::<ShutdownGate>() {
                            if !gate.try_enter() {
                                return; // shutdown already in progress
                            }
                        }
                        if let Some(ct) = app.try_state::<CancellationToken>() {
                            ct.cancel();
                        }
                        tauri::async_runtime::spawn(async move {
                            graceful_shutdown_actors(&app).await;
                            app.exit(0);
                        });
                    }
                }
                tauri::WindowEvent::Destroyed if window.label() == "main" => {
                    // Safety fallback: cancel actors if main window is truly destroyed (e.g. app.exit()).
                    // Skip for secondary windows (e.g. preview) — destroying them must not shut down the app.
                    cancel_for_exit.cancel();
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // macOS: clicking the dock icon when all windows are hidden should reopen the window
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                show_main_window(app_handle);
                log::debug!("[app] reopened window from dock click");
            }
        }

        let _ = (app_handle, event); // suppress unused warnings on non-macOS
    });
}

/// Restore the main window: unminimize if needed, then show and focus.
fn show_main_window(handle: &impl tauri::Manager<tauri::Wry>) {
    if let Some(w) = handle.get_webview_window("main") {
        if w.is_minimized().unwrap_or(false) {
            let _ = w.unminimize();
        }
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Create system tray with Show/Quit menu. Left-click shows the window.
fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder};

    let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &separator, &quit])?;

    let tray_icon_bytes = include_bytes!("../icons/tray-icon.png");
    let tray_img =
        tauri::image::Image::from_bytes(tray_icon_bytes).expect("failed to load tray icon");

    TrayIconBuilder::new()
        .icon(tray_img)
        .icon_as_template(true)
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => {
                show_main_window(app);
            }
            "quit" => {
                if let Some(gate) = app.try_state::<ShutdownGate>() {
                    if !gate.try_enter() {
                        return; // shutdown already in progress
                    }
                }
                if let Some(ct) = app.try_state::<CancellationToken>() {
                    ct.cancel();
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    graceful_shutdown_actors(&app).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    log::debug!("[app] system tray created");
    Ok(())
}

/// Graceful shutdown: wait for actors to self-clean, then force-kill remaining processes.
///
/// Two-phase approach:
/// - Phase 1: Wait up to 3s for actors to exit (cancel token already fired → handle_stop → kill+wait).
/// - Phase 2: Drain remaining actors, try_send Stop, join with 2s timeout, abort if stuck.
/// - Then drain ProcessMap (stream processes).
async fn graceful_shutdown_actors(app: &tauri::AppHandle) {
    use crate::agent::adapter::ActorSessionMap;
    use crate::agent::stream::ProcessMap;

    let sessions = app
        .try_state::<ActorSessionMap>()
        .map(|s| s.inner().clone());
    let processes = app.try_state::<ProcessMap>().map(|s| s.inner().clone());
    graceful_shutdown_core(sessions.as_ref(), processes.as_ref()).await;
}

/// Container-agnostic shutdown — shared by the desktop app and `--headless` mode,
/// which own their session/process maps differently.
async fn graceful_shutdown_core(
    sessions: Option<&crate::agent::adapter::ActorSessionMap>,
    process_map: Option<&crate::agent::stream::ProcessMap>,
) {
    use crate::agent::session_actor::ActorCommand;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);

    // ── Phase 1: Wait for actors to self-cleanup (cancel already fired) ──
    if let Some(sessions) = sessions {
        loop {
            let count = sessions.lock().await.len();
            if count == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                log::warn!(
                    "[app] graceful shutdown: {} actors still alive, force stopping",
                    count
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // ── Phase 2: Force-stop remaining actors ──
        let remaining: Vec<_> = {
            let mut map = sessions.lock().await;
            map.drain().collect()
        };
        for (run_id, handle) in remaining {
            log::debug!("[app] force stopping actor: {}", run_id);
            // try_send avoids blocking if mailbox is full (bounded channel, 64 slots)
            let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
            let _ = handle
                .cmd_tx
                .try_send(ActorCommand::Stop { reply: reply_tx });
            // Get AbortHandle before consuming JoinHandle in timeout
            let abort = handle.join_handle.abort_handle();
            match tokio::time::timeout(std::time::Duration::from_secs(2), handle.join_handle).await
            {
                Ok(Ok(())) => {
                    log::debug!("[app] actor {} exited cleanly", run_id);
                }
                Ok(Err(e)) => {
                    log::warn!("[app] actor {} join error: {}", run_id, e);
                }
                Err(_) => {
                    log::warn!("[app] actor {} did not exit in 2s, aborting task", run_id);
                    abort.abort();
                }
            }
        }
    }

    // ── Kill remaining stream processes ──
    // ProcessMap lock is only held briefly (run_agent/stop_process do remove-then-await),
    // but we keep a timeout as a defensive fallback.
    if let Some(process_map) = process_map {
        let to_kill = match tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let mut map = process_map.lock().await;
            map.drain().collect::<Vec<_>>()
        })
        .await
        {
            Ok(vec) => vec,
            Err(_) => {
                log::warn!(
                    "[app] graceful shutdown: ProcessMap lock timeout, \
                     skipping (kill_on_drop / Job Object may handle)"
                );
                Vec::new()
            }
        };
        for (run_id, mut child) in to_kill {
            log::debug!("[app] graceful shutdown: killing stream process {}", run_id);
            let _ = child.kill().await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
        }
    }

    log::debug!("[app] graceful shutdown complete");
}

// ─────────────────────────────────────────────────────────────────────────────
// Command line
// ─────────────────────────────────────────────────────────────────────────────

const USAGE: &str = "\
OpenCovibe — local-first desktop app for AI-assisted vibe coding

USAGE:
    OpenCovibe [OPTIONS]

OPTIONS:
    --headless, --server    Run as a server with no GUI and no GTK initialization.
                            The full web UI is served over HTTP for browser access.
    --port <PORT>           Override the web server port (1024-65535). Requires --headless.
    --bind <ADDR>           Override the bind address (127.0.0.1, 0.0.0.0, ::1, ::).
                            Requires --headless.
    -h, --help              Print this help.

ENVIRONMENT:
    OPENCOVIBE_WEB_TOKEN    Web access token. When unset, a token is generated on
                            first run, stored in the data directory as `web_token`
                            (mode 0600), and reused on subsequent starts.
";

/// Settings only meaningful for `--headless`.
struct HeadlessConfig {
    port: Option<u16>,
    bind: Option<String>,
}

enum CliMode {
    Desktop,
    Headless(HeadlessConfig),
    Help,
}

/// Parse command line arguments. Kept dependency-free on purpose — this is the
/// only argument surface in the binary.
fn parse_cli_args<I: Iterator<Item = String>>(args: I) -> Result<CliMode, String> {
    let mut headless = false;
    let mut port: Option<u16> = None;
    let mut bind: Option<String> = None;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--headless" | "--server" => headless = true,
            "-h" | "--help" => return Ok(CliMode::Help),
            "--port" => {
                let raw = args.next().ok_or("--port requires a value")?;
                let parsed: u16 = raw
                    .parse()
                    .map_err(|_| format!("invalid --port value: {raw}"))?;
                port = Some(parsed);
            }
            "--bind" => {
                bind = Some(args.next().ok_or("--bind requires a value")?);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if !headless && (port.is_some() || bind.is_some()) {
        return Err("--port/--bind are only valid together with --headless".into());
    }

    Ok(if headless {
        CliMode::Headless(HeadlessConfig { port, bind })
    } else {
        CliMode::Desktop
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Headless server mode
// ─────────────────────────────────────────────────────────────────────────────

/// Headless mode: run the agent core and the embedded web server without ever
/// constructing a Tauri app.
///
/// `tauri::Builder::build()` reaches `tao::EventLoop::new()` → `gtk::init()`, and tao
/// initializes GTK unconditionally on Linux — so there is no way to "skip the window"
/// from inside Tauri. Avoiding `build()` entirely is what makes this usable on a
/// display-less box. Everything below reuses the same storage, agent, and web-server
/// code the desktop app runs; only the event sink differs (`DesktopEmit::Headless`).
fn run_headless(config: HeadlessConfig) -> ! {
    log::info!("OpenCovibe headless server starting");

    // Same OS-level writer lock as the desktop app — prevents a desktop instance and a
    // headless instance from allocating duplicate event sequences over the same data.
    let _data_dir_lock = storage::DataDirLock::acquire().unwrap_or_else(|error| {
        log::error!("[headless] data writer lock failed: {error}");
        eprintln!("error: {error}");
        std::process::exit(1);
    });

    process_ext::setup_job_kill_on_close();
    storage::runs::reconcile_orphaned_runs();
    hooks::setup::cleanup_hook_bridge();

    let token = match resolve_headless_token() {
        Ok(token) => token,
        Err(error) => {
            log::error!("[headless] {error}");
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let exit_code = runtime.block_on(async move {
        let core = web_server::state::CoreState::headless(token.clone());

        hooks::team_watcher::start_team_watcher(
            desktop_emit::DesktopEmit::Headless,
            core.cancel_token.clone(),
        );

        match web_server::start_server(
            &core,
            web_server::StartOptions::forced(config.port, config.bind.clone()),
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                log::error!("[headless] web server did not start");
                return 1;
            }
            Err(error) => {
                log::error!("[headless] web server failed to start: {error}");
                eprintln!("error: {error}");
                return 1;
            }
        }

        let port = core.effective_port.load(Ordering::Relaxed);
        let bind = core.effective_bind.0.read().await.clone();
        let login_url = login_url_for(&bind, port, &token);

        log::info!("[headless] serving on http://{}:{}", bind, port);
        log::info!("[headless] sign in here: {}", login_url);
        // Also print to stdout so the URL is visible regardless of RUST_LOG filtering.
        println!("OpenCovibe is running. Sign in: {login_url}");

        wait_for_shutdown_signal().await;
        log::info!("[headless] shutdown signal received");

        core.cancel_token.cancel();
        graceful_shutdown_core(Some(&core.sessions), Some(&core.process_map)).await;
        0
    });

    std::process::exit(exit_code);
}

/// Build the auto-login URL. `GET /login?token=` exchanges the token for a session
/// cookie server-side, so this link can be opened directly in a browser.
fn login_url_for(bind: &str, port: u16, token: &str) -> String {
    // A wildcard bind isn't dialable — point the user at loopback instead.
    let host = match bind {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" | "[::]" => "[::1]".to_string(),
        other if other.contains(':') => format!("[{other}]"), // bare IPv6 literal
        other => other.to_string(),
    };
    format!("http://{host}:{port}/login?token={token}")
}

/// Resolve the web token: env var first, then a previously persisted token,
/// otherwise generate one and persist it with owner-only permissions.
fn resolve_headless_token() -> Result<String, String> {
    if let Ok(raw) = std::env::var("OPENCOVIBE_WEB_TOKEN") {
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Err("OPENCOVIBE_WEB_TOKEN is set but empty".into());
        }
        log::info!("[headless] using token from OPENCOVIBE_WEB_TOKEN");
        return Ok(token);
    }

    let path = storage::data_dir().join("web_token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim().to_string();
        if !token.is_empty() {
            log::info!("[headless] reusing token from {}", path.display());
            return Ok(token);
        }
    }

    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    persist_token(&path, &token)?;
    log::info!("[headless] generated token → {}", path.display());
    Ok(token)
}

/// Write the token with owner-only permissions. On Unix the file is created 0600;
/// on Windows it inherits the user profile ACL, which is the same trust level as the
/// settings file next to it.
fn persist_token(path: &std::path::Path, token: &str) -> Result<(), String> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .map_err(|e| format!("cannot write token file {}: {e}", path.display()))?;
    writeln!(file, "{token}")
        .map_err(|e| format!("cannot write token file {}: {e}", path.display()))?;
    Ok(())
}

/// Block until SIGINT or (on Unix) SIGTERM.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => log::debug!("[headless] SIGINT received"),
                    _ = terminate.recv() => log::debug!("[headless] SIGTERM received"),
                }
            }
            Err(e) => {
                log::warn!("[headless] cannot listen for SIGTERM ({e}), waiting for SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

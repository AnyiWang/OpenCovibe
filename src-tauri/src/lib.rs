pub mod agent;
pub mod commands;
pub mod hooks;
pub mod models;
pub mod pricing;
pub mod process_ext;
pub mod storage;

use agent::adapter::new_actor_session_map;
use agent::control::CliInfoCache;
use agent::pty::new_pty_map;
use agent::spawn_locks::SpawnLocks;
use agent::stream::new_process_map;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use storage::events::EventWriter;
use tauri::tray::TrayIconEvent;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

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
    // Initialize logging — our crate at debug level by default
    // Override with RUST_LOG env var, e.g. RUST_LOG=warn cargo tauri dev
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("opencovibe_desktop_lib=debug,warn"),
    )
    .format_timestamp_millis()
    .init();

    log::info!("OpenCovibe Desktop starting");

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

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .manage(new_process_map())
        .manage(new_pty_map())
        .manage(new_actor_session_map())
        .manage(CliInfoCache::new())
        .manage(Arc::new(EventWriter::new()))
        .manage(SpawnLocks::new())
        .manage(ShutdownGate::new())
        .manage(cancel_token)
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
            commands::git::get_git_summary,
            commands::git::get_git_branch,
            commands::git::get_git_diff,
            commands::git::get_git_status,
            commands::export::export_conversation,
            commands::files::read_text_file,
            commands::files::write_text_file,
            commands::files::read_task_output,
            commands::files::list_memory_files,
            commands::stats::get_usage_overview,
            commands::stats::get_global_usage_overview,
            commands::stats::clear_usage_cache,
            commands::stats::get_heatmap_daily,
            commands::stats::get_changelog,
            commands::diagnostics::check_agent_cli,
            commands::diagnostics::test_remote_host,
            commands::diagnostics::get_cli_dist_tags,
            commands::diagnostics::check_project_init,
            commands::diagnostics::check_ssh_key,
            commands::diagnostics::generate_ssh_key,
            commands::diagnostics::run_diagnostics,
            commands::diagnostics::detect_local_proxy,
            commands::pty::spawn_pty,
            commands::pty::write_pty,
            commands::pty::resize_pty,
            commands::pty::close_pty,
            commands::session::start_session,
            commands::session::send_session_message,
            commands::session::stop_session,
            commands::session::send_session_control,
            commands::session::get_bus_events,
            commands::session::fork_session,
            commands::session::approve_session_tool,
            commands::session::cancel_control_request,
            commands::session::respond_permission,
            commands::session::respond_hook_callback,
            commands::control::get_cli_info,
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
            commands::plugins::check_community_health,
            commands::plugins::search_community_skills,
            commands::plugins::get_community_skill_detail,
            commands::plugins::install_community_skill,
            commands::agents::list_agents,
            commands::agents::read_agent_file,
            commands::agents::create_agent_file,
            commands::agents::update_agent_file,
            commands::agents::delete_agent_file,
            commands::clipboard::get_clipboard_files,
            commands::clipboard::read_clipboard_file,
            commands::clipboard::save_temp_attachment,
            commands::mcp::list_configured_mcp_servers,
            commands::mcp::add_mcp_server,
            commands::mcp::remove_mcp_server,
            commands::mcp::toggle_mcp_server_config,
            commands::mcp::check_mcp_registry_health,
            commands::mcp::search_mcp_registry,
            commands::cli_config::get_cli_config,
            commands::cli_config::get_project_cli_config,
            commands::cli_config::update_cli_config,
            commands::onboarding::check_auth_status,
            commands::onboarding::detect_install_methods,
            commands::onboarding::run_claude_login,
            commands::onboarding::get_auth_overview,
            commands::onboarding::set_cli_api_key,
            commands::onboarding::remove_cli_api_key,
            commands::screenshot::capture_screenshot,
            commands::screenshot::update_screenshot_hotkey,
            commands::cli_sync::discover_cli_sessions,
            commands::cli_sync::import_cli_session,
            commands::cli_sync::sync_cli_session,
            commands::updates::check_for_updates,
        ])
        .setup(move |app| {
            // Start team file watcher for ~/.claude/teams/ and ~/.claude/tasks/
            let cancel = app.state::<CancellationToken>().inner().clone();
            hooks::team_watcher::start_team_watcher(app.handle().clone(), cancel);

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
                tauri::WindowEvent::Destroyed => {
                    // Safety fallback: cancel actors if window is truly destroyed (e.g. app.exit())
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
/// - Then drain ProcessMap (stream processes) and PtyMap (terminal processes).
async fn graceful_shutdown_actors(app: &tauri::AppHandle) {
    use crate::agent::adapter::ActorSessionMap;
    use crate::agent::pty::PtyMap;
    use crate::agent::session_actor::ActorCommand;
    use crate::agent::stream::ProcessMap;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);

    // ── Phase 1: Wait for actors to self-cleanup (cancel already fired) ──
    if let Some(sessions) = app.try_state::<ActorSessionMap>() {
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
    if let Some(process_map) = app.try_state::<ProcessMap>() {
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

    // ── Kill remaining PTY processes (std sync Mutex) ──
    // Drain to Vec, release lock, then kill. Don't wait — portable_pty::Child::wait()
    // is synchronous blocking; spawn_blocking during runtime shutdown can hang.
    if let Some(pty_map) = app.try_state::<PtyMap>() {
        let to_kill: Vec<_> = match pty_map.lock() {
            Ok(mut map) => map.drain().collect(),
            Err(e) => {
                log::warn!("[app] graceful shutdown: failed to lock PTY map: {}", e);
                Vec::new()
            }
        };
        for (run_id, mut session) in to_kill {
            log::debug!("[app] graceful shutdown: killing PTY process {}", run_id);
            let _ = session.child.kill();
            // No wait — synchronous blocking wait is unsafe during shutdown.
            // Process exit / Job Object handles cleanup.
        }
    }

    log::debug!("[app] graceful shutdown complete");
}

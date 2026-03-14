use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::models::BusEvent;
use crate::storage::events::EventWriter;
use tauri::Emitter;

/// Message envelope for broadcast channels.
#[derive(Debug, Clone)]
pub struct BroadcastMsg {
    /// Event name (e.g. "bus-event", "pty-output", "chat-delta")
    pub event_name: String,
    /// Serialized payload
    pub payload: Value,
    /// For A-class events: sequence number from EventWriter. None for B-class.
    pub seq: Option<u64>,
    /// Optional run_id for run-scoped event filtering
    pub run_id: Option<String>,
}

/// Dual-channel broadcaster: A-class (reliable, replayable) + B-class (lossy, realtime).
#[derive(Clone)]
pub struct EventBroadcaster {
    /// A-class channel: reliable, large capacity, for replayable bus-events
    a_tx: broadcast::Sender<BroadcastMsg>,
    /// B-class channel: lossy, medium capacity, for realtime streams (pty/chat/run-event)
    b_tx: broadcast::Sender<BroadcastMsg>,
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBroadcaster {
    pub fn new() -> Self {
        let (a_tx, _) = broadcast::channel(8192);
        let (b_tx, _) = broadcast::channel(1024);
        log::debug!("[broadcaster] created: A-channel=8192, B-channel=1024");
        Self { a_tx, b_tx }
    }

    /// Subscribe to A-class (replayable) events
    pub fn subscribe_a(&self) -> broadcast::Receiver<BroadcastMsg> {
        self.a_tx.subscribe()
    }

    /// Subscribe to B-class (realtime) events
    pub fn subscribe_b(&self) -> broadcast::Receiver<BroadcastMsg> {
        self.b_tx.subscribe()
    }

    /// Send an A-class event (bus-event with seq)
    pub fn send_a(&self, msg: BroadcastMsg) {
        let _ = self.a_tx.send(msg);
    }

    /// Send a B-class event (realtime, no seq)
    pub fn send_b(&self, msg: BroadcastMsg) {
        let _ = self.b_tx.send(msg);
    }
}

/// Unified emitter that replaces all direct `app.emit()` calls.
/// Handles: persist (A-class) + Tauri emit + broadcast to WS clients.
pub struct BroadcastEmitter {
    writer: Arc<EventWriter>,
    app: tauri::AppHandle,
    broadcaster: EventBroadcaster,
}

impl BroadcastEmitter {
    pub fn new(
        writer: Arc<EventWriter>,
        app: tauri::AppHandle,
        broadcaster: EventBroadcaster,
    ) -> Self {
        log::debug!("[emitter] BroadcastEmitter created");
        Self {
            writer,
            app,
            broadcaster,
        }
    }

    /// A-class: persist to events.jsonl + Tauri emit + broadcast with seq.
    /// This is the ONLY entry point for bus-event emission.
    pub fn persist_and_emit(&self, run_id: &str, event: &BusEvent) {
        let ts = crate::models::now_iso();
        match self.writer.write_bus_event_with_ts(run_id, event, &ts) {
            Ok(seq) => {
                log::trace!(
                    "[emitter] persist_and_emit: run_id={}, seq={}, type={:?}",
                    run_id,
                    seq,
                    event_type_name(event)
                );
                let _ = self.app.emit("bus-event", event);
                let payload = match serde_json::to_value(event) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("[emitter] serialize bus-event failed: {}", e);
                        return;
                    }
                };
                self.broadcaster.send_a(BroadcastMsg {
                    event_name: "bus-event".to_string(),
                    payload,
                    seq: Some(seq),
                    run_id: Some(run_id.to_string()),
                });
            }
            Err(e) => {
                log::warn!("[emitter] persist failed for run_id={}: {}", run_id, e);
                // Still emit to Tauri even if persist failed
                let _ = self.app.emit("bus-event", event);
            }
        }
    }

    /// B-class: Tauri emit + broadcast (no persist, no seq).
    /// For realtime streams: pty-output, pty-exit, chat-delta, chat-done, run-event, etc.
    pub fn emit_realtime<T: Serialize + Clone>(
        &self,
        event_name: &str,
        payload: &T,
        run_id: Option<&str>,
    ) {
        log::trace!(
            "[emitter] emit_realtime: event={}, run_id={:?}",
            event_name,
            run_id
        );
        let _ = self.app.emit(event_name, payload);
        let value = match serde_json::to_value(payload) {
            Ok(v) => v,
            Err(e) => {
                log::error!("[emitter] serialize {} failed: {}", event_name, e);
                return;
            }
        };
        self.broadcaster.send_b(BroadcastMsg {
            event_name: event_name.to_string(),
            payload: value,
            seq: None,
            run_id: run_id.map(|s| s.to_string()),
        });
    }

    /// Get a reference to the inner EventWriter (for direct reads like list_bus_events)
    pub fn writer(&self) -> &EventWriter {
        &self.writer
    }

    /// Get a reference to the inner EventBroadcaster (for WS subscriptions)
    pub fn broadcaster(&self) -> &EventBroadcaster {
        &self.broadcaster
    }

    /// Get a reference to the AppHandle
    pub fn app(&self) -> &tauri::AppHandle {
        &self.app
    }
}

/// Extract event type name for logging
fn event_type_name(event: &BusEvent) -> &'static str {
    match event {
        BusEvent::SessionInit { .. } => "session_init",
        BusEvent::MessageDelta { .. } => "message_delta",
        BusEvent::MessageComplete { .. } => "message_complete",
        BusEvent::UserMessage { .. } => "user_message",
        BusEvent::ToolStart { .. } => "tool_start",
        BusEvent::ToolEnd { .. } => "tool_end",
        BusEvent::RunState { .. } => "run_state",
        BusEvent::UsageUpdate { .. } => "usage_update",
        BusEvent::ThinkingDelta { .. } => "thinking_delta",
        BusEvent::ToolInputDelta { .. } => "tool_input_delta",
        BusEvent::PermissionDenied { .. } => "permission_denied",
        BusEvent::PermissionPrompt { .. } => "permission_prompt",
        BusEvent::CompactBoundary { .. } => "compact_boundary",
        BusEvent::SystemStatus { .. } => "system_status",
        BusEvent::AuthStatus { .. } => "auth_status",
        BusEvent::HookStarted { .. } => "hook_started",
        BusEvent::HookProgress { .. } => "hook_progress",
        BusEvent::HookResponse { .. } => "hook_response",
        BusEvent::HookCallback { .. } => "hook_callback",
        BusEvent::TaskNotification { .. } => "task_notification",
        BusEvent::ToolProgress { .. } => "tool_progress",
        BusEvent::ToolUseSummary { .. } => "tool_use_summary",
        BusEvent::FilesPersisted { .. } => "files_persisted",
        BusEvent::ControlCancelled { .. } => "control_cancelled",
        BusEvent::CommandOutput { .. } => "command_output",
        BusEvent::ElicitationPrompt { .. } => "elicitation_prompt",
        BusEvent::Raw { .. } => "raw",
    }
}

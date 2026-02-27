//! Turn Transaction Engine — types, extractors, and gate functions.
//!
//! Every stdin write belongs to an explicit turn (User or Internal).
//! The engine provides the data model, the `InternalExtractor` trait for
//! pluggable extraction during internal turns, and pure gate functions
//! for auto-context dedup.

use crate::models::BusEvent;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use super::session_actor::AttachmentData;

// ── Turn types ──

#[derive(Debug, Clone, PartialEq)]
pub enum TurnOrigin {
    User(UserTurnKind),
    Internal(InternalJobKind),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UserTurnKind {
    /// Normal message — triggers auto-context. auto_ctx_id is fixed at allocation time.
    Normal { auto_ctx_id: u32 },
    /// Slash command — does not trigger auto-context.
    Slash { command: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnPhase {
    Active,
    /// Soft timeout reached — extractor finalized, events suppressed.
    Draining,
}

pub struct ActiveTurn {
    pub turn_seq: u64,
    pub origin: TurnOrigin,
    pub phase: TurnPhase,
    pub started_at: Instant,
    pub soft_deadline: Instant,
    pub hard_deadline: Instant,
    /// Unified turn index (includes slash), aligns with frontend turnUsages.
    pub turn_index: u32,
}

pub struct UserTurnTicket {
    pub ticket_seq: u64,
    pub text: String,
    pub attachments: Vec<AttachmentData>,
    pub kind: UserTurnKind,
    pub turn_index: u32,
    pub reply: oneshot::Sender<Result<(), String>>,
}

pub struct InternalJob {
    pub job_seq: u64,
    pub kind: InternalJobKind,
    pub for_auto_ctx_id: u32,
    pub for_turn_index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InternalJobKind {
    AutoContext,
}

// ── Internal extractor trait ──

pub trait InternalExtractor: Send {
    fn on_event(&mut self, event: &BusEvent);
    fn finalize(&mut self, timed_out: bool);
}

/// Extracts context data from /context command output during internal turns.
pub struct ContextExtractor {
    pub app: AppHandle,
    pub run_id: String,
    pub for_turn_index: u32,
    pub captured: bool,
}

impl InternalExtractor for ContextExtractor {
    fn on_event(&mut self, event: &BusEvent) {
        match event {
            BusEvent::CommandOutput { content, .. } => {
                log::debug!(
                    "[autoctx] captured source=command_output turn_index={}",
                    self.for_turn_index
                );
                self.emit_context_snapshot(content);
                self.captured = true;
            }
            BusEvent::MessageComplete { text, .. } if !text.is_empty() && !self.captured => {
                log::debug!(
                    "[autoctx] captured source=message_complete turn_index={}",
                    self.for_turn_index
                );
                self.emit_context_snapshot(text);
                self.captured = true;
            }
            _ => {}
        }
    }

    fn finalize(&mut self, timed_out: bool) {
        if timed_out && !self.captured {
            log::warn!(
                "[autoctx] timed out without data for turn_index={}",
                self.for_turn_index
            );
        }
    }
}

impl ContextExtractor {
    fn emit_context_snapshot(&self, content: &str) {
        let _ = self.app.emit(
            "context-snapshot",
            serde_json::json!({
                "runId": self.run_id,
                "content": content,
                "turnIndex": self.for_turn_index,
                "ts": chrono::Utc::now().to_rfc3339(),
            }),
        );
    }
}

// ── Gate functions ──

/// Check if auto-context should trigger for this auto_ctx_id (dedup).
pub fn should_trigger_auto_context(auto_ctx_id: u32, last: Option<u32>) -> bool {
    last != Some(auto_ctx_id)
}

// ── Default timeouts ──

/// User turns get generous timeouts (CLI can take a long time)
pub const USER_SOFT_TIMEOUT: Duration = Duration::from_secs(300);
pub const USER_HARD_TIMEOUT: Duration = Duration::from_secs(600);

/// Internal turns (auto-context) get tight timeouts
pub const INTERNAL_SOFT_TIMEOUT: Duration = Duration::from_secs(8);
pub const INTERNAL_HARD_TIMEOUT: Duration = Duration::from_secs(30);

/// Quarantine secondary timeout (after interrupt sent, wait for CLI response)
pub const QUARANTINE_DEADLINE: Duration = Duration::from_secs(10);

/// Tick interval for the independent timeout clock
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

// ── Unit tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_ctx_skip_duplicate() {
        assert!(!should_trigger_auto_context(1, Some(1)));
    }

    #[test]
    fn auto_ctx_trigger_new() {
        assert!(should_trigger_auto_context(1, None));
    }

    #[test]
    fn auto_ctx_trigger_next() {
        assert!(should_trigger_auto_context(2, Some(1)));
    }
}

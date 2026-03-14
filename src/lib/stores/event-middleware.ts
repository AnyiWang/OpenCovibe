/**
 * EventMiddleware: unified Tauri event listener management.
 *
 * - Registers all 8 Tauri event listeners once
 * - Routes events by run_id to the subscribed SessionStore
 * - Microbatches bus-events (16ms) to reduce reactive updates
 * - PTY/Pipe events go through handler callbacks (DOM-bound)
 */
import { dbg, dbgWarn } from "$lib/utils/debug";
import type { BusEvent, HookEvent } from "$lib/types";
import type { SessionStore } from "./session-store.svelte";
import { markAttention, clearAttention } from "./attention-store.svelte";
import { getTransport } from "$lib/transport";

// ── Handler interfaces (page-level DOM callbacks) ──

export interface PtyHandler {
  onOutput(payload: { run_id: string; data: string }): void;
  onExit(payload: { run_id: string; exit_code: number }): void;
}

export interface PipeHandler {
  onDelta(delta: { text: string }): void;
  onDone(done: { ok: boolean; code: number; error?: string }): void;
}

export interface RunEventHandler {
  onRunEvent(event: { run_id: string; type: string; text: string }): void;
}

// ── Middleware ──

export class EventMiddleware {
  private _unlisteners: (() => void)[] = [];
  private _subscriptions = new Map<string, SessionStore>();
  private _currentRunId: string | null = null;
  private _currentStore: SessionStore | null = null;

  // Handler callbacks (set by page component)
  private _ptyHandler: PtyHandler | null = null;
  private _pipeHandler: PipeHandler | null = null;
  private _runEventHandler: RunEventHandler | null = null;

  // Microbatch buffer for bus events
  private _batchBuffer = new Map<string, BusEvent[]>();
  private _flushScheduled = false;
  private _BATCH_INTERVAL = 16; // ~1 frame
  private _MAX_BUFFER_SIZE = 500; // per-run overflow threshold

  // Idempotent start guard
  private _started = false;

  // Debounce guard for _full_reload
  private _reloadingRuns = new Set<string>();

  // ── Lifecycle ──

  async start(): Promise<void> {
    if (this._started) {
      dbg("middleware", "start skipped (already started)");
      return;
    }
    this._started = true;
    dbg("middleware", "starting event listeners");
    const ul = this._unlisteners;

    const transport = getTransport();

    // Helper: register a single listener via transport (works for both Tauri + WS).
    // Transport.listen delivers payload directly (TauriTransport unwraps the envelope).
    // If one listener fails to register, the rest still get set up (partial degradation).
    const reg = async <T>(name: string, handler: (payload: T) => void) => {
      try {
        ul.push(await transport.listen<T>(name, handler));
      } catch (e) {
        dbgWarn("middleware", `failed to register listener for "${name}":`, e);
      }
    };

    // 1. Bus events (stream session mode) — microbatched
    await reg<BusEvent>("bus-event", (ev) => {
      dbg("middleware", "bus-event", { type: ev.type, run_id: ev.run_id });
      this._handleBusEvent(ev);
    });

    // 2. PTY output
    await reg<{ run_id: string; data: string }>("pty-output", (payload) => {
      dbg("middleware", "pty-output", { run_id: payload.run_id });
      this._ptyHandler?.onOutput(payload);
    });

    // 3. PTY exit
    await reg<{ run_id: string; exit_code: number }>("pty-exit", (payload) => {
      dbg("middleware", "pty-exit", payload);
      this._ptyHandler?.onExit(payload);
    });

    // 4. Chat delta (pipe mode)
    await reg<{ text: string }>("chat-delta", (payload) => {
      dbg("middleware", "chat-delta", { len: payload.text.length });
      this._pipeHandler?.onDelta(payload);
    });

    // 5. Chat done (pipe mode)
    await reg<{ ok: boolean; code: number; error?: string }>("chat-done", (payload) => {
      dbg("middleware", "chat-done", payload);
      this._pipeHandler?.onDone(payload);
    });

    // 6. Run events (pipe mode stderr)
    await reg<{ run_id: string; type: string; text: string }>("run-event", (payload) => {
      dbg("middleware", "run-event", { run_id: payload.run_id, type: payload.type });
      this._runEventHandler?.onRunEvent(payload);
    });

    // 7. Hook events
    await reg<HookEvent>("hook-event", (payload) => {
      dbg("middleware", "hook-event", {
        hook_type: payload.hook_type,
        tool: payload.tool_name,
      });
      this._handleHookEvent(payload);
    });

    // 8. Hook usage
    await reg<{ run_id: string; input_tokens: number; output_tokens: number; cost: number }>(
      "hook-usage",
      (payload) => {
        dbg("middleware", "hook-usage", payload);
        this._handleHookUsage(payload);
      },
    );

    // 9. Full reload (WS-only — Tauri transport won't emit this event)
    if (!transport.isDesktop()) {
      try {
        const unlisten = await transport.listen<{ run_id: string }>("_full_reload", (payload) => {
          const runId = payload.run_id;
          dbgWarn("middleware", "_full_reload", { runId });
          if (this._reloadingRuns.has(runId)) {
            dbg("middleware", "_full_reload debounced", { runId });
            return;
          }
          const store = this._subscriptions.get(runId);
          if (store) {
            this._reloadingRuns.add(runId);
            void store.loadRun(runId).finally(() => {
              this._reloadingRuns.delete(runId);
            });
          }
        });
        ul.push(unlisten);
      } catch (e) {
        dbgWarn("middleware", "failed to register _full_reload listener:", e);
      }
    }

    dbg("middleware", "all listeners registered:", ul.length);
  }

  destroy(): void {
    dbg("middleware", "destroying, unregistering", this._unlisteners.length, "listeners");
    // Unsubscribe all runs from transport before cleanup
    const transport = getTransport();
    for (const runId of this._subscriptions.keys()) {
      transport.unsubscribeRun(runId);
    }
    for (const fn of this._unlisteners) fn();
    this._unlisteners = [];
    this._subscriptions.clear();
    this._currentRunId = null;
    this._currentStore = null;
    this._batchBuffer.clear();
    this._reloadingRuns.clear();
    this._started = false;
  }

  // ── Subscriptions ──

  /** Subscribe a store for a run_id. Clears previous subscription (single-session mode). */
  subscribeCurrent(runId: string, store: SessionStore): void {
    // Idempotent: skip if already subscribed for the same run + store.
    // Re-subscribing for the same pair would clear the batch buffer,
    // dropping in-flight events (e.g. RunState(idle) after resume).
    if (runId && this._currentRunId === runId && this._currentStore === store) {
      return;
    }

    // Clear old subscription (different run or different store)
    if (this._currentRunId) {
      getTransport().unsubscribeRun(this._currentRunId);
      this._subscriptions.delete(this._currentRunId);
      this._batchBuffer.delete(this._currentRunId);
    }
    if (runId) {
      this._currentRunId = runId;
      this._currentStore = store;
      this._subscriptions.set(runId, store);
    } else {
      // Empty runId = clear all (navigating to new chat)
      this._currentRunId = null;
      this._currentStore = null;
    }
    dbg("middleware", "subscribeCurrent", runId || "(cleared)");
  }

  /** Multi-session subscribe (for future subagent support). */
  subscribe(runId: string, store: SessionStore): void {
    this._subscriptions.set(runId, store);
    dbg("middleware", "subscribe", runId);
  }

  unsubscribe(runId: string): void {
    getTransport().unsubscribeRun(runId);
    this._subscriptions.delete(runId);
    this._batchBuffer.delete(runId);
    if (this._currentRunId === runId) {
      this._currentRunId = null;
      this._currentStore = null;
    }
    dbg("middleware", "unsubscribe", runId);
  }

  // ── Handler setters ──

  setPtyHandler(handler: PtyHandler | null): void {
    this._ptyHandler = handler;
  }

  setPipeHandler(handler: PipeHandler | null): void {
    this._pipeHandler = handler;
  }

  setRunEventHandler(handler: RunEventHandler | null): void {
    this._runEventHandler = handler;
  }

  // ── Internal ──

  private _handleBusEvent(ev: BusEvent): void {
    this._trackAttention(ev);

    const store = this._subscriptions.get(ev.run_id);
    if (!store) return;

    // Push to batch buffer
    let buf = this._batchBuffer.get(ev.run_id);
    if (!buf) {
      buf = [];
      this._batchBuffer.set(ev.run_id, buf);
    }
    buf.push(ev);

    // Overflow protection: flush synchronously if buffer grows too large
    if (buf.length >= this._MAX_BUFFER_SIZE) {
      dbgWarn(
        "middleware",
        `buffer overflow for ${ev.run_id} (${buf.length} events), flushing synchronously`,
      );
      this._flush();
      return;
    }

    this._scheduleFlush();
  }

  private _trackAttention(ev: BusEvent): void {
    switch (ev.type) {
      case "permission_prompt":
      case "elicitation_prompt":
        markAttention(ev.run_id, "permission");
        break;
      case "tool_end":
        if (ev.tool_name === "AskUserQuestion" && ev.status === "error") {
          markAttention(ev.run_id, "ask");
        }
        break;
      case "permission_denied":
        clearAttention(ev.run_id, "permission");
        // AskUserQuestion denied: tool_end(error) arrives first and marks ask,
        // but the question was denied, not pending — clear ask too.
        if (ev.tool_name === "AskUserQuestion") {
          clearAttention(ev.run_id, "ask");
        }
        break;
      case "control_cancelled":
        clearAttention(ev.run_id, "permission");
        break;
      case "user_message":
        clearAttention(ev.run_id, "ask");
        break;
      case "run_state":
        switch (ev.state) {
          case "spawning":
          case "idle":
            clearAttention(ev.run_id, "permission");
            break;
          case "running":
            clearAttention(ev.run_id, "ask");
            break;
          case "stopped":
          case "completed":
          case "failed":
            clearAttention(ev.run_id);
            break;
        }
        break;
    }
  }

  private _handleHookEvent(event: HookEvent): void {
    const store = this._subscriptions.get(event.run_id);
    if (!store) return;
    store.applyHookEvent(event);
  }

  private _handleHookUsage(usage: {
    run_id: string;
    input_tokens: number;
    output_tokens: number;
    cost: number;
  }): void {
    const store = this._subscriptions.get(usage.run_id);
    if (!store) return;
    store.applyHookUsage(usage);
  }

  private _scheduleFlush(): void {
    if (this._flushScheduled) return;
    this._flushScheduled = true;
    if (typeof requestAnimationFrame !== "undefined") {
      requestAnimationFrame(() => this._flush());
    } else {
      setTimeout(() => this._flush(), this._BATCH_INTERVAL);
    }
  }

  private _flush(): void {
    this._flushScheduled = false;
    for (const [runId, events] of this._batchBuffer) {
      const store = this._subscriptions.get(runId);
      if (!store) continue;
      try {
        if (events.length === 1) {
          store.applyEvent(events[0]);
        } else if (events.length > 1) {
          store.applyEventBatch(events);
        }
      } catch (e) {
        dbgWarn("middleware", `flush error for run ${runId}:`, e);
      }
    }
    this._batchBuffer.clear();
  }
}

// ── Module-level singleton ──

let _instance: EventMiddleware | null = null;

export function getEventMiddleware(): EventMiddleware {
  if (!_instance) {
    _instance = new EventMiddleware();
  }
  return _instance;
}

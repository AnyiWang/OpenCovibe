/**
 * EventMiddleware: unified Tauri event listener management.
 *
 * - Registers all 10 Tauri event listeners once
 * - Routes events by run_id to the subscribed SessionStore
 * - Microbatches bus-events (16ms) to reduce reactive updates
 * - PTY/Pipe events go through handler callbacks (DOM-bound)
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { dbg, dbgWarn } from "$lib/utils/debug";
import type { BusEvent, HookEvent } from "$lib/types";
import type { SessionStore } from "./session-store.svelte";
import type { TeamStore } from "./team-store.svelte";

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
  private _unlisteners: UnlistenFn[] = [];
  private _subscriptions = new Map<string, SessionStore>();
  private _currentRunId: string | null = null;
  private _currentStore: SessionStore | null = null;

  // Handler callbacks (set by page component)
  private _ptyHandler: PtyHandler | null = null;
  private _pipeHandler: PipeHandler | null = null;
  private _runEventHandler: RunEventHandler | null = null;
  private _teamStore: TeamStore | null = null;

  // Microbatch buffer for bus events
  private _batchBuffer = new Map<string, BusEvent[]>();
  private _flushScheduled = false;
  private _BATCH_INTERVAL = 16; // ~1 frame
  private _MAX_BUFFER_SIZE = 500; // per-run overflow threshold

  // Idempotent start guard
  private _started = false;

  // ── Lifecycle ──

  async start(): Promise<void> {
    if (this._started) {
      dbg("middleware", "start skipped (already started)");
      return;
    }
    this._started = true;
    dbg("middleware", "starting event listeners");
    const ul = this._unlisteners;

    // Helper: register a single listener with error isolation.
    // If one listener fails to register, the rest still get set up (partial degradation).
    const reg = async <T>(name: string, handler: (event: { payload: T }) => void) => {
      try {
        ul.push(await listen<T>(name, handler));
      } catch (e) {
        dbgWarn("middleware", `failed to register listener for "${name}":`, e);
      }
    };

    // 1. Bus events (stream session mode) — microbatched
    await reg<BusEvent>("bus-event", (event) => {
      const ev = event.payload;
      dbg("middleware", "bus-event", { type: ev.type, run_id: ev.run_id });
      this._handleBusEvent(ev);
    });

    // 2. PTY output
    await reg<{ run_id: string; data: string }>("pty-output", (event) => {
      dbg("middleware", "pty-output", { run_id: event.payload.run_id });
      this._ptyHandler?.onOutput(event.payload);
    });

    // 3. PTY exit
    await reg<{ run_id: string; exit_code: number }>("pty-exit", (event) => {
      dbg("middleware", "pty-exit", event.payload);
      this._ptyHandler?.onExit(event.payload);
    });

    // 4. Chat delta (pipe mode)
    await reg<{ text: string }>("chat-delta", (event) => {
      dbg("middleware", "chat-delta", { len: event.payload.text.length });
      this._pipeHandler?.onDelta(event.payload);
    });

    // 5. Chat done (pipe mode)
    await reg<{ ok: boolean; code: number; error?: string }>("chat-done", (event) => {
      dbg("middleware", "chat-done", event.payload);
      this._pipeHandler?.onDone(event.payload);
    });

    // 6. Run events (pipe mode stderr)
    await reg<{ run_id: string; type: string; text: string }>("run-event", (event) => {
      dbg("middleware", "run-event", { run_id: event.payload.run_id, type: event.payload.type });
      this._runEventHandler?.onRunEvent(event.payload);
    });

    // 7. Hook events
    await reg<HookEvent>("hook-event", (event) => {
      dbg("middleware", "hook-event", {
        hook_type: event.payload.hook_type,
        tool: event.payload.tool_name,
      });
      this._handleHookEvent(event.payload);
    });

    // 8. Hook usage
    await reg<{ run_id: string; input_tokens: number; output_tokens: number; cost: number }>(
      "hook-usage",
      (event) => {
        dbg("middleware", "hook-usage", event.payload);
        this._handleHookUsage(event.payload);
      },
    );

    // 9. Team file watcher events
    await reg<{ team_name: string; change: string }>("team-update", (event) => {
      dbg("middleware", "team-update", event.payload);
      this._teamStore?.handleTeamUpdate(event.payload);
    });

    // 10. Task file watcher events
    await reg<{ team_name: string; task_id: string; change: string }>("task-update", (event) => {
      dbg("middleware", "task-update", event.payload);
      this._teamStore?.handleTaskUpdate(event.payload);
    });

    dbg("middleware", "all listeners registered:", ul.length);
  }

  destroy(): void {
    dbg("middleware", "destroying, unregistering", this._unlisteners.length, "listeners");
    for (const fn of this._unlisteners) fn();
    this._unlisteners = [];
    this._subscriptions.clear();
    this._currentRunId = null;
    this._currentStore = null;
    this._batchBuffer.clear();
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

  setTeamStore(store: TeamStore | null): void {
    this._teamStore = store;
  }

  // ── Internal ──

  private _handleBusEvent(ev: BusEvent): void {
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

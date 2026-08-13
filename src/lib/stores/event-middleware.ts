/**
 * EventMiddleware: unified Tauri event listener management.
 *
 * - Registers Tauri event listeners once
 * - Routes events by run_id to the subscribed SessionStore
 * - Microbatches bus-events (16ms) to reduce reactive updates
 * - Pipe events go through handler callbacks (DOM-bound)
 */
import { dbg, dbgWarn } from "$lib/utils/debug";
import type { BusEvent, HookEvent } from "$lib/types";
import type { SessionStore } from "./session-store.svelte";
import { markAttention, clearAttention } from "./attention-store.svelte";
import { getTransport } from "$lib/transport";

// ── Handler interfaces (page-level DOM callbacks) ──

export interface PipeHandler {
  onDelta(delta: { text: string }): void;
  onDone(done: { ok: boolean; code: number; error?: string }): void;
}

export interface RunEventHandler {
  onRunEvent(event: { run_id: string; type: string; text: string }): void;
}

// ── Middleware ──

export class EventMiddleware {
  private static readonly _FULL_RELOAD_MAX_ATTEMPTS = 3;
  private static readonly _FULL_RELOAD_RETRY_MS = [100, 250];
  private _unlisteners: (() => void)[] = [];
  private _subscriptions = new Map<string, SessionStore>();
  private _currentRunId: string | null = null;
  private _currentStore: SessionStore | null = null;

  // Handler callbacks (set by page component)
  private _pipeHandler: PipeHandler | null = null;
  private _runEventHandler: RunEventHandler | null = null;

  // Microbatch buffer for bus events
  private _batchBuffer = new Map<string, BusEvent[]>();
  private _historyLoads = new Map<string, { store: SessionStore; token: number }>();
  private _nextHistoryLoadToken = 0;
  private _flushScheduled = false;
  private _BATCH_INTERVAL = 16; // ~1 frame
  private _MAX_BUFFER_SIZE = 500; // per-run overflow threshold
  private _MAX_HISTORY_BUFFER_SIZE = 8192;
  private _lastFlushTime = 0; // track last flush for idle detection
  private _IDLE_GAP_MS = 100; // gap above which the next token starts a fresh burst

  // Idempotent start guard
  private _started = false;

  // Debounce guard for _full_reload
  private _reloadingRuns = new Set<string>();
  private _reloadAgainRuns = new Set<string>();
  private _reloadEpoch = 0;

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

    // 2. Chat delta (pipe mode)
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
          const store = this._subscriptions.get(runId);
          if (store) this._requestFullReload(runId, store);
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
    this._historyLoads.clear();
    this._reloadingRuns.clear();
    this._reloadAgainRuns.clear();
    this._reloadEpoch++;
    this._started = false;
  }

  // ── Subscriptions ──

  /** The store bound to the active session, or null when none is live. Lets cross-page surfaces
   *  (e.g. the /agents Active scope) read the running session without a global SessionStore.
   *  Plain field — NOT reactive; callers that need updates must poll. */
  get currentStore(): SessionStore | null {
    return this._currentStore;
  }

  /** Run id of the active session, or null. */
  get currentRunId(): string | null {
    return this._currentRunId;
  }

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
      this._historyLoads.delete(this._currentRunId);
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

  /** Buffer live events while a history page is replacing store state. */
  beginHistoryLoad(runId: string, store: SessionStore): number {
    const token = ++this._nextHistoryLoadToken;
    this._historyLoads.set(runId, { store, token });
    dbg("middleware", "history load started", { runId, token });
    return token;
  }

  /** Atomically release events newer than the persisted catch-up checkpoint. */
  finishHistoryLoad(runId: string, store: SessionStore, token: number, lastSeq: number): void {
    const load = this._historyLoads.get(runId);
    if (!load || load.store !== store || load.token !== token) return;
    this._historyLoads.delete(runId);
    const events = (this._batchBuffer.get(runId) ?? []).filter((event) => {
      const seq = ((event as Record<string, unknown>)._seq as number) ?? 0;
      return seq === 0 || seq > lastSeq;
    });
    this._batchBuffer.delete(runId);
    if (events.length === 1) store.applyEvent(events[0]);
    else if (events.length > 1) store.applyEventBatch(events);
    dbg("middleware", "history load released", { runId, token, lastSeq, events: events.length });
  }

  cancelHistoryLoad(runId: string, store: SessionStore, token: number): void {
    const load = this._historyLoads.get(runId);
    if (!load || load.store !== store || load.token !== token) return;
    this._historyLoads.delete(runId);
    this._batchBuffer.delete(runId);
    dbg("middleware", "history load cancelled", { runId, token });
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
    this._historyLoads.delete(runId);
    if (this._currentRunId === runId) {
      this._currentRunId = null;
      this._currentStore = null;
    }
    dbg("middleware", "unsubscribe", runId);
  }

  // ── Handler setters ──

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

    if (this._historyLoads.has(ev.run_id) && buf.length > this._MAX_HISTORY_BUFFER_SIZE) {
      const store = this._subscriptions.get(ev.run_id);
      this._historyLoads.delete(ev.run_id);
      this._batchBuffer.delete(ev.run_id);
      if (store) {
        this._requestFullReload(ev.run_id, store);
      }
      dbgWarn("middleware", "history live buffer overflow; reloading", { runId: ev.run_id });
      return;
    }

    // Overflow protection: flush synchronously if buffer grows too large
    if (!this._historyLoads.has(ev.run_id) && buf.length >= this._MAX_BUFFER_SIZE) {
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
      case "interaction_response_started":
      case "interaction_response_failed":
      case "interaction_resolved":
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

  private _requestFullReload(runId: string, store: SessionStore): void {
    if (this._reloadingRuns.has(runId)) {
      this._reloadAgainRuns.add(runId);
      dbg("middleware", "full reload queued", { runId });
      return;
    }
    this._reloadingRuns.add(runId);
    const epoch = this._reloadEpoch;
    void this._performFullReload(runId, store, epoch).finally(() => {
      this._reloadingRuns.delete(runId);
      if (epoch !== this._reloadEpoch) return;
      if (!this._reloadAgainRuns.delete(runId)) return;
      const current = this._subscriptions.get(runId);
      if (current) this._requestFullReload(runId, current);
    });
  }

  private async _performFullReload(
    runId: string,
    store: SessionStore,
    epoch: number,
  ): Promise<void> {
    for (let attempt = 1; attempt <= EventMiddleware._FULL_RELOAD_MAX_ATTEMPTS; attempt++) {
      if (epoch !== this._reloadEpoch || this._subscriptions.get(runId) !== store) return;
      try {
        const loaded = await store.loadRun(runId);
        if (loaded) {
          dbg("middleware", "full reload complete", { runId, attempt });
          return;
        }
        dbgWarn("middleware", "full reload did not complete", { runId, attempt });
      } catch (error) {
        dbgWarn("middleware", "full reload failed", { runId, attempt, error });
      }

      if (attempt < EventMiddleware._FULL_RELOAD_MAX_ATTEMPTS) {
        await new Promise<void>((resolve) =>
          setTimeout(resolve, EventMiddleware._FULL_RELOAD_RETRY_MS[attempt - 1]),
        );
      }
    }

    if (epoch !== this._reloadEpoch || this._subscriptions.get(runId) !== store) return;
    this._reloadAgainRuns.delete(runId);
    // The server retains full_reload_pending until subscribe/unsubscribe. Exhaustion must clear
    // that state without accepting more live events against an incomplete local timeline.
    getTransport().unsubscribeRun(runId);
    dbgWarn("middleware", "full reload exhausted", { runId });
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

    const now = performance.now();
    const idleGap = now - this._lastFlushTime;

    // If idle for >_IDLE_GAP_MS, this is the first token of a new burst — flush
    // immediately via microtask so the user sees output without a ~16ms gap.
    if (idleGap > this._IDLE_GAP_MS) {
      queueMicrotask(() => this._flush());
    } else if (typeof requestAnimationFrame !== "undefined") {
      requestAnimationFrame(() => this._flush());
    } else {
      setTimeout(() => this._flush(), this._BATCH_INTERVAL);
    }
  }

  private _flush(): void {
    this._flushScheduled = false;
    this._lastFlushTime = performance.now();
    for (const [runId, events] of this._batchBuffer) {
      if (this._historyLoads.has(runId)) continue;
      const store = this._subscriptions.get(runId);
      if (!store) {
        this._batchBuffer.delete(runId);
        continue;
      }
      try {
        if (events.length === 1) {
          store.applyEvent(events[0]);
        } else if (events.length > 1) {
          store.applyEventBatch(events);
        }
      } catch (e) {
        dbgWarn("middleware", `flush error for run ${runId}:`, e);
      }
      this._batchBuffer.delete(runId);
    }
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

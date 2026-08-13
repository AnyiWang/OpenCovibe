/**
 * WsTransport: WebSocket JSON-RPC transport for browser access.
 *
 * - Auto-reconnect with exponential backoff (1s -> 2s -> 4s -> ... -> 30s max)
 * - Close code 4401 -> stop reconnecting (auth failure)
 * - Request/response correlation via `id` field
 * - Server push events dispatched to registered handlers
 * - Cookie-based auth (no token in URL)
 * - Auto _subscribe/_unsubscribe for run-scoped events
 */
import { dbg, dbgWarn } from "$lib/utils/debug";
import type { Transport } from "./index";

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

interface SubscriptionAttempt {
  epoch: number;
  promise: Promise<void>;
}

export class WsTransport implements Transport {
  private ws: WebSocket | null = null;
  private reqId = 0;
  private pending = new Map<string, PendingRequest>();
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** Per-run seq checkpoint for reconnect replay */
  private lastSeq = new Map<string, number>();
  /** Runs we've subscribed to on the server */
  private subscribedRuns = new Set<string>();
  /** Runs that must be acknowledged again on the next healthy connection */
  private needsResubscribe = new Set<string>();
  /** Full reload keeps intent but suspends replay until history establishes a new checkpoint */
  private suspendedRuns = new Set<string>();
  /** New sessions have no history owner, so initial failures retry on reconnect from seq 0 */
  private liveRecoveryRuns = new Set<string>();
  private subscriptionEpochs = new Map<string, number>();
  private subscriptionAttempts = new Map<string, SubscriptionAttempt>();
  private reconnectDelay = 1000;
  private shouldReconnect = true;
  private connectPromise: Promise<void> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  private buildWsUrl(): string {
    const loc = window.location;
    const protocol = loc.protocol === "https:" ? "wss:" : "ws:";
    const url = `${protocol}//${loc.host}/ws`;
    dbg("transport", "ws.buildUrl", { url });
    return url;
  }

  private connect(): Promise<void> {
    if (this.connectPromise) return this.connectPromise;

    const attempt = new Promise<void>((resolve, reject) => {
      const url = this.buildWsUrl();
      dbg("transport", "ws.connecting", { url });

      const ws = new WebSocket(url);
      this.ws = ws;
      let settled = false;
      const timeout = setTimeout(() => {
        if (this.ws === ws && ws.readyState === WebSocket.CONNECTING) {
          settle(new Error("WebSocket connection timeout"));
          ws.close();
        }
      }, 10000);

      const settle = (error?: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (this.connectPromise === attempt) this.connectPromise = null;
        if (error) reject(error);
        else resolve();
      };

      ws.onopen = () => {
        if (this.ws !== ws) {
          ws.close();
          settle(new Error("WebSocket connection superseded"));
          return;
        }
        dbg("transport", "ws.connected");
        this.reconnectDelay = 1000;
        settle();
        // Only runs acknowledged on a previous connection need automatic replay. A first-time
        // subscribe waiting on this connection resumes through its own subscribeRun call.
        this.resubscribeAll();
      };

      ws.onmessage = (ev) => {
        this.handleMessage(ev.data);
      };

      ws.onerror = (ev) => {
        dbgWarn("transport", "ws.error", ev);
      };

      ws.onclose = (ev) => {
        dbg("transport", "ws.closed", { code: ev.code, reason: ev.reason });
        settle(new Error(`WebSocket closed before connection opened (code ${ev.code})`));
        if (this.ws !== ws) return;
        this.ws = null;

        for (const runId of this.subscribedRuns) {
          if (!this.suspendedRuns.has(runId)) this.needsResubscribe.add(runId);
        }

        // Reject all pending requests
        for (const [id, req] of this.pending) {
          req.reject(new Error(`WebSocket closed (code ${ev.code})`));
          this.pending.delete(id);
        }

        if (ev.code === 4401) {
          // Auth failure — stop reconnecting, redirect to login
          dbgWarn("transport", "ws.authFailure, redirecting to /login");
          this.shouldReconnect = false;
          if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
          window.location.href = "/login";
          return;
        }

        this.scheduleReconnect();
      };
    });

    this.connectPromise = attempt;
    void attempt.then(
      () => {
        if (this.connectPromise === attempt) this.connectPromise = null;
      },
      (error) => {
        if (this.connectPromise === attempt) this.connectPromise = null;
        dbgWarn("transport", "ws.connectFailed", error);
        this.scheduleReconnect();
      },
    );
    return attempt;
  }

  private scheduleReconnect(): void {
    if (!this.shouldReconnect || this.reconnectTimer) return;
    const delay = Math.min(this.reconnectDelay, 30000);
    dbg("transport", "ws.reconnecting", { delay });
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
      void this.ensureConnected().catch((error) => {
        dbgWarn("transport", "ws.reconnectFailed", error);
        this.scheduleReconnect();
      });
    }, delay);
  }

  private async ensureConnected(): Promise<void> {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) return;
    await this.connect();
  }

  /** Re-subscribe all tracked runs after reconnect */
  private resubscribeAll(): void {
    for (const runId of [...this.needsResubscribe]) {
      if (!this.subscribedRuns.has(runId) || this.suspendedRuns.has(runId)) {
        this.needsResubscribe.delete(runId);
        continue;
      }
      const lastSeq = this.lastSeq.get(runId) ?? 0;
      const epoch = this.subscriptionEpochs.get(runId);
      if (epoch === undefined) continue;
      dbg("transport", "ws.resubscribe", { runId, lastSeq });
      void this.confirmSubscription(runId, lastSeq, epoch).catch((error) => {
        dbgWarn("transport", "ws.resubscribeFailed", { runId, error });
      });
    }
  }

  /** Subscribe to a run's real-time events on the server */
  async subscribeRun(
    runId: string,
    lastSeq = 0,
    recovery: "history" | "live" = "history",
  ): Promise<void> {
    // Monotonic: prevent checkpoint regression (e.g. accidental lastSeq=0 overwrites)
    const prev = this.lastSeq.get(runId) ?? 0;
    const effectiveSeq = Math.max(prev, lastSeq);
    const epoch = (this.subscriptionEpochs.get(runId) ?? 0) + 1;
    dbg("transport", "ws.subscribeRun", { runId, lastSeq, effectiveSeq });
    this.subscribedRuns.add(runId);
    this.lastSeq.set(runId, effectiveSeq);
    this.subscriptionEpochs.set(runId, epoch);
    if (recovery === "live") this.liveRecoveryRuns.add(runId);
    else this.liveRecoveryRuns.delete(runId);
    this.suspendedRuns.delete(runId);
    this.needsResubscribe.delete(runId);
    try {
      await this.ensureConnected();
      if (this.subscriptionEpochs.get(runId) !== epoch || !this.subscribedRuns.has(runId)) {
        throw new Error(`WebSocket subscription cancelled: ${runId}`);
      }
      await this.confirmSubscription(runId, effectiveSeq, epoch);
    } catch (error) {
      if (this.subscriptionEpochs.get(runId) === epoch && this.subscribedRuns.has(runId)) {
        if (this.liveRecoveryRuns.has(runId)) {
          // A new session has no history owner to retry for it. Preserve the seq-0 intent until a
          // reconnect acknowledges it; subsequent live events advance the checkpoint normally.
          this.suspendedRuns.delete(runId);
          this.needsResubscribe.add(runId);
        } else {
          // The explicit caller owns history buffering and retries. Keep its intent, but suspend
          // automatic replay until that caller establishes a fresh checkpoint on a later attempt.
          this.needsResubscribe.delete(runId);
          this.suspendedRuns.add(runId);
        }
      }
      throw error;
    }
  }

  private confirmSubscription(runId: string, lastSeq: number, epoch: number): Promise<void> {
    const existing = this.subscriptionAttempts.get(runId);
    if (existing?.epoch === epoch) return existing.promise;

    const promise = this.request("_subscribe", { run_id: runId, last_seq: lastSeq })
      .then(() => {
        if (this.subscriptionEpochs.get(runId) !== epoch || !this.subscribedRuns.has(runId)) {
          throw new Error(`WebSocket subscription cancelled: ${runId}`);
        }
        this.needsResubscribe.delete(runId);
        dbg("transport", "ws.subscribeConfirmed", { runId, lastSeq, epoch });
      })
      .catch((error) => {
        if (this.subscriptionEpochs.get(runId) === epoch && this.subscribedRuns.has(runId)) {
          this.needsResubscribe.add(runId);
          // An error/timeout is acknowledgement-ambiguous. Closing discards the server's
          // connection-local checkpoint; the desired subscription survives for the next retry.
          this.ws?.close();
        }
        throw error;
      })
      .finally(() => {
        const current = this.subscriptionAttempts.get(runId);
        if (current?.epoch === epoch) this.subscriptionAttempts.delete(runId);
      });
    this.subscriptionAttempts.set(runId, { epoch, promise });
    return promise;
  }

  /** Unsubscribe from a run's events */
  unsubscribeRun(runId: string): void {
    this.subscribedRuns.delete(runId);
    this.lastSeq.delete(runId);
    this.needsResubscribe.delete(runId);
    this.suspendedRuns.delete(runId);
    this.liveRecoveryRuns.delete(runId);
    this.subscriptionEpochs.set(runId, (this.subscriptionEpochs.get(runId) ?? 0) + 1);
    dbg("transport", "ws.unsubscribeRun", { runId });
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.sendRaw({
        id: `req_${++this.reqId}`,
        method: "_unsubscribe",
        params: { run_id: runId },
      });
    }
  }

  private sendRaw(obj: Record<string, unknown>): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      try {
        this.ws.send(JSON.stringify(obj));
      } catch (error) {
        dbgWarn("transport", "ws.sendFailed", error);
        this.ws.close();
      }
    }
  }

  private request(method: string, params: Record<string, unknown>): Promise<unknown> {
    const id = `req_${++this.reqId}`;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`WebSocket request timed out: ${method}`));
      }, 15000);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timeout);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timeout);
          reject(error);
        },
      });
      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        try {
          this.ws.send(JSON.stringify({ id, method, params }));
        } catch (error) {
          const pending = this.pending.get(id);
          this.pending.delete(id);
          pending?.reject(error instanceof Error ? error : new Error(String(error)));
        }
      } else {
        const pending = this.pending.get(id);
        this.pending.delete(id);
        pending?.reject(new Error("WebSocket not connected"));
      }
    });
  }

  private handleMessage(raw: string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(raw);
    } catch {
      dbgWarn("transport", "ws.invalidJson", { raw: raw.slice(0, 200) });
      return;
    }

    // Response to a request (has `id` field)
    if (typeof msg.id === "string" && this.pending.has(msg.id)) {
      const req = this.pending.get(msg.id)!;
      this.pending.delete(msg.id);

      if (msg.error) {
        req.reject(new Error(String(msg.error)));
      } else {
        req.resolve(msg.result);
      }
      return;
    }

    // Server push event (has `event` field, no `id`)
    if (typeof msg.event === "string") {
      const event = msg.event as string;
      const payload = msg.payload;
      const seq = typeof msg.seq === "number" ? msg.seq : undefined;
      const runId = typeof msg.run_id === "string" ? (msg.run_id as string) : undefined;

      // Handle _full_reload (server signals client should reload a run)
      if (event === "_full_reload") {
        const reloadRunId = typeof msg.run_id === "string" ? msg.run_id : undefined;
        if (reloadRunId) {
          dbgWarn("transport", "ws._full_reload", { reloadRunId });
          this.lastSeq.delete(reloadRunId);
          this.needsResubscribe.delete(reloadRunId);
          // Full reload transfers recovery ownership to the history loader. Clear any previous
          // new-session policy so a concurrent subscribe failure cannot lift this suspension.
          this.liveRecoveryRuns.delete(reloadRunId);
          this.suspendedRuns.add(reloadRunId);
          const handlers = this.listeners.get("_full_reload");
          if (handlers) {
            for (const handler of handlers) handler({ run_id: reloadRunId });
          }
        }
        return;
      }

      // Track sequence checkpoint for reconnect replay
      if (seq !== undefined && runId) {
        const prev = this.lastSeq.get(runId) ?? 0;
        if (seq > prev) {
          this.lastSeq.set(runId, seq);
        }
      }

      // Inject _seq into bus-event payloads for session-store tracking
      if (event === "bus-event" && seq !== undefined && payload && typeof payload === "object") {
        (payload as Record<string, unknown>)._seq = seq;
      }

      const handlers = this.listeners.get(event);
      if (handlers) {
        for (const handler of handlers) {
          try {
            handler(payload);
          } catch (e) {
            dbgWarn("transport", "ws.handlerError", { event, error: e });
          }
        }
      }
    }
  }

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    await this.ensureConnected();

    const id = `req_${++this.reqId}`;
    dbg("transport", "ws.invoke", { cmd, id });

    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
      });

      const message = JSON.stringify({
        id,
        method: cmd,
        params: args ?? {},
      });

      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        this.ws.send(message);
      } else {
        this.pending.delete(id);
        reject(new Error("WebSocket not connected"));
      }
    });
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    dbg("transport", "ws.listen", { event });

    let handlers = this.listeners.get(event);
    if (!handlers) {
      handlers = new Set();
      this.listeners.set(event, handlers);
    }

    const typedHandler = handler as (payload: unknown) => void;
    handlers.add(typedHandler);

    // Ensure connection is established for receiving events
    this.ensureConnected().catch((e) => {
      dbgWarn("transport", "ws.listen.connectFailed", { event, error: e });
    });

    return () => {
      const h = this.listeners.get(event);
      if (h) {
        h.delete(typedHandler);
        if (h.size === 0) this.listeners.delete(event);
      }
    };
  }

  isDesktop(): boolean {
    return false;
  }
}

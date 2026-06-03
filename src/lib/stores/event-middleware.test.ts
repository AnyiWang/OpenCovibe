/**
 * EventMiddleware unit tests.
 *
 * Tests routing, microbatching, subscription management, overflow protection,
 * and error isolation using mocked listen() and SessionStore.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { BusEvent } from "$lib/types";

// ── Mocks ──

// Mock transport — captures registered handlers so tests can fire events synchronously
type TransportListenHandler = (payload: unknown) => void;
const _transportListeners = new Map<string, TransportListenHandler>();
const _unlistenSpies: ReturnType<typeof vi.fn>[] = [];

const mockTransport = {
  isDesktop: vi.fn(() => false),
  listen: vi.fn(async (event: string, handler: TransportListenHandler) => {
    _transportListeners.set(event, handler);
    const unlisten = vi.fn();
    _unlistenSpies.push(unlisten);
    return unlisten;
  }),
  subscribeRun: vi.fn(),
  unsubscribeRun: vi.fn(),
  invoke: vi.fn(),
};

vi.mock("$lib/transport", () => ({
  getTransport: () => mockTransport,
}));

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

vi.mock("./attention-store.svelte", () => ({
  markAttention: vi.fn(),
  clearAttention: vi.fn(),
  hasAttention: vi.fn(),
  _resetForTest: vi.fn(),
}));

// Import after mocks
import { EventMiddleware } from "./event-middleware";
import { dbgWarn } from "$lib/utils/debug";
import { markAttention, clearAttention } from "./attention-store.svelte";

// ── Helpers ──

function makeBusEvent(runId: string, type: string, extra: Record<string, unknown> = {}): BusEvent {
  return { type, run_id: runId, ...extra } as unknown as BusEvent;
}

/** Fire a bus-event through the mocked transport listener */
function fireBusEvent(ev: BusEvent): void {
  const handler = _transportListeners.get("bus-event");
  if (!handler) throw new Error("bus-event listener not registered");
  handler(ev);
}

/** Minimal mock of SessionStore with the methods EventMiddleware calls */
function mockStore() {
  return {
    applyEvent: vi.fn(),
    applyEventBatch: vi.fn(),
    applyHookEvent: vi.fn(),
    applyHookUsage: vi.fn(),
    loadRun: vi.fn().mockResolvedValue(undefined),
  };
}

/** Fire a _full_reload event through the mocked transport listener */
function fireFullReload(runId: string): void {
  const handler = _transportListeners.get("_full_reload");
  if (!handler) throw new Error("_full_reload listener not registered");
  handler({ run_id: runId });
}

// ── Tests ──

describe("EventMiddleware", () => {
  let mw: EventMiddleware;

  beforeEach(() => {
    vi.useFakeTimers();
    _transportListeners.clear();
    _unlistenSpies.length = 0;
    mockTransport.subscribeRun.mockClear();
    mockTransport.unsubscribeRun.mockClear();
    mockTransport.listen.mockClear();
    mw = new EventMiddleware();
  });

  afterEach(() => {
    mw.destroy();
    vi.useRealTimers();
  });

  // ── Lifecycle ──

  describe("lifecycle", () => {
    it("registers all 7 listeners on start() (6 core + _full_reload for non-desktop)", async () => {
      await mw.start();
      expect(_transportListeners.size).toBe(7);
      expect(_transportListeners.has("bus-event")).toBe(true);
      expect(_transportListeners.has("chat-delta")).toBe(true);
      expect(_transportListeners.has("chat-done")).toBe(true);
      expect(_transportListeners.has("run-event")).toBe(true);
      expect(_transportListeners.has("hook-event")).toBe(true);
      expect(_transportListeners.has("hook-usage")).toBe(true);
      expect(_transportListeners.has("_full_reload")).toBe(true);
    });

    it("is idempotent — second start() is a no-op", async () => {
      await mw.start();
      const firstCount = _transportListeners.size;
      await mw.start();
      // Should not have doubled listeners
      expect(_transportListeners.size).toBe(firstCount);
    });

    it("destroy() calls all unlisteners and clears state", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      mw.destroy();

      for (const spy of _unlistenSpies) {
        expect(spy).toHaveBeenCalledOnce();
      }
    });
  });

  // ── Routing ──

  describe("bus-event routing", () => {
    it("routes events to subscribed store", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const ev = makeBusEvent("run-1", "message_complete", { message_id: "m1", text: "hi" });
      fireBusEvent(ev);
      vi.advanceTimersByTime(16);

      expect(store.applyEvent).toHaveBeenCalledWith(ev);
    });

    it("silently discards events for unsubscribed run_id", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const ev = makeBusEvent("run-OTHER", "message_complete", { message_id: "m1", text: "hi" });
      fireBusEvent(ev);
      vi.advanceTimersByTime(16);

      expect(store.applyEvent).not.toHaveBeenCalled();
      expect(store.applyEventBatch).not.toHaveBeenCalled();
    });

    it("routes hook-event to subscribed store", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const handler = _transportListeners.get("hook-event")!;
      handler({ run_id: "run-1", hook_type: "PreToolUse", tool_name: "Bash" });

      expect(store.applyHookEvent).toHaveBeenCalledOnce();
    });

    it("routes hook-usage to subscribed store", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const handler = _transportListeners.get("hook-usage")!;
      handler({ run_id: "run-1", input_tokens: 100, output_tokens: 50, cost: 0.01 });

      expect(store.applyHookUsage).toHaveBeenCalledOnce();
    });
  });

  // ── Microbatching ──

  describe("microbatching", () => {
    it("batches multiple events within 16ms into applyEventBatch", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const ev1 = makeBusEvent("run-1", "message_delta", { text: "a" });
      const ev2 = makeBusEvent("run-1", "message_delta", { text: "b" });
      const ev3 = makeBusEvent("run-1", "message_complete", { message_id: "m1", text: "ab" });

      fireBusEvent(ev1);
      fireBusEvent(ev2);
      fireBusEvent(ev3);

      // Before flush
      expect(store.applyEvent).not.toHaveBeenCalled();
      expect(store.applyEventBatch).not.toHaveBeenCalled();

      vi.advanceTimersByTime(16);

      // All 3 events delivered as a single batch
      expect(store.applyEventBatch).toHaveBeenCalledWith([ev1, ev2, ev3]);
      expect(store.applyEvent).not.toHaveBeenCalled();
    });

    it("uses applyEvent for single-event batch", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const ev = makeBusEvent("run-1", "run_state", { state: "running" });
      fireBusEvent(ev);
      vi.advanceTimersByTime(16);

      expect(store.applyEvent).toHaveBeenCalledWith(ev);
      expect(store.applyEventBatch).not.toHaveBeenCalled();
    });
  });

  // ── Subscription management ──

  describe("subscriptions", () => {
    it("subscribeCurrent replaces previous subscription", async () => {
      await mw.start();
      const store1 = mockStore();
      const store2 = mockStore();

      mw.subscribeCurrent("run-1", store1 as any);
      mw.subscribeCurrent("run-2", store2 as any);

      // Event for old run_id should be discarded
      fireBusEvent(makeBusEvent("run-1", "message_delta", { text: "x" }));
      // Event for new run_id should be delivered
      fireBusEvent(makeBusEvent("run-2", "message_delta", { text: "y" }));
      vi.advanceTimersByTime(16);

      expect(store1.applyEvent).not.toHaveBeenCalled();
      expect(store2.applyEvent).toHaveBeenCalledOnce();
    });

    it("unsubscribe clears buffer and prevents delivery", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      // Buffer an event
      fireBusEvent(makeBusEvent("run-1", "message_delta", { text: "x" }));

      // Unsubscribe before flush
      mw.unsubscribe("run-1");
      vi.advanceTimersByTime(16);

      expect(store.applyEvent).not.toHaveBeenCalled();
    });

    it("multi-session subscribe works alongside current", async () => {
      await mw.start();
      const currentStore = mockStore();
      const otherStore = mockStore();

      mw.subscribeCurrent("run-1", currentStore as any);
      mw.subscribe("run-2", otherStore as any);

      fireBusEvent(makeBusEvent("run-1", "message_delta", { text: "a" }));
      fireBusEvent(makeBusEvent("run-2", "message_delta", { text: "b" }));
      vi.advanceTimersByTime(16);

      expect(currentStore.applyEvent).toHaveBeenCalledOnce();
      expect(otherStore.applyEvent).toHaveBeenCalledOnce();
    });
  });

  // ── Error isolation ──

  describe("flush error isolation", () => {
    it("applyEventBatch error does not prevent other runs from flushing", async () => {
      await mw.start();
      const failStore = mockStore();
      const okStore = mockStore();

      failStore.applyEventBatch.mockImplementation(() => {
        throw new Error("reducer crashed");
      });

      mw.subscribeCurrent("run-1", failStore as any);
      mw.subscribe("run-2", okStore as any);

      // Buffer events for both
      fireBusEvent(makeBusEvent("run-1", "message_delta", { text: "a" }));
      fireBusEvent(makeBusEvent("run-1", "message_delta", { text: "b" }));
      fireBusEvent(makeBusEvent("run-2", "message_delta", { text: "c" }));

      vi.advanceTimersByTime(16);

      // Failing store was called (and threw)
      expect(failStore.applyEventBatch).toHaveBeenCalledOnce();
      // OK store still got its event delivered
      expect(okStore.applyEvent).toHaveBeenCalledOnce();
      // Warning was logged
      expect(dbgWarn).toHaveBeenCalledWith(
        "middleware",
        expect.stringContaining("flush error for run run-1"),
        expect.any(Error),
      );
    });
  });

  // ── Buffer overflow ──

  describe("buffer overflow protection", () => {
    it("flushes synchronously when buffer exceeds MAX_BUFFER_SIZE", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      // Fire 500 events (= MAX_BUFFER_SIZE)
      for (let i = 0; i < 500; i++) {
        fireBusEvent(makeBusEvent("run-1", "message_delta", { text: `chunk-${i}` }));
      }

      // Should have flushed synchronously — no need to advance timers
      expect(store.applyEventBatch).toHaveBeenCalledOnce();
      expect(store.applyEventBatch.mock.calls[0][0]).toHaveLength(500);
      expect(dbgWarn).toHaveBeenCalledWith(
        "middleware",
        expect.stringContaining("buffer overflow"),
      );
    });
  });

  // ── Handler routing (Pipe) ──

  describe("handler routing", () => {
    it("routes chat-delta to pipe handler", async () => {
      await mw.start();
      const onDelta = vi.fn();
      mw.setPipeHandler({ onDelta, onDone: vi.fn() });

      const handler = _transportListeners.get("chat-delta")!;
      handler({ text: "hello" });

      expect(onDelta).toHaveBeenCalledWith({ text: "hello" });
    });

    it("no-op when pipe handler is null", async () => {
      await mw.start();
      // Don't set any handlers — should not throw
      const handler = _transportListeners.get("chat-delta")!;
      expect(() => handler({ text: "x" })).not.toThrow();
    });
  });

  // ── Partial degradation ──

  describe("partial degradation on listener failure", () => {
    it("continues registering other listeners if one fails", async () => {
      let callCount = 0;
      mockTransport.listen.mockImplementation(async (name: string, handler: any) => {
        callCount++;
        if (callCount === 3) {
          // Fail the 3rd listener registration
          throw new Error("listen failed for chat-done");
        }
        _transportListeners.set(name, handler);
        const unlisten = vi.fn();
        _unlistenSpies.push(unlisten);
        return unlisten;
      });

      await mw.start();

      // Should have 6 listeners (7 total including _full_reload, minus 1 failed)
      expect(_transportListeners.size).toBe(6);
      expect(dbgWarn).toHaveBeenCalledWith(
        "middleware",
        expect.stringContaining("failed to register listener"),
        expect.any(Error),
      );
    });
  });

  // ── Attention tracking ──

  describe("attention tracking", () => {
    const markMock = vi.mocked(markAttention);
    const clearMock = vi.mocked(clearAttention);

    beforeEach(() => {
      markMock.mockClear();
      clearMock.mockClear();
    });

    it("tracks attention for unsubscribed runs", async () => {
      await mw.start();
      // No subscription for "run-other"
      fireBusEvent(makeBusEvent("run-other", "permission_prompt", { request_id: "r1" }));
      expect(markMock).toHaveBeenCalledWith("run-other", "permission");
    });

    it("permission_prompt → markAttention('permission')", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);
      fireBusEvent(makeBusEvent("run-1", "permission_prompt", { request_id: "r1" }));
      expect(markMock).toHaveBeenCalledWith("run-1", "permission");
    });

    it("tool_end(AskUserQuestion, error) → markAttention('ask')", async () => {
      await mw.start();
      fireBusEvent(
        makeBusEvent("run-1", "tool_end", {
          tool_use_id: "t1",
          tool_name: "AskUserQuestion",
          status: "error",
          output: {},
        }),
      );
      expect(markMock).toHaveBeenCalledWith("run-1", "ask");
    });

    it("tool_end for other tool does not trigger mark", async () => {
      await mw.start();
      fireBusEvent(
        makeBusEvent("run-1", "tool_end", {
          tool_use_id: "t1",
          tool_name: "Bash",
          status: "error",
          output: {},
        }),
      );
      expect(markMock).not.toHaveBeenCalled();
    });

    it("permission_denied → clearAttention('permission')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "permission_denied", { tool_use_id: "t1" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "permission");
    });

    it("permission_denied(AskUserQuestion) → clears both permission and ask", async () => {
      await mw.start();
      fireBusEvent(
        makeBusEvent("run-1", "permission_denied", {
          tool_use_id: "t1",
          tool_name: "AskUserQuestion",
        }),
      );
      expect(clearMock).toHaveBeenCalledWith("run-1", "permission");
      expect(clearMock).toHaveBeenCalledWith("run-1", "ask");
    });

    it("control_cancelled → clearAttention('permission')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "control_cancelled", { request_id: "r1" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "permission");
    });

    it("run_state(spawning) → clearAttention('permission')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "run_state", { state: "spawning" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "permission");
    });

    it("run_state(idle) → clearAttention('permission')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "run_state", { state: "idle" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "permission");
    });

    it("run_state(running) → clearAttention('ask')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "run_state", { state: "running" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "ask");
    });

    it("run_state(stopped) → clearAttention() (all)", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "run_state", { state: "stopped" }));
      expect(clearMock).toHaveBeenCalledWith("run-1");
    });

    it("user_message → clearAttention('ask')", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "user_message", { text: "hello" }));
      expect(clearMock).toHaveBeenCalledWith("run-1", "ask");
    });

    it("run_state with unknown value does not call clearAttention", async () => {
      await mw.start();
      fireBusEvent(makeBusEvent("run-1", "run_state", { state: "some_future_state" }));
      expect(clearMock).not.toHaveBeenCalled();
    });
  });

  // ── _full_reload protocol ──

  describe("_full_reload protocol (WS-only)", () => {
    it("registers _full_reload listener via transport.listen on start()", async () => {
      await mw.start();
      expect(mockTransport.listen).toHaveBeenCalledWith("_full_reload", expect.any(Function));
      expect(_transportListeners.has("_full_reload")).toBe(true);
    });

    it("triggers store.loadRun on receiving _full_reload", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      fireFullReload("run-1");

      expect(store.loadRun).toHaveBeenCalledWith("run-1");
      expect(store.loadRun).toHaveBeenCalledTimes(1);
    });

    it("does not call loadRun for unsubscribed run_id", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      fireFullReload("run-OTHER");

      expect(store.loadRun).not.toHaveBeenCalled();
    });

    it("debounces consecutive _full_reload for same run_id", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      fireFullReload("run-1");
      expect(store.loadRun).toHaveBeenCalledTimes(1);

      // Second fire while first is still in-flight → debounced
      fireFullReload("run-1");
      expect(store.loadRun).toHaveBeenCalledTimes(1);

      // Flush the promise (.finally clears _reloadingRuns)
      await Promise.resolve();

      // Now debounce is cleared, should accept again
      fireFullReload("run-1");
      expect(store.loadRun).toHaveBeenCalledTimes(2);
    });

    it("handles different run_ids independently (no cross-debounce)", async () => {
      await mw.start();
      const store1 = mockStore();
      const store2 = mockStore();
      mw.subscribeCurrent("run-1", store1 as any);
      mw.subscribe("run-2", store2 as any);

      fireFullReload("run-1");
      fireFullReload("run-2");

      expect(store1.loadRun).toHaveBeenCalledTimes(1);
      expect(store2.loadRun).toHaveBeenCalledTimes(1);
    });

    it("destroy() clears _full_reload unlisten", async () => {
      await mw.start();
      const transportUnlisten = _unlistenSpies[_unlistenSpies.length - 1]; // last pushed
      mw.destroy();
      expect(transportUnlisten).toHaveBeenCalledOnce();
    });
  });

  // ── Transport subscription contract ──

  describe("transport subscription contract", () => {
    it("subscribeCurrent does NOT call transport.subscribeRun", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);
      expect(mockTransport.subscribeRun).not.toHaveBeenCalled();
    });

    it("subscribe does NOT call transport.subscribeRun", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribe("run-1", store as any);
      expect(mockTransport.subscribeRun).not.toHaveBeenCalled();
    });

    it("unsubscribe calls transport.unsubscribeRun", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);
      mockTransport.unsubscribeRun.mockClear();
      mw.unsubscribe("run-1");
      expect(mockTransport.unsubscribeRun).toHaveBeenCalledWith("run-1");
    });
  });

  // ── Idle-aware flush ──
  //
  // Under fake timers: performance.now() starts at 0 and advances with
  // advanceTimersByTime; requestAnimationFrame is undefined, so the
  // non-idle branch falls through to setTimeout(_BATCH_INTERVAL).
  describe("idle-aware flush", () => {
    it("flushes the first token after an idle gap via microtask (no timer wait)", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      // Prime: flush an initial event so _lastFlushTime is recorded.
      fireBusEvent(makeBusEvent("run-1", "message_complete", { message_id: "m0", text: "a" }));
      vi.advanceTimersByTime(16);
      expect(store.applyEvent).toHaveBeenCalledTimes(1);

      // Idle for >_IDLE_GAP_MS, then a new burst arrives.
      vi.advanceTimersByTime(150);
      fireBusEvent(makeBusEvent("run-1", "message_complete", { message_id: "m1", text: "b" }));

      // Microtask path: resolves without advancing timers (no rAF/setTimeout wait).
      await Promise.resolve();
      expect(store.applyEvent).toHaveBeenCalledTimes(2);
    });

    it("batches consecutive tokens within the idle gap (timer path, not microtask)", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      // Prime a flush to set _lastFlushTime.
      fireBusEvent(makeBusEvent("run-1", "message_complete", { message_id: "m0", text: "a" }));
      vi.advanceTimersByTime(16);
      store.applyEvent.mockClear();
      store.applyEventBatch.mockClear();

      // Two tokens arrive <_IDLE_GAP_MS after the last flush → non-idle branch.
      vi.advanceTimersByTime(10);
      fireBusEvent(makeBusEvent("run-1", "message_complete", { message_id: "m1", text: "b" }));
      fireBusEvent(makeBusEvent("run-1", "message_complete", { message_id: "m2", text: "c" }));

      // Microtask alone must NOT flush (proves it took the timer path).
      await Promise.resolve();
      expect(store.applyEvent).not.toHaveBeenCalled();
      expect(store.applyEventBatch).not.toHaveBeenCalled();

      // Timer fires → batched flush.
      vi.advanceTimersByTime(16);
      expect(store.applyEventBatch).toHaveBeenCalledTimes(1);
    });
  });
});

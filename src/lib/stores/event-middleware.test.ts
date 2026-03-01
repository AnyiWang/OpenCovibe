/**
 * EventMiddleware unit tests.
 *
 * Tests routing, microbatching, subscription management, overflow protection,
 * and error isolation using mocked listen() and SessionStore.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { BusEvent } from "$lib/types";

// ── Mocks ──

// Mock Tauri event listener — captures registered handlers so tests can fire events synchronously
type ListenHandler = (event: { payload: unknown }) => void;
const _listeners = new Map<string, ListenHandler>();
const _unlistenSpies: ReturnType<typeof vi.fn>[] = [];

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: ListenHandler) => {
    _listeners.set(name, handler);
    const unlisten = vi.fn();
    _unlistenSpies.push(unlisten);
    return unlisten;
  }),
}));

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

// Import after mocks
import { EventMiddleware } from "./event-middleware";
import { dbgWarn } from "$lib/utils/debug";

// ── Helpers ──

function makeBusEvent(runId: string, type: string, extra: Record<string, unknown> = {}): BusEvent {
  return { type, run_id: runId, ...extra } as unknown as BusEvent;
}

/** Fire a bus-event through the mocked listener */
function fireBusEvent(ev: BusEvent): void {
  const handler = _listeners.get("bus-event");
  if (!handler) throw new Error("bus-event listener not registered");
  handler({ payload: ev });
}

/** Minimal mock of SessionStore with the methods EventMiddleware calls */
function mockStore() {
  return {
    applyEvent: vi.fn(),
    applyEventBatch: vi.fn(),
    applyHookEvent: vi.fn(),
    applyHookUsage: vi.fn(),
  };
}

// ── Tests ──

describe("EventMiddleware", () => {
  let mw: EventMiddleware;

  beforeEach(() => {
    vi.useFakeTimers();
    _listeners.clear();
    _unlistenSpies.length = 0;
    mw = new EventMiddleware();
  });

  afterEach(() => {
    mw.destroy();
    vi.useRealTimers();
  });

  // ── Lifecycle ──

  describe("lifecycle", () => {
    it("registers all 8 listeners on start()", async () => {
      await mw.start();
      expect(_listeners.size).toBe(8);
      expect(_listeners.has("bus-event")).toBe(true);
      expect(_listeners.has("pty-output")).toBe(true);
      expect(_listeners.has("pty-exit")).toBe(true);
      expect(_listeners.has("chat-delta")).toBe(true);
      expect(_listeners.has("chat-done")).toBe(true);
      expect(_listeners.has("run-event")).toBe(true);
      expect(_listeners.has("hook-event")).toBe(true);
      expect(_listeners.has("hook-usage")).toBe(true);
    });

    it("is idempotent — second start() is a no-op", async () => {
      await mw.start();
      const firstCount = _listeners.size;
      await mw.start();
      // Should not have doubled listeners
      expect(_listeners.size).toBe(firstCount);
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

      const handler = _listeners.get("hook-event")!;
      handler({ payload: { run_id: "run-1", hook_type: "PreToolUse", tool_name: "Bash" } });

      expect(store.applyHookEvent).toHaveBeenCalledOnce();
    });

    it("routes hook-usage to subscribed store", async () => {
      await mw.start();
      const store = mockStore();
      mw.subscribeCurrent("run-1", store as any);

      const handler = _listeners.get("hook-usage")!;
      handler({ payload: { run_id: "run-1", input_tokens: 100, output_tokens: 50, cost: 0.01 } });

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

  // ── Handler routing (PTY, Pipe, Permission) ──

  describe("handler routing", () => {
    it("routes pty-output to PTY handler", async () => {
      await mw.start();
      const onOutput = vi.fn();
      mw.setPtyHandler({ onOutput, onExit: vi.fn() });

      const handler = _listeners.get("pty-output")!;
      handler({ payload: { run_id: "run-1", data: "base64data" } });

      expect(onOutput).toHaveBeenCalledWith({ run_id: "run-1", data: "base64data" });
    });

    it("routes chat-delta to pipe handler", async () => {
      await mw.start();
      const onDelta = vi.fn();
      mw.setPipeHandler({ onDelta, onDone: vi.fn() });

      const handler = _listeners.get("chat-delta")!;
      handler({ payload: { text: "hello" } });

      expect(onDelta).toHaveBeenCalledWith({ text: "hello" });
    });

    it("no-op when handler is null", async () => {
      await mw.start();
      // Don't set any handlers — should not throw
      const handler = _listeners.get("pty-output")!;
      expect(() => handler({ payload: { run_id: "run-1", data: "x" } })).not.toThrow();
    });
  });

  // ── Partial degradation ──

  describe("partial degradation on listener failure", () => {
    it("continues registering other listeners if one fails", async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const listenMock = vi.mocked(listen);
      let callCount = 0;
      listenMock.mockImplementation(async (name: string, handler: any) => {
        callCount++;
        if (callCount === 3) {
          // Fail the 3rd listener registration
          throw new Error("listen failed for pty-exit");
        }
        _listeners.set(name, handler);
        const unlisten = vi.fn();
        _unlistenSpies.push(unlisten);
        return unlisten;
      });

      await mw.start();

      // Should have 7 listeners (8 - 1 failed)
      expect(_listeners.size).toBe(7);
      expect(dbgWarn).toHaveBeenCalledWith(
        "middleware",
        expect.stringContaining("failed to register listener"),
        expect.any(Error),
      );
    });
  });
});

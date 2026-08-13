import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/utils/debug", () => ({ dbg: vi.fn(), dbgWarn: vi.fn() }));

import { WsTransport } from "./websocket";

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];
  static emitCloseOnClose = true;

  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;

  constructor(readonly url: string) {
    FakeWebSocket.instances.push(this);
  }

  open(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.({} as Event);
  }

  send(data: string): void {
    if (this.readyState !== FakeWebSocket.OPEN) throw new Error("socket is not open");
    this.sent.push(data);
  }

  close(): void {
    this.readyState = FakeWebSocket.CLOSED;
    if (FakeWebSocket.emitCloseOnClose) this.emitClose();
  }

  emitClose(code = 1006): void {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.({ code, reason: "test close" } as CloseEvent);
  }

  respond(index: number, response: Record<string, unknown> = { result: null }): void {
    const request = JSON.parse(this.sent[index]) as { id: string };
    this.onmessage?.({ data: JSON.stringify({ id: request.id, ...response }) } as MessageEvent);
  }

  push(message: Record<string, unknown>): void {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }

  requests(method: string): Array<Record<string, unknown>> {
    return this.sent
      .map((line) => JSON.parse(line) as Record<string, unknown>)
      .filter((request) => request.method === method);
  }
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe("WsTransport lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeWebSocket.instances = [];
    FakeWebSocket.emitCloseOnClose = true;
    vi.stubGlobal("window", {
      location: { protocol: "http:", host: "localhost:1420", href: "http://localhost:1420/" },
    });
    vi.stubGlobal("WebSocket", FakeWebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("rejects a connection that closes before open", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-1", 4);
    const socket = FakeWebSocket.instances[0];

    socket.emitClose(1006);

    await expect(subscribing).rejects.toThrow("closed before connection opened");
  });

  it("settles a timed-out attempt and ignores its delayed close after a new connection opens", async () => {
    FakeWebSocket.emitCloseOnClose = false;
    const transport = new WsTransport();
    const first = transport.subscribeRun("run-1", 4);
    const firstResult = expect(first).rejects.toThrow("connection timeout");
    const oldSocket = FakeWebSocket.instances[0];

    await vi.advanceTimersByTimeAsync(10_000);
    await firstResult;

    const second = transport.subscribeRun("run-1", 4);
    const newSocket = FakeWebSocket.instances[1];
    newSocket.open();
    await flushPromises();
    oldSocket.emitClose(1006);
    newSocket.respond(0);

    await expect(second).resolves.toBeUndefined();
    expect(newSocket.requests("_subscribe")).toHaveLength(1);
  });

  it("keeps subscription intent across repeated automatic resubscribe failures", async () => {
    const transport = new WsTransport();
    const initial = transport.subscribeRun("run-1", 9);
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();
    firstSocket.respond(0);
    await initial;

    firstSocket.emitClose(1006);
    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    secondSocket.respond(0, { error: "temporary replay failure" });
    await flushPromises();

    await vi.advanceTimersByTimeAsync(1_000);
    const thirdSocket = FakeWebSocket.instances[2];
    thirdSocket.open();
    await flushPromises();
    expect(thirdSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-1", last_seq: 9 } }),
    ]);
    thirdSocket.respond(0, { error: "another temporary replay failure" });
    await flushPromises();

    await vi.advanceTimersByTimeAsync(1_000);
    const fourthSocket = FakeWebSocket.instances[3];
    fourthSocket.open();
    await flushPromises();
    expect(fourthSocket.requests("_subscribe")).toHaveLength(1);
    fourthSocket.respond(0);
    await flushPromises();
    fourthSocket.push({
      event: "bus-event",
      run_id: "run-1",
      seq: 12,
      payload: { type: "message_delta", run_id: "run-1", text: "new" },
    });

    fourthSocket.emitClose(1006);
    await vi.advanceTimersByTimeAsync(1_000);
    const fifthSocket = FakeWebSocket.instances[4];
    fifthSocket.open();
    await flushPromises();
    expect(fifthSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-1", last_seq: 12 } }),
    ]);
  });

  it("keeps a failed explicit subscription suspended until its caller retries", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-1", 7);
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();
    firstSocket.respond(0, { error: "history unavailable" });
    await expect(subscribing).rejects.toThrow("history unavailable");

    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toHaveLength(0);

    const retry = transport.subscribeRun("run-1", 8);
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-1", last_seq: 8 } }),
    ]);
    secondSocket.respond(0);
    await expect(retry).resolves.toBeUndefined();
  });

  it("automatically retries a new-session subscription after its initial connection fails", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-new", 0, "live");
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.emitClose(1006);
    await expect(subscribing).rejects.toThrow("closed before connection opened");

    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 0 } }),
    ]);
    secondSocket.respond(0);
    await flushPromises();

    secondSocket.push({
      event: "bus-event",
      run_id: "run-new",
      seq: 2,
      payload: { type: "run_state", run_id: "run-new", state: "running" },
    });
    secondSocket.emitClose(1006);
    await vi.advanceTimersByTimeAsync(1_000);
    const thirdSocket = FakeWebSocket.instances[2];
    thirdSocket.open();
    await flushPromises();
    expect(thirdSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 2 } }),
    ]);
  });

  it("automatically retries a new-session subscription after its acknowledgement fails", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-new", 0, "live");
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();
    firstSocket.respond(0, { error: "temporary subscribe failure" });
    await expect(subscribing).rejects.toThrow("temporary subscribe failure");

    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 0 } }),
    ]);
    secondSocket.respond(0, { error: "another temporary subscribe failure" });
    await flushPromises();

    await vi.advanceTimersByTimeAsync(1_000);
    const thirdSocket = FakeWebSocket.instances[2];
    thirdSocket.open();
    await flushPromises();
    expect(thirdSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 0 } }),
    ]);
    thirdSocket.respond(0);
    await flushPromises();
  });

  it("keeps full-reload suspension when a pending live subscription fails", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-new", 0, "live");
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();

    firstSocket.push({ event: "_full_reload", run_id: "run-new" });
    firstSocket.respond(0, { error: "replay requires history reload" });
    await expect(subscribing).rejects.toThrow("replay requires history reload");

    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toHaveLength(0);

    const historyRetry = transport.subscribeRun("run-new", 8);
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 8 } }),
    ]);
    secondSocket.respond(0);
    await expect(historyRetry).resolves.toBeUndefined();
  });

  it("switches a live subscription to fail-closed history recovery", async () => {
    const transport = new WsTransport();
    const liveSubscribe = transport.subscribeRun("run-new", 0, "live");
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();
    firstSocket.respond(0);
    await expect(liveSubscribe).resolves.toBeUndefined();

    const historySubscribe = transport.subscribeRun("run-new", 8);
    await flushPromises();
    firstSocket.respond(1, { error: "history checkpoint rejected" });
    await expect(historySubscribe).rejects.toThrow("history checkpoint rejected");

    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toHaveLength(0);
  });

  it("automatically retries a new-session subscription after its acknowledgement times out", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-new", 0, "live");
    const result = expect(subscribing).rejects.toThrow("request timed out");
    const firstSocket = FakeWebSocket.instances[0];
    firstSocket.open();
    await flushPromises();

    await vi.advanceTimersByTimeAsync(15_000);
    await result;
    await vi.advanceTimersByTimeAsync(1_000);
    const secondSocket = FakeWebSocket.instances[1];
    secondSocket.open();
    await flushPromises();
    expect(secondSocket.requests("_subscribe")).toEqual([
      expect.objectContaining({ params: { run_id: "run-new", last_seq: 0 } }),
    ]);
  });

  it("does not revive a run unsubscribed while its acknowledgement is pending", async () => {
    const transport = new WsTransport();
    const subscribing = transport.subscribeRun("run-1", 5);
    const socket = FakeWebSocket.instances[0];
    socket.open();
    await flushPromises();
    transport.unsubscribeRun("run-1");
    socket.respond(0);

    await expect(subscribing).rejects.toThrow("subscription cancelled");
    socket.emitClose(1006);
    await vi.advanceTimersByTimeAsync(1_000);
    const reconnected = FakeWebSocket.instances[1];
    reconnected.open();
    await flushPromises();
    expect(reconnected.requests("_subscribe")).toHaveLength(0);
  });
});

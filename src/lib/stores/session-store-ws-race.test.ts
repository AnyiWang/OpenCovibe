import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getUserSettings: vi.fn(),
  getAgentSettings: vi.fn(),
  startRun: vi.fn(),
  getRun: vi.fn(),
  getHistorySummary: vi.fn(),
  getHistoryPage: vi.fn(),
  getBusEventsPage: vi.fn(),
  syncCliSession: vi.fn(),
  startSession: vi.fn(),
  sendChatMessage: vi.fn(),
  forkSession: vi.fn(),
}));

const transportMocks = vi.hoisted(() => ({
  subscribeRun: vi.fn(),
  unsubscribeRun: vi.fn(),
  isDesktop: vi.fn(() => false),
}));

const middlewareMocks = vi.hoisted(() => {
  const state = {
    currentRunId: null as string | null,
    currentStore: null as unknown,
  };
  return {
    state,
    beginHistoryLoad: vi.fn(() => 1),
    finishHistoryLoad: vi.fn(),
    cancelHistoryLoad: vi.fn(),
    subscribeCurrent: vi.fn((runId: string, store: unknown) => {
      state.currentRunId = runId || null;
      state.currentStore = runId ? store : null;
    }),
  };
});

vi.mock("$lib/api", () => ({
  ...apiMocks,
  getBusEvents: vi.fn(),
  getRunEvents: vi.fn(),
  sendSessionMessage: vi.fn(),
  stopSession: vi.fn(),
  stopRun: vi.fn(),
  sendSessionControl: vi.fn(),
  respondUserInput: vi.fn(),
  steerSession: vi.fn(),
  renameRun: vi.fn(),
}));

vi.mock("$lib/transport", () => ({ getTransport: () => transportMocks }));
vi.mock("./event-middleware", () => ({
  getEventMiddleware: () => ({
    ...middlewareMocks,
    get currentRunId() {
      return middlewareMocks.state.currentRunId;
    },
    get currentStore() {
      return middlewareMocks.state.currentStore;
    },
  }),
}));
vi.mock("$lib/utils/debug", () => ({ dbg: vi.fn(), dbgWarn: vi.fn() }));
vi.mock("$lib/utils/snapshot-cache", () => ({
  readSnapshot: vi.fn().mockResolvedValue(null),
  writeSnapshot: vi.fn().mockResolvedValue(undefined),
  deleteSnapshot: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("./cli-info.svelte", () => ({
  updateInstalledVersion: vi.fn(),
  getCliCommands: vi.fn(() => []),
}));

import { SessionStore } from "./session-store.svelte";

function currentRunId(store: SessionStore): string | undefined {
  return (store.run as { id: string } | null)?.id;
}

function deferred(): {
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: Error) => void;
} {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function deferredValue<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: Error) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function run(id: string, status = "running") {
  return {
    id,
    prompt: "test",
    cwd: "/tmp",
    agent: "claude",
    auth_mode: "cli",
    status,
    started_at: "2026-08-13T00:00:00Z",
    execution_path: "session_actor",
    session_id: `session-${id}`,
  };
}

function history(runId: string) {
  return {
    summary: {
      runId,
      generationId: `gen-${runId}`,
      pageCount: 1,
      totalEntries: 0,
      totalTurns: 0,
      lastSeq: 4,
      sourceSize: 100,
      sourceMtimeNs: 1,
      latestCursor: `gen-${runId}:1`,
      stateEvents: [],
    },
    page: {
      runId,
      generationId: `gen-${runId}`,
      entries: [],
      pageCursor: `gen-${runId}:1`,
      hasMore: false,
      firstSeq: 0,
      lastSeq: 4,
    },
  };
}

describe("SessionStore WebSocket stale guards", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { location: { href: "http://localhost/" } });
    vi.clearAllMocks();
    middlewareMocks.state.currentRunId = null;
    middlewareMocks.state.currentStore = null;
    apiMocks.getRun.mockImplementation(async (runId: string) => run(runId));
    apiMocks.getHistorySummary.mockImplementation(async (runId: string) => history(runId).summary);
    apiMocks.getHistoryPage.mockImplementation(async (runId: string) => history(runId).page);
    apiMocks.getBusEventsPage.mockResolvedValue({
      events: [],
      lastSeq: 4,
      hasMore: false,
      nextOffset: 100,
    });
    apiMocks.syncCliSession.mockResolvedValue({ newEvents: 0 });
    apiMocks.getUserSettings.mockResolvedValue({ auth_mode: "cli" });
    apiMocks.getAgentSettings.mockResolvedValue({});
    apiMocks.startRun.mockImplementation(async () => run("run-new", "pending"));
    apiMocks.startSession.mockResolvedValue(undefined);
    apiMocks.sendChatMessage.mockResolvedValue(undefined);
  });

  it("does not finish an old history load after navigation during WS subscribe", async () => {
    const pending = deferred();
    transportMocks.subscribeRun.mockReturnValueOnce(pending.promise);
    const store = new SessionStore();
    middlewareMocks.subscribeCurrent("run-a", store);
    const loading = store.loadRun("run-a");
    await vi.waitFor(() => expect(transportMocks.subscribeRun).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("", store);
    await store.loadRun("");
    pending.resolve();

    await expect(loading).resolves.toBe(false);
    expect(middlewareMocks.finishHistoryLoad).not.toHaveBeenCalled();
    expect(middlewareMocks.cancelHistoryLoad).toHaveBeenCalledWith("run-a", store, 1);
    expect(transportMocks.unsubscribeRun).toHaveBeenCalledWith("run-a");
  });

  it("does not start the CLI after a resume subscription becomes stale", async () => {
    const pending = deferred();
    transportMocks.subscribeRun.mockReturnValueOnce(pending.promise);
    apiMocks.getRun.mockResolvedValue(run("run-a", "stopped"));
    const store = new SessionStore();
    store.run = run("run-a", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-a", store);
    const resuming = store.resumeSession("run-a", "resume");
    await vi.waitFor(() => expect(transportMocks.subscribeRun).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("", store);
    await store.loadRun("");
    pending.resolve();

    await expect(resuming).resolves.toEqual({ status: "cancelled" });
    expect(apiMocks.startSession).not.toHaveBeenCalled();
    expect(transportMocks.unsubscribeRun).toHaveBeenCalledWith("run-a");
  });

  it("cancels resume when real route ownership changes without an explicit cancel", async () => {
    const pending = deferred();
    transportMocks.subscribeRun.mockReturnValueOnce(pending.promise).mockResolvedValue(undefined);
    apiMocks.getRun.mockImplementation(async (runId: string) =>
      run(runId, runId === "run-a" ? "stopped" : "running"),
    );
    const store = new SessionStore();
    store.run = run("run-a", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-a", store);
    const resuming = store.resumeSession("run-a", "resume");
    await vi.waitFor(() => expect(transportMocks.subscribeRun).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    pending.resolve();

    await expect(resuming).resolves.toEqual({ status: "cancelled" });
    await expect(loadingB).resolves.toBe(true);
    expect(currentRunId(store)).toBe("run-b");
    expect(apiMocks.startSession).not.toHaveBeenCalled();
  });

  it("does not return or retain a fork whose WS subscription resolves after cancellation", async () => {
    const forkSubscribe = deferred();
    transportMocks.subscribeRun
      .mockResolvedValueOnce(undefined)
      .mockReturnValueOnce(forkSubscribe.promise);
    apiMocks.getRun
      .mockResolvedValueOnce(run("run-source", "stopped"))
      .mockResolvedValueOnce(run("run-fork", "pending"));
    apiMocks.forkSession.mockResolvedValue("run-fork");
    const store = new SessionStore();
    store.run = run("run-source", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-source", store);
    const forking = store.resumeSession("run-source", "fork");
    await vi.waitFor(() => expect(transportMocks.subscribeRun).toHaveBeenCalledTimes(2));

    store.cancelResumeOperation();
    middlewareMocks.subscribeCurrent("", store);
    forkSubscribe.resolve();

    await expect(forking).resolves.toEqual({ status: "cancelled" });
    expect(transportMocks.unsubscribeRun).toHaveBeenCalledWith("run-fork");
    expect(apiMocks.startSession).not.toHaveBeenCalled();
  });

  it("does not return the source run when fork history becomes stale before WS subscribe", async () => {
    let resolveForkSummary!: (value: ReturnType<typeof history>["summary"]) => void;
    apiMocks.getRun
      .mockResolvedValueOnce(run("run-source", "stopped"))
      .mockResolvedValueOnce(run("run-fork", "pending"));
    apiMocks.getHistorySummary
      .mockResolvedValueOnce(history("run-source").summary)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveForkSummary = resolve;
          }),
      );
    apiMocks.forkSession.mockResolvedValue("run-fork");
    transportMocks.subscribeRun.mockResolvedValue(undefined);
    const store = new SessionStore();
    store.run = run("run-source", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-source", store);
    const forking = store.resumeSession("run-source", "fork");
    await vi.waitFor(() => expect(apiMocks.getHistorySummary).toHaveBeenCalledTimes(2));

    store.cancelResumeOperation();
    middlewareMocks.subscribeCurrent("", store);
    resolveForkSummary(history("run-fork").summary);

    await expect(forking).resolves.toEqual({ status: "cancelled" });
    expect(transportMocks.subscribeRun).toHaveBeenCalledTimes(1);
    expect(apiMocks.startSession).not.toHaveBeenCalled();
  });

  it("reports cancellation when resume history becomes stale before WS subscribe", async () => {
    let resolveSummary!: (value: ReturnType<typeof history>["summary"]) => void;
    apiMocks.getHistorySummary.mockImplementationOnce(
      (runId: string) =>
        new Promise((resolve) => {
          resolveSummary = resolve;
          expect(runId).toBe("run-a");
        }),
    );
    apiMocks.getRun.mockResolvedValue(run("run-a", "stopped"));
    const store = new SessionStore();
    store.run = run("run-a", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-a", store);
    const resuming = store.resumeSession("run-a", "resume");
    await vi.waitFor(() => expect(apiMocks.getHistorySummary).toHaveBeenCalled());

    store.cancelResumeOperation();
    middlewareMocks.subscribeCurrent("", store);
    resolveSummary(history("run-a").summary);

    await expect(resuming).resolves.toEqual({ status: "cancelled" });
    expect(transportMocks.subscribeRun).not.toHaveBeenCalled();
    expect(apiMocks.startSession).not.toHaveBeenCalled();
  });

  it("reports cancellation when the latest history page resolves after navigation", async () => {
    let resolvePage!: (value: ReturnType<typeof history>["page"]) => void;
    apiMocks.getHistoryPage.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolvePage = resolve;
        }),
    );
    apiMocks.getRun.mockResolvedValue(run("run-a", "stopped"));
    const store = new SessionStore();
    store.run = run("run-a", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-a", store);
    const resuming = store.resumeSession("run-a", "resume");
    await vi.waitFor(() => expect(apiMocks.getHistoryPage).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("", store);
    await store.loadRun("");
    resolvePage(history("run-a").page);

    await expect(resuming).resolves.toEqual({ status: "cancelled" });
    expect(transportMocks.subscribeRun).not.toHaveBeenCalled();
    expect(apiMocks.startSession).not.toHaveBeenCalled();
  });

  it("does not report success or install timers after CLI spawn resolves stale", async () => {
    const spawn = deferred();
    apiMocks.getRun.mockResolvedValue(run("run-a", "stopped"));
    transportMocks.subscribeRun.mockResolvedValue(undefined);
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    store.run = run("run-a", "stopped") as never;
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-a", store);
    const resuming = store.resumeSession("run-a", "resume", "continue");
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    store.cancelResumeOperation();
    middlewareMocks.subscribeCurrent("", store);
    await store.loadRun("");
    spawn.resolve();

    await expect(resuming).resolves.toEqual({ status: "cancelled" });
    expect(store.phase).toBe("empty");
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
    expect((store as unknown as { _responseTimer: unknown })._responseTimer).toBeNull();
  });

  it("does not install a fork step-two timer after its CLI spawn resolves stale", async () => {
    const spawn = deferred();
    transportMocks.subscribeRun.mockResolvedValue(undefined);
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    store.run = run("run-fork", "pending") as never;
    store.phase = "spawning";
    middlewareMocks.subscribeCurrent("run-fork", store);
    const connecting = store.connectSession("run-fork");
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("", store);
    await store.loadRun("");
    spawn.resolve();

    await expect(connecting).resolves.toBeUndefined();
    expect(store.phase).toBe("empty");
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
    expect((store as unknown as { _responseTimer: unknown })._responseTimer).toBeNull();
  });

  it("cancels fork step two when route ownership changes without an explicit cancel", async () => {
    const spawn = deferred();
    transportMocks.subscribeRun.mockResolvedValue(undefined);
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    apiMocks.getRun.mockImplementation(async (runId: string) => run(runId));
    const store = new SessionStore();
    store.run = run("run-fork", "pending") as never;
    store.phase = "spawning";
    middlewareMocks.subscribeCurrent("run-fork", store);
    const connecting = store.connectSession("run-fork");
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    spawn.resolve();

    await expect(connecting).resolves.toBeUndefined();
    await expect(loadingB).resolves.toBe(true);
    expect(currentRunId(store)).toBe("run-b");
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
  });

  it("ignores a fork step-two rejection after route ownership changes", async () => {
    const spawn = deferred();
    transportMocks.subscribeRun.mockResolvedValue(undefined);
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    store.run = run("run-fork", "pending") as never;
    store.phase = "spawning";
    middlewareMocks.subscribeCurrent("run-fork", store);
    const connecting = store.connectSession("run-fork");
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    store.invalidateOperationsForRoute("run-b");
    const loadingB = store.loadRun("run-b");
    spawn.reject(new Error("old spawn failed"));

    await expect(connecting).resolves.toBeUndefined();
    await expect(loadingB).resolves.toBe(true);
    expect(currentRunId(store)).toBe("run-b");
    expect(store.error).toBe("");
  });

  it("does not create or route a new run after navigation while settings load", async () => {
    const settings = deferredValue<{ auth_mode: string; active_platform_id: string }>();
    apiMocks.getUserSettings.mockReturnValueOnce(settings.promise);
    const store = new SessionStore();
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.getUserSettings).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    await expect(loadingB).resolves.toBe(true);
    settings.resolve({ auth_mode: "api", active_platform_id: "platform-a" });

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    expect(apiMocks.startRun).not.toHaveBeenCalled();
    expect(currentRunId(store)).toBe("run-b");
    expect(store.platformId).toBeNull();
  });

  it("does not apply stale agent plan mode to the run opened during settings load", async () => {
    const agentSettings = deferredValue<{ plan_mode: boolean }>();
    apiMocks.getAgentSettings.mockReturnValueOnce(agentSettings.promise);
    const store = new SessionStore();
    store.permissionMode = "default";
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.getAgentSettings).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    await expect(store.loadRun("run-b")).resolves.toBe(true);
    agentSettings.resolve({ plan_mode: true });

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    expect(apiMocks.startRun).not.toHaveBeenCalled();
    expect(currentRunId(store)).toBe("run-b");
    expect(store.permissionMode).toBe("default");
  });

  it("does not apply fallback permission mode after a stale agent-settings rejection", async () => {
    const agentSettings = deferredValue<{ plan_mode: boolean }>();
    apiMocks.getUserSettings.mockResolvedValueOnce({
      auth_mode: "cli",
      permission_mode: "auto_all",
    });
    apiMocks.getAgentSettings.mockReturnValueOnce(agentSettings.promise);
    const store = new SessionStore();
    store.permissionMode = "default";
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.getAgentSettings).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    await expect(store.loadRun("run-b")).resolves.toBe(true);
    agentSettings.reject(new Error("old settings request failed"));

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    expect(apiMocks.startRun).not.toHaveBeenCalled();
    expect(currentRunId(store)).toBe("run-b");
    expect(store.permissionMode).toBe("default");
  });

  it("leaves a newly persisted run in the run list after navigation before spawn", async () => {
    const created = deferredValue<ReturnType<typeof run>>();
    apiMocks.startRun.mockReturnValueOnce(created.promise);
    const store = new SessionStore();
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.startRun).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    created.resolve(run("run-new", "pending"));

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    await expect(loadingB).resolves.toBe(true);
    expect(apiMocks.startSession).not.toHaveBeenCalled();
    expect(currentRunId(store)).toBe("run-b");
  });

  it("does not install timers after a new session spawn resolves on another route", async () => {
    const spawn = deferred();
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    spawn.resolve();

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    await expect(loadingB).resolves.toBe(true);
    expect(currentRunId(store)).toBe("run-b");
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
    expect((store as unknown as { _responseTimer: unknown })._responseTimer).toBeNull();
  });

  it("does not bind a stopped Codex restart timer to the run opened during spawn", async () => {
    const spawn = deferred();
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    store.run = {
      ...run("run-codex", "stopped"),
      agent: "codex",
      conversation_ref: { kind: "codex_thread", id: "thread-1" },
    } as never;
    store.agent = "codex";
    store.phase = "stopped";
    middlewareMocks.subscribeCurrent("run-codex", store);
    const sending = store.sendMessage("continue", []);
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    middlewareMocks.subscribeCurrent("run-b", store);
    expect(store.invalidateOperationsForRoute("run-b")).toBe(true);
    const loadingB = store.loadRun("run-b");
    spawn.resolve();

    await expect(sending).resolves.toEqual({ status: "cancelled" });
    await expect(loadingB).resolves.toBe(true);
    expect(currentRunId(store)).toBe("run-b");
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
  });

  it("cancels a new-session spawn after unmount without installing timers", async () => {
    const spawn = deferred();
    apiMocks.startSession.mockReturnValueOnce(spawn.promise);
    const store = new SessionStore();
    const starting = store.startSession("hello", "/tmp", []);
    await vi.waitFor(() => expect(apiMocks.startSession).toHaveBeenCalled());

    store.unmountGuards();
    spawn.resolve();

    await expect(starting).resolves.toEqual({ status: "cancelled" });
    expect((store as unknown as { _spawnTimer: unknown })._spawnTimer).toBeNull();
    expect((store as unknown as { _responseTimer: unknown })._responseTimer).toBeNull();
  });
});

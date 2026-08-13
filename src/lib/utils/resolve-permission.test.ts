/**
 * Optimistic permission resolve helper tests.
 *
 * Equivalent to testing handlePermissionRespond / handleExitPlanClearContext
 * from +page.svelte, but without the page component dependency.
 */
import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

import { resolvePermissionAfterDelivery, resolvePermissionOptimistic } from "./resolve-permission";
import { markAttention, hasAttention, _resetForTest } from "$lib/stores/attention-store.svelte";

function mockStore() {
  return {
    resolvePermissionAllow: vi.fn(),
    resolvePermissionDeny: vi.fn(),
    pendingPermissionModeOverride: null as string | null,
  };
}

describe("resolvePermissionOptimistic", () => {
  beforeEach(() => {
    _resetForTest();
  });

  // handlePermissionRespond equivalents

  it("allow → calls resolvePermissionAllow + clears permission", () => {
    const store = mockStore();
    markAttention("run-1", "permission");

    resolvePermissionOptimistic(store, "run-1", "req-1", "allow");

    expect(store.resolvePermissionAllow).toHaveBeenCalledWith("req-1");
    expect(store.resolvePermissionDeny).not.toHaveBeenCalled();
    expect(hasAttention("run-1")).toBe(false);
  });

  it("deny → calls resolvePermissionDeny + clears permission", () => {
    const store = mockStore();
    markAttention("run-1", "permission");

    resolvePermissionOptimistic(store, "run-1", "req-1", "deny");

    expect(store.resolvePermissionDeny).toHaveBeenCalledWith("req-1");
    expect(store.resolvePermissionAllow).not.toHaveBeenCalled();
    expect(hasAttention("run-1")).toBe(false);
  });

  it("delivery failure retains deny card and attention for retry", async () => {
    const store = mockStore();
    markAttention("run-1", "permission");

    await expect(
      resolvePermissionAfterDelivery(store, "run-1", "req-1", "deny", () =>
        Promise.reject(new Error("stdin closed")),
      ),
    ).rejects.toThrow("stdin closed");

    expect(store.resolvePermissionDeny).not.toHaveBeenCalled();
    expect(hasAttention("run-1")).toBe(true);
  });

  it("delivery success resolves deny card and attention", async () => {
    const store = mockStore();
    markAttention("run-1", "permission");

    await resolvePermissionAfterDelivery(store, "run-1", "req-1", "deny", () => Promise.resolve());

    expect(store.resolvePermissionDeny).toHaveBeenCalledWith("req-1");
    expect(hasAttention("run-1")).toBe(false);
  });

  it("failed mode-changing allow rolls back before a deny retry", async () => {
    const store = mockStore();
    store.pendingPermissionModeOverride = "default";

    await expect(
      resolvePermissionAfterDelivery(
        store,
        "run-1",
        "exitplan-req",
        "allow",
        () => Promise.reject(new Error("stdin closed")),
        "acceptEdits",
      ),
    ).rejects.toThrow("stdin closed");
    expect(store.pendingPermissionModeOverride).toBe("default");

    await resolvePermissionAfterDelivery(store, "run-1", "exitplan-req", "deny", () =>
      Promise.resolve(),
    );
    expect(store.pendingPermissionModeOverride).toBe("default");
  });

  it("failed delivery does not roll back a newer mode override", async () => {
    const store = mockStore();
    let rejectDelivery!: (error: Error) => void;
    const delivery = new Promise<void>((_, reject) => {
      rejectDelivery = reject;
    });
    const response = resolvePermissionAfterDelivery(
      store,
      "run-1",
      "exitplan-req",
      "allow",
      () => delivery,
      "acceptEdits",
    );
    store.pendingPermissionModeOverride = "bypassPermissions";
    rejectDelivery(new Error("stdin closed"));

    await expect(response).rejects.toThrow("stdin closed");
    expect(store.pendingPermissionModeOverride).toBe("bypassPermissions");
  });

  // handleExitPlanClearContext equivalent

  it("ExitPlanMode allow → resolvePermissionAllow + clears permission", () => {
    const store = mockStore();
    markAttention("run-1", "permission");

    resolvePermissionOptimistic(store, "run-1", "exitplan-req", "allow");

    expect(store.resolvePermissionAllow).toHaveBeenCalledWith("exitplan-req");
    expect(hasAttention("run-1")).toBe(false);
  });

  // Edge cases

  it("allow does not clear ask flag", () => {
    const store = mockStore();
    markAttention("run-1", "permission");
    markAttention("run-1", "ask");

    resolvePermissionOptimistic(store, "run-1", "req-1", "allow");

    // permission cleared, ask remains
    expect(hasAttention("run-1")).toBe(true);
  });

  it("deny clears both permission and ask flags", () => {
    const store = mockStore();
    markAttention("run-1", "permission");
    markAttention("run-1", "ask");

    resolvePermissionOptimistic(store, "run-1", "req-1", "deny");

    // Both cleared — AskUserQuestion deny should not leave ask lingering
    expect(hasAttention("run-1")).toBe(false);
  });

  it("unmarked run — no error, hasAttention stays false", () => {
    const store = mockStore();

    expect(() => {
      resolvePermissionOptimistic(store, "run-unknown", "req-1", "allow");
    }).not.toThrow();

    expect(store.resolvePermissionAllow).toHaveBeenCalledWith("req-1");
    expect(hasAttention("run-unknown")).toBe(false);
  });
});

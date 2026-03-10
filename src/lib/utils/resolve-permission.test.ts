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

import { resolvePermissionOptimistic } from "./resolve-permission";
import { markAttention, hasAttention, _resetForTest } from "$lib/stores/attention-store.svelte";

function mockStore() {
  return {
    resolvePermissionAllow: vi.fn(),
    resolvePermissionDeny: vi.fn(),
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

  it("deny catch branch — same behavior as success", () => {
    // In +page.svelte catch block, deny also calls the helper
    const store = mockStore();
    markAttention("run-1", "permission");

    resolvePermissionOptimistic(store, "run-1", "req-1", "deny");

    expect(store.resolvePermissionDeny).toHaveBeenCalledWith("req-1");
    expect(hasAttention("run-1")).toBe(false);
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

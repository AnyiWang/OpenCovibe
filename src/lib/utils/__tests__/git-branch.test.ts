import { describe, it, expect, vi } from "vitest";
import { createGitBranchPoller } from "../git-branch";

// Mock debug utils
vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

import { dbgWarn } from "$lib/utils/debug";

describe("createGitBranchPoller", () => {
  it("returns branch name on success", async () => {
    const fetch = vi.fn().mockResolvedValue("main");
    const poller = createGitBranchPoller(fetch);

    const result = await poller.refresh("/project");
    expect(result).toBe("main");
    expect(poller.current).toBe("main");
  });

  it("skips fetch when cwd is empty", async () => {
    const fetch = vi.fn();
    const poller = createGitBranchPoller(fetch);

    const result = await poller.refresh("");
    expect(result).toBe("");
    expect(poller.current).toBe("");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("discards stale response when rapid A→B switch (A slow, B fast)", async () => {
    let resolveA: (v: string) => void;
    const promiseA = new Promise<string>((r) => {
      resolveA = r;
    });
    const fetch = vi.fn().mockReturnValueOnce(promiseA).mockResolvedValueOnce("feature-b");

    const poller = createGitBranchPoller(fetch);

    // Fire A then immediately B
    const resultA = poller.refresh("/projectA");
    const resultB = poller.refresh("/projectB");

    // B resolves first
    expect(await resultB).toBe("feature-b");
    expect(poller.current).toBe("feature-b");

    // A resolves late — should be discarded
    resolveA!("feature-a");
    expect(await resultA).toBe("feature-b"); // returns current, not stale
    expect(poller.current).toBe("feature-b");
  });

  it("clears current when refresh('') is called while A is in-flight", async () => {
    let resolveA: (v: string) => void;
    const promiseA = new Promise<string>((r) => {
      resolveA = r;
    });
    const fetch = vi.fn().mockReturnValueOnce(promiseA);
    const poller = createGitBranchPoller(fetch);

    const resultA = poller.refresh("/project");
    // Clear via empty cwd
    const cleared = await poller.refresh("");
    expect(cleared).toBe("");
    expect(poller.current).toBe("");

    // A resolves late — should be discarded
    resolveA!("main");
    expect(await resultA).toBe(""); // returns current (empty)
    expect(poller.current).toBe("");
  });

  it("handles fetch error gracefully", async () => {
    const fetch = vi.fn().mockRejectedValue(new Error("git not found"));
    const poller = createGitBranchPoller(fetch);

    const result = await poller.refresh("/project");
    expect(result).toBe("");
    expect(poller.current).toBe("");
  });

  it("does not overwrite new success with stale error", async () => {
    let rejectA: (e: Error) => void;
    const promiseA = new Promise<string>((_, r) => {
      rejectA = r;
    });
    const fetch = vi.fn().mockReturnValueOnce(promiseA).mockResolvedValueOnce("develop");

    const poller = createGitBranchPoller(fetch);

    const resultA = poller.refresh("/old");
    const resultB = poller.refresh("/new");

    // B succeeds first
    expect(await resultB).toBe("develop");
    expect(poller.current).toBe("develop");

    // A errors late — stale, should not override
    rejectA!(new Error("timeout"));
    expect(await resultA).toBe("develop");
    expect(poller.current).toBe("develop");
  });

  it("suppresses repeated identical error warnings", async () => {
    vi.mocked(dbgWarn).mockClear();
    const fetch = vi.fn().mockRejectedValue(new Error("permission denied"));
    const poller = createGitBranchPoller(fetch);

    await poller.refresh("/project");
    await poller.refresh("/project");

    // Should only warn once for the same cwd + error combo
    expect(dbgWarn).toHaveBeenCalledTimes(1);
  });

  it("resets error suppression after a success", async () => {
    vi.mocked(dbgWarn).mockClear();
    const fetch = vi
      .fn()
      .mockResolvedValueOnce("main") // success
      .mockRejectedValueOnce(new Error("permission denied")); // then error

    const poller = createGitBranchPoller(fetch);

    await poller.refresh("/project"); // success
    await poller.refresh("/project"); // error → should warn

    expect(dbgWarn).toHaveBeenCalledTimes(1);
    expect(dbgWarn).toHaveBeenCalledWith("git-branch", "fetch error", expect.any(Object));
  });
});

import { describe, it, expect } from "vitest";
import { getAgentCaps } from "../agent-caps";

describe("getAgentCaps", () => {
  it("returns full capabilities for claude", () => {
    const caps = getAgentCaps("claude");
    expect(caps.supportsBusEvents).toBe(true);
    expect(caps.supportsSessionInit).toBe(true);
    expect(caps.supportsPermissions).toBe(true);
    expect(caps.supportsSnapshots).toBe(true);
    // Claude's `/add-dir` takes effect immediately on the running session.
    expect(caps.supportsLiveAddDir).toBe(true);
  });

  it("returns bus-events-only caps for codex", () => {
    const caps = getAgentCaps("codex");
    expect(caps.supportsBusEvents).toBe(true);
    expect(caps.supportsSessionInit).toBe(false);
    expect(caps.supportsPermissions).toBe(false);
    expect(caps.supportsSnapshots).toBe(false);
    // Codex reads writableRoots at thread/start, so add-dir is persisted, not live.
    expect(caps.supportsLiveAddDir).toBe(false);
  });

  it("returns minimal caps for unknown agent (never promotes to claude)", () => {
    const caps = getAgentCaps("unknown-agent");
    expect(caps.supportsBusEvents).toBe(false);
    expect(caps.supportsSessionInit).toBe(false);
    expect(caps.supportsPermissions).toBe(false);
    expect(caps.supportsSnapshots).toBe(false);
    expect(caps.supportsLiveAddDir).toBe(false);
  });

  it("codex and unknown caps differ (codex has supportsBusEvents)", () => {
    const codex = getAgentCaps("codex");
    const unknown = getAgentCaps("some-future-agent");
    expect(codex.supportsBusEvents).toBe(true);
    expect(unknown.supportsBusEvents).toBe(false);
  });
});

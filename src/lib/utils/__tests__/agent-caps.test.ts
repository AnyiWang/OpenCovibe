import { describe, it, expect } from "vitest";
import { getAgentCaps, isKnownAgent } from "../agent-caps";

describe("getAgentCaps", () => {
  it("returns full capabilities for claude", () => {
    const caps = getAgentCaps("claude");
    expect(caps.transport).toBe("stream-session");
    expect(caps.supportsResume).toBe(true);
    expect(caps.supportsStructuredTools).toBe(true);
    expect(caps.supportsPermissions).toBe(true);
    expect(caps.supportsEffort).toBe(true);
    expect(caps.supportsPermissionMode).toBe(true);
    expect(caps.supportsAddDir).toBe(true);
  });

  it("returns pipe-exec transport for codex", () => {
    const caps = getAgentCaps("codex");
    expect(caps.transport).toBe("pipe-exec");
    expect(caps.supportsResume).toBe(false);
    expect(caps.supportsStructuredTools).toBe(false);
    expect(caps.supportsPermissions).toBe(false);
    expect(caps.supportsEffort).toBe(false);
    expect(caps.supportsPermissionMode).toBe(false);
    expect(caps.supportsAddDir).toBe(false);
  });

  it("returns minimal caps for unknown agent (never promotes to claude)", () => {
    const caps = getAgentCaps("unknown-agent");
    expect(caps.transport).toBe("pipe-exec");
    expect(caps.supportsResume).toBe(false);
    expect(caps.supportsStructuredTools).toBe(false);
  });

  it("codex caps match minimal caps", () => {
    const codex = getAgentCaps("codex");
    const unknown = getAgentCaps("some-future-agent");
    expect(codex).toEqual(unknown);
  });
});

describe("isKnownAgent", () => {
  it("returns true for claude", () => {
    expect(isKnownAgent("claude")).toBe(true);
  });

  it("returns true for codex", () => {
    expect(isKnownAgent("codex")).toBe(true);
  });

  it("returns false for unknown agents", () => {
    expect(isKnownAgent("gpt")).toBe(false);
    expect(isKnownAgent("")).toBe(false);
    expect(isKnownAgent("Claude")).toBe(false); // case sensitive
  });
});

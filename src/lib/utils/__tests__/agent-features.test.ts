import { describe, it, expect } from "vitest";
import { getAgentFeatures, isKnownAgent } from "../agent-features";

describe("getAgentFeatures", () => {
  it("returns full features for claude", () => {
    const f = getAgentFeatures("claude");
    expect(f.effortSelector).toBe(true);
    expect(f.planModeToggle).toBe(true);
    expect(f.permissionModeSwitch).toBe(true);
    expect(f.slashCommandMenu).toBe(true);
    expect(f.addDirAction).toBe(true);
  });

  it("returns codex features with effortSelector, planModeToggle and addDirAction enabled", () => {
    const f = getAgentFeatures("codex");
    expect(f.effortSelector).toBe(true);
    expect(f.planModeToggle).toBe(true);
    expect(f.permissionModeSwitch).toBe(false);
    expect(f.slashCommandMenu).toBe(true);
    expect(f.addDirAction).toBe(true);
  });

  it("returns minimal features for unknown agent", () => {
    const f = getAgentFeatures("unknown-agent");
    expect(f.effortSelector).toBe(false);
    expect(f.addDirAction).toBe(false);
  });
});

describe("isKnownAgent", () => {
  it("recognizes claude and codex", () => {
    expect(isKnownAgent("claude")).toBe(true);
    expect(isKnownAgent("codex")).toBe(true);
  });

  it("returns false for unknown agents", () => {
    expect(isKnownAgent("gemini")).toBe(false);
    expect(isKnownAgent("")).toBe(false);
  });
});

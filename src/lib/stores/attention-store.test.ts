/**
 * Attention store unit tests.
 */
import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

import {
  markAttention,
  clearAttention,
  hasAttention,
  _resetForTest,
} from "./attention-store.svelte";

describe("attention-store", () => {
  beforeEach(() => {
    _resetForTest();
  });

  it("mark + hasAttention returns true", () => {
    markAttention("run-1", "permission");
    expect(hasAttention("run-1")).toBe(true);
  });

  it("clear single reason does not affect the other reason", () => {
    markAttention("run-1", "permission");
    markAttention("run-1", "ask");
    clearAttention("run-1", "permission");
    // ask still active
    expect(hasAttention("run-1")).toBe(true);
  });

  it("clear all (no reason) clears everything", () => {
    markAttention("run-1", "permission");
    markAttention("run-1", "ask");
    clearAttention("run-1");
    expect(hasAttention("run-1")).toBe(false);
  });

  it("different runIds are independent", () => {
    markAttention("run-a", "permission");
    markAttention("run-b", "ask");
    clearAttention("run-a");
    expect(hasAttention("run-a")).toBe(false);
    expect(hasAttention("run-b")).toBe(true);
  });

  it("repeated clear is idempotent (no throw)", () => {
    clearAttention("run-1", "permission");
    clearAttention("run-1", "permission");
    expect(hasAttention("run-1")).toBe(false);
  });
});

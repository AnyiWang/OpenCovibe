import { describe, it, expect } from "vitest";
import {
  createHistoryState,
  checkAndReset,
  resetHistory,
  shouldIntercept,
  isFirstLine,
  isLastLine,
  getHistoryAction,
} from "../input-history";

// ── shouldIntercept ──

describe("shouldIntercept", () => {
  const noMenus = { atMenuOpen: false, slashMenuOpen: false, modeDropdownOpen: false };
  const noMods = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };

  it("passes when all conditions met", () => {
    expect(shouldIntercept("ArrowUp", noMods, noMenus, 0, 0, 3)).toBe(true);
    expect(shouldIntercept("ArrowDown", noMods, noMenus, 0, 0, 3)).toBe(true);
  });

  it("rejects non-arrow keys", () => {
    expect(shouldIntercept("Enter", noMods, noMenus, 0, 0, 3)).toBe(false);
    expect(shouldIntercept("ArrowLeft", noMods, noMenus, 0, 0, 3)).toBe(false);
  });

  it("rejects when atMenuOpen", () => {
    expect(shouldIntercept("ArrowUp", noMods, { ...noMenus, atMenuOpen: true }, 0, 0, 3)).toBe(
      false,
    );
  });

  it("rejects when slashMenuOpen", () => {
    expect(shouldIntercept("ArrowUp", noMods, { ...noMenus, slashMenuOpen: true }, 0, 0, 3)).toBe(
      false,
    );
  });

  it("rejects when modeDropdownOpen", () => {
    expect(
      shouldIntercept("ArrowUp", noMods, { ...noMenus, modeDropdownOpen: true }, 0, 0, 3),
    ).toBe(false);
  });

  it("rejects with metaKey", () => {
    expect(shouldIntercept("ArrowUp", { ...noMods, metaKey: true }, noMenus, 0, 0, 3)).toBe(false);
  });

  it("rejects with ctrlKey", () => {
    expect(shouldIntercept("ArrowUp", { ...noMods, ctrlKey: true }, noMenus, 0, 0, 3)).toBe(false);
  });

  it("rejects with altKey", () => {
    expect(shouldIntercept("ArrowUp", { ...noMods, altKey: true }, noMenus, 0, 0, 3)).toBe(false);
  });

  it("rejects with shiftKey", () => {
    expect(shouldIntercept("ArrowUp", { ...noMods, shiftKey: true }, noMenus, 0, 0, 3)).toBe(false);
  });

  it("rejects with selection range", () => {
    expect(shouldIntercept("ArrowUp", noMods, noMenus, 0, 5, 3)).toBe(false);
  });

  it("rejects with empty history", () => {
    expect(shouldIntercept("ArrowUp", noMods, noMenus, 0, 0, 0)).toBe(false);
  });
});

// ── isFirstLine / isLastLine ──

describe("isFirstLine", () => {
  it("single line — always first", () => {
    expect(isFirstLine("hello", 0)).toBe(true);
    expect(isFirstLine("hello", 3)).toBe(true);
    expect(isFirstLine("hello", 5)).toBe(true);
  });

  it("multi-line — cursor on first line", () => {
    expect(isFirstLine("hello\nworld", 0)).toBe(true);
    expect(isFirstLine("hello\nworld", 5)).toBe(true);
  });

  it("multi-line — cursor on second line", () => {
    expect(isFirstLine("hello\nworld", 6)).toBe(false);
    expect(isFirstLine("hello\nworld", 10)).toBe(false);
  });

  it("empty string", () => {
    expect(isFirstLine("", 0)).toBe(true);
  });
});

describe("isLastLine", () => {
  it("single line — always last", () => {
    expect(isLastLine("hello", 0)).toBe(true);
    expect(isLastLine("hello", 5)).toBe(true);
  });

  it("multi-line — cursor on last line", () => {
    expect(isLastLine("hello\nworld", 6)).toBe(true);
    expect(isLastLine("hello\nworld", 11)).toBe(true);
  });

  it("multi-line — cursor on first line", () => {
    expect(isLastLine("hello\nworld", 0)).toBe(false);
    expect(isLastLine("hello\nworld", 4)).toBe(false);
  });

  it("empty string", () => {
    expect(isLastLine("", 0)).toBe(true);
  });

  it("cursor at newline position", () => {
    // cursor at index 5 is right before \n — indexOf("\n", 5) = 5, so NOT last line
    expect(isLastLine("hello\nworld", 5)).toBe(false);
  });
});

// ── getHistoryAction ──

describe("getHistoryAction", () => {
  it("ArrowUp from draft enters history", () => {
    const state = createHistoryState();
    const action = getHistoryAction("ArrowUp", state, 3, "hello", 0);
    expect(action).toEqual({ type: "enter", index: 0 });
  });

  it("ArrowUp goes deeper into history", () => {
    const state = createHistoryState();
    state.index = 0;
    const action = getHistoryAction("ArrowUp", state, 3, "msg0", 0);
    expect(action).toEqual({ type: "up", index: 1 });
  });

  it("ArrowUp at top returns boundary", () => {
    const state = createHistoryState();
    state.index = 2;
    const action = getHistoryAction("ArrowUp", state, 3, "msg2", 0);
    expect(action).toEqual({ type: "boundary" });
  });

  it("ArrowDown goes towards recent", () => {
    const state = createHistoryState();
    state.index = 2;
    const action = getHistoryAction("ArrowDown", state, 3, "msg2", 4);
    expect(action).toEqual({ type: "down", index: 1 });
  });

  it("ArrowDown at index 0 restores draft", () => {
    const state = createHistoryState();
    state.index = 0;
    const action = getHistoryAction("ArrowDown", state, 3, "msg0", 4);
    expect(action).toEqual({ type: "restore-draft" });
  });

  it("ArrowDown when not in history mode returns null", () => {
    const state = createHistoryState();
    const action = getHistoryAction("ArrowDown", state, 3, "hello", 5);
    expect(action).toBeNull();
  });

  it("ArrowUp on non-first line returns null", () => {
    const state = createHistoryState();
    const action = getHistoryAction("ArrowUp", state, 3, "hello\nworld", 8);
    expect(action).toBeNull();
  });

  it("ArrowDown on non-last line returns null", () => {
    const state = createHistoryState();
    state.index = 1;
    const action = getHistoryAction("ArrowDown", state, 3, "hello\nworld", 3);
    expect(action).toBeNull();
  });
});

// ── checkAndReset ──

describe("checkAndReset", () => {
  it("resets when length changes", () => {
    const state = createHistoryState();
    state.index = 2;
    state.prevLen = 3;
    state.scopeKey = "run1";
    state.draft = { text: "draft", attachments: [], pastedBlocks: [] };

    const did = checkAndReset(state, 4, "run1");
    expect(did).toBe(true);
    expect(state.index).toBe(-1);
    expect(state.draft).toBeNull();
    expect(state.prevLen).toBe(4);
  });

  it("resets when scopeKey changes", () => {
    const state = createHistoryState();
    state.index = 1;
    state.prevLen = 3;
    state.scopeKey = "run1";
    state.draft = { text: "d", attachments: [], pastedBlocks: [] };

    const did = checkAndReset(state, 3, "run2");
    expect(did).toBe(true);
    expect(state.index).toBe(-1);
    expect(state.draft).toBeNull();
    expect(state.scopeKey).toBe("run2");
  });

  it("does not reset when nothing changed", () => {
    const state = createHistoryState();
    state.index = 1;
    state.prevLen = 3;
    state.scopeKey = "run1";
    state.draft = { text: "d", attachments: [], pastedBlocks: [] };

    const did = checkAndReset(state, 3, "run1");
    expect(did).toBe(false);
    expect(state.index).toBe(1);
    expect(state.draft).not.toBeNull();
  });
});

// ── resetHistory ──

describe("resetHistory", () => {
  it("resets index and draft", () => {
    const state = createHistoryState();
    state.index = 2;
    state.draft = { text: "draft", attachments: [], pastedBlocks: [] };

    resetHistory(state);
    expect(state.index).toBe(-1);
    expect(state.draft).toBeNull();
  });
});

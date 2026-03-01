/**
 * KeybindingStore unit tests.
 *
 * Tests key normalization, display formatting, conflict detection,
 * dispatch behavior, reserved keys, and callback registration.
 *
 * Note: test environment is "node" (no DOM), so KeyboardEvent/HTMLElement
 * are replaced with plain object mocks.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Mock Tauri API
vi.mock("$lib/api", () => ({
  getUserSettings: vi.fn().mockResolvedValue({ keybinding_overrides: [] }),
  updateUserSettings: vi.fn().mockResolvedValue({}),
  readTextFile: vi.fn().mockRejectedValue(new Error("not found")),
}));

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

// Import after mocks
import {
  KeybindingStore,
  normalizeKeyEvent,
  formatKeyDisplay,
  RESERVED_KEYS,
} from "./keybindings.svelte";
import * as api from "$lib/api";

// Detect IS_MAC the same way the store does
const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.platform ?? "");

/**
 * Helper: create a mock KeyboardEvent-like object.
 * In node test env, real KeyboardEvent is not available.
 */
function mockKeyEvent(
  key: string,
  opts: {
    metaKey?: boolean;
    ctrlKey?: boolean;
    altKey?: boolean;
    shiftKey?: boolean;
    target?: unknown;
  } = {},
): KeyboardEvent {
  return {
    key,
    metaKey: opts.metaKey ?? false,
    ctrlKey: opts.ctrlKey ?? false,
    altKey: opts.altKey ?? false,
    shiftKey: opts.shiftKey ?? false,
    target: opts.target ?? null,
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
  } as unknown as KeyboardEvent;
}

/**
 * Helper: create a mock KeyboardEvent that triggers "Cmd+<key>" regardless of platform.
 * On macOS: metaKey=true; on others: ctrlKey=true.
 */
function mockCmdKeyEvent(
  key: string,
  extra: { shiftKey?: boolean; target?: unknown } = {},
): KeyboardEvent {
  return mockKeyEvent(key, {
    metaKey: IS_MAC,
    ctrlKey: !IS_MAC,
    shiftKey: extra.shiftKey,
    target: extra.target,
  });
}

/** Create a fake HTMLElement-like object that looks like an input. */
function fakeInput(): unknown {
  return {
    tagName: "INPUT",
    isContentEditable: false,
    closest: () => null,
  };
}

let warnSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  warnSpy.mockRestore();
});

describe("normalizeKeyEvent", () => {
  // IS_MAC depends on runtime: macOS → metaKey=Cmd; others → ctrlKey=Cmd

  it("normalizes platform Cmd key to Cmd", () => {
    const result = normalizeKeyEvent(mockCmdKeyEvent("b"));
    expect(result).toBe("Cmd+B");
  });

  it("normalizes Shift+Tab", () => {
    const result = normalizeKeyEvent(mockKeyEvent("Tab", { shiftKey: true }));
    expect(result).toBe("Shift+Tab");
  });

  it("normalizes Alt+P", () => {
    const result = normalizeKeyEvent(mockKeyEvent("p", { altKey: true }));
    expect(result).toBe("Alt+P");
  });

  it("normalizes Escape", () => {
    const result = normalizeKeyEvent(mockKeyEvent("Escape"));
    expect(result).toBe("Escape");
  });

  it("normalizes Enter", () => {
    const result = normalizeKeyEvent(mockKeyEvent("Enter"));
    expect(result).toBe("Enter");
  });

  it("normalizes Shift+Enter", () => {
    const result = normalizeKeyEvent(mockKeyEvent("Enter", { shiftKey: true }));
    expect(result).toBe("Shift+Enter");
  });

  it("returns empty for modifier-only presses", () => {
    expect(normalizeKeyEvent(mockKeyEvent("Control"))).toBe("");
    expect(normalizeKeyEvent(mockKeyEvent("Meta"))).toBe("");
    expect(normalizeKeyEvent(mockKeyEvent("Alt"))).toBe("");
    expect(normalizeKeyEvent(mockKeyEvent("Shift"))).toBe("");
  });

  it("normalizes Space", () => {
    const result = normalizeKeyEvent(mockKeyEvent(" "));
    expect(result).toBe("Space");
  });

  it("uppercases single character keys", () => {
    const result = normalizeKeyEvent(mockKeyEvent("a"));
    expect(result).toBe("A");
  });

  it("normalizes Cmd+Enter", () => {
    const result = normalizeKeyEvent(mockCmdKeyEvent("Enter"));
    expect(result).toBe("Cmd+Enter");
  });

  it("normalizes Cmd+Shift combo", () => {
    const result = normalizeKeyEvent(mockCmdKeyEvent("b", { shiftKey: true }));
    expect(result).toBe("Cmd+Shift+B");
  });
});

describe("formatKeyDisplay", () => {
  it("formats Cmd as symbol", () => {
    expect(formatKeyDisplay("Cmd+B")).toBe(IS_MAC ? "⌘B" : "Ctrl+B");
  });

  it("formats Shift as symbol", () => {
    expect(formatKeyDisplay("Shift+Tab")).toBe(IS_MAC ? "⇧⇥" : "Shift+Tab");
  });

  it("formats Alt as symbol", () => {
    expect(formatKeyDisplay("Alt+P")).toBe(IS_MAC ? "⌥P" : "Alt+P");
  });

  it("formats Ctrl as symbol", () => {
    expect(formatKeyDisplay("Ctrl+C")).toBe(IS_MAC ? "⌃C" : "Ctrl+C");
  });

  it("formats Enter as symbol", () => {
    expect(formatKeyDisplay("Enter")).toBe(IS_MAC ? "↵" : "Enter");
  });

  it("formats Escape as symbol", () => {
    expect(formatKeyDisplay("Escape")).toBe(IS_MAC ? "⎋" : "Esc");
  });

  it("formats Cmd+Shift+B", () => {
    expect(formatKeyDisplay("Cmd+Shift+B")).toBe(IS_MAC ? "⌘⇧B" : "Ctrl+Shift+B");
  });

  it("formats Cmd+Enter", () => {
    expect(formatKeyDisplay("Cmd+Enter")).toBe(IS_MAC ? "⌘↵" : "Ctrl+Enter");
  });

  it("returns empty for empty/disabled keys", () => {
    expect(formatKeyDisplay("")).toBe("");
    expect(formatKeyDisplay("disabled")).toBe("");
  });
});

describe("RESERVED_KEYS", () => {
  it("includes Cmd+C, Cmd+V, Cmd+X", () => {
    expect(RESERVED_KEYS.has("Cmd+C")).toBe(true);
    expect(RESERVED_KEYS.has("Cmd+V")).toBe(true);
    expect(RESERVED_KEYS.has("Cmd+X")).toBe(true);
  });

  it("includes Cmd+Q (system quit)", () => {
    expect(RESERVED_KEYS.has("Cmd+Q")).toBe(true);
  });

  it("includes Ctrl variants for non-Mac", () => {
    expect(RESERVED_KEYS.has("Ctrl+C")).toBe(true);
    expect(RESERVED_KEYS.has("Ctrl+A")).toBe(true);
  });

  it("does not include non-reserved combos", () => {
    expect(RESERVED_KEYS.has("Cmd+B")).toBe(false);
    expect(RESERVED_KEYS.has("Cmd+K")).toBe(false);
  });
});

describe("KeybindingStore", () => {
  let store: KeybindingStore;

  beforeEach(() => {
    store = new KeybindingStore();
    vi.clearAllMocks();
  });

  describe("initial state", () => {
    it("has default bindings", () => {
      expect(store.bindings.length).toBeGreaterThan(0);
      expect(store.overrides).toEqual([]);
      expect(store.recording).toBe(false);
    });

    it("resolved bindings match defaults when no overrides", () => {
      const appToggle = store.resolved.find((b) => b.command === "app:toggleSidebar");
      expect(appToggle?.key).toBe("Cmd+B");
    });

    it("includes CLI bindings", () => {
      const cliBindings = store.resolved.filter((b) => b.source === "cli");
      expect(cliBindings.length).toBeGreaterThan(0);
    });
  });

  describe("callback registration", () => {
    it("registers and fires callback on dispatch", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);

      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb).toHaveBeenCalledTimes(1);
      expect(e.preventDefault).toHaveBeenCalled();
    });

    it("unregisters callback correctly", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);
      store.unregisterCallback("app:toggleSidebar");

      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });

    it("only fires one command per keypress", () => {
      const cb1 = vi.fn();
      const cb2 = vi.fn();
      store.registerCallback("app:toggleSidebar", cb1);
      store.registerCallback("app:commandPalette", cb2);

      // Cmd+B should only trigger toggleSidebar
      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb1).toHaveBeenCalledTimes(1);
      expect(cb2).not.toHaveBeenCalled();
    });
  });

  describe("dispatch behavior", () => {
    it("skips dispatch when recording", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);
      store.recording = true;

      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });

    it("skips chat context commands when in editable target", () => {
      const cb = vi.fn();
      store.registerCallback("chat:interrupt", cb);

      // Fake input element as target
      const e = mockKeyEvent("Escape", { target: fakeInput() });
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });

    it("fires global context commands even in editable target", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);

      const e = mockCmdKeyEvent("b", { target: fakeInput() });
      store.dispatch(e);

      expect(cb).toHaveBeenCalledTimes(1);
    });

    it("does not fire for disabled bindings", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);
      store.overrides = [{ command: "app:toggleSidebar", key: "disabled" }];

      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });

    it("does not fire for empty key bindings", () => {
      const cb = vi.fn();
      store.registerCallback("app:toggleSidebar", cb);
      store.overrides = [{ command: "app:toggleSidebar", key: "" }];

      const e = mockCmdKeyEvent("b");
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });

    it("does not fire for CLI source bindings", () => {
      const cb = vi.fn();
      store.registerCallback("cli:interrupt", cb);

      // Even if key matches, CLI bindings are source="cli" and skipped by dispatch
      const e = mockCmdKeyEvent("c");
      store.dispatch(e);

      expect(cb).not.toHaveBeenCalled();
    });
  });

  describe("overrides", () => {
    it("resolved uses override key when present", () => {
      store.overrides = [{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }];
      const b = store.resolved.find((x) => x.command === "app:toggleSidebar");
      expect(b?.key).toBe("Cmd+Shift+B");
    });

    it("non-editable bindings ignore overrides", () => {
      store.overrides = [{ command: "prompt:send", key: "Cmd+Enter" }];
      const b = store.resolved.find((x) => x.command === "prompt:send");
      expect(b?.key).toBe("Enter"); // Should remain unchanged
    });

    it("setOverride persists via API", async () => {
      await store.setOverride("app:toggleSidebar", "Cmd+Shift+B");
      expect(api.updateUserSettings).toHaveBeenCalledWith({
        keybinding_overrides: [{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }],
      });
    });

    it("setOverride replaces existing override for same command", async () => {
      store.overrides = [{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }];
      await store.setOverride("app:toggleSidebar", "Alt+B");
      expect(store.overrides).toEqual([{ command: "app:toggleSidebar", key: "Alt+B" }]);
    });

    it("resetBinding removes override and persists", async () => {
      store.overrides = [{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }];
      await store.resetBinding("app:toggleSidebar");
      expect(store.overrides).toEqual([]);
      expect(api.updateUserSettings).toHaveBeenCalledWith({
        keybinding_overrides: [],
      });
    });

    it("resetAll clears all overrides", async () => {
      store.overrides = [
        { command: "app:toggleSidebar", key: "Cmd+Shift+B" },
        { command: "app:newChat", key: "Cmd+Shift+N" },
      ];
      await store.resetAll();
      expect(store.overrides).toEqual([]);
      expect(api.updateUserSettings).toHaveBeenCalledWith({
        keybinding_overrides: [],
      });
    });
  });

  describe("loadOverrides", () => {
    it("loads overrides from settings", async () => {
      vi.mocked(api.getUserSettings).mockResolvedValue({
        keybinding_overrides: [{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }],
      } as never);

      await store.loadOverrides();
      expect(store.overrides).toEqual([{ command: "app:toggleSidebar", key: "Cmd+Shift+B" }]);
    });

    it("handles load failure gracefully", async () => {
      vi.mocked(api.getUserSettings).mockRejectedValue(new Error("fail"));
      await store.loadOverrides();
      expect(store.overrides).toEqual([]);
    });
  });

  describe("conflict detection", () => {
    it("detects global vs any context conflict", () => {
      // app:toggleSidebar is "Cmd+B" in "global" context
      // "Cmd+B" in "chat" context → global conflicts with all
      const conflict = store.findConflict("Cmd+B", "chat", "chat:interrupt");
      expect(conflict).not.toBeNull();
      expect(conflict?.command).toBe("app:toggleSidebar");
    });

    it("detects global vs global conflict", () => {
      const conflict = store.findConflict("Cmd+K", "global", "app:toggleSidebar");
      expect(conflict).not.toBeNull();
      expect(conflict?.command).toBe("app:commandPalette");
    });

    it("excludes self from conflict check", () => {
      const conflict = store.findConflict("Cmd+B", "global", "app:toggleSidebar");
      expect(conflict).toBeNull();
    });

    it("no conflict between different non-global contexts", () => {
      // prompt:send is Enter in "prompt" context
      // Looking for Enter conflict in "chat" context — different non-global context → no conflict
      const conflict = store.findConflict("Enter", "chat", "chat:sendGlobal");
      expect(conflict).toBeNull();
    });

    it("returns null when no conflict exists", () => {
      const conflict = store.findConflict("F12", "global");
      expect(conflict).toBeNull();
    });

    it("ignores CLI source bindings in conflict check", () => {
      // CLI bindings are source="cli" — findConflict only checks source="app"
      const conflict = store.findConflict("Ctrl+C", "global");
      expect(conflict).toBeNull();
    });
  });

  describe("shifted symbol normalization", () => {
    it("normalizes ? (Shift+/) to plain ?", () => {
      expect(normalizeKeyEvent(mockKeyEvent("?", { shiftKey: true }))).toBe("?");
    });

    it("preserves Shift for Ctrl+Shift+_ (undo)", () => {
      const expected = IS_MAC ? "Ctrl+Shift+_" : "Cmd+Shift+_";
      expect(normalizeKeyEvent(mockKeyEvent("_", { shiftKey: true, ctrlKey: true }))).toBe(
        expected,
      );
    });

    it("preserves Shift for Shift+Tab", () => {
      expect(normalizeKeyEvent(mockKeyEvent("Tab", { shiftKey: true }))).toBe("Shift+Tab");
    });

    it("preserves Shift for Shift+A (letter)", () => {
      expect(normalizeKeyEvent(mockKeyEvent("A", { shiftKey: true }))).toBe("Shift+A");
    });
  });

  describe("new bindings", () => {
    it("has all 8 new app bindings in defaults", () => {
      const store = new KeybindingStore();
      const newCmds = [
        "app:shortcutHelp",
        "app:modelPicker",
        "chat:cyclePermission",
        "chat:stashPrompt",
        "app:toggleFastMode",
        "chat:toggleVerbose",
        "chat:toggleTasks",
        "chat:undoLastTurn",
      ];
      for (const cmd of newCmds) {
        expect(store.resolved.find((b) => b.command === cmd)).toBeDefined();
      }
    });

    it("new bindings have no conflicts among themselves", () => {
      const store = new KeybindingStore();
      const newBindings = store.resolved.filter((b) =>
        [
          "app:shortcutHelp",
          "app:modelPicker",
          "chat:cyclePermission",
          "chat:stashPrompt",
          "app:toggleFastMode",
          "chat:toggleVerbose",
          "chat:toggleTasks",
          "chat:undoLastTurn",
        ].includes(b.command),
      );
      for (const b of newBindings) {
        const conflict = store.findConflict(b.key, b.context, b.command);
        expect(conflict).toBeNull();
      }
    });
  });

  describe("matches", () => {
    it("returns true for matching command + event", () => {
      const e = mockKeyEvent("Escape");
      expect(store.matches(e, "chat:interrupt")).toBe(true);
    });

    it("returns false for non-matching event", () => {
      const e = mockKeyEvent("a");
      expect(store.matches(e, "chat:interrupt")).toBe(false);
    });

    it("returns false for unknown command", () => {
      const e = mockKeyEvent("Escape");
      expect(store.matches(e, "nonexistent:command")).toBe(false);
    });

    it("returns false for disabled binding", () => {
      store.overrides = [{ command: "chat:interrupt", key: "disabled" }];
      const e = mockKeyEvent("Escape");
      expect(store.matches(e, "chat:interrupt")).toBe(false);
    });
  });
});

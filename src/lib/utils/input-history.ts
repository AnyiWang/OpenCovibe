import type { PromptInputSnapshot } from "$lib/types";

export interface HistoryState {
  index: number; // -1 = draft, 0+ = history position
  draft: PromptInputSnapshot | null;
  prevLen: number; // previous history length for change detection
  scopeKey: string; // run ID scope — reset when run changes
}

export function createHistoryState(): HistoryState {
  return { index: -1, draft: null, prevLen: 0, scopeKey: "" };
}

/** Check if history scope or length changed and reset if so. Returns true if reset happened. */
export function checkAndReset(state: HistoryState, currentLen: number, scopeKey: string): boolean {
  if (scopeKey !== state.scopeKey || currentLen !== state.prevLen) {
    state.index = -1;
    state.draft = null;
    state.prevLen = currentLen;
    state.scopeKey = scopeKey;
    return true;
  }
  return false;
}

/** Reset history navigation state. */
export function resetHistory(state: HistoryState): void {
  state.index = -1;
  state.draft = null;
}

/**
 * Whether Up/Down should be intercepted for history navigation.
 * Checks: no menus, no modifiers, no selection range, has history.
 */
export function shouldIntercept(
  key: string,
  e: { metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean },
  menus: { atMenuOpen: boolean; slashMenuOpen: boolean; modeDropdownOpen: boolean },
  selectionStart: number,
  selectionEnd: number,
  historyLen: number,
): boolean {
  if (key !== "ArrowUp" && key !== "ArrowDown") return false;
  if (menus.atMenuOpen || menus.slashMenuOpen || menus.modeDropdownOpen) return false;
  if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return false;
  if (selectionStart !== selectionEnd) return false;
  if (historyLen === 0) return false;
  return true;
}

/** Whether cursor is on the first line of textarea value. */
export function isFirstLine(value: string, cursorPos: number): boolean {
  return value.lastIndexOf("\n", cursorPos - 1) === -1;
}

/** Whether cursor is on the last line of textarea value. */
export function isLastLine(value: string, cursorPos: number): boolean {
  return value.indexOf("\n", cursorPos) === -1;
}

export type HistoryAction =
  | { type: "enter"; index: number } // first Up from draft
  | { type: "up"; index: number } // deeper into history
  | { type: "down"; index: number } // towards recent
  | { type: "restore-draft" } // back to draft
  | { type: "boundary" } // at edge, do nothing
  | null; // not applicable

/**
 * Determine what action to take for an Up/Down key press.
 * Returns null if the key should not be handled (wrong line position).
 */
export function getHistoryAction(
  key: string,
  state: HistoryState,
  historyLen: number,
  textareaValue: string,
  cursorPos: number,
): HistoryAction {
  if (key === "ArrowUp") {
    if (!isFirstLine(textareaValue, cursorPos)) return null;
    if (state.index === -1) {
      return { type: "enter", index: 0 };
    } else if (state.index < historyLen - 1) {
      return { type: "up", index: state.index + 1 };
    }
    return { type: "boundary" };
  }

  if (key === "ArrowDown") {
    if (!isLastLine(textareaValue, cursorPos)) return null;
    if (state.index < 0) return null; // not in history mode
    if (state.index > 0) {
      return { type: "down", index: state.index - 1 };
    }
    return { type: "restore-draft" };
  }

  return null;
}

import { describe, it, expect } from "vitest";
import { spliceText, formatPasteSize } from "../format";

describe("spliceText", () => {
  it("inserts text at cursor position in empty string", () => {
    const result = spliceText("", "hello", 0);
    expect(result.text).toBe("hello");
    expect(result.cursorPos).toBe(5);
  });

  it("inserts text at cursor position in the middle", () => {
    const result = spliceText("Hello World", "Beautiful ", 6);
    expect(result.text).toBe("Hello Beautiful World");
    expect(result.cursorPos).toBe(16);
  });

  it("inserts text at the beginning", () => {
    const result = spliceText("World", "Hello ", 0);
    expect(result.text).toBe("Hello World");
    expect(result.cursorPos).toBe(6);
  });

  it("inserts text at the end", () => {
    const result = spliceText("Hello", " World", 5);
    expect(result.text).toBe("Hello World");
    expect(result.cursorPos).toBe(11);
  });

  it("replaces selected text", () => {
    // "Hello [World]" with "World" selected → replaced with "Beautiful"
    const result = spliceText("Hello World", "Beautiful", 6, 11);
    expect(result.text).toBe("Hello Beautiful");
    expect(result.cursorPos).toBe(15);
  });

  it("replaces entire content", () => {
    const result = spliceText("old text", "new text", 0, 8);
    expect(result.text).toBe("new text");
    expect(result.cursorPos).toBe(8);
  });

  it("handles multibyte characters", () => {
    // Select positions 2–4 ("世界") → replace with "美丽的"
    const result = spliceText("你好世界", "美丽的", 2, 4);
    expect(result.text).toBe("你好美丽的");
    expect(result.cursorPos).toBe(5);
  });

  it("defaults selectionEnd to selectionStart when omitted (no selection)", () => {
    const result = spliceText("abc", "X", 1);
    expect(result.text).toBe("aXbc");
    expect(result.cursorPos).toBe(2);
  });

  it("inserts multiline text at cursor", () => {
    const multiline = "line1\nline2\nline3";
    const result = spliceText("before  after", multiline, 7);
    expect(result.text).toBe("before line1\nline2\nline3 after");
    expect(result.cursorPos).toBe(7 + multiline.length);
  });

  it("replaces selection with multiline text", () => {
    const replacement = "a\nb\nc";
    // "start [REPLACE] end" — select positions 6–15 ("[REPLACE]")
    const result = spliceText("start [REPLACE] end", replacement, 6, 15);
    expect(result.text).toBe("start a\nb\nc end");
    expect(result.cursorPos).toBe(6 + replacement.length);
  });

  it("inserts empty string (no-op)", () => {
    const result = spliceText("hello", "", 3);
    expect(result.text).toBe("hello");
    expect(result.cursorPos).toBe(3);
  });

  it("handles zero-length selection at position 0", () => {
    const result = spliceText("abc", "X", 0, 0);
    expect(result.text).toBe("Xabc");
    expect(result.cursorPos).toBe(1);
  });
});

describe("formatPasteSize", () => {
  it("shows chars for single line", () => {
    expect(formatPasteSize(1, 42)).toBe("42 chars");
  });

  it("shows lines for multi-line", () => {
    expect(formatPasteSize(5, 200)).toBe("5 lines");
  });

  it("shows k lines for large counts", () => {
    expect(formatPasteSize(1500, 50000)).toBe("1.5k lines");
  });

  it("shows 1k lines at exactly 1000", () => {
    expect(formatPasteSize(1000, 30000)).toBe("1.0k lines");
  });

  it("shows chars for single line with zero chars", () => {
    expect(formatPasteSize(1, 0)).toBe("0 chars");
  });

  it("shows lines at 999 (below k threshold)", () => {
    expect(formatPasteSize(999, 50000)).toBe("999 lines");
  });

  it("shows k lines at 1001", () => {
    expect(formatPasteSize(1001, 50000)).toBe("1.0k lines");
  });
});

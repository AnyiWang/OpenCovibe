import { describe, it, expect } from "vitest";
import { ansiToHtml, hasAnsiCodes, escapeHtml, stripAnsi } from "./ansi";

describe("escapeHtml", () => {
  it("escapes HTML special characters", () => {
    expect(escapeHtml('<script>"alert&</script>')).toBe(
      "&lt;script&gt;&quot;alert&amp;&lt;/script&gt;",
    );
  });
});

describe("ansiToHtml", () => {
  it("escapes HTML in input — XSS prevention", () => {
    const result = ansiToHtml("<script>alert(1)</script>");
    expect(result).not.toContain("<script>");
    expect(result).toContain("&lt;script&gt;");
  });

  it("escapes HTML inside ANSI-colored text — injection prevention", () => {
    const result = ansiToHtml('\x1b[31m<img onerror="alert(1)">\x1b[0m');
    expect(result).not.toContain("<img");
    expect(result).toContain("&lt;img");
  });

  it("standard SGR color: green", () => {
    const result = ansiToHtml("\x1b[32mgreen\x1b[0m");
    expect(result).toContain('style="color:#22c55e"');
    expect(result).toContain("green</span>");
  });

  it("multiple attributes: bold + red", () => {
    const result = ansiToHtml("\x1b[1;31mbold red\x1b[0m");
    expect(result).toContain("color:#ef4444");
    expect(result).toContain("font-weight:bold");
  });

  it("unclosed sequence: auto-closes span", () => {
    const result = ansiToHtml("\x1b[31munclosed");
    expect(result).toContain("</span>");
    expect(result).toContain("unclosed");
  });

  it("plain text without ANSI: returns escaped text", () => {
    expect(ansiToHtml("hello world")).toBe("hello world");
    expect(ansiToHtml("a < b & c")).toBe("a &lt; b &amp; c");
  });

  it("256-color foreground", () => {
    const result = ansiToHtml("\x1b[38;5;196mred\x1b[0m");
    // 196 = bright red in 256-color palette (color cube)
    expect(result).toContain("color:#");
    expect(result).toContain("red</span>");
  });

  it("strips non-SGR CSI sequences from output", () => {
    // Cursor movement should be stripped
    const result = ansiToHtml("hello\x1b[2Aworld");
    expect(result).not.toContain("\x1b");
    expect(result).toContain("hello");
    expect(result).toContain("world");
  });
});

describe("hasAnsiCodes", () => {
  it("returns true for text with ANSI codes", () => {
    expect(hasAnsiCodes("\x1b[31mred\x1b[0m")).toBe(true);
  });

  it("returns false for plain text", () => {
    expect(hasAnsiCodes("hello world")).toBe(false);
  });
});

describe("stripAnsi", () => {
  it("removes OSC sequences", () => {
    // OSC hyperlink: \x1b]8;;url\x1b\\text\x1b]8;;\x1b\\
    const input = "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\";
    expect(stripAnsi(input)).toBe("link");
  });

  it("removes private CSI sequences", () => {
    // Hide/show cursor
    expect(stripAnsi("\x1b[?25l")).toBe("");
    expect(stripAnsi("\x1b[?25h")).toBe("");
  });

  it("removes charset designation sequences", () => {
    // \x1b(0 = switch to line drawing charset, \x1b(B = switch back to ASCII
    expect(stripAnsi("\x1b(0")).toBe("");
    expect(stripAnsi("\x1b(B")).toBe("");
  });

  it("comprehensive: mixed sequences → only plain text", () => {
    const input =
      "\x1b[32mgreen\x1b[0m \x1b]8;;url\x07link\x1b]8;;\x07 \x1b(0line\x1b(B \x1b[?25lhidden\x1b[?25h \x1bMreverse";
    const result = stripAnsi(input);
    expect(result).toBe("green link line hidden reverse");
    expect(result).not.toContain("\x1b");
  });

  it("plain text without escape codes: returns unchanged", () => {
    expect(stripAnsi("hello world")).toBe("hello world");
  });
});

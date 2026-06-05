import { describe, it, expect } from "vitest";
import {
  buildPasteToken,
  singlePasteTokenRe,
  insertAtSelection,
  parsePasteTokenSeqs,
  expandPasteTokens,
} from "../paste-tokens";

// Mirrors the i18n token label "Pasted #{seq} · {size}" — the `#N · ` is the parse anchor.
const tok = (seq: number, size = "120 lines") => buildPasteToken(`Pasted #${seq} · ${size}`);

describe("buildPasteToken / singlePasteTokenRe", () => {
  it("builds a clean token with no machine id", () => {
    expect(tok(1, "5 lines")).toBe("[Pasted #1 · 5 lines]");
  });

  it("single-token regex matches only its own sequence", () => {
    const body = `${tok(1)} ${tok(2)}`;
    expect(body.replace(singlePasteTokenRe(1), "X")).toBe(`X ${tok(2)}`);
  });

  it("#1 does not match #10 (digit boundary)", () => {
    const body = tok(10);
    expect(singlePasteTokenRe(1).test(body)).toBe(false);
    expect(singlePasteTokenRe(10).test(body)).toBe(true);
  });
});

describe("insertAtSelection", () => {
  it("inserts at a collapsed cursor", () => {
    expect(insertAtSelection("abcd", 2, 2, "XY")).toBe("abXYcd");
  });

  it("replaces a selection range", () => {
    expect(insertAtSelection("abcd", 1, 3, "XY")).toBe("aXYd");
  });

  it("appends at end", () => {
    expect(insertAtSelection("abc", 3, 3, "Z")).toBe("abcZ");
  });
});

describe("parsePasteTokenSeqs", () => {
  it("returns sequences in document order", () => {
    const body = `intro ${tok(1)} mid ${tok(2)} end`;
    expect(parsePasteTokenSeqs(body)).toEqual([1, 2]);
  });

  it("returns empty for plain text", () => {
    expect(parsePasteTokenSeqs("no tokens here [not a token]")).toEqual([]);
  });

  it("keeps duplicates", () => {
    const body = `${tok(1)} ${tok(1)}`;
    expect(parsePasteTokenSeqs(body)).toEqual([1, 1]);
  });
});

describe("expandPasteTokens", () => {
  const blocks = [
    { seq: 1, text: "FULL-A\nline2" },
    { seq: 2, text: "FULL-B" },
  ];

  it("expands a token in place, preserving surrounding text", () => {
    const body = `before ${tok(1)} after`;
    const { text, usedSeqs } = expandPasteTokens(body, blocks);
    expect(text).toBe("before FULL-A\nline2 after");
    expect([...usedSeqs]).toEqual([1]);
  });

  it("expands multiple tokens at their positions", () => {
    const body = `q: ${tok(2)} note: ${tok(1)}`;
    const { text, usedSeqs } = expandPasteTokens(body, blocks);
    expect(text).toBe("q: FULL-B note: FULL-A\nline2");
    expect(usedSeqs.has(1) && usedSeqs.has(2)).toBe(true);
  });

  it("strips an orphan token whose sequence has no block", () => {
    const body = `x ${tok(9)} y`;
    const { text, usedSeqs } = expandPasteTokens(body, blocks);
    expect(text).toBe("x  y");
    expect(usedSeqs.size).toBe(0);
  });

  it("leaves token-less bodies unchanged and uses no sequences", () => {
    const { text, usedSeqs } = expandPasteTokens("plain typed text", blocks);
    expect(text).toBe("plain typed text");
    expect(usedSeqs.size).toBe(0);
  });
});

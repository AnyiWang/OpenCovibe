import { describe, it, expect, vi, beforeEach } from "vitest";

// Use vi.hoisted to create mock functions in the hoisted scope
const mocks = vi.hoisted(() => ({
  convertToHtml: vi.fn(),
  turndown: vi.fn((html: string) => html.replace(/<[^>]+>/g, "").trim()),
  xlsxLoad: vi.fn(),
  eachSheet: vi.fn(),
}));

vi.mock("mammoth", () => ({
  default: { convertToHtml: mocks.convertToHtml },
}));

vi.mock("turndown", () => ({
  default: function TurndownService() {
    return { turndown: mocks.turndown };
  },
}));

vi.mock("exceljs", () => ({
  default: {
    Workbook: function Workbook() {
      return { xlsx: { load: mocks.xlsxLoad }, eachSheet: mocks.eachSheet };
    },
  },
}));

import { convertFile, MAX_CONVERTED_CHARS } from "$lib/utils/file-convert";

function mockFile(name: string, content: ArrayBuffer = new ArrayBuffer(8)): File {
  return new File([content], name, { type: "" });
}

describe("convertFile", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.turndown.mockImplementation((html: string) => html.replace(/<[^>]+>/g, "").trim());
  });

  describe("docx conversion", () => {
    it("converts docx to markdown", async () => {
      mocks.convertToHtml.mockResolvedValueOnce({
        value: "<h1>Hello</h1><p>World</p>",
        messages: [],
      });

      const file = mockFile("test.docx");
      const result = await convertFile(file);

      expect(result.format).toBe("markdown");
      expect(result.text).toContain("Hello");
      expect(result.text).toContain("World");
    });

    it("throws on empty document", async () => {
      mocks.convertToHtml.mockResolvedValueOnce({
        value: "",
        messages: [],
      });

      const file = mockFile("empty.docx");
      await expect(convertFile(file)).rejects.toThrow("Document appears to be empty");
    });

    it("throws user-friendly error on corrupt input", async () => {
      mocks.convertToHtml.mockRejectedValueOnce(new Error("Can't find end of EOCD"));

      const file = mockFile("corrupt.docx");
      await expect(convertFile(file)).rejects.toThrow("Failed to read Word document");
    });
  });

  describe("xlsx conversion", () => {
    it("converts xlsx to markdown table", async () => {
      mocks.xlsxLoad.mockResolvedValueOnce(undefined);
      mocks.eachSheet.mockImplementationOnce(
        (cb: (sheet: { name: string; eachRow: unknown }) => void) => {
          cb({
            name: "Sheet1",
            eachRow: (
              rowCb: (row: {
                eachCell: (
                  opts: { includeEmpty: boolean },
                  cellCb: (cell: { value: string | number }) => void,
                ) => void;
              }) => void,
            ) => {
              rowCb({
                eachCell: (_opts, cellCb) => {
                  cellCb({ value: "Name" });
                  cellCb({ value: "Age" });
                },
              });
              rowCb({
                eachCell: (_opts, cellCb) => {
                  cellCb({ value: "Alice" });
                  cellCb({ value: 30 });
                },
              });
            },
          });
        },
      );

      const file = mockFile("test.xlsx");
      const result = await convertFile(file);

      expect(result.format).toBe("markdown");
      expect(result.text).toContain("## Sheet: Sheet1");
      expect(result.text).toContain("Name");
      expect(result.text).toContain("Age");
      expect(result.text).toContain("Alice");
      expect(result.text).toContain("30");
    });

    it("throws on empty spreadsheet", async () => {
      mocks.xlsxLoad.mockResolvedValueOnce(undefined);
      mocks.eachSheet.mockImplementationOnce(() => {
        // No sheets iterated
      });

      const file = mockFile("empty.xlsx");
      await expect(convertFile(file)).rejects.toThrow("Spreadsheet appears to be empty");
    });
  });

  describe("unsupported format", () => {
    it("throws for unsupported extension", async () => {
      const file = mockFile("slides.pptx");
      await expect(convertFile(file)).rejects.toThrow("Unsupported conversion format: .pptx");
    });
  });

  describe("truncation", () => {
    it("truncates output exceeding MAX_CONVERTED_CHARS", async () => {
      const longText = "x".repeat(MAX_CONVERTED_CHARS + 500);
      mocks.convertToHtml.mockResolvedValueOnce({
        value: `<p>${longText}</p>`,
        messages: [],
      });
      mocks.turndown.mockReturnValueOnce(longText);

      const file = mockFile("long.docx");
      const result = await convertFile(file);

      expect(result.text.length).toBeLessThanOrEqual(
        MAX_CONVERTED_CHARS + 200, // allow for truncation suffix
      );
      expect(result.text).toContain("[Truncated:");
      expect(result.text).toContain(`showing first ${MAX_CONVERTED_CHARS}`);
    });
  });
});

import { describe, it, expect } from "vitest";
import { getFileExtension, isSpreadsheetExt } from "../file-types";

describe("paste block icon classification", () => {
  // Simulates the PromptInput rendering chain:
  // 1. file.name → getFileExtension → ext
  // 2. ext → isSpreadsheetExt → isSpreadsheet → icon branch

  it("xlsx file gets spreadsheet icon", () => {
    const ext = getFileExtension("report.xlsx");
    expect(isSpreadsheetExt(ext)).toBe(true);
  });

  it("xls file gets spreadsheet icon", () => {
    const ext = getFileExtension("legacy.xls");
    expect(isSpreadsheetExt(ext)).toBe(true);
  });

  it("csv file gets spreadsheet icon", () => {
    const ext = getFileExtension("data.csv");
    expect(isSpreadsheetExt(ext)).toBe(true);
  });

  it("XLSX (uppercase) gets spreadsheet icon", () => {
    const ext = getFileExtension("REPORT.XLSX");
    expect(isSpreadsheetExt(ext)).toBe(true);
  });

  it("txt file gets clipboard icon (not spreadsheet)", () => {
    const ext = getFileExtension("notes.txt");
    expect(isSpreadsheetExt(ext)).toBe(false);
  });

  it("docx file gets clipboard icon (not spreadsheet)", () => {
    const ext = getFileExtension("document.docx");
    expect(isSpreadsheetExt(ext)).toBe(false);
  });

  it("file without extension gets clipboard icon", () => {
    const ext = getFileExtension("README");
    expect(isSpreadsheetExt(ext)).toBe(false);
  });
});

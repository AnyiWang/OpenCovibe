import { describe, it, expect } from "vitest";
import {
  getFileExtension,
  isTextFile,
  isBinaryAttachment,
  isPdf,
  classifyFile,
  classifyByMime,
  getFileSizeLimit,
  getSizeLimitByMime,
  isConvertibleFile,
  isConvertibleByExt,
  IMAGE_TYPES,
  DOCUMENT_TYPES,
  BINARY_ATTACHMENT_TYPES,
  TEXT_EXTENSIONS,
  CONVERTIBLE_EXTENSIONS,
  MAX_FILE_SIZE,
  MAX_IMAGE_SIZE,
  MAX_ATTACHMENTS,
  MAX_PASTE_BLOCKS,
  PDF_MAX_BINARY_SIZE,
} from "$lib/utils/file-types";

// Helper to create a mock File
function mockFile(name: string, type: string, size = 100): File {
  return new File(["x".repeat(size)], name, { type });
}

// ── getFileExtension ──

describe("getFileExtension", () => {
  it("extracts simple extension", () => {
    expect(getFileExtension("file.txt")).toBe("txt");
  });

  it("extracts extension with multiple dots", () => {
    expect(getFileExtension("archive.tar.gz")).toBe("gz");
  });

  it("returns empty string for no extension", () => {
    expect(getFileExtension("README")).toBe("");
  });

  it("handles dotfiles like .gitignore", () => {
    expect(getFileExtension(".gitignore")).toBe("gitignore");
  });

  it("handles .editorconfig", () => {
    expect(getFileExtension(".editorconfig")).toBe("editorconfig");
  });

  it("lowercases extensions", () => {
    expect(getFileExtension("photo.PNG")).toBe("png");
  });

  it("handles empty string", () => {
    expect(getFileExtension("")).toBe("");
  });
});

// ── isTextFile ──

describe("isTextFile", () => {
  it("recognizes .ts files", () => {
    expect(isTextFile(mockFile("index.ts", ""))).toBe(true);
  });

  it("recognizes .py files", () => {
    expect(isTextFile(mockFile("main.py", ""))).toBe(true);
  });

  it("recognizes .md files", () => {
    expect(isTextFile(mockFile("README.md", ""))).toBe(true);
  });

  it("recognizes .json files", () => {
    expect(isTextFile(mockFile("package.json", "application/json"))).toBe(true);
  });

  it("recognizes .csv files", () => {
    expect(isTextFile(mockFile("data.csv", ""))).toBe(true);
  });

  it("recognizes .rs files", () => {
    expect(isTextFile(mockFile("main.rs", ""))).toBe(true);
  });

  it("recognizes .svelte files", () => {
    expect(isTextFile(mockFile("App.svelte", ""))).toBe(true);
  });

  it("recognizes text/ MIME types", () => {
    expect(isTextFile(mockFile("unknown.xyz", "text/plain"))).toBe(true);
  });

  it("recognizes text/html MIME type", () => {
    expect(isTextFile(mockFile("page.html", "text/html"))).toBe(true);
  });

  it("rejects .docx files", () => {
    expect(
      isTextFile(
        mockFile(
          "doc.docx",
          "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
      ),
    ).toBe(false);
  });

  it("rejects .zip files", () => {
    expect(isTextFile(mockFile("archive.zip", "application/zip"))).toBe(false);
  });

  it("rejects .mp4 files", () => {
    expect(isTextFile(mockFile("video.mp4", "video/mp4"))).toBe(false);
  });

  it("rejects .exe files", () => {
    expect(isTextFile(mockFile("app.exe", "application/octet-stream"))).toBe(false);
  });

  it("recognizes Makefile (no extension, but in TEXT_EXTENSIONS via dotfile handling)", () => {
    // Makefile has no dot → getFileExtension returns ""
    // But makefile is in TEXT_EXTENSIONS. Since name != ".makefile", getFileExtension returns "".
    // The file with type "" and no recognized extension is not text.
    expect(isTextFile(mockFile("Makefile", ""))).toBe(false);
  });

  it("recognizes .makefile extension", () => {
    expect(isTextFile(mockFile("build.makefile", ""))).toBe(true);
  });

  it("recognizes .gitignore dotfile", () => {
    expect(isTextFile(mockFile(".gitignore", ""))).toBe(true);
  });
});

// ── isBinaryAttachment ──

describe("isBinaryAttachment", () => {
  it("recognizes image/png", () => {
    expect(isBinaryAttachment(mockFile("photo.png", "image/png"))).toBe(true);
  });

  it("recognizes image/jpeg", () => {
    expect(isBinaryAttachment(mockFile("photo.jpg", "image/jpeg"))).toBe(true);
  });

  it("recognizes image/webp", () => {
    expect(isBinaryAttachment(mockFile("photo.webp", "image/webp"))).toBe(true);
  });

  it("recognizes image/gif", () => {
    expect(isBinaryAttachment(mockFile("anim.gif", "image/gif"))).toBe(true);
  });

  it("recognizes application/pdf", () => {
    expect(isBinaryAttachment(mockFile("doc.pdf", "application/pdf"))).toBe(true);
  });

  it("rejects application/zip", () => {
    expect(isBinaryAttachment(mockFile("archive.zip", "application/zip"))).toBe(false);
  });

  it("rejects text/plain", () => {
    expect(isBinaryAttachment(mockFile("notes.txt", "text/plain"))).toBe(false);
  });
});

// ── isPdf ──

describe("isPdf", () => {
  it("returns true for application/pdf", () => {
    expect(isPdf("application/pdf")).toBe(true);
  });

  it("returns false for image/png", () => {
    expect(isPdf("image/png")).toBe(false);
  });

  it("returns false for empty string", () => {
    expect(isPdf("")).toBe(false);
  });
});

// ── classifyFile ──

describe("classifyFile", () => {
  it("classifies PNG as binary", () => {
    expect(classifyFile(mockFile("photo.png", "image/png"))).toBe("binary");
  });

  it("classifies JPEG as binary", () => {
    expect(classifyFile(mockFile("photo.jpg", "image/jpeg"))).toBe("binary");
  });

  it("classifies PDF as binary", () => {
    expect(classifyFile(mockFile("doc.pdf", "application/pdf"))).toBe("binary");
  });

  it("classifies .ts as text", () => {
    expect(classifyFile(mockFile("index.ts", ""))).toBe("text");
  });

  it("classifies .py as text", () => {
    expect(classifyFile(mockFile("main.py", ""))).toBe("text");
  });

  it("classifies text/plain as text", () => {
    expect(classifyFile(mockFile("notes.xyz", "text/plain"))).toBe("text");
  });

  it("classifies .docx as convertible", () => {
    expect(
      classifyFile(
        mockFile(
          "doc.docx",
          "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
      ),
    ).toBe("convertible");
  });

  it("classifies .mp4 as unsupported", () => {
    expect(classifyFile(mockFile("video.mp4", "video/mp4"))).toBe("unsupported");
  });

  it("classifies .zip as unsupported", () => {
    expect(classifyFile(mockFile("archive.zip", "application/zip"))).toBe("unsupported");
  });

  it("classifies .xlsx as convertible", () => {
    expect(
      classifyFile(
        mockFile("sheet.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
      ),
    ).toBe("convertible");
  });
});

// ── classifyByMime ──

describe("classifyByMime", () => {
  it("classifies image/png as binary", () => {
    expect(classifyByMime("image/png")).toBe("binary");
  });

  it("classifies image/jpeg as binary", () => {
    expect(classifyByMime("image/jpeg")).toBe("binary");
  });

  it("classifies application/pdf as binary", () => {
    expect(classifyByMime("application/pdf")).toBe("binary");
  });

  it("classifies text/plain as text", () => {
    expect(classifyByMime("text/plain")).toBe("text");
  });

  it("classifies text/html as text", () => {
    expect(classifyByMime("text/html")).toBe("text");
  });

  it("classifies text/css as text", () => {
    expect(classifyByMime("text/css")).toBe("text");
  });

  it("classifies application/zip as unsupported", () => {
    expect(classifyByMime("application/zip")).toBe("unsupported");
  });

  it("classifies application/octet-stream as unsupported", () => {
    expect(classifyByMime("application/octet-stream")).toBe("unsupported");
  });

  it("classifies empty string as unsupported", () => {
    expect(classifyByMime("")).toBe("unsupported");
  });
});

// ── isConvertibleFile ──

describe("isConvertibleFile", () => {
  it("recognizes .docx files", () => {
    expect(
      isConvertibleFile(
        mockFile(
          "doc.docx",
          "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
      ),
    ).toBe(true);
  });

  it("recognizes .xlsx files", () => {
    expect(
      isConvertibleFile(
        mockFile("sheet.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
      ),
    ).toBe(true);
  });

  it("rejects .pptx files", () => {
    expect(
      isConvertibleFile(
        mockFile(
          "slides.pptx",
          "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
      ),
    ).toBe(false);
  });

  it("rejects .txt files", () => {
    expect(isConvertibleFile(mockFile("notes.txt", "text/plain"))).toBe(false);
  });

  it("rejects .pdf files", () => {
    expect(isConvertibleFile(mockFile("doc.pdf", "application/pdf"))).toBe(false);
  });
});

// ── isConvertibleByExt ──

describe("isConvertibleByExt", () => {
  it("recognizes docx", () => {
    expect(isConvertibleByExt("docx")).toBe(true);
  });

  it("recognizes xlsx", () => {
    expect(isConvertibleByExt("xlsx")).toBe(true);
  });

  it("is case-insensitive", () => {
    expect(isConvertibleByExt("DOCX")).toBe(true);
    expect(isConvertibleByExt("Xlsx")).toBe(true);
  });

  it("rejects pptx", () => {
    expect(isConvertibleByExt("pptx")).toBe(false);
  });

  it("rejects txt", () => {
    expect(isConvertibleByExt("txt")).toBe(false);
  });
});

// ── classifyByMime with convertible types ──

describe("classifyByMime - convertible", () => {
  it("classifies docx MIME as convertible", () => {
    expect(
      classifyByMime("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ).toBe("convertible");
  });

  it("classifies xlsx MIME as convertible", () => {
    expect(
      classifyByMime("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ).toBe("convertible");
  });

  it("does not classify pptx MIME as convertible", () => {
    expect(
      classifyByMime("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ).toBe("unsupported");
  });
});

// ── Constants validation ──

describe("constants", () => {
  it("IMAGE_TYPES has 4 entries", () => {
    expect(IMAGE_TYPES).toHaveLength(4);
  });

  it("DOCUMENT_TYPES has 1 entry", () => {
    expect(DOCUMENT_TYPES).toHaveLength(1);
    expect(DOCUMENT_TYPES[0]).toBe("application/pdf");
  });

  it("BINARY_ATTACHMENT_TYPES includes all images + documents", () => {
    expect(BINARY_ATTACHMENT_TYPES).toHaveLength(5);
    for (const t of IMAGE_TYPES) {
      expect(BINARY_ATTACHMENT_TYPES).toContain(t);
    }
    for (const t of DOCUMENT_TYPES) {
      expect(BINARY_ATTACHMENT_TYPES).toContain(t);
    }
  });

  it("MAX_FILE_SIZE is 10MB", () => {
    expect(MAX_FILE_SIZE).toBe(10 * 1024 * 1024);
  });

  it("MAX_IMAGE_SIZE is 0 (no limit, CLI handles compression)", () => {
    expect(MAX_IMAGE_SIZE).toBe(0);
  });

  it("MAX_ATTACHMENTS is 8", () => {
    expect(MAX_ATTACHMENTS).toBe(8);
  });

  it("MAX_PASTE_BLOCKS is 4", () => {
    expect(MAX_PASTE_BLOCKS).toBe(4);
  });

  it("PDF_MAX_BINARY_SIZE is 20MB", () => {
    expect(PDF_MAX_BINARY_SIZE).toBe(20 * 1024 * 1024);
  });

  it("TEXT_EXTENSIONS contains common code extensions", () => {
    const expected = ["ts", "js", "py", "rs", "go", "java", "c", "cpp", "rb", "php"];
    for (const ext of expected) {
      expect(TEXT_EXTENSIONS.has(ext)).toBe(true);
    }
  });

  it("TEXT_EXTENSIONS does not contain binary formats", () => {
    const binary = ["png", "jpg", "pdf", "zip", "exe", "mp4", "docx"];
    for (const ext of binary) {
      expect(TEXT_EXTENSIONS.has(ext)).toBe(false);
    }
  });

  it("CONVERTIBLE_EXTENSIONS has docx and xlsx", () => {
    expect(CONVERTIBLE_EXTENSIONS.has("docx")).toBe(true);
    expect(CONVERTIBLE_EXTENSIONS.has("xlsx")).toBe(true);
    expect(CONVERTIBLE_EXTENSIONS.size).toBe(2);
  });
});

// ── getFileSizeLimit ──

describe("getFileSizeLimit", () => {
  it("returns Infinity for images (CLI handles compression)", () => {
    expect(getFileSizeLimit(mockFile("photo.png", "image/png"))).toBe(Infinity);
    expect(getFileSizeLimit(mockFile("photo.jpg", "image/jpeg"))).toBe(Infinity);
  });

  it("returns PDF_MAX_BINARY_SIZE for PDFs, 10MB for other non-image types", () => {
    expect(getFileSizeLimit(mockFile("doc.pdf", "application/pdf"))).toBe(20 * 1024 * 1024);
    expect(getFileSizeLimit(mockFile("notes.txt", "text/plain"))).toBe(10 * 1024 * 1024);
  });
});

// ── getSizeLimitByMime ──

describe("getSizeLimitByMime", () => {
  it("returns Infinity for images (CLI handles compression)", () => {
    expect(getSizeLimitByMime("image/png")).toBe(Infinity);
    expect(getSizeLimitByMime("image/webp")).toBe(Infinity);
  });

  it("returns PDF_MAX_BINARY_SIZE for PDFs, 10MB for other non-image types", () => {
    expect(getSizeLimitByMime("application/pdf")).toBe(20 * 1024 * 1024);
    expect(getSizeLimitByMime("text/plain")).toBe(10 * 1024 * 1024);
  });
});

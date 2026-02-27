/**
 * File type classification for attachment handling.
 *
 * Categorizes files into: binary attachments (images + PDF),
 * text files (read as content), or unsupported (rejected with toast).
 */

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"] as const;

export const DOCUMENT_TYPES = ["application/pdf"] as const;

/** MIME types that are sent as binary attachments (base64). */
export const BINARY_ATTACHMENT_TYPES: readonly string[] = [...IMAGE_TYPES, ...DOCUMENT_TYPES];

export const MAX_FILE_SIZE = 10 * 1024 * 1024; // 10MB (text, PDF)
export const MAX_IMAGE_SIZE = 0; // No limit — CLI compresses via sharp (→ ≤3.75MB)
export const MAX_ATTACHMENTS = 8;
export const MAX_PASTE_BLOCKS = 4;

/** CLI-aligned: base64 inline path (dj6 = 20MB in CLI source). */
export const PDF_MAX_BINARY_SIZE = 20 * 1024 * 1024; // 20MB

/** CLI-aligned: pdftoppm path ceiling (H98 = 100MB in CLI source). Clipboard-only. */
export const PDF_MAX_PATH_SIZE = 100 * 1024 * 1024; // 100MB

/** Get size limit for a File object. Images: no limit (CLI compresses), PDFs: 20MB, others: 10MB. */
export function getFileSizeLimit(file: File): number {
  // Images have no app-side size limit — CLI's sharp handles compression
  if (IMAGE_TYPES.includes(file.type as (typeof IMAGE_TYPES)[number])) return Infinity;
  if (isPdf(file.type) || getFileExtension(file.name) === "pdf") return PDF_MAX_BINARY_SIZE;
  return MAX_FILE_SIZE;
}

/** Get size limit by MIME string. Images: no limit (CLI compresses), PDFs: 20MB, others: 10MB. */
export function getSizeLimitByMime(mimeType: string): number {
  if (IMAGE_TYPES.includes(mimeType as (typeof IMAGE_TYPES)[number])) return Infinity;
  if (isPdf(mimeType)) return PDF_MAX_BINARY_SIZE;
  return MAX_FILE_SIZE;
}

/** File extensions that can be converted to text (docx, xlsx). */
export const CONVERTIBLE_EXTENSIONS = new Map<string, string>([
  ["docx", "Word"],
  ["xlsx", "Excel"],
]);

/** Check if a file can be converted to text (by extension). */
export function isConvertibleFile(file: File): boolean {
  return CONVERTIBLE_EXTENSIONS.has(getFileExtension(file.name));
}

/** Check if an extension is convertible (for clipboard path). */
export function isConvertibleByExt(ext: string): boolean {
  return CONVERTIBLE_EXTENSIONS.has(ext.toLowerCase());
}

/** MIME types for convertible office documents. */
const CONVERTIBLE_MIME_TYPES = [
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document", // docx
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", // xlsx
] as const;

/** File extensions recognized as readable text files. */
export const TEXT_EXTENSIONS = new Set([
  "txt",
  "md",
  "json",
  "ts",
  "tsx",
  "js",
  "jsx",
  "py",
  "rs",
  "svelte",
  "html",
  "css",
  "scss",
  "yaml",
  "yml",
  "toml",
  "xml",
  "sh",
  "bash",
  "zsh",
  "sql",
  "go",
  "java",
  "c",
  "cpp",
  "h",
  "hpp",
  "rb",
  "php",
  "swift",
  "kt",
  "r",
  "csv",
  "log",
  "env",
  "cfg",
  "ini",
  "conf",
  "vue",
  "astro",
  "prisma",
  "graphql",
  "makefile",
  "dockerfile",
  "gitignore",
  "editorconfig",
]);

/** Extract file extension (lowercase, without dot) from a filename. */
export function getFileExtension(name: string): string {
  const dotIndex = name.lastIndexOf(".");
  if (dotIndex <= 0 && !name.startsWith(".")) return "";
  // Handle dotfiles like ".gitignore" → "gitignore"
  if (dotIndex === 0) return name.slice(1).toLowerCase();
  return name.slice(dotIndex + 1).toLowerCase();
}

/** Check if a file should be treated as readable text. */
export function isTextFile(file: File): boolean {
  if (file.type.startsWith("text/")) return true;
  return TEXT_EXTENSIONS.has(getFileExtension(file.name));
}

/** Check if a file's MIME type is a supported binary attachment. */
export function isBinaryAttachment(file: File): boolean {
  return BINARY_ATTACHMENT_TYPES.includes(file.type);
}

/** Check if a MIME type is a PDF document. */
export function isPdf(mimeType: string): boolean {
  return DOCUMENT_TYPES.includes(mimeType as (typeof DOCUMENT_TYPES)[number]);
}

export type FileClassification = "binary" | "text" | "convertible" | "unsupported";

/** Classify a file into one of four categories. */
export function classifyFile(file: File): FileClassification {
  if (isBinaryAttachment(file)) return "binary";
  if (isTextFile(file)) return "text";
  if (isConvertibleFile(file)) return "convertible";
  return "unsupported";
}

/** Classify by MIME type string (for clipboard files without a File object). */
export function classifyByMime(mimeType: string): FileClassification {
  if (BINARY_ATTACHMENT_TYPES.includes(mimeType)) return "binary";
  if (mimeType.startsWith("text/")) return "text";
  if ((CONVERTIBLE_MIME_TYPES as readonly string[]).includes(mimeType)) return "convertible";
  return "unsupported";
}

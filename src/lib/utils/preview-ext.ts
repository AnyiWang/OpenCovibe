export const PREVIEWABLE_EXTENSIONS: ReadonlySet<string> = new Set(["md", "markdown"]);

export const IMAGE_EXTENSIONS: ReadonlySet<string> = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "svg",
  "webp",
  "ico",
  "bmp",
  "avif",
  "tif",
  "tiff",
  "jfif",
]);

export type PreviewKind = "markdown" | "image" | "text";

export function getExtension(path: string): string {
  return path.split(".").pop()?.toLowerCase() ?? "";
}

export function isPreviewable(ext: string): boolean {
  return PREVIEWABLE_EXTENSIONS.has(ext);
}

export function isImage(ext: string): boolean {
  return IMAGE_EXTENSIONS.has(ext);
}

export function classifyPath(path: string): PreviewKind {
  const ext = getExtension(path);
  if (isImage(ext)) return "image";
  if (isPreviewable(ext)) return "markdown";
  return "text";
}

/**
 * Filter memory file candidates for sidebar display.
 *
 * - Always shows files that exist on disk.
 * - Shows non-existing (creatable) files only when `showCreate` is true,
 *   OR when the file is currently selected (to keep highlight visible).
 */
export function filterVisibleCandidates(
  files: { path: string; exists: boolean }[],
  showCreate: boolean,
  selectedPath: string,
): { path: string; exists: boolean }[] {
  return files.filter((f) => f.exists || showCreate || f.path === selectedPath);
}

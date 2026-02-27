import type { FileEntry, TimelineEntry, HookEvent } from "$lib/types";

/** File operation tool names (covers PascalCase + snake_case aliases) */
const FILE_TOOL_NAMES: Record<string, FileEntry["action"]> = {
  Read: "read",
  read_file: "read",
  Write: "write",
  write_file: "write",
  Edit: "edit",
  edit_file: "edit",
  NotebookEdit: "edit",
};

/** Extract file path from tool input (different tools use different field names) */
function extractPath(input: Record<string, unknown>): string | null {
  for (const key of ["file_path", "path", "filename", "notebook_path"]) {
    if (typeof input[key] === "string") return input[key] as string;
  }
  return null;
}

/** Extract file entries from timeline (stream-json mode) */
export function extractFilesFromTimeline(timeline: TimelineEntry[]): FileEntry[] {
  const result: FileEntry[] = [];

  function walk(entries: TimelineEntry[], isTopLevel: boolean): void {
    for (const entry of entries) {
      if (entry.kind !== "tool") continue;
      const action = FILE_TOOL_NAMES[entry.tool.tool_name];
      if (!action) {
        // Not a file tool, but recurse into subTimeline
        if (entry.subTimeline) walk(entry.subTimeline, false);
        continue;
      }
      const path = extractPath(entry.tool.input);
      if (!path) {
        if (entry.subTimeline) walk(entry.subTimeline, false);
        continue;
      }
      result.push({
        path,
        action,
        toolUseId: isTopLevel ? entry.tool.tool_use_id : undefined,
        status: entry.tool.status,
      });
      if (entry.subTimeline) walk(entry.subTimeline, false);
    }
  }

  walk(timeline, true);
  return result;
}

/** Extract file entries from hookToolEvents (pipe/PTY fallback mode) */
export function extractFilesFromHooks(hooks: HookEvent[]): FileEntry[] {
  const result: FileEntry[] = [];
  for (const hook of hooks) {
    if (!hook.tool_name) continue;
    const action = FILE_TOOL_NAMES[hook.tool_name];
    if (!action) continue;
    const input = hook.tool_input;
    if (!input || typeof input !== "object") continue;
    const path = extractPath(input as Record<string, unknown>);
    if (!path) continue;
    result.push({ path, action, status: hook.status });
  }
  return result;
}

/** Safely extract file entries from persistedFiles (unknown[]), supporting multiple shapes */
export function extractFilesFromPersisted(files: unknown[]): FileEntry[] {
  return files.flatMap((f) => {
    let raw: string | undefined;
    // shape 1: plain string → path
    if (typeof f === "string") raw = f;
    else if (typeof f === "object" && f !== null) {
      const obj = f as Record<string, unknown>;
      // shape 2-4: { filename | path | file_path }
      raw =
        typeof obj.filename === "string"
          ? obj.filename
          : typeof obj.path === "string"
            ? obj.path
            : typeof obj.file_path === "string"
              ? obj.file_path
              : undefined;
    }
    // Filter empty/whitespace paths
    if (!raw || !raw.trim()) return [];
    return [{ path: raw.trim(), action: "persisted" as const }];
  });
}

/** Lightweight path normalization: deduplicate /, strip ./ prefix */
function normalizePath(p: string): string {
  return p.replace(/\/+/g, "/").replace(/^\.\//, "");
}

/** Merge priority: write > edit > read > persisted */
const ACTION_PRIORITY: Record<FileEntry["action"], number> = {
  write: 3,
  edit: 2,
  read: 1,
  persisted: 0,
};

/**
 * Merge and deduplicate file entries from multiple sources.
 * Higher action priority wins. Same priority keeps latest occurrence.
 * Sources with hasTemporalOrder: false get _seq = -1 (sort to bottom).
 */
export function mergeFileEntries(
  ...sources: Array<{ entries: FileEntry[]; hasTemporalOrder: boolean }>
): FileEntry[] {
  const map = new Map<string, FileEntry & { _seq: number }>();
  let seq = 0;
  for (const { entries, hasTemporalOrder } of sources) {
    for (const entry of entries) {
      const key = normalizePath(entry.path);
      const entrySeq = hasTemporalOrder ? seq++ : -1;
      const existing = map.get(key);
      if (!existing) {
        map.set(key, { ...entry, _seq: entrySeq });
      } else if (ACTION_PRIORITY[entry.action] > ACTION_PRIORITY[existing.action]) {
        // Action upgrade → overwrite, don't inherit old toolUseId
        map.set(key, {
          ...entry,
          toolUseId: entry.toolUseId,
          _seq: Math.max(existing._seq, entrySeq),
        });
      } else if (ACTION_PRIORITY[entry.action] === ACTION_PRIORITY[existing.action]) {
        // Same priority → keep latest, inherit toolUseId if missing
        map.set(key, {
          ...entry,
          toolUseId: entry.toolUseId ?? existing.toolUseId,
          _seq: Math.max(existing._seq, entrySeq),
        });
      }
      // Lower priority → ignore
    }
  }
  // Sort by _seq descending (most recent first, persisted-only at bottom)
  return [...map.values()].sort((a, b) => b._seq - a._seq).map(({ _seq, ...rest }) => rest);
}

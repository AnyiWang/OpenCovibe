/**
 * Removed-cwds utilities — manage the list of project folders removed from sidebar.
 *
 * Removed cwds are stored in localStorage under "ocv:removed-cwds".
 * Legacy key "ocv:hidden-cwds" is migrated on first load.
 */

import { normalizeCwd } from "./sidebar-groups";

const STORAGE_KEY = "ocv:removed-cwds";
const LEGACY_KEY = "ocv:hidden-cwds";

/** Parse a JSON array from localStorage, returning [] on any error. */
function safeParseArray(key: string): string[] {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed) && parsed.every((v: unknown) => typeof v === "string")) {
      return parsed as string[];
    }
    return [];
  } catch {
    return [];
  }
}

/**
 * Load removed cwds from localStorage, merging legacy key.
 * Returns normalized, deduplicated, non-empty values.
 * Writes back the merged result and removes the legacy key.
 */
export function loadRemovedCwds(): string[] {
  const current = safeParseArray(STORAGE_KEY);
  const legacy = safeParseArray(LEGACY_KEY);

  const merged = [...current, ...legacy];
  const seen = new Set<string>();
  const result: string[] = [];
  for (const raw of merged) {
    const n = normalizeCwd(raw);
    if (n && !seen.has(n)) {
      seen.add(n);
      result.push(n);
    }
  }

  // Persist merged result and clean up legacy key
  if (legacy.length > 0) {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(result));
    localStorage.removeItem(LEGACY_KEY);
  }

  return result;
}

/**
 * Check if a cwd is in the removed set.
 * Empty string ("" = Uncategorized) always returns false — Uncategorized cannot be removed.
 */
export function isRemovedCwd(cwd: string, removedSet: Set<string>): boolean {
  const n = normalizeCwd(cwd);
  if (!n) return false; // "" = Uncategorized is never removed
  return removedSet.has(n);
}

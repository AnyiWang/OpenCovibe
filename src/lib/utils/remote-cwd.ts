/**
 * Persistent helpers for the remote folder picker. Centralizes the
 * `localStorage` keys and try/catch noise that would otherwise be duplicated
 * across `+layout.svelte`, `chat/+page.svelte`, and `FolderPicker.svelte`.
 *
 * `localStorage` access can throw under restricted contexts (private windows,
 * embedded webviews) — every helper swallows the failure and returns the
 * empty / null sentinel rather than propagating.
 */

const REMOTE_CWD_PREFIX = "ocv:remote-cwd:";
const LAST_TARGET_KEY = "ocv:last-target";

/** Last per-host cwd the user picked, or `""` if none stored / storage unavailable. */
export function getStoredRemoteCwd(host: string): string {
  try {
    return localStorage.getItem(`${REMOTE_CWD_PREFIX}${host}`) ?? "";
  } catch {
    return "";
  }
}

export function setStoredRemoteCwd(host: string, cwd: string): void {
  try {
    localStorage.setItem(`${REMOTE_CWD_PREFIX}${host}`, cwd);
  } catch {
    /* storage unavailable */
  }
}

/** Most recently used target host name; `null` for local or storage unavailable. */
export function getLastTarget(): string | null {
  try {
    const v = localStorage.getItem(LAST_TARGET_KEY);
    return v && v.length > 0 ? v : null;
  } catch {
    return null;
  }
}

/** Pass `null` to clear (local target). */
export function setLastTarget(host: string | null): void {
  try {
    localStorage.setItem(LAST_TARGET_KEY, host ?? "");
  } catch {
    /* storage unavailable */
  }
}

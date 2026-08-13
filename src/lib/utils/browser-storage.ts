export interface BrowserStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** Resolve localStorage without letting a restricted WebView abort application startup. */
export function getBrowserStorage(): BrowserStorage | null {
  try {
    if (typeof window !== "undefined") return window.localStorage;
    if (typeof globalThis.localStorage !== "undefined") return globalThis.localStorage;
    return null;
  } catch {
    return null;
  }
}

export function readStorageItem(
  key: string,
  storage: BrowserStorage | null = getBrowserStorage(),
): string | null {
  if (!storage) return null;
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

export function writeStorageItem(
  key: string,
  value: string,
  storage: BrowserStorage | null = getBrowserStorage(),
): boolean {
  if (!storage) return false;
  try {
    storage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

export function removeStorageItem(
  key: string,
  storage: BrowserStorage | null = getBrowserStorage(),
): boolean {
  if (!storage) return false;
  try {
    storage.removeItem(key);
    return true;
  } catch {
    return false;
  }
}

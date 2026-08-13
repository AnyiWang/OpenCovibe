export const THEME_STORAGE_KEY = "ocv:theme";
export const COLOR_SCHEME_STORAGE_KEY = "ocv:colorScheme";

export const THEME_MODES = ["dark", "light", "high-contrast", "system"] as const;
export const COLOR_SCHEMES = ["warm", "neutral"] as const;

export type ThemeMode = (typeof THEME_MODES)[number];
export type ColorScheme = (typeof COLOR_SCHEMES)[number];

export interface ThemeController {
  readonly mode: ThemeMode;
  setMode(mode: ThemeMode): void;
}

export interface ThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface ThemeMediaSource {
  matchMedia?: (query: string) => MediaQueryList;
}

export const THEME_CONTEXT = Symbol("theme");

export function parseThemeMode(value: string | null): ThemeMode {
  return THEME_MODES.includes(value as ThemeMode) ? (value as ThemeMode) : "dark";
}

export function nextThemeMode(mode: ThemeMode): ThemeMode {
  const index = THEME_MODES.indexOf(mode);
  return THEME_MODES[(index + 1) % THEME_MODES.length];
}

export function readStoredTheme(storage: ThemeStorage | null): {
  mode: ThemeMode;
  storageAvailable: boolean;
} {
  if (!storage) return { mode: "dark", storageAvailable: false };
  try {
    return { mode: parseThemeMode(storage.getItem(THEME_STORAGE_KEY)), storageAvailable: true };
  } catch {
    return { mode: "dark", storageAvailable: false };
  }
}

export function persistThemeMode(storage: ThemeStorage | null, mode: ThemeMode): boolean {
  if (!storage) return false;
  try {
    storage.setItem(THEME_STORAGE_KEY, mode);
    return true;
  } catch {
    return false;
  }
}

export function readStoredColorScheme(storage: ThemeStorage | null): {
  scheme: ColorScheme;
  storageAvailable: boolean;
} {
  if (!storage) return { scheme: "warm", storageAvailable: false };
  try {
    const value = storage.getItem(COLOR_SCHEME_STORAGE_KEY);
    return { scheme: value === "neutral" ? "neutral" : "warm", storageAvailable: true };
  } catch {
    return { scheme: "warm", storageAvailable: false };
  }
}

export function persistColorScheme(storage: ThemeStorage | null, scheme: ColorScheme): boolean {
  if (!storage) return false;
  try {
    storage.setItem(COLOR_SCHEME_STORAGE_KEY, scheme);
    return true;
  } catch {
    return false;
  }
}

export function getSystemThemeQuery(source: ThemeMediaSource | null): MediaQueryList | null {
  if (!source || typeof source.matchMedia !== "function") return null;
  try {
    return source.matchMedia("(prefers-color-scheme: dark)");
  } catch {
    return null;
  }
}

export function isDarkTheme(mode: ThemeMode, systemDark: boolean): boolean {
  return mode === "dark" || (mode === "system" && systemDark);
}

export function applyThemeClasses(
  root: Pick<HTMLElement, "classList" | "style">,
  mode: ThemeMode,
  systemDark: boolean,
): void {
  const dark = isDarkTheme(mode, systemDark);
  root.classList.toggle("dark", dark);
  root.classList.toggle("theme-high-contrast", mode === "high-contrast");
  root.style.colorScheme = dark ? "dark" : "light";
}

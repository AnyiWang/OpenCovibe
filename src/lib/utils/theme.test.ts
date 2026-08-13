import { describe, expect, it } from "vitest";
import {
  COLOR_SCHEME_STORAGE_KEY,
  THEME_MODES,
  applyThemeClasses,
  getSystemThemeQuery,
  isDarkTheme,
  nextThemeMode,
  parseThemeMode,
  persistColorScheme,
  persistThemeMode,
  readStoredColorScheme,
  readStoredTheme,
  type ThemeStorage,
  type ThemeMode,
} from "./theme";

function fakeRoot() {
  const classes = new Set<string>();
  return {
    classes,
    root: {
      classList: {
        toggle(name: string, force?: boolean) {
          if (force) classes.add(name);
          else classes.delete(name);
          return Boolean(force);
        },
      },
      style: { colorScheme: "" },
    } as unknown as HTMLElement,
  };
}

describe("theme", () => {
  it("accepts supported persisted modes and rejects stale values", () => {
    for (const mode of THEME_MODES) expect(parseThemeMode(mode)).toBe(mode);
    expect(parseThemeMode("light-high-contrast")).toBe("dark");
    expect(parseThemeMode(null)).toBe("dark");
  });

  it("cycles through every app theme", () => {
    const sequence: ThemeMode[] = [];
    let mode: ThemeMode = "dark";
    for (let i = 0; i < THEME_MODES.length; i++) {
      sequence.push(mode);
      mode = nextThemeMode(mode);
    }
    expect(sequence).toEqual(["dark", "light", "high-contrast", "system"]);
    expect(mode).toBe("dark");
  });

  it("uses the OS preference only in system mode", () => {
    expect(isDarkTheme("system", true)).toBe(true);
    expect(isDarkTheme("system", false)).toBe(false);
    expect(isDarkTheme("dark", false)).toBe(true);
    expect(isDarkTheme("high-contrast", true)).toBe(false);
  });

  it("reads the system theme query and tolerates unavailable WebView APIs", () => {
    const query = { matches: false } as MediaQueryList;
    expect(getSystemThemeQuery({ matchMedia: () => query })).toBe(query);
    expect(getSystemThemeQuery({})).toBeNull();
    expect(
      getSystemThemeQuery({
        matchMedia: () => {
          throw new Error("unavailable");
        },
      }),
    ).toBeNull();
  });

  it("applies dark and high-contrast classes without leaving stale classes", () => {
    const { root, classes } = fakeRoot();

    applyThemeClasses(root, "high-contrast", true);
    expect(classes).toEqual(new Set(["theme-high-contrast"]));
    expect(root.style.colorScheme).toBe("light");

    applyThemeClasses(root, "dark", false);
    expect(classes).toEqual(new Set(["dark"]));
    expect(root.style.colorScheme).toBe("dark");
  });

  it("applies, persists, and restores a selected app theme", () => {
    const values = new Map<string, string>();
    const storage: ThemeStorage = {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    };
    const { root, classes } = fakeRoot();

    applyThemeClasses(root, "high-contrast", false);
    expect(persistThemeMode(storage, "high-contrast")).toBe(true);
    expect(readStoredTheme(storage)).toEqual({
      mode: "high-contrast",
      storageAvailable: true,
    });
    expect(classes).toEqual(new Set(["theme-high-contrast"]));
  });

  it("keeps the in-memory theme usable when WebView storage is unavailable", () => {
    const storage: ThemeStorage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    const { root, classes } = fakeRoot();

    expect(readStoredTheme(storage)).toEqual({ mode: "dark", storageAvailable: false });
    expect(readStoredColorScheme(storage)).toEqual({
      scheme: "warm",
      storageAvailable: false,
    });
    applyThemeClasses(root, "light", false);
    expect(persistThemeMode(storage, "light")).toBe(false);
    expect(persistColorScheme(storage, "neutral")).toBe(false);
    expect(classes).toEqual(new Set());
    expect(root.style.colorScheme).toBe("light");
  });

  it("handles a WebView that denies access to the storage object itself", () => {
    expect(readStoredTheme(null)).toEqual({ mode: "dark", storageAvailable: false });
    expect(persistThemeMode(null, "light")).toBe(false);
    expect(readStoredColorScheme(null)).toEqual({
      scheme: "warm",
      storageAvailable: false,
    });
    expect(persistColorScheme(null, "neutral")).toBe(false);
  });

  it("persists and restores the UI color scheme through the guarded storage path", () => {
    const values = new Map<string, string>();
    const storage: ThemeStorage = {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    };

    expect(readStoredColorScheme(storage)).toEqual({
      scheme: "warm",
      storageAvailable: true,
    });
    expect(persistColorScheme(storage, "neutral")).toBe(true);
    expect(values.get(COLOR_SCHEME_STORAGE_KEY)).toBe("neutral");
    expect(readStoredColorScheme(storage)).toEqual({
      scheme: "neutral",
      storageAvailable: true,
    });
  });
});

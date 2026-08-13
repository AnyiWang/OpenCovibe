import { describe, expect, it } from "vitest";
import {
  readStorageItem,
  removeStorageItem,
  writeStorageItem,
  type BrowserStorage,
} from "./browser-storage";

describe("browser storage", () => {
  it("reads, writes, and removes values", () => {
    const values = new Map<string, string>();
    const storage: BrowserStorage = {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
      removeItem: (key) => values.delete(key),
    };

    expect(writeStorageItem("theme", "light", storage)).toBe(true);
    expect(readStorageItem("theme", storage)).toBe("light");
    expect(removeStorageItem("theme", storage)).toBe(true);
    expect(readStorageItem("theme", storage)).toBeNull();
  });

  it("returns safe sentinels when WebView storage operations fail", () => {
    const storage: BrowserStorage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
      removeItem: () => {
        throw new Error("blocked");
      },
    };

    expect(readStorageItem("theme", storage)).toBeNull();
    expect(writeStorageItem("theme", "light", storage)).toBe(false);
    expect(removeStorageItem("theme", storage)).toBe(false);
    expect(readStorageItem("theme", null)).toBeNull();
  });
});

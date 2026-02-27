import type en from "$messages/en.json";

/** Union of all valid message keys (derived from en.json). */
export type MessageKey = keyof typeof en;

/** Variables for interpolation: `{ variable: string }`. */
export type MessageParams = Record<string, string>;

/** Re-export Locale from registry for convenience. */
export type { Locale } from "./registry";

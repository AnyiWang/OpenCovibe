/**
 * Self-built i18n runtime for OpenCovibe.
 *
 * Supports: `{variable}` interpolation, fallback chain (locale -> en -> raw key),
 * reactive locale via Svelte 5 $state, localStorage persistence, async loading.
 */
import en from "$messages/en.json";
import zhCN from "$messages/zh-CN.json";
import { LOCALE_REGISTRY, SUPPORTED_LOCALES, BASE_LOCALE, isLocale, getEntry } from "./registry";
import type { Locale } from "./registry";
import type { MessageKey, MessageParams } from "./types";
import { dbg, dbgWarn } from "$lib/utils/debug";

// ── Re-exports from registry ────────────────────────────────────
export { LOCALE_REGISTRY, SUPPORTED_LOCALES, BASE_LOCALE, isLocale, getEntry };
export type { Locale };

// ── Backward-compat aliases ─────────────────────────────────────
export const baseLocale = BASE_LOCALE;
export const locales = SUPPORTED_LOCALES;

// ── localStorage key migration ──────────────────────────────────
const LOCAL_STORAGE_KEY = "ocv:locale";
const LEGACY_STORAGE_KEY = "PARAGLIDE_LOCALE";

// ── Message cache ───────────────────────────────────────────────
// en + zh-CN are pre-cached (zero flicker for initial locales).
// Future locales use async loaders for code splitting.
const messageCache: Record<string, Record<string, string>> = {
  en: en as Record<string, string>,
  "zh-CN": zhCN as Record<string, string>,
};

// Explicit loader map — one line per locale, deterministic bundling.
// Pre-cached locales still have loaders (loadMessages checks cache first).
// To add a language: add a loader here + registry.ts entry + messages/<code>.json
const loaders: Record<string, () => Promise<{ default: Record<string, string> }>> = {
  "zh-CN": () => import("$messages/zh-CN.json"),
};

async function loadMessages(code: string): Promise<Record<string, string>> {
  if (messageCache[code]) return messageCache[code];
  const loader = loaders[code];
  if (!loader) return messageCache[BASE_LOCALE];
  const mod = await loader();
  messageCache[code] = mod.default as Record<string, string>;
  return messageCache[code];
}

// ── Reactive state ───────────────────────────────────────────────
let _locale: string = $state(BASE_LOCALE);
let _switchGen = 0; // race-condition guard for rapid switches

// ── HTML attribute helpers ───────────────────────────────────────
function applyHtmlAttrs(locale: string): void {
  if (typeof document !== "undefined") {
    const entry = getEntry(locale);
    document.documentElement.lang = locale;
    document.documentElement.dir = entry?.dir ?? "ltr";
  }
}

function persistLocale(locale: string): void {
  if (typeof window !== "undefined") {
    localStorage.setItem(LOCAL_STORAGE_KEY, locale);
  }
}

// ── Init / Switch / Read ─────────────────────────────────────────

/**
 * Detect and set the initial locale.
 * Priority: localStorage (ocv:locale) -> legacy (PARAGLIDE_LOCALE) -> navigator.languages -> baseLocale.
 * Must be called once in root layout before any t() usage.
 */
export function initLocale(): void {
  let detected: string | null = null;

  if (typeof window !== "undefined") {
    // 1. New key
    detected = localStorage.getItem(LOCAL_STORAGE_KEY);

    // 2. Legacy key migration
    if (!detected) {
      detected = localStorage.getItem(LEGACY_STORAGE_KEY);
      if (detected && isLocale(detected)) {
        // Migrate: write new key, leave legacy (harmless)
        localStorage.setItem(LOCAL_STORAGE_KEY, detected);
        dbg("i18n", "migrated legacy locale key", { locale: detected });
      }
    }
  }

  // 3. navigator.languages
  if (!detected && typeof navigator !== "undefined" && navigator.languages) {
    for (const lang of navigator.languages) {
      if (isLocale(lang)) {
        detected = lang;
        break;
      }
      const base = lang.split("-")[0];
      if (isLocale(base)) {
        detected = base;
        break;
      }
    }
  }

  // 4. fallback
  if (!detected || !isLocale(detected)) {
    detected = BASE_LOCALE;
  }

  _locale = detected;
  applyHtmlAttrs(detected);

  // Ensure messages are loaded for detected locale
  if (!messageCache[detected] && loaders[detected]) {
    const gen = ++_switchGen;
    loadMessages(detected).then(() => {
      if (_switchGen === gen) {
        _locale = detected; // re-assign to trigger $state reactivity once cache is ready
      }
    });
  }

  dbg("i18n", "init", { locale: _locale, locales: SUPPORTED_LOCALES });
}

/**
 * Switch locale at runtime without page reload.
 * Synchronous when messages are cached; async-loads otherwise (fallback to en during load).
 */
export function switchLocale(newLocale: string): void {
  if (!isLocale(newLocale) || newLocale === _locale) return;

  dbg("i18n", "switch", { from: _locale, to: newLocale });

  if (messageCache[newLocale]) {
    // Cached — instant switch (zero flicker)
    _locale = newLocale;
    persistLocale(newLocale);
    applyHtmlAttrs(newLocale);
    return;
  }

  // Not cached — switch locale immediately (t() falls back to en), load in background
  const gen = ++_switchGen;
  _locale = newLocale;
  persistLocale(newLocale);
  applyHtmlAttrs(newLocale);
  loadMessages(newLocale).then(() => {
    if (_switchGen === gen) {
      _locale = newLocale; // re-assign triggers $state update, cache now has messages
    }
  });
}

/**
 * Reactive read of the current locale.
 * Use in templates / $derived / $effect to trigger re-renders.
 */
export function currentLocale(): string {
  return _locale;
}

// ── Translation function ─────────────────────────────────────────

/**
 * Translate a message key with optional variable interpolation.
 *
 * Fallback chain: current locale -> baseLocale (en) -> raw key.
 * Variables use `{name}` syntax: `t('greeting', { name: 'World' })`.
 */
export function t(key: MessageKey, params?: MessageParams): string {
  // 1. Try current locale
  let value = messageCache[_locale]?.[key];

  // 2. Fallback to base locale
  if (value === undefined && _locale !== BASE_LOCALE) {
    value = messageCache[BASE_LOCALE]?.[key];
  }

  // 3. Key not found at all
  if (value === undefined) {
    if (import.meta.env.DEV) {
      dbgWarn("i18n", `missing key: "${key}" (locale=${_locale})`);
    }
    return key;
  }

  // 4. Interpolate {variable} placeholders
  if (params) {
    return value.replace(/\{(\w+)\}/g, (_, name: string) => {
      return params[name] !== undefined ? params[name] : `{${name}}`;
    });
  }

  return value;
}

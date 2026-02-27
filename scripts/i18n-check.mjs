#!/usr/bin/env node
/**
 * i18n quality gate — checks all messages/<locale>.json files against en.json.
 *
 * Rules:
 *   1. Key alignment — every locale must have the same keys as en.json
 *   2. Placeholder consistency — {variable} sets must match en.json
 *   3. Empty / untranslated detection — no empty strings; value === key warns
 *
 * Exit code: 1 if any errors, 0 if only warnings.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, basename } from "node:path";

const MESSAGES_DIR = join(import.meta.dirname, "..", "messages");

// Key prefixes where value === enValue is expected (brand names, commands, etc.)
const UNTRANSLATED_ALLOWLIST_PREFIXES = [
  "common_brand",
  "auth_oauth",
  "nav_",      // Short nav labels may match across languages
  "app_name",
  "cmd_versionContent", // Technical version string
];

/** Whether a key matches the allowlist (prefix-based). */
function isAllowlisted(key) {
  return UNTRANSLATED_ALLOWLIST_PREFIXES.some((prefix) => key.startsWith(prefix));
}

/** Whether a value looks like a path, command, or technical token (not needing translation). */
function isTechnicalValue(value) {
  if (value.length <= 5) return true; // Short: brand abbrevs, codes, "v{version}"
  if (value.includes("/")) return true; // Paths
  if (/^[a-zA-Z0-9_{}.,-]+$/.test(value)) return true; // Pure ASCII identifiers / technical tokens
  return false;
}

// ── Helpers ──────────────────────────────────────────────────────

function extractPlaceholders(value) {
  const matches = value.match(/\{(\w+)\}/g);
  return matches ? new Set(matches) : new Set();
}

function setsEqual(a, b) {
  if (a.size !== b.size) return false;
  for (const item of a) {
    if (!b.has(item)) return false;
  }
  return true;
}

// ── Main ─────────────────────────────────────────────────────────

let errors = 0;
let warnings = 0;

// Load en.json as baseline
const enPath = join(MESSAGES_DIR, "en.json");
const enData = JSON.parse(readFileSync(enPath, "utf-8"));
const enKeys = new Set(Object.keys(enData));

// Find all other locale files
const localeFiles = readdirSync(MESSAGES_DIR)
  .filter((f) => f.endsWith(".json") && f !== "en.json");

if (localeFiles.length === 0) {
  console.log("No non-en locale files found. Nothing to check.");
  process.exit(0);
}

for (const file of localeFiles) {
  const locale = basename(file, ".json");
  const filePath = join(MESSAGES_DIR, file);
  let data;
  try {
    data = JSON.parse(readFileSync(filePath, "utf-8"));
  } catch (e) {
    console.error(`ERROR [${locale}] Failed to parse ${file}: ${e.message}`);
    errors++;
    continue;
  }

  const localeKeys = new Set(Object.keys(data));

  // Rule 1: Key alignment
  for (const key of enKeys) {
    if (!localeKeys.has(key)) {
      console.error(`ERROR [${locale}] Missing key: "${key}"`);
      errors++;
    }
  }
  for (const key of localeKeys) {
    if (!enKeys.has(key)) {
      console.warn(`WARN  [${locale}] Extra key (not in en.json): "${key}"`);
      warnings++;
    }
  }

  // Rule 2 + 3: Check each shared key
  for (const key of enKeys) {
    if (!localeKeys.has(key)) continue;

    const enValue = enData[key];
    const localeValue = data[key];

    // Rule 3a: Empty string
    if (localeValue === "") {
      console.error(`ERROR [${locale}] Empty value for key: "${key}"`);
      errors++;
      continue;
    }

    // Rule 3b: Value equals en value (possibly untranslated)
    if (
      localeValue === enValue &&
      !isAllowlisted(key) &&
      !isTechnicalValue(enValue)
    ) {
      console.warn(`WARN  [${locale}] Value same as en (untranslated?): "${key}"`);
      warnings++;
    }

    // Rule 2: Placeholder consistency
    const enPlaceholders = extractPlaceholders(enValue);
    const localePlaceholders = extractPlaceholders(localeValue);
    if (!setsEqual(enPlaceholders, localePlaceholders)) {
      const enList = [...enPlaceholders].join(", ");
      const localeList = [...localePlaceholders].join(", ");
      console.error(
        `ERROR [${locale}] Placeholder mismatch for "${key}": en={${enList}} ${locale}={${localeList}}`
      );
      errors++;
    }
  }
}

// Summary
console.log("");
console.log(`i18n check: ${localeFiles.length} locale(s), ${errors} error(s), ${warnings} warning(s)`);

if (errors > 0) {
  process.exit(1);
}

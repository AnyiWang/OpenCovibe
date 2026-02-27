#!/usr/bin/env node
/**
 * Scaffold a new locale for i18n.
 *
 * Usage:
 *   node scripts/i18n-add-locale.mjs ja "日本語" "日"
 *
 * This will:
 *   1. Create messages/<code>.json with all keys from en.json (values set to "")
 *   2. Print instructions for manual steps (registry.ts + index.svelte.ts)
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const MESSAGES_DIR = join(import.meta.dirname, "..", "messages");

const [code, nativeName, shortLabel] = process.argv.slice(2);

if (!code || !nativeName || !shortLabel) {
  console.error("Usage: node scripts/i18n-add-locale.mjs <code> <nativeName> <shortLabel>");
  console.error('Example: node scripts/i18n-add-locale.mjs ja "日本語" "日"');
  process.exit(1);
}

const targetPath = join(MESSAGES_DIR, `${code}.json`);
if (existsSync(targetPath)) {
  console.error(`ERROR: ${targetPath} already exists.`);
  process.exit(1);
}

// Load en.json and create empty-value copy
const enPath = join(MESSAGES_DIR, "en.json");
const enData = JSON.parse(readFileSync(enPath, "utf-8"));
const newData = {};
for (const key of Object.keys(enData)) {
  newData[key] = "";
}

writeFileSync(targetPath, JSON.stringify(newData, null, 2) + "\n", "utf-8");
console.log(`Created: messages/${code}.json (${Object.keys(newData).length} keys, all empty)`);

console.log("");
console.log("Manual steps remaining:");
console.log("");
console.log(`1. Add to src/lib/i18n/registry.ts:`);
console.log(`   { code: "${code}", nativeName: "${nativeName}", shortLabel: "${shortLabel}", dir: "ltr", status: "beta" },`);
console.log("");
console.log(`2. Add to src/lib/i18n/index.svelte.ts (loaders map):`);
console.log(`   "${code}": () => import("$messages/${code}.json"),`);
console.log("");
console.log(`3. Translate the empty values in messages/${code}.json`);
console.log(`4. Run: node scripts/i18n-check.mjs`);

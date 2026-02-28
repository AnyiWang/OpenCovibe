#!/usr/bin/env node
/**
 * Release script — bump version across all config files, commit, and tag.
 *
 * Usage:
 *   npm run release 0.2.0        # set explicit version
 *   npm run release patch         # 0.1.0 → 0.1.1
 *   npm run release minor         # 0.1.0 → 0.2.0
 *   npm run release major         # 0.1.0 → 1.0.0
 */

import { readFileSync, writeFileSync } from "fs";
import { execSync } from "child_process";

// ── Paths ────────────────────────────────────────────────────────────
const FILES = {
  pkg: "package.json",
  tauri: "src-tauri/tauri.conf.json",
  cargo: "src-tauri/Cargo.toml",
};

// ── Read current version ─────────────────────────────────────────────
const pkg = JSON.parse(readFileSync(FILES.pkg, "utf-8"));
const current = pkg.version;

// ── Resolve next version ─────────────────────────────────────────────
const arg = process.argv[2];
if (!arg) {
  console.error("Usage: npm run release <version|patch|minor|major>");
  process.exit(1);
}

function bump(version, level) {
  const [major, minor, patch] = version.split(".").map(Number);
  switch (level) {
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "major":
      return `${major + 1}.0.0`;
    default:
      return null;
  }
}

const next = bump(current, arg) ?? arg;

// Validate semver format
if (!/^\d+\.\d+\.\d+$/.test(next)) {
  console.error(`Invalid version: "${next}". Expected format: x.y.z`);
  process.exit(1);
}

if (next === current) {
  console.error(`Version is already ${current}`);
  process.exit(1);
}

console.log(`  ${current} → ${next}\n`);

// ── Update files ─────────────────────────────────────────────────────
// package.json
pkg.version = next;
writeFileSync(FILES.pkg, JSON.stringify(pkg, null, 2) + "\n");
console.log(`  ✓ ${FILES.pkg}`);

// tauri.conf.json
const tauri = JSON.parse(readFileSync(FILES.tauri, "utf-8"));
tauri.version = next;
writeFileSync(FILES.tauri, JSON.stringify(tauri, null, 2) + "\n");
console.log(`  ✓ ${FILES.tauri}`);

// Cargo.toml (replace first occurrence of version = "x.y.z")
let cargo = readFileSync(FILES.cargo, "utf-8");
cargo = cargo.replace(
  /^version = ".*"$/m,
  `version = "${next}"`,
);
writeFileSync(FILES.cargo, cargo);
console.log(`  ✓ ${FILES.cargo}`);

// ── Git commit & tag ─────────────────────────────────────────────────
const tag = `v${next}`;

execSync(`git add ${FILES.pkg} ${FILES.tauri} ${FILES.cargo}`, {
  stdio: "inherit",
});
execSync(`git commit -m "chore: release ${tag}"`, { stdio: "inherit" });
execSync(`git tag ${tag}`, { stdio: "inherit" });

console.log(`\n  ✓ Committed and tagged ${tag}`);
console.log(`\n  Next steps:`);
console.log(`    git push && git push origin ${tag}`);
console.log(
  `    cargo tauri build --target universal-apple-darwin`,
);
console.log(
  `    gh release create ${tag} src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg --title "${tag}"`,
);

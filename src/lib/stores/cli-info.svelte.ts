import * as api from "$lib/api";
import type { CliInfo, CliModelInfo, CliCommand } from "$lib/types";
import { dbg, dbgWarn } from "$lib/utils/debug";

let _info: CliInfo | null = $state(null);
let _loading = false;
let _loaded = false;

export function getCliModels(): CliModelInfo[] {
  return _info?.models ?? [];
}

export function getCliCommands(): CliCommand[] {
  return _info?.commands ?? [];
}

/** The model currently active in Claude Code (from ~/.claude/settings.json). */
export function getCliCurrentModel(): string | undefined {
  return _info?.current_model ?? undefined;
}

export function getCliInfo_cached(): CliInfo | null {
  return _info;
}

export async function loadCliInfo(force = false): Promise<CliInfo | null> {
  if (_loaded && !force) return _info;
  if (_loading) return _info; // dedupe concurrent calls
  _loading = true;
  try {
    dbg("cli-info", "loading", { force });
    _info = await api.getCliInfo(force);
    _loaded = true;
    dbg("cli-info", "loaded", { models: _info?.models.length });
  } catch (e) {
    dbgWarn("cli-info", "failed to load", e);
  } finally {
    _loading = false;
  }
  return _info;
}

// ── Codex Models ──

// Source: pricing.rs GPT-5.x / Codex entries. Only models with explicit pricing included.
// When adding new models, ensure matching pricing entry exists in pricing.rs.
const CODEX_MODELS: CliModelInfo[] = [
  { value: "gpt-5.4", displayName: "GPT-5.4", description: "Balanced" },
  { value: "gpt-5.4-mini", displayName: "GPT-5.4 Mini", description: "Compact" },
  { value: "gpt-5.4-nano", displayName: "GPT-5.4 Nano", description: "Lightweight" },
  { value: "gpt-5.3-codex", displayName: "GPT-5.3 Codex", description: "Coding optimized" },
  { value: "gpt-5.1-codex", displayName: "GPT-5.1 Codex", description: "Coding optimized" },
  { value: "gpt-5-codex-mini", displayName: "GPT-5 Codex Mini", description: "Affordable" },
];

export function getCodexModels(): CliModelInfo[] {
  return CODEX_MODELS;
}

// ── CLI Version Info ──

export interface CliVersionInfo {
  installed?: string;
  channel?: string;
  latest?: string;
  stable?: string;
}

let _versionInfo: CliVersionInfo | null = $state(null);
let _versionLoading = $state(false);

// ── Codex Version (global cache) ──
let _codexVersion: string | null = $state(null);
export function getCodexVersion(): string | null {
  return _codexVersion;
}

export function getCliVersionInfo_cached(): CliVersionInfo | null {
  return _versionInfo;
}

export function isCliVersionLoading(): boolean {
  return _versionLoading;
}

/** Update the cached installed version (e.g. after CLI self-updates during a session). */
export function updateInstalledVersion(version: string): void {
  if (!version || !_versionInfo) return;
  if (_versionInfo.installed === version) return;
  dbg("cli-info", "updateInstalledVersion", { from: _versionInfo.installed, to: version });
  _versionInfo = { ..._versionInfo, installed: version };
}

export async function loadCliVersionInfo(): Promise<void> {
  if (_versionLoading) return;
  _versionLoading = true;
  try {
    dbg("cli-info", "loadCliVersionInfo");
    const [cliCheck, codexCheck, distTags, cliConfig] = await Promise.all([
      api.checkAgentCli("claude").catch(() => null),
      api.checkAgentCli("codex").catch(() => null),
      api.getCliDistTags().catch(() => ({ latest: undefined, stable: undefined })),
      api.getCliConfig().catch((): Record<string, unknown> => ({})),
    ]);

    // Cache Codex version before Claude early return
    _codexVersion = codexCheck?.version ?? null;

    if (!cliCheck?.found) {
      _versionInfo = null;
      dbg("cli-info", "loadCliVersionInfo: CLI not found");
      return;
    }

    _versionInfo = {
      installed: cliCheck.version ?? undefined,
      channel: (cliConfig.autoUpdatesChannel as string) ?? undefined,
      latest: distTags.latest ?? undefined,
      stable: distTags.stable ?? undefined,
    };
    dbg("cli-info", "loadCliVersionInfo done", _versionInfo);
  } catch (e) {
    dbgWarn("cli-info", "loadCliVersionInfo failed", e);
  } finally {
    _versionLoading = false;
  }
}

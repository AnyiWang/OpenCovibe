import * as api from "$lib/api";
import type { CliInfo, CliModelInfo, CliCommand, CodexModelList } from "$lib/types";
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

// Pulled live from `codex app-server` (model/list) — see api.getCodexModels.
// No hardcoded list: new upstream models (e.g. gpt-5.5) appear without an app release.
let _codex: CodexModelList | null = $state(null);
let _codexLoading = false;
let _codexLoaded = false;

export function getCodexModels(): CliModelInfo[] {
  return _codex?.models ?? [];
}

/** The model marked `isDefault` in the live Codex catalog, if known. */
export function getCodexDefaultModel(): string | undefined {
  return _codex?.defaultModel ?? undefined;
}

/**
 * Resolve the model list to show for an agent, folding in third-party platform models.
 * Centralizes the `agent === "codex" ? getCodexModels() : …CLI/platform…` branch that
 * was copy-pasted across ModelSelector, SessionStatusBar, and the chat page.
 *
 * - Codex → the live Codex catalog (platform models don't apply).
 * - Other agents → platform models when present (see `merge`), else the CLI catalog.
 *   - `merge: false` (default): platform models REPLACE the CLI list when non-empty.
 *   - `merge: true`: platform models are PREPENDED to the CLI list (used where label
 *     lookups need to resolve both platform and CLI model values).
 */
export function getModelsForAgent(
  agent: string,
  opts: { platformModels?: CliModelInfo[]; merge?: boolean } = {},
): CliModelInfo[] {
  if (agent === "codex") return getCodexModels();
  const platform = opts.platformModels ?? [];
  if (opts.merge) return [...platform, ...getCliModels()];
  return platform.length > 0 ? platform : getCliModels();
}

export async function loadCodexModels(force = false): Promise<CodexModelList | null> {
  if (_codexLoaded && !force) return _codex;
  if (_codexLoading) return _codex; // dedupe concurrent calls
  _codexLoading = true;
  try {
    dbg("cli-info", "loading codex models", { force });
    _codex = await api.getCodexModels(force);
    _codexLoaded = true;
    dbg("cli-info", "loaded codex models", {
      models: _codex?.models.length,
      default: _codex?.defaultModel,
    });
  } catch (e) {
    dbgWarn("cli-info", "failed to load codex models", e);
  } finally {
    _codexLoading = false;
  }
  return _codex;
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
      channel: ((cliConfig as Record<string, unknown>).autoUpdatesChannel as string) ?? undefined,
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

/**
 * Per-agent protocol/runtime capabilities.
 * Describes what the CLI *emits* and which session-runtime operations it honors —
 * not UI feature gates (AgentFeatures) or resume logic (canResumeStructurally).
 */
export interface AgentCapabilities {
  supportsBusEvents: boolean; // CLI produces structured bus-events
  supportsSessionInit: boolean; // CLI sends session_init
  supportsPermissions: boolean; // CLI supports can_use_tool
  supportsSnapshots: boolean; // CLI supports snapshot restore
  // Add-dir applies live to the running session (Claude: `/add-dir` takes effect
  // immediately). When false, add-dir is persisted to settings and applies later
  // (Codex: writableRoots are read at thread/start, so a saved dir applies to the
  // next spawn / new thread rather than the current turn).
  supportsLiveAddDir: boolean;
}

const CLAUDE_CAPS: AgentCapabilities = {
  supportsBusEvents: true,
  supportsSessionInit: true,
  supportsPermissions: true,
  supportsSnapshots: true,
  supportsLiveAddDir: true,
};

const CODEX_CAPS: AgentCapabilities = {
  supportsBusEvents: true,
  supportsSessionInit: false,
  supportsPermissions: false,
  supportsSnapshots: false,
  supportsLiveAddDir: false,
};

// Minimal capability set — unknown agents should not be silently promoted to Claude
const MINIMAL_CAPS: AgentCapabilities = {
  supportsBusEvents: false,
  supportsSessionInit: false,
  supportsPermissions: false,
  supportsSnapshots: false,
  supportsLiveAddDir: false,
};

const CAPS_MAP: Record<string, AgentCapabilities> = {
  claude: CLAUDE_CAPS,
  codex: CODEX_CAPS,
};

export function getAgentCaps(agent: string): AgentCapabilities {
  return CAPS_MAP[agent] ?? MINIMAL_CAPS;
}

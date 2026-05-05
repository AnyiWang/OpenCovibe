/**
 * Per-agent protocol output capabilities.
 * Only describes what the CLI *emits* — not transport (ExecutionPath),
 * UI feature gates (AgentFeatures), or resume logic (canResumeStructurally).
 */
export interface AgentCapabilities {
  supportsBusEvents: boolean; // CLI produces structured bus-events
  supportsSessionInit: boolean; // CLI sends session_init
  supportsPermissions: boolean; // CLI supports can_use_tool
  supportsSnapshots: boolean; // CLI supports snapshot restore
}

const CLAUDE_CAPS: AgentCapabilities = {
  supportsBusEvents: true,
  supportsSessionInit: true,
  supportsPermissions: true,
  supportsSnapshots: true,
};

const CODEX_CAPS: AgentCapabilities = {
  supportsBusEvents: true,
  supportsSessionInit: false,
  supportsPermissions: false,
  supportsSnapshots: false,
};

// Minimal capability set — unknown agents should not be silently promoted to Claude
const MINIMAL_CAPS: AgentCapabilities = {
  supportsBusEvents: false,
  supportsSessionInit: false,
  supportsPermissions: false,
  supportsSnapshots: false,
};

const CAPS_MAP: Record<string, AgentCapabilities> = {
  claude: CLAUDE_CAPS,
  codex: CODEX_CAPS,
};

export function getAgentCaps(agent: string): AgentCapabilities {
  return CAPS_MAP[agent] ?? MINIMAL_CAPS;
}

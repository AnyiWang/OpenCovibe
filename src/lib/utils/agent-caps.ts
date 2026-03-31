export interface AgentCapabilities {
  transport: "stream-session" | "pipe-exec";
  supportsResume: boolean;
  supportsStructuredTools: boolean;
  supportsPermissions: boolean;
  supportsSlashCommands: boolean;
  supportsSessionInit: boolean;
  supportsSnapshots: boolean;
  supportsEffort: boolean;
  supportsPlanMode: boolean;
  supportsPermissionMode: boolean;
  supportsAddDir: boolean;
}

const CLAUDE_CAPS: AgentCapabilities = {
  transport: "stream-session",
  supportsResume: true,
  supportsStructuredTools: true,
  supportsPermissions: true,
  supportsSlashCommands: true,
  supportsSessionInit: true,
  supportsSnapshots: true,
  supportsEffort: true,
  supportsPlanMode: true,
  supportsPermissionMode: true,
  supportsAddDir: true,
};

const CODEX_CAPS: AgentCapabilities = {
  transport: "pipe-exec",
  supportsResume: false,
  supportsStructuredTools: false,
  supportsPermissions: false,
  supportsSlashCommands: false,
  supportsSessionInit: false,
  supportsSnapshots: false,
  supportsEffort: false,
  supportsPlanMode: false,
  supportsPermissionMode: false,
  supportsAddDir: false,
};

// Minimal capability set — unknown agents should not be silently promoted to Claude
const MINIMAL_CAPS: AgentCapabilities = { ...CODEX_CAPS };

const CAPS_MAP: Record<string, AgentCapabilities> = {
  claude: CLAUDE_CAPS,
  codex: CODEX_CAPS,
};

export function getAgentCaps(agent: string): AgentCapabilities {
  return CAPS_MAP[agent] ?? MINIMAL_CAPS;
}

export function isKnownAgent(agent: string): boolean {
  return Object.hasOwn(CAPS_MAP, agent);
}
